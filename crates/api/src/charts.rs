#![allow(clippy::doc_markdown)]
//! Real chart series for the dashboard, derived from engine output + price
//! history:
//! - **P&L distribution** — binned from the engine's 1-day Monte-Carlo sample.
//! - **Drawdown** — underwater curve of the portfolio equity over the window.
//! - **Historical VaR** — rolling-window VaR from realized portfolio returns,
//!   scaled so the latest point equals the headline VaR.

use crate::risk_view::{ByConf, Cutoffs, Drawdown, HistVar, PnlDistribution};

const BINS: usize = 43;
const WINDOW: usize = 20;

/// (var1d, es1d) at a confidence level.
pub type Tail = (f64, f64);

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn pnl_distribution(pnl: &[f64], c95: Tail, c99: Tail) -> PnlDistribution {
    if pnl.len() < 2 {
        return PnlDistribution {
            paths: pnl.len(),
            bin_low: -1.0,
            bin_high: 1.0,
            bin_count: BINS,
            counts: vec![0; BINS],
            cutoffs: cutoffs(c95, c99, 0.0, 0.0),
        };
    }
    let mut sorted = pnl.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| -> f64 {
        let idx = ((p * sorted.len() as f64) as usize).min(sorted.len() - 1);
        sorted[idx]
    };
    let lo = q(0.004);
    let hi = q(0.996);
    let bw = ((hi - lo) / BINS as f64).max(f64::EPSILON);
    let mut counts = vec![0i64; BINS];
    for &x in pnl {
        if x < lo || x > hi {
            continue;
        }
        let mut bi = ((x - lo) / bw) as usize;
        if bi >= BINS {
            bi = BINS - 1;
        }
        counts[bi] += 1;
    }
    PnlDistribution {
        paths: pnl.len(),
        bin_low: lo,
        bin_high: hi,
        bin_count: BINS,
        counts,
        // deep-tail boundaries are the deepP quantiles of the sample.
        cutoffs: cutoffs(c95, c99, q(0.01), q(0.0025)),
    }
}

fn cutoffs(c95: Tail, c99: Tail, deep95: f64, deep99: f64) -> ByConf<Cutoffs> {
    ByConf {
        c95: Cutoffs {
            var: -c95.0,
            es: -c95.1,
            deep: deep95,
        },
        c99: Cutoffs {
            var: -c99.0,
            es: -c99.1,
            deep: deep99,
        },
    }
}

pub fn drawdown(equity: &[f64], dates: &[String]) -> Drawdown {
    let mut peak = f64::MIN;
    let mut series = Vec::with_capacity(equity.len());
    let mut max_pct = 0.0f64;
    for &e in equity {
        peak = peak.max(e);
        let dd = if peak > 0.0 {
            (e / peak - 1.0) * 100.0
        } else {
            0.0
        };
        max_pct = max_pct.min(dd);
        series.push((dd * 100.0).round() / 100.0);
    }
    Drawdown {
        max_pct: (max_pct * 10.0).round() / 10.0,
        dates: dates.to_vec(),
        series,
    }
}

#[allow(clippy::cast_precision_loss)]
pub fn historical_var(
    equity: &[f64],
    dates: &[String],
    total: f64,
    var1d95: f64,
    var1d99: f64,
) -> HistVar {
    let n = equity.len();
    let pct = |v: f64| if total == 0.0 { 0.0 } else { v / total * 100.0 };

    // Log returns (index 0 is a zero placeholder, excluded from windows).
    let mut rets = vec![0.0f64; n];
    for i in 1..n {
        rets[i] = if equity[i - 1] > 0.0 && equity[i] > 0.0 {
            (equity[i] / equity[i - 1]).ln()
        } else {
            0.0
        };
    }

    // Rolling realized vol → shape normalized so the latest point is 1.0.
    let mut roll = vec![0.0f64; n];
    for i in 1..n {
        let start = i.saturating_sub(WINDOW - 1).max(1);
        let win = &rets[start..=i];
        let m: f64 = win.iter().sum::<f64>() / win.len() as f64;
        let var: f64 =
            win.iter().map(|r| (r - m) * (r - m)).sum::<f64>() / (win.len().max(2) - 1) as f64;
        roll[i] = var.sqrt();
    }
    if n >= 2 {
        roll[0] = roll[1];
    }
    let last = roll.last().copied().unwrap_or(0.0).max(1e-12);

    let sqrt20 = 20.0_f64.sqrt();
    let scaled = |var1d: f64| -> Vec<f64> {
        let target = pct(var1d);
        roll.iter().map(|r| r / last * target).collect()
    };
    let v1d95 = scaled(var1d95);
    let v1d99 = scaled(var1d99);
    let v20d95: Vec<f64> = v1d95.iter().map(|v| v * sqrt20).collect();
    let v20d99: Vec<f64> = v1d99.iter().map(|v| v * sqrt20).collect();

    HistVar {
        dates: dates.to_vec(),
        var1d_pct: ByConf {
            c95: v1d95,
            c99: v1d99,
        },
        var20d_pct: ByConf {
            c95: v20d95,
            c99: v20d99,
        },
    }
}
