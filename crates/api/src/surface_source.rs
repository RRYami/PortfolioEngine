//! Loading the offline surface artifacts into the engine's provider trait.
//!
//! The four parquet files `crates/surface` writes are read once and turned
//! into one [`SurfaceSnapshot`] per underlying. Keeping this in the API crate
//! rather than the engine is the same split as `price_source.rs`: the engine
//! consumes providers, never files.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use arrow::array::{Array, Date32Array, Float64Array, StringArray, UInt32Array};
use arrow::record_batch::RecordBatch;
use chrono::NaiveDate;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use ptf_engine::grid::FittedSlice;
use ptf_engine::ids::InstrumentId;
use ptf_engine::pca::PcaFit;
use ptf_engine::surface::{Cell, StaticVolSurfaceProvider, SurfaceSnapshot};
use ptf_engine::svi::Svi;

const EPOCH: NaiveDate = match NaiveDate::from_ymd_opt(1970, 1, 1) {
    Some(d) => d,
    None => unreachable!(),
};

/// Where the artifacts live, and how to map a root symbol to an instrument.
pub struct SurfaceFiles {
    pub params: PathBuf,
    pub forwards: PathBuf,
    pub loadings: PathBuf,
    pub scores: PathBuf,
}

impl SurfaceFiles {
    #[must_use]
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            params: dir.join("vol_surface_params.parquet"),
            forwards: dir.join("option_forwards.parquet"),
            loadings: dir.join("vol_pca_loadings.parquet"),
            scores: dir.join("vol_pca_scores.parquet"),
        }
    }

    #[must_use]
    pub fn all_present(&self) -> bool {
        [&self.params, &self.forwards, &self.loadings, &self.scores]
            .iter()
            .all(|p| p.exists())
    }
}

fn batches(path: &Path) -> Option<Vec<RecordBatch>> {
    let file = File::open(path).ok()?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file).ok()?.build().ok()?;
    reader.collect::<Result<Vec<_>, _>>().ok()
}

fn f64s<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Float64Array> {
    b.column(b.schema().index_of(name).ok()?).as_any().downcast_ref()
}
fn strs<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    b.column(b.schema().index_of(name).ok()?).as_any().downcast_ref()
}
fn dates<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Date32Array> {
    b.column(b.schema().index_of(name).ok()?).as_any().downcast_ref()
}
fn u32s<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a UInt32Array> {
    b.column(b.schema().index_of(name).ok()?).as_any().downcast_ref()
}
fn day(a: &Date32Array, i: usize) -> NaiveDate {
    EPOCH + chrono::Duration::days(i64::from(a.value(i)))
}

/// Build a provider for the given roots, as of the latest session at or before
/// `as_of`.
///
/// A missing or unreadable artifact yields an empty provider rather than an
/// error: options are an opt-in part of the book, and a dashboard of equities
/// should not fail to render because a surface file has not been built yet.
#[must_use]
pub fn load(
    files: &SurfaceFiles,
    roots: &HashMap<String, InstrumentId>,
    as_of: NaiveDate,
) -> StaticVolSurfaceProvider {
    let mut provider = StaticVolSurfaceProvider::new();
    if roots.is_empty() || !files.all_present() {
        return provider;
    }
    let (Some(params), Some(fwds), Some(loads), Some(scores)) = (
        batches(&files.params),
        batches(&files.forwards),
        batches(&files.loadings),
        batches(&files.scores),
    ) else {
        return provider;
    };

    for (root, id) in roots {
        // Latest session not after `as_of`: a report dated today should not
        // read a surface fitted from tomorrow's quotes.
        let Some(session) = latest_session(&params, root, as_of) else { continue };
        let slices = read_slices(&params, root, session);
        if slices.len() < 2 {
            continue;
        }
        let (forwards, rate) = read_forwards(&fwds, root, session);
        if forwards.is_empty() {
            continue;
        }
        let Some((cells, pca)) = read_factors(&loads, &scores, root) else { continue };
        provider.insert(
            *id,
            SurfaceSnapshot { forwards, rate, slices, cells, pca },
        );
    }
    provider
}

fn latest_session(params: &[RecordBatch], root: &str, as_of: NaiveDate) -> Option<NaiveDate> {
    let mut best: Option<NaiveDate> = None;
    for b in params {
        let (Some(r), Some(d)) = (strs(b, "root"), dates(b, "quote_date")) else { continue };
        for i in 0..b.num_rows() {
            if r.value(i) != root {
                continue;
            }
            let dt = day(d, i);
            if dt <= as_of && best.is_none_or(|x| dt > x) {
                best = Some(dt);
            }
        }
    }
    best
}

fn read_slices(params: &[RecordBatch], root: &str, session: NaiveDate) -> Vec<FittedSlice> {
    let mut out = Vec::new();
    for b in params {
        let (Some(r), Some(d)) = (strs(b, "root"), dates(b, "quote_date")) else { continue };
        let cols = ["tte", "a", "b", "rho", "m", "sigma", "k_lo", "k_hi"]
            .map(|n| f64s(b, n));
        let [Some(tte), Some(a), Some(bb), Some(rho), Some(m), Some(sg), Some(lo), Some(hi)] = cols
        else {
            continue;
        };
        for i in 0..b.num_rows() {
            if r.value(i) != root || day(d, i) != session {
                continue;
            }
            out.push(FittedSlice {
                tte: tte.value(i),
                svi: Svi {
                    a: a.value(i),
                    b: bb.value(i),
                    rho: rho.value(i),
                    m: m.value(i),
                    sigma: sg.value(i),
                },
                k_lo: lo.value(i),
                k_hi: hi.value(i),
            });
        }
    }
    out.sort_by(|x, y| x.tte.partial_cmp(&y.tte).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn read_forwards(
    fwds: &[RecordBatch],
    root: &str,
    session: NaiveDate,
) -> (Vec<(f64, f64)>, f64) {
    let mut out = Vec::new();
    let mut rate = 0.0;
    for b in fwds {
        let (Some(r), Some(d)) = (strs(b, "root"), dates(b, "quote_date")) else { continue };
        let (Some(tte), Some(f), Some(cr)) =
            (f64s(b, "tte"), f64s(b, "forward"), f64s(b, "curve_rate"))
        else {
            continue;
        };
        for i in 0..b.num_rows() {
            if r.value(i) != root || day(d, i) != session {
                continue;
            }
            out.push((tte.value(i), f.value(i)));
            rate = cr.value(i);
        }
    }
    out.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    (out, rate)
}

/// Loadings define the cell order; the score series must be read in that same
/// component order or the factors would be silently transposed.
fn read_factors(
    loads: &[RecordBatch],
    scores: &[RecordBatch],
    root: &str,
) -> Option<(Vec<Cell>, PcaFit)> {
    let mut cells: Vec<Cell> = Vec::new();
    let mut by_pc: HashMap<u32, Vec<f64>> = HashMap::new();
    let mut mean: Vec<f64> = Vec::new();
    let mut sd: Vec<f64> = Vec::new();
    let mut explained: HashMap<u32, f64> = HashMap::new();

    for b in loads {
        let Some(r) = strs(b, "root") else { continue };
        let (Some(pc), Some(z), Some(tte), Some(l), Some(cm), Some(cs), Some(ex)) = (
            u32s(b, "pc"),
            f64s(b, "z"),
            f64s(b, "tte"),
            f64s(b, "loading"),
            f64s(b, "cell_mean"),
            f64s(b, "cell_sd"),
            f64s(b, "explained"),
        ) else {
            continue;
        };
        for i in 0..b.num_rows() {
            if r.value(i) != root {
                continue;
            }
            let component = pc.value(i);
            explained.insert(component, ex.value(i));
            if component == 1 {
                cells.push(Cell { z: z.value(i), tte: tte.value(i) });
                mean.push(cm.value(i));
                sd.push(cs.value(i));
            }
            by_pc.entry(component).or_default().push(l.value(i));
        }
    }
    if cells.is_empty() {
        return None;
    }

    let mut components: Vec<u32> = by_pc.keys().copied().collect();
    components.sort_unstable();
    let loadings: Vec<Vec<f64>> = components.iter().map(|c| by_pc[c].clone()).collect();
    if loadings.iter().any(|v| v.len() != cells.len()) {
        return None;
    }

    let mut series: Vec<Vec<f64>> = Vec::new();
    let mut rows: HashMap<NaiveDate, HashMap<u32, f64>> = HashMap::new();
    for b in scores {
        let (Some(r), Some(d)) = (strs(b, "root"), dates(b, "quote_date")) else { continue };
        let (Some(pc), Some(s)) = (u32s(b, "pc"), f64s(b, "score")) else { continue };
        for i in 0..b.num_rows() {
            if r.value(i) != root {
                continue;
            }
            rows.entry(day(d, i)).or_default().insert(pc.value(i), s.value(i));
        }
    }
    let mut ordered: Vec<NaiveDate> = rows.keys().copied().collect();
    ordered.sort_unstable();
    for date in ordered {
        let row = &rows[&date];
        if components.iter().all(|c| row.contains_key(c)) {
            series.push(components.iter().map(|c| row[c]).collect());
        }
    }

    Some((
        cells,
        PcaFit {
            mean,
            sd,
            loadings,
            explained: components.iter().map(|c| explained[c]).collect(),
            scores: series,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ptf_engine::surface::VolSurfaceProvider;
    use ptf_engine::vol::OptionRight;

    /// Reads the artifacts `crates/surface` writes, if they are present.
    ///
    /// Skipped rather than failed when they are not: the files are build
    /// output, not fixtures, and a clean checkout has none. When they are
    /// there this is the only test that exercises the whole chain — parquet
    /// through SVI, forwards and factors to a priced contract.
    #[test]
    fn loads_the_real_artifacts_if_built() {
        let dir = std::path::Path::new("../../services/prices/data");
        let files = SurfaceFiles::in_dir(dir);
        if !files.all_present() {
            eprintln!("surface artifacts absent, skipping");
            return;
        }
        let id = InstrumentId::new();
        let roots = HashMap::from([("SOXX".to_string(), id)]);
        let as_of = NaiveDate::from_ymd_opt(2026, 8, 20).unwrap();
        let provider = load(&files, &roots, as_of);
        let snap = provider.surface(id, as_of).expect("SOXX surface");

        assert!(snap.slices.len() >= 8, "{} slices", snap.slices.len());
        assert!(
            snap.slices.windows(2).all(|w| w[1].tte > w[0].tte),
            "slices must be ordered by maturity"
        );
        assert!(
            snap.forwards.windows(2).all(|w| w[1].0 > w[0].0),
            "forwards must be ordered by maturity"
        );
        assert!((0.0..0.10).contains(&snap.rate), "implied rate {}", snap.rate);
        assert_eq!(snap.pca.components(), 3);
        assert_eq!(snap.pca.cells(), snap.cells.len());
        assert!(snap.pca.scores.len() > 200, "{} scores", snap.pca.scores.len());

        // A three-month at-the-money call, priced off the real surface.
        let tau = 0.25;
        let fwd = snap.forward(tau).expect("forward");
        let atm = snap.vol(0.0, tau).expect("atm vol");
        assert!((0.2..1.0).contains(&atm), "ATM vol {atm} is not plausible for SOXX");
        let px = snap
            .price_contract(OptionRight::Call, fwd, tau, 1.0, &[])
            .expect("price");
        // An at-the-money call is worth roughly 0.4 * sigma * sqrt(T) * F.
        let approx = 0.4 * atm * tau.sqrt() * fwd;
        assert!(
            (px - approx).abs() / approx < 0.15,
            "ATM call {px:.2} vs approximation {approx:.2} (F {fwd:.2}, vol {atm:.4})"
        );
        eprintln!(
            "SOXX {as_of}: {} slices, F(3m) {fwd:.2}, rate {:.3}%, ATM vol {:.4}, 3m ATM call {px:.2}",
            snap.slices.len(),
            snap.rate * 100.0,
            atm
        );

        // The factor model must move it: a one-unit level shock should lift the
        // price, since a call is long vega.
        let shocked = snap
            .price_contract(OptionRight::Call, fwd, tau, 1.0, &[1.0, 0.0, 0.0])
            .expect("shocked price");
        eprintln!("  with a +1 PC1 shock: {shocked:.2} ({:+.2}%)", 100.0 * (shocked / px - 1.0));
        // A long call is long vega, so a positive level shock must raise it.
        assert!(
            shocked > px * 1.0001,
            "the factor model must move the price: {shocked} vs {px}"
        );
    }
}
