-- Create a security definer function to list tenants with pending outbox entries.
-- This bypasses RLS and allows the global outbox sender sweep to know which tenants to process.
CREATE OR REPLACE FUNCTION dsl_bus.list_pending_outbox_tenants()
RETURNS TABLE(tenant_id VARCHAR) AS $$
BEGIN
  RETURN QUERY SELECT DISTINCT o.tenant_id FROM dsl_bus.outbox o WHERE o.status = 'pending';
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;

GRANT EXECUTE ON FUNCTION dsl_bus.list_pending_outbox_tenants() TO bpmn_lite_app;
