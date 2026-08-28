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

/// Sizes arrive as `INTEGER` from `DuckDB` but the writer's width and
/// signedness are not guaranteed, so accept any of the four rather than
/// failing the load.
///
/// The unsigned arms matter: the raw vendor snapshots store sizes as
/// `UINTEGER`, and only the round trip through `DuckDB`'s signed `INTEGER`
/// column made them readable. Reading a raw partition directly -- or the
/// archive, which preserves the vendor types -- silently yielded a size of
/// zero for every row, because a failed downcast is indistinguishable from an
/// absent value here.
fn int_at(b: &arrow::record_batch::RecordBatch, i: usize, row: usize) -> Option<i32> {
    use arrow::array::{Int32Array, Int64Array, UInt32Array, UInt64Array};
    let c = b.column(i);
    if c.is_null(row) {
        return None;
    }
    if let Some(a) = c.as_any().downcast_ref::<Int32Array>() {
        return Some(a.value(row));
    }
    if let Some(a) = c.as_any().downcast_ref::<UInt32Array>() {
        return Some(i32::try_from(a.value(row)).unwrap_or(i32::MAX));
    }
    if let Some(a) = c.as_any().downcast_ref::<UInt64Array>() {
        return Some(i32::try_from(a.value(row)).unwrap_or(i32::MAX));
    }
    c.as_any()
        .downcast_ref::<Int64Array>()
        .map(|a| i32::try_from(a.value(row)).unwrap_or(i32::MAX))
}

/// Load the same [`Chain`] from `market.option_quote`.
///
/// The pipeline downstream is synchronous and CPU-bound, so rather than
/// colouring it async the runtime is created here and dropped when the query
/// returns. One statement, one await, one allocation of the whole result --
/// the same residency the file loader already assumes.
pub fn load_postgres(dsn: &str, root: Option<&str>) -> Result<Chain, SurfaceError> {
    use sqlx::Row;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| SurfaceError::Io("tokio runtime".into(), e))?;

    runtime.block_on(async {
        let pool = sqlx::PgPool::connect(dsn)
            .await
            .map_err(|e| SurfaceError::Database(e.to_string()))?;

        // `mid`, `strike` and `tte` are what the fit needs; a row missing any
        // of them is skipped exactly as the file loader skips nulls.
        let rows = sqlx::query(
            "SELECT quote_date, root, expiry, opt_right, strike, mid,
                    rel_spread, tte, bid_size, ask_size, last_update_ts
             FROM market.option_quote
             WHERE ($1::text IS NULL OR root = $1)
               AND strike IS NOT NULL AND mid IS NOT NULL AND tte IS NOT NULL
             ORDER BY quote_date, root, expiry",
        )
        .bind(root)
        .fetch_all(&pool)
        .await
        .map_err(|e| SurfaceError::Database(e.to_string()))?;

        let mut chain: Chain = BTreeMap::new();
        for row in &rows {
            let quote_date: NaiveDate = row.get("quote_date");
            let root: String = row.get("root");
            let expiry: NaiveDate = row.get("expiry");
            let right: String = row.get("opt_right");
            let tte: f64 = row.get("tte");

            // A quote is only as deep as its thinner side; either size absent
            // means no weight hint, matching the file loader.
            let size = match (
                row.get::<Option<i32>, _>("bid_size"),
                row.get::<Option<i32>, _>("ask_size"),
            ) {
                (Some(b), Some(a)) => f64::from(b.min(a)),
                _ => 0.0,
            };
            let stale = row
                .get::<Option<chrono::DateTime<chrono::Utc>>, _>("last_update_ts")
                .is_none();

            let q = Quote {
                right: if right == "C" { OptionRight::Call } else { OptionRight::Put },
                strike: row.get("strike"),
                mid: row.get("mid"),
                rel_spread: row
                    .get::<Option<f64>, _>("rel_spread")
                    .unwrap_or(f64::NAN),
                size,
                stale,
            };
            chain
                .entry((quote_date, root))
                .or_default()
                .entry(expiry)
                .or_insert_with(|| (tte, Vec::new()))
                .1
                .push(q);
        }
        Ok(chain)
    })
}
