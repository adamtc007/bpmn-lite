-- Migration 040: Denormalize tenant_id onto child tables and enforce RLS + FORCE RLS.

-- 1. Add tenant_id (nullable initially) to instance-scoped child tables
ALTER TABLE fibers ADD COLUMN IF NOT EXISTS tenant_id TEXT;
ALTER TABLE join_barriers ADD COLUMN IF NOT EXISTS tenant_id TEXT;
ALTER TABLE event_sequences ADD COLUMN IF NOT EXISTS tenant_id TEXT;
ALTER TABLE event_log ADD COLUMN IF NOT EXISTS tenant_id TEXT;
ALTER TABLE payload_history ADD COLUMN IF NOT EXISTS tenant_id TEXT;
ALTER TABLE bpmn_pending_invocation ADD COLUMN IF NOT EXISTS tenant_id TEXT;
ALTER TABLE bpmn_process_instance ADD COLUMN IF NOT EXISTS tenant_id TEXT;

-- 2. Backfill tenant_id from parent process_instances
UPDATE fibers f SET tenant_id = pi.tenant_id FROM process_instances pi WHERE f.instance_id = pi.instance_id;
UPDATE join_barriers j SET tenant_id = pi.tenant_id FROM process_instances pi WHERE j.instance_id = pi.instance_id;
UPDATE event_sequences es SET tenant_id = pi.tenant_id FROM process_instances pi WHERE es.instance_id = pi.instance_id;
UPDATE event_log el SET tenant_id = pi.tenant_id FROM process_instances pi WHERE el.instance_id = pi.instance_id;
UPDATE payload_history ph SET tenant_id = pi.tenant_id FROM process_instances pi WHERE ph.instance_id = pi.instance_id;
UPDATE bpmn_pending_invocation bpi SET tenant_id = pi.tenant_id FROM process_instances pi WHERE bpi.process_instance_id = pi.instance_id;
UPDATE bpmn_process_instance bpi SET tenant_id = pi.tenant_id FROM process_instances pi WHERE bpi.id = pi.instance_id;

-- For any orphaned/new child rows, raise an exception to fail loud rather than fabricating tenants
DO $$
DECLARE
    orphan_cnt INT;
    sample_ids TEXT;
BEGIN
    -- fibers
    SELECT COUNT(*) INTO orphan_cnt FROM fibers WHERE tenant_id IS NULL;
    IF orphan_cnt > 0 THEN
        SELECT STRING_AGG(fiber_id::text, ', ') INTO sample_ids FROM (SELECT fiber_id FROM fibers WHERE tenant_id IS NULL LIMIT 5) t;
        RAISE EXCEPTION 'Table fibers has % orphaned rows. Sample fiber_ids: %', orphan_cnt, sample_ids;
    END IF;

    -- join_barriers
    SELECT COUNT(*) INTO orphan_cnt FROM join_barriers WHERE tenant_id IS NULL;
    IF orphan_cnt > 0 THEN
        SELECT STRING_AGG(join_id::text, ', ') INTO sample_ids FROM (SELECT join_id FROM join_barriers WHERE tenant_id IS NULL LIMIT 5) t;
        RAISE EXCEPTION 'Table join_barriers has % orphaned rows. Sample join_ids: %', orphan_cnt, sample_ids;
    END IF;

    -- event_sequences
    SELECT COUNT(*) INTO orphan_cnt FROM event_sequences WHERE tenant_id IS NULL;
    IF orphan_cnt > 0 THEN
        SELECT STRING_AGG(instance_id::text, ', ') INTO sample_ids FROM (SELECT instance_id FROM event_sequences WHERE tenant_id IS NULL LIMIT 5) t;
        RAISE EXCEPTION 'Table event_sequences has % orphaned rows. Sample instance_ids: %', orphan_cnt, sample_ids;
    END IF;

    -- event_log
    SELECT COUNT(*) INTO orphan_cnt FROM event_log WHERE tenant_id IS NULL;
    IF orphan_cnt > 0 THEN
        SELECT STRING_AGG(instance_id::text, ', ') INTO sample_ids FROM (SELECT instance_id FROM event_log WHERE tenant_id IS NULL LIMIT 5) t;
        RAISE EXCEPTION 'Table event_log has % orphaned rows. Sample instance_ids: %', orphan_cnt, sample_ids;
    END IF;

    -- payload_history
    SELECT COUNT(*) INTO orphan_cnt FROM payload_history WHERE tenant_id IS NULL;
    IF orphan_cnt > 0 THEN
        SELECT STRING_AGG(instance_id::text, ', ') INTO sample_ids FROM (SELECT instance_id FROM payload_history WHERE tenant_id IS NULL LIMIT 5) t;
        RAISE EXCEPTION 'Table payload_history has % orphaned rows. Sample instance_ids: %', orphan_cnt, sample_ids;
    END IF;

    -- bpmn_pending_invocation
    SELECT COUNT(*) INTO orphan_cnt FROM bpmn_pending_invocation WHERE tenant_id IS NULL;
    IF orphan_cnt > 0 THEN
        SELECT STRING_AGG(callout_id::text, ', ') INTO sample_ids FROM (SELECT callout_id FROM bpmn_pending_invocation WHERE tenant_id IS NULL LIMIT 5) t;
        RAISE EXCEPTION 'Table bpmn_pending_invocation has % orphaned rows. Sample callout_ids: %', orphan_cnt, sample_ids;
    END IF;

    -- bpmn_process_instance
    SELECT COUNT(*) INTO orphan_cnt FROM bpmn_process_instance WHERE tenant_id IS NULL;
    IF orphan_cnt > 0 THEN
        SELECT STRING_AGG(id::text, ', ') INTO sample_ids FROM (SELECT id FROM bpmn_process_instance WHERE tenant_id IS NULL LIMIT 5) t;
        RAISE EXCEPTION 'Table bpmn_process_instance has % orphaned rows. Sample ids: %', orphan_cnt, sample_ids;
    END IF;
END $$;

-- 3. Set columns to NOT NULL
ALTER TABLE fibers ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE join_barriers ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE event_sequences ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE event_log ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE payload_history ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE bpmn_pending_invocation ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE bpmn_process_instance ALTER COLUMN tenant_id SET NOT NULL;

-- 4. Enable RLS and FORCE RLS on all child tables (including message buffer/dedupe)
ALTER TABLE fibers ENABLE ROW LEVEL SECURITY;
ALTER TABLE fibers FORCE ROW LEVEL SECURITY;

ALTER TABLE join_barriers ENABLE ROW LEVEL SECURITY;
ALTER TABLE join_barriers FORCE ROW LEVEL SECURITY;

ALTER TABLE event_sequences ENABLE ROW LEVEL SECURITY;
ALTER TABLE event_sequences FORCE ROW LEVEL SECURITY;

ALTER TABLE event_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE event_log FORCE ROW LEVEL SECURITY;

ALTER TABLE payload_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE payload_history FORCE ROW LEVEL SECURITY;

ALTER TABLE message_buffer ENABLE ROW LEVEL SECURITY;
ALTER TABLE message_buffer FORCE ROW LEVEL SECURITY;

ALTER TABLE message_dedupe ENABLE ROW LEVEL SECURITY;
ALTER TABLE message_dedupe FORCE ROW LEVEL SECURITY;

ALTER TABLE bpmn_pending_invocation ENABLE ROW LEVEL SECURITY;
ALTER TABLE bpmn_pending_invocation FORCE ROW LEVEL SECURITY;

ALTER TABLE bpmn_process_instance ENABLE ROW LEVEL SECURITY;
ALTER TABLE bpmn_process_instance FORCE ROW LEVEL SECURITY;

-- 5. Create Row-Level Security policies with USING and WITH CHECK
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON fibers;
CREATE POLICY bpmn_lite_tenant_isolation ON fibers
    FOR ALL TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON join_barriers;
CREATE POLICY bpmn_lite_tenant_isolation ON join_barriers
    FOR ALL TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON event_sequences;
CREATE POLICY bpmn_lite_tenant_isolation ON event_sequences
    FOR ALL TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON event_log;
CREATE POLICY bpmn_lite_tenant_isolation ON event_log
    FOR ALL TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON payload_history;
CREATE POLICY bpmn_lite_tenant_isolation ON payload_history
    FOR ALL TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON message_buffer;
CREATE POLICY bpmn_lite_tenant_isolation ON message_buffer
    FOR ALL TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON message_dedupe;
CREATE POLICY bpmn_lite_tenant_isolation ON message_dedupe
    FOR ALL TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON bpmn_pending_invocation;
CREATE POLICY bpmn_lite_tenant_isolation ON bpmn_pending_invocation
    FOR ALL TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON bpmn_process_instance;
CREATE POLICY bpmn_lite_tenant_isolation ON bpmn_process_instance
    FOR ALL TO PUBLIC
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));
