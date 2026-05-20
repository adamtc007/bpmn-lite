-- T3 — workflow plan storage and plan-based ProcessInstance fields.

CREATE TABLE workflow_plans (
    plan_hash   BYTEA PRIMARY KEY,
    plan_body   JSONB NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE process_instances
    ADD COLUMN plan_hash       BYTEA,
    ADD COLUMN current_node_id TEXT,
    ADD COLUMN placeholder_values JSONB;
