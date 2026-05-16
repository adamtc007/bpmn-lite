ALTER TABLE process_instances
    ADD COLUMN IF NOT EXISTS lease_owner TEXT;

ALTER TABLE process_instances
    ADD COLUMN IF NOT EXISTS lease_until TIMESTAMPTZ;

ALTER TABLE process_instances
    ADD COLUMN IF NOT EXISTS last_tick_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_instances_scheduler_claim
ON process_instances (tenant_id, lease_until, updated_at)
WHERE state = '"Running"'::jsonb;
