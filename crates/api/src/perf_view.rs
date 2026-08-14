//! Performance-ratio tab: a risk-adjusted "tearsheet" for the current book.
//!
//! Everything here is derived from the trailing daily-return series of the
//! portfolio equity curve (see [`crate::equity`]). We surface three layers:
//! snapshot (headline ratios over the full window), rolling (a trailing-window
//! Sharpe/Sortino series for the trend chart), and byHorizon (the same ratios
//! over 1M/3M/6M/1Y/ITD windows). When a benchmark series is supplied we add
//! benchmark-relative stats (beta, alpha, information ratio, Treynor, capture).
//!
//! Returns are simple daily returns; ratios annualize with √252. The risk-free
//! rate is a caller-supplied annual figure (0 ⇒ ratios collapse to return/vol).
//! Undefined ratios (zero-volatility / no-downside windows) return 0 as an "n/a"
//! sentinel rather than infinities.
#![allow(clippy::cast_precision_loss, clippy::many_single_char_names)]

use std::collections::BTreeMap;

use chrono::NaiveDate;
use ptf_engine::{Portfolio, PortfolioState};
use serde::Serialize;

use crate::equity;
use crate::error::ApiError;
use crate::price_source::PriceData;

const TRADING_DAYS: f64 = 252.0;
const ROLL_WINDOW: usize = 63; // ~3 trading months

/// A benchmark's base-currency value on each historical date, for relative stats.
pub struct BenchmarkSeries {
    pub symbol: String,
    pub values: BTreeMap<NaiveDate, f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformancePayload {
    pub as_of: String,
    pub base_ccy: String,
    pub rf_annual_pct: f64,
    /// Number of daily returns backing the snapshot (window length − 1).
    pub sample_days: usize,
    pub snapshot: RatioSet,
    pub rolling: Rolling,
    pub by_horizon: Vec<HorizonRow>,
    /// Present only when a benchmark series was available.
    pub relative: Option<RelativeStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RatioSet {
    pub sharpe: f64,
    pub sortino: f64,
    pub calmar: f64,
    pub omega: f64,
    pub ann_return_pct: f64,
    pub ann_vol_pct: f64,
    pub downside_dev_pct: f64,
    pub max_drawdown_pct: f64,
    pub best_day_pct: f64,
    pub worst_day_pct: f64,
    pub win_rate_pct: f64,
    pub skew: f64,
    pub kurtosis: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rolling {
    pub window: usize,
    pub dates: Vec<String>,
    /// Aligned to `dates`; `null` until a full window is available.
    pub sharpe: Vec<Option<f64>>,
    pub sortino: Vec<Option<f64>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HorizonRow {
    pub label: String,
    pub days: usize,
    pub sharpe: f64,
    pub sortino: f64,
    pub calmar: f64,
    pub ann_return_pct: f64,
    pub ann_vol_pct: f64,
    pub max_drawdown_pct: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelativeStats {
    pub benchmark: String,
    pub beta: f64,
    pub alpha_pct: f64,
    pub information_ratio: f64,
    pub treynor: f64,
    pub correlation: f64,
    pub r_squared: f64,
    pub up_capture_pct: f64,
    pub down_capture_pct: f64,
    pub benchmark_ann_return_pct: f64,
}

/// Build the performance payload from the current book's equity curve.
pub fn build(
    portfolio: &Portfolio,
    state: &PortfolioState,
    pd: &PriceData,
    as_of: NaiveDate,
    lookback_days: u32,
    rf_annual: f64,
    benchmark: Option<&BenchmarkSeries>,
) -> Result<PerformancePayload, ApiError> {
    let base = portfolio.base_currency;
    let (equity, dates) = equity::series(state, pd, base, as_of, lookback_days, None)?;
    let rets = returns(&equity);
    let rf_daily = rf_annual / TRADING_DAYS;

    let snapshot = ratio_set(&equity, &rets, rf_daily);
    let rolling = rolling_series(&dates, &rets, rf_daily);
    let by_horizon = horizons(&equity, &rets, rf_daily);
    let relative = benchmark.and_then(|b| relative_stats(&dates, &rets, rf_daily, b));

    Ok(PerformancePayload {
        as_of: as_of.format("%Y-%m-%d").to_string(),
        base_ccy: base.to_string(),
        rf_annual_pct: rf_annual * 100.0,
        sample_days: rets.len(),
        snapshot,
        rolling,
        by_horizon,
        relative,
    })
}

/// Simple daily returns from an equity curve (length `equity.len() - 1`).
fn returns(equity: &[f64]) -> Vec<f64> {
    equity
        .windows(2)
        .map(|w| if w[0] == 0.0 { 0.0 } else { w[1] / w[0] - 1.0 })
        .collect()
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Sample standard deviation (n − 1).
fn stdev(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    let var = xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (xs.len() - 1) as f64;
    var.sqrt()
}

/// Downside deviation below a minimum-acceptable daily return (population form).
fn downside_dev(xs: &[f64], mar: f64) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let sq = xs
        .iter()
        .map(|x| (x - mar).min(0.0))
        .map(|d| d * d)
        .sum::<f64>()
        / xs.len() as f64;
    sq.sqrt()
}

/// Geometric annualized return (CAGR) implied by a return series.
fn ann_return(rets: &[f64]) -> f64 {
    if rets.is_empty() {
        return 0.0;
    }
    let growth: f64 = rets.iter().map(|r| 1.0 + r).product();
    if growth <= 0.0 {
        return -1.0;
    }
    growth.powf(TRADING_DAYS / rets.len() as f64) - 1.0
}

fn sharpe(rets: &[f64], rf_daily: f64) -> f64 {
    let s = stdev(rets);
    if s == 0.0 {
        return 0.0;
    }
    (mean(rets) - rf_daily) / s * TRADING_DAYS.sqrt()
}

fn sortino(rets: &[f64], rf_daily: f64) -> f64 {
    let dd = downside_dev(rets, rf_daily);
    if dd == 0.0 {
        return 0.0;
    }
    (mean(rets) - rf_daily) / dd * TRADING_DAYS.sqrt()
}

fn omega(rets: &[f64], thr: f64) -> f64 {
    let (gain, loss): (f64, f64) = rets.iter().fold((0.0, 0.0), |(g, l), &r| {
        let d = r - thr;
        if d >= 0.0 { (g + d, l) } else { (g, l - d) }
    });
    if loss == 0.0 { 0.0 } else { gain / loss }
}

/// Max drawdown as a negative percentage (0 when the curve only rises).
fn max_drawdown_pct(equity: &[f64]) -> f64 {
    let mut peak = f64::MIN;
    let mut mdd = 0.0f64;
    for &e in equity {
        peak = peak.max(e);
        if peak > 0.0 {
            mdd = mdd.min(e / peak - 1.0);
        }
    }
    mdd * 100.0
}

fn skewness(rets: &[f64]) -> f64 {
    let n = rets.len();
    let s = stdev(rets);
    if n < 3 || s == 0.0 {
        return 0.0;
    }
    let m = mean(rets);
    let sum: f64 = rets.iter().map(|x| ((x - m) / s).powi(3)).sum();
    sum * n as f64 / ((n - 1) as f64 * (n - 2) as f64)
}

/// Excess kurtosis (0 for a normal distribution).
fn kurtosis(rets: &[f64]) -> f64 {
    let n = rets.len();
    let s = stdev(rets);
    if n < 4 || s == 0.0 {
        return 0.0;
    }
    let m = mean(rets);
    let sum: f64 = rets.iter().map(|x| ((x - m) / s).powi(4)).sum();
    let nf = n as f64;
    let a = nf * (nf + 1.0) / ((nf - 1.0) * (nf - 2.0) * (nf - 3.0));
    let b = 3.0 * (nf - 1.0) * (nf - 1.0) / ((nf - 2.0) * (nf - 3.0));
    a * sum - b
}

fn calmar(rets: &[f64], equity: &[f64]) -> f64 {
    let mdd = max_drawdown_pct(equity) / 100.0;
    if mdd == 0.0 {
        0.0
    } else {
        ann_return(rets) / mdd.abs()
    }
}

fn ratio_set(equity: &[f64], rets: &[f64], rf_daily: f64) -> RatioSet {
    RatioSet {
        sharpe: sharpe(rets, rf_daily),
        sortino: sortino(rets, rf_daily),
        calmar: calmar(rets, equity),
        omega: omega(rets, rf_daily),
        ann_return_pct: ann_return(rets) * 100.0,
        ann_vol_pct: stdev(rets) * TRADING_DAYS.sqrt() * 100.0,
        downside_dev_pct: downside_dev(rets, rf_daily) * TRADING_DAYS.sqrt() * 100.0,
        max_drawdown_pct: max_drawdown_pct(equity),
        best_day_pct: rets.iter().copied().fold(f64::MIN, f64::max).max(0.0) * 100.0,
        worst_day_pct: rets.iter().copied().fold(f64::MAX, f64::min).min(0.0) * 100.0,
        win_rate_pct: if rets.is_empty() {
            0.0
        } else {
            rets.iter().filter(|&&r| r > 0.0).count() as f64 / rets.len() as f64 * 100.0
        },
        skew: skewness(rets),
        kurtosis: kurtosis(rets),
    }
}

/// Trailing-window Sharpe/Sortino aligned to `dates` (null until a full window).
fn rolling_series(dates: &[NaiveDate], rets: &[f64], rf_daily: f64) -> Rolling {
    let n = dates.len();
    let mut sharpe_v = vec![None; n];
    let mut sortino_v = vec![None; n];
    // return index j corresponds to dates[j + 1].
    for j in 0..rets.len() {
        if j + 1 >= ROLL_WINDOW {
            let win = &rets[j + 1 - ROLL_WINDOW..=j];
            sharpe_v[j + 1] = Some(round2(sharpe(win, rf_daily)));
            sortino_v[j + 1] = Some(round2(sortino(win, rf_daily)));
        }
    }
    Rolling {
        window: ROLL_WINDOW,
        dates: equity::iso(dates),
        sharpe: sharpe_v,
        sortino: sortino_v,
    }
}

fn horizons(equity: &[f64], rets: &[f64], rf_daily: f64) -> Vec<HorizonRow> {
    let n = rets.len();
    let mut out = Vec::new();
    let windows = [("1M", 21usize), ("3M", 63), ("6M", 126), ("1Y", 252)];
    for (label, days) in windows {
        if days <= n {
            let r = &rets[n - days..];
            let e = &equity[equity.len() - days - 1..];
            out.push(horizon_row(label, days, e, r, rf_daily));
        }
    }
    if n >= 2 {
        out.push(horizon_row("ITD", n, equity, rets, rf_daily));
    }
    out
}

fn horizon_row(
    label: &str,
    days: usize,
    equity: &[f64],
    rets: &[f64],
    rf_daily: f64,
) -> HorizonRow {
    HorizonRow {
        label: label.to_string(),
        days,
        sharpe: round2(sharpe(rets, rf_daily)),
        sortino: round2(sortino(rets, rf_daily)),
        calmar: round2(calmar(rets, equity)),
        ann_return_pct: round2(ann_return(rets) * 100.0),
        ann_vol_pct: round2(stdev(rets) * TRADING_DAYS.sqrt() * 100.0),
        max_drawdown_pct: round2(max_drawdown_pct(equity)),
    }
}

/// Benchmark-relative stats. Aligns the benchmark to the portfolio's dates
/// (forward-filled) and returns `None` if overlap is too thin to be meaningful.
fn relative_stats(
    dates: &[NaiveDate],
    rets: &[f64],
    rf_daily: f64,
    bench: &BenchmarkSeries,
) -> Option<RelativeStats> {
    // Benchmark value on each portfolio date, forward-filled across gaps.
    let mut bench_eq = Vec::with_capacity(dates.len());
    let mut last: Option<f64> = None;
    let mut hits = 0usize;
    for d in dates {
        if let Some(&v) = bench.values.get(d) {
            last = Some(v);
            hits += 1;
        }
        bench_eq.push(last);
    }
    // Need coverage over most of the window and a value on the first date.
    if hits * 2 < dates.len() || bench_eq.first().copied().flatten().is_none() {
        return None;
    }
    let bench_eq: Vec<f64> = bench_eq.into_iter().map(|v| v.unwrap_or(0.0)).collect();
    let rb = returns(&bench_eq);
    if rb.len() != rets.len() || rb.len() < 2 {
        return None;
    }

    let mr = mean(rets);
    let mb = mean(&rb);
    let var_b = rb.iter().map(|x| (x - mb) * (x - mb)).sum::<f64>() / (rb.len() - 1) as f64;
    let cov = rets
        .iter()
        .zip(&rb)
        .map(|(p, b)| (p - mr) * (b - mb))
        .sum::<f64>()
        / (rb.len() - 1) as f64;
    let beta = if var_b == 0.0 { 0.0 } else { cov / var_b };
    let alpha = (mr - rf_daily - beta * (mb - rf_daily)) * TRADING_DAYS;

    let active: Vec<f64> = rets.iter().zip(&rb).map(|(p, b)| p - b).collect();
    let te = stdev(&active);
    let information_ratio = if te == 0.0 {
        0.0
    } else {
        mean(&active) / te * TRADING_DAYS.sqrt()
    };
    let treynor = if beta == 0.0 {
        0.0
    } else {
        (mr - rf_daily) * TRADING_DAYS / beta
    };
    let corr = {
        let denom = stdev(rets) * stdev(rb.as_slice());
        if denom == 0.0 { 0.0 } else { cov / denom }
    };

    // Up/down capture: portfolio mean vs benchmark mean on benchmark up/down days.
    let up: Vec<usize> = (0..rb.len()).filter(|&i| rb[i] > 0.0).collect();
    let down: Vec<usize> = (0..rb.len()).filter(|&i| rb[i] < 0.0).collect();
    let capture = |idx: &[usize]| -> f64 {
        if idx.is_empty() {
            return 0.0;
        }
        let pb: f64 = idx.iter().map(|&i| rb[i]).sum::<f64>() / idx.len() as f64;
        if pb == 0.0 {
            return 0.0;
        }
        let pp: f64 = idx.iter().map(|&i| rets[i]).sum::<f64>() / idx.len() as f64;
        pp / pb * 100.0
    };

    Some(RelativeStats {
        benchmark: bench.symbol.clone(),
        beta: round2(beta),
        alpha_pct: round2(alpha * 100.0),
        information_ratio: round2(information_ratio),
        treynor: round2(treynor * 100.0),
        correlation: round2(corr),
        r_squared: round2(corr * corr),
        up_capture_pct: round2(capture(&up)),
        down_capture_pct: round2(capture(&down)),
        benchmark_ann_return_pct: round2(ann_return(&rb) * 100.0),
    })
}

fn round2(x: f64) -> f64 {
    if x.is_finite() {
        (x * 100.0).round() / 100.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_and_moments() {
        let eq = vec![100.0, 101.0, 100.0, 102.0];
        let r = returns(&eq);
        assert_eq!(r.len(), 3);
        assert!((mean(&r) - r.iter().sum::<f64>() / 3.0).abs() < 1e-12);
        assert!(stdev(&r) > 0.0);
    }

    #[test]
    fn sharpe_positive_for_uptrend() {
        // Noisy but upward-drifting series → positive Sharpe & Sortino, zero rf.
        // A tiny alternating wiggle keeps some downside days so Sortino is finite.
        let eq: Vec<f64> = (0..60)
            .map(|i| {
                let wiggle = if i % 2 == 0 { 1.0 } else { 0.996 };
                100.0 * 1.002_f64.powi(i) * wiggle
            })
            .collect();
        let r = returns(&eq);
        assert!(sharpe(&r, 0.0) > 0.0, "sharpe {}", sharpe(&r, 0.0));
        assert!(sortino(&r, 0.0) > 0.0, "sortino {}", sortino(&r, 0.0));
        // With downside present, Sortino divides by a smaller denominator ⇒ ≥ Sharpe.
        assert!(sortino(&r, 0.0) >= sharpe(&r, 0.0));
    }

    #[test]
    fn drawdown_is_negative_on_dip() {
        let eq = vec![100.0, 120.0, 90.0, 110.0];
        assert!((max_drawdown_pct(&eq) - (-25.0)).abs() < 1e-9);
    }

    #[test]
    #[allow(clippy::float_cmp)] // functions return an exact 0.0 sentinel here
    fn flat_series_gives_zero_ratios() {
        let eq = vec![100.0; 30];
        let r = returns(&eq);
        assert_eq!(sharpe(&r, 0.0), 0.0);
        assert_eq!(sortino(&r, 0.0), 0.0);
        assert_eq!(max_drawdown_pct(&eq), 0.0);
    }

    #[test]
    fn beta_one_when_identical() {
        let dates: Vec<NaiveDate> = (0..40)
            .map(|i| NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(i))
            .collect();
        let eq: Vec<f64> = (0..40).map(|i| 100.0 + f64::from(i)).collect();
        let rets = returns(&eq);
        let values: BTreeMap<NaiveDate, f64> =
            dates.iter().zip(&eq).map(|(d, &v)| (*d, v)).collect();
        let bench = BenchmarkSeries {
            symbol: "BM".into(),
            values,
        };
        let rel = relative_stats(&dates, &rets, 0.0, &bench).unwrap();
        assert!((rel.beta - 1.0).abs() < 1e-6);
        assert!((rel.correlation - 1.0).abs() < 1e-6);
    }
}
