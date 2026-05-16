CREATE TABLE dead_letter_queue (
    name       INTEGER NOT NULL,
    corr_key   TEXT NOT NULL,
    payload    BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (name, corr_key)
);
