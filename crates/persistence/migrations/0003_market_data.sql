-- market: vendor data, as opposed to the portfolios and users in public.
--
-- These tables are empty after this migration. Nothing reads or writes them
-- yet; the loaders move over in a later step. Creating them first keeps the
-- schema change separable from the behaviour change.
CREATE SCHEMA IF NOT EXISTS market;

-- trading_day: the session calendar, and the reason it exists.
--
-- Synthetic demo prices once leaked into the price table on a business-day
-- calendar, which skips weekends but not holidays. NVDA acquired a close on
-- Thanksgiving 31% below the previous session. Nothing rejected it, because
-- nothing knew which days the market was open: its annualised volatility read
-- 114% against a true 45%, and the return computed against that fabricated
-- close injected a +36% day into the risk factor panel.
--
-- A row here is an assertion that the venue traded. The foreign keys below
-- turn "a price on a day the market was shut" from a bug that survives into
-- live risk numbers into an insert that fails.
--
-- Single-venue by construction: everything held today trades on XNYS, and the
-- OPRA chains follow the same calendar. A second venue means a composite key
-- here and a venue column on every table that references it -- a real
-- migration, deliberately deferred rather than guessed at now.
CREATE TABLE market.trading_day (
    session_date DATE PRIMARY KEY,
    venue        TEXT NOT NULL DEFAULT 'XNYS'
);

-- Seed this from an exchange calendar, never from the price data.
--
-- Deriving it from observed dates is circular, and measurably so: on the data
-- as it stood before the holiday rows were removed, the union of every
-- symbol's sessions contained all twelve of them, so a calendar built that way
-- would have authorised exactly the rows it exists to reject. Restricting the
-- union to symbols known to be clean does produce the right 545 sessions, but
-- only because which symbols to trust was already known -- which is the
-- conclusion, not the input.
--
-- A calendar is reference data. Take it from a source that knows when the
-- exchange was open, then use it to judge the prices.
COMMENT ON TABLE market.trading_day IS
    'Sessions the venue was open. Seed from an exchange calendar (XNYS), not '
    'from observed price dates -- see the note in 0003_market_data.sql.';

-- equity_close: adjusted daily closes.
--
-- Adjusted, so the series is not stable over time: a split or dividend
-- retroactively rescales every earlier row. Refetching is cheap but does not
-- reproduce, which is why these are archived rather than treated as
-- regenerable.
CREATE TABLE market.equity_close (
    symbol       TEXT NOT NULL,
    session_date DATE NOT NULL REFERENCES market.trading_day(session_date),
    close        DOUBLE PRECISION NOT NULL,
    -- Hypertable constraint: the partition column must appear in the key.
    PRIMARY KEY (symbol, session_date)
);

SELECT create_hypertable('market.equity_close', 'session_date',
       chunk_time_interval => INTERVAL '1 year');

-- fx_rate: USD per unit of currency.
--
-- Deliberately *not* keyed to trading_day. FX trades through US equity
-- holidays, so its calendar is a superset of the equity one and a foreign key
-- would reject perfectly good rows. Lookups forward-fill from the most recent
-- rate on or before a date, so the two calendars need not agree.
CREATE TABLE market.fx_rate (
    ccy          CHAR(3) NOT NULL,
    rate_date    DATE NOT NULL,
    usd_per_unit DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (ccy, rate_date)
);

SELECT create_hypertable('market.fx_rate', 'rate_date',
       chunk_time_interval => INTERVAL '1 year');

-- option_quote: one minute-snapshot quote per contract per session.
--
-- The natural key matches what the ingest already upserts on. `raw_symbol` is
-- the OCC symbol and is unique per contract, but the decomposed terms are what
-- every query filters by.
CREATE TABLE market.option_quote (
    quote_date     DATE NOT NULL REFERENCES market.trading_day(session_date),
    root           TEXT NOT NULL,
    expiry         DATE NOT NULL,
    opt_right      CHAR(1) NOT NULL,
    strike         DOUBLE PRECISION NOT NULL,
    instrument_id  BIGINT,
    raw_symbol     TEXT,
    bid            DOUBLE PRECISION,
    ask            DOUBLE PRECISION,
    mid            DOUBLE PRECISION,
    rel_spread     DOUBLE PRECISION,
    tte            DOUBLE PRECISION,
    -- Receipt time, not event time: event time is null whenever the book did
    -- not update inside the snapshot minute, which is most of the chain.
    snapshot_ts    TIMESTAMPTZ,
    bid_size       INTEGER,
    ask_size       INTEGER,
    last_update_ts TIMESTAMPTZ,
    PRIMARY KEY (quote_date, root, expiry, opt_right, strike),
    CONSTRAINT option_quote_right CHECK (opt_right IN ('C', 'P'))
);

SELECT create_hypertable('market.option_quote', 'quote_date',
       chunk_time_interval => INTERVAL '3 months');

-- Chain for one root on one session: the surface builder's only access path.
CREATE INDEX idx_option_quote_root_date
ON market.option_quote (root, quote_date);

-- option_ingest_log: which sessions were fetched, and what came back.
--
-- No foreign key to trading_day, and that is the point: this table records
-- market holidays as `empty`. Without it an absent session is ambiguous --
-- never fetched, or nothing to fetch? -- and a resumed backfill reads a run of
-- holidays as a run of failures and aborts.
CREATE TABLE market.option_ingest_log (
    quote_date  DATE NOT NULL,
    root        TEXT NOT NULL,
    contracts   INTEGER,
    status      TEXT NOT NULL,
    detail      TEXT,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (quote_date, root)
);
