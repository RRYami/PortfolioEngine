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
