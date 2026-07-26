use std::sync::Arc;

use ptf_engine::{InstrumentRepository, PortfolioRepository, TransactionRepository};

use crate::price_source::PriceSource;

/// Shared application state. Cheap to clone (all `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub portfolios: Arc<dyn PortfolioRepository>,
    pub transactions: Arc<dyn TransactionRepository>,
    pub instruments: Arc<dyn InstrumentRepository>,
    pub prices: Arc<dyn PriceSource>,
    /// Base URL of the Python prices service, if configured. When set, the API
    /// fetches prices on holding-add (ensure-on-add).
    pub prices_url: Option<String>,
}

impl AppState {
    pub fn new(
        portfolios: Arc<dyn PortfolioRepository>,
        transactions: Arc<dyn TransactionRepository>,
        instruments: Arc<dyn InstrumentRepository>,
        prices: Arc<dyn PriceSource>,
        prices_url: Option<String>,
    ) -> Self {
        Self {
            portfolios,
            transactions,
            instruments,
            prices,
            prices_url,
        }
    }
}
