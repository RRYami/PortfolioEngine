use crate::currency::Currency;
use crate::ids::InstrumentId;

/// A tradable instrument (equity, bond, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Instrument {
    pub id: InstrumentId,
    pub symbol: String,
    pub name: String,
    pub currency: Currency,
    pub kind: InstrumentKind,
}

/// Classification of an instrument.
///
/// Uses struct variants so that adding instrument-specific metadata later is
/// a non-breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum InstrumentKind {
    Equity {},
}
