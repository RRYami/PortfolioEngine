-- vol: the fitted volatility model.
--
-- Everything here is derived -- rerunning ptf-surface over the same quotes
-- reproduces it -- so it is disposable in a way market data is not. What it
-- has lacked is provenance: the builder overwrote six parquet files in place
-- and the API read whichever version happened to be on disk. There was no way
-- to say which build produced a number, no way to roll back a bad fit, and no
-- way to serve one fit while computing the next.
--
-- Every artifact now belongs to a run, and the API reads through a pointer.
CREATE SCHEMA IF NOT EXISTS vol;

-- surface_run: one execution of the fitting pipeline for one root.
CREATE TABLE vol.surface_run (
    run_id        UUID PRIMARY KEY,
    root          TEXT NOT NULL,
    -- Date the factor model was fitted as of; the sessions below are the
    -- window it was fitted over.
    as_of         DATE NOT NULL,
    first_session DATE NOT NULL,
    last_session  DATE NOT NULL,
    components    INTEGER NOT NULL,
    git_sha       TEXT,
    -- Grid definition, wing bounds, coverage thresholds: enough to rerun it.
    config        JSONB NOT NULL DEFAULT '{}'::jsonb,
    built_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT surface_run_window CHECK (first_session <= last_session)
);

CREATE INDEX idx_surface_run_root_as_of ON vol.surface_run (root, as_of DESC);

-- current_run: which run each root serves from.
--
-- A separate table rather than a flag on surface_run so publishing is a single
-- row update: build a new run alongside the live one, point at it when it
-- validates, point back if it does not. A flag would need two writes and has a
-- window where a root has no current run or two.
CREATE TABLE vol.current_run (
    root      TEXT PRIMARY KEY,
    run_id    UUID NOT NULL REFERENCES vol.surface_run(run_id),
    -- Deliberately not ON DELETE CASCADE: dropping the run a root is serving
    -- should fail loudly rather than silently leave it unserved.
    promoted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- iv_quote: per-contract implied vol, the input to the smile fits.
--
-- Kept rather than discarded after fitting: when a slice looks wrong this is
-- the only way to see whether the quotes or the calibration caused it.
CREATE TABLE vol.iv_quote (
    run_id        UUID NOT NULL REFERENCES vol.surface_run(run_id) ON DELETE CASCADE,
    quote_date    DATE NOT NULL,
    root          TEXT NOT NULL,
    expiry        DATE NOT NULL,
    opt_right     CHAR(1) NOT NULL,
    strike        DOUBLE PRECISION NOT NULL,
    mid           DOUBLE PRECISION,
    tte           DOUBLE PRECISION,
    log_moneyness DOUBLE PRECISION,
    iv            DOUBLE PRECISION,
    vega          DOUBLE PRECISION,
    forward       DOUBLE PRECISION,
    rel_spread    DOUBLE PRECISION,
    size          DOUBLE PRECISION,
    -- Quote older than the snapshot it was captured in; down-weighted in the
    -- fit rather than dropped.
    stale         BOOLEAN,
    PRIMARY KEY (run_id, quote_date, expiry, opt_right, strike)
);

SELECT create_hypertable('vol.iv_quote', 'quote_date',
       chunk_time_interval => INTERVAL '3 months');

-- forward_curve: parity forward and discount factor per expiry.
CREATE TABLE vol.forward_curve (
    run_id         UUID NOT NULL REFERENCES vol.surface_run(run_id) ON DELETE CASCADE,
    quote_date     DATE NOT NULL,
    root           TEXT NOT NULL,
    expiry         DATE NOT NULL,
    tte            DOUBLE PRECISION NOT NULL,
    forward        DOUBLE PRECISION NOT NULL,
    discount       DOUBLE PRECISION NOT NULL,
    pairs_used     INTEGER,
    rmse           DOUBLE PRECISION,
    -- Curve-level fields, repeated per expiry: the discount curve is fitted
    -- globally across expiries, since per-expiry factors produced DF > 1.
    curve_rate     DOUBLE PRECISION,
    curve_expiries INTEGER,
    -- Fitted rate came out negative and was clamped at zero.
    curve_clamped  BOOLEAN,
    PRIMARY KEY (run_id, quote_date, expiry)
);

SELECT create_hypertable('vol.forward_curve', 'quote_date',
       chunk_time_interval => INTERVAL '1 year');

-- svi_slice: the calibrated smile for one expiry on one session.
CREATE TABLE vol.svi_slice (
    run_id        UUID NOT NULL REFERENCES vol.surface_run(run_id) ON DELETE CASCADE,
    quote_date    DATE NOT NULL,
    root          TEXT NOT NULL,
    expiry        DATE NOT NULL,
    tte           DOUBLE PRECISION NOT NULL,
    a             DOUBLE PRECISION NOT NULL,
    b             DOUBLE PRECISION NOT NULL,
    rho           DOUBLE PRECISION NOT NULL,
    m             DOUBLE PRECISION NOT NULL,
    sigma         DOUBLE PRECISION NOT NULL,
    rmse_vol      DOUBLE PRECISION,
    points        INTEGER,
    -- Butterfly condition at its worst point. Negative means the slice admits
    -- arbitrage; the calibration tightens wing bounds until it does not.
    min_durrleman DOUBLE PRECISION,
    k_lo          DOUBLE PRECISION,
    k_hi          DOUBLE PRECISION,
    PRIMARY KEY (run_id, quote_date, expiry),
    CONSTRAINT svi_slice_rho CHECK (rho > -1.0 AND rho < 1.0),
    CONSTRAINT svi_slice_b CHECK (b >= 0.0),
    CONSTRAINT svi_slice_sigma CHECK (sigma > 0.0)
);

SELECT create_hypertable('vol.svi_slice', 'quote_date',
       chunk_time_interval => INTERVAL '1 year');

-- grid_cell: the surface resampled onto constant maturity and standardized
-- moneyness, which is what the factor model is fitted on.
CREATE TABLE vol.grid_cell (
    run_id         UUID NOT NULL REFERENCES vol.surface_run(run_id) ON DELETE CASCADE,
    quote_date     DATE NOT NULL,
    root           TEXT NOT NULL,
    z              DOUBLE PRECISION NOT NULL,
    tte            DOUBLE PRECISION NOT NULL,
    k              DOUBLE PRECISION NOT NULL,
    total_variance DOUBLE PRECISION NOT NULL,
    vol            DOUBLE PRECISION NOT NULL,
    -- Sampled beyond the fitted strike range for that session.
    extrapolated   BOOLEAN NOT NULL,
    PRIMARY KEY (run_id, quote_date, z, tte)
);

SELECT create_hypertable('vol.grid_cell', 'quote_date',
       chunk_time_interval => INTERVAL '1 year');

-- pca_loading: one fit per run, so no session column. `explained` is the
-- variance share of the whole component and repeats across its cells.
CREATE TABLE vol.pca_loading (
    run_id    UUID NOT NULL REFERENCES vol.surface_run(run_id) ON DELETE CASCADE,
    pc        INTEGER NOT NULL,
    z         DOUBLE PRECISION NOT NULL,
    tte       DOUBLE PRECISION NOT NULL,
    loading   DOUBLE PRECISION NOT NULL,
    cell_mean DOUBLE PRECISION NOT NULL,
    cell_sd   DOUBLE PRECISION NOT NULL,
    explained DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (run_id, pc, z, tte)
);

-- pca_score: the factor time series.
--
-- Stored dated, and the risk engine joins it to spot returns on those dates.
-- The surface is only fitted on sessions with a usable chain, so this series
-- skips days the price history keeps; aligning the two by position instead
-- slid the whole vol factor against spot and lost the leverage effect.
CREATE TABLE vol.pca_score (
    run_id     UUID NOT NULL REFERENCES vol.surface_run(run_id) ON DELETE CASCADE,
    quote_date DATE NOT NULL,
    pc         INTEGER NOT NULL,
    score      DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (run_id, quote_date, pc)
);

SELECT create_hypertable('vol.pca_score', 'quote_date',
       chunk_time_interval => INTERVAL '1 year');
