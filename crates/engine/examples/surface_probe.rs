//! Throwaway: run the real kernel over one session's quotes and dump the IV
//! cloud, so the surface can be eyeballed before any of this is committed to a
//! crate. Reads a CSV so it needs no parquet plumbing; delete once the driver
//! exists.
//!
//! usage: `cargo run -p ptf-engine --example surface_probe -- <csv> [--cloud]`
//!
//! Default output is one row per (session, expiry) — the curve and forward
//! diagnostics. `--cloud` emits the full IV point cloud instead.

use std::collections::BTreeMap;
use std::env;
use std::fs;

use ptf_engine::forward::{
    DEFAULT_BAND, DiscountCurve, ParityPair, forward_at, implied_forward,
};
use ptf_engine::vol::{OptionRight, VolError, implied_vol, vega};

/// Expected CSV layout:
/// `quote_date,tte,opt_right,strike,mid,rel_spread,bid_size,ask_size,stale,expiry`
const COLUMNS: usize = 10;

#[derive(Debug, Clone, Copy)]
struct Quote {
    right: OptionRight,
    strike: f64,
    mid: f64,
    rel_spread: f64,
    size: f64,
    stale: bool,
}

fn main() {
    let path = env::args().nth(1).expect("usage: surface_probe <csv> [--cloud]");
    let cloud = env::args().any(|a| a == "--cloud");
    let text = fs::read_to_string(&path).expect("read csv");

    // (session, expiry) -> (tte, quotes)
    let mut sessions: BTreeMap<String, BTreeMap<String, (f64, Vec<Quote>)>> = BTreeMap::new();
    // One fixed layout, checked against the header rather than sniffed:
    // quote_date,tte,opt_right,strike,mid,rel_spread,bid_size,ask_size,stale,expiry
    let header = text.lines().next().unwrap_or_default();
    assert!(
        header.starts_with("quote_date,tte,opt_right,strike,mid,rel_spread"),
        "unexpected csv layout: {header}"
    );
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < COLUMNS {
            continue;
        }
        let q = Quote {
            right: if f[2] == "C" { OptionRight::Call } else { OptionRight::Put },
            strike: f[3].parse().unwrap(),
            mid: f[4].parse().unwrap(),
            rel_spread: f[5].parse().unwrap_or(f64::NAN),
            size: f[6].parse::<f64>().unwrap_or(0.0).min(f[7].parse::<f64>().unwrap_or(0.0)),
            stale: f[8] == "1",
        };
        let e = sessions
            .entry(f[0].to_string())
            .or_default()
            .entry(f[9].to_string())
            .or_insert_with(|| (f[1].parse().unwrap(), vec![]));
        e.1.push(q);
    }

    if cloud {
        println!("expiry,tte,right,strike,k,iv,vega,rel_spread,size,stale,forward,df");
    } else {
        println!(
            "session,expiry,tte,forward,df,rate,pairs,rmse,inverted,unstable,\
             below_intrinsic,curve_rate,curve_rms,curve_n"
        );
    }
    for (session, slices) in &sessions {
        run_session(session, slices, cloud);
    }
}

#[allow(clippy::too_many_lines)]
fn run_session(session: &str, slices: &BTreeMap<String, (f64, Vec<Quote>)>, cloud: bool) {

    // Pass 1: per-expiry regressions, purely to seed the curve.
    let mut seeds = Vec::new();
    for (tte, qs) in slices.values() {
        if let Ok(fit) = implied_forward(&pairs_of(qs), DEFAULT_BAND) {
            seeds.push((*tte, fit.discount));
        }
    }
    let Ok(curve) = DiscountCurve::fit(&seeds) else {
        eprintln!("# {session}: NO CURVE ({} seeds)", seeds.len());
        return;
    };
    let bad_seeds = seeds.iter().filter(|(_, d)| *d > 1.0).count();

    for (expiry, (tte, qs)) in slices {
        let df = curve.df(*tte);
        let Ok(fwd) = forward_at(&pairs_of(qs), df, DEFAULT_BAND) else {
            eprintln!("# {session} {expiry}: NO FORWARD");
            continue;
        };
        let f = fwd.forward;
        let (mut kept, mut unstable, mut below) = (0usize, 0usize, 0usize);
        for q in qs {
            // OTM only, per the plan: ITM premiums are nearly all intrinsic.
            let otm = match q.right {
                OptionRight::Call => q.strike >= f,
                OptionRight::Put => q.strike < f,
            };
            if !otm {
                continue;
            }
            match implied_vol(q.right, q.mid, f, q.strike, *tte, df) {
                Ok(iv) => {
                    kept += 1;
                    if cloud {
                        let k = (q.strike / f).ln();
                        let vg = vega(f, q.strike, *tte, iv, df);
                        println!(
                            "{expiry},{tte:.6},{},{},{k:.6},{iv:.6},{vg:.6},{:.6},{},{},{f:.4},{df:.6}",
                            q.right, q.strike, q.rel_spread, q.size, u8::from(q.stale)
                        );
                    }
                }
                Err(VolError::Unstable { .. }) => unstable += 1,
                Err(VolError::BelowIntrinsic { .. }) => below += 1,
                Err(_) => {}
            }
        }
        if !cloud {
            let rate = -df.ln() / tte;
            println!(
                "{session},{expiry},{tte:.6},{f:.4},{df:.6},{rate:.6},{},{:.4},{kept},{unstable},\
                 {below},{:.6},{:.6},{}",
                fwd.pairs_used,
                fwd.rmse,
                curve.rate,
                curve.rms_residual,
                curve.expiries_used
            );
        }
    }
    if bad_seeds > 0 {
        eprintln!("# {session}: {bad_seeds} of {} raw per-expiry DF were > 1", seeds.len());
    }
}

fn pairs_of(qs: &[Quote]) -> Vec<ParityPair> {
    let mut calls: BTreeMap<u64, &Quote> = BTreeMap::new();
    let mut puts: BTreeMap<u64, &Quote> = BTreeMap::new();
    for q in qs {
        // Strikes are quoted to a tenth of a dollar; a milli-dollar integer key
        // pairs calls with puts without float-equality games.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let key = (q.strike * 1000.0).round() as u64;
        match q.right {
            OptionRight::Call => calls.insert(key, q),
            OptionRight::Put => puts.insert(key, q),
        };
    }
    calls
        .iter()
        .filter_map(|(k, c)| {
            puts.get(k).map(|p| {
                // Weight by the tighter of the two spreads: a pair is only as
                // trustworthy as its worse leg.
                let w = 1.0 / (1.0 + c.rel_spread.max(p.rel_spread).max(0.0));
                ParityPair::weighted(c.strike, c.mid, p.mid, w)
            })
        })
        .collect()
}
