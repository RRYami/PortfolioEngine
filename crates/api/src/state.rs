use std::sync::Arc;

use ptf_engine::{
    InstrumentRepository, PortfolioRepository, TransactionRepository, UserRepository,
};

use crate::price_source::PriceSource;

/// Shared application state. Cheap to clone (all `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub portfolios: Arc<dyn PortfolioRepository>,
    pub transactions: Arc<dyn TransactionRepository>,
    pub instruments: Arc<dyn InstrumentRepository>,
    pub users: Arc<dyn UserRepository>,
    pub prices: Arc<dyn PriceSource>,
    /// Base URL of the Python prices service, if configured. When set, the API
    /// fetches prices on holding-add (ensure-on-add).
    pub prices_url: Option<String>,
    /// Whether `POST /api/auth/register` accepts new accounts
    /// (`PTF_DISABLE_REGISTRATION=1` closes it).
    pub registration_open: bool,
}

impl AppState {
    pub fn new(
        portfolios: Arc<dyn PortfolioRepository>,
        transactions: Arc<dyn TransactionRepository>,
        instruments: Arc<dyn InstrumentRepository>,
        users: Arc<dyn UserRepository>,
        prices: Arc<dyn PriceSource>,
        prices_url: Option<String>,
        registration_open: bool,
    ) -> Self {
        Self {
            portfolios,
            transactions,
            instruments,
            users,
            prices,
            prices_url,
            registration_open,
        }
    }
}
