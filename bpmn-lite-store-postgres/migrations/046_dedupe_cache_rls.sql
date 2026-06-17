-- Migration 046: Add tenant_id to dedupe_cache, backfill, and enable RLS.

-- 1. Add nullable tenant_id column
ALTER TABLE dedupe_cache ADD COLUMN tenant_id TEXT;

-- 2. Backfill tenant_id from process_instances by parsing the UUID prefix from job_key
UPDATE dedupe_cache d
SET tenant_id = p.tenant_id
FROM process_instances p
WHERE CAST(SUBSTRING(d.job_key FROM 1 FOR 36) AS UUID) = p.instance_id;

-- 3. Fail-loud RAISE EXCEPTION on any NULL remainder
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM dedupe_cache WHERE tenant_id IS NULL) THEN
        RAISE EXCEPTION 'Backfill failed: some dedupe_cache rows have NULL tenant_id after backfill!';
    END IF;
END $$;

-- 4. Set NOT NULL constraint
ALTER TABLE dedupe_cache ALTER COLUMN tenant_id SET NOT NULL;

-- 5. Enable and Force Row Level Security
ALTER TABLE dedupe_cache ENABLE ROW LEVEL SECURITY;
ALTER TABLE dedupe_cache FORCE ROW LEVEL SECURITY;

-- 6. Create the canonical USING + WITH CHECK policy on tenant_id
DROP POLICY IF EXISTS bpmn_lite_tenant_isolation ON dedupe_cache;
CREATE POLICY bpmn_lite_tenant_isolation ON dedupe_cache
    FOR ALL USING (tenant_id = current_setting('app.current_tenant', true))
    WITH CHECK (tenant_id = current_setting('app.current_tenant', true));
