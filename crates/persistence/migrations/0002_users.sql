-- users: account records. Plain reference table (not a hypertable).
-- email is normalized to lowercase at the app layer; the unique index
-- enforces case-insensitive uniqueness.
CREATE TABLE users (
    id UUID PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at DATE NOT NULL,
    updated_at DATE NOT NULL
);

-- portfolios gain an owner. Existing dev databases must be wiped
-- (make db-reset) before this applies on a populated table.
ALTER TABLE portfolios
    ADD COLUMN user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE;

CREATE INDEX idx_portfolios_user_id ON portfolios(user_id);
