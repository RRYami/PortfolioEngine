//! Building the PCA panel from the grid and fitting it.
//!
//! Runs once over the whole history rather than per session: a decomposition
//! needs a window of daily changes, not a snapshot.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use ptf_engine::pca::{PcaError, PcaFit, fit};

use crate::build::GridRow;

/// Components retained. PC4 and PC5 contribute under 5% each on a year of
/// SOXX; past three, the sample size is fitting noise.
pub const COMPONENTS: usize = 3;

/// Drop a cell if it needed extrapolating past the fitted quote range on more
/// than this share of sessions. Four of the 24 cells fail it — the deep
/// downside at 1m, 6m and 1y, plus `z = -1.5` at 1y — and removing them is
/// worth roughly six points of explained variance, because what they mostly
/// carry is SVI wing extrapolation rather than quotes.
pub const MAX_EXTRAPOLATED: f64 = 0.25;

/// Fewest sessions a cell must appear in to be worth keeping.
///
/// A cell is missing when a session's expiries do not bracket its maturity,
/// which happens whenever the front expiry sits past one month. Demanding
/// presence in *every* session is far too strict over a multi-year window: on
/// two years of SOXX that discarded the entire one-month row and left 11 cells
/// of 24.
///
/// The threshold cannot be chosen by explained variance, which rises
/// mechanically as columns are removed — three factors span a smaller space
/// trivially. Measured on that data: 21 cells over 462 sessions explains 84%,
/// 16 cells over 495 explains 90%, and 11 cells over 496 explains 95%, all of
/// the same surface. The grid should instead describe as much of the surface as
/// the data supports, so this keeps the one-month row at the cost of the ~34
/// sessions that cannot reach it. Short-dated volatility is what a one-day `VaR`
/// is mostly made of; a model that cannot represent a move in it is worse than
/// one fitted on slightly less history.
pub const MIN_CELL_COVERAGE: f64 = 0.90;

/// A grid cell, ordered so the panel's columns are stable across refits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub z: f64,
    pub tte: f64,
}

pub struct FactorFit {
    pub root: String,
    /// Last session in the window, i.e. the date the fit is "as of".
    pub as_of: NaiveDate,
    pub cells: Vec<Cell>,
    pub dates: Vec<NaiveDate>,
    pub fit: PcaFit,
    /// Cells excluded for being mostly extrapolation.
    pub dropped: Vec<Cell>,
}

/// Grid coordinates are exact constants, but they arrive through parquet;
/// rounding to a fixed grain keys them without float equality. The values are
/// small (`|z| <= 2`, `tte <= 1`) so the cast cannot truncate.
#[allow(clippy::cast_possible_truncation)]
fn key(x: f64) -> i64 {
    (x * 1e6).round() as i64
}

/// Cell key, then vol and whether it was extrapolated.
type CellObs = BTreeMap<(i64, i64), (f64, bool)>;

/// Assemble the panel and decompose it, for one root.
///
/// Returns `None` when the history is too short or every cell was dropped —
/// both are ordinary outcomes on a thin root, not errors worth aborting for.
pub fn fit_root(root: &str, rows: &[GridRow]) -> Result<FactorFit, PcaError> {
    // session -> cell -> (vol, extrapolated)
    let mut by_date: BTreeMap<NaiveDate, CellObs> = BTreeMap::new();
    let mut seen: BTreeMap<(i64, i64), Cell> = BTreeMap::new();
    for r in rows {
        let k = (key(r.tte), key(r.z));
        seen.insert(k, Cell { z: r.z, tte: r.tte });
        by_date.entry(r.quote_date).or_default().insert(k, (r.vol, r.extrapolated));
    }
    let sessions = by_date.len();
    if sessions < 2 {
        return Err(PcaError::TooFewObservations { got: sessions, need: 2 });
    }

    // Choose cells on coverage and extrapolation, then keep the sessions that
    // have all of them. Selecting cells first and trimming sessions second
    // maximises the panel: the alternative discards a whole column for one
    // absent day.
    #[allow(clippy::cast_precision_loss)]
    let total = sessions as f64;
    let mut kept: Vec<(i64, i64)> = Vec::new();
    let mut dropped = Vec::new();
    for (k, cell) in &seen {
        #[allow(clippy::cast_precision_loss)]
        let present = by_date.values().filter(|d| d.contains_key(k)).count() as f64 / total;
        let observed: Vec<&(f64, bool)> = by_date.values().filter_map(|d| d.get(k)).collect();
        #[allow(clippy::cast_precision_loss)]
        let ex = if observed.is_empty() {
            1.0
        } else {
            observed.iter().filter(|o| o.1).count() as f64 / observed.len() as f64
        };
        if present < MIN_CELL_COVERAGE || ex > MAX_EXTRAPOLATED {
            dropped.push(*cell);
        } else {
            kept.push(*k);
        }
    }
    if kept.is_empty() {
        return Err(PcaError::Malformed);
    }
    let cells: Vec<Cell> = kept.iter().map(|k| seen[k]).collect();
    by_date.retain(|_, cellmap| kept.iter().all(|k| cellmap.contains_key(k)));
    if by_date.len() < 2 {
        return Err(PcaError::TooFewObservations { got: by_date.len(), need: 2 });
    }

    // Daily log-vol changes. Consecutive rows of the map are consecutive
    // *sessions*, which is what a change should span -- no calendar gap
    // handling is needed here because the store holds a contiguous run.
    let dates: Vec<NaiveDate> = by_date.keys().copied().collect();
    let mut panel = Vec::with_capacity(sessions.saturating_sub(1));
    let mut prev: Option<Vec<f64>> = None;
    let mut change_dates = Vec::new();
    for (d, cellmap) in &by_date {
        let logs: Vec<f64> = kept.iter().map(|k| cellmap[k].0.ln()).collect();
        if let Some(p) = &prev {
            panel.push(logs.iter().zip(p).map(|(a, b)| a - b).collect::<Vec<f64>>());
            change_dates.push(*d);
        }
        prev = Some(logs);
    }

    let pca = fit(&panel, COMPONENTS)?;
    Ok(FactorFit {
        root: root.to_string(),
        as_of: *dates.last().expect("non-empty"),
        cells,
        dates: change_dates,
        fit: pca,
        dropped,
    })
}
