-- 064: G4.2 — seal the typed parameter manifest (G4.1) alongside the
-- compiled DTO snapshot on workflow_templates (013). Nullable, not
-- NOT NULL DEFAULT '[]' like task_manifest, because pre-G4 rows have no
-- manifest at all — the Rust side (`TemplateRow::into_template`) treats
-- NULL as an empty `ParameterManifest`, never as an error.
ALTER TABLE workflow_templates
    ADD COLUMN IF NOT EXISTS parameter_manifest JSONB;

-- Extend the immutability trigger (013's enforce_template_immutability) so
-- a published template's manifest is exactly as immutable as its
-- dto_snapshot/bytecode_version/task_manifest — the manifest is derived
-- from the same publish-time compile, so it belongs on the same
-- immutable-content list.
CREATE OR REPLACE FUNCTION enforce_template_immutability()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.state = 'published' THEN
        IF NEW.state NOT IN ('published', 'retired') THEN
            RAISE EXCEPTION 'Published template cannot revert to %', NEW.state;
        END IF;
        IF NEW.dto_snapshot IS DISTINCT FROM OLD.dto_snapshot
           OR NEW.bytecode_version IS DISTINCT FROM OLD.bytecode_version
           OR NEW.task_manifest IS DISTINCT FROM OLD.task_manifest
           OR NEW.parameter_manifest IS DISTINCT FROM OLD.parameter_manifest THEN
            RAISE EXCEPTION 'Published template content is immutable';
        END IF;
    END IF;

    IF OLD.state = 'retired' THEN
        IF NEW.state != 'retired' THEN
            RAISE EXCEPTION 'Retired template cannot transition to %', NEW.state;
        END IF;
    END IF;

    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
