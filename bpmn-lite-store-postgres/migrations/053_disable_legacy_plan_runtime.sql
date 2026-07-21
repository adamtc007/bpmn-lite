-- T9.3/T9.5: preserve legacy plan identity only in the migration report and
-- make it impossible to create a new runtime-selected plan instance.
UPDATE workflow_instances
SET plan_hash = NULL
WHERE plan_hash IS NOT NULL;

ALTER TABLE workflow_instances
    ADD CONSTRAINT workflow_instances_no_legacy_plan_runtime CHECK (plan_hash IS NULL);

-- Explicitly irreversible: re-enabling the retired runtime is not a supported
-- rollback. Restore legacy hashes from workflow_migration_quarantine only for
-- offline inspection, never into the production aggregate.

