use chrono::NaiveDate;
use proptest::collection::vec as pvec;
use proptest::prelude::*;
use ptf_engine::{
    CorporateAction, Currency, InstrumentId, LotMethod, Money, PortfolioConfig, PortfolioState,
    Transaction, TransactionKind,
};
use rust_decimal::Decimal;
use std::collections::HashMap;

// ===================================================================
// Helpers
// ===================================================================

fn make_tx(kind: TransactionKind, date: NaiveDate) -> Transaction {
    Transaction::new(ptf_engine::TransactionId::new(), date, date, kind).unwrap()
}

/// Compare two portfolio states, ignoring LotIds (which are randomly
/// generated inside `fold` and therefore differ across calls).
fn states_equivalent(a: &PortfolioState, b: &PortfolioState) -> bool {
    cash_equal(a, b) && pnl_equal(a, b) && positions_equivalent(a, b)
}

fn cash_equal(a: &PortfolioState, b: &PortfolioState) -> bool {
    let all: Vec<_> = a.currencies().chain(b.currencies()).collect();
    all.iter().all(|c| a.cash_balance(*c) == b.cash_balance(*c))
}

fn pnl_equal(a: &PortfolioState, b: &PortfolioState) -> bool {
    let all: Vec<_> = a.currencies().chain(b.currencies()).collect();
    all.iter()
        .all(|c| a.realized_pnl_in(*c) == b.realized_pnl_in(*c))
}

fn positions_equivalent(a: &PortfolioState, b: &PortfolioState) -> bool {
    a.positions().len() == b.positions().len()
        && a.positions()
            .iter()
            .all(|(inst, pos_a)| match b.positions().get(inst) {
                Some(pos_b) => pos_equivalent(pos_a, pos_b),
                None => false,
            })
}

fn pos_equivalent(a: &ptf_engine::Position, b: &ptf_engine::Position) -> bool {
    a.currency() == b.currency()
        && a.lots().len() == b.lots().len()
        && a.lots().iter().zip(b.lots().iter()).all(|(la, lb)| {
            la.side() == lb.side()
                && la.quantity() == lb.quantity()
                && la.basis_per_unit() == lb.basis_per_unit()
                && la.open_date() == lb.open_date()
                && la.source_transaction_id() == lb.source_transaction_id()
        })
}

/// Sum expected cash per currency from the raw transaction stream.
///
/// **Note:** This function intentionally mirrors the fold's cash arithmetic.
/// It is an oracle that guards against future refactoring breaking the cash
/// conservation contract, not a verification of the original arithmetic itself.
fn expected_cash(txs: &[Transaction]) -> HashMap<Currency, Decimal> {
    let mut cash: HashMap<Currency, Decimal> = HashMap::new();
    for tx in txs {
        match &tx.kind {
            TransactionKind::Deposit { amount } => {
                *cash.entry(amount.currency).or_insert(Decimal::ZERO) += amount.amount;
            }
            TransactionKind::Withdrawal { amount } => {
                *cash.entry(amount.currency).or_insert(Decimal::ZERO) -= amount.amount;
            }
            TransactionKind::Fee { amount, .. } => {
                *cash.entry(amount.currency).or_insert(Decimal::ZERO) -= amount.amount;
            }
            TransactionKind::Buy {
                quantity,
                price,
                fees,
                ..
            } => {
                let cost = quantity * price.amount + fees.amount;
                *cash.entry(price.currency).or_insert(Decimal::ZERO) -= cost;
            }
            TransactionKind::Sell {
                quantity,
                price,
                fees,
                ..
            } => {
                let proceeds = quantity * price.amount - fees.amount;
                *cash.entry(price.currency).or_insert(Decimal::ZERO) += proceeds;
            }
            TransactionKind::Dividend { amount, .. } => {
                *cash.entry(amount.currency).or_insert(Decimal::ZERO) += amount.amount;
            }
            TransactionKind::CorporateAction(_) => {}
        }
    }
    cash
}

fn expected_net_quantity(txs: &[Transaction], instrument: InstrumentId) -> Decimal {
    let mut qty = Decimal::ZERO;
    for tx in txs {
        match &tx.kind {
            TransactionKind::Buy {
                instrument: inst,
                quantity,
                ..
            } if *inst == instrument => qty += quantity,
            TransactionKind::Sell {
                instrument: inst,
                quantity,
                ..
            } if *inst == instrument => qty -= quantity,
            _ => {}
        }
    }
    qty
}

// ===================================================================
// Universe
// ===================================================================

fn universe() -> Vec<(InstrumentId, Currency)> {
    vec![
        (InstrumentId::new(), Currency::USD),
        (InstrumentId::new(), Currency::EUR),
        (InstrumentId::new(), Currency::GBP),
    ]
}

fn all_currencies() -> Vec<Currency> {
    vec![Currency::USD, Currency::EUR, Currency::GBP]
}

// ===================================================================
// Generators
// ===================================================================

fn gen_date() -> impl Strategy<Value = NaiveDate> {
    (1u32..366u32).prop_map(|doy| NaiveDate::from_yo_opt(2024, doy).unwrap())
}

fn gen_config() -> impl Strategy<Value = PortfolioConfig> {
    prop_oneof![Just(LotMethod::Fifo), Just(LotMethod::Lifo)]
        .prop_map(|method| PortfolioConfig::new(method, Currency::USD))
}

// -------------------------------------------------------------------
// Composed sequence generators
// -------------------------------------------------------------------

/// General-purpose sequence: deposits, withdrawals, fees, buys.
/// Never fails — no sells, dividends, or splits.
fn gen_any_sequence(min_len: usize, max_len: usize) -> BoxedStrategy<Vec<Transaction>> {
    let universe = universe();
    let currencies = all_currencies();
    (min_len..=max_len)
        .prop_flat_map(move |n| {
            let universe = universe.clone();
            let currencies = currencies.clone();
            (
                pvec(gen_date(), n),
                pvec(0u8..4u8, n),         // 0=deposit, 1=withdrawal, 2=fee, 3=buy
                pvec(0usize..3, n),        // instrument index (for buys)
                pvec(0usize..3, n),        // currency index (for deposits/fees)
                pvec(1u32..100u32, n),     // quantity
                pvec(100u32..10000u32, n), // price / amount cents
                pvec(0u32..500u32, n),     // fee cents
            )
                .prop_map(move |(dates, kinds, insts, currs, qtys, prices, fees)| {
                    let mut txs = Vec::new();
                    for i in 0..n {
                        let date = dates[i];
                        let (inst, inst_currency) = universe[insts[i] % universe.len()];
                        let currency = currencies[currs[i] % currencies.len()];
                        let qty = Decimal::from(qtys[i]);
                        let amount = Money::new(Decimal::new(prices[i] as i64, 2), currency);
                        let price = Money::new(Decimal::new(prices[i] as i64, 2), inst_currency);
                        let fee = Money::new(Decimal::new(fees[i] as i64, 2), inst_currency);

                        let kind = match kinds[i] % 4 {
                            0 => TransactionKind::deposit(amount).unwrap(),
                            1 => TransactionKind::withdrawal(amount).unwrap(),
                            2 => TransactionKind::fee(amount, None).unwrap(),
                            3 => TransactionKind::buy(inst, qty, price, fee, None).unwrap(),
                            _ => unreachable!(),
                        };
                        txs.push(make_tx(kind, date));
                    }
                    txs.sort_by(|a, b| a.trade_date.cmp(&b.trade_date));
                    txs
                })
        })
        .boxed()
}

/// Only cash-flow transactions (deposits, withdrawals, fees). Never fails.
fn gen_cash_only_sequence(min_len: usize, max_len: usize) -> BoxedStrategy<Vec<Transaction>> {
    let currencies = all_currencies();
    (min_len..=max_len)
        .prop_flat_map(move |n| {
            let currencies = currencies.clone();
            (
                pvec(gen_date(), n),
                pvec(0u8..3u8, n),         // 0=deposit, 1=withdrawal, 2=fee
                pvec(0usize..3, n),        // currency index
                pvec(100u32..10000u32, n), // amount cents
            )
                .prop_map(move |(dates, kinds, currs, amounts)| {
                    let mut txs = Vec::new();
                    for i in 0..n {
                        let date = dates[i];
                        let currency = currencies[currs[i] % currencies.len()];
                        let amount = Money::new(Decimal::new(amounts[i] as i64, 2), currency);
                        let kind = match kinds[i] % 3 {
                            0 => TransactionKind::deposit(amount).unwrap(),
                            1 => TransactionKind::withdrawal(amount).unwrap(),
                            2 => TransactionKind::fee(amount, None).unwrap(),
                            _ => unreachable!(),
                        };
                        txs.push(make_tx(kind, date));
                    }
                    txs.sort_by(|a, b| a.trade_date.cmp(&b.trade_date));
                    txs
                })
        })
        .boxed()
}

/// Scripted buy-then-sell sequence: a buy of qty Q at date D1,
/// followed by a sell of qty Q at date D2 > D1, same instrument and currency.
/// Always valid by construction; exercises the sell / lot-closing path.
fn gen_scripted_buy_then_sell(min_len: usize, max_len: usize) -> BoxedStrategy<Vec<Transaction>> {
    let inst = InstrumentId::new();
    let currency = Currency::USD;
    (min_len..=max_len)
        .prop_flat_map(move |n| {
            (
                pvec(gen_date(), n),
                pvec(1u32..100u32, n),     // qty
                pvec(100u32..10000u32, n), // price cents
                pvec(0u32..500u32, n),     // fee cents
            )
                .prop_map(move |(dates, qtys, prices, fees)| {
                    let mut txs = Vec::new();
                    for i in 0..n {
                        let buy_date = dates[i];
                        let qty = Decimal::from(qtys[i]);
                        let price = Money::new(Decimal::new(prices[i] as i64, 2), currency);
                        let fee = Money::new(Decimal::new(fees[i] as i64, 2), currency);
                        txs.push(make_tx(
                            TransactionKind::buy(inst, qty, price, fee, None).unwrap(),
                            buy_date,
                        ));
                        // Sell on the next day (or same day if last in year)
                        let sell_date = buy_date.succ_opt().unwrap_or(buy_date);
                        txs.push(make_tx(
                            TransactionKind::sell(inst, qty, price, fee, None).unwrap(),
                            sell_date,
                        ));
                    }
                    txs.sort_by(|a, b| a.trade_date.cmp(&b.trade_date));
                    txs
                })
        })
        .boxed()
}

/// No sells, splits, or dividends — only deposits, buys, fees, withdrawals.
/// Never fails.
fn gen_no_sell_sequence(min_len: usize, max_len: usize) -> BoxedStrategy<Vec<Transaction>> {
    let universe = universe();
    let currencies = all_currencies();
    (min_len..=max_len)
        .prop_flat_map(move |n| {
            let universe = universe.clone();
            let currencies = currencies.clone();
            (
                pvec(gen_date(), n),
                pvec(0u8..4u8, n),         // 0=deposit, 1=buy, 2=fee, 3=withdrawal
                pvec(0usize..3, n),        // instrument (for buys)
                pvec(0usize..3, n),        // currency
                pvec(1u32..100u32, n),     // qty
                pvec(100u32..10000u32, n), // amount / price cents
                pvec(0u32..500u32, n),     // fee cents
            )
                .prop_map(move |(dates, kinds, insts, currs, qtys, prices, fees)| {
                    let mut txs = Vec::new();
                    for i in 0..n {
                        let date = dates[i];
                        let (inst, inst_currency) = universe[insts[i] % universe.len()];
                        let currency = currencies[currs[i] % currencies.len()];
                        let qty = Decimal::from(qtys[i]);
                        let amount = Money::new(Decimal::new(prices[i] as i64, 2), currency);
                        let price = Money::new(Decimal::new(prices[i] as i64, 2), inst_currency);
                        let fee = Money::new(Decimal::new(fees[i] as i64, 2), inst_currency);

                        let kind = match kinds[i] % 4 {
                            0 => TransactionKind::deposit(amount).unwrap(),
                            1 => TransactionKind::buy(inst, qty, price, fee, None).unwrap(),
                            2 => TransactionKind::fee(amount, None).unwrap(),
                            3 => TransactionKind::withdrawal(amount).unwrap(),
                            _ => unreachable!(),
                        };
                        txs.push(make_tx(kind, date));
                    }
                    txs.sort_by(|a, b| a.trade_date.cmp(&b.trade_date));
                    txs
                })
        })
        .boxed()
}

/// Single instrument, single currency, only buys and sells.
fn gen_single_instrument_sequence(
    min_len: usize,
    max_len: usize,
) -> BoxedStrategy<Vec<Transaction>> {
    let inst = InstrumentId::new();
    let currency = Currency::USD;
    (min_len..=max_len)
        .prop_flat_map(move |n| {
            (
                pvec(gen_date(), n),
                pvec(0u8..2u8, n),         // 0=buy, 1=sell
                pvec(1u32..100u32, n),     // qty
                pvec(100u32..10000u32, n), // price cents
                pvec(0u32..500u32, n),     // fee cents
            )
                .prop_map(move |(dates, kinds, qtys, prices, fees)| {
                    let mut txs = Vec::new();
                    for i in 0..n {
                        let date = dates[i];
                        let qty = Decimal::from(qtys[i]);
                        let price = Money::new(Decimal::new(prices[i] as i64, 2), currency);
                        let fee = Money::new(Decimal::new(fees[i] as i64, 2), currency);
                        let kind = if kinds[i] % 2 == 0 {
                            TransactionKind::buy(inst, qty, price, fee, None).unwrap()
                        } else {
                            TransactionKind::sell(inst, qty, price, fee, None).unwrap()
                        };
                        txs.push(make_tx(kind, date));
                    }
                    txs.sort_by(|a, b| a.trade_date.cmp(&b.trade_date));
                    txs
                })
        })
        .boxed()
}

/// All transactions share the same trade_date.
/// Only cash flows and buys (no sells / splits / dividends) so that
/// order cannot affect validity.
fn gen_same_date_sequence(min_len: usize, max_len: usize) -> BoxedStrategy<Vec<Transaction>> {
    let currencies = all_currencies();
    (min_len..=max_len)
        .prop_flat_map(move |n| {
            let currencies = currencies.clone();
            let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
            (
                pvec(0u8..3u8, n),         // 0=deposit, 1=withdrawal, 2=fee, 3=buy
                pvec(0usize..3, n),        // currency
                pvec(0usize..3, n),        // instrument (for buys)
                pvec(100u32..10000u32, n), // amount / price cents
                pvec(1u32..50u32, n),      // qty (for buys)
                pvec(0u32..500u32, n),     // fee cents
            )
                .prop_map(move |(kinds, currs, insts, amounts, qtys, fee_cents)| {
                    let universe = universe();
                    let mut txs = Vec::new();
                    for i in 0..n {
                        let currency = currencies[currs[i] % currencies.len()];
                        let amount = Money::new(Decimal::new(amounts[i] as i64, 2), currency);
                        let kind = match kinds[i] % 4 {
                            0 => TransactionKind::deposit(amount).unwrap(),
                            1 => TransactionKind::withdrawal(amount).unwrap(),
                            2 => TransactionKind::fee(amount, None).unwrap(),
                            3 => {
                                let (inst, inst_currency) = universe[insts[i] % universe.len()];
                                let price =
                                    Money::new(Decimal::new(amounts[i] as i64, 2), inst_currency);
                                let fee =
                                    Money::new(Decimal::new(fee_cents[i] as i64, 2), inst_currency);
                                TransactionKind::buy(inst, Decimal::from(qtys[i]), price, fee, None)
                                    .unwrap()
                            }
                            _ => unreachable!(),
                        };
                        txs.push(make_tx(kind, date));
                    }
                    // All same date – sorting is a no-op but keeps precondition valid.
                    txs.sort_by(|a, b| a.trade_date.cmp(&b.trade_date));
                    txs
                })
        })
        .boxed()
}

// ===================================================================
// Properties
// ===================================================================

proptest! {
    // -----------------------------------------------------------------
    // 1. Determinism
    // -----------------------------------------------------------------
    #[test]
    fn determinism(txs in gen_single_instrument_sequence(5, 25)) {
        prop_assume!(ptf_engine::fold(&txs, &PortfolioConfig::new(LotMethod::Fifo, Currency::USD)).is_ok());
        prop_assume!(ptf_engine::fold(&txs, &PortfolioConfig::new(LotMethod::Lifo, Currency::USD)).is_ok());

        let state_fifo_1 = ptf_engine::fold(&txs, &PortfolioConfig::new(LotMethod::Fifo, Currency::USD)).unwrap();
        let state_fifo_2 = ptf_engine::fold(&txs, &PortfolioConfig::new(LotMethod::Fifo, Currency::USD)).unwrap();
        prop_assert!(states_equivalent(&state_fifo_1, &state_fifo_2), "FIFO not deterministic");

        let state_lifo_1 = ptf_engine::fold(&txs, &PortfolioConfig::new(LotMethod::Lifo, Currency::USD)).unwrap();
        let state_lifo_2 = ptf_engine::fold(&txs, &PortfolioConfig::new(LotMethod::Lifo, Currency::USD)).unwrap();
        prop_assert!(states_equivalent(&state_lifo_1, &state_lifo_2), "LIFO not deterministic");
    }

    // -----------------------------------------------------------------
    // 2. Empty fold is empty state
    // -----------------------------------------------------------------
    #[test]
    fn empty_fold(cfg in gen_config()) {
        let state = ptf_engine::fold(&[], &cfg).unwrap();
        prop_assert_eq!(state, PortfolioState::new());
    }

    // -----------------------------------------------------------------
    // 3. Cash conservation (no trades)
    // -----------------------------------------------------------------
    #[test]
    fn cash_conservation_no_trades(txs in gen_cash_only_sequence(3, 20), cfg in gen_config()) {
        let state = ptf_engine::fold(&txs, &cfg).unwrap();
        let expected = expected_cash(&txs);

        for (currency, expected_balance) in expected {
            let actual = state.cash_balance(currency);
            prop_assert_eq!(
                actual, expected_balance,
                "cash mismatch for {}: expected {}, got {}",
                currency, expected_balance, actual
            );
        }
    }

    // -----------------------------------------------------------------
    // 4. Cash conservation including trades
    // -----------------------------------------------------------------
    #[test]
    fn cash_conservation_with_trades(txs in gen_no_sell_sequence(5, 25), cfg in gen_config()) {
        let state = ptf_engine::fold(&txs, &cfg).unwrap();
        let expected = expected_cash(&txs);

        for (currency, expected_balance) in expected {
            let actual = state.cash_balance(currency);
            prop_assert_eq!(
                actual, expected_balance,
                "cash mismatch for {}: expected {}, got {}",
                currency, expected_balance, actual
            );
        }
    }

    #[test]
    fn cash_conservation_scripted_buy_then_sell(
        txs in gen_scripted_buy_then_sell(3, 10),
        cfg in gen_config(),
    ) {
        prop_assume!(ptf_engine::fold(&txs, &cfg).is_ok());

        let state = ptf_engine::fold(&txs, &cfg).unwrap();
        let expected = expected_cash(&txs);

        for (currency, expected_balance) in expected {
            let actual = state.cash_balance(currency);
            prop_assert_eq!(
                actual, expected_balance,
                "cash mismatch for {}: expected {}, got {}",
                currency, expected_balance, actual
            );
        }
    }

    // -----------------------------------------------------------------
    // 5. Basis conservation under splits
    // -----------------------------------------------------------------
    #[test]
    fn basis_conservation_under_splits(
        buy_qty in 1u32..100u32,
        buy_price_cents in 100u32..10000u32,
        split_ratio in prop_oneof![Just(2u32), Just(3u32), Just(4u32), Just(5u32), Just(7u32), Just(8u32)],
        cfg in gen_config(),
    ) {
        let inst = InstrumentId::new();
        let currency = Currency::USD;
        let date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();

        let buy_price = Money::new(Decimal::new(buy_price_cents as i64, 2), currency);
        let buy_fee = Money::new(Decimal::ZERO, currency);
        let buy_qty = Decimal::from(buy_qty);

        let split = Decimal::from(split_ratio);
        let reverse = Decimal::ONE / split;

        let txs = vec![
            make_tx(
                TransactionKind::deposit(Money::new(Decimal::from(100000), currency)).unwrap(),
                date,
            ),
            make_tx(
                TransactionKind::buy(inst, buy_qty, buy_price, buy_fee, None).unwrap(),
                date,
            ),
            make_tx(
                TransactionKind::CorporateAction(
                    CorporateAction::split(inst, split).unwrap(),
                ),
                date,
            ),
            make_tx(
                TransactionKind::CorporateAction(
                    CorporateAction::reverse_split(inst, reverse).unwrap(),
                ),
                date,
            ),
        ];

        let state = ptf_engine::fold(&txs, &cfg).unwrap();
        let pos = state.position(inst).unwrap();

        // Quantity should be restored to original (modulo Decimal rounding
        // when the ratio or its inverse is non-terminating, e.g. 3 or 7).
        let expected_qty = buy_qty;
        let actual_qty = pos.net_quantity();
        let qty_diff = (actual_qty - expected_qty).abs();
        let epsilon = Decimal::new(1, 10); // 1e-10
        prop_assert!(
            qty_diff <= epsilon,
            "quantity mismatch: expected {}, got {}, diff {}",
            expected_qty, actual_qty, qty_diff
        );

        // Basis should be preserved (modulo Decimal rounding for non-exact ratios).
        // Ratios like 3 or 7 produce repeating decimals; basis_per_unit gets
        // truncated at Decimal's max scale, so qty * basis_per_unit may differ
        // from the original total by ~1e-28.
        let expected_basis = buy_qty * buy_price.amount;
        let actual_basis = pos.long_cost_basis().amount;
        let diff = (actual_basis - expected_basis).abs();
        let epsilon = Decimal::new(1, 10); // 1e-10
        prop_assert!(
            diff <= epsilon,
            "basis mismatch: expected {}, got {}, diff {}",
            expected_basis, actual_basis, diff
        );
    }

    // -----------------------------------------------------------------
    // 5b. Split preserves basis in isolation (no reverse split)
    // -----------------------------------------------------------------
    #[test]
    fn split_preserves_basis(
        buy_qty in 1u32..100u32,
        buy_price_cents in 100u32..10000u32,
        split_ratio in prop_oneof![Just(2u32), Just(3u32), Just(4u32), Just(5u32), Just(7u32), Just(8u32)],
        cfg in gen_config(),
    ) {
        let inst = InstrumentId::new();
        let currency = Currency::USD;
        let date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();

        let buy_price = Money::new(Decimal::new(buy_price_cents as i64, 2), currency);
        let buy_fee = Money::new(Decimal::ZERO, currency);
        let buy_qty = Decimal::from(buy_qty);
        let split = Decimal::from(split_ratio);

        let txs = vec![
            make_tx(
                TransactionKind::deposit(Money::new(Decimal::from(100000), currency)).unwrap(),
                date,
            ),
            make_tx(
                TransactionKind::buy(inst, buy_qty, buy_price, buy_fee, None).unwrap(),
                date,
            ),
            make_tx(
                TransactionKind::CorporateAction(
                    CorporateAction::split(inst, split).unwrap(),
                ),
                date,
            ),
        ];

        let state = ptf_engine::fold(&txs, &cfg).unwrap();
        let pos = state.position(inst).unwrap();

        // Total cost basis must be unchanged by the split (modulo Decimal
        // rounding when the ratio produces a non-terminating decimal).
        let expected_basis = buy_qty * buy_price.amount;
        let actual_basis = pos.long_cost_basis().amount;
        let diff = (actual_basis - expected_basis).abs();
        let epsilon = Decimal::new(1, 10); // 1e-10
        prop_assert!(
            diff <= epsilon,
            "basis mismatch after split: expected {}, got {}, diff {}",
            expected_basis, actual_basis, diff
        );
    }

    // -----------------------------------------------------------------
    // 6. Net quantity correctness
    // -----------------------------------------------------------------
    #[test]
    fn net_quantity_correctness(txs in gen_single_instrument_sequence(3, 15), cfg in gen_config()) {
        prop_assume!(ptf_engine::fold(&txs, &cfg).is_ok());

        let state = ptf_engine::fold(&txs, &cfg).unwrap();
        // Find the single instrument used in this sequence
        let instrument = txs.iter().find_map(|tx| match &tx.kind {
            TransactionKind::Buy { instrument, .. } => Some(*instrument),
            TransactionKind::Sell { instrument, .. } => Some(*instrument),
            _ => None,
        }).unwrap();

        let expected_qty = expected_net_quantity(&txs, instrument);
        let pos = state.position(instrument);
        let actual_qty = pos.map(|p| p.net_quantity()).unwrap_or(Decimal::ZERO);

        prop_assert_eq!(
            actual_qty, expected_qty,
            "net quantity mismatch: expected {}, got {}",
            expected_qty, actual_qty
        );
    }

    // -----------------------------------------------------------------
    // 7. Realized PnL only changes on lot-closing events
    // -----------------------------------------------------------------
    #[test]
    fn realized_pnl_zero_without_sells(txs in gen_no_sell_sequence(3, 20), cfg in gen_config()) {
        let state = ptf_engine::fold(&txs, &cfg).unwrap();
        for currency in all_currencies() {
            prop_assert_eq!(
                state.realized_pnl_in(currency),
                Decimal::ZERO,
                "realized PnL for {} should be zero when no sells occur",
                currency
            );
        }
    }

    // -----------------------------------------------------------------
    // 8. Currency isolation
    // -----------------------------------------------------------------
    #[test]
    fn currency_isolation(txs in gen_any_sequence(5, 20), cfg in gen_config()) {
        let result = ptf_engine::fold(&txs, &cfg);
        prop_assume!(result.is_ok());

        let state = result.unwrap();
        let currencies = all_currencies();

        // For every currency, check that only transactions in that currency affected it.
        for currency in &currencies {
            let expected = expected_cash(&txs).get(currency).copied().unwrap_or(Decimal::ZERO);
            let actual = state.cash_balance(*currency);
            prop_assert_eq!(
                actual, expected,
                "cash for {} should be isolated", currency
            );
        }
    }

    // -----------------------------------------------------------------
    // 9. Order-independent invariants under same date
    // -----------------------------------------------------------------
    #[test]
    fn same_date_order_independence(txs in gen_same_date_sequence(3, 8), cfg in gen_config()) {
        prop_assume!(ptf_engine::fold(&txs, &cfg).is_ok());

        let state_orig = ptf_engine::fold(&txs, &cfg).unwrap();

        // Reversed order
        let mut reversed = txs.clone();
        reversed.reverse();
        let state_rev = ptf_engine::fold(&reversed, &cfg).unwrap();

        // Cash must match
        for currency in all_currencies() {
            prop_assert_eq!(
                state_orig.cash_balance(currency),
                state_rev.cash_balance(currency),
                "cash mismatch for {} between original and reversed order", currency
            );
        }

        // Net quantities must match for every instrument that appears
        let instruments: std::collections::HashSet<_> = txs.iter().filter_map(|tx| match &tx.kind {
            TransactionKind::Buy { instrument, .. } => Some(*instrument),
            _ => None,
        }).collect();

        for inst in instruments {
            let qty_orig = state_orig.position(inst).map(|p| p.net_quantity()).unwrap_or(Decimal::ZERO);
            let qty_rev = state_rev.position(inst).map(|p| p.net_quantity()).unwrap_or(Decimal::ZERO);
            prop_assert_eq!(
                qty_orig, qty_rev,
                "net quantity mismatch for {:?} between original and reversed order", inst
            );
        }
    }
}
