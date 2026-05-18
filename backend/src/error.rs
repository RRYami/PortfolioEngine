use crate::currency::Currency;
use crate::ids::{InstrumentId, LotId};

/// Typed errors for the domain layer.
///
/// Grouped by phase: validation happens at construction time; fold errors
/// happen when replaying transactions into portfolio state.
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    // Validation errors (transaction construction)
    #[error("{0} must be positive")]
    InvalidAmount(&'static str),
    #[error("{0} must be positive")]
    InvalidRatio(&'static str),
    #[error("price must be positive")]
    InvalidPrice,
    #[error("fee cannot be negative")]
    InvalidFee,
    #[error("cost basis allocation must be between 0 and 1")]
    InvalidAllocation,
    #[error("{0}")]
    InvalidArgument(&'static str),
    #[error("currency mismatch: expected {expected}, got {got}")]
    CurrencyMismatch { expected: Currency, got: Currency },
    #[error("invalid currency code: {0}")]
    InvalidCurrencyCode(String),

    // Fold errors (runtime state)
    #[error("insufficient lots for {instrument:?}: requested {requested}, available {available}")]
    InsufficientLots {
        instrument: InstrumentId,
        requested: rust_decimal::Decimal,
        available: rust_decimal::Decimal,
    },
    #[error("lot {0:?} not found")]
    LotNotFound(LotId),
    #[error("specified lots don't sum to transaction quantity: lots={lots}, tx={tx}")]
    LotQuantityMismatch {
        lots: rust_decimal::Decimal,
        tx: rust_decimal::Decimal,
    },
    #[error("transactions not in chronological order at index {0}")]
    UnorderedTransactions(usize),
    #[error("no position found for instrument {0:?}")]
    PositionNotFound(InstrumentId),
    #[error("dividend on short position is not supported in v1: {0:?}")]
    DividendOnShortPosition(InstrumentId),
}
