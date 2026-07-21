-- T8: one durable authority for all externally-dispatched workflow effects.
-- Effect rows are committed with the owning workflow transition; dispatch and
-- response ingestion occur outside the instance transaction.
CREATE TABLE workflow_effects (
    tenant_id TEXT NOT NULL,
    effect_id UUID PRIMARY KEY,
    instance_id UUID NOT NULL REFERENCES process_instances(instance_id) ON DELETE CASCADE,
    schema_version SMALLINT NOT NULL DEFAULT 1 CHECK (schema_version > 0),
    kind TEXT NOT NULL CHECK (kind IN ('ffi', 'bus_invocation', 'bus_result')),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'dispatching', 'accepted', 'completed', 'dead_letter', 'failed')),
    operation TEXT NOT NULL,
    input BYTEA NOT NULL,
    idempotency_key TEXT NOT NULL,
    execution_id UUID,
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    policy_version INTEGER NOT NULL DEFAULT 1 CHECK (policy_version > 0),
    next_due_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT,
    terminal BOOLEAN NOT NULL DEFAULT FALSE,
    claim_owner TEXT,
    claim_token UUID,
    claim_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, idempotency_key),
    CHECK ((claim_owner IS NULL) = (claim_token IS NULL)),
    CHECK ((claim_owner IS NULL) = (claim_until IS NULL)),
    CHECK (NOT terminal OR state IN ('completed', 'dead_letter', 'failed'))
);

CREATE INDEX workflow_effects_dispatch_idx
    ON workflow_effects (tenant_id, state, next_due_at)
    WHERE terminal = FALSE;
CREATE INDEX workflow_effects_instance_idx
    ON workflow_effects (tenant_id, instance_id, created_at);
CREATE UNIQUE INDEX workflow_effects_execution_idx
    ON workflow_effects (tenant_id, execution_id)
    WHERE execution_id IS NOT NULL;

CREATE TABLE workflow_inbox (
    tenant_id TEXT NOT NULL,
    effect_id UUID PRIMARY KEY REFERENCES workflow_effects(effect_id) ON DELETE CASCADE,
    schema_version SMALLINT NOT NULL DEFAULT 1 CHECK (schema_version > 0),
    response BYTEA NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX workflow_inbox_tenant_received_idx
    ON workflow_inbox (tenant_id, received_at);

ALTER TABLE workflow_effects ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_effects FORCE ROW LEVEL SECURITY;
CREATE POLICY bpmn_lite_tenant_isolation ON workflow_effects
    FOR ALL
    USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

ALTER TABLE workflow_inbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_inbox FORCE ROW LEVEL SECURITY;
CREATE POLICY bpmn_lite_tenant_isolation ON workflow_inbox
    FOR ALL
    USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- Preserve existing FFI lifecycle history under the new authority. Legacy
-- invocation UUIDs predate deterministic EffectId and are retained verbatim.
INSERT INTO workflow_effects (
    tenant_id, effect_id, instance_id, kind, state, operation, input,
    idempotency_key, next_due_at, last_error, terminal, created_at, updated_at
)
SELECT
    ffi.tenant_id,
    ffi.invocation_id,
    ffi.caller_process_instance_id,
    'ffi',
    CASE ffi.outcome_kind
        WHEN 'pending' THEN 'pending'
        WHEN 'success' THEN 'completed'
        WHEN 'no_match' THEN 'completed'
        WHEN 'incident' THEN 'failed'
    END,
    ffi.caller_task_id,
    ffi.input_payload,
    ffi.invocation_id::text,
    ffi.invoked_at,
    CASE WHEN ffi.outcome_kind = 'incident'
         THEN encode(COALESCE(ffi.error_payload, ''::bytea), 'base64')
         ELSE NULL END,
    ffi.outcome_kind <> 'pending',
    ffi.invoked_at,
    ffi.invoked_at
FROM ffi_invocation_record AS ffi
JOIN process_instances AS instance
  ON instance.instance_id = ffi.caller_process_instance_id
 AND instance.tenant_id = ffi.tenant_id
ON CONFLICT (effect_id) DO NOTHING;

INSERT INTO workflow_inbox (tenant_id, effect_id, response, received_at)
SELECT
    ffi.tenant_id,
    ffi.invocation_id,
    COALESCE(ffi.output_payload, ffi.error_payload, ''::bytea),
    ffi.invoked_at
FROM ffi_invocation_record AS ffi
JOIN workflow_effects AS effect
  ON effect.effect_id = ffi.invocation_id
 AND effect.tenant_id = ffi.tenant_id
WHERE ffi.outcome_kind <> 'pending'
ON CONFLICT (effect_id) DO NOTHING;

-- Preserve in-flight bus submissions. The outbox payload is copied so the new
-- dispatcher no longer needs to join legacy transport tables after cut-over.
-- Some deployments run the bus migrations after the engine migrations, so a
-- fresh engine-only database legitimately has no dsl_bus.outbox yet.
DO $migration$
BEGIN
IF to_regclass('dsl_bus.outbox') IS NOT NULL THEN
INSERT INTO workflow_effects (
    tenant_id, effect_id, instance_id, kind, state, operation, input,
    idempotency_key, execution_id, attempt, next_due_at, terminal,
    created_at, updated_at
)
SELECT
    pending.tenant_id,
    pending.callout_id,
    pending.process_instance_id,
    'bus_invocation',
    CASE WHEN pending.execution_id IS NULL THEN 'pending' ELSE 'accepted' END,
    pending.verb_id,
    COALESCE(outbox.payload, ''::bytea),
    pending.idempotency_key::text,
    pending.execution_id,
    COALESCE(outbox.attempt_count, 0),
    COALESCE(outbox.next_attempt_at, pending.submitted_at),
    FALSE,
    pending.submitted_at,
    COALESCE(pending.ack_received_at, pending.submitted_at)
FROM bpmn_pending_invocation AS pending
JOIN process_instances AS instance
  ON instance.instance_id = pending.process_instance_id
 AND instance.tenant_id = pending.tenant_id
LEFT JOIN dsl_bus.outbox AS outbox
    ON outbox.callout_id = pending.callout_id
   AND outbox.tenant_id = pending.tenant_id
ON CONFLICT (effect_id) DO NOTHING;
END IF;
END
$migration$;

-- Explicitly destructive down path:
--   DROP TABLE workflow_inbox;
--   DROP TABLE workflow_effects;
-- Rolling back loses post-cut-over effect/inbox state, so operators must drain
-- dispatchers and export both tables before executing those statements.
