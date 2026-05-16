ALTER TABLE message_buffer
    ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'buffered';

ALTER TABLE message_buffer
    ADD COLUMN IF NOT EXISTS claim_token TEXT;

ALTER TABLE message_buffer
    ADD COLUMN IF NOT EXISTS claimed_at TIMESTAMPTZ;

ALTER TABLE message_buffer
    ADD COLUMN IF NOT EXISTS claim_until TIMESTAMPTZ;

ALTER TABLE message_buffer
    ADD COLUMN IF NOT EXISTS consumed_by_instance_id UUID;

ALTER TABLE message_buffer
    ADD COLUMN IF NOT EXISTS consumed_by_fiber_id UUID;

CREATE INDEX IF NOT EXISTS idx_message_buffer_claimed
ON message_buffer (claim_until)
WHERE consumed_at IS NULL AND claim_token IS NOT NULL;

