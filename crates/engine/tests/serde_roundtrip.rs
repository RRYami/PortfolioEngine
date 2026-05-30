//! Serde round-trip tests — only compiled when the `serde` feature is enabled.

#[cfg(feature = "serde")]
mod serde_tests {
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use serde_json;

    use ptf_engine::fold;
    use ptf_engine::{
        CorporateAction, Currency, InstrumentId, LotId, LotMethod, LotSelection, LotSelectionEntry,
        LotSide, Money, PortfolioConfig, PortfolioState, Transaction, TransactionId,
        TransactionKind,
    };

    fn usd(amount: &str) -> Money {
        Money::new(Decimal::from_str_exact(amount).unwrap(), Currency::USD)
    }

    fn eur(amount: &str) -> Money {
        Money::new(Decimal::from_str_exact(amount).unwrap(), Currency::EUR)
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 6, day).unwrap()
    }

    // ── Currency ───────────────────────────────────────────────────────────

    #[test]
    fn currency_serializes_to_string() {
        let json = serde_json::to_string(&Currency::USD).unwrap();
        assert_eq!(json, "\"USD\"");
    }

    #[test]
    fn currency_deserializes_from_string() {
        let c: Currency = serde_json::from_str("\"EUR\"").unwrap();
        assert_eq!(c, Currency::EUR);
    }

    #[test]
    fn currency_rejects_invalid_code() {
        let result: Result<Currency, _> = serde_json::from_str("\"us\"");
        assert!(result.is_err());

        let result: Result<Currency, _> = serde_json::from_str("\"usd\"");
        assert!(result.is_err());
    }

    #[test]
    fn currency_roundtrips() {
        for &c in &[
            Currency::USD,
            Currency::EUR,
            Currency::GBP,
            Currency::JPY,
            Currency::CHF,
        ] {
            let json = serde_json::to_string(&c).unwrap();
            let decoded: Currency = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, c);
        }
    }

    // ── Money ──────────────────────────────────────────────────────────────

    #[test]
    fn money_roundtrips() {
        let m = usd("100.50");
        let json = serde_json::to_string(&m).unwrap();
        let decoded: Money = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, m);
    }

    #[test]
    fn money_json_format() {
        let m = usd("100.50");
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"amount\":\"100.50\""));
        assert!(json.contains("\"currency\":\"USD\""));
    }

    // ── ID types ─────────────────────────────────────────────────────────

    #[test]
    fn instrument_id_roundtrips() {
        let id = InstrumentId::new();
        let json = serde_json::to_string(&id).unwrap();
        let decoded: InstrumentId = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn transaction_id_roundtrips() {
        let id = TransactionId::new();
        let json = serde_json::to_string(&id).unwrap();
        let decoded: TransactionId = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn lot_id_roundtrips() {
        let id = LotId::new();
        let json = serde_json::to_string(&id).unwrap();
        let decoded: LotId = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, id);
    }

    // ── LotSide, LotMethod ───────────────────────────────────────────────

    #[test]
    fn lot_side_roundtrips() {
        for &side in &[LotSide::Long, LotSide::Short] {
            let json = serde_json::to_string(&side).unwrap();
            let decoded: LotSide = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, side);
        }
    }

    #[test]
    fn lot_side_snake_case() {
        let json = serde_json::to_string(&LotSide::Long).unwrap();
        assert_eq!(json, "\"long\"");
        let json = serde_json::to_string(&LotSide::Short).unwrap();
        assert_eq!(json, "\"short\"");
    }

    #[test]
    fn lot_method_roundtrips() {
        for &method in &[
            LotMethod::Fifo,
            LotMethod::Lifo,
            LotMethod::HighestCost,
            LotMethod::LowestCost,
            LotMethod::AverageCost,
        ] {
            let json = serde_json::to_string(&method).unwrap();
            let decoded: LotMethod = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, method);
        }
    }

    // ── LotSelection ───────────────────────────────────────────────────────

    #[test]
    fn lot_selection_method_roundtrips() {
        let sel = LotSelection::Method(LotMethod::Fifo);
        let json = serde_json::to_string(&sel).unwrap();
        let decoded: LotSelection = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, sel);
    }

    #[test]
    fn lot_selection_specific_roundtrips() {
        let sel = LotSelection::Specific(vec![LotSelectionEntry {
            lot_id: LotId::new(),
            quantity: Decimal::from(10),
        }]);
        let json = serde_json::to_string(&sel).unwrap();
        let decoded: LotSelection = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, sel);
    }

    // ── TransactionKind variants ───────────────────────────────────────────

    #[test]
    fn transaction_kind_buy_roundtrips() {
        let kind = TransactionKind::buy(
            InstrumentId::new(),
            Decimal::from(10),
            usd("50.00"),
            usd("1.00"),
            Some(LotSelection::Method(LotMethod::Fifo)),
        )
        .unwrap();
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: TransactionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }

    #[test]
    fn transaction_kind_sell_roundtrips() {
        let kind = TransactionKind::sell(
            InstrumentId::new(),
            Decimal::from(5),
            usd("60.00"),
            usd("0.50"),
            None,
        )
        .unwrap();
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: TransactionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }

    #[test]
    fn transaction_kind_deposit_roundtrips() {
        let kind = TransactionKind::deposit(usd("1000.00")).unwrap();
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: TransactionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }

    #[test]
    fn transaction_kind_withdrawal_roundtrips() {
        let kind = TransactionKind::withdrawal(usd("500.00")).unwrap();
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: TransactionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }

    #[test]
    fn transaction_kind_fee_roundtrips() {
        let kind = TransactionKind::fee(usd("9.99"), Some("commission".to_string())).unwrap();
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: TransactionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }

    #[test]
    fn transaction_kind_dividend_roundtrips() {
        let kind =
            TransactionKind::dividend(InstrumentId::new(), usd("25.00"), Some(date(15))).unwrap();
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: TransactionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }

    #[test]
    fn transaction_kind_corporate_action_split_roundtrips() {
        let kind = TransactionKind::CorporateAction(
            CorporateAction::split(InstrumentId::new(), Decimal::from(2)).unwrap(),
        );
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: TransactionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }

    #[test]
    fn transaction_kind_corporate_action_reverse_split_roundtrips() {
        let kind = TransactionKind::CorporateAction(
            CorporateAction::reverse_split(InstrumentId::new(), Decimal::new(1, 1)).unwrap(),
        );
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: TransactionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }

    #[test]
    fn transaction_kind_internally_tagged_format() {
        let kind = TransactionKind::deposit(usd("100.00")).unwrap();
        let json = serde_json::to_string(&kind).unwrap();
        assert!(json.contains("\"kind\":\"deposit\""));
    }

    // ── CorporateAction variants ───────────────────────────────────────────

    #[test]
    fn corporate_action_split_roundtrips() {
        let ca = CorporateAction::split(InstrumentId::new(), Decimal::from(2)).unwrap();
        let json = serde_json::to_string(&ca).unwrap();
        let decoded: CorporateAction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ca);
    }

    #[test]
    fn corporate_action_reverse_split_roundtrips() {
        let ca = CorporateAction::reverse_split(InstrumentId::new(), Decimal::new(1, 1)).unwrap();
        let json = serde_json::to_string(&ca).unwrap();
        let decoded: CorporateAction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ca);
    }

    #[test]
    fn corporate_action_spinoff_roundtrips() {
        let ca = CorporateAction::spinoff(
            InstrumentId::new(),
            InstrumentId::new(),
            Decimal::from(1),
            Decimal::new(5, 1),
        )
        .unwrap();
        let json = serde_json::to_string(&ca).unwrap();
        let decoded: CorporateAction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ca);
    }

    #[test]
    fn corporate_action_merger_roundtrips() {
        let ca = CorporateAction::merger(
            InstrumentId::new(),
            InstrumentId::new(),
            Decimal::from(1),
            Some(usd("5.00")),
        )
        .unwrap();
        let json = serde_json::to_string(&ca).unwrap();
        let decoded: CorporateAction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ca);
    }

    #[test]
    fn corporate_action_stock_dividend_roundtrips() {
        let ca = CorporateAction::stock_dividend(InstrumentId::new(), Decimal::from(1)).unwrap();
        let json = serde_json::to_string(&ca).unwrap();
        let decoded: CorporateAction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ca);
    }

    #[test]
    fn corporate_action_return_of_capital_roundtrips() {
        let ca = CorporateAction::return_of_capital(InstrumentId::new(), usd("0.50")).unwrap();
        let json = serde_json::to_string(&ca).unwrap();
        let decoded: CorporateAction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ca);
    }

    #[test]
    fn corporate_action_symbol_change_roundtrips() {
        let ca = CorporateAction::symbol_change(InstrumentId::new(), InstrumentId::new()).unwrap();
        let json = serde_json::to_string(&ca).unwrap();
        let decoded: CorporateAction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ca);
    }

    #[test]
    fn corporate_action_internally_tagged_format() {
        let ca = CorporateAction::split(InstrumentId::new(), Decimal::from(2)).unwrap();
        let json = serde_json::to_string(&ca).unwrap();
        assert!(json.contains("\"action\":\"split\""));
    }

    // ── Full Transaction ───────────────────────────────────────────────────

    #[test]
    fn transaction_roundtrips() {
        let kind = TransactionKind::buy(
            InstrumentId::new(),
            Decimal::from(10),
            usd("50.00"),
            usd("1.00"),
            None,
        )
        .unwrap();
        let tx = Transaction::new(TransactionId::new(), date(1), date(3), kind).unwrap();
        let json = serde_json::to_string(&tx).unwrap();
        let decoded: Transaction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, tx);
    }

    // ── PortfolioState ─────────────────────────────────────────────────────

    #[test]
    fn empty_portfolio_state_roundtrips() {
        let state = PortfolioState::new();
        let json = serde_json::to_string(&state).unwrap();
        let decoded: PortfolioState = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn portfolio_state_with_cash_roundtrips() {
        let txs = vec![
            Transaction::new(
                TransactionId::new(),
                date(1),
                date(1),
                TransactionKind::deposit(usd("1000.00")).unwrap(),
            )
            .unwrap(),
            Transaction::new(
                TransactionId::new(),
                date(2),
                date(2),
                TransactionKind::deposit(eur("500.00")).unwrap(),
            )
            .unwrap(),
        ];
        let state = fold(&txs, &PortfolioConfig::new(LotMethod::Fifo, Currency::USD)).unwrap();
        let json = serde_json::to_string(&state).unwrap();
        let decoded: PortfolioState = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn portfolio_state_with_positions_roundtrips() {
        let inst = InstrumentId::new();
        let txs = vec![
            Transaction::new(
                TransactionId::new(),
                date(1),
                date(1),
                TransactionKind::deposit(usd("1000.00")).unwrap(),
            )
            .unwrap(),
            Transaction::new(
                TransactionId::new(),
                date(2),
                date(2),
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("1.00"), None)
                    .unwrap(),
            )
            .unwrap(),
        ];
        let state = fold(&txs, &PortfolioConfig::new(LotMethod::Fifo, Currency::USD)).unwrap();
        let json = serde_json::to_string(&state).unwrap();
        let decoded: PortfolioState = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, state);
    }
}
