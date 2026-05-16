CREATE TABLE join_barriers (
    instance_id  UUID NOT NULL REFERENCES process_instances(instance_id) ON DELETE CASCADE,
    join_id      INTEGER NOT NULL,
    arrive_count SMALLINT NOT NULL DEFAULT 0,
    PRIMARY KEY (instance_id, join_id)
);
