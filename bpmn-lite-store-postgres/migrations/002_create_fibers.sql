CREATE TABLE fibers (
    instance_id  UUID NOT NULL REFERENCES process_instances(instance_id) ON DELETE CASCADE,
    fiber_id     UUID NOT NULL,
    pc           INTEGER NOT NULL,
    stack        JSONB NOT NULL DEFAULT '[]',
    regs         JSONB NOT NULL DEFAULT '[]',
    wait_state   JSONB NOT NULL,
    loop_epoch   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (instance_id, fiber_id)
);
