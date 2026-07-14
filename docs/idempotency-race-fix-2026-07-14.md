# `spawn-instance` idempotency-race fix (2026-07-14)

## Origin

ob-poc's control-plane graduation program (`EOP-SESSION-CONTROLPLANE-G6B-G6C-IMPL-001.md`
§3.3) flagged a suspected TOCTOU race in `bpmn_controller::start_instance`'s
idempotency check, citing `rust/crates/bpmn-controller/src/instance.rs` and a
non-unique `idx_instances_correlation` index. That file is a stale
compiled-against snapshot on the ob-poc side — it does not reflect this
repo's current source. This document records what the *real*, current
mechanism turned out to be, why it is still a genuine race (just not the one
originally described), and the fix.

## What the original characterization got right and wrong

**Right:** there is a real check-then-act race in the instance-spawn path
that can produce duplicate work for one logical idempotent request.

**Wrong in the specifics:**

- There is no `bpmn_controller` crate and no `instance.rs` file in this repo.
- `process_instances.correlation_id` (`bpmn-lite-store-postgres/migrations/001_create_process_instances.sql:13,18`)
  is real and its index (`idx_instances_correlation`) is indeed a plain
  non-unique btree — but this table is the fiber VM's *inner*
  per-callout execution state, and nothing in this codebase does an
  idempotent "look up by correlation_id, else insert" against it. It is
  irrelevant to the actual bug.
- The outer, durable "one BPMN workflow instance" table is
  `bpmn_process_instance` (`bpmn-lite-store-postgres/migrations/034_bpmn_process_instance.sql`).
  It has **no `correlation_id` column at all** (a separate, likely dead,
  latent bug: `bpmn-lite-bus-handler/src/lib.rs:243`'s `correlate_message`
  queries `bpmn_process_instance WHERE correlation_id = $1`, a column that
  has never existed in any migration — out of scope for this fix, not
  touched).
- The real per-request idempotency mechanism is `bpmn_spawn_idempotency`
  (`bpmn-lite-store-postgres/migrations/042_verb_defect_and_isolation_fixes.sql`),
  added by a prior "D1: Idempotency in spawn-instance" fix. Its
  `idempotency_key` column **is already a `PRIMARY KEY`** — i.e. a real
  DB-level uniqueness guarantee already existed for the bookkeeping row.
  So "add a unique index" (the originally prescribed fix) was already
  done, and was not the gap.

## The real bug

`bpmn-lite-bus-handler/src/lib.rs`, `InvocationDispatcher::dispatch`,
`"spawn-instance"` arm (pre-fix, lines ~444–503):

1. `lookup_spawn_idempotency` (line 455, pre-fix `L130-151`) does a bare
   `SELECT instance_id FROM bpmn_spawn_idempotency WHERE idempotency_key = $1`
   on its own connection, outside any transaction that scopes the rest of
   the work.
2. If nothing is found, `spawn_process_with_idempotency` (pre-fix `L153-231`)
   opens **a separate transaction** `tx` and calls
   `PlanWalker::start_process` (`bpmn-lite-engine/src/plan_walker.rs:377`) —
   a real, side-effecting instance creation that writes the fiber VM's
   `process_instances`/`fibers` rows via `engine_ref.store()`, which is
   **not participating in `tx`** — before inserting the
   `bpmn_process_instance` and `bpmn_spawn_idempotency` bookkeeping rows
   inside `tx` and committing.

Two concurrent `spawn-instance` calls carrying the *same* `idempotency_key`
can both pass step 1 (both see "not found") before either commits step 2.
Both then call `PlanWalker::start_process()` — producing **two real,
fully-persisted process instances**, not one. Only one of the two
subsequent `INSERT INTO bpmn_spawn_idempotency` calls can commit (the PK
stops the second), so:

- The "losing" caller's request fails with an **unhandled Postgres unique-
  violation error** surfaced as `BusServerError::Internal(...)` — not a
  graceful "here's the existing instance" reply.
- The losing caller's `PlanWalker::start_process()` side effects (a real
  process instance with its own fibers, potentially already-dispatched
  outbox entries) are **not** rolled back by the losing `tx`'s rollback,
  because they were written through the engine's own store, independent
  of `tx`. That instance becomes a live, orphaned duplicate with no
  bookkeeping row pointing at it.

So the existing PK on `bpmn_spawn_idempotency.idempotency_key` correctly
prevents two *bookkeeping rows* for one key, but does nothing to prevent
the side-effecting work itself from running twice — the check-then-act gap
is between the `lookup_spawn_idempotency` read and the
`PlanWalker::start_process()` write, not between two competing table
inserts.

## The fix

`bpmn-lite-bus-handler/src/lib.rs`, `spawn_process_with_idempotency`
(now returns `Result<(Uuid, bool), BusServerError>`, the bool meaning
`was_replay`):

1. Open `tx` (unchanged) and set the tenant GUC (unchanged).
2. **New:** immediately acquire a transaction-scoped Postgres advisory
   lock keyed on the idempotency key —
   `SELECT pg_advisory_xact_lock(hashtext($1::text), 0)` — before any
   side-effecting work. This is the very first statement after opening
   `tx`, so it serializes concurrent callers for the *same* key; callers
   for different keys are unaffected (an occasional `hashtext` collision
   only costs a harmless extra serialization, never a correctness
   violation — the row-level re-check below is still authoritative).
3. **New:** re-check `bpmn_spawn_idempotency` under the lock. If a row now
   exists (a concurrent caller won the race and committed while this
   caller was blocked on the lock), commit `tx` (a no-op transaction) and
   return `(existing_instance_id, true)` — an idempotency replay, with no
   duplicate `PlanWalker::start_process()` call.
4. Otherwise, proceed exactly as before: `start_process()`, insert
   `bpmn_process_instance` + `bpmn_spawn_idempotency`, commit, return
   `(new_instance_id, false)`.

The advisory lock releases automatically on `tx` commit or rollback
(`pg_advisory_xact_lock` semantics), so there is no separate unlock path
to get wrong, and no lock leaks across error returns (the `?` operator
drops `tx`, which rolls back and releases the lock).

The `"spawn-instance"` dispatch arm now sets the outcome `detail` to
`"idempotency replay"` when `was_replay` is true, matching the wording the
pre-existing fast-path pre-check (`lookup_spawn_idempotency`, still kept
as an optimization — it lets a non-racing replay skip taking the lock
entirely) already used.

Files changed:

- `bpmn-lite-bus-handler/src/lib.rs` — the fix described above.
- `bpmn-lite-bus-handler/tests/sage_macro_assembly_tests.rs` — new
  regression test (below).

No schema migration was needed: the DB-level uniqueness guarantee
(`bpmn_spawn_idempotency_pkey`) already existed; the gap was purely in the
application code's failure to make the check-and-act atomic with the
side-effecting work it was guarding.

## Test

`test_concurrent_spawn_instance_same_idempotency_key_creates_exactly_one_instance`
(`bpmn-lite-bus-handler/tests/sage_macro_assembly_tests.rs`):

- Registers a template via `define-template`.
- Fires 8 concurrent `spawn-instance` invocations (via `tokio::task::JoinSet`,
  through an `Arc<BpmnLiteBusHandler>` shared across tasks) that all carry
  **one shared `idempotency_key`**.
- Asserts: all 8 outcomes report the same `execution_id`; exactly one
  outcome has detail `"Instance spawned"` and the other 7 have
  `"idempotency replay"`; and, querying the database directly,
  `bpmn_process_instance` has exactly one row for that id and
  `bpmn_spawn_idempotency` has exactly one row for that key.

Skips gracefully (prints and returns) if `BPMN_LITE_TEST_DATABASE_URL` /
`DATABASE_URL` doesn't resolve to a reachable Postgres, matching the
existing tests' convention in this file.

## RED → GREEN evidence

**RED** (pre-fix `bpmn-lite-bus-handler/src/lib.rs`, via
`git stash push -- bpmn-lite-bus-handler/src/lib.rs` to isolate just the
fix while keeping the new test):

```
$ BPMN_LITE_TEST_DATABASE_URL=postgresql:///bpmn_lite_test cargo test -p bpmn-lite-bus-handler \
    --test sage_macro_assembly_tests \
    test_concurrent_spawn_instance_same_idempotency_key_creates_exactly_one_instance -- --nocapture

thread 'test_concurrent_spawn_instance_same_idempotency_key_creates_exactly_one_instance' panicked at
bpmn-lite-bus-handler/tests/sage_macro_assembly_tests.rs:595:51:
spawn-instance dispatch failed: Internal("error returned from database: duplicate key value violates
unique constraint \"bpmn_spawn_idempotency_pkey\"")
test test_concurrent_spawn_instance_same_idempotency_key_creates_exactly_one_instance ... FAILED
```

This confirms the predicted failure mode exactly: the pre-existing PK does
stop the second bookkeeping insert, but the caller gets an unhandled
database error instead of a graceful replay — the check-then-act gap is
real and observable.

**GREEN** (fix restored via `git stash pop`, resolving an unrelated
`Cargo.lock` conflict from this repo's pre-existing dirty working tree by
`git checkout -- Cargo.lock` before popping — `Cargo.lock`,
`.DS_Store`, `docker-compose.yml`, and various `scripts/*` files were
already modified/untracked in this repo before this session started and
are not part of this change):

```
$ BPMN_LITE_TEST_DATABASE_URL=postgresql:///bpmn_lite_test cargo test -p bpmn-lite-bus-handler \
    --test sage_macro_assembly_tests \
    test_concurrent_spawn_instance_same_idempotency_key_creates_exactly_one_instance -- --nocapture

running 1 test
test test_concurrent_spawn_instance_same_idempotency_key_creates_exactly_one_instance ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.04s
```

## Full verification (post-fix)

`cargo build --workspace`:

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.76s
```

(One pre-existing, unrelated warning in `bpmn-lite-engine/src/plan_walker.rs:21`
— unused imports `BusEndpoint`/`OutboxEntry`/`insert_outbox`. Confirmed
pre-existing on `main` HEAD before this change, not touched by this fix.)

`cargo test -p bpmn-lite-bus-handler` (`BPMN_LITE_TEST_DATABASE_URL=postgresql:///bpmn_lite_test`):

```
running 5 tests (unit)
test tests::reject_invocation_dispatcher_responds_with_unknown_verb ... ok
test tests::outcome_kind_unspecified_when_proto_value_unknown ... ok
test tests::dispatch_records_input_via_concrete_arc ... ok
test tests::unknown_execution_advancer_error_maps_to_internal ... ok
test tests::malformed_advancer_error_maps_to_malformed ... ok
test result: ok. 5 passed; 0 failed

running 4 tests (tests/sage_macro_assembly_tests.rs)
test test_sage_template_registration_via_bus ... ok
test test_sage_transitive_validation_propagation ... ok
test test_postgres_store_and_retrieve_via_bus_handler ... FAILED
test test_concurrent_spawn_instance_same_idempotency_key_creates_exactly_one_instance ... ok
test result: FAILED. 3 passed; 1 failed
```

`test_postgres_store_and_retrieve_via_bus_handler` is a **pre-existing,
unrelated flake**: it registers a template named `"onboarding-postgres-test"`
against the shared, non-reset `bpmn_lite_test` database and asserts the
literal outcome detail `"Template registered"`; because the database
already has prior versions of that template name from earlier test runs
(no truncation/cleanup between runs), the handler correctly returns
`"Template registered as version N"` for N > 1 and the test's hard-coded
string assertion fails. This is a test-isolation gap in `define-template`
versioning, orthogonal to `spawn-instance` idempotency — this fix does not
touch `define-template` code, and re-running the test in isolation
reproduces the same failure with an incrementing N each time,
confirming it as pre-existing accumulated state rather than something
introduced here.

`cargo clippy -p bpmn-lite-bus-handler --all-targets` (no `-D warnings`,
since the pre-existing `bpmn-lite-engine` warning fails `-D warnings` at
the workspace level regardless of this change — confirmed by running the
same clippy invocation against the unmodified crate on `main` HEAD, which
fails identically):

```
    Checking bpmn-lite-bus-handler v0.1.0 (/Users/adamtc007/dev/bpmn-lite/bpmn-lite-bus-handler)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.16s
```

Zero warnings from the touched crate.

## Disposition

Changes are **uncommitted** in the working tree, left for the operator to
review and commit/push/tag at their discretion, per instructions. Nothing
pushed to `origin`, no tag created.

### Files changed

- `bpmn-lite-bus-handler/src/lib.rs`
- `bpmn-lite-bus-handler/tests/sage_macro_assembly_tests.rs`
- `docs/idempotency-race-fix-2026-07-14.md` (this file)

Not touched (pre-existing dirty state in this repo, unrelated to this
task): `.DS_Store`, `Cargo.lock`, `docker-compose.yml`,
`scripts/test_ui_snapshot.json`, and the various untracked `scripts/*`
scratch files.
