use std::collections::{BTreeMap, HashMap};

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use ptf_engine::{
    Currency, FxRateProvider, HistoricalPriceProvider, Instrument, InstrumentId, InstrumentKind,
    Money, Portfolio, PortfolioConfig, PortfolioId, Transaction, TransactionId, TransactionKind,
    UserId, fold,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::Deserialize;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::PeerIpKeyExtractor;
use uuid::Uuid;

use crate::auth::{self, SessionUser};
use crate::dto::{
    AddHoldingReq, CreatePortfolioReq, PortfolioDetail, PortfolioSummary, PositionView,
    parse_currency, parse_lot_method,
};
use crate::error::ApiError;
use crate::perf_view::{self, BenchmarkSeries, PerformancePayload};
use crate::positions_view::{self, PositionsPayload};
use crate::price_source::HeldInstrument;
use crate::risk_view::{self, RiskPayload};
use crate::state::AppState;

/// Default benchmark for the performance tab's relative stats.
const DEFAULT_BENCHMARK: &str = "SPY";

pub fn router(state: AppState) -> Router {
    // Login and register are rate-limited per client IP against brute force.
    let limited_auth = Router::new()
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/register", post(auth::register))
        .layer(tower_governor::GovernorLayer::new(
            GovernorConfigBuilder::default()
                .per_second(60)
                .burst_size(5)
                .key_extractor(PeerIpKeyExtractor)
                .finish()
                .expect("valid governor config"),
        ));

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/logout", post(auth::logout))
        .merge(limited_auth)
        .route(
            "/api/portfolios",
            get(list_portfolios).post(create_portfolio),
        )
        .route("/api/portfolios/{id}", get(get_portfolio))
        .route("/api/portfolios/{id}/holdings", post(add_holding))
        .route("/api/portfolios/{id}/positions", get(get_positions))
        .route("/api/portfolios/{id}/risk", get(get_risk))
        .route("/api/portfolios/{id}/performance", get(get_performance))
        .with_state(state)
}

fn parse_id(id: &str) -> Result<PortfolioId, ApiError> {
    Uuid::parse_str(id)
        .map(PortfolioId)
        .map_err(|_| ApiError::BadRequest(format!("invalid portfolio id: {id}")))
}

/// Loads the portfolio, requiring ownership: 404 when it is missing **or**
/// owned by someone else (no cross-tenant enumeration).
async fn owned_portfolio(
    app: &AppState,
    user_id: UserId,
    pid: PortfolioId,
) -> Result<Portfolio, ApiError> {
    let portfolio = app.portfolios.get(pid).await?;
    if portfolio.user_id != user_id {
        return Err(ApiError::NotFound);
    }
    Ok(portfolio)
}

async fn list_portfolios(
    user: SessionUser,
    State(app): State<AppState>,
) -> Result<Json<Vec<PortfolioSummary>>, ApiError> {
    let list = app.portfolios.list(user.0.id).await?;
    Ok(Json(list.iter().map(PortfolioSummary::from).collect()))
}

async fn create_portfolio(
    user: SessionUser,
    State(app): State<AppState>,
    Json(req): Json<CreatePortfolioReq>,
) -> Result<Json<PortfolioSummary>, ApiError> {
    let base = parse_currency(&req.base_ccy)?;
    let lot_method = parse_lot_method(&req.lot_method)?;
    let inception = req
        .inception_date
        .unwrap_or_else(|| Utc::now().date_naive());
    let portfolio = Portfolio::new(
        PortfolioId::new(),
        user.0.id,
        req.name,
        base,
        lot_method,
        inception,
    );
    app.portfolios.create(&portfolio).await?;
    Ok(Json(PortfolioSummary::from(&portfolio)))
}

async fn get_portfolio(
    user: SessionUser,
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PortfolioDetail>, ApiError> {
    let pid = parse_id(&id)?;
    let portfolio = owned_portfolio(&app, user.0.id, pid).await?;
    let state = fold_state(&app, &portfolio).await?;

    let mut positions = Vec::new();
    for (inst_id, pos) in state.positions() {
        let inst = app.instruments.get(*inst_id).await.ok();
        positions.push(PositionView {
            ticker: inst
                .as_ref()
                .map_or_else(|| inst_id.0.to_string(), |i| i.symbol.clone()),
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
    user: SessionUser,
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AddHoldingReq>,
) -> Result<Json<PortfolioSummary>, ApiError> {
    let pid = parse_id(&id)?;
    let portfolio = owned_portfolio(&app, user.0.id, pid).await?;
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
    let inst_id = if let Ok(existing) = app.instruments.by_symbol(&req.ticker).await {
        existing.id
    } else {
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
    user: SessionUser,
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RiskPayload>, ApiError> {
    let pid = parse_id(&id)?;
    let portfolio = owned_portfolio(&app, user.0.id, pid).await?;
    let state = fold_state(&app, &portfolio).await?;
    let (holdings, names) = gather_holdings(&app, &state).await;

    let as_of = Utc::now().date_naive();
    let lookback = ptf_engine::MonteCarloConfig::default_var().lookback_days;
    let pd = app
        .prices
        .build(&holdings, portfolio.base_currency, as_of, lookback)?;
    let payload = risk_view::build(&portfolio, &state, &holdings, &names, &pd, as_of)?;
    Ok(Json(payload))
}

async fn get_positions(
    user: SessionUser,
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PositionsPayload>, ApiError> {
    let pid = parse_id(&id)?;
    let portfolio = owned_portfolio(&app, user.0.id, pid).await?;
    let state = fold_state(&app, &portfolio).await?;
    let (holdings, names) = gather_holdings(&app, &state).await;

    let as_of = Utc::now().date_naive();
    // Positions only need spot + FX; reuse the same lookback so parquet/synthetic
    // sources have a valid window to source the latest close from.
    let lookback = ptf_engine::MonteCarloConfig::default_var().lookback_days;
    let pd = app
        .prices
        .build(&holdings, portfolio.base_currency, as_of, lookback)?;
    let payload = positions_view::build(&portfolio, &state, &holdings, &names, &pd, as_of)?;
    Ok(Json(payload))
}

#[derive(Debug, Deserialize)]
struct PerfQuery {
    /// Annual risk-free rate as a fraction (e.g. `0.04`). Defaults to 0.
    rf: Option<f64>,
    /// Benchmark ticker for relative stats. Defaults to [`DEFAULT_BENCHMARK`].
    benchmark: Option<String>,
}

async fn get_performance(
    user: SessionUser,
    State(app): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PerfQuery>,
) -> Result<Json<PerformancePayload>, ApiError> {
    let pid = parse_id(&id)?;
    let portfolio = owned_portfolio(&app, user.0.id, pid).await?;
    let state = fold_state(&app, &portfolio).await?;
    let (holdings, _names) = gather_holdings(&app, &state).await;

    let as_of = Utc::now().date_naive();
    let lookback = ptf_engine::MonteCarloConfig::default_var().lookback_days;
    let pd = app
        .prices
        .build(&holdings, portfolio.base_currency, as_of, lookback)?;

    // Benchmark is best-effort: if it can't be fetched/priced we still return the
    // self-contained ratios, just without the `relative` block.
    let bench_symbol = q
        .benchmark
        .unwrap_or_else(|| DEFAULT_BENCHMARK.to_string())
        .to_uppercase();
    let benchmark = benchmark_series(
        &app,
        portfolio.base_currency,
        as_of,
        lookback,
        &bench_symbol,
    )
    .await
    .ok()
    .flatten();

    let rf = q.rf.unwrap_or(0.0);
    let payload = perf_view::build(
        &portfolio,
        &state,
        &pd,
        as_of,
        lookback,
        rf,
        benchmark.as_ref(),
    )?;
    Ok(Json(payload))
}

/// Fetch + price a benchmark symbol and return its base-currency value on each
/// historical date. `Ok(None)` when the price source has no data for it.
async fn benchmark_series(
    app: &AppState,
    base: Currency,
    as_of: chrono::NaiveDate,
    lookback: u32,
    symbol: &str,
) -> Result<Option<BenchmarkSeries>, ApiError> {
    // Ensure-on-read so the benchmark is available even if never held. Ignore
    // failures — the caller degrades to no benchmark.
    let _ = ensure_prices(app, symbol).await;

    let bid = InstrumentId::new();
    let holdings = vec![HeldInstrument {
        id: bid,
        symbol: symbol.to_string(),
        currency: Currency::USD,
    }];
    let Ok(pd) = app.prices.build(&holdings, base, as_of, lookback) else {
        return Ok(None);
    };
    let fx = pd
        .fx
        .rate(Currency::USD, base, as_of)
        .ok()
        .and_then(|r| r.to_f64())
        .unwrap_or(1.0);
    let from = as_of - Duration::days(i64::from(lookback));
    let Ok(hist) = pd.historical.prices(bid, from, as_of) else {
        return Ok(None);
    };
    let values: BTreeMap<_, _> = hist
        .iter()
        .map(|(d, m)| (*d, m.amount.to_f64().unwrap_or(0.0) * fx))
        .collect();
    if values.is_empty() {
        return Ok(None);
    }
    Ok(Some(BenchmarkSeries {
        symbol: symbol.to_string(),
        values,
    }))
}

/// Held-instrument metadata + a ticker→name map for the instruments in `state`.
async fn gather_holdings(
    app: &AppState,
    state: &ptf_engine::PortfolioState,
) -> (Vec<HeldInstrument>, HashMap<String, String>) {
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
    (holdings, names)
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
///
/// The service answers HTTP 200 even for an unknown ticker, reporting the
/// failure per-symbol in the body (`{"symbols": {"XYZ": {"error": ...}}}`). We
/// must surface that here as a `BadRequest`, because `add_holding` calls this
/// *before* persisting: if a bad ticker slipped through, it would be saved with
/// no price data and every later `/positions` and `/risk` load would 400.
async fn ensure_prices(app: &AppState, ticker: &str) -> Result<(), ApiError> {
    let Some(url) = &app.prices_url else {
        return Ok(());
    };
    let symbol = ticker.to_uppercase();
    let body = serde_json::json!({ "symbols": [symbol] });
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

    let payload: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Internal(format!("bad prices response: {e}")))?;
    let entry = payload.get("symbols").and_then(|s| s.get(&symbol));
    match entry {
        // A real fetch (fresh or cached) has no "error" key.
        Some(e) if e.get("error").is_none() => Ok(()),
        _ => Err(ApiError::BadRequest(format!(
            "ticker '{symbol}' not found — check the symbol and try again"
        ))),
    }
}
