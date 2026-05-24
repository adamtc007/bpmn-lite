-- v0.6 §8.1 — federated DSL bus outbox.
--
-- Numbered in the 900000 range because some domains run bus storage
-- migrations in the same database as their application migrations; SQLx uses
-- one `_sqlx_migrations` table per database.
CREATE TABLE dsl_bus.outbox (
    id                UUID PRIMARY KEY,
    target_domain     TEXT NOT NULL,
    target_endpoint   TEXT NOT NULL
                      CHECK (target_endpoint IN ('invocation', 'result')),
    payload           BYTEA NOT NULL,
    idempotency_key   UUID NOT NULL,
    execution_id      UUID,
    callout_id        UUID,
    status            TEXT NOT NULL DEFAULT 'pending'
                      CHECK (status IN ('pending', 'submitted', 'retrying', 'failed')),
    attempt_count     INT NOT NULL DEFAULT 0,
    next_attempt_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error        TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    submitted_at      TIMESTAMPTZ,
    UNIQUE (idempotency_key, target_endpoint)
);

CREATE INDEX idx_dsl_bus_outbox_pending
    ON dsl_bus.outbox(next_attempt_at) WHERE status = 'pending';
