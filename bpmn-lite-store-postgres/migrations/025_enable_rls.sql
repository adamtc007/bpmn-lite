-- A16 — Row-Level Security for bpmn-lite tenant isolation.
--
-- Activation requirements:
--   1. A non-superuser role `bpmn_lite_app` must exist:
--      CREATE ROLE bpmn_lite_app LOGIN PASSWORD '...';
--      GRANT ALL ON ALL TABLES IN SCHEMA public TO bpmn_lite_app;
--      GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO bpmn_lite_app;
--   2. The application must connect as `bpmn_lite_app` (not superuser).
--      Superusers bypass RLS; superuser connections defeat the policy.
--   3. Each transaction must set: SET LOCAL app.tenant_id = '<tenant>';
--      The store's `execute_as_tenant(tenant_id)` helper wraps this.
--
-- Tables with direct tenant_id columns get per-row policies.
-- Tables without tenant_id (fibers, join_barriers, etc.) are tenant-bounded
-- through their parent process_instance; application-level filtering on
-- those tables remains the primary enforcement mechanism for now.
--
-- Policies use `USING (tenant_id = current_setting('app.tenant_id', true))`
-- where `true` = no error if app.tenant_id is not set (returns NULL → no rows).

-- ── process_instances ──────────────────────────────────────────────────────

ALTER TABLE process_instances ENABLE ROW LEVEL SECURITY;

-- Bypass for the migration runner / superuser (bypasses automatically).
-- This policy allows the app role to see only its tenant's rows.
CREATE POLICY bpmn_lite_tenant_isolation ON process_instances
    AS PERMISSIVE
    FOR ALL
    TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- ── job_queue ──────────────────────────────────────────────────────────────

ALTER TABLE job_queue ENABLE ROW LEVEL SECURITY;

CREATE POLICY bpmn_lite_tenant_isolation ON job_queue
    AS PERMISSIVE
    FOR ALL
    TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- ── ffi_template ───────────────────────────────────────────────────────────
-- Templates may be tenant-specific or GLOBAL (tenant_id = '00000000-...-000').
-- GLOBAL templates must be visible to all tenants; the policy passes both.

ALTER TABLE ffi_template ENABLE ROW LEVEL SECURITY;

CREATE POLICY bpmn_lite_tenant_ffi ON ffi_template
    AS PERMISSIVE
    FOR ALL
    TO PUBLIC
    USING (
        tenant_id = current_setting('app.tenant_id', true)
        OR tenant_id = '00000000-0000-0000-0000-000000000000'
    )
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- ── ffi_invocation_record ──────────────────────────────────────────────────

ALTER TABLE ffi_invocation_record ENABLE ROW LEVEL SECURITY;

CREATE POLICY bpmn_lite_tenant_isolation ON ffi_invocation_record
    AS PERMISSIVE
    FOR ALL
    TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- ── incidents ──────────────────────────────────────────────────────────────
-- incidents.tenant_id was not added in earlier migrations; add it now.

ALTER TABLE incidents
    ADD COLUMN IF NOT EXISTS tenant_id TEXT NOT NULL DEFAULT 'default';

ALTER TABLE incidents ENABLE ROW LEVEL SECURITY;

CREATE POLICY bpmn_lite_tenant_isolation ON incidents
    AS PERMISSIVE
    FOR ALL
    TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- NOTE: fibers, join_barriers, event_log, compiled_programs, and other
-- tables are not RLS-gated here. Their rows are bounded by instance_id
-- relationships to process_instances (which IS RLS-gated). Application-level
-- filtering on those tables is sufficient for v1.1.
--
-- Full RLS coverage of all tables is A16 Phase 2 work, requiring:
-- - Adding tenant_id to fibers, join_barriers, event_log, incidents
-- - Migrating existing rows to carry the correct tenant_id
-- - Updating all store queries to SET LOCAL app.tenant_id per transaction
