-- EOP-SAGE-REPL-BPMN-001 T1: designer-session aggregate.
-- Append-only event log (revisions carry FULL dsl source; undo is a
-- re-append, never a delete). Tenant scoping is via tenant_id WHERE
-- clauses in the store impl for now; RLS policy alignment for these two
-- tables is an explicit T5 deployment-review item (recorded, not
-- silently skipped).

CREATE TABLE IF NOT EXISTS design_sessions (
    id UUID PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('draft', 'saved')),
    template_name TEXT,
    template_version INT,
    template_plan_hash BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_design_sessions_tenant
    ON design_sessions (tenant_id);

CREATE TABLE IF NOT EXISTS design_session_events (
    session_id UUID NOT NULL REFERENCES design_sessions(id) ON DELETE CASCADE,
    seq BIGINT NOT NULL,
    payload JSONB NOT NULL,
    at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (session_id, seq)
);
