-- 013: Workflow template registry for publish lifecycle.

CREATE TABLE IF NOT EXISTS workflow_templates (
    template_key     VARCHAR(255) NOT NULL,
    template_version INTEGER NOT NULL,
    process_key      VARCHAR(255) NOT NULL,
    bytecode_version VARCHAR(64) NOT NULL,
    state            VARCHAR(20) NOT NULL DEFAULT 'draft'
                     CHECK (state IN ('draft', 'published', 'retired')),
    source_format    VARCHAR(20) NOT NULL DEFAULT 'yaml',
    dto_snapshot     JSONB NOT NULL,
    task_manifest    JSONB NOT NULL DEFAULT '[]',
    bpmn_xml         TEXT,
    summary_md       TEXT,
    verb_registry_hash VARCHAR(64),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    published_at     TIMESTAMPTZ,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (template_key, template_version)
);

-- Fast lookup for latest published version of a template.
CREATE INDEX IF NOT EXISTS idx_wf_templates_latest
    ON workflow_templates (template_key, state, template_version DESC)
    WHERE state = 'published';

-- Immutability guard: published content cannot change, retired cannot revert.
CREATE OR REPLACE FUNCTION enforce_template_immutability()
RETURNS TRIGGER AS $$
BEGIN
    -- Published templates: only state change to 'retired' is allowed
    IF OLD.state = 'published' THEN
        IF NEW.state NOT IN ('published', 'retired') THEN
            RAISE EXCEPTION 'Published template cannot revert to %', NEW.state;
        END IF;
        -- Content fields must not change
        IF NEW.dto_snapshot IS DISTINCT FROM OLD.dto_snapshot
           OR NEW.bytecode_version IS DISTINCT FROM OLD.bytecode_version
           OR NEW.task_manifest IS DISTINCT FROM OLD.task_manifest THEN
            RAISE EXCEPTION 'Published template content is immutable';
        END IF;
    END IF;

    -- Retired templates: no state changes allowed
    IF OLD.state = 'retired' THEN
        IF NEW.state != 'retired' THEN
            RAISE EXCEPTION 'Retired template cannot transition to %', NEW.state;
        END IF;
    END IF;

    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_template_immutability ON workflow_templates;
CREATE TRIGGER trg_template_immutability
    BEFORE UPDATE ON workflow_templates
    FOR EACH ROW
    EXECUTE FUNCTION enforce_template_immutability();
