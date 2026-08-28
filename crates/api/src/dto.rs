use chrono::NaiveDate;
use ptf_engine::{Currency, LotMethod, Portfolio};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePortfolioReq {
    pub name: String,
    pub base_ccy: String,
    #[serde(default = "default_lot_method")]
    pub lot_method: String,
    pub inception_date: Option<NaiveDate>,
}

fn default_lot_method() -> String {
    "fifo".into()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioSummary {
    pub id: String,
    pub name: String,
    pub base_ccy: String,
    pub lot_method: String,
    pub inception_date: NaiveDate,
}

impl From<&Portfolio> for PortfolioSummary {
    fn from(p: &Portfolio) -> Self {
        Self {
            id: p.id.0.to_string(),
            name: p.name.clone(),
            base_ccy: p.base_currency.to_string(),
            lot_method: lot_method_str(p.lot_method).into(),
            inception_date: p.inception_date,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddHoldingReq {
    pub ticker: String,
    pub name: Option<String>,
    pub quantity: Decimal,
    /// Cost per unit, in `currency`.
    pub cost: Decimal,
    pub currency: String,
    /// Trade date; defaults to the portfolio inception date.
    pub date: Option<NaiveDate>,
}

/// Buy a listed option.
///
/// Separate from [`AddHoldingReq`] rather than an optional block on it: an
/// option needs five contract terms an equity has none of, and folding them in
/// as optional fields would make every one of them silently ignorable.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddOptionReq {
    /// Ticker of the underlying, e.g. `SOXX`. Must be something the surface
    /// pipeline has fitted, since that is what prices the contract.
    pub underlying: String,
    /// `call` or `put` (also accepts `c` / `p`).
    pub right: String,
    pub strike: Decimal,
    pub expiry: NaiveDate,
    /// Number of contracts. Positive only — writing options is not reachable
    /// through this API, the same way short-selling equities is not.
    pub contracts: Decimal,
    /// Premium **per share**, the way options are quoted. The cost basis is
    /// this times the multiplier.
    pub premium: Decimal,
    pub currency: String,
    /// Shares per contract. Defaults to the listed-option standard of 100.
    pub multiplier: Option<Decimal>,
    /// `american` (default) or `european`.
    pub exercise: Option<String>,
    /// Trade date; defaults to the portfolio inception date.
    pub date: Option<NaiveDate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SellHoldingReq {
    pub ticker: String,
    pub quantity: Decimal,
    /// Sale price per unit, in the instrument's currency.
    pub price: Decimal,
    /// Trade date; defaults to today.
    pub date: Option<NaiveDate>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionView {
    pub ticker: String,
    pub name: String,
    pub ccy: String,
    pub quantity: Decimal,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioDetail {
    #[serde(flatten)]
    pub summary: PortfolioSummary,
    pub positions: Vec<PositionView>,
}

pub fn parse_currency(code: &str) -> Result<Currency, ApiError> {
    Currency::try_from(code)
        .map_err(|_| ApiError::BadRequest(format!("invalid currency code: {code}")))
}

pub fn parse_lot_method(s: &str) -> Result<LotMethod, ApiError> {
    match s.to_ascii_lowercase().as_str() {
        "fifo" => Ok(LotMethod::Fifo),
        "lifo" => Ok(LotMethod::Lifo),
        "highest_cost" | "highestcost" => Ok(LotMethod::HighestCost),
        "lowest_cost" | "lowestcost" => Ok(LotMethod::LowestCost),
        "average_cost" | "averagecost" => Ok(LotMethod::AverageCost),
        other => Err(ApiError::BadRequest(format!("invalid lot method: {other}"))),
    }
}

pub fn lot_method_str(m: LotMethod) -> &'static str {
    match m {
        LotMethod::Fifo => "fifo",
        LotMethod::Lifo => "lifo",
        LotMethod::HighestCost => "highest_cost",
        LotMethod::LowestCost => "lowest_cost",
        LotMethod::AverageCost => "average_cost",
    }
}
