//! Forward and discount factor recovered from an option chain by put-call
//! parity — no external rate curve, no dividend estimate.
//!
//! For one `(quote_date, root, expiry)` slice, parity says
//!
//! ```text
//! C(K) - P(K) = DF*F - DF*K
//! ```
//!
//! which is linear in the strike with slope `-DF` and intercept `DF*F`. A
//! regression across paired strikes therefore yields both quantities at once,
//! and yields the ones the market is actually transacting on: whatever the
//! chain implies about financing, borrow and dividends is already inside them.
//! Sourcing a rate curve separately would be more work and less correct.
//!
//! Only near-the-money pairs carry the signal. Wing quotes are wide and their
//! `C-P` is dominated by spread noise, so they are excluded by a moneyness
//! band rather than allowed to drag the fit.

use crate::vol::{OptionRight, price};

/// A strike quoted on both sides.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParityPair {
    pub strike: f64,
    pub call_mid: f64,
    pub put_mid: f64,
    /// Relative confidence in this pair — inverse spread, quote size, or 1.0.
    pub weight: f64,
}

impl ParityPair {
    #[must_use]
    pub fn new(strike: f64, call_mid: f64, put_mid: f64) -> Self {
        Self { strike, call_mid, put_mid, weight: 1.0 }
    }

    #[must_use]
    pub fn weighted(strike: f64, call_mid: f64, put_mid: f64, weight: f64) -> Self {
        Self { strike, call_mid, put_mid, weight }
    }
}

/// Result of the parity regression, with the diagnostics needed to decide
/// whether to trust it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpliedForward {
    pub forward: f64,
    pub discount: f64,
    /// Pairs inside the moneyness band that the fit actually used.
    pub pairs_used: usize,
    /// Root-mean-square residual of `C-P` against the fitted line, in price
    /// units. A slice whose rmse is a large fraction of the typical spread is
    /// telling you the quotes are stale or the chain is mislabelled.
    pub rmse: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ForwardError {
    TooFewPairs { got: usize, need: usize },
    /// Every usable pair sat at one strike, so the slope is undetermined.
    Degenerate,
    /// Implied discount factor outside `(0, 1]` — parity cannot hold.
    DiscountOutOfRange(f64),
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooFewPairs { got, need } => {
                write!(f, "{got} paired strikes, need at least {need}")
            }
            Self::Degenerate => write!(f, "paired strikes are collinear in K"),
            Self::DiscountOutOfRange(d) => {
                write!(f, "implied discount factor {d} outside (0, 1]")
            }
        }
    }
}

impl std::error::Error for ForwardError {}

/// Fewest pairs worth regressing. Two would determine a line exactly and
/// report a flawless fit however bad the quotes were; four leaves enough
/// freedom for the residual to mean something.
pub const MIN_PAIRS: usize = 4;

/// Default half-width of the moneyness band, in log-strike.
pub const DEFAULT_BAND: f64 = 0.15;

/// Recover `(F, DF)` from paired quotes.
///
/// Runs the fit twice: the band has to be centred on the forward, which is
/// what is being solved for, so the first pass centres on the strike where
/// `|C-P|` is smallest — the empirical at-the-money crossover, where `K ~ F` —
/// and the second re-centres on the fitted forward. That makes the result
/// independent of how the seed happened to land.
pub fn implied_forward(pairs: &[ParityPair], band: f64) -> Result<ImpliedForward, ForwardError> {
    if pairs.len() < MIN_PAIRS {
        return Err(ForwardError::TooFewPairs { got: pairs.len(), need: MIN_PAIRS });
    }

    let mut centre = pairs
        .iter()
        .min_by(|a, b| {
            (a.call_mid - a.put_mid)
                .abs()
                .partial_cmp(&(b.call_mid - b.put_mid).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map_or(f64::NAN, |p| p.strike);

    let mut fit = fit_band(pairs, centre, band)?;
    centre = fit.forward;
    if let Ok(second) = fit_band(pairs, centre, band) {
        fit = second;
    }
    Ok(fit)
}

fn fit_band(pairs: &[ParityPair], centre: f64, band: f64) -> Result<ImpliedForward, ForwardError> {
    let inside: Vec<&ParityPair> = pairs
        .iter()
        .filter(|p| p.strike > 0.0 && p.weight > 0.0 && (p.strike / centre).ln().abs() <= band)
        .collect();
    // Falling back to the whole chain beats refusing: a thin expiry with a
    // wide ladder can have too few strikes inside the band while still
    // pinning the line down perfectly well.
    let used: Vec<&ParityPair> =
        if inside.len() >= MIN_PAIRS { inside } else { pairs.iter().collect() };
    if used.len() < MIN_PAIRS {
        return Err(ForwardError::TooFewPairs { got: used.len(), need: MIN_PAIRS });
    }

    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for p in &used {
        let (w, x, y) = (p.weight, p.strike, p.call_mid - p.put_mid);
        sw += w;
        sx += w * x;
        sy += w * y;
        sxx += w * x * x;
        sxy += w * x * y;
    }
    let den = sw * sxx - sx * sx;
    if den.abs() < f64::EPSILON * sw.max(1.0) * sxx.max(1.0) {
        return Err(ForwardError::Degenerate);
    }
    let slope = (sw * sxy - sx * sy) / den;
    let intercept = (sy - slope * sx) / sw;

    let discount = -slope;
    if !(discount > 0.0 && discount <= 1.0) {
        return Err(ForwardError::DiscountOutOfRange(discount));
    }
    let forward = intercept / discount;

    let sse: f64 = used
        .iter()
        .map(|p| {
            let resid = (p.call_mid - p.put_mid) - (intercept + slope * p.strike);
            p.weight * resid * resid
        })
        .sum();

    Ok(ImpliedForward {
        forward,
        discount,
        pairs_used: used.len(),
        rmse: (sse / sw).sqrt(),
    })
}

/// A flat continuously-compounded rate fitted across a whole chain.
///
/// Per-expiry discount factors are hopeless at the short end: at three weeks
/// the true `DF` is about 0.9998, so the slope carries ~2e-4 of signal against
/// a quote residual of a few tens of cents. On a real SOXX chain that produced
/// `DF > 1` on three of eighteen expiries — arithmetically impossible, and a
/// per-expiry rate scattering from -4% to +4% with no term structure.
///
/// Fitting one rate across every expiry fixes it without any extra data. The
/// long end determines the rate well, and short expiries then inherit a
/// sensible discount factor instead of an unusable one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiscountCurve {
    /// Continuously compounded, per year.
    pub rate: f64,
    pub expiries_used: usize,
    /// RMS of `-ln(DF_i) - r*T_i` across the fitted expiries.
    pub rms_residual: f64,
}

impl DiscountCurve {
    /// Discount factor at `tte` years.
    #[must_use]
    pub fn df(&self, tte: f64) -> f64 {
        (-self.rate * tte).exp()
    }

    /// Fit `-ln(DF_i) = r*T_i` through the origin over per-expiry estimates.
    ///
    /// Least squares through the origin is exactly the right weighting here and
    /// needs no hand-tuned weights. The regression slope carries roughly
    /// constant absolute error whatever the tenor, so `-ln(DF)` does too, and
    /// `r = sum(T*y)/sum(T*T)` therefore weights each expiry by its maturity —
    /// which is precisely how much information about the rate it holds. Short
    /// expiries contribute almost nothing and their noise averages out rather
    /// than dominating, including estimates above 1.0 that arrive as a negative
    /// `y`.
    pub fn fit(observations: &[(f64, f64)]) -> Result<Self, ForwardError> {
        let (mut sty, mut stt, mut n) = (0.0, 0.0, 0usize);
        for &(tte, df) in observations {
            if !(tte > 0.0 && df > 0.0 && df.is_finite()) {
                continue;
            }
            let y = -df.ln();
            sty += tte * y;
            stt += tte * tte;
            n += 1;
        }
        if n < 2 {
            return Err(ForwardError::TooFewPairs { got: n, need: 2 });
        }
        if stt <= 0.0 {
            return Err(ForwardError::Degenerate);
        }
        let rate = sty / stt;

        let (mut sse, mut count) = (0.0, 0.0);
        for &(tte, df) in observations {
            if !(tte > 0.0 && df > 0.0 && df.is_finite()) {
                continue;
            }
            let resid = -df.ln() - rate * tte;
            sse += resid * resid;
            count += 1.0;
        }
        Ok(Self { rate, expiries_used: n, rms_residual: (sse / count).sqrt() })
    }
}

/// Solve one expiry for its forward with the discount factor already known.
///
/// With `DF` supplied by the curve, parity rearranges to
/// `F = (C - P)/DF + K` at every strike, so the forward is a weighted mean
/// rather than a regression — no slope to estimate, and nothing left to go
/// unstable at short tenors.
pub fn forward_at(
    pairs: &[ParityPair],
    df: f64,
    band: f64,
) -> Result<ImpliedForward, ForwardError> {
    if !(df > 0.0 && df <= 1.0) {
        return Err(ForwardError::DiscountOutOfRange(df));
    }
    if pairs.len() < MIN_PAIRS {
        return Err(ForwardError::TooFewPairs { got: pairs.len(), need: MIN_PAIRS });
    }
    let centre = pairs
        .iter()
        .min_by(|a, b| {
            (a.call_mid - a.put_mid)
                .abs()
                .partial_cmp(&(b.call_mid - b.put_mid).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map_or(f64::NAN, |p| p.strike);

    let solve = |centre: f64| -> Option<(f64, f64, usize)> {
        let inside: Vec<&ParityPair> = pairs
            .iter()
            .filter(|p| {
                p.strike > 0.0 && p.weight > 0.0 && (p.strike / centre).ln().abs() <= band
            })
            .collect();
        let used: Vec<&ParityPair> =
            if inside.len() >= MIN_PAIRS { inside } else { pairs.iter().collect() };
        if used.len() < MIN_PAIRS {
            return None;
        }
        let (mut sw, mut sf) = (0.0, 0.0);
        for p in &used {
            sw += p.weight;
            sf += p.weight * ((p.call_mid - p.put_mid) / df + p.strike);
        }
        let forward = sf / sw;
        let sse: f64 = used
            .iter()
            .map(|p| {
                let resid = (p.call_mid - p.put_mid) - df * (forward - p.strike);
                p.weight * resid * resid
            })
            .sum();
        Some((forward, (sse / sw).sqrt(), used.len()))
    };

    let (forward, _, _) =
        solve(centre).ok_or(ForwardError::TooFewPairs { got: pairs.len(), need: MIN_PAIRS })?;
    let (forward, rmse, pairs_used) = solve(forward)
        .ok_or(ForwardError::TooFewPairs { got: pairs.len(), need: MIN_PAIRS })?;
    Ok(ImpliedForward { forward, discount: df, pairs_used, rmse })
}

/// Build a synthetic pair from a model, for tests and for checking a fit.
#[must_use]
pub fn parity_pair_from_model(
    strike: f64,
    forward: f64,
    tte: f64,
    vol: f64,
    df: f64,
) -> ParityPair {
    ParityPair::new(
        strike,
        price(OptionRight::Call, forward, strike, tte, vol, df),
        price(OptionRight::Put, forward, strike, tte, vol, df),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real SOXX pairs, 2026-08-20, expiry 2026-09-11.
    const REAL: &[(f64, f64, f64)] = &[
    (467.5, 59.2500, 5.2000),
    (475.0, 53.4000, 6.3000),
    (485.0, 45.7000, 8.3500),
    (495.0, 38.6500, 10.8000),
    (502.5, 32.9000, 13.1000),
    (510.0, 28.2000, 15.9500),
    (517.5, 23.9500, 18.9500),
    (525.0, 19.9000, 22.5000),
    (535.0, 15.2500, 27.8500),
    (542.5, 12.4000, 32.8000),
    (550.0, 9.8500, 37.0500),
    (557.5, 7.8500, 43.3000),
    (565.0, 6.2500, 48.5500),
    (572.5, 5.0000, 55.4500),
    (585.0, 3.3750, 65.6000),
    (605.0, 1.8250, 84.1000),
    ];

    #[test]
    fn recovers_the_model_exactly() {
        // Parity is an identity of Black-76, so whatever smile generates the
        // quotes, the regression must return the F and DF that made them.
        let (f, df, t) = (522.38, 0.997_877, 0.060_233);
        let pairs: Vec<ParityPair> = (440..=600)
            .step_by(5)
            .map(|k| {
                let k = f64::from(k);
                // A skewed smile, so the test cannot pass by accident on a
                // flat-vol chain.
                let vol = 0.42 - 0.5 * (k / f).ln();
                parity_pair_from_model(k, f, t, vol, df)
            })
            .collect();
        let got = implied_forward(&pairs, DEFAULT_BAND).expect("fit");
        assert!((got.forward - f).abs() < 1e-8, "F {} vs {f}", got.forward);
        assert!((got.discount - df).abs() < 1e-12, "DF {} vs {df}", got.discount);
        assert!(got.rmse < 1e-9, "rmse {}", got.rmse);
    }

    #[test]
    fn survives_spread_noise() {
        let (f, df, t) = (522.38, 0.997_877, 0.060_233);
        let mut pairs = Vec::new();
        for (i, k) in (460..=580).step_by(5).enumerate() {
            let k = f64::from(k);
            let mut p = parity_pair_from_model(k, f, t, 0.42, df);
            // Deterministic alternating half-tick perturbation, the shape a
            // mid taken from a discrete quote grid actually has.
            let tick = if i % 2 == 0 { 0.025 } else { -0.025 };
            p.call_mid += tick;
            p.put_mid -= tick;
            pairs.push(p);
        }
        let got = implied_forward(&pairs, DEFAULT_BAND).expect("fit");
        assert!((got.forward - f).abs() < 0.20, "F {} vs {f}", got.forward);
        assert!(got.rmse > 0.0, "noise must show in the residual");
    }

    #[test]
    fn matches_the_reference_fit_on_real_quotes() {
        let pairs: Vec<ParityPair> =
            REAL.iter().map(|&(k, c, p)| ParityPair::new(k, c, p)).collect();
        let got = implied_forward(&pairs, DEFAULT_BAND).expect("fit");
        // Independent numpy least-squares on these same 16 rows.
        assert!((got.forward - 522.336_250).abs() < 1e-3, "F {}", got.forward);
        assert!((got.discount - 0.996_078_981).abs() < 1e-6, "DF {}", got.discount);
        // And it must stay economically sane: the full 48-pair fit gives
        // 522.3805, so a 16-point subset should land within a dollar.
        assert!((got.forward - 522.380_488).abs() < 1.0);
        // ~22 days at a few percent.
        let rate = -got.discount.ln() / 0.060_233;
        assert!((0.0..0.15).contains(&rate), "implied rate {rate}");
    }

    #[test]
    fn curve_recovers_a_constant_rate() {
        let r = 0.0412_f64;
        let obs: Vec<(f64, f64)> = [0.02_f64, 0.1, 0.25, 0.5, 1.0, 2.0, 2.5]
            .iter()
            .map(|&t| (t, (-r * t).exp()))
            .collect();
        let c = DiscountCurve::fit(&obs).expect("fit");
        assert!((c.rate - r).abs() < 1e-12, "rate {}", c.rate);
        assert!(c.rms_residual < 1e-12);
        assert!((c.df(1.0) - (-r).exp()).abs() < 1e-12);
    }

    /// The failure this whole type exists for: real per-expiry estimates from
    /// SOXX 2026-08-20, three of which are above 1.0 and therefore not
    /// discount factors at all.
    #[test]
    fn curve_repairs_impossible_short_dated_estimates() {
        const RAW: &[(f64, f64)] = &[
            (0.0219, 1.0001), (0.0411, 1.0017), (0.0602, 0.9979), (0.0794, 0.9966),
            (0.0986, 0.9982), (0.1177, 0.9968), (0.1561, 1.0005), (0.2519, 0.9991),
            (0.3285, 0.9932), (0.4052, 0.9928), (0.5010, 0.9937), (0.5777, 0.9912),
            (0.6543, 0.9773), (1.0760, 0.9833), (1.3251, 0.9742), (1.4209, 0.9774),
            (1.8234, 0.9631), (2.3217, 0.9498),
        ];
        assert_eq!(RAW.iter().filter(|(_, df)| *df > 1.0).count(), 3);

        let c = DiscountCurve::fit(RAW).expect("fit");
        // Independent numpy through-origin least squares on the same rows.
        assert!((c.rate - 0.020_116).abs() < 1e-5, "rate {}", c.rate);
        assert_eq!(c.expiries_used, RAW.len());

        // Every expiry now gets a usable discount factor, monotone in tenor.
        let mut prev = 1.0;
        for &(t, _) in RAW {
            let df = c.df(t);
            assert!(df > 0.0 && df < 1.0, "df({t}) = {df}");
            assert!(df <= prev, "not monotone at {t}");
            prev = df;
        }
        assert!((c.df(0.0219) - 0.999_560).abs() < 1e-5);
    }

    #[test]
    fn forward_at_recovers_the_model_given_the_discount() {
        let (f, df, t) = (522.38, 0.997_877, 0.060_233);
        let pairs: Vec<ParityPair> = (460..=580)
            .step_by(5)
            .map(|k| {
                let k = f64::from(k);
                parity_pair_from_model(k, f, t, 0.42 - 0.5 * (k / f).ln(), df)
            })
            .collect();
        let got = forward_at(&pairs, df, DEFAULT_BAND).expect("solve");
        assert!((got.forward - f).abs() < 1e-9, "F {}", got.forward);
        assert!(got.rmse < 1e-9);
        assert!(matches!(
            forward_at(&pairs, 1.5, DEFAULT_BAND),
            Err(ForwardError::DiscountOutOfRange(_))
        ));
    }

    /// Short-dated slices are exactly where the unconstrained regression fails
    /// and the curve-supplied discount succeeds.
    #[test]
    fn curve_beats_regression_at_the_short_end() {
        let (f, t) = (522.0_f64, 0.0219_f64);
        let df = (-0.0201 * t).exp();
        let mut pairs = Vec::new();
        for (i, k) in (500..=545).step_by(5).enumerate() {
            let k = f64::from(k);
            let mut p = parity_pair_from_model(k, f, t, 0.40, df);
            // Half-tick mid noise, which at three weeks swamps the ~4e-4 of
            // discount signal in the slope.
            let tick = if i % 2 == 0 { 0.025 } else { -0.025 };
            p.call_mid += tick;
            p.put_mid -= tick;
            pairs.push(p);
        }
        let free = implied_forward(&pairs, DEFAULT_BAND);
        let pinned = forward_at(&pairs, df, DEFAULT_BAND).expect("curve-supplied df works");
        // The forward survives either way; it is the discount that does not.
        assert!((pinned.forward - f).abs() < 0.30, "F {}", pinned.forward);
        if let Ok(free) = free {
            assert!(
                (free.discount - df).abs() > (pinned.discount - df).abs()
                    || (free.discount - df).abs() < 1e-6,
                "unconstrained discount {} vs true {df}",
                free.discount
            );
        }
    }

    #[test]
    fn rejects_unusable_input() {
        assert!(matches!(
            implied_forward(&[ParityPair::new(500.0, 20.0, 10.0)], DEFAULT_BAND),
            Err(ForwardError::TooFewPairs { .. })
        ));
        let flat: Vec<ParityPair> = (0..6).map(|_| ParityPair::new(500.0, 20.0, 10.0)).collect();
        assert!(matches!(
            implied_forward(&flat, DEFAULT_BAND),
            Err(ForwardError::Degenerate)
        ));
        // C-P rising in K is the wrong sign: implies a negative discount.
        let bad: Vec<ParityPair> = (0..6)
            .map(|i| ParityPair::new(500.0 + f64::from(i) * 5.0, 10.0 + f64::from(i), 5.0))
            .collect();
        assert!(matches!(
            implied_forward(&bad, DEFAULT_BAND),
            Err(ForwardError::DiscountOutOfRange(_))
        ));
        assert!(matches!(
            DiscountCurve::fit(&[(1.0, 0.98)]),
            Err(ForwardError::TooFewPairs { .. })
        ));
        assert!(matches!(
            DiscountCurve::fit(&[(1.0, -0.5), (2.0, 0.0)]),
            Err(ForwardError::TooFewPairs { .. })
        ));
    }
}
