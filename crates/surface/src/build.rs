//! Turning a session's chain into forwards and an implied-vol point cloud.
//!
//! Pure over the loaded quotes — no I/O — so the whole pipeline is testable
//! without a parquet file.

use chrono::NaiveDate;
use ptf_engine::forward::{DEFAULT_BAND, DiscountCurve, ParityPair, fit_curve, forward_at};
use ptf_engine::grid::{FittedSlice, sample_grid};
use ptf_engine::svi::{MIN_POINTS, SlicePoint, Svi, calibrate};
use ptf_engine::vol::{OptionRight, VolError, implied_vol, vega};

use crate::quotes::Quote;

/// One row of `option_forwards`.
#[derive(Debug, Clone)]
pub struct ForwardRow {
    pub quote_date: NaiveDate,
    pub root: String,
    pub expiry: NaiveDate,
    pub tte: f64,
    pub forward: f64,
    pub discount: f64,
    pub pairs_used: usize,
    pub rmse: f64,
    /// The session-wide rate this expiry's discount came from.
    pub curve_rate: f64,
    pub curve_expiries: usize,
    /// The session's rate fit was negative and floored at zero.
    pub curve_clamped: bool,
}

/// One row of `option_iv`.
#[derive(Debug, Clone)]
pub struct IvRow {
    pub quote_date: NaiveDate,
    pub root: String,
    pub expiry: NaiveDate,
    pub opt_right: OptionRight,
    pub strike: f64,
    pub mid: f64,
    pub tte: f64,
    pub log_moneyness: f64,
    pub iv: f64,
    pub vega: f64,
    pub forward: f64,
    pub rel_spread: f64,
    pub size: f64,
    pub stale: bool,
}

/// Why a quote produced no volatility. Counted rather than discarded silently,
/// because the mix is the health check: a session that suddenly rejects on
/// something other than moneyness has a data problem, not a market one.
#[derive(Debug, Default, Clone, Copy)]
pub struct Rejects {
    pub in_the_money: usize,
    pub unstable: usize,
    pub below_intrinsic: usize,
    pub above_ceiling: usize,
    pub other: usize,
    pub no_forward: usize,
}

/// One row of `vol_surface_params`.
#[derive(Debug, Clone)]
pub struct SviRow {
    pub quote_date: NaiveDate,
    pub root: String,
    pub expiry: NaiveDate,
    pub tte: f64,
    pub params: Svi,
    pub rmse_vol: f64,
    pub points: usize,
    pub min_durrleman: f64,
    pub k_lo: f64,
    pub k_hi: f64,
}

/// Standardised-moneyness axis.
///
/// Deliberately asymmetric, because the chain is. Measured over 3472 fitted
/// slices, only 7.7% span +/-2 symmetrically while 68.9% reach -2 on the
/// downside: the ingest's moneyness prefilter is a *strike ratio*, so in `z`
/// terms the call wing truncates as maturity grows. A symmetric grid would be
/// mostly SVI extrapolation on the upside. It also happens to match where the
/// risk is on an equity surface.
pub const GRID_Z: [f64; 6] = [-2.0, -1.5, -1.0, -0.5, 0.0, 0.5];

/// Constant maturities, in years. All four are bracketed by 100% of sessions;
/// 18 months drops to 90% and two years to 74%, so they are left out rather
/// than punching holes in the panel the PCA consumes.
pub const GRID_TAU: [f64; 4] = [1.0 / 12.0, 0.25, 0.5, 1.0];

/// One row of `vol_grid`.
#[derive(Debug, Clone)]
pub struct GridRow {
    pub quote_date: NaiveDate,
    pub root: String,
    pub z: f64,
    pub tte: f64,
    pub k: f64,
    pub total_variance: f64,
    pub vol: f64,
    pub extrapolated: bool,
}

pub struct SessionOutput {
    pub forwards: Vec<ForwardRow>,
    pub ivs: Vec<IvRow>,
    pub svis: Vec<SviRow>,
    pub grid: Vec<GridRow>,
    pub rejects: Rejects,
    pub curve: Option<DiscountCurve>,
}

/// Pair calls with puts at matching strikes, weighted by the *worse* of the two
/// relative spreads: a parity pair is only as trustworthy as its weaker leg.
#[must_use]
pub fn parity_pairs(quotes: &[Quote]) -> Vec<ParityPair> {
    use std::collections::BTreeMap;
    // Milli-dollar integer key: strikes are quoted in tenths, and this pairs
    // them without float-equality games.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let key = |s: f64| (s * 1000.0).round() as u64;

    let mut calls: BTreeMap<u64, &Quote> = BTreeMap::new();
    let mut puts: BTreeMap<u64, &Quote> = BTreeMap::new();
    for q in quotes {
        match q.right {
            OptionRight::Call => calls.insert(key(q.strike), q),
            OptionRight::Put => puts.insert(key(q.strike), q),
        };
    }
    calls
        .iter()
        .filter_map(|(k, c)| {
            puts.get(k).map(|p| {
                let worst = c.rel_spread.max(p.rel_spread);
                let w = if worst.is_finite() { 1.0 / (1.0 + worst.max(0.0)) } else { 0.1 };
                ParityPair::weighted(c.strike, c.mid, p.mid, w)
            })
        })
        .collect()
}

/// Build one `(quote_date, root)` session.
///
/// The discount curve is fitted once for the whole session rather than per
/// expiry — at three weeks the slope carries too little signal to determine a
/// discount factor on its own, and seeding from every expiry (including the
/// ones whose raw estimate exceeds 1.0) is what keeps the rate stable.
pub fn build_session(
    quote_date: NaiveDate,
    root: &str,
    slices: &std::collections::BTreeMap<NaiveDate, (f64, Vec<Quote>)>,
) -> SessionOutput {
    let mut rejects = Rejects::default();
    let pairs_by_expiry: Vec<(NaiveDate, f64, Vec<ParityPair>)> = slices
        .iter()
        .map(|(e, (tte, qs))| (*e, *tte, parity_pairs(qs)))
        .collect();

    let Ok(curve) = fit_curve(
        pairs_by_expiry.iter().map(|(_, tte, p)| (*tte, p.as_slice())),
        DEFAULT_BAND,
    ) else {
        rejects.no_forward += slices.values().map(|(_, q)| q.len()).sum::<usize>();
        return SessionOutput {
            forwards: vec![],
            ivs: vec![],
            svis: vec![],
            grid: vec![],
            rejects,
            curve: None,
        };
    };

    let mut forwards = Vec::new();
    let mut ivs = Vec::new();
    for (expiry, tte, pairs) in &pairs_by_expiry {
        let df = curve.df(*tte);
        let Ok(fwd) = forward_at(pairs, df, DEFAULT_BAND) else {
            rejects.no_forward += slices[expiry].1.len();
            continue;
        };
        let f = fwd.forward;
        forwards.push(ForwardRow {
            quote_date,
            root: root.to_string(),
            expiry: *expiry,
            tte: *tte,
            forward: f,
            discount: df,
            pairs_used: fwd.pairs_used,
            rmse: fwd.rmse,
            curve_rate: curve.rate,
            curve_expiries: curve.expiries_used,
            curve_clamped: curve.clamped,
        });

        for q in &slices[expiry].1 {
            // Out-of-the-money only. An in-the-money premium is nearly all
            // intrinsic, so it carries almost no volatility information and
            // inverts badly -- the kernel would flag most of them Unstable
            // anyway, but skipping them up front is cheaper and clearer.
            let otm = match q.right {
                OptionRight::Call => q.strike >= f,
                OptionRight::Put => q.strike < f,
            };
            if !otm {
                rejects.in_the_money += 1;
                continue;
            }
            match implied_vol(q.right, q.mid, f, q.strike, *tte, df) {
                Ok(iv) => ivs.push(IvRow {
                    quote_date,
                    root: root.to_string(),
                    expiry: *expiry,
                    opt_right: q.right,
                    strike: q.strike,
                    mid: q.mid,
                    tte: *tte,
                    log_moneyness: (q.strike / f).ln(),
                    iv,
                    vega: vega(f, q.strike, *tte, iv, df),
                    forward: f,
                    rel_spread: q.rel_spread,
                    size: q.size,
                    stale: q.stale,
                }),
                Err(VolError::Unstable { .. }) => rejects.unstable += 1,
                Err(VolError::BelowIntrinsic { .. }) => rejects.below_intrinsic += 1,
                Err(VolError::AboveCeiling { .. }) => rejects.above_ceiling += 1,
                Err(_) => rejects.other += 1,
            }
        }
    }
    let svis = fit_slices(&ivs);
    let slices: Vec<FittedSlice> = svis
        .iter()
        .map(|s| FittedSlice { tte: s.tte, svi: s.params, k_lo: s.k_lo, k_hi: s.k_hi })
        .collect();
    let grid = sample_grid(&slices, &GRID_Z, &GRID_TAU)
        .into_iter()
        .map(|c| GridRow {
            quote_date,
            root: root.to_string(),
            z: c.z,
            tte: c.tte,
            k: c.k,
            total_variance: c.total_variance,
            vol: c.vol,
            extrapolated: c.extrapolated,
        })
        .collect();
    SessionOutput { forwards, ivs, svis, grid, rejects, curve: Some(curve) }
}

/// Weight one observation by how precisely its quote pins the volatility.
///
/// A mid is only known to about half the bid-ask width, and that price
/// uncertainty maps to vol through vega: `d(sigma) ~ dPrice / vega`. Fitting in
/// total variance, `w = sigma^2 * T`, so `dw = 2*sigma*T*d(sigma)`. The weight
/// is the inverse variance of that. It is why wing quotes -- wide spreads over
/// small vega -- barely move the fit, without needing an arbitrary cutoff.
fn point_weight(iv: &IvRow) -> f64 {
    if iv.vega <= 0.0 || !iv.vega.is_finite() || !iv.rel_spread.is_finite()
        || iv.rel_spread <= 0.0
    {
        return 0.0;
    }
    let price_sd = 0.5 * iv.rel_spread * iv.mid;
    let vol_sd = price_sd / iv.vega;
    let w_sd = 2.0 * iv.iv * iv.tte * vol_sd;
    if w_sd > 0.0 && w_sd.is_finite() { 1.0 / (w_sd * w_sd) } else { 0.0 }
}

/// Calibrate one SVI slice per expiry from the session's point cloud.
fn fit_slices(ivs: &[IvRow]) -> Vec<SviRow> {
    use std::collections::BTreeMap;
    let mut by_expiry: BTreeMap<NaiveDate, Vec<&IvRow>> = BTreeMap::new();
    for r in ivs {
        by_expiry.entry(r.expiry).or_default().push(r);
    }
    let mut out = Vec::new();
    for (expiry, rows) in by_expiry {
        if rows.len() < MIN_POINTS {
            continue;
        }
        let tte = rows[0].tte;
        let points: Vec<SlicePoint> = rows
            .iter()
            .map(|r| SlicePoint { k: r.log_moneyness, w: r.iv * r.iv * tte, weight: point_weight(r) })
            .collect();
        let Ok(fit) = calibrate(&points, tte) else { continue };
        let (k_lo, k_hi) = rows
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), r| (l.min(r.log_moneyness), h.max(r.log_moneyness)));
        out.push(SviRow {
            quote_date: rows[0].quote_date,
            root: rows[0].root.clone(),
            expiry,
            tte,
            params: fit.params,
            rmse_vol: fit.rmse_vol,
            points: fit.points,
            min_durrleman: fit.min_durrleman,
            k_lo,
            k_hi,
        });
    }
    out
}
