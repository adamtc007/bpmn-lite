-- T6: Canonical, versioned executable artifacts.
-- Existing rows remain readable as legacy artifacts until the load adapter
-- verifies and rewrites them. New artifacts always populate all three columns.
ALTER TABLE compiled_programs
    ADD COLUMN artifact_hash BYTEA,
    ADD COLUMN canonical_bytes BYTEA,
    ADD COLUMN abi_version INTEGER;

CREATE UNIQUE INDEX compiled_programs_artifact_hash_unique
    ON compiled_programs (artifact_hash)
    WHERE artifact_hash IS NOT NULL;

CREATE TABLE artifact_lineage (
    old_hash BYTEA PRIMARY KEY,
    new_hash BYTEA NOT NULL,
    migrated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (octet_length(old_hash) = 32),
    CHECK (octet_length(new_hash) = 32)
);

-- Down path (destructive for envelope-only artifacts):
-- DROP TABLE artifact_lineage;
-- DROP INDEX compiled_programs_artifact_hash_unique;
-- ALTER TABLE compiled_programs DROP COLUMN abi_version,
--   DROP COLUMN canonical_bytes, DROP COLUMN artifact_hash;
