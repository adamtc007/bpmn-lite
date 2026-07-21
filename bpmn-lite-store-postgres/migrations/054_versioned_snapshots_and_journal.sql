-- T10: versioned durable snapshots and an append-only deterministic journal.
-- Existing rows intentionally remain NULL and are rejected by the T10 recovery
-- readiness gate until explicitly upgraded or quarantined.
ALTER TABLE workflow_instances
    ADD COLUMN snapshot_schema_version SMALLINT,
    ADD COLUMN artifact_abi INTEGER,
    ADD COLUMN snapshot_envelope BYTEA;

CREATE TABLE workflow_journal (
    tenant_id TEXT NOT NULL,
    instance_id UUID NOT NULL REFERENCES workflow_instances(instance_id) ON DELETE CASCADE,
    schema_version SMALLINT NOT NULL CHECK (schema_version > 0),
    command_schema_version SMALLINT NOT NULL CHECK (command_schema_version > 0),
    command_id UUID NOT NULL,
    command_type TEXT NOT NULL,
    logical_time BIGINT NOT NULL,
    prior_revision BIGINT NOT NULL CHECK (prior_revision >= -1),
    new_revision BIGINT NOT NULL CHECK (new_revision = prior_revision + 1),
    artifact_hash BYTEA NOT NULL CHECK (octet_length(artifact_hash) = 32),
    state_hash BYTEA NOT NULL CHECK (octet_length(state_hash) = 32),
    record_envelope BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, instance_id, new_revision),
    UNIQUE (tenant_id, instance_id, command_id)
);

CREATE INDEX workflow_journal_replay_idx
    ON workflow_journal (tenant_id, instance_id, new_revision);

ALTER TABLE workflow_journal ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_journal FORCE ROW LEVEL SECURITY;
CREATE POLICY bpmn_lite_tenant_isolation ON workflow_journal
    FOR ALL
    USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- Explicitly irreversible without data loss:
--   DROP TABLE workflow_journal;
--   ALTER TABLE workflow_instances DROP COLUMN snapshot_envelope,
--     DROP COLUMN artifact_abi, DROP COLUMN snapshot_schema_version;
-- A down migration destroys the authoritative replay chain and is therefore
-- permitted only after an operator export of both snapshots and journal rows.
