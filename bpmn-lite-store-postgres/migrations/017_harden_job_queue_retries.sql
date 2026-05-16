ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS attempt_count INTEGER NOT NULL DEFAULT 0;

ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS last_error TEXT;

ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS last_failed_at TIMESTAMPTZ;

ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS dead_lettered_at TIMESTAMPTZ;

ALTER TABLE job_queue
    DROP CONSTRAINT IF EXISTS chk_job_queue_status;

ALTER TABLE job_queue
    ADD CONSTRAINT chk_job_queue_status
    CHECK (status IN ('pending', 'claimed', 'dead_lettered'));

CREATE INDEX IF NOT EXISTS idx_jobs_dead_lettered
ON job_queue (tenant_id, dead_lettered_at)
WHERE status = 'dead_lettered';
