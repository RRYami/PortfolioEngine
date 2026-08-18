//! Lightweight per-position view for the "Portfolio Positions" page.
//!
//! Unlike [`crate::risk_view`], this runs no Monte-Carlo: it just values the
//! current holdings at spot (FX-converted to base) and exposes the individual
//! tax lots behind each position for the table's drill-in.

use std::collections::HashMap;

use chrono::NaiveDate;
use ptf_engine::{FxRateProvider, Portfolio, PortfolioState, PriceProvider};
use rust_decimal::prelude::ToPrimitive;
use serde::Serialize;

use crate::error::ApiError;
use crate::price_source::{HeldInstrument, PriceData};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionsPayload {
    pub as_of: String,
    pub base_ccy: String,
    pub total_value: f64,
    pub positions: Vec<PositionDetail>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionDetail {
    pub ticker: String,
    pub name: String,
    pub ccy: String,
    pub quantity: f64,
    /// Weighted-average cost per unit, in the position's native currency.
    pub avg_cost: f64,
    /// Latest spot price, native currency.
    pub last: f64,
    /// Market value in base currency, at the spot rate.
    pub market_value: f64,
    /// Market value in the position's own currency.
    pub market_value_native: f64,
    /// Cost basis in base currency, each lot converted at **its own trade-date
    /// rate** — so the base return includes the FX leg the investor actually
    /// experienced.
    pub cost_basis: f64,
    /// Cost basis in the position's own currency.
    pub cost_basis_native: f64,
    pub weight_pct: f64,
    /// Total unrealized P&L in base currency (`price` + `fx` components).
    pub unrealized_pnl: f64,
    /// Unrealized P&L in the position's own currency — the pure price move.
    pub unrealized_pnl_native: f64,
    /// The price move, expressed in base at today's rate.
    pub unrealized_pnl_price: f64,
    /// The currency move: what the base return gains or loses purely because
    /// the rate changed between each lot's trade date and today.
    pub unrealized_pnl_fx: f64,
    pub unrealized_pnl_pct: f64,
    /// Spot rate used for market value, native → base.
    pub fx_rate: f64,
    /// Individual open long lots behind this position.
    pub lots: Vec<LotView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LotView {
    pub date: String,
    pub quantity: f64,
    /// Cost per unit at acquisition, native currency.
    pub price: f64,
    /// `quantity * price`, native currency.
    pub cost: f64,
    /// Native → base rate on this lot's trade date.
    pub fx_rate: f64,
    /// `cost` converted at that trade-date rate.
    pub cost_base: f64,
}

fn f(x: rust_decimal::Decimal) -> f64 {
    x.to_f64().unwrap_or(0.0)
}

/// Build the positions payload: current holdings valued at spot with their lots.
pub fn build(
    portfolio: &Portfolio,
    state: &PortfolioState,
    holdings: &[HeldInstrument],
    names: &HashMap<String, String>,
    pd: &PriceData,
    as_of: NaiveDate,
) -> Result<PositionsPayload, ApiError> {
    let base = portfolio.base_currency;
    let by_id: HashMap<_, _> = holdings.iter().map(|h| (h.id, h)).collect();

    let mut rows: Vec<PositionDetail> = Vec::new();
    let mut total_mv = 0.0;
    for (inst_id, pos) in state.positions() {
        let meta = by_id.get(inst_id);
        let ccy = pos.currency();
        let qty = f(pos.total_long_quantity());
        let price = f(pd.prices.price(*inst_id, as_of)?.amount);
        let fx = f(pd.fx.rate(ccy, base, as_of)?);
        let cost_native = f(pos.long_cost_basis().amount);
        let mv_native = qty * price;
        let mv_base = mv_native * fx;
        total_mv += mv_base;

        let ticker = meta.map_or_else(|| inst_id.0.to_string(), |m| m.symbol.clone());
        let name = names
            .get(&ticker)
            .cloned()
            .unwrap_or_else(|| ticker.clone());

        // Each lot converts at the rate on *its* trade date, so a book held
        // across an FX move shows the return its owner actually earned.
        let mut cost_base = 0.0;
        let mut lots: Vec<LotView> = Vec::new();
        for lot in pos.long_lots() {
            let lq = f(lot.quantity());
            let lp = f(lot.basis_per_unit().amount);
            let open = lot.open_date();
            let lot_fx = f(pd.fx_trade_date.rate(ccy, base, open)?);
            let cost = lq * lp;
            cost_base += cost * lot_fx;
            lots.push(LotView {
                date: open.format("%Y-%m-%d").to_string(),
                quantity: lq,
                price: lp,
                cost,
                fx_rate: lot_fx,
                cost_base: cost * lot_fx,
            });
        }

        let upnl_native = mv_native - cost_native;
        let upnl_base = mv_base - cost_base;
        // Split the base P&L: the price leg is the native move carried over at
        // today's rate; the rest is what the currency did.
        let upnl_price = upnl_native * fx;
        let upnl_fx = upnl_base - upnl_price;

        rows.push(PositionDetail {
            ticker,
            name,
            ccy: ccy.to_string(),
            quantity: qty,
            avg_cost: if qty == 0.0 { 0.0 } else { cost_native / qty },
            last: price,
            market_value: mv_base,
            market_value_native: mv_native,
            cost_basis: cost_base,
            cost_basis_native: cost_native,
            weight_pct: 0.0, // filled below
            unrealized_pnl: upnl_base,
            unrealized_pnl_native: upnl_native,
            unrealized_pnl_price: upnl_price,
            unrealized_pnl_fx: upnl_fx,
            unrealized_pnl_pct: if cost_base == 0.0 {
                0.0
            } else {
                upnl_base / cost_base * 100.0
            },
            fx_rate: fx,
            lots,
        });
    }

    for r in &mut rows {
        r.weight_pct = if total_mv == 0.0 {
            0.0
        } else {
            (r.market_value / total_mv * 1000.0).round() / 10.0
        };
    }
    rows.sort_by(|a, b| b.market_value.partial_cmp(&a.market_value).unwrap());

    Ok(PositionsPayload {
        as_of: as_of.format("%Y-%m-%d").to_string(),
        base_ccy: base.to_string(),
        total_value: total_mv,
        positions: rows,
    })
}
