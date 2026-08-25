//! Invariants of the Black-76 kernel that must hold for every contract, not
//! just the hand-picked ones in the unit tests.

use proptest::prelude::*;
use ptf_engine::vol::{
    OptionRight, VOL_RESOLUTION, VolError, ceiling, delta, implied_vol, intrinsic, price, vega,
};

/// Ranges chosen to cover a real option chain: sub-dollar names through index
/// levels, one week to five years, 1% to 300% vol, and discount factors down
/// to five years at ~4%.
fn contract() -> impl Strategy<Value = (f64, f64, f64, f64, f64)> {
    (
        1.0f64..5_000.0,      // forward
        1.0f64..5_000.0,      // strike
        (1.0 / 365.25)..5.0,  // tte
        0.01f64..3.0,         // vol
        0.8f64..1.0,          // discount factor
    )
}

fn rights() -> impl Strategy<Value = OptionRight> {
    prop_oneof![Just(OptionRight::Call), Just(OptionRight::Put)]
}

proptest! {
    /// Parity is an identity of the formula, so it must hold to rounding.
    #[test]
    fn put_call_parity((f, k, t, v, df) in contract()) {
        let c = price(OptionRight::Call, f, k, t, v, df);
        let p = price(OptionRight::Put, f, k, t, v, df);
        let expected = df * (f - k);
        let scale = (df * (f + k)).max(1.0);
        prop_assert!((c - p - expected).abs() <= 1e-9 * scale,
            "C-P={} expected={}", c - p, expected);
    }

    /// A premium can never sit outside its no-arbitrage envelope.
    #[test]
    fn price_within_no_arbitrage_bounds((f, k, t, v, df) in contract(), r in rights()) {
        let px = price(r, f, k, t, v, df);
        let lo = intrinsic(r, f, k, df);
        let hi = ceiling(r, f, k, df);
        let scale = hi.max(1.0);
        prop_assert!(px >= lo - 1e-9 * scale, "{px} below intrinsic {lo}");
        prop_assert!(px <= hi + 1e-9 * scale, "{px} above ceiling {hi}");
    }

    /// More volatility is never worth less, for either side.
    #[test]
    fn price_is_monotone_in_vol((f, k, t, v, df) in contract(), r in rights()) {
        let lo = price(r, f, k, t, v, df);
        let hi = price(r, f, k, t, v * 1.10, df);
        prop_assert!(hi >= lo - 1e-9 * hi.abs().max(1.0), "{lo} -> {hi}");
    }

    /// Vega is non-negative and agrees with a central difference wherever the
    /// contract carries enough vol sensitivity to measure.
    #[test]
    fn vega_non_negative_and_consistent((f, k, t, v, df) in contract()) {
        let g = vega(f, k, t, v, df);
        prop_assert!(g >= 0.0, "negative vega {g}");

        // The step is the delicate part, not the formula. A central difference
        // subtracts two nearly equal premiums, so shrinking h past the f64
        // round-off floor makes it *less* accurate: at h = 1e-6*v on a deep-ITM
        // contract it is already wrong by 2e-4, while the closed form is exact
        // to all 17 digits (checked against an independent implementation).
        // 1e-5 sits near the minimum of truncation against cancellation.
        let h = 1e-5 * v;
        let fd = (price(OptionRight::Call, f, k, t, v + h, df)
                - price(OptionRight::Call, f, k, t, v - h, df)) / (2.0 * h);

        // Only compare where the difference is resolvable at all: the change in
        // premium across 2h has to stand clear of the f64 resolution of the
        // premium itself, or the difference is measuring noise.
        let px = price(OptionRight::Call, f, k, t, v, df);
        let signal = 2.0 * h * g;
        let noise = f64::EPSILON * px.abs().max(1.0);
        if signal > 1e5 * noise {
            prop_assert!((g - fd).abs() <= 1e-4 * g, "vega {g} vs fd {fd}");
        }
    }

    /// Forward delta is a discounted probability, so it is bounded by `df`.
    #[test]
    fn delta_is_bounded((f, k, t, v, df) in contract(), r in rights()) {
        let d = delta(r, f, k, t, v, df);
        match r {
            OptionRight::Call => prop_assert!((0.0..=df).contains(&d), "call delta {d}"),
            OptionRight::Put => prop_assert!((-df..=0.0).contains(&d), "put delta {d}"),
        }
    }

    /// The central contract: pricing then inverting returns the input vol to
    /// within the advertised resolution, or refuses with a reason. It must
    /// never return a confident answer that is wrong.
    #[test]
    fn inversion_round_trips_or_refuses((f, k, t, v, df) in contract(), r in rights()) {
        let px = price(r, f, k, t, v, df);
        match implied_vol(r, px, f, k, t, df) {
            Ok(got) => prop_assert!((got - v).abs() <= VOL_RESOLUTION,
                "recovered {got} from {v} (k/f={:.3}, t={t})", k / f),
            Err(VolError::BelowIntrinsic { .. }
            | VolError::AboveCeiling { .. }
            | VolError::Unstable { .. }) => {}
            Err(e) => prop_assert!(false, "unexpected failure: {e}"),
        }
    }
}
