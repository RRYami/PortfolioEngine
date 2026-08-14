//! HTTP API for the portfolio analytics engine.
//!
//! Thin Axum layer over `ptf-engine`: session-authenticated portfolios +
//! holdings CRUD (Postgres when `DATABASE_URL` is set, in-memory otherwise)
//! and a `/risk` endpoint that folds transactions into a `PortfolioState`,
//! runs `compute_var`, and shapes the result into the dashboard's JSON
//! contract. Market data is supplied by a pluggable [`PriceSource`].

mod auth;
mod charts;
mod dto;
mod equity;
mod error;
mod handlers;
mod perf_view;
mod positions_view;
mod price_source;
mod risk_view;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;

use ptf_engine::{
    InMemoryInstrumentRepository, InMemoryPortfolioRepository, InMemoryTransactionRepository,
    InMemoryUserRepository, InstrumentRepository, PortfolioRepository, TransactionRepository,
    UserRepository,
};
use ptf_persistence::{
    PgInstrumentRepository, PgPortfolioRepository, PgTransactionRepository, PgUserRepository,
};
use sqlx::PgPool;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_sessions::MemoryStore;
use tower_sessions_sqlx_store::PostgresStore;

use crate::price_source::{ParquetPriceSource, PriceSource, SyntheticPriceSource};
use crate::state::AppState;

/// Repository set plus the pool (when Postgres-backed) for the session store.
struct Storage {
    portfolios: Arc<dyn PortfolioRepository>,
    transactions: Arc<dyn TransactionRepository>,
    instruments: Arc<dyn InstrumentRepository>,
    users: Arc<dyn UserRepository>,
    pool: Option<PgPool>,
}

/// Build the repositories from env. When `DATABASE_URL` is set the API runs
/// on Postgres (embedded migrations are applied on boot); otherwise it falls
/// back to throwaway in-memory repositories.
async fn storage() -> Storage {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        let pool = ptf_persistence::create_pool(&url)
            .await
            .expect("DATABASE_URL is set but the database is unreachable or migrations failed");
        tracing::info!("storage: postgres (migrated)");
        return Storage {
            portfolios: Arc::new(PgPortfolioRepository::new(pool.clone())),
            transactions: Arc::new(PgTransactionRepository::new(pool.clone())),
            instruments: Arc::new(PgInstrumentRepository::new(pool.clone())),
            users: Arc::new(PgUserRepository::new(pool.clone())),
            pool: Some(pool),
        };
    }
    tracing::info!("storage: in-memory (set DATABASE_URL for postgres)");
    Storage {
        portfolios: Arc::new(InMemoryPortfolioRepository::new()),
        transactions: Arc::new(InMemoryTransactionRepository::new()),
        instruments: Arc::new(InMemoryInstrumentRepository::new()),
        users: Arc::new(InMemoryUserRepository::new()),
        pool: None,
    }
}

/// Build the price source from env. `PTF_PRICES=parquet` reads the Python
/// service's Parquet export (with ensure-on-add via `PRICES_URL`); otherwise a
/// deterministic synthetic feed is used.
fn price_source() -> (Arc<dyn PriceSource>, Option<String>) {
    if std::env::var("PTF_PRICES").as_deref() == Ok("parquet") {
        let prices = std::env::var("PRICES_PARQUET")
            .unwrap_or_else(|_| "services/prices/data/prices.parquet".into());
        let fx = std::env::var("FX_PARQUET")
            .unwrap_or_else(|_| "services/prices/data/fx.parquet".into());
        let url = std::env::var("PRICES_URL").unwrap_or_else(|_| "http://127.0.0.1:8001".into());
        tracing::info!("prices: parquet ({prices}), ensure via {url}");
        (Arc::new(ParquetPriceSource::new(prices, fx)), Some(url))
    } else {
        tracing::info!("prices: synthetic (set PTF_PRICES=parquet for yfinance)");
        (Arc::new(SyntheticPriceSource), None)
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ptf_api=info,tower_http=info".into()),
        )
        .init();

    let storage = storage().await;
    let (prices, prices_url) = price_source();
    let registration_open = !env_flag("PTF_DISABLE_REGISTRATION");
    // Secure cookies need TLS; off by default for local HTTP dev.
    let secure_cookies = env_flag("PTF_SECURE_COOKIES");
    if !registration_open {
        tracing::info!("registration disabled (PTF_DISABLE_REGISTRATION)");
    }

    let state = AppState::new(
        storage.portfolios,
        storage.transactions,
        storage.instruments,
        storage.users.clone(),
        prices,
        prices_url,
        registration_open,
    );
    let backend = auth::Backend::new(storage.users);
    let router = handlers::router(state);

    // Both arms produce the same `Router` type; only the session store differs.
    let app = if let Some(pool) = storage.pool {
        let store = PostgresStore::new(pool);
        store
            .migrate()
            .await
            .expect("session store migration failed");
        router.layer(auth::auth_layer(
            backend,
            auth::session_layer(store, secure_cookies),
        ))
    } else {
        router.layer(auth::auth_layer(
            backend,
            auth::session_layer(MemoryStore::default(), secure_cookies),
        ))
    };
    let app = app
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive());

    let addr: SocketAddr = std::env::var("PTF_API_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()
        .expect("valid PTF_API_ADDR");

    tracing::info!("ptf-api listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind listener");
    // ConnectInfo provides the client IP for the auth rate limiter.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server error");
}
