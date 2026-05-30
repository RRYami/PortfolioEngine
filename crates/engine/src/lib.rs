//! Portfolio analytics engine — domain layer.
//!
//! Pure types and logic for portfolios, instruments, transactions,
//! positions, lots, and multi-currency cash.

pub mod currency;
pub mod error;
pub mod fold;
pub mod fx;
pub mod historical_price;
pub mod ids;
pub mod instrument;
pub mod lot;
pub mod lot_method;
pub mod money;
pub mod portfolio;
pub mod portfolio_config;
pub mod portfolio_state;
pub mod position;
pub mod price;
pub mod repository;
pub mod risk;
pub mod transaction;
pub mod valuation;

pub use currency::Currency;
pub use error::DomainError;
pub use fold::fold;
pub use fx::{FxError, FxRateProvider, StaticFxRateProvider, TriangulatingFxProvider};
pub use historical_price::{HistoricalPriceProvider, StaticHistoricalPriceProvider};
pub use ids::{InstrumentId, LotId, PortfolioId, TransactionId};
pub use instrument::{Instrument, InstrumentKind};
pub use lot::Lot;
pub use lot_method::{LotMethod, LotSelection, LotSelectionEntry, LotSide};
pub use money::Money;
pub use portfolio::Portfolio;
pub use portfolio_config::PortfolioConfig;
pub use portfolio_state::PortfolioState;
pub use position::Position;
pub use price::{PriceError, PriceProvider, StaticPriceProvider};
pub use repository::{InstrumentRepository, PortfolioRepository, RepoError, TransactionRepository};
pub use risk::{AssetRisk, MonteCarloConfig, RiskError, VaREntry, VaRReport, compute_var};
pub use transaction::{CorporateAction, Transaction, TransactionKind};
pub use valuation::ValuationError;

#[cfg(any(test, feature = "in-memory-repo"))]
pub use repository::memory::{
    InMemoryInstrumentRepository, InMemoryPortfolioRepository, InMemoryTransactionRepository,
};
