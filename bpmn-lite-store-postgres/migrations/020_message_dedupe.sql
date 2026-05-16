CREATE TABLE IF NOT EXISTS message_dedupe (
    tenant_id   TEXT NOT NULL,
    instance_id UUID NOT NULL,
    msg_id      TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, instance_id, msg_id)
);

CREATE INDEX IF NOT EXISTS idx_message_dedupe_created_at
ON message_dedupe (created_at);
