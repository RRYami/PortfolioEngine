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

use crate::build::{ForwardRow, GridRow, IvRow, SviRow};
use crate::factors::FactorFit;
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
        Field::new("mid", DataType::Float64, false),
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
        f64c(|r| r.mid),
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

pub fn svis(path: &Path, rows: &[SviRow]) -> Result<(), SurfaceError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("quote_date", DataType::Date32, false),
        Field::new("root", DataType::Utf8, false),
        Field::new("expiry", DataType::Date32, false),
        Field::new("tte", DataType::Float64, false),
        Field::new("a", DataType::Float64, false),
        Field::new("b", DataType::Float64, false),
        Field::new("rho", DataType::Float64, false),
        Field::new("m", DataType::Float64, false),
        Field::new("sigma", DataType::Float64, false),
        Field::new("rmse_vol", DataType::Float64, false),
        Field::new("points", DataType::UInt32, false),
        Field::new("min_durrleman", DataType::Float64, false),
        // The moneyness range the slice was fitted over. Evaluating outside it
        // is extrapolation, and downstream should know where that starts.
        Field::new("k_lo", DataType::Float64, false),
        Field::new("k_hi", DataType::Float64, false),
    ]));
    let f64c = |f: fn(&SviRow) -> f64| -> ArrayRef {
        Arc::new(Float64Array::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    write(path, schema, vec![
        Arc::new(Date32Array::from(rows.iter().map(|r| days(r.quote_date)).collect::<Vec<_>>())),
        Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.root.as_str()))),
        Arc::new(Date32Array::from(rows.iter().map(|r| days(r.expiry)).collect::<Vec<_>>())),
        f64c(|r| r.tte),
        f64c(|r| r.params.a),
        f64c(|r| r.params.b),
        f64c(|r| r.params.rho),
        f64c(|r| r.params.m),
        f64c(|r| r.params.sigma),
        f64c(|r| r.rmse_vol),
        Arc::new(UInt32Array::from(
            rows.iter().map(|r| u32::try_from(r.points).unwrap_or(u32::MAX)).collect::<Vec<_>>(),
        )),
        f64c(|r| r.min_durrleman),
        f64c(|r| r.k_lo),
        f64c(|r| r.k_hi),
    ])
}

pub fn grid(path: &Path, rows: &[GridRow]) -> Result<(), SurfaceError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("quote_date", DataType::Date32, false),
        Field::new("root", DataType::Utf8, false),
        Field::new("z", DataType::Float64, false),
        Field::new("tte", DataType::Float64, false),
        Field::new("k", DataType::Float64, false),
        Field::new("total_variance", DataType::Float64, false),
        Field::new("vol", DataType::Float64, false),
        Field::new("extrapolated", DataType::Boolean, false),
    ]));
    let f64c = |f: fn(&GridRow) -> f64| -> ArrayRef {
        Arc::new(Float64Array::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    write(path, schema, vec![
        Arc::new(Date32Array::from(rows.iter().map(|r| days(r.quote_date)).collect::<Vec<_>>())),
        Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.root.as_str()))),
        f64c(|r| r.z),
        f64c(|r| r.tte),
        f64c(|r| r.k),
        f64c(|r| r.total_variance),
        f64c(|r| r.vol),
        Arc::new(BooleanArray::from(rows.iter().map(|r| r.extrapolated).collect::<Vec<_>>())),
    ])
}

/// Loadings, one row per (component, cell), plus the standardisation each cell
/// needs in order to invert a reconstructed shock.
pub fn pca_loadings(path: &Path, fits: &[FactorFit]) -> Result<(), SurfaceError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("root", DataType::Utf8, false),
        Field::new("as_of", DataType::Date32, false),
        Field::new("pc", DataType::UInt32, false),
        Field::new("z", DataType::Float64, false),
        Field::new("tte", DataType::Float64, false),
        Field::new("loading", DataType::Float64, false),
        Field::new("cell_mean", DataType::Float64, false),
        Field::new("cell_sd", DataType::Float64, false),
        Field::new("explained", DataType::Float64, false),
    ]));
    let (mut root, mut as_of, mut pc) = (vec![], vec![], vec![]);
    let (mut z, mut tte, mut load) = (vec![], vec![], vec![]);
    let (mut cmean, mut csd, mut expl) = (vec![], vec![], vec![]);
    for f in fits {
        for (j, vec) in f.fit.loadings.iter().enumerate() {
            for (i, cell) in f.cells.iter().enumerate() {
                root.push(f.root.as_str());
                as_of.push(days(f.as_of));
                pc.push(u32::try_from(j + 1).unwrap_or(u32::MAX));
                z.push(cell.z);
                tte.push(cell.tte);
                load.push(vec[i]);
                cmean.push(f.fit.mean[i]);
                csd.push(f.fit.sd[i]);
                expl.push(f.fit.explained[j]);
            }
        }
    }
    write(path, schema, vec![
        Arc::new(StringArray::from_iter_values(root)),
        Arc::new(Date32Array::from(as_of)),
        Arc::new(UInt32Array::from(pc)),
        Arc::new(Float64Array::from(z)),
        Arc::new(Float64Array::from(tte)),
        Arc::new(Float64Array::from(load)),
        Arc::new(Float64Array::from(cmean)),
        Arc::new(Float64Array::from(csd)),
        Arc::new(Float64Array::from(expl)),
    ])
}

/// The historical score series. Kept as a series rather than a covariance so
/// the engine can estimate the joint distribution of scores and spot returns
/// with the machinery it already has.
pub fn pca_scores(path: &Path, fits: &[FactorFit]) -> Result<(), SurfaceError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("root", DataType::Utf8, false),
        Field::new("quote_date", DataType::Date32, false),
        Field::new("pc", DataType::UInt32, false),
        Field::new("score", DataType::Float64, false),
    ]));
    let (mut root, mut date, mut pc, mut score) = (vec![], vec![], vec![], vec![]);
    for f in fits {
        for (i, d) in f.dates.iter().enumerate() {
            for (j, s) in f.fit.scores[i].iter().enumerate() {
                root.push(f.root.as_str());
                date.push(days(*d));
                pc.push(u32::try_from(j + 1).unwrap_or(u32::MAX));
                score.push(*s);
            }
        }
    }
    write(path, schema, vec![
        Arc::new(StringArray::from_iter_values(root)),
        Arc::new(Date32Array::from(date)),
        Arc::new(UInt32Array::from(pc)),
        Arc::new(Float64Array::from(score)),
    ])
}
