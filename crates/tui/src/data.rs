use chrono::NaiveDate;
use ptf_engine::{
    fold, CorporateAction, Currency, Instrument, InstrumentId, InstrumentKind, LotMethod, Money,
    Portfolio, PortfolioConfig, PortfolioId, PortfolioState, StaticFxRateProvider,
    StaticPriceProvider, Transaction, TransactionId, TransactionKind,
};
use rust_decimal::Decimal;

/// Pre-seeded demo portfolio with a short-flip story.
pub struct SeedData {
    pub portfolio: Portfolio,
    pub config: PortfolioConfig,
    pub instruments: Vec<Instrument>,
    pub transactions: Vec<Transaction>,
    pub state: PortfolioState,
    pub prices: StaticPriceProvider,
    pub fx: StaticFxRateProvider,
    pub as_of: NaiveDate,
}

fn d(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn tx(date: NaiveDate, kind: TransactionKind) -> Transaction {
    Transaction::new(TransactionId::new(), date, date, kind).unwrap()
}

fn usd(amount: &str) -> Money {
    Money::new(Decimal::from_str_exact(amount).unwrap(), Currency::USD)
}

fn eur(amount: &str) -> Money {
    Money::new(Decimal::from_str_exact(amount).unwrap(), Currency::EUR)
}

fn jpy(amount: &str) -> Money {
    Money::new(Decimal::from_str_exact(amount).unwrap(), Currency::JPY)
}

#[allow(clippy::too_many_lines)]
pub fn seed_data() -> SeedData {
    let portfolio = Portfolio::new(
        PortfolioId::new(),
        "Global Multi-Asset",
        Currency::EUR,
        LotMethod::Fifo,
        d(2024, 1, 2),
    );

    let config = PortfolioConfig::new(LotMethod::Fifo, Currency::EUR);

    let aapl_id = InstrumentId::new();
    let nvda_id = InstrumentId::new();
    let sap_id = InstrumentId::new();
    let sony_id = InstrumentId::new();

    let instruments = vec![
        Instrument {
            id: aapl_id,
            symbol: "AAPL".into(),
            name: "Apple Inc.".into(),
            currency: Currency::USD,
            kind: InstrumentKind::Equity {},
        },
        Instrument {
            id: nvda_id,
            symbol: "NVDA".into(),
            name: "NVIDIA Corp.".into(),
            currency: Currency::USD,
            kind: InstrumentKind::Equity {},
        },
        Instrument {
            id: sap_id,
            symbol: "SAP".into(),
            name: "SAP SE".into(),
            currency: Currency::EUR,
            kind: InstrumentKind::Equity {},
        },
        Instrument {
            id: sony_id,
            symbol: "SONY".into(),
            name: "Sony Group Corp.".into(),
            currency: Currency::JPY,
            kind: InstrumentKind::Equity {},
        },
    ];

    let transactions = vec![
        // 1. Fund the account
        tx(d(2024, 1, 2), TransactionKind::deposit(usd("100000")).unwrap()),
        tx(d(2024, 1, 2), TransactionKind::deposit(eur("50000")).unwrap()),
        // 2. Build long positions
        tx(
            d(2024, 1, 3),
            TransactionKind::buy(aapl_id, Decimal::from(100), usd("185.00"), usd("9.99"), None)
                .unwrap(),
        ),
        tx(
            d(2024, 1, 5),
            TransactionKind::buy(nvda_id, Decimal::from(50), usd("495.00"), usd("9.99"), None)
                .unwrap(),
        ),
        tx(
            d(2024, 1, 8),
            TransactionKind::buy(sap_id, Decimal::from(80), eur("125.00"), eur("5.00"), None)
                .unwrap(),
        ),
        // 3. SHORT FLIP — sell more AAPL than we own
        tx(
            d(2024, 1, 10),
            TransactionKind::sell(aapl_id, Decimal::from(130), usd("190.00"), usd("9.99"), None)
                .unwrap(),
        ),
        // 4. Cover the short + reopen long
        tx(
            d(2024, 1, 12),
            TransactionKind::buy(aapl_id, Decimal::from(50), usd("188.00"), usd("9.99"), None)
                .unwrap(),
        ),
        // 5. Add another AAPL lot (now two long lots: 20 + 30)
        tx(
            d(2024, 1, 15),
            TransactionKind::buy(aapl_id, Decimal::from(30), usd("188.00"), usd("5.00"), None)
                .unwrap(),
        ),
        // 6. Dividend on the remaining long shares
        tx(
            d(2024, 1, 15),
            TransactionKind::dividend(aapl_id, usd("5.00"), None).unwrap(),
        ),
        // 7. Corporate action: 2-for-1 split on NVDA
        tx(
            d(2024, 1, 18),
            TransactionKind::CorporateAction(
                CorporateAction::split(nvda_id, Decimal::from(2)).unwrap(),
            ),
        ),
        // 8. Second NVDA lot (now two long lots after split)
        tx(
            d(2024, 1, 20),
            TransactionKind::buy(nvda_id, Decimal::from(25), usd("260.00"), usd("5.00"), None)
                .unwrap(),
        ),
        // 9. Multi-currency position (JPY)
        tx(
            d(2024, 1, 20),
            TransactionKind::buy(sony_id, Decimal::from(10), jpy("12000"), jpy("500"), None)
                .unwrap(),
        ),
        // 10. Fee + withdrawal
        tx(
            d(2024, 1, 22),
            TransactionKind::fee(usd("25.00"), Some("Platform fee".into())).unwrap(),
        ),
        tx(
            d(2024, 1, 25),
            TransactionKind::withdrawal(usd("5000.00")).unwrap(),
        ),
    ];

    let state = fold(&transactions, &config).unwrap();

    let as_of = d(2024, 1, 25);
    let mut prices = StaticPriceProvider::new();
    prices.insert(aapl_id, as_of, usd("190.00"));
    prices.insert(nvda_id, as_of, usd("260.00"));
    prices.insert(sap_id, as_of, eur("130.00"));
    prices.insert(sony_id, as_of, jpy("12500"));

    let mut fx = StaticFxRateProvider::new();
    fx.insert(
        Currency::EUR,
        Currency::USD,
        as_of,
        Decimal::from_str_exact("1.0850").unwrap(),
    );
    fx.insert(
        Currency::USD,
        Currency::EUR,
        as_of,
        Decimal::from_str_exact("0.9217").unwrap(),
    );
    fx.insert(
        Currency::EUR,
        Currency::JPY,
        as_of,
        Decimal::from_str_exact("160.00").unwrap(),
    );
    fx.insert(
        Currency::JPY,
        Currency::EUR,
        as_of,
        Decimal::from_str_exact("0.00625").unwrap(),
    );
    // Direct USD ↔ JPY rates (triangulation via EUR also works, but explicit
    // rates make the demo faster and the FX popup cleaner).
    fx.insert(
        Currency::USD,
        Currency::JPY,
        as_of,
        Decimal::from_str_exact("147.47").unwrap(),
    );
    fx.insert(
        Currency::JPY,
        Currency::USD,
        as_of,
        Decimal::from_str_exact("0.006781").unwrap(),
    );

    SeedData {
        portfolio,
        config,
        instruments,
        transactions,
        state,
        prices,
        fx,
        as_of,
    }
}

pub fn seed_data_us_growth() -> SeedData {
    let portfolio = Portfolio::new(
        PortfolioId::new(),
        "US Growth",
        Currency::USD,
        LotMethod::Fifo,
        d(2024, 1, 15),
    );

    let config = PortfolioConfig::new(LotMethod::Fifo, Currency::USD);

    let tsla_id = InstrumentId::new();
    let meta_id = InstrumentId::new();

    let instruments = vec![
        Instrument {
            id: tsla_id,
            symbol: "TSLA".into(),
            name: "Tesla Inc.".into(),
            currency: Currency::USD,
            kind: InstrumentKind::Equity {},
        },
        Instrument {
            id: meta_id,
            symbol: "META".into(),
            name: "Meta Platforms Inc.".into(),
            currency: Currency::USD,
            kind: InstrumentKind::Equity {},
        },
    ];

    let transactions = vec![
        tx(d(2024, 1, 15), TransactionKind::deposit(usd("50000")).unwrap()),
        tx(
            d(2024, 1, 16),
            TransactionKind::buy(tsla_id, Decimal::from(20), usd("250.00"), usd("5.00"), None)
                .unwrap(),
        ),
        tx(
            d(2024, 1, 17),
            TransactionKind::buy(meta_id, Decimal::from(10), usd("300.00"), usd("5.00"), None)
                .unwrap(),
        ),
        tx(
            d(2024, 1, 18),
            TransactionKind::sell(tsla_id, Decimal::from(5), usd("260.00"), usd("5.00"), None)
                .unwrap(),
        ),
    ];

    let state = fold(&transactions, &config).unwrap();

    let as_of = d(2024, 1, 25);
    let mut prices = StaticPriceProvider::new();
    prices.insert(tsla_id, as_of, usd("255.00"));
    prices.insert(meta_id, as_of, usd("310.00"));

    let fx = StaticFxRateProvider::new();

    SeedData {
        portfolio,
        config,
        instruments,
        transactions,
        state,
        prices,
        fx,
        as_of,
    }
}
