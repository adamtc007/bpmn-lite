-- D1: Idempotency in spawn-instance
CREATE TABLE bpmn_spawn_idempotency (
    idempotency_key UUID PRIMARY KEY,
    instance_id UUID NOT NULL REFERENCES bpmn_process_instance(id) ON DELETE CASCADE,
    tenant_id VARCHAR NOT NULL
);

ALTER TABLE bpmn_spawn_idempotency ENABLE ROW LEVEL SECURITY;
ALTER TABLE bpmn_spawn_idempotency FORCE ROW LEVEL SECURITY;
CREATE POLICY spawn_idempotency_tenant_isolation ON bpmn_spawn_idempotency
  USING (tenant_id = current_setting('app.current_tenant', true))
  WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

GRANT SELECT, INSERT, UPDATE, DELETE ON bpmn_spawn_idempotency TO bpmn_lite_app;
