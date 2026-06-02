-- Recreate RLS policies to use app.current_tenant instead of app.tenant_id

-- 1. process_instances
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON process_instances;
CREATE POLICY bpmn_lite_tenant_isolation ON process_instances
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 2. job_queue
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON job_queue;
CREATE POLICY bpmn_lite_tenant_isolation ON job_queue
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 3. ffi_template
DROP POLICY IF EXISTS bpmn_lite_tenant_ffi ON ffi_template;
CREATE POLICY bpmn_lite_tenant_ffi ON ffi_template
    FOR ALL USING (
        tenant_id = current_setting('app.current_tenant', true)
        OR tenant_id = '00000000-0000-0000-0000-000000000000'
    )
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 4. ffi_invocation_record
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON ffi_invocation_record;
CREATE POLICY bpmn_lite_tenant_isolation ON ffi_invocation_record
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 5. incidents
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON incidents;
CREATE POLICY bpmn_lite_tenant_isolation ON incidents
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 6. fibers
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON fibers;
CREATE POLICY bpmn_lite_tenant_isolation ON fibers
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 7. join_barriers
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON join_barriers;
CREATE POLICY bpmn_lite_tenant_isolation ON join_barriers
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 8. event_sequences
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON event_sequences;
CREATE POLICY bpmn_lite_tenant_isolation ON event_sequences
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 9. event_log
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON event_log;
CREATE POLICY bpmn_lite_tenant_isolation ON event_log
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 10. payload_history
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON payload_history;
CREATE POLICY bpmn_lite_tenant_isolation ON payload_history
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 11. message_buffer
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON message_buffer;
CREATE POLICY bpmn_lite_tenant_isolation ON message_buffer
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 12. message_dedupe
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON message_dedupe;
CREATE POLICY bpmn_lite_tenant_isolation ON message_dedupe
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 13. bpmn_pending_invocation
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON bpmn_pending_invocation;
CREATE POLICY bpmn_lite_tenant_isolation ON bpmn_pending_invocation
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- 14. bpmn_process_instance
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON bpmn_process_instance;
CREATE POLICY bpmn_lite_tenant_isolation ON bpmn_process_instance
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));

-- Also force row level security
ALTER TABLE process_instances FORCE ROW LEVEL SECURITY;
ALTER TABLE job_queue FORCE ROW LEVEL SECURITY;
ALTER TABLE ffi_template FORCE ROW LEVEL SECURITY;
ALTER TABLE ffi_invocation_record FORCE ROW LEVEL SECURITY;
ALTER TABLE incidents FORCE ROW LEVEL SECURITY;
ALTER TABLE fibers FORCE ROW LEVEL SECURITY;
ALTER TABLE join_barriers FORCE ROW LEVEL SECURITY;
ALTER TABLE event_sequences FORCE ROW LEVEL SECURITY;
ALTER TABLE event_log FORCE ROW LEVEL SECURITY;
ALTER TABLE payload_history FORCE ROW LEVEL SECURITY;
ALTER TABLE message_buffer FORCE ROW LEVEL SECURITY;
ALTER TABLE message_dedupe FORCE ROW LEVEL SECURITY;
ALTER TABLE bpmn_pending_invocation FORCE ROW LEVEL SECURITY;
ALTER TABLE bpmn_process_instance FORCE ROW LEVEL SECURITY;
