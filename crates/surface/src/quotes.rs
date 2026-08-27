//! Reading `options.parquet` — the artifact the Python ingest publishes.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use arrow::array::{Array, Date32Array, Float64Array, StringArray};
use chrono::NaiveDate;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use ptf_engine::vol::OptionRight;

use crate::error::SurfaceError;

/// One quoted contract, carrying only what the surface needs.
#[derive(Debug, Clone, Copy)]
pub struct Quote {
    pub right: OptionRight,
    pub strike: f64,
    pub mid: f64,
    pub rel_spread: f64,
    /// Smaller of the two displayed sizes — a market is only as deep as its
    /// thinner side.
    pub size: f64,
    /// The book never updated during the snapshot minute, so this quote was
    /// carried forward rather than set. True for ~77% of a real chain.
    pub stale: bool,
}

/// A session's chain, grouped the way every downstream stage consumes it.
pub type Chain = BTreeMap<(NaiveDate, String), BTreeMap<NaiveDate, (f64, Vec<Quote>)>>;

const EPOCH: NaiveDate = NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch");

fn date_at(col: &Date32Array, i: usize) -> NaiveDate {
    EPOCH + chrono::Duration::days(i64::from(col.value(i)))
}

/// Load and group by `(quote_date, root)` then expiry.
///
/// Reads the whole file into memory on purpose: a decade of one root is a few
/// million rows, and grouping streamed batches by session would need the same
/// residency anyway. Revisit if this ever spans many roots at once.
pub fn load(path: &Path) -> Result<Chain, SurfaceError> {
    let file = File::open(path).map_err(|e| SurfaceError::Io(path.display().to_string(), e))?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;

    let mut chain: Chain = BTreeMap::new();
    for batch in reader {
        let batch = batch?;
        let col = |name: &str| -> Result<usize, SurfaceError> {
            batch
                .schema()
                .index_of(name)
                .map_err(|_| SurfaceError::MissingColumn(name.to_string()))
        };
        let quote_date = batch
            .column(col("quote_date")?)
            .as_any()
            .downcast_ref::<Date32Array>()
            .ok_or_else(|| SurfaceError::BadColumn("quote_date"))?;
        let expiry = batch
            .column(col("expiry")?)
            .as_any()
            .downcast_ref::<Date32Array>()
            .ok_or_else(|| SurfaceError::BadColumn("expiry"))?;
        let root = str_col(&batch, col("root")?, "root")?;
        let right = str_col(&batch, col("opt_right")?, "opt_right")?;
        let strike = f64_col(&batch, col("strike")?, "strike")?;
        let mid = f64_col(&batch, col("mid")?, "mid")?;
        let rel_spread = f64_col(&batch, col("rel_spread")?, "rel_spread")?;
        let tte = f64_col(&batch, col("tte")?, "tte")?;
        // Sizes and staleness post-date the first ingest, so a store written
        // before that change still loads — those rows simply carry no weight
        // hint and are treated as stale-unknown.
        let bid_size = batch.schema().index_of("bid_size").ok();
        let ask_size = batch.schema().index_of("ask_size").ok();
        let updated = batch.schema().index_of("last_update_ts").ok();

        for i in 0..batch.num_rows() {
            if strike.is_null(i) || mid.is_null(i) || tte.is_null(i) {
                continue;
            }
            let size = match (bid_size, ask_size) {
                (Some(b), Some(a)) => {
                    let bs = int_at(&batch, b, i);
                    let as_ = int_at(&batch, a, i);
                    match (bs, as_) {
                        (Some(x), Some(y)) => f64::from(x.min(y)),
                        _ => 0.0,
                    }
                }
                _ => 0.0,
            };
            let stale = updated.is_some_and(|c| batch.column(c).is_null(i));
            let q = Quote {
                right: if right.value(i) == "C" { OptionRight::Call } else { OptionRight::Put },
                strike: strike.value(i),
                mid: mid.value(i),
                rel_spread: if rel_spread.is_null(i) { f64::NAN } else { rel_spread.value(i) },
                size,
                stale,
            };
            chain
                .entry((date_at(quote_date, i), root.value(i).to_string()))
                .or_default()
                .entry(date_at(expiry, i))
                .or_insert_with(|| (tte.value(i), Vec::new()))
                .1
                .push(q);
        }
    }
    Ok(chain)
}

fn str_col<'a>(
    b: &'a arrow::record_batch::RecordBatch,
    i: usize,
    name: &'static str,
) -> Result<&'a StringArray, SurfaceError> {
    b.column(i)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or(SurfaceError::BadColumn(name))
}

fn f64_col<'a>(
    b: &'a arrow::record_batch::RecordBatch,
    i: usize,
    name: &'static str,
) -> Result<&'a Float64Array, SurfaceError> {
    b.column(i)
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or(SurfaceError::BadColumn(name))
}

/// Sizes arrive as `INTEGER` from `DuckDB` but the writer's width is not
/// guaranteed, so accept either 32- or 64-bit rather than failing the load.
fn int_at(b: &arrow::record_batch::RecordBatch, i: usize, row: usize) -> Option<i32> {
    use arrow::array::{Int32Array, Int64Array};
    let c = b.column(i);
    if c.is_null(row) {
        return None;
    }
    if let Some(a) = c.as_any().downcast_ref::<Int32Array>() {
        return Some(a.value(row));
    }
    c.as_any()
        .downcast_ref::<Int64Array>()
        .map(|a| i32::try_from(a.value(row)).unwrap_or(i32::MAX))
}
