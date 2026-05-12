use rust_decimal::Decimal;

use crate::error::DomainError;
use crate::ids::LotId;
use crate::lot::Lot;
use crate::lot_method::{LotMethod, LotSide};
use crate::money::Money;
use crate::portfolio_config::PortfolioConfig;
use crate::portfolio_state::PortfolioState;
use crate::position::Position;
use crate::transaction::{Transaction, TransactionKind};

/// Fold a chronological slice of transactions into a [`PortfolioState`].
///
/// # Precondition
/// Input `transactions` must be ordered by non-decreasing `trade_date`.
/// Violations return [`DomainError::UnorderedTransactions`].
///
/// # Cash semantics
/// Cash balances may go negative — callers needing overdraft detection
/// should check balances themselves.
pub fn fold(
    transactions: &[Transaction],
    config: &PortfolioConfig,
) -> Result<PortfolioState, DomainError> {
    // Chronological validation
    for (i, window) in transactions.windows(2).enumerate() {
        let prev = &window[0];
        let curr = &window[1];
        if curr.trade_date < prev.trade_date {
            return Err(DomainError::UnorderedTransactions(i + 1));
        }
    }

    let mut state = PortfolioState::new();
    for tx in transactions {
        apply(&mut state, tx, config)?;
    }
    Ok(state)
}

/// Apply a single transaction to mutable state.
fn apply(
    state: &mut PortfolioState,
    tx: &Transaction,
    config: &PortfolioConfig,
) -> Result<(), DomainError> {
    match &tx.kind {
        TransactionKind::Deposit { amount } => {
            *state.cash.entry(amount.currency).or_insert(Decimal::ZERO) += amount.amount;
            Ok(())
        }
        TransactionKind::Withdrawal { amount } => {
            *state.cash.entry(amount.currency).or_insert(Decimal::ZERO) -= amount.amount;
            Ok(())
        }
        TransactionKind::Fee { amount, .. } => {
            *state.cash.entry(amount.currency).or_insert(Decimal::ZERO) -= amount.amount;
            Ok(())
        }
        TransactionKind::Buy {
            instrument,
            quantity,
            price,
            fees,
            ..
        } => {
            let total_cost = quantity * price.amount + fees.amount;
            *state.cash.entry(price.currency).or_insert(Decimal::ZERO) -= total_cost;

            // TODO: rounding behavior under fractional fee allocation.
            let cost_per_unit = price.amount + (fees.amount / quantity);

            let position = state
                .positions
                .entry(*instrument)
                .or_insert_with(|| Position::new(*instrument, price.currency));

            if position.currency() != price.currency {
                return Err(DomainError::CurrencyMismatch {
                    expected: position.currency(),
                    got: price.currency,
                });
            }

            let mut realized_pnl = Decimal::ZERO;
            let mut remaining = *quantity;

            // Cover existing short lots first.
            let short_qty = position.total_short_quantity();
            if short_qty > Decimal::ZERO {
                let cover_qty = remaining.min(short_qty);
                let mut remaining_cover = cover_qty;

                // performance: O(n log n) sort with temp vec; revisit if lot counts grow.
                let mut short_lots: Vec<&mut Lot> = position
                    .lots
                    .iter_mut()
                    .filter(|l| l.side() == LotSide::Short)
                    .collect();
                sort_lots(&mut short_lots, config.lot_method);

                for lot in short_lots {
                    if remaining_cover <= Decimal::ZERO {
                        break;
                    }
                    let close_qty = remaining_cover.min(lot.quantity);
                    // Short-cover PnL: (original_proceeds - cover_cost)
                    let lot_pnl = (lot.basis_per_unit().amount - cost_per_unit) * close_qty;
                    realized_pnl += lot_pnl;
                    lot.quantity -= close_qty;
                    remaining_cover -= close_qty;
                }

                remaining -= cover_qty;
            }

            // Any remainder opens a new long lot.
            if remaining > Decimal::ZERO {
                let seq = state.next_lot_sequence;
                state.next_lot_sequence += 1;
                position.lots.push(Lot::new(
                    LotId::new(),
                    seq,
                    LotSide::Long,
                    remaining,
                    Money::new(cost_per_unit, price.currency),
                    tx.trade_date,
                    tx.id,
                ));
            }

            position.lots.retain(|l| l.quantity > Decimal::ZERO);
            if position.is_empty() {
                state.positions.remove(instrument);
            }

            *state
                .realized_pnl
                .entry(price.currency)
                .or_insert(Decimal::ZERO) += realized_pnl;

            Ok(())
        }
        TransactionKind::Sell {
            instrument,
            quantity,
            price,
            fees,
            ..
        } => {
            let total_proceeds = quantity * price.amount - fees.amount;
            *state.cash.entry(price.currency).or_insert(Decimal::ZERO) += total_proceeds;

            // TODO: rounding behavior under fractional fee allocation.
            let sale_price_per_unit = price.amount - (fees.amount / quantity);

            let position = state
                .positions
                .entry(*instrument)
                .or_insert_with(|| Position::new(*instrument, price.currency));

            if position.currency() != price.currency {
                return Err(DomainError::CurrencyMismatch {
                    expected: position.currency(),
                    got: price.currency,
                });
            }

            let mut realized_pnl = Decimal::ZERO;
            let mut remaining = *quantity;

            // Close existing long lots first.
            let long_qty = position.total_long_quantity();
            if long_qty > Decimal::ZERO {
                let close_qty = remaining.min(long_qty);
                let mut remaining_close = close_qty;

                // performance: O(n log n) sort with temp vec; revisit if lot counts grow.
                let mut long_lots: Vec<&mut Lot> = position
                    .lots
                    .iter_mut()
                    .filter(|l| l.side() == LotSide::Long)
                    .collect();
                sort_lots(&mut long_lots, config.lot_method);

                for lot in long_lots {
                    if remaining_close <= Decimal::ZERO {
                        break;
                    }
                    let qty = remaining_close.min(lot.quantity);
                    // Long-close PnL: (sale_price - cost_basis)
                    let lot_pnl = (sale_price_per_unit - lot.basis_per_unit().amount) * qty;
                    realized_pnl += lot_pnl;
                    lot.quantity -= qty;
                    remaining_close -= qty;
                }

                remaining -= close_qty;
            }

            // Any remainder opens a new short lot.
            if remaining > Decimal::ZERO {
                let seq = state.next_lot_sequence;
                state.next_lot_sequence += 1;
                position.lots.push(Lot::new(
                    LotId::new(),
                    seq,
                    LotSide::Short,
                    remaining,
                    Money::new(sale_price_per_unit, price.currency),
                    tx.trade_date,
                    tx.id,
                ));
            }

            position.lots.retain(|l| l.quantity > Decimal::ZERO);
            if position.is_empty() {
                state.positions.remove(instrument);
            }

            *state
                .realized_pnl
                .entry(price.currency)
                .or_insert(Decimal::ZERO) += realized_pnl;

            Ok(())
        }
        TransactionKind::Dividend { .. } | TransactionKind::CorporateAction(_) => {
            unimplemented!()
        }
    }
}

fn sort_lots(lots: &mut Vec<&mut Lot>, method: LotMethod) {
    match method {
        LotMethod::Fifo => {
            lots.sort_by(|a, b| a.open_date.cmp(&b.open_date).then(a.sequence.cmp(&b.sequence)));
        }
        LotMethod::Lifo => {
            lots.sort_by(|a, b| b.open_date.cmp(&a.open_date).then(b.sequence.cmp(&a.sequence)));
        }
        _ => unimplemented!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{InstrumentId, TransactionId};
    use crate::lot_method::LotMethod;
    use crate::{Currency, Money};
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    fn cfg() -> PortfolioConfig {
        PortfolioConfig::new(LotMethod::Fifo, Currency::USD)
    }

    fn tx(date: NaiveDate, kind: TransactionKind) -> Transaction {
        Transaction::new(TransactionId::new(), date, date, kind).unwrap()
    }

    fn usd(dollars: &str) -> Money {
        Money::new(Decimal::from_str_exact(dollars).unwrap(), Currency::USD)
    }

    fn eur(amount: &str) -> Money {
        Money::new(Decimal::from_str_exact(amount).unwrap(), Currency::EUR)
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new()
    }

    #[test]
    fn deposit_increases_cash() {
        let txs = vec![tx(
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            TransactionKind::deposit(usd("100.00")).unwrap(),
        )];
        let state = fold(&txs, &cfg()).unwrap();
        assert_eq!(state.cash_balance(Currency::USD), Decimal::from(100));
    }

    #[test]
    fn withdrawal_decreases_cash() {
        let txs = vec![tx(
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            TransactionKind::withdrawal(usd("50.00")).unwrap(),
        )];
        let state = fold(&txs, &cfg()).unwrap();
        assert_eq!(state.cash_balance(Currency::USD), Decimal::from(-50));
    }

    #[test]
    fn fee_decreases_cash() {
        let txs = vec![tx(
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            TransactionKind::fee(usd("5.00"), None).unwrap(),
        )];
        let state = fold(&txs, &cfg()).unwrap();
        assert_eq!(state.cash_balance(Currency::USD), Decimal::from(-5));
    }

    #[test]
    fn multiple_deposits_same_currency() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let txs = vec![
            tx(d1, TransactionKind::deposit(usd("100.00")).unwrap()),
            tx(d2, TransactionKind::deposit(usd("50.00")).unwrap()),
        ];
        let state = fold(&txs, &cfg()).unwrap();
        assert_eq!(state.cash_balance(Currency::USD), Decimal::from(150));
    }

    #[test]
    fn multi_currency_cash() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let txs = vec![
            tx(d1, TransactionKind::deposit(usd("100.00")).unwrap()),
            tx(d2, TransactionKind::deposit(eur("80.00")).unwrap()),
        ];
        let state = fold(&txs, &cfg()).unwrap();
        assert_eq!(state.cash_balance(Currency::USD), Decimal::from(100));
        assert_eq!(state.cash_balance(Currency::EUR), Decimal::from(80));
        assert_eq!(state.cash_balance(Currency::GBP), Decimal::ZERO);
    }

    #[test]
    fn deposit_withdraw_fee_sequence() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();
        let txs = vec![
            tx(d1, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(d2, TransactionKind::withdrawal(usd("200.00")).unwrap()),
            tx(d3, TransactionKind::fee(usd("9.99"), None).unwrap()),
        ];
        let state = fold(&txs, &cfg()).unwrap();
        assert_eq!(state.cash_balance(Currency::USD), Decimal::new(79001, 2));
    }

    #[test]
    fn unordered_transactions_fails() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d1, TransactionKind::deposit(usd("100.00")).unwrap()),
            tx(d2, TransactionKind::deposit(usd("50.00")).unwrap()),
        ];
        let result = fold(&txs, &cfg());
        assert!(matches!(result, Err(DomainError::UnorderedTransactions(1))));
    }

    #[test]
    fn same_date_is_allowed() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("100.00")).unwrap()),
            tx(d, TransactionKind::withdrawal(usd("30.00")).unwrap()),
        ];
        let state = fold(&txs, &cfg()).unwrap();
        assert_eq!(state.cash_balance(Currency::USD), Decimal::from(70));
    }

    // ------------------------------------------------------------------
    // Slice 2 — long-only Buy / Sell
    // ------------------------------------------------------------------

    #[test]
    fn buy_creates_long_lot() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("10.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        assert_eq!(state.cash_balance(Currency::USD), Decimal::from(490));
        let pos = state.position(inst).unwrap();
        assert_eq!(pos.net_quantity(), Decimal::from(10));
        assert_eq!(pos.long_cost_basis(), Money::new(Decimal::from(510), Currency::USD));
    }

    #[test]
    fn partial_sell_shrinks_lot() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("10.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(3), usd("60.00"), usd("5.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        let pos = state.position(inst).unwrap();
        assert_eq!(pos.net_quantity(), Decimal::from(7));
        assert_eq!(pos.lots().len(), 1);
    }

    #[test]
    fn full_sell_removes_lot_and_position() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("10.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(10), usd("60.00"), usd("5.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        assert!(state.position(inst).is_none());
        assert!(state.positions().is_empty());
    }

    #[test]
    fn sell_fifo_order() {
        let inst = instrument();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let txs = vec![
            tx(d1, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d1,
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d2,
                TransactionKind::buy(inst, Decimal::from(10), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d2,
                TransactionKind::sell(inst, Decimal::from(10), usd("70.00"), usd("0.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap(); // FIFO

        let pos = state.position(inst).unwrap();
        assert_eq!(pos.net_quantity(), Decimal::from(10));
        // The $60 lot should remain (FIFO closed the $50 lot first).
        assert_eq!(pos.long_cost_basis(), Money::new(Decimal::from(600), Currency::USD));
    }

    #[test]
    fn sell_lifo_order() {
        let inst = instrument();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let txs = vec![
            tx(d1, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d1,
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d2,
                TransactionKind::buy(inst, Decimal::from(10), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d2,
                TransactionKind::sell(inst, Decimal::from(10), usd("70.00"), usd("0.00"), None)
                    .unwrap(),
            ),
        ];
        let lifo_cfg = PortfolioConfig::new(LotMethod::Lifo, Currency::USD);
        let state = fold(&txs, &lifo_cfg).unwrap();

        let pos = state.position(inst).unwrap();
        assert_eq!(pos.net_quantity(), Decimal::from(10));
        // The $50 lot should remain (LIFO closed the $60 lot first).
        assert_eq!(pos.long_cost_basis(), Money::new(Decimal::from(500), Currency::USD));
    }

    #[test]
    fn fifo_same_date_determinism() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(5), usd("70.00"), usd("0.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        // FIFO closes the first buy (sequence 0), leaving second buy intact.
        let pos = state.position(inst).unwrap();
        assert_eq!(pos.net_quantity(), Decimal::from(15));
        // Cost basis: 5 * 50 + 10 * 60 = 250 + 600 = 850
        assert_eq!(
            pos.long_cost_basis(),
            Money::new(Decimal::from(850), Currency::USD)
        );
    }

    #[test]
    fn lifo_same_date_determinism() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(5), usd("70.00"), usd("0.00"), None)
                    .unwrap(),
            ),
        ];
        let lifo_cfg = PortfolioConfig::new(LotMethod::Lifo, Currency::USD);
        let state = fold(&txs, &lifo_cfg).unwrap();

        // LIFO closes the second buy (sequence 1), leaving first buy intact.
        let pos = state.position(inst).unwrap();
        assert_eq!(pos.net_quantity(), Decimal::from(15));
        // Cost basis: 10 * 50 + 5 * 60 = 500 + 300 = 800
        assert_eq!(
            pos.long_cost_basis(),
            Money::new(Decimal::from(800), Currency::USD)
        );
    }

    #[test]
    fn multi_currency_positions() {
        let usd_inst = instrument();
        let eur_inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(d, TransactionKind::deposit(eur("500.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(usd_inst, Decimal::from(10), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::buy(eur_inst, Decimal::from(5), eur("80.00"), eur("0.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        assert_eq!(state.cash_balance(Currency::USD), Decimal::from(500));
        assert_eq!(state.cash_balance(Currency::EUR), Decimal::from(100));
        assert_eq!(
            state.position(usd_inst).unwrap().long_cost_basis(),
            Money::new(Decimal::from(500), Currency::USD)
        );
        assert_eq!(
            state.position(eur_inst).unwrap().long_cost_basis(),
            Money::new(Decimal::from(400), Currency::EUR)
        );
    }

    #[test]
    fn buy_currency_mismatch_errors() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(5), eur("80.00"), eur("0.00"), None)
                    .unwrap(),
            ),
        ];
        let result = fold(&txs, &cfg());
        assert!(matches!(
            result,
            Err(DomainError::CurrencyMismatch { expected, got })
                if expected == Currency::USD && got == Currency::EUR
        ));
    }

    #[test]
    fn sell_currency_mismatch_errors() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(5), eur("80.00"), eur("0.00"), None)
                    .unwrap(),
            ),
        ];
        let result = fold(&txs, &cfg());
        assert!(matches!(
            result,
            Err(DomainError::CurrencyMismatch { expected, got })
                if expected == Currency::USD && got == Currency::EUR
        ));
    }

    #[test]
    fn realized_pnl_on_sell() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("50.00"), usd("10.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(10), usd("70.00"), usd("5.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        // Basis: 10 * (50 + 1) = 510
        // Proceeds: 10 * 70 - 5 = 695
        // PnL: 695 - 510 = 185
        assert_eq!(state.realized_pnl_in(Currency::USD), Decimal::from(185));
    }

    // ------------------------------------------------------------------
    // Slice 3 — short-side flips
    // ------------------------------------------------------------------

    #[test]
    fn sell_past_long_opens_short() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(5), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(8), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        // Closed 5 longs, opened 3 shorts.
        let pos = state.position(inst).unwrap();
        assert_eq!(pos.net_quantity(), Decimal::from(-3));
        assert_eq!(pos.total_short_quantity(), Decimal::from(3));
        assert_eq!(pos.total_long_quantity(), Decimal::ZERO);
    }

    #[test]
    fn buy_past_short_opens_long() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(5), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(8), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(10), usd("55.00"), usd("0.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        // Cover 3 shorts, open 7 longs.
        let pos = state.position(inst).unwrap();
        assert_eq!(pos.net_quantity(), Decimal::from(7));
        assert_eq!(pos.total_long_quantity(), Decimal::from(7));
        assert_eq!(pos.total_short_quantity(), Decimal::ZERO);
    }

    #[test]
    fn sell_exactly_long_leaves_flat() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(5), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(5), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        assert!(state.position(inst).is_none());
    }

    #[test]
    fn buy_exactly_short_leaves_flat() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(5), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(8), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(3), usd("55.00"), usd("0.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        assert!(state.position(inst).is_none());
    }

    #[test]
    fn short_cover_pnl_profit() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(3), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(3), usd("50.00"), usd("0.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        // Short opened at $60, covered at $50 → $10 profit per share * 3 = $30
        assert_eq!(state.realized_pnl_in(Currency::USD), Decimal::from(30));
    }

    #[test]
    fn short_cover_pnl_loss() {
        let inst = instrument();
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let txs = vec![
            tx(d, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d,
                TransactionKind::sell(inst, Decimal::from(3), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ),
            tx(
                d,
                TransactionKind::buy(inst, Decimal::from(3), usd("70.00"), usd("0.00"), None)
                    .unwrap(),
            ),
        ];
        let state = fold(&txs, &cfg()).unwrap();

        // Short opened at $60, covered at $70 → $10 loss per share * 3 = -$30
        assert_eq!(state.realized_pnl_in(Currency::USD), Decimal::from(-30));
    }

    #[test]
    fn multiple_shorts_covered_fifo() {
        let inst = instrument();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let txs = vec![
            tx(d1, TransactionKind::deposit(usd("1000.00")).unwrap()),
            tx(
                d1,
                TransactionKind::sell(inst, Decimal::from(5), usd("60.00"), usd("0.00"), None)
                    .unwrap(),
            ), // short 5 at $60
            tx(
                d2,
                TransactionKind::sell(inst, Decimal::from(5), usd("70.00"), usd("0.00"), None)
                    .unwrap(),
            ), // short 5 at $70
            tx(
                d2,
                TransactionKind::buy(inst, Decimal::from(5), usd("55.00"), usd("0.00"), None)
                    .unwrap(),
            ), // cover 5
        ];
        let state = fold(&txs, &cfg()).unwrap();

        // FIFO covers the $60 short first: profit = (60 - 55) * 5 = $25
        let pos = state.position(inst).unwrap();
        assert_eq!(pos.net_quantity(), Decimal::from(-5));
        assert_eq!(pos.total_short_quantity(), Decimal::from(5));
        assert_eq!(state.realized_pnl_in(Currency::USD), Decimal::from(25));
    }
}
