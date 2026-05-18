use chrono::NaiveDate;
use proptest::prelude::*;
use ptf_engine::{
    Currency, FxRateProvider, InstrumentId, Money, PortfolioConfig, PortfolioState,
    StaticFxRateProvider, StaticPriceProvider, Transaction, TransactionKind,
    TriangulatingFxProvider, fold,
};
use rust_decimal::Decimal;

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2024, 6, 15).unwrap()
}

fn all_currencies() -> Vec<Currency> {
    vec![
        Currency::USD,
        Currency::EUR,
        Currency::GBP,
        Currency::JPY,
        Currency::CHF,
    ]
}

fn gen_currency() -> impl Strategy<Value = Currency> {
    prop::sample::select(all_currencies())
}

fn gen_rate() -> impl Strategy<Value = Decimal> {
    (1u32..1000u32).prop_map(|n| Decimal::new(n as i64, 2))
}

fn gen_amount() -> impl Strategy<Value = Decimal> {
    (100u32..1000000u32).prop_map(|n| Decimal::new(n as i64, 2))
}

fn epsilon() -> Decimal {
    Decimal::new(1, 10)
}

proptest! {
    // -----------------------------------------------------------------
    // 1. Triangulation symmetry
    //    If EUR → USD = r1 and USD → GBP = r2, then rate(EUR, GBP)
    //    via triangulation equals r1 * r2 exactly. Both legs are direct
    //    so no inversion rounding is involved.
    // -----------------------------------------------------------------
    #[test]
    fn triangulation_symmetry(
        eur_usd in gen_rate(),
        usd_gbp in gen_rate(),
    ) {
        let inner = StaticFxRateProvider::new()
            .with_rate(Currency::EUR, Currency::USD, date(), eur_usd)
            .with_rate(Currency::USD, Currency::GBP, date(), usd_gbp);

        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        let triangulated = tri.rate(Currency::EUR, Currency::GBP, date()).unwrap();
        let expected = eur_usd * usd_gbp;

        prop_assert_eq!(
            triangulated, expected,
            "triangulation mismatch: r1*r2={}, got={}",
            expected, triangulated
        );
    }

    // -----------------------------------------------------------------
    // 2. Round-trip invariance
    //    For any rate r and amount a, convert(a, X, Y) * rate(Y, X)
    //    is within epsilon of a. Uses TriangulatingFxProvider so that
    //    inversion happens via the provider's own logic.
    // -----------------------------------------------------------------
    #[test]
    fn round_trip_invariance(
        from in gen_currency(),
        to in gen_currency(),
        rate in gen_rate(),
        amount in gen_amount(),
    ) {
        prop_assume!(from != to);

        let inner = StaticFxRateProvider::new()
            .with_rate(from, to, date(), rate);
        let tri = TriangulatingFxProvider::new(inner, Currency::USD);

        let forward = tri.rate(from, to, date()).unwrap();
        let backward = tri.rate(to, from, date()).unwrap();

        let converted = amount * forward;
        let round_trip = converted * backward;

        let diff = (round_trip - amount).abs();
        prop_assert!(
            diff <= epsilon(),
            "round-trip mismatch: original={}, round_trip={}, diff={}",
            amount, round_trip, diff
        );
    }

    // -----------------------------------------------------------------
    // 3. Single-currency identity
    //    For a portfolio with only USD holdings, total_value in USD
    //    equals cash_balance(USD) + sum(market values in USD) exactly.
    //    The FX layer is a no-op for same-currency portfolios.
    // -----------------------------------------------------------------
    #[test]
    fn single_currency_identity(
        deposit_cents in 10000u32..100000u32,
        qty in 1u32..100u32,
        price_cents in 100u32..10000u32,
    ) {
        let currency = Currency::USD;
        let inst = InstrumentId::new();
        let qty = Decimal::from(qty);
        let price = Decimal::new(price_cents as i64, 2);
        let deposit = Decimal::new(deposit_cents as i64, 2);
        let fee = Money::new(Decimal::ZERO, currency);

        let txs = vec![
            Transaction::new(
                ptf_engine::TransactionId::new(),
                date(),
                date(),
                TransactionKind::deposit(Money::new(deposit, currency)).unwrap(),
            ).unwrap(),
            Transaction::new(
                ptf_engine::TransactionId::new(),
                date(),
                date(),
                TransactionKind::buy(inst, qty, Money::new(price, currency), fee, None).unwrap(),
            ).unwrap(),
        ];

        let cfg = PortfolioConfig::new(ptf_engine::LotMethod::Fifo, currency);
        let state = fold(&txs, &cfg).unwrap();

        let fx = StaticFxRateProvider::new();
        let prices = StaticPriceProvider::new().with_price(
            inst,
            date(),
            Money::new(price, currency),
        );

        let total = state.total_value(&fx, &prices, currency, date()).unwrap();
        let expected = state.cash_balance(currency) + qty * price;

        prop_assert_eq!(
            total.amount, expected,
            "single-currency total mismatch: expected={}, got={}",
            expected, total.amount
        );
        prop_assert_eq!(total.currency, currency);
    }

    // -----------------------------------------------------------------
    // 4. Conservation under currency conversion
    //    total_value(_, _, A, _) * rate(A, B) is within epsilon of
    //    total_value(_, _, B, _). This verifies that "total value" is
    //    a well-defined concept independent of base currency choice,
    //    modulo rounding.
    // -----------------------------------------------------------------
    #[test]
    fn conservation_under_currency_conversion(
        deposit_usd_cents in 10000u32..100000u32,
        deposit_eur_cents in 10000u32..100000u32,
        qty_cents in 10u32..100u32,
        price_usd_cents in 100u32..10000u32,
        price_eur_cents in 100u32..10000u32,
        eur_to_usd_cents in 50u32..200u32,
    ) {
        let usd_inst = InstrumentId::new();
        let eur_inst = InstrumentId::new();
        let deposit_usd = Decimal::new(deposit_usd_cents as i64, 2);
        let deposit_eur = Decimal::new(deposit_eur_cents as i64, 2);
        let qty = Decimal::from(qty_cents);
        let price_usd = Decimal::new(price_usd_cents as i64, 2);
        let price_eur = Decimal::new(price_eur_cents as i64, 2);
        let eur_to_usd = Decimal::new(eur_to_usd_cents as i64, 2);

        let txs = vec![
            Transaction::new(
                ptf_engine::TransactionId::new(),
                date(),
                date(),
                TransactionKind::deposit(Money::new(deposit_usd, Currency::USD)).unwrap(),
            ).unwrap(),
            Transaction::new(
                ptf_engine::TransactionId::new(),
                date(),
                date(),
                TransactionKind::deposit(Money::new(deposit_eur, Currency::EUR)).unwrap(),
            ).unwrap(),
            Transaction::new(
                ptf_engine::TransactionId::new(),
                date(),
                date(),
                TransactionKind::buy(
                    usd_inst,
                    qty,
                    Money::new(price_usd, Currency::USD),
                    Money::new(Decimal::ZERO, Currency::USD),
                    None,
                ).unwrap(),
            ).unwrap(),
            Transaction::new(
                ptf_engine::TransactionId::new(),
                date(),
                date(),
                TransactionKind::buy(
                    eur_inst,
                    qty,
                    Money::new(price_eur, Currency::EUR),
                    Money::new(Decimal::ZERO, Currency::EUR),
                    None,
                ).unwrap(),
            ).unwrap(),
        ];

        let cfg = PortfolioConfig::new(ptf_engine::LotMethod::Fifo, Currency::USD);
        let state = fold(&txs, &cfg).unwrap();

        // We need rates in both directions. Provide EUR→USD and let
        // the triangulator compute USD→EUR by inversion.
        let fx_static = StaticFxRateProvider::new()
            .with_rate(Currency::EUR, Currency::USD, date(), eur_to_usd);
        let fx = TriangulatingFxProvider::new(fx_static, Currency::USD);

        let prices = StaticPriceProvider::new()
            .with_price(usd_inst, date(), Money::new(price_usd, Currency::USD))
            .with_price(eur_inst, date(), Money::new(price_eur, Currency::EUR));

        let total_usd = state.total_value(&fx, &prices, Currency::USD, date()).unwrap();
        let total_eur = state.total_value(&fx, &prices, Currency::EUR, date()).unwrap();

        // Convert total_eur to USD using the FX provider's rate
        let eur_to_usd_rate = fx.rate(Currency::EUR, Currency::USD, date()).unwrap();
        let total_eur_in_usd = total_eur.amount * eur_to_usd_rate;

        // Use a generous epsilon because we're composing inversions
        let diff = (total_usd.amount - total_eur_in_usd).abs();
        let generous_epsilon = Decimal::new(1, 8);
        prop_assert!(
            diff <= generous_epsilon,
            "conservation under currency conversion: total_usd={}, total_eur_in_usd={}, diff={}",
            total_usd.amount, total_eur_in_usd, diff
        );
    }

    // -----------------------------------------------------------------
    // 5. Empty portfolio
    //    total_value on an empty PortfolioState returns zero in any
    //    base currency, even when FX rates are available.
    // -----------------------------------------------------------------
    #[test]
    fn empty_portfolio(base in gen_currency()) {
        let state = PortfolioState::new();
        let fx = TriangulatingFxProvider::new(
            StaticFxRateProvider::new()
                .with_rate(Currency::EUR, Currency::USD, date(), Decimal::new(110, 2)),
            Currency::USD,
        );

        let total = state.total_value(&fx, &StaticPriceProvider::new(), base, date()).unwrap();
        prop_assert_eq!(total.amount, Decimal::ZERO);
        prop_assert_eq!(total.currency, base);
    }
}
