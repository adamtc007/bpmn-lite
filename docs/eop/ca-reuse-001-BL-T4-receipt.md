# BL-T4 receipt — write-once guard on the inbox processed-stamp (EOP-PLAN-CA-REUSE-001)

**Finding (F5):** `dsl-bus-storage/src/inbox.rs::mark_inbox_processed` (pre-fix `:72`)
stamped `status = 'processed', processed_at = now()` with no status precondition and no
rows-affected check — a duplicate mark silently re-stamped `processed_at`, and a mark for
a missing row silently succeeded.

## What changed

- UPDATE now carries `AND status = 'received'`; `rows_affected != 1` is a new typed
  `BusStorageError::InboxMarkLost(key)` (`dsl-bus-storage/src/types.rs`), joining the
  crate's existing `LostClaim` family. Doc comment states the write-once contract.
- One query, one variant: `mark_inbox_processed` takes a single-use `impl PgExecutor`, so
  the already-processed vs missing split (which BL-T1 made with a follow-up SELECT) is not
  distinguishable without an API change; both are `InboxMarkLost`. No production caller of
  this function exists in this repo (grep: only the lib doc, the re-export, and tests) —
  the contract change lands on the API surface consumed by the bus receiver side.

## Red→green

`tests::inbox_mark_processed_is_write_once` — **failed before the fix**
("second mark must be InboxMarkLost, got Ok(())"), passes after. Asserts: second mark ⇒
typed error, `processed_at` not re-stamped, missing-row mark ⇒ typed error.

## Receipts

- `cargo test -p dsl-bus-storage -- --test-threads=1` (live DB) — 11 passed / 0 failed (+1 doc test).
- `cargo check --workspace` — clean.
