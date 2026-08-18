//! Market-data abstraction. The engine consumes *providers*, never the data
//! source directly, so this trait is the seam where Phase 2 swaps the
//! deterministic synthetic series for the Python/yfinance Parquet feed.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use arrow::array::{Array, Float64Array, StringArray};
use arrow::record_batch::RecordBatch;
use chrono::{Duration, NaiveDate};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use ptf_engine::{
    HistoricalFxProvider,
    Currency, InstrumentId, Money, StaticHistoricalPriceProvider,
    StaticPriceProvider,
};
use rust_decimal::Decimal;

use crate::error::ApiError;

/// A held instrument that needs price data.
pub struct HeldInstrument {
    pub id: InstrumentId,
    pub symbol: String,
    pub currency: Currency,
}

/// The providers `compute_var` / `valuation` require, fully populated.
///
/// Two FX views, because the price history is remapped onto consecutive
/// calendar days ending at `as_of` (see [`ParquetPriceSource`]):
///
/// - `fx` is keyed by those same remapped days, so it lines up with
///   `historical` for equity-curve and `VaR` math.
/// - `fx_trade_date` is keyed by real calendar dates, for converting a tax
///   lot's cost at the rate that actually applied on its trade date.
///
/// Using `fx` for lot conversion (or `fx_trade_date` for the curve) would
/// silently mis-date the rate, so they are deliberately separate fields.
pub struct PriceData {
    pub historical: StaticHistoricalPriceProvider,
    pub prices: StaticPriceProvider,
    pub fx: HistoricalFxProvider,
    pub fx_trade_date: HistoricalFxProvider,
}

/// Supplies price/FX data for a set of holdings as of a date.
pub trait PriceSource: Send + Sync {
    fn build(
        &self,
        holdings: &[HeldInstrument],
        base: Currency,
        as_of: NaiveDate,
        lookback_days: u32,
    ) -> Result<PriceData, ApiError>;
}

/// USD-per-unit reference rates for the supported currencies.
fn usd_per(ccy: Currency) -> f64 {
    match ccy.as_str() {
        "EUR" => 1.08,
        "GBP" => 1.27,
        "JPY" => 0.0067,
        "CHF" => 1.12,
        _ => 1.0, // USD and anything else
    }
}

const SUPPORTED: [&str; 5] = ["USD", "EUR", "GBP", "JPY", "CHF"];

/// Deterministic synthetic prices for Phase 1 — real engine math, fake feed.
#[derive(Default)]
pub struct SyntheticPriceSource;

impl PriceSource for SyntheticPriceSource {
    fn build(
        &self,
        holdings: &[HeldInstrument],
        base: Currency,
        as_of: NaiveDate,
        lookback_days: u32,
    ) -> Result<PriceData, ApiError> {
        let mut historical = StaticHistoricalPriceProvider::new();
        let mut prices = StaticPriceProvider::new();
        let mut fx = HistoricalFxProvider::new();

        // FX: every supported currency → base, flat across the window. The
        // synthetic feed has no FX path; forward-fill from the window start
        // makes every date in range resolvable.
        let start = as_of - Duration::days(i64::from(lookback_days) + 3650);
        let base_usd = usd_per(base);
        for code in SUPPORTED {
            if let Ok(from) = Currency::try_from(code) {
                if from != base {
                    let rate = usd_per(from) / base_usd;
                    fx.insert(from, base, start, dec(rate));
                }
            }
        }

        // History: one close per calendar day across the lookback window,
        // guaranteeing `lookback + 1` observations.
        let from = as_of - Duration::days(i64::from(lookback_days));
        let span = (as_of - from).num_days();
        for h in holdings {
            let mut rng = Mulberry32::seeded(symbol_seed(&h.symbol));
            // Per-symbol anchor price + daily vol.
            let mut price = 50.0 + f64::from(symbol_seed(&h.symbol) % 250);
            let vol = 0.010 + f64::from(symbol_seed(&h.symbol) % 20) / 1000.0;
            for d in 0..=span {
                let date = from + Duration::days(d);
                historical.insert(h.id, date, Money::new(dec(price), h.currency));
                if date == as_of {
                    prices.insert(h.id, date, Money::new(dec(price), h.currency));
                }
                let ret = rng.randn() * vol + 0.0003;
                price = (price * (1.0 + ret)).max(1.0);
            }
        }

        Ok(PriceData {
            historical,
            prices,
            fx: fx.clone(),
            fx_trade_date: fx,
        })
    }
}

/// Reads the Python service's Parquet export. No C++ `DuckDB` — pure-Rust
/// `arrow`/`parquet`. Closes are remapped to consecutive calendar days ending
/// at `as_of` so `compute_var`'s calendar-day lookback window is filled with
/// real trading-day returns (avoids weekend zero-return dilution).
pub struct ParquetPriceSource {
    prices_path: PathBuf,
    fx_path: PathBuf,
}

impl ParquetPriceSource {
    pub fn new(prices_path: impl Into<PathBuf>, fx_path: impl Into<PathBuf>) -> Self {
        Self {
            prices_path: prices_path.into(),
            fx_path: fx_path.into(),
        }
    }
}

impl PriceSource for ParquetPriceSource {
    fn build(
        &self,
        holdings: &[HeldInstrument],
        base: Currency,
        as_of: NaiveDate,
        lookback_days: u32,
    ) -> Result<PriceData, ApiError> {
        let closes = read_prices(&self.prices_path)?;
        let fxmap = read_fx(&self.fx_path)?;

        let mut historical = StaticHistoricalPriceProvider::new();
        let mut prices = StaticPriceProvider::new();
        let mut fx = HistoricalFxProvider::new();
        let mut fx_trade_date = HistoricalFxProvider::new();

        let need = lookback_days as usize + 1;

        // Every currency actually in play. A missing series is a hard error
        // rather than a hardcoded guess: valuing a book at an invented rate is
        // wrong in a way nobody can see downstream.
        let mut needed: Vec<Currency> = holdings.iter().map(|h| h.currency).collect();
        needed.push(base);
        needed.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
        needed.dedup();
        for c in &needed {
            if !fxmap.contains_key(c.as_str()) {
                return Err(ApiError::BadRequest(format!(
                    "no FX data for {c} — run the prices service to refresh fx.parquet"
                )));
            }
        }

        let base_series: &[(NaiveDate, f64)] = &fxmap[base.as_str()];

        // Populate every currency the parquet knows, not just the ones held
        // today: valuation also converts cash balances and realized P&L, which
        // outlive the position that created them. A book that sold out of its
        // only USD holding still has a USD realized-P&L entry to convert.
        let mut available: Vec<Currency> = fxmap
            .keys()
            .filter_map(|c| Currency::try_from(c.as_str()).ok())
            .collect();
        available.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
        available.dedup();

        for c in &available {
            if *c == base {
                continue; // identity, handled by the trait
            }
            let series = &fxmap[c.as_str()];

            // True-dated view, for converting a lot at its own trade date.
            for &(d, usd) in series {
                if let Some(b) = lookup(base_series, d) {
                    fx_trade_date.insert(*c, base, d, dec(usd / b));
                }
            }

            // Remapped view, aligned with the price history's synthetic
            // calendar days so the equity curve joins FX correctly.
            let take = series.len().min(need);
            let recent = &series[series.len() - take..];
            let k = recent.len();
            for (i, &(d, usd)) in recent.iter().enumerate() {
                let offset = i64::try_from(k - 1 - i).unwrap_or(0);
                if let Some(b) = lookup(base_series, d) {
                    fx.insert(*c, base, as_of - Duration::days(offset), dec(usd / b));
                }
            }
        }

        for h in holdings {
            let series = closes.get(&h.symbol).ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "no price data for {} — add it again to fetch",
                    h.symbol
                ))
            })?;
            let take = series.len().min(need);
            let recent = &series[series.len() - take..];
            let k = recent.len();
            for (i, &close) in recent.iter().enumerate() {
                let offset = i64::try_from(k - 1 - i).unwrap_or(0);
                let date = as_of - Duration::days(offset);
                historical.insert(h.id, date, Money::new(dec(close), h.currency));
            }
            if let Some(&last) = recent.last() {
                prices.insert(h.id, as_of, Money::new(dec(last), h.currency));
            }
        }

        Ok(PriceData {
            historical,
            prices,
            fx,
            fx_trade_date,
        })
    }
}

fn read_prices(path: &Path) -> Result<HashMap<String, Vec<f64>>, ApiError> {
    let file = File::open(path).map_err(|_| {
        ApiError::BadRequest("price data unavailable — add a holding to fetch prices".into())
    })?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .and_then(ParquetRecordBatchReaderBuilder::build)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut map: HashMap<String, Vec<f64>> = HashMap::new();
    for batch in reader {
        let batch = batch.map_err(|e| ApiError::Internal(e.to_string()))?;
        let sym = col_str(&batch, "symbol")?;
        let close = col_f64(&batch, "close")?;
        for i in 0..batch.num_rows() {
            map.entry(sym.value(i).to_string())
                .or_default()
                .push(close.value(i));
        }
    }
    Ok(map)
}

/// Most recent rate on or before `date` in a date-sorted series.
fn lookup(series: &[(NaiveDate, f64)], date: NaiveDate) -> Option<f64> {
    let i = series.partition_point(|(d, _)| *d <= date);
    (i > 0).then(|| series[i - 1].1)
}

/// USD-per-unit series per currency, ascending by date.
fn read_fx(path: &Path) -> Result<HashMap<String, Vec<(NaiveDate, f64)>>, ApiError> {
    let file = File::open(path).map_err(|_| {
        ApiError::BadRequest("FX data unavailable — run the prices service to fetch rates".into())
    })?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .and_then(ParquetRecordBatchReaderBuilder::build)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut map: HashMap<String, Vec<(NaiveDate, f64)>> = HashMap::new();
    for batch in reader {
        let batch = batch.map_err(|e| ApiError::Internal(e.to_string()))?;
        let ccy = col_str(&batch, "ccy")?;
        let date = col_str(&batch, "date")?;
        let usd = col_f64(&batch, "usd_per_unit")?;
        for i in 0..batch.num_rows() {
            let Ok(d) = NaiveDate::parse_from_str(date.value(i), "%Y-%m-%d") else {
                continue;
            };
            map.entry(ccy.value(i).to_string())
                .or_default()
                .push((d, usd.value(i)));
        }
    }
    for series in map.values_mut() {
        series.sort_by_key(|(d, _)| *d);
    }
    Ok(map)
}

fn col_str<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray, ApiError> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| ApiError::Internal(format!("parquet column `{name}` missing or not utf8")))
}

fn col_f64<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Float64Array, ApiError> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
        .ok_or_else(|| ApiError::Internal(format!("parquet column `{name}` missing or not f64")))
}

fn dec(x: f64) -> Decimal {
    Decimal::from_f64_retain(x).unwrap_or_default().round_dp(6)
}

fn symbol_seed(symbol: &str) -> u32 {
    symbol.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ptf_engine::{HistoricalPriceProvider, InstrumentId, PriceProvider};

    #[test]
    fn synthetic_supplies_enough_history_for_lookback() {
        let lookback = 252u32;
        let as_of = NaiveDate::from_ymd_opt(2026, 6, 26).unwrap();
        let id = InstrumentId::new();
        let holdings = vec![HeldInstrument {
            id,
            symbol: "NVDA".into(),
            currency: Currency::USD,
        }];

        let pd = SyntheticPriceSource
            .build(&holdings, Currency::USD, as_of, lookback)
            .unwrap();

        // compute_var needs >= lookback + 1 observations in [from, as_of].
        let from = as_of - Duration::days(i64::from(lookback));
        let series = pd.historical.prices(id, from, as_of).unwrap();
        assert!(
            series.len() >= (lookback + 1) as usize,
            "got {} observations",
            series.len()
        );
        // A spot price must exist exactly at as_of for valuation.
        assert!(pd.prices.price(id, as_of).is_ok());
    }
}

/// Tiny deterministic PRNG (mulberry32) + Box–Muller, so synthetic series are
/// reproducible per symbol.
struct Mulberry32(u32);

impl Mulberry32 {
    fn seeded(seed: u32) -> Self {
        Self(seed)
    }

    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6D2B_79F5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(0x1 | t);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(0x3d | t));
        f64::from((t ^ (t >> 14)) >> 8) / f64::from(1u32 << 24)
    }

    fn randn(&mut self) -> f64 {
        let mut u = 0.0;
        while u <= f64::EPSILON {
            u = self.next_f64();
        }
        let v = self.next_f64();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}
