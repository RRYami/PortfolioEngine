use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::currency::Currency;
use crate::ids::InstrumentId;
use crate::vol::OptionRight;

/// A tradable instrument (equity, bond, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Instrument {
    pub id: InstrumentId,
    pub symbol: String,
    pub name: String,
    pub currency: Currency,
    pub kind: InstrumentKind,
}

/// Classification of an instrument.
///
/// Uses struct variants so that adding instrument-specific metadata later is
/// a non-breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum InstrumentKind {
    Equity {},
    /// A listed option on an equity or ETF.
    ///
    /// `strike` and `multiplier` are [`Decimal`] rather than `f64` for the same
    /// reason money is: they are exact contract terms, not measurements. It
    /// also keeps this enum `Copy`, `Eq` and `Hash`, which the repositories and
    /// the `Json<InstrumentKind>` persistence column already rely on. The
    /// conversion to `f64` happens at the pricing boundary, exactly as
    /// `risk.rs` already does for covariance work.
    EquityOption {
        /// The instrument whose spot drives this contract. An option is priced
        /// as a *function of* its underlying, never from a price history of its
        /// own: moneyness and time to expiry shift underneath it, so its own
        /// past prices are not a usable return series.
        underlying: InstrumentId,
        right: OptionRight,
        strike: Decimal,
        expiry: NaiveDate,
        /// Shares per contract — 100 for a standard listed option. Carried
        /// explicitly because omitting it is a silent hundred-fold error in
        /// any P&L or risk number, and a default would hide the mistake.
        multiplier: Decimal,
        exercise: ExerciseStyle,
    },
}

/// When the holder may exercise.
///
/// Recorded even though the pricing kernel currently treats every contract as
/// European: the distinction is a property of the contract, and the surface
/// pipeline needs it to decide whether put-call parity holds as an identity or
/// only as an inequality. SOXX, SPY and single names are American; SPX is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ExerciseStyle {
    American,
    European,
}

/// Day-count used to turn an expiry into a year fraction.
///
/// ACT/365.25, matching what the ingest writes into `tte` and therefore what
/// the fitted surface is indexed by. A different convention here would read
/// the surface at the wrong maturity — a small, invisible, systematic
/// mispricing.
pub const DAYS_PER_YEAR: f64 = 365.25;

impl InstrumentKind {
    /// The instrument this one derives from, if any.
    #[must_use]
    pub fn underlying(&self) -> Option<InstrumentId> {
        match self {
            Self::Equity {} => None,
            Self::EquityOption { underlying, .. } => Some(*underlying),
        }
    }

    /// Whether this instrument must be revalued through a model rather than
    /// shocked directly.
    #[must_use]
    pub fn is_derivative(&self) -> bool {
        matches!(self, Self::EquityOption { .. })
    }

    /// Shares per contract; one for anything that is not a derivative.
    #[must_use]
    pub fn multiplier(&self) -> Decimal {
        match self {
            Self::Equity {} => Decimal::ONE,
            Self::EquityOption { multiplier, .. } => *multiplier,
        }
    }

    /// Years to expiry from `as_of`, or `None` for a non-expiring instrument.
    ///
    /// Returns a negative fraction past expiry rather than clamping, so a stale
    /// position surfaces as obviously wrong instead of silently pricing as if
    /// it expires today.
    #[must_use]
    pub fn year_fraction(&self, as_of: NaiveDate) -> Option<f64> {
        match self {
            Self::Equity {} => None,
            Self::EquityOption { expiry, .. } => {
                Some(f64::from(i32::try_from((*expiry - as_of).num_days()).unwrap_or(i32::MAX))
                    / DAYS_PER_YEAR)
            }
        }
    }

    /// Strike as `f64`, for the pricing kernel.
    #[must_use]
    pub fn strike_f64(&self) -> Option<f64> {
        match self {
            Self::Equity {} => None,
            Self::EquityOption { strike, .. } => strike.to_f64(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(strike: &str, expiry: NaiveDate) -> InstrumentKind {
        InstrumentKind::EquityOption {
            underlying: InstrumentId::new(),
            right: OptionRight::Call,
            strike: Decimal::from_str_exact(strike).unwrap(),
            expiry,
            multiplier: Decimal::from(100),
            exercise: ExerciseStyle::American,
        }
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// The year fraction has to agree with the `tte` the ingest writes, because
    /// that is the axis the fitted surface is indexed by. This is a real pair
    /// from the store: SOXX 2026-08-20, expiry 2026-09-25, tte 0.098563.
    #[test]
    fn year_fraction_matches_the_surface_day_count() {
        let k = opt("520", day(2026, 9, 25));
        let t = k.year_fraction(day(2026, 8, 20)).unwrap();
        assert!((t - 0.098_563).abs() < 1e-6, "{t}");
        assert!((t - 36.0 / 365.25).abs() < 1e-12);
    }

    #[test]
    fn expired_contracts_report_negative_time() {
        // Not clamped to zero: a stale position should look obviously wrong
        // rather than quietly price as if it expires today.
        let k = opt("520", day(2026, 8, 1));
        assert!(k.year_fraction(day(2026, 8, 20)).unwrap() < 0.0);
        assert!((k.year_fraction(day(2026, 8, 1)).unwrap() - 0.0).abs() < 1e-12);
    }

    #[test]
    fn equity_has_no_option_metadata() {
        let e = InstrumentKind::Equity {};
        assert!(e.underlying().is_none());
        assert!(!e.is_derivative());
        assert_eq!(e.multiplier(), Decimal::ONE);
        assert!(e.year_fraction(day(2026, 8, 20)).is_none());
        assert!(e.strike_f64().is_none());
    }

    #[test]
    fn option_exposes_what_pricing_needs() {
        let under = InstrumentId::new();
        let k = InstrumentKind::EquityOption {
            underlying: under,
            right: OptionRight::Put,
            strike: Decimal::from_str_exact("522.50").unwrap(),
            expiry: day(2026, 9, 25),
            multiplier: Decimal::from(100),
            exercise: ExerciseStyle::European,
        };
        assert_eq!(k.underlying(), Some(under));
        assert!(k.is_derivative());
        assert_eq!(k.multiplier(), Decimal::from(100));
        assert!((k.strike_f64().unwrap() - 522.5).abs() < 1e-12);
    }

    /// The repositories and the `Json<InstrumentKind>` persistence column rely
    /// on these; using f64 for the strike would have silently removed them.
    #[test]
    fn kind_stays_copy_eq_and_hashable() {
        use std::collections::HashSet;
        let k = opt("520", day(2026, 9, 25));
        let copied = k;
        assert_eq!(k, copied);
        let mut set = HashSet::new();
        set.insert(k);
        assert!(set.contains(&copied));
        assert_ne!(k, opt("525", day(2026, 9, 25)));
    }
}
