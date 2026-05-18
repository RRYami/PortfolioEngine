//! Within this crate, prefer the `TransactionKind::*` and `CorporateAction::*`
//! constructor functions over direct enum variant construction so that
//! validation is always exercised.

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::error::DomainError;
use crate::ids::{InstrumentId, TransactionId};
use crate::lot_method::LotSelection;
use crate::money::Money;

/// A single immutable portfolio event.
///
/// `trade_date` drives lot ordering, realized PnL date, and holding-period
/// calculations. `settle_date` drives cash availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub id: TransactionId,
    pub trade_date: NaiveDate,
    pub settle_date: NaiveDate,
    pub kind: TransactionKind,
}

impl Transaction {
    pub fn new(
        id: TransactionId,
        trade_date: NaiveDate,
        settle_date: NaiveDate,
        kind: TransactionKind,
    ) -> Result<Self, DomainError> {
        if settle_date < trade_date {
            return Err(DomainError::InvalidArgument(
                "settle_date must be on or after trade_date",
            ));
        }
        Ok(Self {
            id,
            trade_date,
            settle_date,
            kind,
        })
    }
}

/// The payload of a [`Transaction`].
///
/// **Warning:** Direct pattern-matching construction of enum variants skips
/// validation. Prefer the `TransactionKind::*` constructor functions
/// (`buy`, `sell`, `deposit`, etc.) which enforce domain invariants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionKind {
    Buy {
        instrument: InstrumentId,
        quantity: Decimal,
        price: Money,
        fees: Money,
        lot_override: Option<LotSelection>,
    },
    Sell {
        instrument: InstrumentId,
        quantity: Decimal,
        price: Money,
        fees: Money,
        lot_override: Option<LotSelection>,
    },
    Deposit {
        amount: Money,
    },
    Withdrawal {
        amount: Money,
    },
    Fee {
        amount: Money,
        description: Option<String>,
    },
    Dividend {
        instrument: InstrumentId,
        amount: Money,
        ex_date: Option<NaiveDate>,
    },
    CorporateAction(CorporateAction),
}

/// An externally-imposed event that mutates holdings without a cash flow
/// initiated by the portfolio owner.
///
/// Only [`Split`] and [`ReverseSplit`] are implemented for v1. The remaining
/// variants are placeholders so that dispatch shape is stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorporateAction {
    Split {
        instrument: InstrumentId,
        ratio: Decimal,
    },
    ReverseSplit {
        instrument: InstrumentId,
        ratio: Decimal,
    },
    Spinoff {
        from: InstrumentId,
        to: InstrumentId,
        ratio: Decimal,
        cost_basis_allocation: Decimal,
    },
    Merger {
        from: InstrumentId,
        to: InstrumentId,
        ratio: Decimal,
        cash_per_share: Option<Money>,
    },
    StockDividend {
        instrument: InstrumentId,
        ratio: Decimal,
    },
    ReturnOfCapital {
        instrument: InstrumentId,
        amount_per_share: Money,
    },
    SymbolChange {
        from: InstrumentId,
        to: InstrumentId,
    },
}

impl TransactionKind {
    pub fn buy(
        instrument: InstrumentId,
        quantity: Decimal,
        price: Money,
        fees: Money,
        lot_override: Option<LotSelection>,
    ) -> Result<Self, DomainError> {
        if quantity <= Decimal::ZERO {
            return Err(DomainError::InvalidAmount("quantity"));
        }
        if price.amount <= Decimal::ZERO {
            return Err(DomainError::InvalidPrice);
        }
        if fees.amount < Decimal::ZERO {
            return Err(DomainError::InvalidFee);
        }
        if fees.currency != price.currency {
            return Err(DomainError::CurrencyMismatch {
                expected: price.currency,
                got: fees.currency,
            });
        }
        Ok(Self::Buy {
            instrument,
            quantity,
            price,
            fees,
            lot_override,
        })
    }

    pub fn sell(
        instrument: InstrumentId,
        quantity: Decimal,
        price: Money,
        fees: Money,
        lot_override: Option<LotSelection>,
    ) -> Result<Self, DomainError> {
        if quantity <= Decimal::ZERO {
            return Err(DomainError::InvalidAmount("quantity"));
        }
        if price.amount <= Decimal::ZERO {
            return Err(DomainError::InvalidPrice);
        }
        if fees.amount < Decimal::ZERO {
            return Err(DomainError::InvalidFee);
        }
        if fees.currency != price.currency {
            return Err(DomainError::CurrencyMismatch {
                expected: price.currency,
                got: fees.currency,
            });
        }
        Ok(Self::Sell {
            instrument,
            quantity,
            price,
            fees,
            lot_override,
        })
    }

    pub fn deposit(amount: Money) -> Result<Self, DomainError> {
        if amount.amount <= Decimal::ZERO {
            return Err(DomainError::InvalidAmount("amount"));
        }
        Ok(Self::Deposit { amount })
    }

    pub fn withdrawal(amount: Money) -> Result<Self, DomainError> {
        if amount.amount <= Decimal::ZERO {
            return Err(DomainError::InvalidAmount("amount"));
        }
        Ok(Self::Withdrawal { amount })
    }

    pub fn fee(amount: Money, description: Option<String>) -> Result<Self, DomainError> {
        if amount.amount < Decimal::ZERO {
            return Err(DomainError::InvalidFee);
        }
        Ok(Self::Fee {
            amount,
            description,
        })
    }

    pub fn dividend(
        instrument: InstrumentId,
        amount: Money,
        ex_date: Option<NaiveDate>,
    ) -> Result<Self, DomainError> {
        if amount.amount <= Decimal::ZERO {
            return Err(DomainError::InvalidAmount("amount"));
        }
        Ok(Self::Dividend {
            instrument,
            amount,
            ex_date,
        })
    }
}

impl CorporateAction {
    pub fn split(instrument: InstrumentId, ratio: Decimal) -> Result<Self, DomainError> {
        if ratio <= Decimal::ZERO {
            return Err(DomainError::InvalidRatio("ratio"));
        }
        Ok(Self::Split { instrument, ratio })
    }

    pub fn reverse_split(instrument: InstrumentId, ratio: Decimal) -> Result<Self, DomainError> {
        if ratio <= Decimal::ZERO {
            return Err(DomainError::InvalidRatio("ratio"));
        }
        Ok(Self::ReverseSplit { instrument, ratio })
    }

    pub fn stock_dividend(instrument: InstrumentId, ratio: Decimal) -> Result<Self, DomainError> {
        if ratio <= Decimal::ZERO {
            return Err(DomainError::InvalidRatio("ratio"));
        }
        Ok(Self::StockDividend { instrument, ratio })
    }

    pub fn return_of_capital(
        instrument: InstrumentId,
        amount_per_share: Money,
    ) -> Result<Self, DomainError> {
        if amount_per_share.amount <= Decimal::ZERO {
            return Err(DomainError::InvalidAmount("amount per share"));
        }
        Ok(Self::ReturnOfCapital {
            instrument,
            amount_per_share,
        })
    }

    pub fn symbol_change(from: InstrumentId, to: InstrumentId) -> Result<Self, DomainError> {
        if from == to {
            return Err(DomainError::InvalidArgument(
                "from and to instruments must differ",
            ));
        }
        Ok(Self::SymbolChange { from, to })
    }

    /// `cost_basis_allocation` is the fraction of the original cost basis
    /// allocated to the spun-off instrument. Valid range is **0.0 to 1.0**
    /// inclusive; 0.0 means no basis moves, 1.0 means all basis moves.
    pub fn spinoff(
        from: InstrumentId,
        to: InstrumentId,
        ratio: Decimal,
        cost_basis_allocation: Decimal,
    ) -> Result<Self, DomainError> {
        if ratio <= Decimal::ZERO {
            return Err(DomainError::InvalidRatio("ratio"));
        }
        if cost_basis_allocation < Decimal::ZERO || cost_basis_allocation > Decimal::ONE {
            return Err(DomainError::InvalidAllocation);
        }
        Ok(Self::Spinoff {
            from,
            to,
            ratio,
            cost_basis_allocation,
        })
    }

    pub fn merger(
        from: InstrumentId,
        to: InstrumentId,
        ratio: Decimal,
        cash_per_share: Option<Money>,
    ) -> Result<Self, DomainError> {
        if ratio <= Decimal::ZERO {
            return Err(DomainError::InvalidRatio("ratio"));
        }
        Ok(Self::Merger {
            from,
            to,
            ratio,
            cash_per_share,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Currency;
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    fn usd(dollars: &str) -> Money {
        Money::new(Decimal::from_str_exact(dollars).unwrap(), Currency::USD)
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new()
    }

    #[test]
    fn buy_happy_path() {
        let kind =
            TransactionKind::buy(instrument(), Decimal::ONE, usd("10.00"), usd("1.00"), None);
        assert!(kind.is_ok());
    }

    #[test]
    fn buy_zero_quantity_fails() {
        let kind =
            TransactionKind::buy(instrument(), Decimal::ZERO, usd("10.00"), usd("1.00"), None);
        assert!(matches!(kind, Err(DomainError::InvalidAmount("quantity"))));
    }

    #[test]
    fn buy_negative_quantity_fails() {
        let kind = TransactionKind::buy(
            instrument(),
            Decimal::NEGATIVE_ONE,
            usd("10.00"),
            usd("1.00"),
            None,
        );
        assert!(matches!(kind, Err(DomainError::InvalidAmount("quantity"))));
    }

    #[test]
    fn buy_zero_price_fails() {
        let kind = TransactionKind::buy(instrument(), Decimal::ONE, usd("0.00"), usd("1.00"), None);
        assert!(matches!(kind, Err(DomainError::InvalidPrice)));
    }

    #[test]
    fn buy_negative_price_fails() {
        let kind =
            TransactionKind::buy(instrument(), Decimal::ONE, usd("-1.00"), usd("1.00"), None);
        assert!(matches!(kind, Err(DomainError::InvalidPrice)));
    }

    #[test]
    fn buy_negative_fee_fails() {
        let kind =
            TransactionKind::buy(instrument(), Decimal::ONE, usd("10.00"), usd("-1.00"), None);
        assert!(matches!(kind, Err(DomainError::InvalidFee)));
    }

    #[test]
    fn buy_zero_fee_succeeds() {
        let kind =
            TransactionKind::buy(instrument(), Decimal::ONE, usd("10.00"), usd("0.00"), None);
        assert!(kind.is_ok());
    }

    #[test]
    fn buy_currency_mismatch_fails() {
        let price = Money::new(Decimal::ONE, Currency::USD);
        let fees = Money::new(Decimal::ONE, Currency::EUR);
        let kind = TransactionKind::buy(instrument(), Decimal::ONE, price, fees, None);
        assert!(matches!(
            kind,
            Err(DomainError::CurrencyMismatch {
                expected: Currency::USD,
                got: Currency::EUR,
            })
        ));
    }

    #[test]
    fn sell_validations_mirror_buy() {
        let kind =
            TransactionKind::sell(instrument(), Decimal::ONE, usd("10.00"), usd("1.00"), None);
        assert!(kind.is_ok());

        let kind =
            TransactionKind::sell(instrument(), Decimal::ZERO, usd("10.00"), usd("1.00"), None);
        assert!(matches!(kind, Err(DomainError::InvalidAmount("quantity"))));
    }

    #[test]
    fn deposit_positive_amount_ok() {
        let kind = TransactionKind::deposit(usd("100.00"));
        assert!(kind.is_ok());
    }

    #[test]
    fn deposit_zero_amount_fails() {
        let kind = TransactionKind::deposit(usd("0.00"));
        assert!(matches!(kind, Err(DomainError::InvalidAmount("amount"))));
    }

    #[test]
    fn withdrawal_positive_amount_ok() {
        let kind = TransactionKind::withdrawal(usd("50.00"));
        assert!(kind.is_ok());
    }

    #[test]
    fn fee_zero_amount_ok() {
        let kind = TransactionKind::fee(usd("0.00"), None);
        assert!(kind.is_ok());
    }

    #[test]
    fn fee_negative_amount_fails() {
        let kind = TransactionKind::fee(usd("-5.00"), None);
        assert!(matches!(kind, Err(DomainError::InvalidFee)));
    }

    #[test]
    fn dividend_positive_amount_ok() {
        let kind = TransactionKind::dividend(instrument(), usd("10.00"), None);
        assert!(kind.is_ok());
    }

    #[test]
    fn dividend_zero_amount_fails() {
        let kind = TransactionKind::dividend(instrument(), usd("0.00"), None);
        assert!(matches!(kind, Err(DomainError::InvalidAmount("amount"))));
    }

    #[test]
    fn dividend_with_ex_date() {
        let ex = NaiveDate::from_ymd_opt(2024, 6, 15);
        let kind = TransactionKind::dividend(instrument(), usd("10.00"), ex);
        assert!(kind.is_ok());
        match kind.unwrap() {
            TransactionKind::Dividend { ex_date, .. } => assert_eq!(ex_date, ex),
            _ => panic!("expected Dividend"),
        }
    }

    #[test]
    fn split_positive_ratio_ok() {
        let ca = CorporateAction::split(instrument(), Decimal::from(2));
        assert!(ca.is_ok());
    }

    #[test]
    fn split_zero_ratio_fails() {
        let ca = CorporateAction::split(instrument(), Decimal::ZERO);
        assert!(matches!(ca, Err(DomainError::InvalidRatio("ratio"))));
    }

    #[test]
    fn reverse_split_positive_ratio_ok() {
        let ca = CorporateAction::reverse_split(instrument(), Decimal::new(5, 1));
        assert!(ca.is_ok());
    }

    #[test]
    fn stock_dividend_validation() {
        let ca = CorporateAction::stock_dividend(instrument(), Decimal::from(1));
        assert!(ca.is_ok());

        let ca = CorporateAction::stock_dividend(instrument(), Decimal::ZERO);
        assert!(matches!(ca, Err(DomainError::InvalidRatio("ratio"))));
    }

    #[test]
    fn return_of_capital_validation() {
        let ca = CorporateAction::return_of_capital(instrument(), usd("0.50"));
        assert!(ca.is_ok());

        let ca = CorporateAction::return_of_capital(instrument(), usd("0.00"));
        assert!(matches!(
            ca,
            Err(DomainError::InvalidAmount("amount per share"))
        ));
    }

    #[test]
    fn symbol_change_valid() {
        let from = instrument();
        let to = instrument();
        let ca = CorporateAction::symbol_change(from, to);
        assert!(ca.is_ok());
    }

    #[test]
    fn symbol_change_same_instrument_fails() {
        let id = instrument();
        let ca = CorporateAction::symbol_change(id, id);
        assert!(matches!(
            ca,
            Err(DomainError::InvalidArgument(
                "from and to instruments must differ"
            ))
        ));
    }

    #[test]
    fn spinoff_validation() {
        let from = instrument();
        let to = instrument();
        let ca = CorporateAction::spinoff(from, to, Decimal::from(1), Decimal::new(5, 1));
        assert!(ca.is_ok());

        let ca = CorporateAction::spinoff(from, to, Decimal::ZERO, Decimal::new(5, 1));
        assert!(matches!(ca, Err(DomainError::InvalidRatio("ratio"))));

        let ca = CorporateAction::spinoff(from, to, Decimal::from(1), Decimal::new(-1, 1));
        assert!(matches!(ca, Err(DomainError::InvalidAllocation)));

        let ca = CorporateAction::spinoff(from, to, Decimal::from(1), Decimal::new(15, 1));
        assert!(matches!(ca, Err(DomainError::InvalidAllocation)));
    }

    #[test]
    fn spinoff_edge_cases_zero_and_one() {
        let from = instrument();
        let to = instrument();
        let ca = CorporateAction::spinoff(from, to, Decimal::from(1), Decimal::ZERO);
        assert!(ca.is_ok());

        let ca = CorporateAction::spinoff(from, to, Decimal::from(1), Decimal::ONE);
        assert!(ca.is_ok());
    }

    #[test]
    fn merger_validation() {
        let from = instrument();
        let to = instrument();
        let ca = CorporateAction::merger(from, to, Decimal::from(1), None);
        assert!(ca.is_ok());

        let ca = CorporateAction::merger(from, to, Decimal::ZERO, None);
        assert!(matches!(ca, Err(DomainError::InvalidRatio("ratio"))));
    }

    #[test]
    fn transaction_construction_valid() {
        let id = TransactionId::new();
        let trade = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let settle = NaiveDate::from_ymd_opt(2024, 1, 17).unwrap();
        let kind =
            TransactionKind::buy(instrument(), Decimal::ONE, usd("10.00"), usd("0.00"), None)
                .unwrap();
        let tx = Transaction::new(id, trade, settle, kind.clone());
        assert!(tx.is_ok());
        let tx = tx.unwrap();
        assert_eq!(tx.id, id);
        assert_eq!(tx.trade_date, trade);
        assert_eq!(tx.settle_date, settle);
        assert_eq!(tx.kind, kind);
    }

    #[test]
    fn transaction_construction_settle_before_trade_fails() {
        let id = TransactionId::new();
        let trade = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let settle = NaiveDate::from_ymd_opt(2024, 1, 14).unwrap();
        let kind =
            TransactionKind::buy(instrument(), Decimal::ONE, usd("10.00"), usd("0.00"), None)
                .unwrap();
        let tx = Transaction::new(id, trade, settle, kind);
        assert!(matches!(
            tx,
            Err(DomainError::InvalidArgument(
                "settle_date must be on or after trade_date"
            ))
        ));
    }
}
