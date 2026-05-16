CREATE TABLE incidents (
    incident_id          UUID PRIMARY KEY,
    process_instance_id  UUID NOT NULL REFERENCES process_instances(instance_id),
    fiber_id             UUID NOT NULL,
    service_task_id      TEXT NOT NULL,
    bytecode_addr        INTEGER NOT NULL,
    error_class          JSONB NOT NULL,
    message              TEXT NOT NULL,
    retry_count          INTEGER NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at          TIMESTAMPTZ,
    resolution           TEXT
);
CREATE INDEX idx_incidents_instance ON incidents (process_instance_id);
