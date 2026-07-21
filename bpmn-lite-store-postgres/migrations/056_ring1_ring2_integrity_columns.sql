-- V2.2/V2.3: D3 Ring 1 (physical integrity) and Ring 2 (frame chain) columns.
-- EOP-PLAN-BPMN-ISA-002, EOP-VS-BPMN-ISA-002 §6.
--
-- Ring 1: `frame_hash` is BLAKE3 over the exact `snapshot_envelope` BYTEA as
-- stored — verified on load BEFORE `SnapshotEnvelope::decode` runs, so a
-- corrupted frame never reaches the deserializer.
--
-- Ring 2: `prior_state_hash` completes the journal chain
-- (`prior_state_hash -> state_hash` per record); resume verifies each
-- record's `prior_state_hash` equals the previous record's `state_hash`.
-- Greenfield: no production instances exist (EOP-VS-BPMN-ISA-002 §3), so
-- existing rows are backfilled with a zero hash rather than migrated —
-- any pre-V2 snapshot_envelope predates the Ring 2 hash-domain redefinition
-- (concurrency table + pending effects folded in) and cannot be
-- retroactively re-hashed to match; it must be recomputed on next commit.
ALTER TABLE workflow_journal
    ADD COLUMN prior_state_hash BYTEA CHECK (prior_state_hash IS NULL OR octet_length(prior_state_hash) = 32);

UPDATE workflow_journal SET prior_state_hash = '\x0000000000000000000000000000000000000000000000000000000000000000'::bytea
    WHERE prior_state_hash IS NULL;

ALTER TABLE workflow_journal
    ALTER COLUMN prior_state_hash SET NOT NULL;

ALTER TABLE workflow_instances
    ADD COLUMN frame_hash BYTEA CHECK (frame_hash IS NULL OR octet_length(frame_hash) = 32);

-- Explicitly irreversible without data loss:
--   ALTER TABLE workflow_journal DROP COLUMN prior_state_hash;
--   ALTER TABLE workflow_instances DROP COLUMN frame_hash;
-- Dropping either destroys Ring 1/2 verification history; permitted only
-- after an operator export, per the greenfield destructive-down rule.
