ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS worker_id TEXT;

ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS claim_token TEXT;

ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS claim_expires_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_jobs_claim_owner
ON job_queue (worker_id, claim_token)
WHERE status = 'claimed';
