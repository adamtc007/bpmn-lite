-- T9.4: the VM aggregate is the sole writable workflow authority.
-- This migration is intentionally irreversible: the legacy outer-walker table
-- is dropped after its lineage has already been backfilled into the canonical
-- aggregate/start-command rows by migration 051.

ALTER TABLE process_instances RENAME TO workflow_instances;

CREATE TABLE workflow_migration_quarantine (
    tenant_id TEXT NOT NULL,
    instance_id UUID NOT NULL,
    schema_version SMALLINT NOT NULL DEFAULT 1,
    legacy_plan_hash BYTEA NOT NULL,
    legacy_node_id TEXT,
    reason TEXT NOT NULL,
    quarantined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, instance_id),
    FOREIGN KEY (tenant_id, instance_id)
        REFERENCES workflow_instances(tenant_id, instance_id)
        ON DELETE CASCADE
);

ALTER TABLE workflow_migration_quarantine ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_migration_quarantine FORCE ROW LEVEL SECURITY;
CREATE POLICY workflow_migration_quarantine_tenant_isolation
    ON workflow_migration_quarantine
    USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));
GRANT SELECT, INSERT, UPDATE, DELETE ON workflow_migration_quarantine TO bpmn_lite_app;

-- SQL cannot prove a legacy plan-node -> verified bytecode-PC equivalence. Such
-- rows are reported and quarantined; an offline migration tool may later clear
-- the quarantine only after comparing compiler-emitted debug metadata.
INSERT INTO workflow_migration_quarantine
    (tenant_id, instance_id, legacy_plan_hash, legacy_node_id, reason)
SELECT tenant_id, instance_id, plan_hash, current_node_id,
       'no verifier-proved legacy node to canonical PC mapping'
FROM workflow_instances
WHERE plan_hash IS NOT NULL
ON CONFLICT (tenant_id, instance_id) DO NOTHING;

UPDATE workflow_instances
SET quarantine_state = 't9_legacy_plan_requires_verified_mapping'
WHERE plan_hash IS NOT NULL;

CREATE TABLE workflow_migration_orphans (
    tenant_id TEXT NOT NULL,
    entity_kind TEXT NOT NULL,
    entity_id UUID NOT NULL,
    schema_version SMALLINT NOT NULL DEFAULT 1,
    detail JSONB NOT NULL,
    quarantined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, entity_kind, entity_id)
);

ALTER TABLE workflow_migration_orphans ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_migration_orphans FORCE ROW LEVEL SECURITY;
CREATE POLICY workflow_migration_orphans_tenant_isolation
    ON workflow_migration_orphans
    USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));
GRANT SELECT, INSERT, UPDATE, DELETE ON workflow_migration_orphans TO bpmn_lite_app;

INSERT INTO workflow_migration_orphans
    (tenant_id, entity_kind, entity_id, detail)
SELECT pending.tenant_id, 'pending_invocation', pending.callout_id,
       jsonb_build_object(
           'process_instance_id', pending.process_instance_id,
           'reason', 'pending invocation references no canonical workflow instance'
       )
FROM bpmn_pending_invocation AS pending
LEFT JOIN workflow_instances AS instance
  ON instance.tenant_id = pending.tenant_id
 AND instance.instance_id = pending.process_instance_id
WHERE instance.instance_id IS NULL
ON CONFLICT DO NOTHING;

DELETE FROM bpmn_pending_invocation AS pending
WHERE NOT EXISTS (
    SELECT 1 FROM workflow_instances AS instance
    WHERE instance.tenant_id = pending.tenant_id
      AND instance.instance_id = pending.process_instance_id
);

ALTER TABLE bpmn_pending_invocation
    ADD CONSTRAINT bpmn_pending_invocation_workflow_instance_fkey
    FOREIGN KEY (tenant_id, process_instance_id)
    REFERENCES workflow_instances(tenant_id, instance_id)
    ON DELETE CASCADE;

CREATE OR REPLACE FUNCTION resolve_instance_tenant(target_id UUID)
RETURNS TEXT LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT tenant_id FROM workflow_instances WHERE instance_id = target_id
$$;

REVOKE EXECUTE ON FUNCTION resolve_instance_tenant(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION resolve_instance_tenant(UUID) TO bpmn_lite_app;

DROP TABLE bpmn_process_instance;
