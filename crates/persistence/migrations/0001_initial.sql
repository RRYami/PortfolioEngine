-- portfolios: portfolio metadata
CREATE TABLE portfolios (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    base_currency CHAR(3) NOT NULL,
    lot_method TEXT NOT NULL,
    inception_date DATE NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- instruments: tradable securities
CREATE TABLE instruments (
    id UUID PRIMARY KEY,
    symbol TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    currency CHAR(3) NOT NULL,
    kind TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- transactions: immutable source of truth for portfolio state
CREATE TABLE transactions (
    id UUID PRIMARY KEY,
    portfolio_id UUID NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
    trade_date DATE NOT NULL,
    settle_date DATE NOT NULL,
    kind JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- chronological listing index (trade_date, then insertion order)
CREATE INDEX idx_transactions_portfolio_trade_date
ON transactions(portfolio_id, trade_date, created_at);
