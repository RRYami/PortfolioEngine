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

mod backtest;
mod build;
mod error;
mod factors;
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
    let mut svis = Vec::new();
    let mut grid = Vec::new();
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
        svis.extend(out.svis);
        grid.extend(out.grid);
    }

    let fwd_path = out.join("option_forwards.parquet");
    let iv_path = out.join("option_iv.parquet");
    let svi_path = out.join("vol_surface_params.parquet");
    let grid_path = out.join("vol_grid.parquet");
    write::forwards(&fwd_path, &forwards)?;
    write::ivs(&iv_path, &ivs)?;
    write::svis(&svi_path, &svis)?;
    write::grid(&grid_path, &grid)?;

    let fits = fit_factors(&grid);
    // Backtest: only meaningful once there is history to hold out.
    if std::env::args().any(|a| a == "--backtest") {
        run_backtest(&svis, &forwards, &grid);
    }

    let load_path = out.join("vol_pca_loadings.parquet");
    let score_path = out.join("vol_pca_scores.parquet");
    write::pca_loadings(&load_path, &fits)?;
    write::pca_scores(&score_path, &fits)?;

    let arb = svis.iter().filter(|s| s.min_durrleman < 0.0).count();
    let worst = svis.iter().map(|s| s.rmse_vol).fold(0.0_f64, f64::max);
    eprintln!(
        "wrote {} ({} slices), {} ({} points), {} ({} smiles, {arb} with a butterfly \
         violation, worst fit {:.4} vol pts)",
        fwd_path.display(),
        forwards.len(),
        iv_path.display(),
        ivs.len(),
        svi_path.display(),
        svis.len(),
        worst
    );
    let extrap = grid.iter().filter(|g| g.extrapolated).count();
    let expected = build::GRID_Z.len() * build::GRID_TAU.len() * chain.len();
    eprintln!(
        "wrote {} ({} cells of {expected} possible, {extrap} extrapolated)",
        grid_path.display(),
        grid.len()
    );
    eprintln!("wrote {} and {}", load_path.display(), score_path.display());
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

/// Score the risk model out of sample and print the verdict.
///
/// The book is 100 shares plus one roughly at-the-money three-month call, so
/// the test exercises both legs the engine handles differently: a linear one
/// and a convex one whose value depends on the surface.
fn run_backtest(svis: &[build::SviRow], forwards: &[build::ForwardRow], grid: &[build::GridRow]) {
    use std::collections::BTreeSet;
    let roots: BTreeSet<&str> = svis.iter().map(|s| s.root.as_str()).collect();
    for root in roots {
        let s: Vec<build::SviRow> =
            svis.iter().filter(|r| r.root == root).cloned().collect();
        let f: Vec<build::ForwardRow> =
            forwards.iter().filter(|r| r.root == root).cloned().collect();
        let g: Vec<build::GridRow> =
            grid.iter().filter(|r| r.root == root).cloned().collect();
        let book = backtest::Book { shares: 100.0, calls: 1.0, option_tenor: 0.25 };
        match backtest::run(&s, &f, &g, book, 0x5eed) {
            Some((days, r95, r99)) => {
                let mean_var = days.iter().map(|d| d.var95).sum::<f64>()
                    / f64::from(u32::try_from(days.len()).unwrap_or(1));
                let mean_value = days.iter().map(|d| d.value).sum::<f64>()
                    / f64::from(u32::try_from(days.len()).unwrap_or(1));
                eprintln!(
                    "\nbacktest {root}: {} scored days, mean book {mean_value:.0}, \
                     mean 95% VaR {mean_var:.0} ({:.2}% of value)",
                    days.len(),
                    100.0 * mean_var / mean_value
                );
                for r in [&r95, &r99] {
                    eprintln!("  {r}");
                    let t = r.transitions;
                    eprintln!(
                        "      transitions quiet->quiet {} quiet->hit {} hit->quiet {} hit->hit {}{}",
                        t[0][0], t[0][1], t[1][0], t[1][1],
                        if t[1][1] == 0 {
                            "  (no back-to-back exceptions: independence is undetermined, not confirmed)"
                        } else {
                            ""
                        }
                    );
                }
                let worst = days
                    .iter()
                    .map(|d| d.realised_loss / d.var95)
                    .fold(0.0_f64, f64::max);
                eprintln!("      worst breach: {worst:.2}x the 95% VaR");
            }
            None => eprintln!("\nbacktest {root}: not enough history"),
        }
    }
}

/// Fit the factor model per root, over the whole history rather than per
/// session: a decomposition needs a window of daily changes, not a snapshot.
fn fit_factors(grid: &[build::GridRow]) -> Vec<factors::FactorFit> {
    let mut by_root: std::collections::BTreeMap<&str, Vec<build::GridRow>> =
        std::collections::BTreeMap::new();
    for g in grid {
        by_root.entry(g.root.as_str()).or_default().push(g.clone());
    }
    let mut fits = Vec::new();
    for (r, rows) in &by_root {
        match factors::fit_root(r, rows) {
            Ok(f) => {
                eprintln!(
                    "  {r}: {} components over {} cells and {} changes; they explain {:.1}% \
                     ({} cells dropped as mostly extrapolated)",
                    f.fit.components(),
                    f.cells.len(),
                    f.dates.len(),
                    f.fit.retained_variance() * 100.0,
                    f.dropped.len()
                );
                fits.push(f);
            }
            Err(e) => eprintln!("  {r}: no factor fit ({e})"),
        }
    }
    fits
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
