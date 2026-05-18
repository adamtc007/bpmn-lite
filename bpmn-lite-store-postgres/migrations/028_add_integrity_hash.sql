-- A19 — Integrity hash and quarantine state for process_instances.
--
-- integrity_hash: BLAKE3 hash of the 7 immutable fields computed at instance
--   creation and stored once. This is a birth-certificate / forensic fingerprint.
--   Immutability of the 7 identity fields is enforced by the BEFORE UPDATE
--   trigger in migration 029 — not by runtime hash re-verification at pickup.
--   NULL for rows created before A19; new instances always have this field set.
--
-- quarantine_state: NULL for normal instances. Reserved for operational use
--   (e.g. marking instances that require operator intervention). Quarantined
--   instances are skipped by claim_running_instances. Only written by
--   quarantine_instance(); original row fields preserved for forensic inspection.
--
-- Hash input format (BLAKE3, fixed field order):
--   instance_id (16 bytes UUID) | "|"
--   tenant_id (UTF-8 bytes)     | "|"
--   bytecode_version (32 bytes) | "|"
--   created_at_ms (8 bytes LE)  | "|"
--   process_key (UTF-8 bytes)   | "|"
--   entry_id (16 bytes UUID)    | "|"
--   runbook_id (16 bytes UUID)  | "|"
--   b""  (created_by_identity placeholder — v0.2 field, absent in v0.1)

ALTER TABLE process_instances
    ADD COLUMN IF NOT EXISTS integrity_hash BYTEA,
    ADD COLUMN IF NOT EXISTS quarantine_state TEXT;

-- Partial index — only indexes the rare non-NULL case.
-- Keeps claim_running_instances fast; quarantine IS NULL filter is cheap.
CREATE INDEX IF NOT EXISTS idx_instances_quarantined
    ON process_instances (quarantine_state)
    WHERE quarantine_state IS NOT NULL;
