use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::currency::Currency;
use crate::fx::{FxError, FxRateProvider};
use crate::ids::InstrumentId;
use crate::money::Money;
use crate::portfolio_state::PortfolioState;
use crate::price::{PriceError, PriceProvider};

/// Errors that can occur during portfolio valuation.
#[derive(Debug, thiserror::Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ValuationError {
    #[error(transparent)]
    Fx(#[from] FxError),
    #[error(transparent)]
    Price(#[from] PriceError),
    #[error(
        "price currency mismatch for {instrument:?}: position is in {position_currency}, price is in {price_currency}"
    )]
    PriceCurrencyMismatch {
        instrument: InstrumentId,
        position_currency: Currency,
        price_currency: Currency,
    },
}

impl PortfolioState {
    /// Compute the total portfolio value in `base` currency as of `as_of`.
    ///
    /// The total is the sum of:
    /// - Cash balances (per currency, converted to `base` via `fx`)
    /// - Position market values (`net_quantity * price`, converted to `base` via `fx`)
    ///
    /// Short positions naturally subtract from the total because
    /// [`Position::net_quantity`] is negative for net-short positions.
    ///
    /// # Errors
    ///
    /// Returns [`ValuationError::Price`] if the price provider cannot supply a
    /// price for any held instrument on `as_of`. Returns [`ValuationError::Fx`]
    /// if the FX provider cannot supply a required cross-currency rate. Returns
    /// [`ValuationError::PriceCurrencyMismatch`] if a price's currency does not
    /// match the position's currency.
    ///
    /// **No silent substitution**: missing prices, missing rates, and currency
    /// mismatches are loud failures, not zero.
    pub fn total_value(
        &self,
        fx: &dyn FxRateProvider,
        price: &dyn PriceProvider,
        base: Currency,
        as_of: NaiveDate,
    ) -> Result<Money, ValuationError> {
        let mut total = Decimal::ZERO;

        // Cash: convert each balance to base
        for (&currency, &balance) in &self.cash {
            let rate = fx.rate(currency, base, as_of)?;
            total += balance * rate;
        }

        // Positions: qty * price → position currency, then FX to base
        for (instrument_id, position) in &self.positions {
            let market_price = price.price(*instrument_id, as_of)?;
            if market_price.currency != position.currency() {
                return Err(ValuationError::PriceCurrencyMismatch {
                    instrument: *instrument_id,
                    position_currency: position.currency(),
                    price_currency: market_price.currency,
                });
            }
            let qty = position.net_quantity();
            let position_value = qty * market_price.amount;

            let fx_rate = fx.rate(position.currency(), base, as_of)?;
            total += position_value * fx_rate;
        }

        Ok(Money::new(total, base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{InstrumentId, LotId, TransactionId};
    use crate::lot::Lot;
    use crate::lot_method::LotSide;
    use crate::position::Position;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 6, day).unwrap()
    }

    fn make_lot(side: LotSide, qty: Decimal, basis: &str, currency: Currency) -> Lot {
        Lot::new(
            LotId::new(),
            0,
            side,
            qty,
            Money::new(Decimal::from_str_exact(basis).unwrap(), currency),
            date(1),
            TransactionId::new(),
        )
    }

    fn position_with(instrument: InstrumentId, currency: Currency, lots: Vec<Lot>) -> Position {
        let mut pos = Position::new(instrument, currency);
        pos.lots = lots;
        pos
    }

    // ── empty portfolio ────────────────────────────────────────────────────

    #[test]
    fn empty_portfolio_total_is_zero() {
        let state = PortfolioState::new();
        let fx = crate::fx::StaticFxRateProvider::new();
        let prices = crate::price::StaticPriceProvider::new();

        let total = state
            .total_value(&fx, &prices, Currency::USD, date(15))
            .unwrap();
        assert_eq!(total, Money::new(Decimal::ZERO, Currency::USD));
    }

    // ── single-currency portfolio (cash + positions) ───────────────────────

    #[test]
    fn single_currency_cash_plus_long_position() {
        let inst = InstrumentId::new();
        let mut state = PortfolioState::new();
        state.cash.insert(Currency::USD, Decimal::from(1000));
        state.positions.insert(
            inst,
            position_with(
                inst,
                Currency::USD,
                vec![make_lot(
                    LotSide::Long,
                    Decimal::from(10),
                    "50.00",
                    Currency::USD,
                )],
            ),
        );

        let fx = crate::fx::StaticFxRateProvider::new();
        let prices = crate::price::StaticPriceProvider::new().with_price(
            inst,
            date(15),
            Money::new(Decimal::from(60), Currency::USD),
        );

        let total = state
            .total_value(&fx, &prices, Currency::USD, date(15))
            .unwrap();
        // Cash 1000 + 10 * 60 = 1600
        assert_eq!(total, Money::new(Decimal::from(1600), Currency::USD));
    }

    #[test]
    fn single_currency_with_short_position() {
        let inst = InstrumentId::new();
        let mut state = PortfolioState::new();
        state.cash.insert(Currency::USD, Decimal::from(2000));
        state.positions.insert(
            inst,
            position_with(
                inst,
                Currency::USD,
                vec![make_lot(
                    LotSide::Short,
                    Decimal::from(5),
                    "50.00",
                    Currency::USD,
                )],
            ),
        );

        let fx = crate::fx::StaticFxRateProvider::new();
        let prices = crate::price::StaticPriceProvider::new().with_price(
            inst,
            date(15),
            Money::new(Decimal::from(60), Currency::USD),
        );

        let total = state
            .total_value(&fx, &prices, Currency::USD, date(15))
            .unwrap();
        // Cash 2000 + (-5) * 60 = 2000 - 300 = 1700
        assert_eq!(total, Money::new(Decimal::from(1700), Currency::USD));
    }

    // ── multi-currency portfolio (FX conversion) ───────────────────────────

    #[test]
    fn multi_currency_converts_and_sums() {
        let usd_inst = InstrumentId::new();
        let eur_inst = InstrumentId::new();

        let mut state = PortfolioState::new();
        state.cash.insert(Currency::USD, Decimal::from(500));
        state.cash.insert(Currency::EUR, Decimal::from(400));
        state.positions.insert(
            usd_inst,
            position_with(
                usd_inst,
                Currency::USD,
                vec![make_lot(
                    LotSide::Long,
                    Decimal::from(10),
                    "50.00",
                    Currency::USD,
                )],
            ),
        );
        state.positions.insert(
            eur_inst,
            position_with(
                eur_inst,
                Currency::EUR,
                vec![make_lot(
                    LotSide::Long,
                    Decimal::from(5),
                    "80.00",
                    Currency::EUR,
                )],
            ),
        );

        let fx = crate::fx::StaticFxRateProvider::new().with_rate(
            Currency::EUR,
            Currency::USD,
            date(15),
            Decimal::new(110, 2),
        ); // 1.10

        let prices = crate::price::StaticPriceProvider::new()
            .with_price(
                usd_inst,
                date(15),
                Money::new(Decimal::from(60), Currency::USD),
            )
            .with_price(
                eur_inst,
                date(15),
                Money::new(Decimal::from(90), Currency::EUR),
            );

        let total = state
            .total_value(&fx, &prices, Currency::USD, date(15))
            .unwrap();
        // Cash: 500 USD + 400 EUR * 1.10 = 500 + 440 = 940
        // Positions: 10 * 60 USD + 5 * 90 EUR * 1.10 = 600 + 495 = 1095
        // Total: 940 + 1095 = 2035
        assert_eq!(total, Money::new(Decimal::from(2035), Currency::USD));
    }

    // ── missing price is loud failure ──────────────────────────────────────

    #[test]
    fn missing_price_errors() {
        let inst = InstrumentId::new();
        let mut state = PortfolioState::new();
        state.positions.insert(
            inst,
            position_with(
                inst,
                Currency::USD,
                vec![make_lot(
                    LotSide::Long,
                    Decimal::from(10),
                    "50.00",
                    Currency::USD,
                )],
            ),
        );

        let fx = crate::fx::StaticFxRateProvider::new();
        let prices = crate::price::StaticPriceProvider::new();

        assert!(matches!(
            state.total_value(&fx, &prices, Currency::USD, date(15)),
            Err(ValuationError::Price(PriceError::PriceUnavailable { .. }))
        ));
    }

    // ── missing fx rate is loud failure ────────────────────────────────────

    #[test]
    fn missing_fx_rate_errors() {
        let eur_inst = InstrumentId::new();
        let mut state = PortfolioState::new();
        state.cash.insert(Currency::EUR, Decimal::from(400));
        state.positions.insert(
            eur_inst,
            position_with(
                eur_inst,
                Currency::EUR,
                vec![make_lot(
                    LotSide::Long,
                    Decimal::from(5),
                    "80.00",
                    Currency::EUR,
                )],
            ),
        );

        let fx = crate::fx::StaticFxRateProvider::new(); // no rates
        let prices = crate::price::StaticPriceProvider::new().with_price(
            eur_inst,
            date(15),
            Money::new(Decimal::from(90), Currency::EUR),
        );

        assert!(matches!(
            state.total_value(&fx, &prices, Currency::USD, date(15)),
            Err(ValuationError::Fx(FxError::RateUnavailable { .. }))
        ));
    }

    // ── negative cash balance ──────────────────────────────────────────────

    #[test]
    fn negative_cash_is_included() {
        let mut state = PortfolioState::new();
        state.cash.insert(Currency::USD, Decimal::from(-200));

        let fx = crate::fx::StaticFxRateProvider::new();
        let prices = crate::price::StaticPriceProvider::new();

        let total = state
            .total_value(&fx, &prices, Currency::USD, date(15))
            .unwrap();
        assert_eq!(total, Money::new(Decimal::from(-200), Currency::USD));
    }

    // ── flat position contributes zero ─────────────────────────────────────

    #[test]
    fn flat_position_contributes_zero() {
        let inst = InstrumentId::new();
        let mut state = PortfolioState::new();
        state.positions.insert(
            inst,
            position_with(
                inst,
                Currency::USD,
                vec![
                    make_lot(LotSide::Long, Decimal::from(10), "50.00", Currency::USD),
                    make_lot(LotSide::Short, Decimal::from(10), "55.00", Currency::USD),
                ],
            ),
        );

        let fx = crate::fx::StaticFxRateProvider::new();
        let prices = crate::price::StaticPriceProvider::new().with_price(
            inst,
            date(15),
            Money::new(Decimal::from(100), Currency::USD),
        );

        let total = state
            .total_value(&fx, &prices, Currency::USD, date(15))
            .unwrap();
        // Net qty = 0, so position contributes 0. Cash = 0.
        assert_eq!(total, Money::new(Decimal::ZERO, Currency::USD));
    }

    // ── mixed long and short lots in one position ───────────────────────

    #[test]
    fn mixed_lots_net_correctly() {
        let inst = InstrumentId::new();
        let mut state = PortfolioState::new();
        state.positions.insert(
            inst,
            position_with(
                inst,
                Currency::USD,
                vec![
                    make_lot(LotSide::Long, Decimal::from(10), "50.00", Currency::USD),
                    make_lot(LotSide::Short, Decimal::from(3), "60.00", Currency::USD),
                ],
            ),
        );

        let fx = crate::fx::StaticFxRateProvider::new();
        let prices = crate::price::StaticPriceProvider::new().with_price(
            inst,
            date(15),
            Money::new(Decimal::from(70), Currency::USD),
        );

        let total = state
            .total_value(&fx, &prices, Currency::USD, date(15))
            .unwrap();
        // Net qty = 7, position value = 7 * 70 = 490
        assert_eq!(total, Money::new(Decimal::from(490), Currency::USD));
    }

    // ── price currency mismatch is loud failure ──────────────────────────

    #[test]
    fn price_currency_mismatch_errors() {
        let inst = InstrumentId::new();
        let mut state = PortfolioState::new();
        state.positions.insert(
            inst,
            position_with(
                inst,
                Currency::EUR,
                vec![make_lot(
                    LotSide::Long,
                    Decimal::from(10),
                    "50.00",
                    Currency::EUR,
                )],
            ),
        );

        let fx = crate::fx::StaticFxRateProvider::new();
        // Price is in USD but position is in EUR — must not silently use it
        let prices = crate::price::StaticPriceProvider::new().with_price(
            inst,
            date(15),
            Money::new(Decimal::from(60), Currency::USD),
        );

        assert!(matches!(
            state.total_value(&fx, &prices, Currency::USD, date(15)),
            Err(ValuationError::PriceCurrencyMismatch {
                instrument: _,
                position_currency: Currency::EUR,
                price_currency: Currency::USD,
            })
        ));
    }
}
