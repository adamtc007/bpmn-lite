CREATE TABLE event_log (
    instance_id  UUID NOT NULL,
    seq          BIGINT NOT NULL,
    event        JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (instance_id, seq)
);
