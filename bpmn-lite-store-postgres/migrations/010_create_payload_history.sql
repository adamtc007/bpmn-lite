CREATE TABLE payload_history (
    instance_id      UUID NOT NULL,
    payload_hash     BYTEA NOT NULL,
    domain_payload   TEXT NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (instance_id, payload_hash)
);
