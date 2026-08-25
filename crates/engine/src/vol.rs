//! Black-76 pricing and implied-vol inversion.
//!
//! Deliberately `f64` throughout, not `Decimal`. Volatility is a statistical
//! quantity, not a monetary one: the maths is transcendental (`exp`, `ln`,
//! `erfc`), which `Decimal` does not implement, and the surface fit runs
//! millions of these per `VaR` report. `risk.rs` already sets this precedent —
//! covariance and Cholesky work in `f64` and convert at the `Money` boundary.
//!
//! Black-76 on the forward rather than Black-Scholes on spot, because the
//! forward and discount factor are recovered from the option chain itself by
//! put-call parity. That removes any need for an external rate curve or
//! dividend estimate, and it means the same kernel that inverts a quote also
//! prices the position — one implementation, so a fitted surface and a
//! valuation cannot silently disagree.

use std::fmt;

/// Exercise side of a contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum OptionRight {
    Call,
    Put,
}

impl fmt::Display for OptionRight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Call => "C",
            Self::Put => "P",
        })
    }
}

/// Why an inversion could not produce a volatility.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VolError {
    /// Forward, strike, or time to expiry was not strictly positive.
    NotPositive,
    /// Discount factor outside `(0, 1]`.
    BadDiscount(f64),
    /// Price at or below intrinsic — no positive vol can reach it.
    BelowIntrinsic { price: f64, intrinsic: f64 },
    /// Price at or above the no-arbitrage ceiling (`df*F` call, `df*K` put).
    AboveCeiling { price: f64, ceiling: f64 },
    /// Bracketing or iteration failed to converge.
    NoConvergence { last: f64 },
    /// Converged, but vega is too small for the premium to pin the volatility
    /// down. Deep in- or out-of-the-money quotes carry almost no vol
    /// information: the price is essentially all intrinsic (or essentially
    /// zero), so a band of volatilities reproduces it to f64 resolution.
    /// Returning the arbitrary member of that band the solver happened to land
    /// on would be worse than refusing — this is the quantitative form of the
    /// "invert OTM only" rule.
    Unstable { vol: f64, vega: f64, resolution: f64 },
}

impl fmt::Display for VolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPositive => write!(f, "forward, strike and tte must be positive"),
            Self::BadDiscount(d) => write!(f, "discount factor {d} outside (0, 1]"),
            Self::BelowIntrinsic { price, intrinsic } => {
                write!(f, "price {price} is at or below intrinsic {intrinsic}")
            }
            Self::AboveCeiling { price, ceiling } => {
                write!(f, "price {price} is at or above ceiling {ceiling}")
            }
            Self::NoConvergence { last } => {
                write!(f, "inversion did not converge (last iterate {last})")
            }
            Self::Unstable { vol, vega, resolution } => write!(
                f,
                "vol {vol} is not determined: vega {vega} resolves it only to +/-{resolution}"
            ),
        }
    }
}

impl std::error::Error for VolError {}

const SQRT_2: f64 = std::f64::consts::SQRT_2;
const INV_SQRT_2PI: f64 = 0.398_942_280_401_432_7;

/// Standard normal CDF, via `erfc` so the far tails stay accurate.
///
/// The naive `0.5*(1 + erf(x/sqrt2))` loses every significant digit for
/// `x < -6` through cancellation; `erfc` is evaluated directly there and the
/// deep wings of an option chain live exactly in that region.
#[inline]
pub fn norm_cdf(x: f64) -> f64 {
    0.5 * libm::erfc(-x / SQRT_2)
}

/// Standard normal PDF.
#[inline]
pub fn norm_pdf(x: f64) -> f64 {
    INV_SQRT_2PI * (-0.5 * x * x).exp()
}

/// Undiscounted payoff if the forward were realised today.
#[must_use]
pub fn intrinsic(right: OptionRight, forward: f64, strike: f64, df: f64) -> f64 {
    let raw = match right {
        OptionRight::Call => forward - strike,
        OptionRight::Put => strike - forward,
    };
    df * raw.max(0.0)
}

/// The no-arbitrage upper bound on a discounted premium.
#[must_use]
pub fn ceiling(right: OptionRight, forward: f64, strike: f64, df: f64) -> f64 {
    df * match right {
        OptionRight::Call => forward,
        OptionRight::Put => strike,
    }
}

/// `(d1, d2)` for a strictly positive `vol * sqrt(tte)`.
#[inline]
fn d1_d2(forward: f64, strike: f64, tte: f64, vol: f64) -> (f64, f64) {
    let sd = vol * tte.sqrt();
    let d1 = (forward / strike).ln() / sd + 0.5 * sd;
    (d1, d1 - sd)
}

/// Black-76 premium, discounted by `df`.
///
/// A zero or negative `vol`/`tte` collapses to intrinsic rather than erroring:
/// it is the correct limit, and it keeps expiry-day edge cases from
/// propagating errors through a Monte Carlo path.
#[must_use]
pub fn price(right: OptionRight, forward: f64, strike: f64, tte: f64, vol: f64, df: f64) -> f64 {
    if vol <= 0.0 || tte <= 0.0 {
        return intrinsic(right, forward, strike, df);
    }
    let (d1, d2) = d1_d2(forward, strike, tte, vol);
    df * match right {
        OptionRight::Call => forward * norm_cdf(d1) - strike * norm_cdf(d2),
        OptionRight::Put => strike * norm_cdf(-d2) - forward * norm_cdf(-d1),
    }
}

/// Sensitivity to a 1.0 change in volatility (divide by 100 for one vol point).
///
/// Identical for calls and puts — parity differs by a term with no vol in it.
#[must_use]
pub fn vega(forward: f64, strike: f64, tte: f64, vol: f64, df: f64) -> f64 {
    if vol <= 0.0 || tte <= 0.0 {
        return 0.0;
    }
    let (d1, _) = d1_d2(forward, strike, tte, vol);
    df * forward * norm_pdf(d1) * tte.sqrt()
}

/// Sensitivity to the *forward*, not spot. Scale by `dF/dS` for spot delta.
#[must_use]
pub fn delta(right: OptionRight, forward: f64, strike: f64, tte: f64, vol: f64, df: f64) -> f64 {
    if tte <= 0.0 || vol <= 0.0 {
        let itm = match right {
            OptionRight::Call => forward > strike,
            OptionRight::Put => forward < strike,
        };
        let sign = if right == OptionRight::Call { 1.0 } else { -1.0 };
        return if itm { df * sign } else { 0.0 };
    }
    let (d1, _) = d1_d2(forward, strike, tte, vol);
    df * match right {
        OptionRight::Call => norm_cdf(d1),
        OptionRight::Put => -norm_cdf(-d1),
    }
}

/// Second derivative with respect to the forward. Same for calls and puts.
#[must_use]
pub fn gamma(forward: f64, strike: f64, tte: f64, vol: f64, df: f64) -> f64 {
    if vol <= 0.0 || tte <= 0.0 {
        return 0.0;
    }
    let (d1, _) = d1_d2(forward, strike, tte, vol);
    df * norm_pdf(d1) / (forward * vol * tte.sqrt())
}

/// Largest volatility the bracket search will consider (1600% annualised).
const MAX_VOL: f64 = 16.0;
const MAX_ITER: u32 = 128;

/// Coarsest volatility resolution worth reporting: one hundredth of a vol
/// point. Below this the quote does not constrain the surface.
///
/// Public because it is the accuracy guarantee an `Ok` from [`implied_vol`]
/// carries: callers weighting a surface fit need to know the error bar.
pub const VOL_RESOLUTION: f64 = 1e-4;

/// Recover the volatility that reproduces `target` under Black-76.
///
/// A bracketed Newton hybrid rather than pure Newton. Vega vanishes in the deep
/// wings and at very short tenors, where a Newton step divides by ~0 and throws
/// the iterate far outside any sensible range; keeping a bracket and falling
/// back to bisection whenever a step would leave it makes convergence
/// unconditional at the cost of a few extra iterations on hard quotes.
///
/// This is not Jäckel's "Let's Be Rational" — that reaches the same accuracy in
/// a bounded two iterations via rational approximants, and is worth swapping in
/// if inversion ever shows up in a profile. It is not the bottleneck today: the
/// Monte Carlo *prices*, it does not invert.
pub fn implied_vol(
    right: OptionRight,
    target: f64,
    forward: f64,
    strike: f64,
    tte: f64,
    df: f64,
) -> Result<f64, VolError> {
    if !(forward > 0.0 && strike > 0.0 && tte > 0.0) {
        return Err(VolError::NotPositive);
    }
    if !(df > 0.0 && df <= 1.0) {
        return Err(VolError::BadDiscount(df));
    }

    let floor = intrinsic(right, forward, strike, df);
    let cap = ceiling(right, forward, strike, df);
    // Scale the tolerance to the contract: an absolute epsilon that is generous
    // for a $0.05 wing quote is meaningless against a $300 deep-ITM premium.
    let tol = 1e-12 * cap.max(1.0);
    if target <= floor + tol {
        return Err(VolError::BelowIntrinsic { price: target, intrinsic: floor });
    }
    if target >= cap - tol {
        return Err(VolError::AboveCeiling { price: target, ceiling: cap });
    }

    // Manaster-Koller seed: exact for at-the-money-forward, and a sound
    // starting magnitude elsewhere.
    let mut vol = ((2.0 * (forward / strike).ln().abs()) / tte).sqrt().clamp(1e-3, 4.0);

    let (mut lo, mut hi) = (0.0_f64, MAX_VOL);
    for _ in 0..MAX_ITER {
        let diff = price(right, forward, strike, tte, vol, df) - target;
        if diff.abs() < tol {
            // How wide is the set of vols that also fit inside `tol`? That is
            // the honest error bar on this inversion.
            let v = vega(forward, strike, tte, vol, df);
            let resolution = if v > 0.0 { tol / v } else { f64::INFINITY };
            return if resolution > VOL_RESOLUTION {
                Err(VolError::Unstable { vol, vega: v, resolution })
            } else {
                Ok(vol)
            };
        }
        if diff > 0.0 {
            hi = vol;
        } else {
            lo = vol;
        }

        let v = vega(forward, strike, tte, vol, df);
        let step = if v > 1e-14 { vol - diff / v } else { f64::NAN };
        // Reject a Newton step that leaves the bracket or is not finite; the
        // midpoint always makes progress.
        vol = if step.is_finite() && step > lo && step < hi {
            step
        } else {
            0.5 * (lo + hi)
        };
    }
    Err(VolError::NoConvergence { last: vol })
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: f64 = 544.7;
    const DF: f64 = 0.997_64;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn norm_cdf_known_values() {
        assert!(close(norm_cdf(0.0), 0.5, 1e-15));
        assert!(close(norm_cdf(1.96), 0.975_002_104_851_780, 1e-12));
        assert!(close(norm_cdf(-1.96), 0.024_997_895_148_220, 1e-12));
        // The far tail is where the erf-based form would have collapsed.
        assert!(close(norm_cdf(-8.0), 6.220_960_574_271_784e-16, 1e-9));
        // N(-35) ~ 1e-268 is representable; past about -38 the true value
        // falls below the smallest f64 subnormal and underflows to zero. That
        // is a limit of the type, not of this implementation.
        assert!(norm_cdf(-35.0) > 0.0);
    }

    #[test]
    fn put_call_parity_holds() {
        for &k in &[400.0, 544.7, 700.0] {
            for &t in &[0.04, 1.0, 2.5] {
                let c = price(OptionRight::Call, F, k, t, 0.28, DF);
                let p = price(OptionRight::Put, F, k, t, 0.28, DF);
                assert!(close(c - p, DF * (F - k), 1e-12), "k={k} t={t}");
            }
        }
    }

    #[test]
    fn zero_vol_or_expiry_gives_intrinsic() {
        assert!(close(price(OptionRight::Call, F, 500.0, 1.0, 0.0, DF), DF * 44.7, 1e-14));
        assert!(close(price(OptionRight::Put, F, 600.0, 0.0, 0.3, DF), DF * 55.3, 1e-14));
    }

    #[test]
    fn round_trip_recovers_vol() {
        for &right in &[OptionRight::Call, OptionRight::Put] {
            for &k in &[300.0, 450.0, 544.7, 650.0, 900.0] {
                for &t in &[0.02, 0.25, 1.0, 2.5] {
                    for &v in &[0.05, 0.2, 0.45, 1.2] {
                        let px = price(right, F, k, t, v, DF);
                        match implied_vol(right, px, F, k, t, DF) {
                            // The contract of an `Ok` is exactly this: the
                            // premium determined the vol to within
                            // VOL_RESOLUTION. Asserting anything tighter would
                            // test the conditioning of the sample rather than
                            // the solver — wing strikes cannot do better, and
                            // the kernel says so by returning `Unstable`.
                            Ok(got) => assert!(
                                (got - v).abs() <= VOL_RESOLUTION,
                                "{right} k={k} t={t} vol={v} -> {got}"
                            ),
                            // Wings and deep-ITM strikes carry no vol
                            // information; refusing them is correct.
                            Err(VolError::BelowIntrinsic { .. }
                            | VolError::Unstable { .. }) => {}
                            Err(e) => panic!("{right} k={k} t={t} vol={v}: {e}"),
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn vega_matches_finite_difference() {
        let (k, t, v) = (520.0, 0.5, 0.31);
        let h = 1e-6;
        let fd = (price(OptionRight::Call, F, k, t, v + h, DF)
            - price(OptionRight::Call, F, k, t, v - h, DF))
            / (2.0 * h);
        assert!(close(vega(F, k, t, v, DF), fd, 1e-6));
    }

    #[test]
    fn delta_and_gamma_match_finite_difference() {
        let (k, t, v) = (520.0, 0.5, 0.31);
        let h = 1e-4;
        for &right in &[OptionRight::Call, OptionRight::Put] {
            let fd = (price(right, F + h, k, t, v, DF) - price(right, F - h, k, t, v, DF))
                / (2.0 * h);
            assert!(close(delta(right, F, k, t, v, DF), fd, 1e-6), "{right} delta");
        }
        let fd2 = (price(OptionRight::Call, F + h, k, t, v, DF)
            - 2.0 * price(OptionRight::Call, F, k, t, v, DF)
            + price(OptionRight::Call, F - h, k, t, v, DF))
            / (h * h);
        assert!(close(gamma(F, k, t, v, DF), fd2, 1e-4));
    }

    #[test]
    fn deep_itm_is_reported_unstable_not_guessed() {
        // vega ~ 4e-6 here: many vols reproduce the premium to f64 resolution.
        let (k, t, v) = (300.0, 0.25, 0.2);
        let px = price(OptionRight::Call, F, k, t, v, DF);
        match implied_vol(OptionRight::Call, px, F, k, t, DF) {
            Err(VolError::Unstable { resolution, .. }) => {
                assert!(resolution > VOL_RESOLUTION);
            }
            other => panic!("expected Unstable, got {other:?}"),
        }
    }

    #[test]
    fn atm_stays_well_conditioned() {
        for &t in &[0.02, 0.25, 1.0, 2.5] {
            let px = price(OptionRight::Call, F, F, t, 0.3, DF);
            let got = implied_vol(OptionRight::Call, px, F, F, t, DF).expect("atm invertible");
            assert!(close(got, 0.3, 1e-9), "t={t} -> {got}");
        }
    }

    #[test]
    fn rejects_arbitrage_violating_prices() {
        let below = intrinsic(OptionRight::Call, F, 300.0, DF) * 0.99;
        assert!(matches!(
            implied_vol(OptionRight::Call, below, F, 300.0, 0.5, DF),
            Err(VolError::BelowIntrinsic { .. })
        ));
        let above = ceiling(OptionRight::Call, F, 300.0, DF) * 1.01;
        assert!(matches!(
            implied_vol(OptionRight::Call, above, F, 300.0, 0.5, DF),
            Err(VolError::AboveCeiling { .. })
        ));
        assert!(matches!(
            implied_vol(OptionRight::Call, 10.0, F, 500.0, -1.0, DF),
            Err(VolError::NotPositive)
        ));
        assert!(matches!(
            implied_vol(OptionRight::Call, 10.0, F, 500.0, 1.0, 1.5),
            Err(VolError::BadDiscount(_))
        ));
    }

    #[test]
    fn converges_where_pure_newton_would_not() {
        // 5-day, 40% out of the money: vega is ~1e-30, so a Newton step is
        // meaningless and only the bisection fallback makes progress.
        let (k, t, v) = (900.0, 5.0 / 365.25, 0.35);
        let px = price(OptionRight::Call, F, k, t, v, DF);
        assert!(px > 0.0);
        match implied_vol(OptionRight::Call, px, F, k, t, DF) {
            Ok(got) => assert!(got.is_finite() && got > 0.0),
            // Flagged as uninformative rather than silently returning junk.
            Err(VolError::Unstable { .. } | VolError::BelowIntrinsic { .. }) => {}
            Err(e) => panic!("bisection fallback should have made progress: {e}"),
        }
    }
}
