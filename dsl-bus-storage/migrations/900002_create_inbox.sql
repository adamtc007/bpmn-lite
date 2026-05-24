-- v0.6 §8.2 — federated DSL bus inbox.
--
-- Numbered in the 900000 range because some domains run bus storage
-- migrations in the same database as their application migrations; SQLx uses
-- one `_sqlx_migrations` table per database.
CREATE TABLE dsl_bus.inbox (
    idempotency_key   UUID PRIMARY KEY,
    source_domain     TEXT NOT NULL,
    endpoint          TEXT NOT NULL
                      CHECK (endpoint IN ('invocation', 'result')),
    execution_id      UUID,
    received_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at      TIMESTAMPTZ,
    status            TEXT NOT NULL DEFAULT 'received'
                      CHECK (status IN ('received', 'processed')),
    payload           BYTEA
);

CREATE INDEX idx_dsl_bus_inbox_source ON dsl_bus.inbox(source_domain, received_at);
