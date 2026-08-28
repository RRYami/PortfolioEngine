//! Rolling out-of-sample backtest of the surface risk model.
//!
//! For each day in the out-of-sample window the model is refitted on only the
//! preceding sessions, a one-day `VaR` is produced, and the *next* session's
//! realised move is compared against it. Nothing from after the decision date
//! is used to make it, which is the whole point: a model scored on data it was
//! fitted to will always look good.
//!
//! The portfolio is re-formed each day rather than held: a one-day `VaR` is a
//! claim about the next day only, so each observation stands alone and there is
//! no rolling of expiring contracts to model.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use ptf_engine::backtest::{BacktestReport, Outcome, assess};
use ptf_engine::grid::FittedSlice;
use ptf_engine::pca::fit;
use ptf_engine::surface::{Cell, SurfaceSnapshot};
use ptf_engine::vol::OptionRight;

use crate::build::{ForwardRow, GridRow, SviRow};
use crate::factors::{COMPONENTS, MAX_EXTRAPOLATED, MIN_CELL_COVERAGE};

/// Sessions used to fit before the first scored day.
pub const FIT_WINDOW: usize = 252;

/// Paths a Monte Carlo draws per scored day. Lower than a production report
/// because the whole window is rescored on every run and the quantile only
/// needs to be stable to a fraction of a percent.
pub const PATHS: usize = 4000;

/// What the portfolio holds. Re-formed each day at the money.
#[derive(Debug, Clone, Copy)]
pub struct Book {
    pub shares: f64,
    /// Contracts of a roughly at-the-money call at `option_tenor`.
    pub calls: f64,
    pub option_tenor: f64,
}

pub struct DayResult {
    /// The session the outcome was realised on, for anyone plotting the series.
    #[allow(dead_code)]
    pub date: NaiveDate,
    pub value: f64,
    pub var95: f64,
    pub var99: f64,
    pub realised_loss: f64,
}

/// Everything one session contributes.
struct Session {
    slices: Vec<FittedSlice>,
    forwards: Vec<(f64, f64)>,
    rate: f64,
    grid: BTreeMap<(i64, i64), (f64, bool)>,
}

fn key(x: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let k = (x * 1e6).round() as i64;
    k
}

/// Run the backtest for one root.
///
/// Returns per-day results plus the assessment at both confidence levels.
#[allow(clippy::too_many_lines)]
pub fn run(
    svis: &[SviRow],
    forwards: &[ForwardRow],
    grid: &[GridRow],
    book: Book,
    seed: u64,
) -> Option<(Vec<DayResult>, BacktestReport, BacktestReport)> {
    // Assemble per-session inputs.
    let mut sessions: BTreeMap<NaiveDate, Session> = BTreeMap::new();
    for s in svis {
        let e = sessions.entry(s.quote_date).or_insert_with(|| Session {
            slices: Vec::new(),
            forwards: Vec::new(),
            rate: 0.0,
            grid: BTreeMap::new(),
        });
        e.slices.push(FittedSlice { tte: s.tte, svi: s.params, k_lo: s.k_lo, k_hi: s.k_hi });
    }
    for f in forwards {
        if let Some(s) = sessions.get_mut(&f.quote_date) {
            s.forwards.push((f.tte, f.forward));
            s.rate = f.curve_rate;
        }
    }
    for g in grid {
        if let Some(s) = sessions.get_mut(&g.quote_date) {
            s.grid.insert((key(g.tte), key(g.z)), (g.vol, g.extrapolated));
        }
    }
    for s in sessions.values_mut() {
        s.slices.sort_by(|a, b| a.tte.partial_cmp(&b.tte).unwrap_or(std::cmp::Ordering::Equal));
        s.forwards.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    }
    sessions.retain(|_, s| s.slices.len() >= 2 && !s.forwards.is_empty());

    let dates: Vec<NaiveDate> = sessions.keys().copied().collect();
    if dates.len() < FIT_WINDOW + ptf_engine::backtest::MIN_OBSERVATIONS + 2 {
        return None;
    }

    // Cells fixed once, on the same rules the production fit uses, so the
    // backtest scores the model that would actually be deployed.
    let cells = choose_cells(&sessions)?;
    let usable: Vec<NaiveDate> = dates
        .iter()
        .copied()
        .filter(|d| cells.iter().all(|c| sessions[d].grid.contains_key(c)))
        .collect();
    if usable.len() < FIT_WINDOW + 2 {
        return None;
    }

    let mut rng = Lcg::new(seed);
    let mut results = Vec::new();
    // `i` indexes the day the decision is made; it is scored against `i + 1`.
    for i in FIT_WINDOW..usable.len() - 1 {
        let today = usable[i];
        let tomorrow = usable[i + 1];

        // Refit on the preceding window only.
        let panel: Vec<Vec<f64>> = (i - FIT_WINDOW + 1..=i)
            .map(|j| {
                let (prev, cur) = (&sessions[&usable[j - 1]].grid, &sessions[&usable[j]].grid);
                cells.iter().map(|c| (cur[c].0 / prev[c].0).ln()).collect()
            })
            .collect();
        let Ok(pca) = fit(&panel, COMPONENTS) else { continue };

        let snapshot = SurfaceSnapshot {
            forwards: forwards_of(&sessions[&today]),
            rate: sessions[&today].rate,
            slices: sessions[&today].slices.clone(),
            cells: cells.iter().map(|c| Cell { z: unkey(c.1), tte: unkey(c.0) }).collect(),
            pca,
        };
        let Some(spot) = snapshot.forward(book.option_tenor) else { continue };
        let strike = (spot / 5.0).round() * 5.0;
        let Some(premium) =
            snapshot.price_contract(OptionRight::Call, strike, book.option_tenor, 1.0, &[])
        else {
            continue;
        };
        let value = book.shares.mul_add(spot, book.calls * premium * 100.0);

        // Spot and the vol factors are drawn *jointly*, from the covariance of
        // the same window the model was fitted on. Drawing them independently
        // would discard the leverage effect and score a different model than
        // the engine runs: a book holding a call is long vega and long delta,
        // and it matters a great deal that vol rises exactly when spot falls.
        //
        // Spot returns come from the parity forward series rather than a
        // closing price, because the forward falls out of the same 15:45
        // snapshot as the surface. A close from a separate feed is fifteen
        // minutes out of step, and that mismatch lands directly in the
        // correlation this is here to capture.
        let window = &usable[i - FIT_WINDOW + 1..=i];
        let spot_rets = forward_returns(window, &sessions);
        let mut factors: Vec<Vec<f64>> = vec![spot_rets];
        for j in 0..COMPONENTS {
            let series: Vec<f64> = snapshot.pca.scores.iter().map(|r| r[j]).collect();
            factors.push(series);
        }
        let common = factors.iter().map(Vec::len).min().unwrap_or(0);
        if common < 2 {
            continue;
        }
        for f in &mut factors {
            let start = f.len() - common;
            f.drain(..start);
        }
        let Some(chol) = cholesky(&covariance(&factors)) else { continue };

        let mut pnl: Vec<f64> = Vec::with_capacity(PATHS);
        for _ in 0..PATHS {
            let z: Vec<f64> = (0..factors.len()).map(|_| rng.normal()).collect();
            let draw = mat_vec(&chol, &z);
            let ret = draw[0];
            let scores: Vec<f64> = draw[1..].to_vec();
            let ratio = ret.exp();
            let tau = book.option_tenor - 1.0 / 252.0;
            let Some(v) =
                snapshot.price_contract(OptionRight::Call, strike, tau, ratio, &scores)
            else {
                continue;
            };
            let sim = book.shares.mul_add(spot * ratio, book.calls * v * 100.0);
            pnl.push(value - sim); // loss-positive
        }
        if pnl.len() < PATHS / 2 {
            continue;
        }
        pnl.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Realised: the same contract, one session later, on tomorrow's surface.
        let next = &sessions[&tomorrow];
        let next_snapshot = SurfaceSnapshot {
            forwards: forwards_of(next),
            rate: next.rate,
            slices: next.slices.clone(),
            cells: Vec::new(),
            pca: snapshot.pca.clone(),
        };
        let Some(next_spot) = next_snapshot.forward(book.option_tenor) else { continue };
        let Some(next_premium) = next_snapshot.price_contract(
            OptionRight::Call,
            strike,
            book.option_tenor - 1.0 / 252.0,
            1.0,
            &[],
        ) else {
            continue;
        };
        let next_value = book.shares.mul_add(next_spot, book.calls * next_premium * 100.0);

        results.push(DayResult {
            date: tomorrow,
            value,
            var95: quantile(&pnl, 0.95),
            var99: quantile(&pnl, 0.99),
            realised_loss: value - next_value,
        });
    }

    let out95: Vec<Outcome> = results
        .iter()
        .map(|r| Outcome { var: r.var95, loss: r.realised_loss })
        .collect();
    let out99: Vec<Outcome> = results
        .iter()
        .map(|r| Outcome { var: r.var99, loss: r.realised_loss })
        .collect();
    let a = assess(&out95, 0.95).ok()?;
    let b = assess(&out99, 0.99).ok()?;
    Some((results, a, b))
}

fn unkey(k: i64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let v = k as f64 / 1e6;
    v
}

fn forwards_of(s: &Session) -> Vec<(f64, f64)> {
    s.forwards.clone()
}

fn choose_cells(sessions: &BTreeMap<NaiveDate, Session>) -> Option<Vec<(i64, i64)>> {
    let n = sessions.len();
    #[allow(clippy::cast_precision_loss)]
    let total = n as f64;
    let mut seen: BTreeMap<(i64, i64), (usize, usize)> = BTreeMap::new();
    for s in sessions.values() {
        for (k, (_, ex)) in &s.grid {
            let e = seen.entry(*k).or_insert((0, 0));
            e.0 += 1;
            e.1 += usize::from(*ex);
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let kept: Vec<(i64, i64)> = seen
        .iter()
        .filter(|(_, (present, ex))| {
            *present as f64 / total >= MIN_CELL_COVERAGE
                && *ex as f64 / *present as f64 <= MAX_EXTRAPOLATED
        })
        .map(|(k, _)| *k)
        .collect();
    (!kept.is_empty()).then_some(kept)
}

/// Log-returns of the front parity forward across a window.
fn forward_returns(window: &[NaiveDate], sessions: &BTreeMap<NaiveDate, Session>) -> Vec<f64> {
    let fronts: Vec<f64> = window
        .iter()
        .filter_map(|d| sessions.get(d).and_then(|s| s.forwards.first()).map(|f| f.1))
        .collect();
    fronts
        .windows(2)
        .filter(|w| w[0] > 0.0 && w[1] > 0.0)
        .map(|w| (w[1] / w[0]).ln())
        .collect()
}

/// Sample covariance of equal-length series.
fn covariance(series: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = series.len();
    #[allow(clippy::cast_precision_loss)]
    let obs = series[0].len() as f64;
    let means: Vec<f64> = series.iter().map(|s| s.iter().sum::<f64>() / obs).collect();
    let mut cov = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let c: f64 = series[i]
                .iter()
                .zip(series[j].iter())
                .map(|(a, b)| (a - means[i]) * (b - means[j]))
                .sum();
            cov[i][j] = c / (obs - 1.0);
        }
    }
    cov
}

/// Lower-triangular Cholesky factor, or `None` if not positive definite.
fn cholesky(cov: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = cov.len();
    let mut lower = vec![vec![0.0; n]; n];
    for row in 0..n {
        for col in 0..=row {
            let dot: f64 = (0..col).map(|k| lower[row][k] * lower[col][k]).sum();
            if row == col {
                let diag = cov[row][row] - dot;
                if diag <= 0.0 {
                    return None;
                }
                lower[row][col] = diag.sqrt();
            } else {
                lower[row][col] = (cov[row][col] - dot) / lower[col][col];
            }
        }
    }
    Some(lower)
}

fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let idx = ((sorted.len() as f64) * q).ceil() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Small deterministic generator, so a backtest is reproducible.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        #[allow(clippy::cast_precision_loss)]
        let u = (self.0 >> 11) as f64 / 9_007_199_254_740_992.0;
        u
    }
    /// Box-Muller.
    fn normal(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-12);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}
