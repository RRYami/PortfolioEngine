use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use ptf_engine::{
    Instrument, InstrumentId, InstrumentKind, InstrumentRepository, Money, Portfolio,
    PortfolioConfig, PortfolioId, PortfolioRepository, Transaction, TransactionId, TransactionKind,
    TransactionRepository, fold,
};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::dto::{
    AddHoldingReq, CreatePortfolioReq, PortfolioDetail, PortfolioSummary, PositionView,
    parse_currency, parse_lot_method,
};
use crate::error::ApiError;
use crate::price_source::HeldInstrument;
use crate::risk_view::{self, RiskPayload};
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/portfolios", get(list_portfolios).post(create_portfolio))
        .route("/api/portfolios/{id}", get(get_portfolio))
        .route("/api/portfolios/{id}/holdings", post(add_holding))
        .route("/api/portfolios/{id}/risk", get(get_risk))
        .with_state(state)
}

fn parse_id(id: &str) -> Result<PortfolioId, ApiError> {
    Uuid::parse_str(id)
        .map(PortfolioId)
        .map_err(|_| ApiError::BadRequest(format!("invalid portfolio id: {id}")))
}

async fn list_portfolios(
    State(app): State<AppState>,
) -> Result<Json<Vec<PortfolioSummary>>, ApiError> {
    let list = app.portfolios.list().await?;
    Ok(Json(list.iter().map(PortfolioSummary::from).collect()))
}

async fn create_portfolio(
    State(app): State<AppState>,
    Json(req): Json<CreatePortfolioReq>,
) -> Result<Json<PortfolioSummary>, ApiError> {
    let base = parse_currency(&req.base_ccy)?;
    let lot_method = parse_lot_method(&req.lot_method)?;
    let inception = req.inception_date.unwrap_or_else(|| Utc::now().date_naive());
    let portfolio = Portfolio::new(PortfolioId::new(), req.name, base, lot_method, inception);
    app.portfolios.create(&portfolio).await?;
    Ok(Json(PortfolioSummary::from(&portfolio)))
}

async fn get_portfolio(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PortfolioDetail>, ApiError> {
    let pid = parse_id(&id)?;
    let portfolio = app.portfolios.get(pid).await?;
    let state = fold_state(&app, &portfolio).await?;

    let mut positions = Vec::new();
    for (inst_id, pos) in state.positions() {
        let inst = app.instruments.get(*inst_id).await.ok();
        positions.push(PositionView {
            ticker: inst.as_ref().map_or_else(|| inst_id.0.to_string(), |i| i.symbol.clone()),
            name: inst.as_ref().map_or_else(String::new, |i| i.name.clone()),
            ccy: pos.currency().to_string(),
            quantity: pos.net_quantity(),
        });
    }
    Ok(Json(PortfolioDetail {
        summary: PortfolioSummary::from(&portfolio),
        positions,
    }))
}

async fn add_holding(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AddHoldingReq>,
) -> Result<Json<PortfolioSummary>, ApiError> {
    let pid = parse_id(&id)?;
    let portfolio = app.portfolios.get(pid).await?;
    let ccy = parse_currency(&req.currency)?;
    if req.quantity <= Decimal::ZERO {
        return Err(ApiError::BadRequest("quantity must be positive".into()));
    }
    if req.cost <= Decimal::ZERO {
        return Err(ApiError::BadRequest("cost must be positive".into()));
    }

    // Ensure-on-add: fetch/cache prices for this symbol before recording it, so
    // /risk can price it. No-op when the prices service isn't configured.
    ensure_prices(&app, &req.ticker).await?;

    // Register the instrument (idempotent by symbol).
    let inst_id = if let Ok(existing) = app.instruments.by_symbol(&req.ticker).await { existing.id } else {
        let inst = Instrument {
            id: InstrumentId::new(),
            symbol: req.ticker.clone(),
            name: req.name.clone().unwrap_or_else(|| req.ticker.clone()),
            currency: ccy,
            kind: InstrumentKind::Equity {},
        };
        app.instruments.upsert(&inst).await?;
        inst.id
    };

    let date = req.date.unwrap_or(portfolio.inception_date);
    let cost = Money::new(req.cost, ccy);
    let zero = Money::new(Decimal::ZERO, ccy);

    // Desugar: deposit enough cash, then buy.
    let deposit = TransactionKind::deposit(Money::new(req.quantity * req.cost, ccy))?;
    let buy = TransactionKind::buy(inst_id, req.quantity, cost, zero, None)?;
    let dep_tx = Transaction::new(TransactionId::new(), date, date, deposit)?;
    let buy_tx = Transaction::new(TransactionId::new(), date, date, buy)?;
    app.transactions.append(pid, &dep_tx).await?;
    app.transactions.append(pid, &buy_tx).await?;

    Ok(Json(PortfolioSummary::from(&portfolio)))
}

async fn get_risk(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RiskPayload>, ApiError> {
    let pid = parse_id(&id)?;
    let portfolio = app.portfolios.get(pid).await?;
    let state = fold_state(&app, &portfolio).await?;

    // Gather held-instrument metadata + a ticker→name map.
    let mut holdings = Vec::new();
    let mut names = HashMap::new();
    for inst_id in state.positions().keys() {
        if let Ok(inst) = app.instruments.get(*inst_id).await {
            names.insert(inst.symbol.clone(), inst.name.clone());
            holdings.push(HeldInstrument {
                id: inst.id,
                symbol: inst.symbol,
                currency: inst.currency,
            });
        }
    }

    let as_of = Utc::now().date_naive();
    let lookback = ptf_engine::MonteCarloConfig::default_var().lookback_days;
    let pd = app.prices.build(&holdings, portfolio.base_currency, as_of, lookback)?;
    let payload = risk_view::build(&portfolio, &state, &holdings, &names, &pd, as_of)?;
    Ok(Json(payload))
}

async fn fold_state(
    app: &AppState,
    portfolio: &Portfolio,
) -> Result<ptf_engine::PortfolioState, ApiError> {
    let txns: Vec<Transaction> = app.transactions.list(portfolio.id).await?;
    let config = PortfolioConfig::new(portfolio.lot_method, portfolio.base_currency);
    Ok(fold(&txns, &config)?)
}

/// Ask the Python prices service to fetch+cache+export a symbol. No-op when the
/// service isn't configured (synthetic mode).
async fn ensure_prices(app: &AppState, ticker: &str) -> Result<(), ApiError> {
    let Some(url) = &app.prices_url else {
        return Ok(());
    };
    let body = serde_json::json!({ "symbols": [ticker.to_uppercase()] });
    let resp = reqwest::Client::new()
        .post(format!("{url}/ensure"))
        .json(&body)
        .send()
        .await
        .map_err(|e| ApiError::BadRequest(format!("prices service unreachable: {e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::BadRequest(format!(
            "price fetch failed: HTTP {}",
            resp.status()
        )));
    }
    Ok(())
}
