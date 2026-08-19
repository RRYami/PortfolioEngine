//! Maps the engine's `VaRReport` + `PortfolioState` into the dashboard JSON
//! contract (mirrors `frontend/app/lib/riskTypes.ts`).
//!
//! Headline VaR/ES, component `VaR`, positions and portfolio value come from the
//! real engine. The three chart series (distribution, drawdown, historical `VaR`)
//! are still synthetic in Phase 1 — see `charts.rs` — and flagged accordingly.

use chrono::NaiveDate;
use ptf_engine::{
    FxRateProvider, MonteCarloConfig, Portfolio, PortfolioState, PriceProvider, VaRReport,
    compute_var,
};
use rust_decimal::prelude::ToPrimitive;
use serde::Serialize;

use crate::charts;
use crate::equity;
use crate::error::ApiError;
use crate::price_source::{HeldInstrument, PriceData};

/// Number of trailing days shown in the drawdown / historical-VaR charts.
const CHART_DAYS: usize = 180;

/// A value provided for both confidence levels, serialized as `{ "95":…, "99":… }`.
#[derive(Debug, Serialize)]
pub struct ByConf<T> {
    #[serde(rename = "95")]
    pub c95: T,
    #[serde(rename = "99")]
    pub c99: T,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskPayload {
    pub as_of: String,
    pub base_ccy: String,
    pub portfolio_value: f64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    /// Today's move on the current book, in percent. `None` when the window
    /// has fewer than two points to compare.
    pub today_return_pct: Option<f64>,
    pub ann_vol_pct: f64,
    pub positions: Vec<PositionRow>,
    pub drawdown: Drawdown,
    pub hist_var: HistVar,
    pub risk: ByConf<RiskBlock>,
    pub pnl_distribution: PnlDistribution,
    /// Phase 1 marker: the three chart series are synthetic until the engine
    /// exposes them (plan Phase 3).
    pub synthetic_charts: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionRow {
    pub ticker: String,
    pub name: String,
    pub ccy: String,
    pub qty: f64,
    pub last: f64,
    pub market_value: f64,
    pub weight_pct: f64,
    pub unrealized_pnl: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskBlock {
    pub var1d: f64,
    pub var1d_pct: f64,
    pub var20d: f64,
    pub var20d_pct: f64,
    pub es1d: f64,
    pub es1d_pct: f64,
    pub component_var: Vec<ComponentVar>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentVar {
    pub ticker: String,
    pub value: f64,
    pub pct_of_var: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drawdown {
    pub max_pct: f64,
    pub dates: Vec<String>,
    pub series: Vec<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistVar {
    pub dates: Vec<String>,
    pub var1d_pct: ByConf<Vec<f64>>,
    pub var20d_pct: ByConf<Vec<f64>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PnlDistribution {
    pub paths: usize,
    pub bin_low: f64,
    pub bin_high: f64,
    pub bin_count: usize,
    pub counts: Vec<i64>,
    pub cutoffs: ByConf<Cutoffs>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cutoffs {
    pub var: f64,
    pub es: f64,
    pub deep: f64,
}

fn f(x: rust_decimal::Decimal) -> f64 {
    x.to_f64().unwrap_or(0.0)
}

/// Find an entry's (var, cvar) in base currency for a (confidence, horizon).
fn entry(report: &VaRReport, conf: f64, horizon: u32) -> (f64, f64) {
    report
        .entries
        .iter()
        .find(|e| {
            (e.confidence.to_f64().unwrap_or(0.0) - conf).abs() < 1e-9 && e.horizon_days == horizon
        })
        .map_or((0.0, 0.0), |e| {
            (f(e.portfolio_var.amount), f(e.portfolio_cvar.amount))
        })
}

/// Build the dashboard payload. `holdings` carries symbol/name/ccy metadata for
/// the instruments held in `state`.
pub fn build(
    portfolio: &Portfolio,
    state: &PortfolioState,
    holdings: &[HeldInstrument],
    names: &std::collections::HashMap<String, String>,
    pd: &PriceData,
    as_of: NaiveDate,
) -> Result<RiskPayload, ApiError> {
    let base = portfolio.base_currency;
    let cfg = MonteCarloConfig::default_var();
    let report = compute_var(state, &pd.historical, &pd.fx, &pd.prices, &cfg, base, as_of)?;

    let total = f(state.total_value(&pd.fx, &pd.prices, base, as_of)?.amount);

    // ---- positions ----
    let by_id: std::collections::HashMap<_, _> = holdings.iter().map(|h| (h.id, h)).collect();
    let mut rows: Vec<PositionRow> = Vec::new();
    let mut total_mv = 0.0;
    for (inst_id, pos) in state.positions() {
        let meta = by_id.get(inst_id);
        let ccy = pos.currency();
        let qty = f(pos.net_quantity());
        let price = f(pd.prices.price(*inst_id, as_of)?.amount);
        let fx = f(pd.fx.rate(ccy, base, as_of)?);
        let mv_native = qty * price;
        let mv_base = mv_native * fx;
        // Cost per lot at its own trade-date rate, matching the positions page:
        // the two views must not disagree about the same book's P&L.
        let mut cost_base = 0.0;
        for lot in pos.long_lots() {
            let lot_fx = f(pd.fx_trade_date.rate(ccy, base, lot.open_date())?);
            cost_base += f(lot.quantity()) * f(lot.basis_per_unit().amount) * lot_fx;
        }
        let upnl_base = mv_base - cost_base;
        total_mv += mv_base;
        let ticker = meta.map_or_else(|| inst_id.0.to_string(), |m| m.symbol.clone());
        let name = names
            .get(&ticker)
            .cloned()
            .unwrap_or_else(|| ticker.clone());
        rows.push(PositionRow {
            ticker,
            name,
            ccy: ccy.to_string(),
            qty,
            last: price,
            market_value: mv_base,
            weight_pct: 0.0, // filled below
            unrealized_pnl: upnl_base,
        });
    }
    for r in &mut rows {
        r.weight_pct = if total_mv == 0.0 {
            0.0
        } else {
            (r.market_value / total_mv * 100.0 * 10.0).round() / 10.0
        };
    }
    rows.sort_by(|a, b| b.market_value.partial_cmp(&a.market_value).unwrap());
    let unrealized_pnl: f64 = rows.iter().map(|r| r.unrealized_pnl).sum();

    // realized pnl across currencies → base
    let mut realized_pnl = 0.0;
    for (&ccy, &amt) in state.realized_pnl() {
        let fx = pd.fx.rate(ccy, base, as_of).map_or(1.0, f);
        realized_pnl += f(amt) * fx;
    }

    // ---- headline risk per confidence ----
    let (var1d95, es1d95) = entry(&report, 0.95, 1);
    let (var20d95, _) = entry(&report, 0.95, 20);
    let (var1d99, es1d99) = entry(&report, 0.99, 1);
    let (var20d99, _) = entry(&report, 0.99, 20);

    // component VaR shares (from component CVaR), scaled to each conf's var1d.
    let shares = component_shares(&report, &by_id);
    let risk = ByConf {
        c95: risk_block(&shares, var1d95, var20d95, es1d95, total),
        c99: risk_block(&shares, var1d99, var20d99, es1d99, total),
    };

    // annualized vol proxy from the 95% 1-day VaR (z=1.645).
    let var1d_pct95 = pct(var1d95, total);
    let ann_vol_pct = (var1d_pct95 / 1.645) * (252.0_f64).sqrt();

    // ---- real chart series ----
    // P&L distribution from the engine's Monte-Carlo sample.
    let pnl_distribution =
        charts::pnl_distribution(&report.pnl_1d, (var1d95, es1d95), (var1d99, es1d99));
    // Drawdown + historical VaR from the portfolio's equity curve over the window.
    let (equity, date_vec) =
        equity::series(state, pd, base, as_of, cfg.lookback_days, Some(CHART_DAYS))?;
    let dates = equity::iso(&date_vec);
    // Today's move, from the last two points of that same curve. This is the
    // *current* book's move (the curve values today's holdings backwards), not
    // the return of the book as it stood yesterday; they differ if you traded
    // today. `None` rather than 0.0 when there is nothing to compare against —
    // a genuinely flat day and an uncomputable one must not look identical.
    let today_return_pct = match equity.as_slice() {
        [.., prev, last] if *prev != 0.0 => Some((last / prev - 1.0) * 100.0),
        _ => None,
    };
    let drawdown = charts::drawdown(&equity, &dates);
    let hist_var = charts::historical_var(&equity, &dates, total, var1d95, var1d99);

    Ok(RiskPayload {
        as_of: format!("{as_of}T16:00:00-04:00"),
        base_ccy: base.to_string(),
        portfolio_value: total,
        realized_pnl,
        unrealized_pnl,
        today_return_pct,
        ann_vol_pct,
        positions: rows,
        drawdown,
        hist_var,
        risk,
        pnl_distribution,
        synthetic_charts: false,
    })
}

fn pct(x: f64, total: f64) -> f64 {
    if total == 0.0 { 0.0 } else { x / total * 100.0 }
}

struct Share {
    ticker: String,
    share: f64,
}

fn component_shares(
    report: &VaRReport,
    by_id: &std::collections::HashMap<ptf_engine::InstrumentId, &HeldInstrument>,
) -> Vec<Share> {
    let sum: f64 = report
        .per_asset
        .iter()
        .map(|a| f(a.component_cvar.amount))
        .sum();
    let mut v: Vec<Share> = report
        .per_asset
        .iter()
        .map(|a| Share {
            // The engine leaves `symbol` blank (PortfolioState has no symbols);
            // resolve it from the held-instrument map by id.
            ticker: by_id
                .get(&a.instrument)
                .map_or_else(|| a.symbol.clone(), |h| h.symbol.clone()),
            share: if sum == 0.0 {
                0.0
            } else {
                f(a.component_cvar.amount) / sum
            },
        })
        .collect();
    v.sort_by(|a, b| b.share.partial_cmp(&a.share).unwrap());
    v
}

fn risk_block(shares: &[Share], var1d: f64, var20d: f64, es1d: f64, total: f64) -> RiskBlock {
    let component_var = shares
        .iter()
        .map(|s| ComponentVar {
            ticker: s.ticker.clone(),
            value: s.share * var1d,
            #[allow(clippy::cast_possible_truncation)]
            pct_of_var: (s.share * 100.0).round() as i64,
        })
        .collect();
    RiskBlock {
        var1d,
        var1d_pct: pct(var1d, total),
        var20d,
        var20d_pct: pct(var20d, total),
        es1d,
        es1d_pct: pct(es1d, total),
        component_var,
    }
}
