-- Phase 3A (F-01 remediation scaffolding,
-- docs/todo/zed_agent_execution_lease_remediation_plan.md Phase 3): the
-- durable ready-activation table. This migration adds the schema and
-- store primitives ONLY — nothing in the engine enqueues to or consumes
-- from this table yet (see docs/todo/PHASE3-durable-activation-queue.md).
-- The existing `claim_running_instances` population-scan dispatch path
-- is untouched and remains the live scheduler until a later phase wires
-- producers, dual-write-verifies in shadow, and cuts over.
CREATE TABLE workflow_activations (
    tenant_id TEXT NOT NULL,
    activation_id UUID PRIMARY KEY,
    instance_id UUID NOT NULL REFERENCES workflow_instances(instance_id) ON DELETE CASCADE,
    -- Stable dedupe identity for the durable command that produced this
    -- activation (a timer firing, a message delivery, a job result...).
    -- Enqueue is idempotent on (tenant_id, command_id): the same command
    -- reported ready twice inserts once.
    command_id UUID NOT NULL,
    command_kind TEXT NOT NULL,
    command_envelope JSONB NOT NULL,
    -- Base revision the activation was enqueued against, where known —
    -- diagnostic only in 3A (no consumer yet checks it for staleness).
    base_revision BIGINT,
    status TEXT NOT NULL DEFAULT 'ready'
        CHECK (status IN ('ready', 'claimed', 'completed', 'cancelled', 'dead_lettered')),
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    priority INTEGER NOT NULL DEFAULT 0,
    -- Deterministic tie-break ordering within a priority band.
    seq BIGSERIAL,
    claim_owner TEXT,
    claim_token TEXT,
    claim_expires_at TIMESTAMPTZ,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    last_error TEXT,
    CHECK ((claim_owner IS NULL) = (claim_token IS NULL)),
    CHECK ((claim_owner IS NULL) = (claim_expires_at IS NULL)),
    UNIQUE (tenant_id, command_id)
);

-- Ready-selection: the scan a future scheduler would run instead of
-- `claim_running_instances`'s full-population sweep.
CREATE INDEX workflow_activations_ready_idx
    ON workflow_activations (tenant_id, priority, available_at, seq)
    WHERE status = 'ready';

CREATE INDEX workflow_activations_instance_idx
    ON workflow_activations (tenant_id, instance_id, status);

-- I-8 (per-instance serialisation): at most one CLAIMED activation per
-- instance at a time. A partial unique index, not application logic —
-- the database enforces it even if every caller has a bug.
CREATE UNIQUE INDEX workflow_activations_one_claimed_per_instance
    ON workflow_activations (tenant_id, instance_id)
    WHERE status = 'claimed';

-- Reclaim: stale claimed rows, ordered by how overdue they are.
CREATE INDEX workflow_activations_claim_expiry_idx
    ON workflow_activations (claim_expires_at)
    WHERE status = 'claimed';

ALTER TABLE workflow_activations ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_activations FORCE ROW LEVEL SECURITY;

CREATE POLICY bpmn_lite_tenant_isolation ON workflow_activations
    FOR ALL
    USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- Down path (destructive by definition): DROP TABLE workflow_activations;
-- Nothing depends on this table yet in 3A, so a rollback is a plain drop.
