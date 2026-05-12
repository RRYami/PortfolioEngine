//! Portfolio analytics engine — domain layer.
//!
//! Pure types and logic for portfolios, instruments, transactions,
//! positions, lots, and multi-currency cash.

pub mod currency;
pub mod error;
pub mod fold;
pub mod ids;
pub mod instrument;
pub mod lot;
pub mod lot_method;
pub mod money;
pub mod portfolio_config;
pub mod portfolio_state;
pub mod position;
pub mod transaction;

pub use currency::Currency;
pub use error::DomainError;
pub use fold::fold;
pub use ids::{InstrumentId, LotId, PortfolioId, TransactionId};
pub use instrument::{Instrument, InstrumentKind};
pub use lot::Lot;
pub use lot_method::{LotMethod, LotSelection, LotSelectionEntry, LotSide};
pub use money::Money;
pub use portfolio_config::PortfolioConfig;
pub use portfolio_state::PortfolioState;
pub use position::Position;
pub use transaction::{CorporateAction, Transaction, TransactionKind};
