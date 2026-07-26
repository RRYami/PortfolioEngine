-- TimescaleDB: transactions is the domain's time-series event log.
CREATE EXTENSION IF NOT EXISTS timescaledb;

-- portfolios: portfolio metadata
CREATE TABLE portfolios (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    base_currency CHAR(3) NOT NULL,
    lot_method JSONB NOT NULL,
    inception_date DATE NOT NULL,
    created_at DATE NOT NULL,
    updated_at DATE NOT NULL
);

-- instruments: tradable securities
CREATE TABLE instruments (
    id UUID PRIMARY KEY,
    symbol TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    currency CHAR(3) NOT NULL,
    kind JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- transactions: immutable source of truth for portfolio state.
-- Partitioned by trade_date (the column repository queries range on).
-- Hypertable constraint: any unique index/PK must include the partition column.
CREATE TABLE transactions (
    id UUID NOT NULL,
    portfolio_id UUID NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
    trade_date DATE NOT NULL,
    settle_date DATE NOT NULL,
    kind JSONB NOT NULL,
    -- Monotonic insertion order: deterministic tiebreak for same-day trades.
    seq BIGINT GENERATED ALWAYS AS IDENTITY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, trade_date)
);

SELECT create_hypertable('transactions', 'trade_date',
       chunk_time_interval => INTERVAL '1 month');

-- chronological listing index (trade_date, then insertion order)
CREATE INDEX idx_transactions_portfolio_trade_date
ON transactions(portfolio_id, trade_date, seq);
