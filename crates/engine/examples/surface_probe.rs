//! Throwaway: run the real kernel over one session's quotes and dump the IV
//! cloud, so the surface can be eyeballed before any of this is committed to a
//! crate. Reads a CSV so it needs no parquet plumbing; delete once the driver
//! exists.
//!
//! usage: `cargo run -p ptf-engine --example surface_probe -- <session.csv>`

use std::collections::BTreeMap;
use std::env;
use std::fs;

use ptf_engine::forward::{
    DEFAULT_BAND, DiscountCurve, ParityPair, forward_at, implied_forward,
};
use ptf_engine::vol::{OptionRight, VolError, implied_vol, vega};

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
    let path = env::args().nth(1).expect("usage: surface_probe <csv>");
    let text = fs::read_to_string(&path).expect("read csv");

    let mut slices: BTreeMap<String, (f64, Vec<Quote>)> = BTreeMap::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 11 {
            continue;
        }
        let q = Quote {
            right: if f[1] == "C" { OptionRight::Call } else { OptionRight::Put },
            strike: f[2].parse().unwrap(),
            mid: f[3].parse().unwrap(),
            rel_spread: f[6].parse().unwrap_or(f64::NAN),
            size: f[7].parse::<f64>().unwrap_or(0.0).min(f[8].parse::<f64>().unwrap_or(0.0)),
            stale: f[9] == "1",
        };
        let e = slices.entry(f[10].to_string()).or_insert_with(|| (f[0].parse().unwrap(), vec![]));
        e.1.push(q);
    }

    // Pass 1: per-expiry regressions, purely to seed the curve.
    let mut seeds = Vec::new();
    for (tte, qs) in slices.values() {
        if let Ok(fit) = implied_forward(&pairs_of(qs), DEFAULT_BAND) {
            seeds.push((*tte, fit.discount));
        }
    }
    let curve = DiscountCurve::fit(&seeds).expect("curve");
    eprintln!(
        "# curve: rate={:.4}% from {} expiries (rms {:.5}); {} of {} raw DF were > 1",
        curve.rate * 100.0,
        curve.expiries_used,
        curve.rms_residual,
        seeds.iter().filter(|(_, d)| *d > 1.0).count(),
        seeds.len()
    );

    println!("expiry,tte,right,strike,k,iv,vega,rel_spread,size,stale,forward,df");
    let mut kept = 0usize;
    let mut rejected: BTreeMap<&str, usize> = BTreeMap::new();
    for (expiry, (tte, qs)) in &slices {
        let df = curve.df(*tte);
        let Ok(fwd) = forward_at(&pairs_of(qs), df, DEFAULT_BAND) else {
            *rejected.entry("no forward").or_default() += qs.len();
            continue;
        };
        let f = fwd.forward;
        eprintln!(
            "# {expiry} tte={tte:.4} F={f:.2} df={df:.6} pairs={} rmse={:.3}",
            fwd.pairs_used, fwd.rmse
        );
        for q in qs {
            // OTM only, per the plan: ITM premiums are nearly all intrinsic.
            let otm = match q.right {
                OptionRight::Call => q.strike >= f,
                OptionRight::Put => q.strike < f,
            };
            if !otm {
                *rejected.entry("itm").or_default() += 1;
                continue;
            }
            match implied_vol(q.right, q.mid, f, q.strike, *tte, df) {
                Ok(iv) => {
                    kept += 1;
                    let k = (q.strike / f).ln();
                    let vg = vega(f, q.strike, *tte, iv, df);
                    println!(
                        "{expiry},{tte:.6},{},{},{k:.6},{iv:.6},{vg:.6},{:.6},{},{},{f:.4},{df:.6}",
                        q.right, q.strike, q.rel_spread, q.size, u8::from(q.stale)
                    );
                }
                Err(VolError::Unstable { .. }) => *rejected.entry("unstable").or_default() += 1,
                Err(VolError::BelowIntrinsic { .. }) => {
                    *rejected.entry("below intrinsic").or_default() += 1;
                }
                Err(VolError::AboveCeiling { .. }) => {
                    *rejected.entry("above ceiling").or_default() += 1;
                }
                Err(_) => *rejected.entry("other").or_default() += 1,
            }
        }
    }
    eprintln!("# kept {kept}; rejected {rejected:?}");
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
