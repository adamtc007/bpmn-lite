-- T5: durable workflow timers. The table is the authority for timer delivery;
-- fiber wait state alone is not schedulable after a process restart.
CREATE TABLE workflow_timers (
    tenant_id TEXT NOT NULL,
    timer_id UUID PRIMARY KEY,
    instance_id UUID NOT NULL REFERENCES process_instances(instance_id) ON DELETE CASCADE,
    fiber_id UUID NOT NULL,
    due_at TIMESTAMPTZ NOT NULL,
    kind JSONB NOT NULL,
    repeat_spec JSONB,
    state TEXT NOT NULL DEFAULT 'armed' CHECK (state IN ('armed', 'consumed', 'cancelled')),
    claim_owner TEXT,
    claim_token UUID,
    claim_until TIMESTAMPTZ,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((claim_owner IS NULL) = (claim_token IS NULL)),
    CHECK ((claim_owner IS NULL) = (claim_until IS NULL))
);

CREATE INDEX workflow_timers_due_idx
    ON workflow_timers (tenant_id, state, due_at);
CREATE INDEX workflow_timers_instance_idx
    ON workflow_timers (tenant_id, instance_id, state);

ALTER TABLE workflow_timers ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_timers FORCE ROW LEVEL SECURITY;

CREATE POLICY bpmn_lite_tenant_isolation ON workflow_timers
    FOR ALL
    USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- Down path (destructive by definition): DROP TABLE workflow_timers;
-- A rollback discards outstanding deadlines, so operators must drain or export
-- armed rows before applying that down statement.
