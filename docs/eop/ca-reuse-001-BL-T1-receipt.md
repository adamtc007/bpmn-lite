# BL-T1 receipt — write-once guard on the pending-invocation ack (EOP-PLAN-CA-REUSE-001)

**Finding:** `bpmn-lite-store-postgres/src/pending_store.rs:87` (pre-fix) updated
`bpmn_pending_invocation.execution_id` with no `execution_id IS NULL` guard, while the
transition-coupled sibling ack in `store_postgres.rs` (the `pending_ack` arm, guard at
`WHERE … AND execution_id IS NULL`, pre-fix `:2203`) enforces write-once and returns
`CommitError::Conflict` on a second ack. A stale/duplicate SubmissionAck could silently
overwrite the first recorded execution identity.

## What changed

- `bpmn-lite-store/src/error.rs` — added `StoreError::Conflict(String)` + `conflict()`
  helper, mirroring the sibling's `CommitError::Conflict` shape (no third outcome invented).
- `bpmn-lite-store-postgres/src/pending_store.rs::record_ack` — UPDATE now carries
  `AND execution_id IS NULL`; on zero rows a same-transaction SELECT distinguishes
  `NotFound` (row missing, pre-existing behaviour) from `Conflict` (already acked).
- `bpmn-lite-store/src/pending.rs::MemoryPendingInvocationStore::record_ack` — same defect,
  same fix (`execution_id.is_some()` → `Conflict`), keeping trait-parity.
- `PendingInvocationStore::record_ack` trait doc now states the write-once contract.

## Red→green

- `pending_store::tests::record_ack_is_write_once_second_ack_conflicts` (postgres) —
  **failed before the fix** (second ack succeeded and overwrote), passes after.
- `pending::tests::record_ack_is_write_once_second_ack_conflicts` (memory) —
  **failed before the fix**, passes after.
- Both assert: second ack ⇒ `Err(StoreError::Conflict)`, first `execution_id` preserved.

## Audit of every write path in `pending_store.rs`

| path | guard | verdict |
|---|---|---|
| `insert` (`ON CONFLICT DO NOTHING`, PK `callout_id` + UNIQUE `idempotency_key`) | dedupe by constraint | clean |
| `record_ack` | was unguarded | **fixed this tranche** |
| `take_by_execution_id` (`DELETE … RETURNING`) | only matches an acked row; duplicate take is a contractual clean `None` | clean |

Parity items outside the file: `MemoryPendingInvocationStore::record_ack` — same defect,
**fixed**; transition-coupled ack in `store_postgres.rs` — already guarded (the reference).
Related but out of scope (logged, not touched): `dsl-bus-storage/src/inbox.rs:72` re-stamps
`status = 'processed'` with no status precondition (recon BL-12 note).

## Receipts

- `cargo test -p bpmn-lite-store` — 47 passed / 0 failed.
- `cargo test -p bpmn-lite-store-postgres -- --test-threads=1` (live `bpmn_lite_test`) — 95 passed / 0 failed.
- `cargo check --workspace` — clean (one pre-existing unrelated warning in
  `bpmn-lite-server-designer/src/rest.rs`, untouched).
