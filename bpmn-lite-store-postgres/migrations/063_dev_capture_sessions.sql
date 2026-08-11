-- G1.2 (EOP-PLAN-BPMN-DESIGN-003 v0.3): dev-capture session durability.
-- Replaces an in-memory-only `Mutex<HashMap>` (bpmn-lite-server-designer's
-- `DesignerState.dev_capture`) that silently lost every captured
-- interaction on process restart -- DIR-004 Phase 1's "train-on-able, not
-- hash-only" guarantee is worthless if the records evaporate before they
-- can ever be exported for training. Session-id-scoped, not tenant-scoped
-- (dev capture is Adam-only, not a tenant-facing feature). Mirrors
-- design_sessions/design_session_events' append-only, composite-PK shape.

CREATE TABLE IF NOT EXISTS dev_capture_sessions (
    session_id TEXT PRIMARY KEY,
    consent_statement_timestamp TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS dev_capture_records (
    session_id TEXT NOT NULL REFERENCES dev_capture_sessions(session_id) ON DELETE CASCADE,
    seq BIGINT NOT NULL,
    record JSONB NOT NULL,
    at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (session_id, seq)
);
