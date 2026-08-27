//! Build the implied-vol surface tables from the ingested option chains.
//!
//! Reads `options.parquet` (written by `services/prices`), recovers a forward
//! and discount factor per expiry by put-call parity, inverts the
//! out-of-the-money quotes through the engine's Black-76 kernel, and writes
//! `option_forwards.parquet` and `option_iv.parquet`.
//!
//! Deliberately a separate binary from `crates/api`: this is an offline batch
//! stage that runs after an ingest, not something a request path touches. The
//! engine crate stays free of I/O, which is why the maths lives there and the
//! parquet plumbing lives here.
//!
//! usage: `ptf-surface [<options.parquet>] [--out <dir>]`

mod build;
mod error;
mod quotes;
mod write;

use std::path::PathBuf;
use std::process::ExitCode;

use build::Rejects;

const DEFAULT_IN: &str = "services/prices/data/options.parquet";
const DEFAULT_OUT: &str = "services/prices/data";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ptf-surface: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), error::SurfaceError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map_or_else(|| PathBuf::from(DEFAULT_OUT), PathBuf::from);
    let input = args
        .iter()
        .find(|a| !a.starts_with("--") && a.as_str() != out.to_string_lossy())
        .map_or_else(|| PathBuf::from(DEFAULT_IN), PathBuf::from);

    eprintln!("reading {}", input.display());
    let chain = quotes::load(&input)?;
    let quotes_in: usize =
        chain.values().flat_map(BTreeValues::values).map(|(_, q)| q.len()).sum();
    eprintln!("{} sessions, {quotes_in} quotes", chain.len());

    let mut forwards = Vec::new();
    let mut ivs = Vec::new();
    let mut totals = Rejects::default();
    let mut no_curve = 0usize;

    for ((date, root), slices) in &chain {
        let out = build::build_session(*date, root, slices);
        if out.curve.is_none() {
            no_curve += 1;
            eprintln!("  {date} {root}: no discount curve, session skipped");
        }
        totals.in_the_money += out.rejects.in_the_money;
        totals.unstable += out.rejects.unstable;
        totals.below_intrinsic += out.rejects.below_intrinsic;
        totals.above_ceiling += out.rejects.above_ceiling;
        totals.other += out.rejects.other;
        totals.no_forward += out.rejects.no_forward;
        forwards.extend(out.forwards);
        ivs.extend(out.ivs);
    }

    let fwd_path = out.join("option_forwards.parquet");
    let iv_path = out.join("option_iv.parquet");
    write::forwards(&fwd_path, &forwards)?;
    write::ivs(&iv_path, &ivs)?;

    eprintln!(
        "wrote {} ({} slices) and {} ({} points)",
        fwd_path.display(),
        forwards.len(),
        iv_path.display(),
        ivs.len()
    );
    eprintln!(
        "rejected: {} itm, {} unstable, {} below intrinsic, {} above ceiling, \
         {} other, {} no forward; {no_curve} sessions without a curve",
        totals.in_the_money,
        totals.unstable,
        totals.below_intrinsic,
        totals.above_ceiling,
        totals.other,
        totals.no_forward
    );
    Ok(())
}

/// Tiny helper so the quote count reads without naming the nested map type.
trait BTreeValues {
    type Item;
    fn values(&self) -> std::collections::btree_map::Values<'_, chrono::NaiveDate, Self::Item>;
}

impl BTreeValues for std::collections::BTreeMap<chrono::NaiveDate, (f64, Vec<quotes::Quote>)> {
    type Item = (f64, Vec<quotes::Quote>);
    fn values(&self) -> std::collections::btree_map::Values<'_, chrono::NaiveDate, Self::Item> {
        std::collections::BTreeMap::values(self)
    }
}
