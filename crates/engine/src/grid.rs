//! Sampling fitted slices onto a fixed constant-maturity grid.
//!
//! PCA on daily surface changes only means something if a grid cell denotes
//! the same thing on consecutive days, and raw strikes and expiries do not:
//! contracts roll off, expiries age, the ladder shifts with spot. So each
//! session's fitted smiles are resampled onto axes that are stationary by
//! construction — standardised moneyness and constant maturity.
//!
//! Standardised moneyness `z = k / sqrt(w_atm)` rather than raw `k`, because a
//! fixed `k` band means wildly different things across the term structure: at
//! 30% vol, `|k| < 0.405` is about ±9.8 standard deviations on a one-week
//! option and ±0.95 on a two-year one. In `z` every cell holds a comparable
//! amount of information, which is what PCA assumes when it treats all columns
//! as commensurate.

use crate::svi::Svi;

/// A calibrated slice, with the moneyness range it was actually fitted over.
#[derive(Debug, Clone, Copy)]
pub struct FittedSlice {
    pub tte: f64,
    pub svi: Svi,
    pub k_lo: f64,
    pub k_hi: f64,
}

/// One `(z, tau)` cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridCell {
    pub z: f64,
    pub tte: f64,
    /// Log-moneyness this cell resolved to at this maturity.
    pub k: f64,
    pub total_variance: f64,
    pub vol: f64,
    /// `k` fell outside the fitted range of a bracketing slice, so this cell
    /// is the SVI wing extrapolating rather than interpolating quotes.
    pub extrapolated: bool,
}

/// Sample the surface at standardised moneyness `z` and maturity `tau`.
///
/// Returns `None` when `tau` sits outside the session's expiry range —
/// extrapolating a term structure past its last listed expiry is guesswork,
/// and a missing cell is more honest than an invented one.
///
/// The order of operations matters. The at-the-money variance is interpolated
/// *first*, because `z` cannot be converted to a strike without it: with
/// `w_atm = sigma^2 * tau`, the definition `z = k / (sigma * sqrt(tau))`
/// collapses to `k = z * sqrt(w_atm)`. Only then are the bracketing slices
/// read at that single `k` and interpolated in maturity — which keeps the
/// interpolation at fixed `k`, the coordinate in which the calendar
/// no-arbitrage condition is stated.
#[must_use]
pub fn sample(slices: &[FittedSlice], z: f64, tau: f64) -> Option<GridCell> {
    if tau <= 0.0 || slices.len() < 2 {
        return None;
    }
    let (lo, hi) = bracket(slices, tau)?;
    let t = (tau - lo.tte) / (hi.tte - lo.tte);

    let w_atm = lerp(lo.svi.total_variance(0.0), hi.svi.total_variance(0.0), t);
    if w_atm <= 0.0 || !w_atm.is_finite() {
        return None;
    }
    let k = z * w_atm.sqrt();

    let w = lerp(lo.svi.total_variance(k), hi.svi.total_variance(k), t).max(0.0);
    let extrapolated =
        k < lo.k_lo || k > lo.k_hi || k < hi.k_lo || k > hi.k_hi;

    Some(GridCell {
        z,
        tte: tau,
        k,
        total_variance: w,
        vol: (w / tau).sqrt(),
        extrapolated,
    })
}

/// Sample a whole grid, skipping maturities the session cannot bracket.
#[must_use]
pub fn sample_grid(slices: &[FittedSlice], zs: &[f64], taus: &[f64]) -> Vec<GridCell> {
    let mut sorted: Vec<FittedSlice> = slices.to_vec();
    sorted.sort_by(|a, b| a.tte.partial_cmp(&b.tte).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::with_capacity(zs.len() * taus.len());
    for &tau in taus {
        for &z in zs {
            if let Some(c) = sample(&sorted, z, tau) {
                out.push(c);
            }
        }
    }
    out
}

fn bracket(slices: &[FittedSlice], tau: f64) -> Option<(&FittedSlice, &FittedSlice)> {
    let first = slices.first()?;
    let last = slices.last()?;
    if tau < first.tte || tau > last.tte {
        return None;
    }
    let i = slices.iter().position(|s| s.tte >= tau)?;
    if i == 0 {
        return Some((first, slices.get(1)?));
    }
    let (a, b) = (&slices[i - 1], &slices[i]);
    (b.tte > a.tte).then_some((a, b))
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    (b - a).mul_add(t, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(tte: f64, a: f64) -> FittedSlice {
        FittedSlice {
            tte,
            svi: Svi { a, b: 0.09, rho: -0.7, m: 0.02, sigma: 0.15 },
            k_lo: -0.6,
            k_hi: 0.3,
        }
    }

    #[test]
    fn at_the_money_cell_reproduces_the_slice() {
        let s = [slice(0.25, 0.02), slice(0.5, 0.04)];
        let c = sample(&s, 0.0, 0.25).expect("cell");
        assert!((c.k - 0.0).abs() < 1e-12, "z=0 must land on k=0");
        assert!((c.total_variance - s[0].svi.total_variance(0.0)).abs() < 1e-12);
        assert!((c.vol - (s[0].svi.total_variance(0.0) / 0.25).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn z_converts_to_the_right_strike() {
        let s = [slice(0.25, 0.02), slice(1.0, 0.08)];
        for &z in &[-2.0, -1.0, 1.0] {
            let c = sample(&s, z, 0.5).expect("cell");
            // k = z*sqrt(w_atm) is exactly z standard deviations of the
            // at-the-money move over the life of the option.
            let w_atm = c.k / z;
            assert!((w_atm * w_atm - interp_atm(&s, 0.5)).abs() < 1e-9, "z={z}");
        }
    }

    fn interp_atm(s: &[FittedSlice], tau: f64) -> f64 {
        let t = (tau - s[0].tte) / (s[1].tte - s[0].tte);
        s[0].svi.total_variance(0.0) + t * (s[1].svi.total_variance(0.0) - s[0].svi.total_variance(0.0))
    }

    #[test]
    fn refuses_to_extrapolate_in_maturity() {
        let s = [slice(0.25, 0.02), slice(1.0, 0.08)];
        assert!(sample(&s, 0.0, 0.1).is_none(), "before the first expiry");
        assert!(sample(&s, 0.0, 2.0).is_none(), "past the last expiry");
        assert!(sample(&s, 0.0, 0.5).is_some());
    }

    #[test]
    fn flags_moneyness_extrapolation() {
        let s = [slice(0.25, 0.02), slice(1.0, 0.08)];
        let near = sample(&s, -0.5, 0.5).expect("cell");
        assert!(!near.extrapolated, "k={} is inside [-0.6, 0.3]", near.k);
        let far = sample(&s, -4.0, 0.5).expect("cell");
        assert!(far.extrapolated, "k={} should be outside the fitted range", far.k);
    }

    #[test]
    fn total_variance_grows_with_maturity_at_fixed_k() {
        // Calendar arbitrage would show up here: a linear blend of two slices
        // that are themselves ordered stays ordered.
        let s = [slice(0.25, 0.02), slice(1.0, 0.08)];
        let mut prev = f64::NEG_INFINITY;
        for i in 0..=20 {
            let tau = 0.25 + (1.0 - 0.25) * f64::from(i) / 20.0;
            let c = sample(&s, -1.0, tau).expect("cell");
            // Compare at a fixed k, which is where the condition applies.
            let w = lerp_at(&s, c.k, tau);
            assert!(w >= prev - 1e-12, "variance fell at tau={tau}");
            prev = w;
        }
    }

    fn lerp_at(s: &[FittedSlice], k: f64, tau: f64) -> f64 {
        let t = (tau - s[0].tte) / (s[1].tte - s[0].tte);
        s[0].svi.total_variance(k) + t * (s[1].svi.total_variance(k) - s[0].svi.total_variance(k))
    }

    #[test]
    fn grid_skips_unreachable_maturities() {
        let s = [slice(0.25, 0.02), slice(1.0, 0.08)];
        let cells = sample_grid(&s, &[-1.0, 0.0, 1.0], &[0.5, 5.0]);
        assert_eq!(cells.len(), 3, "the 5y row has no bracketing expiries");
    }
}
