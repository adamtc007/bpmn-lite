-- Migration 030: tenant_pools table
--
-- Each pool represents a pool of bpmn-lite worker pods.  The default pool
-- is always present; dedicated pools are provisioned by bpmn-controller.

CREATE TABLE IF NOT EXISTS tenant_pools (
    pool_id      TEXT        NOT NULL,
    pool_type    TEXT        NOT NULL DEFAULT 'default',  -- 'default' | 'dedicated'
    description  TEXT,
    paused       BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT tenant_pools_pk          PRIMARY KEY (pool_id),
    CONSTRAINT tenant_pools_type_check  CHECK (pool_type IN ('default', 'dedicated'))
);
