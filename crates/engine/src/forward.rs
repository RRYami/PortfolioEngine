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
    }
}
