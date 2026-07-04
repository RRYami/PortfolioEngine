use std::sync::Arc;

use ptf_engine::{
    InMemoryInstrumentRepository, InMemoryPortfolioRepository, InMemoryTransactionRepository,
};

use crate::price_source::PriceSource;

/// Shared application state. Cheap to clone (all `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub portfolios: Arc<InMemoryPortfolioRepository>,
    pub transactions: Arc<InMemoryTransactionRepository>,
    pub instruments: Arc<InMemoryInstrumentRepository>,
    pub prices: Arc<dyn PriceSource>,
    /// Base URL of the Python prices service, if configured. When set, the API
    /// fetches prices on holding-add (ensure-on-add).
    pub prices_url: Option<String>,
}

impl AppState {
    pub fn new(prices: Arc<dyn PriceSource>, prices_url: Option<String>) -> Self {
        Self {
            portfolios: Arc::new(InMemoryPortfolioRepository::new()),
            transactions: Arc::new(InMemoryTransactionRepository::new()),
            instruments: Arc::new(InMemoryInstrumentRepository::new()),
            prices,
            prices_url,
        }
    }
}
