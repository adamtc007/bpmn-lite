-- Migration 038: Define security definer functions for resolve_tenant_id and resolve_template_tenant_id to bypass RLS for tenant resolution.
-- These run as the migration runner (superuser) and thus can query tenant_id without setting app.tenant_id.

DROP FUNCTION IF EXISTS resolve_job_tenant_id(TEXT);

CREATE OR REPLACE FUNCTION resolve_tenant_id(target_iid UUID)
RETURNS TEXT AS $$
    SELECT tenant_id FROM process_instances WHERE instance_id = target_iid;
$$ LANGUAGE sql SECURITY DEFINER;

CREATE OR REPLACE FUNCTION resolve_template_tenant_id(target_id BYTEA)
RETURNS TEXT AS $$
    SELECT tenant_id FROM ffi_template WHERE template_id = target_id;
$$ LANGUAGE sql SECURITY DEFINER;

-- Revoke default PUBLIC execute privilege (security hardening)
REVOKE EXECUTE ON FUNCTION resolve_tenant_id(UUID) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION resolve_template_tenant_id(BYTEA) FROM PUBLIC;

-- Grant execution specifically to bpmn_lite_app role
GRANT EXECUTE ON FUNCTION resolve_tenant_id(UUID) TO bpmn_lite_app;
GRANT EXECUTE ON FUNCTION resolve_template_tenant_id(BYTEA) TO bpmn_lite_app;
