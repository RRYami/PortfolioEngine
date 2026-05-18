use rust_decimal::Decimal;

use crate::ids::LotId;

/// Method used to select which lots are closed by a sell or buy-to-cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum LotMethod {
    Fifo,
    Lifo,
    HighestCost,
    LowestCost,
    AverageCost,
}

/// Side of a lot: long (owned) or short (borrowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum LotSide {
    Long,
    Short,
}

/// A single entry in a user-specified lot selection.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LotSelectionEntry {
    pub lot_id: LotId,
    pub quantity: Decimal,
}

/// Override for lot selection on a per-transaction basis.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LotSelection {
    /// Use the given method instead of the portfolio default.
    Method(LotMethod),
    /// Explicitly select lots and quantities to close.
    Specific(Vec<LotSelectionEntry>),
}
