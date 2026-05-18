use std::collections::HashMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::currency::Currency;

/// Errors returned by FX rate providers.
#[derive(Debug, thiserror::Error)]
pub enum FxError {
    #[error("rate unavailable {from} -> {to} on {date}")]
    RateUnavailable {
        from: Currency,
        to: Currency,
        date: NaiveDate,
    },
    #[error("provider error: {0}")]
    ProviderError(String),
    #[error("division by zero inverting rate {from} -> {to} on {date}")]
    DivisionByZero {
        from: Currency,
        to: Currency,
        date: NaiveDate,
    },
}

/// Synchronous FX rate provider.
///
/// The default [`rate`](FxRateProvider::rate) method handles same-currency
/// identity (`from == to` → `Decimal::ONE`) and delegates to
/// [`rate_impl`](FxRateProvider::rate_impl) for everything else. Implementors
/// only need to provide `rate_impl`.
pub trait FxRateProvider: Send + Sync {
    /// Returns the FX rate from `from` to `to` on `date`.
    ///
    /// Same-currency requests return `Decimal::ONE` without calling
    /// `rate_impl`. All other requests delegate to `rate_impl`.
    fn rate(&self, from: Currency, to: Currency, date: NaiveDate) -> Result<Decimal, FxError> {
        if from == to {
            return Ok(Decimal::ONE);
        }
        self.rate_impl(from, to, date)
    }

    /// Implementation hook for actual rate lookups.
    ///
    /// This method is **not** called when `from == to`; the trait's `rate`
    /// default handles identity. Implementors may assume `from != to`.
    fn rate_impl(&self, from: Currency, to: Currency, date: NaiveDate) -> Result<Decimal, FxError>;
}

/// In-memory FX rate provider backed by a [`HashMap`].
///
/// Only stores explicit rates; no triangulation, no inversion. The caller
/// must populate every rate they expect to use (or wrap this provider in
/// [`TriangulatingFxProvider`]).
#[derive(Debug, Clone, Default)]
pub struct StaticFxRateProvider {
    rates: HashMap<(Currency, Currency, NaiveDate), Decimal>,
}

impl StaticFxRateProvider {
    pub fn new() -> Self {
        Self {
            rates: HashMap::new(),
        }
    }

    pub fn insert(&mut self, from: Currency, to: Currency, date: NaiveDate, rate: Decimal) {
        self.rates.insert((from, to, date), rate);
    }

    pub fn with_rate(
        mut self,
        from: Currency,
        to: Currency,
        date: NaiveDate,
        rate: Decimal,
    ) -> Self {
        self.insert(from, to, date, rate);
        self
    }
}

impl FxRateProvider for StaticFxRateProvider {
    fn rate_impl(&self, from: Currency, to: Currency, date: NaiveDate) -> Result<Decimal, FxError> {
        self.rates
            .get(&(from, to, date))
            .copied()
            .ok_or(FxError::RateUnavailable { from, to, date })
    }
}

/// Wrapper that triangulates FX rates via a pivot currency when a direct rate
/// is unavailable.
///
/// Algorithm for `rate_impl(from, to, date)`:
///
/// 1. Try direct rate via inner provider.
/// 2. Try inverse rate (`1 / inner.rate(to, from, date)`).
/// 3. Try triangulation via pivot:
///    - `inner.rate(from, pivot) * inner.rate(pivot, to)`
///    - `inner.rate(from, pivot) / inner.rate(to, pivot)` (second leg inverted)
///    - `inner.rate(pivot, to) / inner.rate(pivot, from)` (first leg inverted)
///    - `1 / (inner.rate(pivot, from) * inner.rate(to, pivot))` (both inverted)
/// 4. Return [`FxError::RateUnavailable`] if all attempts fail.
///
/// ## Rounding behavior
///
/// Inverting a rate (`1 / rate`) is exact only when the rate divides `1`
/// exactly in `Decimal` arithmetic. In general, `rate(X, Y) * rate(Y, X)`
/// will not equal `Decimal::ONE`; round-trip tests should use an epsilon.
/// This is expected and documented.
///
/// ## Panics
///
/// Never panics. Division by zero returns [`FxError::DivisionByZero`].
pub struct TriangulatingFxProvider<P> {
    inner: P,
    pivot: Currency,
}

impl<P> TriangulatingFxProvider<P> {
    pub fn new(inner: P, pivot: Currency) -> Self {
        Self { inner, pivot }
    }
}

impl<P: FxRateProvider> FxRateProvider for TriangulatingFxProvider<P> {
    fn rate_impl(&self, from: Currency, to: Currency, date: NaiveDate) -> Result<Decimal, FxError> {
        let pivot = self.pivot;

        // 1. Direct rate
        if let Ok(rate) = self.inner.rate(from, to, date) {
            return Ok(rate);
        }

        // 2. Inverse rate
        if let Ok(inv) = self.inner.rate(to, from, date) {
            if inv.is_zero() {
                return Err(FxError::DivisionByZero { from, to, date });
            }
            return Ok(Decimal::ONE / inv);
        }

        // Helper for safe division
        let safe_invert = |rate: Decimal, f: Currency, t: Currency| -> Result<Decimal, FxError> {
            if rate.is_zero() {
                Err(FxError::DivisionByZero {
                    from: f,
                    to: t,
                    date,
                })
            } else {
                Ok(Decimal::ONE / rate)
            }
        };

        // 3. Triangulation via pivot — try all 4 leg-direction combinations

        // 3a. from->pivot * pivot->to
        if let Ok(leg1) = self.inner.rate(from, pivot, date)
            && let Ok(leg2) = self.inner.rate(pivot, to, date)
        {
            return Ok(leg1 * leg2);
        }

        // 3b. from->pivot * (to->pivot)^-1
        if let Ok(leg1) = self.inner.rate(from, pivot, date)
            && let Ok(inv_leg2) = self.inner.rate(to, pivot, date)
            && let Ok(leg2) = safe_invert(inv_leg2, to, pivot)
        {
            return Ok(leg1 * leg2);
        }

        // 3c. (pivot->from)^-1 * pivot->to
        if let Ok(inv_leg1) = self.inner.rate(pivot, from, date)
            && let Ok(leg1) = safe_invert(inv_leg1, pivot, from)
            && let Ok(leg2) = self.inner.rate(pivot, to, date)
        {
            return Ok(leg1 * leg2);
        }

        // 3d. (pivot->from)^-1 * (to->pivot)^-1
        if let Ok(inv_leg1) = self.inner.rate(pivot, from, date)
            && let Ok(leg1) = safe_invert(inv_leg1, pivot, from)
            && let Ok(inv_leg2) = self.inner.rate(to, pivot, date)
            && let Ok(leg2) = safe_invert(inv_leg2, to, pivot)
        {
            return Ok(leg1 * leg2);
        }

        Err(FxError::RateUnavailable { from, to, date })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, day).unwrap()
    }

    // ── StaticFxRateProvider ────────────────────────────────────────────────

    #[test]
    fn static_direct_hit() {
        let mut fx = StaticFxRateProvider::new();
        fx.insert(Currency::USD, Currency::EUR, date(1), Decimal::new(85, 2)); // 0.85

        assert_eq!(
            fx.rate(Currency::USD, Currency::EUR, date(1)).unwrap(),
            Decimal::new(85, 2)
        );
    }

    #[test]
    fn static_direct_miss() {
        let fx = StaticFxRateProvider::new();
        assert!(matches!(
            fx.rate(Currency::USD, Currency::EUR, date(1)),
            Err(FxError::RateUnavailable { .. })
        ));
    }

    #[test]
    fn static_same_currency_returns_one() {
        let fx = StaticFxRateProvider::new();
        assert_eq!(
            fx.rate(Currency::USD, Currency::USD, date(1)).unwrap(),
            Decimal::ONE
        );
    }

    #[test]
    fn static_different_date_misses() {
        let mut fx = StaticFxRateProvider::new();
        fx.insert(Currency::USD, Currency::EUR, date(1), Decimal::new(85, 2));

        assert!(matches!(
            fx.rate(Currency::USD, Currency::EUR, date(2)),
            Err(FxError::RateUnavailable { .. })
        ));
    }

    #[test]
    fn static_builder_pattern() {
        let fx = StaticFxRateProvider::new()
            .with_rate(Currency::USD, Currency::EUR, date(1), Decimal::new(85, 2))
            .with_rate(Currency::EUR, Currency::GBP, date(1), Decimal::new(88, 2));

        assert_eq!(
            fx.rate(Currency::USD, Currency::EUR, date(1)).unwrap(),
            Decimal::new(85, 2)
        );
        assert_eq!(
            fx.rate(Currency::EUR, Currency::GBP, date(1)).unwrap(),
            Decimal::new(88, 2)
        );
    }

    // ── TriangulatingFxProvider: direct ───────────────────────────────────

    #[test]
    fn tri_direct_rate() {
        let inner = StaticFxRateProvider::new().with_rate(
            Currency::USD,
            Currency::EUR,
            date(1),
            Decimal::new(85, 2),
        );
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        assert_eq!(
            tri.rate(Currency::USD, Currency::EUR, date(1)).unwrap(),
            Decimal::new(85, 2)
        );
    }

    // ── TriangulatingFxProvider: same-currency identity ────────────────────

    #[test]
    fn tri_same_currency_identity() {
        let inner = StaticFxRateProvider::new();
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        assert_eq!(
            tri.rate(Currency::EUR, Currency::EUR, date(1)).unwrap(),
            Decimal::ONE
        );
    }

    // ── TriangulatingFxProvider: inversion ─────────────────────────────────
    // These verify that the provider attempts inversion before triangulation,
    // matching the documented order in rate_impl.

    #[test]
    fn tri_inverse_rate() {
        // Store only USD->EUR; ask for EUR->USD
        let inner = StaticFxRateProvider::new().with_rate(
            Currency::USD,
            Currency::EUR,
            date(1),
            Decimal::new(85, 2),
        );
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        let rate = tri.rate(Currency::EUR, Currency::USD, date(1)).unwrap();
        // 1 / 0.85 = 1.1764705882...
        assert!(!rate.is_zero());
        // Verify it's the inverse within reasonable epsilon
        let expected = Decimal::ONE / Decimal::new(85, 2);
        assert_eq!(rate, expected);
    }

    #[test]
    fn tri_inverse_round_trip_not_exact() {
        // Store only USD->EUR at 0.87 (a rate whose inverse does not round-trip
        // exactly in Decimal arithmetic).
        let inner = StaticFxRateProvider::new().with_rate(
            Currency::USD,
            Currency::EUR,
            date(1),
            Decimal::new(87, 2),
        );
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        let eur_to_usd = tri.rate(Currency::EUR, Currency::USD, date(1)).unwrap();
        let usd_to_eur = tri.rate(Currency::USD, Currency::EUR, date(1)).unwrap();

        let product = eur_to_usd * usd_to_eur;
        // The round-trip product is very close to 1.0 but not exact, because
        // the inverse was truncated at Decimal's max precision.
        let epsilon = Decimal::new(1, 10);
        assert!((product - Decimal::ONE).abs() < epsilon);
    }

    // ── TriangulatingFxProvider: triangulation ─────────────────────────────
    // Each test verifies that the provider resolves rates in the documented
    // order: direct → inverse → pivot legs (3a–3d).

    #[test]
    fn tri_via_pivot_both_direct() {
        // EUR -> GBP via USD: EUR->USD = 1.10, USD->GBP = 0.80
        // Expected: 1.10 * 0.80 = 0.88
        let inner = StaticFxRateProvider::new()
            .with_rate(Currency::EUR, Currency::USD, date(1), Decimal::new(110, 2))
            .with_rate(Currency::USD, Currency::GBP, date(1), Decimal::new(80, 2));
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        assert_eq!(
            tri.rate(Currency::EUR, Currency::GBP, date(1)).unwrap(),
            Decimal::new(88, 2)
        );
    }

    #[test]
    fn tri_via_pivot_second_leg_inverted() {
        // EUR -> GBP via USD:
        // EUR->USD = 1.10 (direct)
        // GBP->USD = 1.25 (stored), so USD->GBP = 1/1.25 = 0.80
        // Expected: 1.10 * 0.80 = 0.88
        let inner = StaticFxRateProvider::new()
            .with_rate(Currency::EUR, Currency::USD, date(1), Decimal::new(110, 2))
            .with_rate(Currency::GBP, Currency::USD, date(1), Decimal::new(125, 2));
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        let rate = tri.rate(Currency::EUR, Currency::GBP, date(1)).unwrap();
        let expected = Decimal::new(110, 2) * (Decimal::ONE / Decimal::new(125, 2));
        assert_eq!(rate, expected);
    }

    #[test]
    fn tri_via_pivot_first_leg_inverted() {
        // EUR -> GBP via USD:
        // USD->EUR = 0.90 (stored), so EUR->USD = 1/0.90 = 1.111...
        // USD->GBP = 0.80 (direct)
        let inner = StaticFxRateProvider::new()
            .with_rate(Currency::USD, Currency::EUR, date(1), Decimal::new(90, 2))
            .with_rate(Currency::USD, Currency::GBP, date(1), Decimal::new(80, 2));
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        let rate = tri.rate(Currency::EUR, Currency::GBP, date(1)).unwrap();
        let expected = (Decimal::ONE / Decimal::new(90, 2)) * Decimal::new(80, 2);
        assert_eq!(rate, expected);
    }

    #[test]
    fn tri_via_pivot_both_inverted() {
        // EUR -> GBP via USD:
        // USD->EUR = 0.90, so EUR->USD = 1/0.90
        // GBP->USD = 1.25, so USD->GBP = 1/1.25
        let inner = StaticFxRateProvider::new()
            .with_rate(Currency::USD, Currency::EUR, date(1), Decimal::new(90, 2))
            .with_rate(Currency::GBP, Currency::USD, date(1), Decimal::new(125, 2));
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        let rate = tri.rate(Currency::EUR, Currency::GBP, date(1)).unwrap();
        let expected = (Decimal::ONE / Decimal::new(90, 2)) * (Decimal::ONE / Decimal::new(125, 2));
        assert_eq!(rate, expected);
    }

    #[test]
    fn tri_falls_back_to_inverse_when_triangulation_fails() {
        // Only store EUR->USD; ask for USD->EUR
        // Direct: miss. Inverse: EUR->USD exists, return 1/rate.
        // Triangulation won't help because no USD->??? or ???->EUR rates.
        let inner = StaticFxRateProvider::new().with_rate(
            Currency::EUR,
            Currency::USD,
            date(1),
            Decimal::new(110, 2),
        );
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        let rate = tri.rate(Currency::USD, Currency::EUR, date(1)).unwrap();
        assert_eq!(rate, Decimal::ONE / Decimal::new(110, 2));
    }

    #[test]
    fn tri_unavailable_when_nothing_works() {
        let inner = StaticFxRateProvider::new();
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        assert!(matches!(
            tri.rate(Currency::EUR, Currency::GBP, date(1)),
            Err(FxError::RateUnavailable { .. })
        ));
    }

    #[test]
    fn tri_division_by_zero() {
        let mut inner = StaticFxRateProvider::new();
        inner.insert(Currency::USD, Currency::EUR, date(1), Decimal::ZERO);
        let tri = TriangulatingFxProvider::new(inner, Currency::GBP);

        // Asking for EUR->USD requires inverting USD->EUR = 0 → division by zero
        assert!(matches!(
            tri.rate(Currency::EUR, Currency::USD, date(1)),
            Err(FxError::DivisionByZero { .. })
        ));
    }

    #[test]
    fn tri_triangulation_equality_with_direct() {
        // If both direct and triangulated rates exist, direct wins (first check)
        let inner = StaticFxRateProvider::new()
            .with_rate(Currency::EUR, Currency::GBP, date(1), Decimal::new(90, 2))
            .with_rate(Currency::EUR, Currency::USD, date(1), Decimal::new(110, 2))
            .with_rate(Currency::USD, Currency::GBP, date(1), Decimal::new(80, 2));
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        // Direct rate is 0.90, triangulated would be 0.88
        assert_eq!(
            tri.rate(Currency::EUR, Currency::GBP, date(1)).unwrap(),
            Decimal::new(90, 2)
        );
    }
}
