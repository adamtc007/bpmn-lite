-- Migration 044: Define resolve_instance_tenant SECURITY DEFINER function to bypass RLS for tenant resolution.
-- This function runs as the migration runner (superuser) and thus can query tenant_id without setting app.current_tenant.

CREATE OR REPLACE FUNCTION resolve_instance_tenant(target_id UUID)
RETURNS TEXT AS $$
    SELECT tenant_id FROM process_instances WHERE instance_id = target_id
    UNION ALL
    SELECT tenant_id FROM bpmn_process_instance WHERE id = target_id
    LIMIT 1;
$$ LANGUAGE sql SECURITY DEFINER;

-- Revoke default PUBLIC execute privilege (security hardening)
REVOKE EXECUTE ON FUNCTION resolve_instance_tenant(UUID) FROM PUBLIC;

-- Grant execution specifically to bpmn_lite_app role
GRANT EXECUTE ON FUNCTION resolve_instance_tenant(UUID) TO bpmn_lite_app;
