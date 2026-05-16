ALTER TABLE process_instances
    ADD COLUMN IF NOT EXISTS session_stack JSONB NOT NULL DEFAULT '{}';

ALTER TABLE job_queue
    ADD COLUMN IF NOT EXISTS session_stack JSONB NOT NULL DEFAULT '{}';
