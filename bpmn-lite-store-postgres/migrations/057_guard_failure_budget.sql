-- V&S §15 (v0.7) ruling F: a store-side, per-guard repeated-failure
-- budget. Keyed (tenant_id, instance_id, guard_addr) because a guard's
-- RecordId does not survive its own re-open (the record retires with the
-- rollback cascade) but its static bytecode address does. This is
-- accounting *about* execution (like the lease and the fence), never
-- consulted by the pure kernel and never in the Ring 2 hash domain —
-- exhausting the budget quarantines the instance via the existing
-- quarantine_state mechanism (T10.3's claim gate already refuses claims
-- for any quarantined instance; no new claim-path code needed).
CREATE TABLE guard_failure_budget (
    tenant_id TEXT NOT NULL,
    instance_id UUID NOT NULL REFERENCES workflow_instances(instance_id) ON DELETE CASCADE,
    guard_addr INTEGER NOT NULL,
    failure_count INTEGER NOT NULL DEFAULT 0 CHECK (failure_count >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, instance_id, guard_addr)
);

ALTER TABLE guard_failure_budget ENABLE ROW LEVEL SECURITY;
ALTER TABLE guard_failure_budget FORCE ROW LEVEL SECURITY;
CREATE POLICY bpmn_lite_tenant_isolation ON guard_failure_budget
    FOR ALL
    USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));
