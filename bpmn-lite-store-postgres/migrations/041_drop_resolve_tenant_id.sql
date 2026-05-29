-- Migration 041: Drop resolve_tenant_id DB helper.
DROP FUNCTION IF EXISTS resolve_tenant_id(UUID);
