-- T4: revision/fence substrate for deterministic transition commits.
--
-- Irreversible down note: removing these columns would discard concurrency
-- history and permit stale writers. A rollback must deploy code that no longer
-- consumes revision/fence before explicitly dropping the columns.

ALTER TABLE process_instances
    ADD COLUMN IF NOT EXISTS revision BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS fence BIGINT NOT NULL DEFAULT 0;

ALTER TABLE process_instances
    ADD CONSTRAINT process_instances_revision_nonnegative CHECK (revision >= 0),
    ADD CONSTRAINT process_instances_fence_nonnegative CHECK (fence >= 0);
