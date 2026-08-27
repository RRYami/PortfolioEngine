//! Writing the two engine-facing parquet artifacts.
//!
//! Column names and order are a published interface, exactly as
//! `options_db.COLUMNS` is on the Python side — extend at the end, never
//! reorder.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanArray, Date32Array, Float64Array, StringArray, UInt32Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::NaiveDate;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use crate::build::{ForwardRow, IvRow};
use crate::error::SurfaceError;

const EPOCH: NaiveDate = NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch");

#[allow(clippy::cast_possible_truncation)]
fn days(d: NaiveDate) -> i32 {
    (d - EPOCH).num_days() as i32
}

fn write(path: &Path, schema: Arc<Schema>, cols: Vec<ArrayRef>) -> Result<(), SurfaceError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SurfaceError::Io(parent.display().to_string(), e))?;
    }
    let batch = RecordBatch::try_new(schema.clone(), cols)?;
    let file = File::create(path).map_err(|e| SurfaceError::Io(path.display().to_string(), e))?;
    let props = WriterProperties::builder().set_compression(Compression::SNAPPY).build();
    let mut w = ArrowWriter::try_new(file, schema, Some(props))?;
    w.write(&batch)?;
    w.close()?;
    Ok(())
}

pub fn forwards(path: &Path, rows: &[ForwardRow]) -> Result<(), SurfaceError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("quote_date", DataType::Date32, false),
        Field::new("root", DataType::Utf8, false),
        Field::new("expiry", DataType::Date32, false),
        Field::new("tte", DataType::Float64, false),
        Field::new("forward", DataType::Float64, false),
        Field::new("discount", DataType::Float64, false),
        Field::new("pairs_used", DataType::UInt32, false),
        Field::new("rmse", DataType::Float64, false),
        Field::new("curve_rate", DataType::Float64, false),
        Field::new("curve_expiries", DataType::UInt32, false),
        Field::new("curve_clamped", DataType::Boolean, false),
    ]));
    let u32c = |f: fn(&ForwardRow) -> usize| -> ArrayRef {
        Arc::new(UInt32Array::from(
            rows.iter().map(|r| u32::try_from(f(r)).unwrap_or(u32::MAX)).collect::<Vec<_>>(),
        ))
    };
    let f64c = |f: fn(&ForwardRow) -> f64| -> ArrayRef {
        Arc::new(Float64Array::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    write(path, schema, vec![
        Arc::new(Date32Array::from(rows.iter().map(|r| days(r.quote_date)).collect::<Vec<_>>())),
        Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.root.as_str()))),
        Arc::new(Date32Array::from(rows.iter().map(|r| days(r.expiry)).collect::<Vec<_>>())),
        f64c(|r| r.tte),
        f64c(|r| r.forward),
        f64c(|r| r.discount),
        u32c(|r| r.pairs_used),
        f64c(|r| r.rmse),
        f64c(|r| r.curve_rate),
        u32c(|r| r.curve_expiries),
        Arc::new(BooleanArray::from(rows.iter().map(|r| r.curve_clamped).collect::<Vec<_>>())),
    ])
}

pub fn ivs(path: &Path, rows: &[IvRow]) -> Result<(), SurfaceError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("quote_date", DataType::Date32, false),
        Field::new("root", DataType::Utf8, false),
        Field::new("expiry", DataType::Date32, false),
        Field::new("opt_right", DataType::Utf8, false),
        Field::new("strike", DataType::Float64, false),
        Field::new("tte", DataType::Float64, false),
        Field::new("log_moneyness", DataType::Float64, false),
        Field::new("iv", DataType::Float64, false),
        Field::new("vega", DataType::Float64, false),
        Field::new("forward", DataType::Float64, false),
        Field::new("rel_spread", DataType::Float64, true),
        Field::new("size", DataType::Float64, false),
        Field::new("stale", DataType::Boolean, false),
    ]));
    let f64c = |f: fn(&IvRow) -> f64| -> ArrayRef {
        Arc::new(Float64Array::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    write(path, schema, vec![
        Arc::new(Date32Array::from(rows.iter().map(|r| days(r.quote_date)).collect::<Vec<_>>())),
        Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.root.as_str()))),
        Arc::new(Date32Array::from(rows.iter().map(|r| days(r.expiry)).collect::<Vec<_>>())),
        Arc::new(StringArray::from_iter_values(rows.iter().map(|r| {
            if matches!(r.opt_right, ptf_engine::vol::OptionRight::Call) { "C" } else { "P" }
        }))),
        f64c(|r| r.strike),
        f64c(|r| r.tte),
        f64c(|r| r.log_moneyness),
        f64c(|r| r.iv),
        f64c(|r| r.vega),
        f64c(|r| r.forward),
        // NaN means "no spread recorded"; keep it null rather than poisoning
        // downstream weighting with a not-a-number.
        Arc::new(Float64Array::from(
            rows.iter().map(|r| r.rel_spread.is_finite().then_some(r.rel_spread)).collect::<Vec<_>>(),
        )),
        f64c(|r| r.size),
        Arc::new(BooleanArray::from(rows.iter().map(|r| r.stale).collect::<Vec<_>>())),
    ])
}
