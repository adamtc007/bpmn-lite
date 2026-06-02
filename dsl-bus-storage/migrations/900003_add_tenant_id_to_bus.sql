-- Add tenant isolation and RLS to inbox/outbox tables.
ALTER TABLE dsl_bus.inbox ADD COLUMN tenant_id VARCHAR NOT NULL;
ALTER TABLE dsl_bus.outbox ADD COLUMN tenant_id VARCHAR NOT NULL;

ALTER TABLE dsl_bus.inbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE dsl_bus.inbox FORCE ROW LEVEL SECURITY;
ALTER TABLE dsl_bus.outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE dsl_bus.outbox FORCE ROW LEVEL SECURITY;

CREATE POLICY inbox_tenant_policy ON dsl_bus.inbox
  USING (tenant_id = current_setting('app.current_tenant', true))
  WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

CREATE POLICY outbox_tenant_policy ON dsl_bus.outbox
  USING (tenant_id = current_setting('app.current_tenant', true))
  WITH CHECK (tenant_id = current_setting('app.current_tenant', true));
