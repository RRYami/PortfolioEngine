use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
    /// When each symbol was last ensured, for [`ENSURE_TTL`]. Read paths (the
    /// benchmark on the performance tab) would otherwise make a cross-service
    /// round trip on every request; writes bypass this and always ensure.
    pub ensured: Arc<Mutex<HashMap<String, Instant>>>,
}

/// How long an ensure is considered good for on a read path. Prices move
/// daily, so this only has to be short enough that an intraday refresh lands
/// reasonably soon — not per-request.
pub const ENSURE_TTL: std::time::Duration = std::time::Duration::from_secs(900);

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
            ensured: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl AppState {
    /// Whether `symbol` is due an ensure on a read path.
    ///
    /// Records the attempt as it answers, so concurrent requests do not all
    /// stampede the prices service. A poisoned lock means some request panicked
    /// mid-update; ensuring again is harmless, so it is not worth propagating.
    pub fn should_ensure(&self, symbol: &str) -> bool {
        let Ok(mut seen) = self.ensured.lock() else {
            return true;
        };
        let now = Instant::now();
        match seen.get(symbol) {
            Some(at) if now.duration_since(*at) < ENSURE_TTL => false,
            _ => {
                seen.insert(symbol.to_string(), now);
                true
            }
        }
    }
}
