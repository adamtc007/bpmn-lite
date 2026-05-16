ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS not_before TIMESTAMPTZ;

ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS failure_count INTEGER NOT NULL DEFAULT 0;

ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS last_error_class TEXT;

ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS last_error_message TEXT;

ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS incident_id UUID;

CREATE INDEX IF NOT EXISTS idx_jobs_claimable
ON job_queue (tenant_id, task_type, not_before, created_at)
WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_jobs_reclaimable
ON job_queue (tenant_id, claim_expires_at)
WHERE status = 'claimed';

CREATE TABLE IF NOT EXISTS message_buffer (
    tenant_id           TEXT NOT NULL,
    message_name        TEXT NOT NULL,
    correlation_key     TEXT NOT NULL,
    msg_id              TEXT NOT NULL,
    payload             BYTEA NOT NULL,
    payload_hash        BYTEA,
    received_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at          TIMESTAMPTZ NOT NULL,
    consumed_at         TIMESTAMPTZ,
    process_instance_id UUID,
    PRIMARY KEY (tenant_id, message_name, correlation_key, msg_id)
);

CREATE INDEX IF NOT EXISTS idx_message_buffer_match
ON message_buffer (tenant_id, message_name, correlation_key, received_at)
WHERE consumed_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_message_buffer_expiry
ON message_buffer (expires_at)
WHERE consumed_at IS NULL;
