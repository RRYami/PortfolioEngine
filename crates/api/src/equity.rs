//! Shared portfolio equity-curve construction.
//!
//! Both the risk dashboard (drawdown / historical `VaR`) and the performance-ratio
//! tab derive their series from the same trailing equity curve: the *current*
//! holdings valued at each historical close, FX-converted to base at the
//! rate that applied on that date.
//! It is a current-book curve — a "what if I had held today's book over the
//! window" series — not a realized position-by-position track record.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Duration, NaiveDate};
use ptf_engine::{Currency, FxRateProvider, HistoricalPriceProvider, PortfolioState};
use rust_decimal::prelude::ToPrimitive;

use crate::error::ApiError;
use crate::price_source::PriceData;

fn f(x: rust_decimal::Decimal) -> f64 {
    x.to_f64().unwrap_or(0.0)
}

/// Trailing equity curve of the current book, oldest→newest, on the dates where
/// **every** held instrument has a price. `cap` optionally keeps only the most
/// recent N points (the risk charts trim to a shorter window than the ratios).
pub fn series(
    state: &PortfolioState,
    pd: &PriceData,
    base: Currency,
    as_of: NaiveDate,
    lookback_days: u32,
    cap: Option<usize>,
) -> Result<(Vec<f64>, Vec<NaiveDate>), ApiError> {
    let from = as_of - Duration::days(i64::from(lookback_days));
    let mut maps: Vec<BTreeMap<NaiveDate, f64>> = Vec::new();
    let mut common: Option<BTreeSet<NaiveDate>> = None;

    for (inst_id, pos) in state.positions() {
        let qty = f(pos.net_quantity());
        let ccy = pos.currency();
        let hist = pd.historical.prices(*inst_id, from, as_of)?;
        // FX is applied per date, not once at `as_of`: for a base currency
        // that moved against the position's currency, the FX leg is part of
        // the return, and freezing it understates realised volatility.
        let mut map: BTreeMap<NaiveDate, f64> = BTreeMap::new();
        for (d, m) in &hist {
            let fx = f(pd.fx.rate(ccy, base, *d)?);
            map.insert(*d, f(m.amount) * qty * fx);
        }
        let dates: BTreeSet<NaiveDate> = map.keys().copied().collect();
        common = Some(match common {
            None => dates,
            Some(c) => c.intersection(&dates).copied().collect(),
        });
        maps.push(map);
    }

    let mut dates: Vec<NaiveDate> = common.unwrap_or_default().into_iter().collect();
    if let Some(n) = cap {
        if dates.len() > n {
            dates = dates.split_off(dates.len() - n);
        }
    }
    let equity: Vec<f64> = dates
        .iter()
        .map(|d| maps.iter().filter_map(|m| m.get(d)).sum())
        .collect();
    Ok((equity, dates))
}

/// ISO-8601 (`YYYY-MM-DD`) strings for a date vector — the JSON contract.
pub fn iso(dates: &[NaiveDate]) -> Vec<String> {
    dates
        .iter()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .collect()
}
