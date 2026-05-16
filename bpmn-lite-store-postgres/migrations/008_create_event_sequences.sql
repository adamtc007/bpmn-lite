CREATE TABLE event_sequences (
    instance_id  UUID PRIMARY KEY,
    next_seq     BIGINT NOT NULL DEFAULT 0
);
