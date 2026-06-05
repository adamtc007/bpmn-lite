-- v0.6 §8.5 — workflow template catalog (Commitment B scope).
CREATE TABLE workflow_template_catalog (
    template_name   VARCHAR(255) NOT NULL,
    version         INT NOT NULL,
    plan_hash       BYTEA NOT NULL REFERENCES workflow_plans(plan_hash),
    dsl_body        TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (template_name, version)
);

CREATE INDEX idx_workflow_template_catalog_plan_hash ON workflow_template_catalog(plan_hash);
