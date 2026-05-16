CREATE TABLE process_instances (
    instance_id       UUID PRIMARY KEY,
    tenant_id         TEXT NOT NULL DEFAULT 'default',
    process_key       TEXT NOT NULL,
    bytecode_version  BYTEA NOT NULL,
    domain_payload    TEXT NOT NULL,
    domain_payload_hash BYTEA NOT NULL,
    session_stack     JSONB NOT NULL DEFAULT '{}',
    flags             JSONB NOT NULL DEFAULT '{}',
    counters          JSONB NOT NULL DEFAULT '{}',
    join_expected     JSONB NOT NULL DEFAULT '{}',
    state             JSONB NOT NULL,
    correlation_id    TEXT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_instances_process_key ON process_instances (process_key);
CREATE INDEX idx_instances_correlation ON process_instances (correlation_id);
CREATE INDEX idx_instances_tenant_running ON process_instances (tenant_id, updated_at);
