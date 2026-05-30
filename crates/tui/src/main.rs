mod data;

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use ptf_engine::{
    Currency, FxRateProvider, Instrument, InstrumentId, LotMethod, LotSide, Money,
    MonteCarloConfig, PortfolioState, Position, PriceProvider, TransactionKind, VaRReport,
    compute_var, fold,
};

use crate::data::{SeedData, seed_data, seed_data_us_growth};

// ── App State ───────────────────────────────────────────────────────────

enum Screen {
    Picker,
    Dashboard,
    Ledger,
    LotInspector(InstrumentId),
    Analytics,
}

enum Popup {
    Valuation { base: Currency, value: Money },
}

struct App {
    screen: Screen,
    seeds: Vec<SeedData>,
    current: usize,
    picker_selected: usize,
    dash_table_state: TableState,
    ledger_scroll: usize,
    popup: Option<Popup>,
    valuation_bases: Vec<Currency>,
    valuation_idx: usize,
    should_quit: bool,
    // Time machine: fold(&transactions[..time_index], config)
    time_index: usize,
    time_states: Vec<PortfolioState>,
    // Toggle bottom-right panel between Realized PnL and Currency Exposure
    show_currency_exposure: bool,
    // Cached VaR report
    var_report: Option<VaRReport>,
}

impl App {
    fn new(seeds: Vec<SeedData>) -> Self {
        let mut app = Self {
            screen: Screen::Picker,
            seeds,
            current: 0,
            picker_selected: 0,
            dash_table_state: TableState::default().with_selected(Some(0)),
            ledger_scroll: 0,
            popup: None,
            valuation_bases: vec![Currency::EUR, Currency::USD, Currency::JPY],
            valuation_idx: 0,
            should_quit: false,
            time_index: 0,
            time_states: Vec::new(),
            show_currency_exposure: false,
            var_report: None,
        };
        app.recompute_time_states();
        app
    }

    fn recompute_time_states(&mut self) {
        let seed = &self.seeds[self.current];
        self.time_states = (0..=seed.transactions.len())
            .map(|i| fold(&seed.transactions[..i], &seed.config).unwrap())
            .collect();
        self.time_index = seed.transactions.len();
        self.var_report = None;
    }

    fn current_seed(&self) -> &SeedData {
        &self.seeds[self.current]
    }

    fn display_state(&self) -> &PortfolioState {
        &self.time_states[self.time_index]
    }

    fn current_valuation_base(&self) -> Currency {
        self.valuation_bases[self.valuation_idx % self.valuation_bases.len()]
    }

    fn compute_var_report(&mut self) -> &VaRReport {
        if self.var_report.is_none() {
            let seed = self.current_seed();
            let state = self.display_state();
            let config = MonteCarloConfig::default_var();
            let report = compute_var(
                state,
                &seed.historical_prices,
                &seed.fx,
                &seed.prices,
                &config,
                seed.portfolio.base_currency,
                seed.as_of,
            )
            .unwrap_or_else(|e| {
                eprintln!("VaR computation failed: {e}");
                VaRReport {
                    as_of: seed.as_of,
                    base_currency: seed.portfolio.base_currency,
                    entries: Vec::new(),
                    per_asset: Vec::new(),
                }
            });
            self.var_report = Some(report);
        }
        self.var_report.as_ref().unwrap()
    }

    fn next_valuation_base(&mut self) {
        self.valuation_idx += 1;
    }

    fn compute_valuation(&self, base: Currency) -> Money {
        let seed = self.current_seed();
        self.display_state()
            .total_value(&seed.fx, &seed.prices, base, seed.as_of)
            .unwrap_or_else(|_e| Money::new(Decimal::ZERO, base))
    }

    fn time_machine_as_of(&self) -> String {
        let seed = self.current_seed();
        if self.time_index == 0 {
            format!("{}", seed.portfolio.inception_date)
        } else if self.time_index <= seed.transactions.len() {
            format!("{}", seed.transactions[self.time_index - 1].trade_date)
        } else {
            format!("{}", seed.as_of)
        }
    }

    fn time_machine_label(&self) -> String {
        let seed = self.current_seed();
        let max = seed.transactions.len();
        if self.time_index == max {
            "Live".into()
        } else {
            format!("Replay {}/{max}", self.time_index)
        }
    }

    fn time_machine_gauge(&self) -> String {
        let seed = self.current_seed();
        let max = seed.transactions.len();
        if max == 0 {
            return String::new();
        }
        let filled = (self.time_index * 20) / max;
        let empty = 20 - filled;
        format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn money_str(m: Money) -> String {
    let sym = match m.currency {
        Currency::USD => "$",
        Currency::EUR => "€",
        Currency::GBP => "£",
        Currency::JPY => "¥",
        Currency::CHF => "Fr",
        _ => m.currency.as_str(),
    };
    format!("{sym}{}", format_money(m.amount))
}

fn currency_sym(c: Currency) -> String {
    match c {
        Currency::USD => "$".into(),
        Currency::EUR => "€".into(),
        Currency::GBP => "£".into(),
        Currency::JPY => "¥".into(),
        Currency::CHF => "Fr".into(),
        _ => c.as_str().to_string(),
    }
}

fn format_money(d: Decimal) -> String {
    d.round_dp(2).to_string()
}

fn text_bar(value: Decimal, max: Decimal, width: usize) -> String {
    if max.is_zero() {
        return "░".repeat(width);
    }
    let ratio = (value / max).min(Decimal::ONE);
    #[allow(clippy::cast_possible_truncation)]
    let filled = (ratio * Decimal::from(width as u64))
        .to_u64()
        .unwrap_or(0)
        .min(width as u64) as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn lot_method_str(m: LotMethod) -> &'static str {
    match m {
        LotMethod::Fifo => "FIFO",
        LotMethod::Lifo => "LIFO",
        LotMethod::HighestCost => "HighestCost",
        LotMethod::LowestCost => "LowestCost",
        LotMethod::AverageCost => "AverageCost",
    }
}

fn position_symbol<'a>(pos: &Position, instruments: &'a [Instrument]) -> Option<&'a str> {
    instruments
        .iter()
        .find(|i| i.id == pos.instrument())
        .map(|i| i.symbol.as_str())
}

fn instrument_by_id(id: InstrumentId, instruments: &[Instrument]) -> Option<&Instrument> {
    instruments.iter().find(|i| i.id == id)
}

fn tx_color(kind: &TransactionKind) -> Color {
    match kind {
        TransactionKind::Buy { .. } => Color::Green,
        TransactionKind::Sell { .. } | TransactionKind::Withdrawal { .. } => Color::Red,
        TransactionKind::Deposit { .. } => Color::Blue,
        TransactionKind::Fee { .. } => Color::Magenta,
        TransactionKind::Dividend { .. } => Color::Cyan,
        TransactionKind::CorporateAction(_) => Color::Yellow,
    }
}

fn tx_label(kind: &TransactionKind) -> &'static str {
    match kind {
        TransactionKind::Buy { .. } => "Buy",
        TransactionKind::Sell { .. } => "Sell",
        TransactionKind::Deposit { .. } => "Deposit",
        TransactionKind::Withdrawal { .. } => "Withdrawal",
        TransactionKind::Fee { .. } => "Fee",
        TransactionKind::Dividend { .. } => "Dividend",
        TransactionKind::CorporateAction(ca) => match ca {
            ptf_engine::CorporateAction::Split { .. } => "Split",
            ptf_engine::CorporateAction::ReverseSplit { .. } => "Rev-Split",
            _ => "Corp-Action",
        },
    }
}

fn side_label(pos: &Position) -> &'static str {
    if pos.is_long() {
        "Long"
    } else if pos.is_short() {
        "Short"
    } else {
        "Flat"
    }
}

fn format_decimal(d: Decimal) -> String {
    let s = d.normalize().to_string();
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn position_rows(
    state: &PortfolioState,
    instruments: &[Instrument],
    prices: &impl PriceProvider,
    fx: &impl FxRateProvider,
    base: Currency,
    as_of: chrono::NaiveDate,
) -> Vec<Row<'static>> {
    let mut positions: Vec<_> = state.positions().values().collect();
    positions.sort_by(|a, b| {
        let sa = position_symbol(a, instruments).unwrap_or("ZZZ");
        let sb = position_symbol(b, instruments).unwrap_or("ZZZ");
        sa.cmp(sb)
    });

    positions
        .into_iter()
        .map(|pos| {
            let sym = position_symbol(pos, instruments).unwrap_or("?");
            let qty = pos.net_quantity();
            let side = side_label(pos);
            let avg = pos.long_cost_basis().amount / pos.total_long_quantity().max(Decimal::ONE);
            let avg_cost = if pos.is_short() {
                pos.short_proceeds_basis()
            } else {
                Money::new(avg, pos.currency())
            };

            let price = prices
                .price(pos.instrument(), as_of)
                .unwrap_or(Money::new(Decimal::ZERO, pos.currency()));

            let base_value = if let Ok(rate) = fx.rate(pos.currency(), base, as_of) {
                qty * price.amount * rate
            } else {
                Decimal::ZERO
            };

            let unrealized = if pos.is_long() {
                let cb = pos.long_cost_basis().amount;
                let mv = pos.total_long_quantity() * price.amount;
                mv - cb
            } else if pos.is_short() {
                let sb = pos.short_proceeds_basis().amount;
                let mv = pos.total_short_quantity() * price.amount;
                sb - mv
            } else {
                Decimal::ZERO
            };

            let base_unrealized = if let Ok(rate) = fx.rate(pos.currency(), base, as_of) {
                unrealized * rate
            } else {
                Decimal::ZERO
            };

            Row::new(vec![
                Cell::from(sym.to_string()),
                Cell::from(side.to_string()),
                Cell::from(format_decimal(qty)),
                Cell::from(money_str(avg_cost)),
                Cell::from(money_str(price)),
                Cell::from(format!(
                    "{}{}",
                    currency_sym(base),
                    format_money(base_value)
                )),
                Cell::from(format!(
                    "{}{}",
                    currency_sym(base),
                    format_money(base_unrealized)
                )),
            ])
        })
        .collect()
}

fn currency_exposure(
    state: &PortfolioState,
    prices: &impl PriceProvider,
    as_of: chrono::NaiveDate,
) -> Vec<(Currency, Decimal)> {
    let mut exposure: HashMap<Currency, Decimal> = state.cash().clone();
    for (inst_id, pos) in state.positions() {
        let price = prices
            .price(*inst_id, as_of)
            .unwrap_or(Money::new(Decimal::ZERO, pos.currency()));
        let pos_value = pos.net_quantity() * price.amount;
        *exposure.entry(pos.currency()).or_insert(Decimal::ZERO) += pos_value;
    }
    let mut v: Vec<_> = exposure.into_iter().collect();
    v.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    v
}

fn days_between(start: chrono::NaiveDate, end: chrono::NaiveDate) -> i64 {
    (end - start).num_days()
}

// ── Rendering ─────────────────────────────────────────────────────────────

fn render_picker(f: &mut Frame, app: &App) {
    let area = f.area();

    let block = Block::default()
        .title(" ptf-engine — Portfolio Manager ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    let rows: Vec<Row> = app
        .seeds
        .iter()
        .enumerate()
        .map(|(i, seed)| {
            let style = if i == app.picker_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let pos_count = seed.state.positions().len();
            Row::new(vec![
                Cell::from(seed.portfolio.name.as_str()),
                Cell::from(seed.portfolio.base_currency.to_string()),
                Cell::from(lot_method_str(seed.portfolio.lot_method)),
                Cell::from(seed.portfolio.inception_date.to_string()),
                Cell::from(format!("{pos_count} positions")),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(35),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
            Constraint::Percentage(20),
            Constraint::Percentage(21),
        ],
    )
    .header(
        Row::new(vec!["Name", "Base", "Method", "Inception", "Description"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::NONE));

    f.render_widget(table, chunks[0]);

    let help =
        Paragraph::new("↑↓ select · Enter open · q quit").style(Style::default().fg(Color::Gray));
    f.render_widget(help, chunks[1]);
}

#[allow(clippy::too_many_lines)]
fn render_dashboard(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Pre-compute all data from app before any mutable borrow.
    let (
        title,
        base,
        total,
        time_as_of,
        time_label,
        time_gauge,
        rows,
        cash_rows,
        pnl_rows,
        exposure_rows,
    ) = {
        let seed = app.current_seed();
        let base = seed.portfolio.base_currency;
        let total = app.compute_valuation(base);
        let title = format!(
            " {} — {} ",
            seed.portfolio.name, seed.portfolio.base_currency
        );
        let as_of = seed.as_of;
        let state = app.display_state();
        let rows = position_rows(
            state,
            &seed.instruments,
            &seed.prices,
            &seed.fx,
            base,
            as_of,
        );

        // Cash rows
        let cash_rows: Vec<Row> = {
            let mut currencies: Vec<_> = state.cash().keys().copied().collect();
            currencies.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            currencies
                .into_iter()
                .map(|c| {
                    let bal = state.cash_balance(c);
                    Row::new(vec![
                        Cell::from(c.to_string()),
                        Cell::from(format!("{}{}", currency_sym(c), format_money(bal))),
                    ])
                })
                .collect()
        };

        // PnL rows
        let pnl_rows: Vec<Row> = {
            let mut currencies: Vec<_> = state.realized_pnl().keys().copied().collect();
            currencies.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            currencies
                .into_iter()
                .map(|c| {
                    let pnl = state.realized_pnl_in(c);
                    let color = if pnl >= Decimal::ZERO {
                        Color::Green
                    } else {
                        Color::Red
                    };
                    Row::new(vec![
                        Cell::from(c.to_string()),
                        Cell::from(format!("{}{}", currency_sym(c), format_money(pnl)))
                            .style(Style::default().fg(color)),
                    ])
                })
                .collect()
        };

        // Exposure rows (for text bar chart)
        let exposure_rows: Vec<Row<'static>> = {
            let exposure = currency_exposure(state, &seed.prices, seed.as_of);
            // Convert each to base currency for relative bar sizing
            let base_values: Vec<(Currency, Decimal, Decimal)> = exposure
                .iter()
                .map(|(c, v)| {
                    let base_v = if *c == base {
                        *v
                    } else {
                        seed.fx
                            .rate(*c, base, seed.as_of)
                            .map_or(Decimal::ZERO, |r| *v * r)
                    };
                    (*c, *v, base_v)
                })
                .collect();
            let max_base = base_values
                .iter()
                .map(|(_, _, bv)| bv.abs())
                .max()
                .unwrap_or(Decimal::ONE);

            base_values
                .into_iter()
                .map(|(c, native, base_v)| {
                    let color = if native >= Decimal::ZERO {
                        Color::Green
                    } else {
                        Color::Red
                    };
                    let bar = text_bar(base_v.abs(), max_base, 18);
                    Row::new(vec![
                        Cell::from(c.to_string()),
                        Cell::from(format!("{}{}", currency_sym(c), format_money(native)))
                            .style(Style::default().fg(color)),
                        Cell::from(bar).style(Style::default().fg(color)),
                    ])
                })
                .collect()
        };

        (
            title,
            base,
            total,
            app.time_machine_as_of(),
            app.time_machine_label(),
            app.time_machine_gauge(),
            rows,
            cash_rows,
            pnl_rows,
            exposure_rows,
        )
    };

    let main_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = main_block.inner(area);
    f.render_widget(main_block, area);

    // Layout: header, positions, [cash | bottom-right], help
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(4), // header (4 lines for time machine)
            Constraint::Min(6),    // positions table
            Constraint::Length(7), // bottom panels
            Constraint::Length(1), // help
        ])
        .split(inner);

    // ── Header ──
    let header_text = Text::from(vec![
        Line::from(vec![
            Span::styled("Base Currency: ", Style::default().fg(Color::Gray)),
            Span::styled(base.to_string(), Style::default().fg(Color::White)),
            Span::raw("   "),
            Span::styled("As of: ", Style::default().fg(Color::Gray)),
            Span::styled(time_as_of, Style::default().fg(Color::White)),
            Span::raw("   "),
            Span::styled("Total AUM: ", Style::default().fg(Color::Gray)),
            Span::styled(
                money_str(total),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Time Machine: ", Style::default().fg(Color::Gray)),
            Span::styled(time_label, Style::default().fg(Color::Yellow)),
            Span::raw("  "),
            Span::styled(time_gauge, Style::default().fg(Color::Yellow)),
            Span::raw("  [ / ] step · \\ live · r reset"),
        ]),
    ]);
    let header = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(header, chunks[0]);

    // ── Positions Table ──
    let pos_table = Table::new(
        rows,
        [
            Constraint::Percentage(12),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
            Constraint::Percentage(16),
            Constraint::Percentage(16),
            Constraint::Percentage(18),
            Constraint::Percentage(18),
        ],
    )
    .header(
        Row::new(vec![
            "Symbol",
            "Side",
            "Qty",
            "Avg Cost",
            "Mkt Price",
            "Base Value",
            "Unrealized",
        ])
        .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ")
    .block(Block::default().title(" Positions ").borders(Borders::ALL));

    f.render_stateful_widget(pos_table, chunks[1], &mut app.dash_table_state);

    // ── Bottom split ──
    let bottom_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);

    // Cash (left side, always)
    let cash_table = Table::new(
        cash_rows,
        [Constraint::Percentage(30), Constraint::Percentage(70)],
    )
    .header(
        Row::new(vec!["Currency", "Balance"]).style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().title(" Cash ").borders(Borders::ALL));
    f.render_widget(cash_table, bottom_chunks[0]);

    // Right side: Realized PnL OR Currency Exposure
    if app.show_currency_exposure {
        let exp_table = Table::new(
            exposure_rows,
            [
                Constraint::Percentage(18),
                Constraint::Percentage(32),
                Constraint::Percentage(50),
            ],
        )
        .header(
            Row::new(vec!["CCY", "Exposure", "Bar"])
                .style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .block(
            Block::default()
                .title(" Currency Exposure ")
                .borders(Borders::ALL),
        );
        f.render_widget(exp_table, bottom_chunks[1]);
    } else {
        let pnl_table = Table::new(
            pnl_rows,
            [Constraint::Percentage(30), Constraint::Percentage(70)],
        )
        .header(
            Row::new(vec!["Currency", "Realized PnL"])
                .style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .block(
            Block::default()
                .title(" Realized PnL ")
                .borders(Borders::ALL),
        );
        f.render_widget(pnl_table, bottom_chunks[1]);
    }

    // ── Help ──
    let help = Paragraph::new(
        "↑↓ select · t ledger · i inspect · v revalue · c toggle · [ ] replay · b back · q quit",
    )
    .style(Style::default().fg(Color::Gray));
    f.render_widget(help, chunks[3]);

    // ── Popups ──
    if let Some(ref popup) = app.popup {
        match popup {
            Popup::Valuation { base, value } => {
                render_valuation_popup(f, app, *base, *value);
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
fn render_lot_inspector(f: &mut Frame, app: &App, id: InstrumentId) {
    let area = f.area();

    let seed = app.current_seed();
    let Some(inst) = instrument_by_id(id, &seed.instruments) else {
        return;
    };

    let pos = app.display_state().position(id);

    let title = format!(" Lot Inspector — {} ", inst.symbol);
    let main_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = main_block.inner(area);
    f.render_widget(main_block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(4), // metadata
            Constraint::Min(6),    // lots table
            Constraint::Length(3), // summary
            Constraint::Length(1), // help
        ])
        .split(inner);

    // Metadata
    let meta_text = Text::from(vec![
        Line::from(vec![
            Span::styled("Name:     ", Style::default().fg(Color::Gray)),
            Span::raw(&inst.name),
        ]),
        Line::from(vec![
            Span::styled("Currency: ", Style::default().fg(Color::Gray)),
            Span::raw(inst.currency.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Kind:     ", Style::default().fg(Color::Gray)),
            Span::raw("Equity"),
        ]),
    ]);
    let meta = Paragraph::new(meta_text).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(meta, chunks[0]);

    // Lots table
    let mut lot_rows: Vec<Row> = Vec::new();
    let mut total_long = Decimal::ZERO;
    let mut total_short = Decimal::ZERO;
    let mut total_unrealized = Decimal::ZERO;

    if let Some(pos) = pos {
        let price = seed
            .prices
            .price(pos.instrument(), seed.as_of)
            .unwrap_or(Money::new(Decimal::ZERO, pos.currency()));

        for (idx, lot) in pos.lots().iter().enumerate() {
            let side = match lot.side() {
                LotSide::Long => "Long",
                LotSide::Short => "Short",
            };
            let side_color = match lot.side() {
                LotSide::Long => Color::Green,
                LotSide::Short => Color::Red,
            };

            let days = days_between(lot.open_date(), seed.as_of);
            let unrealized = if lot.side() == LotSide::Long {
                (price.amount - lot.basis_per_unit().amount) * lot.quantity()
            } else {
                (lot.basis_per_unit().amount - price.amount) * lot.quantity()
            };

            if lot.side() == LotSide::Long {
                total_long += lot.quantity();
            } else {
                total_short += lot.quantity();
            }
            total_unrealized += unrealized;

            lot_rows.push(Row::new(vec![
                Cell::from(format!("#{idx}")),
                Cell::from(side.to_string()).style(Style::default().fg(side_color)),
                Cell::from(format_decimal(lot.quantity())),
                Cell::from(money_str(lot.basis_per_unit())),
                Cell::from(lot.open_date().to_string()),
                Cell::from(format!("{days}d")),
                Cell::from(money_str(Money::new(unrealized, pos.currency()))),
            ]));
        }
    }

    let lots_table = Table::new(
        lot_rows,
        [
            Constraint::Percentage(8),
            Constraint::Percentage(10),
            Constraint::Percentage(12),
            Constraint::Percentage(18),
            Constraint::Percentage(16),
            Constraint::Percentage(10),
            Constraint::Percentage(26),
        ],
    )
    .header(
        Row::new(vec![
            "#",
            "Side",
            "Qty",
            "Basis",
            "Open Date",
            "Age",
            "Unrealized",
        ])
        .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::NONE));

    f.render_widget(lots_table, chunks[1]);

    // Summary
    let net = total_long - total_short;
    let summary_text = Text::from(vec![Line::from(vec![
        Span::styled("Total Long:  ", Style::default().fg(Color::Gray)),
        Span::styled(
            format_decimal(total_long),
            Style::default().fg(Color::Green),
        ),
        Span::raw("   "),
        Span::styled("Total Short: ", Style::default().fg(Color::Gray)),
        Span::styled(format_decimal(total_short), Style::default().fg(Color::Red)),
        Span::raw("   "),
        Span::styled("Net: ", Style::default().fg(Color::Gray)),
        Span::styled(
            format_decimal(net),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled("Unrealized: ", Style::default().fg(Color::Gray)),
        Span::styled(
            money_str(Money::new(total_unrealized, inst.currency)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ])]);
    let summary = Paragraph::new(summary_text).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(summary, chunks[2]);

    let help = Paragraph::new("b back · q quit").style(Style::default().fg(Color::Gray));
    f.render_widget(help, chunks[3]);
}

#[allow(clippy::too_many_lines)]
fn render_analytics(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let report = app.compute_var_report().clone();
    let seed = app.current_seed();

    let block = Block::default()
        .title(" Risk Analytics (VaR / CVaR) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(6),    // VaR table
            Constraint::Length(1), // divider
            Constraint::Min(6),    // Asset risk table
            Constraint::Length(1), // help
        ])
        .split(inner);

    // Header
    let header_text = Text::from(vec![
        Line::from(vec![
            Span::styled("Base Currency: ", Style::default().fg(Color::Gray)),
            Span::styled(
                seed.portfolio.base_currency.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled("Method: ", Style::default().fg(Color::Gray)),
            Span::styled("Monte Carlo", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled("Simulations: ", Style::default().fg(Color::Gray)),
            Span::styled("10,000", Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("Horizon: ", Style::default().fg(Color::Gray)),
            Span::styled("1d / 20d", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled("Lookback: ", Style::default().fg(Color::Gray)),
            Span::styled("252d", Style::default().add_modifier(Modifier::BOLD)),
        ]),
    ]);
    let header = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(header, chunks[0]);

    // VaR summary table
    let mut var_rows: Vec<Row> = Vec::new();
    for entry in &report.entries {
        let conf_str = format!("{:.0}%", entry.confidence * Decimal::from(100));
        let horizon_str = format!("{}", entry.horizon_days);
        let var_str = money_str(entry.portfolio_var);
        let cvar_str = money_str(entry.portfolio_cvar);
        var_rows.push(Row::new(vec![
            Cell::from(conf_str),
            Cell::from(horizon_str),
            Cell::from(var_str),
            Cell::from(cvar_str),
        ]));
    }

    let var_table = Table::new(
        var_rows,
        [
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
        ],
    )
    .header(
        Row::new(vec!["Confidence", "Horizon", "VaR", "CVaR"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::NONE));

    f.render_widget(var_table, chunks[1]);

    // Asset risk table
    let mut asset_rows: Vec<Row> = Vec::new();
    for ar in &report.per_asset {
        let symbol =
            instrument_by_id(ar.instrument, &seed.instruments).map_or("?", |i| i.symbol.as_str());
        let weight = format!("{:.1}%", ar.weight * Decimal::from(100));
        let stand_var = money_str(ar.standalone_var);
        let comp_cvar = money_str(ar.component_cvar);
        asset_rows.push(Row::new(vec![
            Cell::from(symbol),
            Cell::from(weight),
            Cell::from(stand_var),
            Cell::from(comp_cvar),
        ]));
    }

    let asset_table = Table::new(
        asset_rows,
        [
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
        ],
    )
    .header(
        Row::new(vec!["Asset", "Weight", "Standalone VaR", "Component CVaR"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::NONE));

    f.render_widget(asset_table, chunks[3]);

    let help = Paragraph::new("b back · q quit").style(Style::default().fg(Color::Gray));
    f.render_widget(help, chunks[4]);
}

#[allow(clippy::too_many_lines)]
fn render_ledger(f: &mut Frame, app: &App) {
    let area = f.area();
    let seed = app.current_seed();

    let block = Block::default()
        .title(" Transaction Ledger ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    let rows: Vec<Row> = seed
        .transactions
        .iter()
        .enumerate()
        .skip(app.ledger_scroll)
        .map(|(idx, tx)| {
            let color = tx_color(&tx.kind);
            let label = tx_label(&tx.kind);
            // Dim transactions not yet included in time machine
            let row_style = if idx < app.time_index {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let (sym, qty, price, fees) = match &tx.kind {
                TransactionKind::Buy {
                    instrument,
                    quantity,
                    price,
                    fees,
                    ..
                }
                | TransactionKind::Sell {
                    instrument,
                    quantity,
                    price,
                    fees,
                    ..
                } => {
                    let sym = instrument_by_id(*instrument, &seed.instruments)
                        .map_or("?", |i| i.symbol.as_str());
                    (
                        sym,
                        format_decimal(*quantity),
                        money_str(*price),
                        money_str(*fees),
                    )
                }
                TransactionKind::Deposit { amount }
                | TransactionKind::Withdrawal { amount }
                | TransactionKind::Fee { amount, .. } => {
                    ("-", "-".into(), money_str(*amount), "-".into())
                }
                TransactionKind::Dividend {
                    instrument, amount, ..
                } => {
                    let sym = instrument_by_id(*instrument, &seed.instruments)
                        .map_or("?", |i| i.symbol.as_str());
                    (sym, "-".into(), money_str(*amount), "-".into())
                }
                TransactionKind::CorporateAction(ca) => match ca {
                    ptf_engine::CorporateAction::Split { instrument, ratio } => {
                        let sym = instrument_by_id(*instrument, &seed.instruments)
                            .map_or("?", |i| i.symbol.as_str());
                        (sym, format!("{ratio}:1"), "-".into(), "-".into())
                    }
                    ptf_engine::CorporateAction::ReverseSplit { instrument, ratio } => {
                        let sym = instrument_by_id(*instrument, &seed.instruments)
                            .map_or("?", |i| i.symbol.as_str());
                        (sym, format!("1:{ratio}"), "-".into(), "-".into())
                    }
                    _ => ("?", "-".into(), "-".into(), "-".into()),
                },
            };

            Row::new(vec![
                Cell::from(tx.trade_date.to_string()),
                Cell::from(label).style(Style::default().fg(color)),
                Cell::from(sym),
                Cell::from(qty),
                Cell::from(price),
                Cell::from(fees),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(14),
            Constraint::Percentage(14),
            Constraint::Percentage(12),
            Constraint::Percentage(14),
            Constraint::Percentage(22),
            Constraint::Percentage(24),
        ],
    )
    .header(
        Row::new(vec![
            "Date",
            "Kind",
            "Symbol",
            "Qty",
            "Price / Amount",
            "Fees",
        ])
        .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::NONE));

    f.render_widget(table, chunks[0]);

    let help =
        Paragraph::new("↑↓ scroll · b back · q quit").style(Style::default().fg(Color::Gray));
    f.render_widget(help, chunks[1]);
}

fn render_valuation_popup(f: &mut Frame, app: &App, base: Currency, value: Money) {
    let area = f.area();
    let popup_area = centered_rect(45, 30, area);

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Portfolio Valuation ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    let header = Paragraph::new(Text::from(vec![
        Line::from(vec![
            Span::styled("Base Currency: ", Style::default().fg(Color::Gray)),
            Span::styled(
                base.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Total Value:   ", Style::default().fg(Color::Gray)),
            Span::styled(
                money_str(value),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ]));
    f.render_widget(header, chunks[0]);

    let mut rate_lines = vec![Line::from(Span::styled(
        "FX Rates Used:",
        Style::default().add_modifier(Modifier::BOLD),
    ))];

    let seed = app.current_seed();
    let state = app.display_state();
    let needed: Vec<_> = state
        .currencies()
        .chain(state.positions().values().map(Position::currency))
        .collect();

    let mut visited = std::collections::HashSet::new();
    for curr in needed {
        if curr == base {
            continue;
        }
        let pair = (curr, base);
        if !visited.insert(pair) {
            continue;
        }
        match seed.fx.rate(curr, base, seed.as_of) {
            Ok(rate) => {
                rate_lines.push(Line::from(format!(
                    "  {curr} → {base} = {}",
                    format_decimal(rate)
                )));
            }
            Err(e) => {
                rate_lines.push(Line::from(format!("  {curr} → {base} = error: {e}")));
            }
        }
    }

    let rates = Paragraph::new(Text::from(rate_lines));
    f.render_widget(rates, chunks[1]);

    let help =
        Paragraph::new("v cycle currency · Esc close").style(Style::default().fg(Color::Gray));
    f.render_widget(help, chunks[2]);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// ── Event Handling ──────────────────────────────────────────────────────

fn handle_events(app: &mut App) -> io::Result<()> {
    if event::poll(Duration::from_millis(50))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match app.screen {
                    Screen::Picker => handle_picker_keys(app, key.code),
                    Screen::Dashboard => handle_dashboard_keys(app, key.code),
                    Screen::Ledger => handle_ledger_keys(app, key.code),
                    Screen::LotInspector(_) => handle_lot_inspector_keys(app, key.code),
                    Screen::Analytics => handle_analytics_keys(app, key.code),
                }
            }
        }
    }
    Ok(())
}

fn handle_picker_keys(app: &mut App, code: KeyCode) {
    let max = app.seeds.len().saturating_sub(1);
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.picker_selected = app.picker_selected.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.picker_selected = (app.picker_selected + 1).min(max);
        }
        KeyCode::Enter => {
            app.current = app.picker_selected;
            app.recompute_time_states();
            app.screen = Screen::Dashboard;
        }
        KeyCode::Char('q' | 'Q') => app.should_quit = true,
        _ => {}
    }
}

fn handle_dashboard_keys(app: &mut App, code: KeyCode) {
    if let Some(ref popup) = app.popup {
        match popup {
            Popup::Valuation { .. } => match code {
                KeyCode::Esc => app.popup = None,
                KeyCode::Char('v' | 'V') => {
                    app.next_valuation_base();
                    let base = app.current_valuation_base();
                    let value = app.compute_valuation(base);
                    app.popup = Some(Popup::Valuation { base, value });
                }
                _ => {}
            },
        }
        if code == KeyCode::Char('q') || code == KeyCode::Char('Q') {
            app.should_quit = true;
        }
        return;
    }

    let seed = app.current_seed();
    let state = app.display_state();
    let pos_count = state.positions().len();
    let tx_count = seed.transactions.len();

    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            let sel = app.dash_table_state.selected().unwrap_or(0);
            let new_sel = sel.saturating_sub(1);
            app.dash_table_state.select(Some(new_sel));
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let sel = app.dash_table_state.selected().unwrap_or(0);
            let new_sel = (sel + 1).min(pos_count.saturating_sub(1));
            app.dash_table_state.select(Some(new_sel));
        }
        KeyCode::Char('t' | 'T') => {
            app.screen = Screen::Ledger;
        }
        KeyCode::Char('i' | 'I') => {
            let sel = app.dash_table_state.selected().unwrap_or(0);
            let mut positions: Vec<_> = state.positions().values().collect();
            positions.sort_by(|a, b| {
                let sa = position_symbol(a, &seed.instruments).unwrap_or("ZZZ");
                let sb = position_symbol(b, &seed.instruments).unwrap_or("ZZZ");
                sa.cmp(sb)
            });
            if let Some(pos) = positions.get(sel) {
                app.screen = Screen::LotInspector(pos.instrument());
            }
        }
        KeyCode::Char('v' | 'V') => {
            let base = app.current_valuation_base();
            let value = app.compute_valuation(base);
            app.popup = Some(Popup::Valuation { base, value });
        }
        KeyCode::Char('c' | 'C') => {
            app.show_currency_exposure = !app.show_currency_exposure;
        }
        KeyCode::Char('a' | 'A') => {
            app.compute_var_report();
            app.screen = Screen::Analytics;
        }
        // Time machine controls
        KeyCode::Char('[') => {
            app.time_index = app.time_index.saturating_sub(1);
        }
        KeyCode::Char(']') => {
            app.time_index = (app.time_index + 1).min(tx_count);
        }
        KeyCode::Char('\\' | 'r' | 'R') => {
            app.time_index = tx_count;
        }
        KeyCode::Char('b' | 'B') | KeyCode::Esc => {
            app.screen = Screen::Picker;
        }
        KeyCode::Char('q' | 'Q') => app.should_quit = true,
        _ => {}
    }
}

fn handle_ledger_keys(app: &mut App, code: KeyCode) {
    let seed = app.current_seed();
    let max_scroll = seed.transactions.len().saturating_sub(1);
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.ledger_scroll = app.ledger_scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.ledger_scroll = (app.ledger_scroll + 1).min(max_scroll);
        }
        KeyCode::Char('b' | 'B') | KeyCode::Esc => {
            app.screen = Screen::Dashboard;
        }
        KeyCode::Char('q' | 'Q') => app.should_quit = true,
        _ => {}
    }
}

fn handle_lot_inspector_keys(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('b' | 'B') | KeyCode::Esc => {
            app.screen = Screen::Dashboard;
        }
        KeyCode::Char('q' | 'Q') => app.should_quit = true,
        _ => {}
    }
}

fn handle_analytics_keys(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('b' | 'B') | KeyCode::Esc => {
            app.screen = Screen::Dashboard;
        }
        KeyCode::Char('q' | 'Q') => app.should_quit = true,
        _ => {}
    }
}

// ── Main ────────────────────────────────────────────────────────────────

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let seeds = vec![seed_data(), seed_data_us_growth()];
    let mut app = App::new(seeds);

    loop {
        terminal.draw(|f| match app.screen {
            Screen::Picker => render_picker(f, &app),
            Screen::Dashboard => render_dashboard(f, &mut app),
            Screen::Ledger => render_ledger(f, &app),
            Screen::LotInspector(id) => render_lot_inspector(f, &app, id),
            Screen::Analytics => render_analytics(f, &mut app),
        })?;

        handle_events(&mut app)?;

        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(LeaveAlternateScreen)?;

    Ok(())
}
