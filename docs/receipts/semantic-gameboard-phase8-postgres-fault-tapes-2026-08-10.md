# Semantic gameboard Phase 8 — PostgreSQL fault-tape replay for designer sessions

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification
(`docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14, "Replay minimized session/revision
tapes against real PostgreSQL with two identities, connection loss before/after commit
and process restart").

## What was found before any code was written

The designer/gameboard session store (`bpmn-lite-store::store::RuntimeStore`'s
`create_design_session` / `load_design_session` / `append_design_session_event` /
`mark_design_session_saved` methods, migration `059_design_sessions.sql`) has a real,
complete Postgres implementation in `bpmn-lite-store-postgres` — this is not a
compatibility shim. `bpmn-lite-server-designer` currently wires up
`bpmn_lite_store::store_memory::MemoryStore` exclusively (confirmed by grep across
`rest.rs`); the Postgres path exists but isn't live in production yet, which is a
separate, later rollout decision (Phase 9), not reopened here. Before this receipt,
**zero tests** exercised the Postgres implementation of these four methods at all.

## What was built

Local test Postgres: `docker run postgres:16-bookworm` on host port 5433 (5432 was
already bound by a pre-existing native Postgres process on this machine — left
untouched; picked a different port instead of touching it), matching
`.github/workflows/nightly-chaos.yml`'s existing recipe otherwise. Torn down after this
work completed — throwaway, not committed to any dev-environment config.

Three new tests in `bpmn-lite-store-postgres/src/store_postgres.rs`, reusing this
crate's own established idioms exactly (`setup()`'s migration/role-grant harness, the
`pg_terminate_backend` technique already proven in
`test_pg_connection_loss_surfaces_cleanly_and_pool_recovers`, `Arc<PostgresWorkflowStore>`
+ `tokio::spawn` for concurrency, matching `test_concurrent_claim_and_recovery`):

- `test_pg_design_session_survives_restart_with_identical_replay` — create a session,
  append 2 events, drop the store/pool entirely, build a fully independent reconnection
  (as a fresh process would), and assert the reloaded record is byte-identical
  (`Debug`-string equality) with exact, unrenumbered sequence numbers.
- `test_pg_design_session_append_recovers_after_connection_loss` — kill a live
  connection out from under the store's own pool via `pg_terminate_backend`, then prove
  the very next append still succeeds via a fresh pooled connection and lands **exactly
  once** (no partial or duplicate event from the interrupted connection).
- `test_pg_design_session_concurrent_identities_and_appends` — two tenants, two
  sessions, 8 concurrent `tokio::spawn`ed appends racing on the SAME session; asserts a
  gap-free, duplicate-free sequence, and that neither tenant's identity can load the
  other's session.

## A real bug found, not a test artifact

The concurrent-identity test failed deterministically (5/5 runs before the fix, 0/5
after): `append_design_session_event`'s original SQL computed the next `seq` via
`INSERT ... SELECT COALESCE(MAX(seq) + 1, 0) ... WHERE session_id = s.id` with no
locking. Two concurrent transactions on the same session can both read the same
`MAX(seq)` before either commits; the loser hits a raw
`design_session_events_pkey` unique-constraint violation, surfaced as the generic
`StoreError::Unavailable` rather than a distinguishable, retriable conflict — a real
correctness gap in a code path with zero prior test coverage.

**Fixed**, with the user's explicit sign-off (surfaced as a fork before touching
production code, per the working contract, rather than silently patched or silently
downgraded to "accept the race and weaken the test"): `append_design_session_event` now
runs inside an explicit transaction that first locks the parent `design_sessions` row
(`SELECT id ... FOR UPDATE`) before computing `MAX(seq) + 1` and inserting. This
serializes concurrent appends to the same session on the row lock, so the seq
computation can no longer race. `updated_at` is now updated inside the same
transaction too (previously a separate, non-transactional statement after the insert —
folded in while already touching this function, not a separate decision).

Verified as a real fix, not a coincidence: reproduced the original failure 5/5 runs,
then confirmed the fixed version passes 5/5 runs before moving on.

## Wired into a recurring gate, not just run once locally

Added a new step to `.github/workflows/nightly-chaos.yml` running all three tests, 20
passes each, alongside the existing kernel-level chaos cut points — matching the "the
gate that doesn't run is not a gate" principle. A local pass proves the bug existed and
the fix works; the CI step is what keeps it caught if it ever regresses.

## Results

- `cargo test -p bpmn-lite-store-postgres` (against local Postgres, `--test-threads=1`):
  102 passed (was 99), 0 failed, 0 regressions.
- The 3 new tests individually re-run 5x each post-fix: 15/15 passed, 0 flakes.
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, surface hashes
  unchanged (no `pub` surface change — the fix is internal to one function's SQL).

## Scope note

This closes the designer-session slice of Phase 8's PostgreSQL fault-tape bullet
specifically — the "session/revision tapes" the bullet's own wording describes, which
is the gameboard/designer model this entire Phase 7/8 effort has been about. It does
not claim to cover every Postgres-backed table in this workspace (the kernel/workflow-
instance path already has its own, separate chaos coverage from before this session).
