-- T9.2: versioned, tenant-qualified start idempotency belongs to the canonical
-- process_instances aggregate. Down migration is intentionally destructive:
-- dropping the added lineage columns loses start-command audit data.

ALTER TABLE process_instances
    ADD CONSTRAINT process_instances_tenant_instance_unique
    UNIQUE (tenant_id, instance_id);

ALTER TABLE bpmn_spawn_idempotency
    DROP CONSTRAINT bpmn_spawn_idempotency_instance_id_fkey,
    DROP CONSTRAINT bpmn_spawn_idempotency_pkey,
    ADD COLUMN schema_version SMALLINT NOT NULL DEFAULT 1,
    ADD COLUMN artifact_hash BYTEA,
    ADD COLUMN entry_id UUID,
    ADD COLUMN runbook_id UUID,
    ADD COLUMN initial_payload_hash BYTEA,
    ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();

UPDATE bpmn_spawn_idempotency AS starts
SET artifact_hash = instances.bytecode_version,
    entry_id = instances.entry_id,
    runbook_id = instances.runbook_id,
    initial_payload_hash = instances.domain_payload_hash
FROM process_instances AS instances
WHERE starts.tenant_id = instances.tenant_id
  AND starts.instance_id = instances.instance_id;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM bpmn_spawn_idempotency
        WHERE artifact_hash IS NULL
           OR entry_id IS NULL
           OR runbook_id IS NULL
           OR initial_payload_hash IS NULL
    ) THEN
        RAISE EXCEPTION 'T9 start lineage backfill found a row without a canonical instance';
    END IF;
END $$;

ALTER TABLE bpmn_spawn_idempotency
    ALTER COLUMN artifact_hash SET NOT NULL,
    ALTER COLUMN entry_id SET NOT NULL,
    ALTER COLUMN runbook_id SET NOT NULL,
    ALTER COLUMN initial_payload_hash SET NOT NULL,
    ADD CONSTRAINT bpmn_spawn_idempotency_pkey PRIMARY KEY (tenant_id, idempotency_key),
    ADD CONSTRAINT bpmn_spawn_idempotency_canonical_instance_fkey
        FOREIGN KEY (tenant_id, instance_id)
        REFERENCES process_instances(tenant_id, instance_id)
        ON DELETE CASCADE;

