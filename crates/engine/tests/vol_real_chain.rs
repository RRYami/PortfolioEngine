//! The kernel against a real chain, not synthetic inputs.
//!
//! SOXX 2026-08-20, expiry 2026-09-11, OTM quotes within |k| < 0.25. The
//! forward and discount factor are the put-call parity regression over the 49
//! paired strikes of that expiry (slope `-DF`, intercept `DF*F`) — no external
//! rate or dividend input, which is the whole point of pricing on the forward.
//!
//! Synthetic round-trips prove the solver inverts its own formula. This proves
//! the formula produces a believable market smile from quotes we actually paid
//! for, which is a different and more useful claim.

use ptf_engine::vol::{OptionRight, VolError, implied_vol};

const FORWARD: f64 = 522.3806;
const DF: f64 = 0.997_877;
const TTE: f64 = 0.060_233;

const CHAIN: &[(OptionRight, f64, f64)] = &[
    (OptionRight::Put, 410.0, 1.2500),
    (OptionRight::Put, 425.0, 1.7250),
    (OptionRight::Put, 445.0, 2.8000),
    (OptionRight::Put, 457.5, 3.9500),
    (OptionRight::Put, 465.0, 4.9000),
    (OptionRight::Put, 475.0, 6.3000),
    (OptionRight::Put, 482.5, 7.7000),
    (OptionRight::Put, 490.0, 9.4000),
    (OptionRight::Put, 497.5, 11.5500),
    (OptionRight::Put, 507.5, 14.9500),
    (OptionRight::Put, 515.0, 17.8000),
    (OptionRight::Call, 522.5, 21.2000),
    (OptionRight::Call, 530.0, 17.4500),
    (OptionRight::Call, 540.0, 13.2500),
    (OptionRight::Call, 547.5, 10.6000),
    (OptionRight::Call, 555.0, 8.9500),
    (OptionRight::Call, 562.5, 6.9000),
    (OptionRight::Call, 572.5, 5.0000),
    (OptionRight::Call, 585.0, 3.3750),
    (OptionRight::Call, 600.0, 1.8750),
    (OptionRight::Call, 615.0, 1.3000),
    (OptionRight::Call, 635.0, 0.7500),
];

#[test]
fn real_chain_produces_a_believable_smile() {
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for &(right, strike, mid) in CHAIN {
        match implied_vol(right, mid, FORWARD, strike, TTE, DF) {
            Ok(v) => {
                let k = (strike / FORWARD).ln();
                println!("{right} K={strike:7.1} k={k:+.4} mid={mid:8.4} iv={:.4}", v);
                pts.push((k, v));
            }
            // Far wings may not constrain a vol; that is a legitimate outcome.
            Err(VolError::Unstable { .. }) => println!("{right} K={strike:7.1} unstable"),
            Err(e) => panic!("{right} K={strike} mid={mid}: {e}"),
        }
    }
    assert!(pts.len() >= 15, "only {} strikes inverted", pts.len());

    // Every vol must be in a range a semiconductor ETF could plausibly print.
    for &(k, v) in &pts {
        assert!((0.05..=2.0).contains(&v), "iv {v} at k={k} is not plausible");
    }

    // Equity index skew: downside puts carry more vol than upside calls.
    let dn: Vec<f64> = pts.iter().filter(|(k, _)| *k < -0.08).map(|(_, v)| *v).collect();
    let up: Vec<f64> = pts.iter().filter(|(k, _)| *k > 0.08).map(|(_, v)| *v).collect();
    assert!(!dn.is_empty() && !up.is_empty(), "need both wings");
    let mean = |xs: &[f64]| xs.iter().sum::<f64>() / xs.len() as f64;
    assert!(mean(&dn) > mean(&up),
        "expected negative skew, got puts {:.4} vs calls {:.4}", mean(&dn), mean(&up));

    // A smile is smooth: no neighbouring strikes should disagree wildly.
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for w in pts.windows(2) {
        let jump = (w[1].1 - w[0].1).abs() / (w[1].0 - w[0].0).abs().max(1e-9);
        assert!(jump < 6.0, "iv jumps {jump:.1} per unit k between {:?} and {:?}", w[0], w[1]);
    }
}
