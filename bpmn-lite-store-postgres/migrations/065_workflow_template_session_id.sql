-- 065: G6.4 — template->tape linkage. Nullable back-reference from a
-- published template to the Designer session (design_sessions.id, 059)
-- that authored it. YAML-fronted publishes carry no session and stay
-- NULL. Deliberately absent from the ON CONFLICT DO UPDATE SET list in
-- PostgresTemplateStore::save and from enforce_template_immutability's
-- content-equality check (013/064) -- this is provenance metadata, set
-- once at insert time, not part of the published content a re-save must
-- reproduce byte-for-byte.
ALTER TABLE workflow_templates
    ADD COLUMN IF NOT EXISTS session_id UUID;
