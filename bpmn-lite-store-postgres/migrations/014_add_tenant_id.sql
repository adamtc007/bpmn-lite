ALTER TABLE process_instances
ADD COLUMN IF NOT EXISTS tenant_id TEXT NOT NULL DEFAULT 'default';

ALTER TABLE job_queue
ADD COLUMN IF NOT EXISTS tenant_id TEXT NOT NULL DEFAULT 'default';

CREATE INDEX IF NOT EXISTS idx_instances_tenant_running
ON process_instances (tenant_id, updated_at);

CREATE INDEX IF NOT EXISTS idx_jobs_tenant_pending
ON job_queue (tenant_id, task_type, created_at)
WHERE status = 'pending';
