use std::collections::{BTreeMap, HashMap};

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use ptf_engine::{
    Currency, ExerciseStyle, FxRateProvider, HistoricalPriceProvider, Instrument, InstrumentId,
    InstrumentKind, Money, Portfolio, PortfolioConfig, PortfolioId, Transaction, TransactionId,
    TransactionKind, UserId, fold,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::Deserialize;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::PeerIpKeyExtractor;
use uuid::Uuid;

use crate::auth::{self, SessionUser};
use crate::dto::{
    AddHoldingReq, AddOptionReq, CreatePortfolioReq, PortfolioDetail, PortfolioSummary,
    PositionView, SellHoldingReq, parse_currency, parse_lot_method,
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
        .route("/api/portfolios/{id}/options", post(add_option))
        .route("/api/portfolios/{id}/sell", post(sell_holding))
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

/// The OCC contract symbol: root padded to six, `YYMMDD`, `C`/`P`, then the
/// strike in thousandths padded to eight.
///
/// Deliberately the same format the ingest writes into `raw_symbol`, so a
/// position in the book can be matched against a row in the option chain
/// without a translation table.
fn occ_symbol(root: &str, expiry: chrono::NaiveDate, right: ptf_engine::vol::OptionRight,
              strike: Decimal) -> String {
    use chrono::Datelike;
    let thousandths = (strike * Decimal::from(1000)).round().to_i64().unwrap_or(0);
    format!(
        "{:<6}{:02}{:02}{:02}{}{:08}",
        root.to_uppercase(),
        expiry.year() % 100,
        expiry.month(),
        expiry.day(),
        right,
        thousandths
    )
}

fn parse_right(s: &str) -> Result<ptf_engine::vol::OptionRight, ApiError> {
    match s.to_ascii_lowercase().as_str() {
        "call" | "c" => Ok(ptf_engine::vol::OptionRight::Call),
        "put" | "p" => Ok(ptf_engine::vol::OptionRight::Put),
        other => Err(ApiError::BadRequest(format!(
            "right must be call or put, got {other}"
        ))),
    }
}

fn parse_exercise(s: Option<&str>) -> Result<ExerciseStyle, ApiError> {
    match s.map(str::to_ascii_lowercase).as_deref() {
        None | Some("american") => Ok(ExerciseStyle::American),
        Some("european") => Ok(ExerciseStyle::European),
        Some(other) => Err(ApiError::BadRequest(format!(
            "exercise must be american or european, got {other}"
        ))),
    }
}

/// Buy a listed option.
///
/// Long, because most of it is validation: an option has five contract terms
/// that each have a way of being wrong, and each deserves its own message.
///
/// Mirrors [`add_holding`]'s deposit-then-buy desugaring, with two differences
/// that matter. The instrument is registered with its full contract terms, so
/// the risk engine can revalue it through a surface instead of shocking it like
/// a stock. And the underlying is registered and price-ensured too, because an
/// option is priced as a function of its underlying and the risk run needs that
/// history whether or not the underlying is itself held.
#[allow(clippy::too_many_lines)]
async fn add_option(
    user: SessionUser,
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AddOptionReq>,
) -> Result<Json<PortfolioSummary>, ApiError> {
    let pid = parse_id(&id)?;
    let portfolio = owned_portfolio(&app, user.0.id, pid).await?;
    let ccy = parse_currency(&req.currency)?;
    let right = parse_right(&req.right)?;
    let exercise = parse_exercise(req.exercise.as_deref())?;
    let multiplier = req.multiplier.unwrap_or_else(|| Decimal::from(100));

    if req.contracts <= Decimal::ZERO {
        return Err(ApiError::BadRequest(
            "contracts must be positive — writing options is not supported here".into(),
        ));
    }
    if req.strike <= Decimal::ZERO {
        return Err(ApiError::BadRequest("strike must be positive".into()));
    }
    if req.premium <= Decimal::ZERO {
        return Err(ApiError::BadRequest("premium must be positive".into()));
    }
    if multiplier <= Decimal::ZERO {
        return Err(ApiError::BadRequest("multiplier must be positive".into()));
    }
    let date = req.date.unwrap_or(portfolio.inception_date);
    if req.expiry <= date {
        return Err(ApiError::BadRequest(format!(
            "expiry {} must be after the trade date {date}",
            req.expiry
        )));
    }

    // The underlying has to exist as an instrument and have price history: the
    // risk run drives the option off the underlying's returns, not the
    // option's own.
    ensure_prices(&app, &req.underlying).await?;
    let underlying_id = if let Ok(existing) = app.instruments.by_symbol(&req.underlying).await {
        existing.id
    } else {
        let inst = Instrument {
            id: InstrumentId::new(),
            symbol: req.underlying.clone(),
            name: req.underlying.clone(),
            currency: ccy,
            kind: InstrumentKind::Equity {},
        };
        app.instruments.upsert(&inst).await?;
        inst.id
    };

    // Refuse a position the risk engine could not price. Accepting it would
    // leave the book in a state where /risk fails outright, since an option
    // without a surface is an error rather than a skipped row — under-reporting
    // risk silently is the worse failure.
    let dir = std::env::var("PTF_SURFACES").unwrap_or_else(|_| "services/prices/data".into());
    let files = crate::surface_source::SurfaceFiles::in_dir(std::path::Path::new(&dir));
    let roots = HashMap::from([(req.underlying.clone(), underlying_id)]);
    let missing = files.missing();
    if !missing.is_empty() {
        // The files are absent, which is a different problem from this
        // underlying not being fitted: naming the directory turns a puzzling
        // message into an obvious one when the API is running somewhere the
        // relative default does not resolve, such as a container.
        return Err(ApiError::BadRequest(format!(
            "no surface artifacts in {} (missing: {}) — run ptf-surface, or point \
             PTF_SURFACES at the directory holding them",
            files.search_dir(),
            missing.join(", ")
        )));
    }
    let surfaces = load_surfaces(&app, &roots, Utc::now().date_naive()).await;
    if surfaces.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "surface artifacts in {} contain no fitted surface for {} — ingest its \
             option chains and re-run ptf-surface",
            files.search_dir(),
            req.underlying
        )));
    }

    let symbol = occ_symbol(&req.underlying, req.expiry, right, req.strike);
    let inst_id = if let Ok(existing) = app.instruments.by_symbol(&symbol).await {
        existing.id
    } else {
        let inst = Instrument {
            id: InstrumentId::new(),
            symbol: symbol.clone(),
            name: format!(
                "{} {} {} {}",
                req.underlying.to_uppercase(),
                req.expiry,
                right,
                req.strike.normalize()
            ),
            currency: ccy,
            kind: InstrumentKind::EquityOption {
                underlying: underlying_id,
                right,
                strike: req.strike,
                expiry: req.expiry,
                multiplier,
                exercise,
            },
        };
        app.instruments.upsert(&inst).await?;
        inst.id
    };

    // Quotes are per share; a contract costs the premium times the multiplier,
    // and the position is measured in contracts. Getting this pairing wrong is
    // the hundred-fold error the multiplier exists to prevent.
    let cost_per_contract = req.premium * multiplier;
    let cost = Money::new(cost_per_contract, ccy);
    let zero = Money::new(Decimal::ZERO, ccy);
    let deposit = TransactionKind::deposit(Money::new(req.contracts * cost_per_contract, ccy))?;
    let buy = TransactionKind::buy(inst_id, req.contracts, cost, zero, None)?;
    let dep_tx = Transaction::new(TransactionId::new(), date, date, deposit)?;
    let buy_tx = Transaction::new(TransactionId::new(), date, date, buy)?;
    app.transactions.append(pid, &dep_tx).await?;
    app.transactions.append(pid, &buy_tx).await?;

    Ok(Json(PortfolioSummary::from(&portfolio)))
}

/// Sell some or all of a position.
///
/// The mirror of [`add_holding`]'s deposit+buy desugaring: a `Sell` plus a
/// `Withdrawal` of the proceeds, so cash does not accumulate in a book that
/// only ever models securities.
///
/// The transaction log is append-only, so this is how a position is "removed":
/// selling the full quantity leaves no open lots and the position stops
/// appearing in the folded state.
async fn sell_holding(
    user: SessionUser,
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SellHoldingReq>,
) -> Result<Json<PortfolioSummary>, ApiError> {
    let pid = parse_id(&id)?;
    let portfolio = owned_portfolio(&app, user.0.id, pid).await?;
    if req.quantity <= Decimal::ZERO {
        return Err(ApiError::BadRequest("quantity must be positive".into()));
    }
    if req.price <= Decimal::ZERO {
        return Err(ApiError::BadRequest("price must be positive".into()));
    }

    let instrument = app
        .instruments
        .by_symbol(&req.ticker)
        .await
        .map_err(|_| ApiError::BadRequest(format!("unknown holding: {}", req.ticker)))?;

    let date = req.date.unwrap_or_else(|| Utc::now().date_naive());

    // Check availability *as of the trade date*, not today: this rejects both
    // overselling and a sale dated before the shares were acquired, either of
    // which would otherwise be appended and then break every later fold.
    let prior: Vec<Transaction> = app.transactions.list_until(pid, date).await?;
    let config = PortfolioConfig::new(portfolio.lot_method, portfolio.base_currency);
    let held = fold(&prior, &config)?
        .positions()
        .get(&instrument.id)
        .map_or(Decimal::ZERO, ptf_engine::Position::total_long_quantity);
    if held < req.quantity {
        return Err(ApiError::BadRequest(format!(
            "cannot sell {} {} on {date} — only {held} held",
            req.quantity, req.ticker
        )));
    }

    let ccy = instrument.currency;
    let price = Money::new(req.price, ccy);
    let zero = Money::new(Decimal::ZERO, ccy);

    // Desugar: sell, then withdraw the proceeds.
    let sell = TransactionKind::sell(instrument.id, req.quantity, price, zero, None)?;
    let withdrawal =
        TransactionKind::withdrawal(Money::new(req.quantity * req.price, ccy))?;
    let sell_tx = Transaction::new(TransactionId::new(), date, date, sell)?;
    let wd_tx = Transaction::new(TransactionId::new(), date, date, withdrawal)?;
    app.transactions.append(pid, &sell_tx).await?;
    app.transactions.append(pid, &wd_tx).await?;

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
    let (pd, surfaces) =
        build_prices(&app, &holdings, portfolio.base_currency, as_of, lookback).await?;
    let payload =
        risk_view::build(&portfolio, &state, &holdings, &names, &pd, &surfaces, as_of)?;
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
    let (pd, _surfaces) =
        build_prices(&app, &holdings, portfolio.base_currency, as_of, lookback).await?;
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
    // Options are marked from the surface here too, so the performance page
    // values the same book the positions page does.
    let (pd, _surfaces) =
        build_prices(&app, &holdings, portfolio.base_currency, as_of, lookback).await?;

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
    // Ensure-on-read so the benchmark is available even if never held — but
    // rate-limited, because this is a GET: an unconditional ensure put a
    // cross-service round trip (and a database write) on every page load.
    // Ignore failures; the caller degrades to no benchmark.
    if app.should_ensure(symbol) {
        let _ = ensure_prices(app, symbol).await;
    }

    let bid = InstrumentId::new();
    let holdings = vec![HeldInstrument {
        id: bid,
        symbol: symbol.to_string(),
        currency: Currency::USD,
        kind: InstrumentKind::Equity {},
    }];
    let Ok(pd) = app.prices.build(&holdings, base, as_of, lookback).await else {
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
    let mut underlyings: Vec<InstrumentId> = Vec::new();
    for inst_id in state.positions().keys() {
        if let Ok(inst) = app.instruments.get(*inst_id).await {
            names.insert(inst.symbol.clone(), inst.name.clone());
            if let Some(u) = inst.kind.underlying() {
                underlyings.push(u);
            }
            holdings.push(HeldInstrument {
                id: inst.id,
                symbol: inst.symbol,
                currency: inst.currency,
                kind: inst.kind,
            });
        }
    }
    // The underlying of a held option is not itself a position, but the risk
    // run needs its price history (an option is driven by its underlying's
    // returns, not its own) and its surface is keyed by that ticker. It only
    // adds metadata: rows are built from the folded positions, so this cannot
    // create a phantom holding.
    for u in underlyings {
        if holdings.iter().any(|h| h.id == u) {
            continue;
        }
        if let Ok(inst) = app.instruments.get(u).await {
            names.insert(inst.symbol.clone(), inst.name.clone());
            holdings.push(HeldInstrument {
                id: inst.id,
                symbol: inst.symbol,
                currency: inst.currency,
                kind: inst.kind,
            });
        }
    }
    (holdings, names)
}

/// Build the price data for a book, marking any options from their surface.
///
/// Options are filtered out of the feed request rather than passed to it: a
/// listed contract has no row in the price file and never will, so asking for
/// one is an error by construction. Their underlyings are still in the list,
/// which is what the risk run actually needs.
async fn build_prices(
    app: &AppState,
    holdings: &[HeldInstrument],
    base: Currency,
    as_of: chrono::NaiveDate,
    lookback: u32,
) -> Result<(crate::price_source::PriceData, ptf_engine::StaticVolSurfaceProvider), ApiError> {
    let feed: Vec<HeldInstrument> = holdings
        .iter()
        .filter(|h| !h.kind.is_derivative())
        .cloned()
        .collect();
    let mut pd = app.prices.build(&feed, base, as_of, lookback).await?;
    let surfaces = mark_options(app, holdings, &mut pd, as_of).await;
    Ok((pd, surfaces))
}

/// Load the fitted surfaces for whatever underlyings this book needs, and mark
/// every option position from them.
///
/// Marking here rather than in each view is what keeps the book consistent: the
/// positions page, the portfolio value and the risk run all read the same
/// `PriceProvider`, so they cannot disagree about what an option is worth. The
/// mark is the same model that revalues it on every simulated path, so a
/// reported P&L is risk rather than risk plus a mark-to-model gap.
/// Fitted surfaces from `vol.*` when a database is configured, else from the
/// parquet artifacts. The database is pinned to `vol.current_run`, so a build
/// in progress cannot be read half-finished.
async fn load_surfaces(
    app: &AppState,
    roots: &HashMap<String, InstrumentId>,
    as_of: chrono::NaiveDate,
) -> ptf_engine::StaticVolSurfaceProvider {
    if let Some(pool) = app.pool.as_ref() {
        return crate::surface_source::load_postgres(pool, roots, as_of).await;
    }
    let dir = std::env::var("PTF_SURFACES").unwrap_or_else(|_| "services/prices/data".into());
    let files = crate::surface_source::SurfaceFiles::in_dir(std::path::Path::new(&dir));
    crate::surface_source::load(&files, roots, as_of)
}

async fn mark_options(
    app: &AppState,
    holdings: &[HeldInstrument],
    pd: &mut crate::price_source::PriceData,
    as_of: chrono::NaiveDate,
) -> ptf_engine::StaticVolSurfaceProvider {
    use ptf_engine::PriceProvider;
    use ptf_engine::surface::VolSurfaceProvider;

    let roots: HashMap<String, InstrumentId> = holdings
        .iter()
        .filter(|h| !h.kind.is_derivative())
        .map(|h| (h.symbol.clone(), h.id))
        .collect();
    let surfaces = load_surfaces(app, &roots, as_of).await;

    for h in holdings.iter().filter(|h| h.kind.is_derivative()) {
        let InstrumentKind::EquityOption { underlying, right, .. } = h.kind else { continue };
        let (Some(strike), Some(tte)) = (h.kind.strike_f64(), h.kind.year_fraction(as_of)) else {
            continue;
        };
        let Some(snapshot) = surfaces.surface(underlying, as_of) else { continue };
        let Some(per_contract) = snapshot.price_contract(right, strike, tte, 1.0, &[]) else {
            continue;
        };
        // A surface fitted for one price level against a feed quoting another
        // is a silent misvaluation: the option marks off the surface's forward
        // while the stock marks off the feed, so a mixed book is sized wrong.
        // It is loud rather than fatal because the *risk* still works -- the
        // underlying drives the option through a return, which is scale-free.
        if let (Ok(spot), Some(front)) = (
            pd.prices.price(underlying, as_of),
            snapshot.forwards.first().map(|f| f.1),
        ) {
            let spot = spot.amount.to_f64().unwrap_or(0.0);
            if spot > 0.0 && (front / spot).max(spot / front) > 1.25 {
                tracing::warn!(
                    symbol = %h.symbol,
                    spot,
                    forward = front,
                    "price feed and volatility surface disagree about the underlying's level"
                );
            }
        }
        let mult = h.kind.multiplier().to_f64().unwrap_or(1.0);
        if let Some(amount) = Decimal::from_f64_retain(per_contract * mult) {
            pd.prices
                .insert(h.id, as_of, Money::new(amount.round_dp(6), h.currency));
        }
    }
    surfaces
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

#[cfg(test)]
mod tests {
    use super::*;
    use ptf_engine::vol::OptionRight;

    fn day(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Pinned against real rows from the option store, so a position in the
    /// book can be matched to a row in the chain without translation.
    #[test]
    fn occ_symbols_match_the_ingested_chain() {
        assert_eq!(
            occ_symbol("SOXX", day(2026, 7, 17), OptionRight::Call, Decimal::from(520)),
            "SOXX  260717C00520000"
        );
        assert_eq!(
            occ_symbol("SOXX", day(2025, 8, 29), OptionRight::Call, Decimal::from(160)),
            "SOXX  250829C00160000"
        );
        // Fractional strikes survive the thousandths encoding.
        assert_eq!(
            occ_symbol("spy", day(2026, 1, 16), OptionRight::Put, Decimal::from_str_exact("522.50").unwrap()),
            "SPY   260116P00522500"
        );
        // A long root fills the field without shifting the tail.
        assert_eq!(
            occ_symbol("GOOGL", day(2026, 3, 20), OptionRight::Call, Decimal::from(200)),
            "GOOGL 260320C00200000"
        );
    }

    #[test]
    fn right_and_exercise_parse_forgivingly_but_not_loosely() {
        assert_eq!(parse_right("call").unwrap(), OptionRight::Call);
        assert_eq!(parse_right("C").unwrap(), OptionRight::Call);
        assert_eq!(parse_right("Put").unwrap(), OptionRight::Put);
        assert_eq!(parse_right("p").unwrap(), OptionRight::Put);
        assert!(parse_right("straddle").is_err());
        assert!(parse_right("").is_err());

        assert_eq!(parse_exercise(None).unwrap(), ExerciseStyle::American);
        assert_eq!(parse_exercise(Some("EUROPEAN")).unwrap(), ExerciseStyle::European);
        assert!(parse_exercise(Some("bermudan")).is_err());
    }
}
