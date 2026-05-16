CREATE TABLE job_queue (
    job_key              TEXT PRIMARY KEY,
    tenant_id            TEXT NOT NULL DEFAULT 'default',
    process_instance_id  UUID NOT NULL,
    task_type            TEXT NOT NULL,
    service_task_id      TEXT NOT NULL,
    domain_payload       TEXT NOT NULL,
    domain_payload_hash  BYTEA NOT NULL,
    session_stack        JSONB NOT NULL DEFAULT '{}',
    orch_flags           JSONB NOT NULL DEFAULT '{}',
    retries_remaining    INTEGER NOT NULL DEFAULT 3,
    status               TEXT NOT NULL DEFAULT 'pending',
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    claimed_at           TIMESTAMPTZ
);
CREATE INDEX idx_jobs_pending ON job_queue (task_type, created_at) WHERE status = 'pending';
CREATE INDEX idx_jobs_instance ON job_queue (process_instance_id);
CREATE INDEX idx_jobs_tenant_pending ON job_queue (tenant_id, task_type, created_at) WHERE status = 'pending';
