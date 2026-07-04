//! HTTP API for the portfolio analytics engine.
//!
//! Thin Axum layer over `ptf-engine`: portfolios + holdings CRUD (in-memory)
//! and a `/risk` endpoint that folds transactions into a `PortfolioState`,
//! runs `compute_var`, and shapes the result into the dashboard's JSON
//! contract. Market data is supplied by a pluggable [`PriceSource`].

mod charts;
mod dto;
mod error;
mod handlers;
mod price_source;
mod risk_view;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;

use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::price_source::{ParquetPriceSource, PriceSource, SyntheticPriceSource};
use crate::state::AppState;

/// Build the price source from env. `PTF_PRICES=parquet` reads the Python
/// service's Parquet export (with ensure-on-add via `PRICES_URL`); otherwise a
/// deterministic synthetic feed is used.
fn price_source() -> (Arc<dyn PriceSource>, Option<String>) {
    if std::env::var("PTF_PRICES").as_deref() == Ok("parquet") {
        let prices = std::env::var("PRICES_PARQUET")
            .unwrap_or_else(|_| "services/prices/data/prices.parquet".into());
        let fx = std::env::var("FX_PARQUET")
            .unwrap_or_else(|_| "services/prices/data/fx.parquet".into());
        let url = std::env::var("PRICES_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8001".into());
        tracing::info!("prices: parquet ({prices}), ensure via {url}");
        (Arc::new(ParquetPriceSource::new(prices, fx)), Some(url))
    } else {
        tracing::info!("prices: synthetic (set PTF_PRICES=parquet for yfinance)");
        (Arc::new(SyntheticPriceSource), None)
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ptf_api=info,tower_http=info".into()),
        )
        .init();

    let (prices, prices_url) = price_source();
    let state = AppState::new(prices, prices_url);
    let app = handlers::router(state)
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
    axum::serve(listener, app).await.expect("server error");
}
