-- T11.5: durable queue/effect rows reference content-addressed payloads.
-- Existing inline columns remain as empty compatibility sentinels for one ABI
-- window; authoritative bytes live only in workflow_payloads for new writes.
CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE workflow_payloads (
    tenant_id TEXT NOT NULL,
    payload_hash BYTEA NOT NULL CHECK (octet_length(payload_hash) = 32),
    schema_version SMALLINT NOT NULL DEFAULT 1 CHECK (schema_version > 0),
    payload BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, payload_hash)
);

ALTER TABLE workflow_payloads ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_payloads FORCE ROW LEVEL SECURITY;
CREATE POLICY bpmn_lite_tenant_isolation ON workflow_payloads
    FOR ALL
    USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

INSERT INTO workflow_payloads (tenant_id, payload_hash, payload)
SELECT tenant_id, domain_payload_hash, convert_to(domain_payload, 'UTF8')
FROM workflow_instances
ON CONFLICT (tenant_id, payload_hash) DO NOTHING;

INSERT INTO workflow_payloads (tenant_id, payload_hash, payload)
SELECT tenant_id, domain_payload_hash, convert_to(domain_payload, 'UTF8')
FROM job_queue
ON CONFLICT (tenant_id, payload_hash) DO NOTHING;

ALTER TABLE job_queue ADD COLUMN payload_ref_hash BYTEA;
UPDATE job_queue SET payload_ref_hash = domain_payload_hash;
ALTER TABLE job_queue
    ADD CONSTRAINT job_queue_payload_ref_fk
        FOREIGN KEY (tenant_id, payload_ref_hash)
        REFERENCES workflow_payloads(tenant_id, payload_hash);

INSERT INTO workflow_payloads (tenant_id, payload_hash, payload)
SELECT tenant_id, digest(input, 'sha256'), input
FROM workflow_effects
ON CONFLICT (tenant_id, payload_hash) DO NOTHING;

ALTER TABLE workflow_effects ADD COLUMN input_ref_hash BYTEA;
UPDATE workflow_effects SET input_ref_hash = digest(input, 'sha256');
ALTER TABLE workflow_effects
    ADD CONSTRAINT workflow_effects_input_ref_fk
        FOREIGN KEY (tenant_id, input_ref_hash)
        REFERENCES workflow_payloads(tenant_id, payload_hash);

-- Down path (data-preserving): hydrate inline columns from workflow_payloads,
-- drop the two foreign keys/reference columns, then drop workflow_payloads.
