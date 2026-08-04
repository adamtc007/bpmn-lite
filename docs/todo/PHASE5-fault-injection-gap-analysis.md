# Phase 5 — fault-injection and release qualification: gap analysis + activation-queue coverage

Status: **partial — existing harness extended twice, first release
criterion closed, full plan scope not attempted**. This session closes
two concrete fault-injection gaps (the activation queue and the
production Postgres backend both had zero coverage), completes the
I-1..I-10 invariant audit (`docs/todo/PHASE5-invariant-audit.md`), and
documents, item by item, how far the rest of Phase 5's checklist is
from done. Phase 5 as specified is a full release-qualification suite
(11 named fault-injection points × 3 execution modes, plus a release-
criteria checklist that is effectively the whole project's Definition
of Done) — too large for one pass; this is a status report and three
targeted pieces of work, not a completed phase.

## What already existed (predates this session's Phase 0-4 work)

`bpmn-lite-engine/fuzz/src/fault.rs` ("F8.2/F8.3", its own header
comment) already had a working fault-injection harness:

- `FaultStore`: wraps any `RuntimeStore` (in this codebase, always
  `MemoryStore`) and injects `Unavailable` either **Before** (fails
  before the store sees the call) or **After** (the operation happens,
  then failure is reported — the durable-effect/lost-response hazard)
  at a tape-seeded rate, via one macro (`faulty!`) wrapping every single
  `RuntimeStore` method — this generically covers "before" and "after"
  injection at every store call boundary already, including all 7
  Phase 3A/3B activation-queue methods (`enqueue_activation` through
  `reclaim_expired_activations`), without any changes needed this
  session.
- Three cement tests: `restart_mid_run_resumes_and_conserves` (engine
  dropped and rebuilt mid-run, a fresh engine must finish the instance
  with conservation intact), `full_fault_storm_errors_cleanly_then_
  recovers` (rate-16 fault storm surfaces as errors not panics, then a
  quiet store recovers to completion), `recovery_driver_steps_clean_
  over_tape_population` (40 tape-seeded runs of `drive_recovery`, mixing
  fault rates, restarts, job completions/failures, cancels).
- A `libfuzzer-sys`-based `engine_recovery` fuzz target sharing the same
  `drive_recovery` driver, for actual continuous fuzzing via
  `cargo-fuzz` (not run in this session — needs a nightly toolchain and
  is separate from the `cargo test` receipts below).

All of this drives ticks through `run_instance` → `tick_instance`, the
direct per-instance path. **None of it ever called `tick_activated_
batch`** — the Phase 3C scheduler-facing consumer that claims from the
durable activation queue — because that method didn't exist when this
harness was written.

## What this session added

Two new tests in the same file, mirroring the two most load-bearing
existing ones exactly but routed through `tick_activated_batch` instead
of `tick_instance`:

- `restart_mid_run_resumes_and_conserves_via_activation_queue` — an
  engine dropped mid-run and rebuilt must still finish the instance via
  `claim_ready_activations` → drain → `consume_activation`, with R-O2
  conservation intact across the restart.
- `full_fault_storm_survives_activation_queue_dispatch` — a rate-16
  fault storm against the activation-queue dispatch path (claim,
  consume, release all wrapped in `faulty!`) must surface as errors,
  never panic, and the instance must still reach completion once the
  storm quiets.

A small helper, `run_instance_via_activation_queue`, factors the
`tick_activated_batch` + `activate_jobs_for_worker` sequence (no
`instance_id` parameter — unlike `tick_instance`, the batch consumer
doesn't target one instance, it claims whatever the tenant's queue has
ready, which in these single-instance tests is always the instance
under test).

Required one dependency addition: `anyhow = "1"` to `bpmn-lite-engine/
fuzz/Cargo.toml` (this fuzz crate is its own standalone `[workspace]`,
so it can't use `{ workspace = true }` — added as a plain version
pin, same style as the crate's existing `serde_json = "1"` etc.), since
`tick_activated_batch` returns `anyhow::Result`.

### Receipts

```
cargo test --lib fault::                    (in bpmn-lite-engine/fuzz) → 5/5
cargo test --lib                            (in bpmn-lite-engine/fuzz) → 22/22 (2 ignored, corpus-writer helpers)
cargo clippy --lib --tests --no-deps        (in bpmn-lite-engine/fuzz) → clean (pre-existing warnings only, none in new code)
cargo build --workspace --tests             (main workspace)           → clean
```

### Update: PostgreSQL connection-loss coverage (gap #11)

Added `test_pg_connection_loss_surfaces_cleanly_and_pool_recovers`
(`bpmn-lite-store-postgres/src/store_postgres.rs`) — the recommendation
this doc made at the bottom, picked over the alternative (an I-1..I-10
invariant audit) because it closes a real gap in the backend that ships
in production, not `MemoryStore`. Uses `pg_terminate_backend` from a
second connection to kill a live pooled connection, proving (a) the
dead connection surfaces a clean error, never a panic/hang, and (b) the
pool self-heals — a normal `load_instance` and a full fenced
`claim_instance_for_transition`/`commit_transition` cycle issued right
after both succeed via a fresh connection, so one dead connection does
not poison the pool for every subsequent caller.

```
BPMN_LITE_TEST_DATABASE_URL=... cargo test \
  -p bpmn-lite-store-postgres --lib test_pg_connection_loss \
  -- --test-threads=1                                            → 1/1
BPMN_LITE_TEST_DATABASE_URL=... cargo test \
  -p bpmn-lite-store-postgres --lib -- --test-threads=1           → 99/99
cargo clippy -p bpmn-lite-store-postgres --lib --tests --no-deps  → clean
cargo build --workspace --tests                                  → clean
```

## Gap analysis: the plan's 11 fault-injection points

| # | Plan's injection point | Current coverage |
|---|---|---|
| 1 | Immediately before/after activation claim | **Covered** (generic, via `faulty!` wrapping `claim_ready_activations`) — exercised by the two new tests above. |
| 2 | After snapshot load | **Covered generically** — `faulty!` wraps `load_instance`/`load_fiber`/`load_fibers`, exercised by the existing storm test (rate-16 hits every call site including these). Not a *dedicated* scenario proving this specific point in isolation. |
| 3 | During artifact load | **Not covered.** `load_artifact`/`load_program` are `ArtifactRepository` methods — `FaultStore` only implements `RuntimeStore`; artifact-load faults are entirely untested. |
| 4 | Before SQL transaction | **Not applicable to `MemoryStore`** (no SQL) and **not covered** for `PostgresWorkflowStore` — this fuzz harness only ever runs against `MemoryStore`. Postgres-specific mid-transaction fault injection does not exist anywhere in this codebase. |
| 5 | After conditional process update but before journal/activation consumption | **Not covered** — this is a fault point *inside* `commit_transition`'s single transaction, below the `RuntimeStore` method boundary `faulty!` operates at. Would require instrumenting `commit_transition` itself (both store backends) with conditional injection hooks — a materially larger, more invasive change (same category of decision as the 3B atomicity fork, i.e. worth surfacing before attempting, not silently building). |
| 6 | Immediately before SQL commit | **Not covered**, same reason as #4/#5 — Postgres-only, sub-transaction granularity. |
| 7 | Immediately after SQL commit but before response delivery | **Partially covered in spirit** — the existing `After` fault class (operation happens, then failure reported) models exactly this hazard at the `RuntimeStore`-method granularity (e.g. `commit_transition` succeeds, caller sees `Unavailable`), just not at the finer "commit succeeded, journal write about to happen" granularity SQL-internal instrumentation would give. |
| 8 | During renewal | **Not covered** — `renew_activation_claim` is fault-injectable generically (wrapped in `faulty!`) but nothing calls it yet (per Phase 3's own doc: renewal isn't wired to a long-running tick), so there's no scenario to drive faults through. |
| 9 | During graceful shutdown | **Not covered.** Phase 4's shutdown sequence lives in `bpmn-lite-server-runner`'s `main()` (a binary, not exercised by this engine-level fuzz harness at all) and has no fault-injection hooks of its own. |
| 10 | After job validation data received but before atomic job completion | **Partially covered generically** — `validate_job_claim` and job-completion commit paths are both `faulty!`-wrapped and exercised by the storm test, but not as a named, isolated scenario. |
| 11 | Under PostgreSQL connection loss and transaction retry | **Partially covered, this update.** `test_pg_connection_loss_surfaces_cleanly_and_pool_recovers` uses `pg_terminate_backend` from a second connection to kill a live pooled connection out from under `PgPool`, proving the dead connection errors cleanly (not a panic/hang) and — the real point — that the pool self-heals: a normal `load_instance` and a full fenced `claim_instance_for_transition`/`commit_transition` cycle issued right after both succeed via a fresh connection. **Not covered**: killing a connection *mid-transaction* (interrupting `commit_transition` while its own transaction is open) — that needs a synchronization hook inside `commit_transition` to pause at a known point, which doesn't exist and would be new production instrumentation, not test-only code. |

**Summary (updated): 2 of 11 points have dedicated new coverage across
this session's two passes (#1 activation claim, #11 connection loss —
partially); 2-3 more are covered generically by the existing storm test
as a side effect of hitting every `RuntimeStore` call (#2, #7, #10
partially); the remaining 5-6 are gaps, and most of those (#4-6, #9)
would require fault-injection hooks that don't exist at all yet
(SQL-transaction-internal instrumentation, a shutdown-sequence hook) —
not a matter of writing another test against existing infrastructure.**

## Gap analysis: "run under one executor, multiple native executors, and the intended Wasmtime pool"

- **One executor**: what every existing test already does.
- **Multiple native executors**: **not covered**. Every existing test
  uses a single `BpmnLiteEngine` (or a sequential drop-and-rebuild, not
  concurrent engines). No test races two live engines against the same
  store concurrently under fault injection.
- **Wasmtime pool**: **does not exist in this codebase.** Grep confirms
  no Wasmtime dependency or pooled-instance execution model anywhere in
  the workspace — the plan's "Wasm instance destruction must be
  indistinguishable from native worker death to the durable authority
  model" describes a target architecture this codebase has not built
  yet, not a gap in test coverage of existing code.

## Gap analysis: release criteria

| Criterion | Status |
|---|---|
| I-1 through I-10 have named automated tests | **Done** — see `docs/todo/PHASE5-invariant-audit.md`. All 10 mapped to specific, verified-passing tests; I-10 is only partially covered (acquisition-stop half proven, drain-before-exit half has no test harness) and stated as such, not rounded up to a clean pass. |
| No stale acquisition can release/renew/ack/consume newer work | **Largely covered** — Phase 2 (F-04 lease tokens) and Phase 3A (activation claim tokens) both have dedicated tests for exactly this. |
| No business transition can commit from a lost external-work claim | **Covered** for jobs (Phase 1, F-02) and transitions (Phase 2); not separately proven for activations. |
| Active-active startup and rolling restart pass repeatedly under load | **Partially covered** — F-03 (Phase 4) proves one contended lease is skipped correctly; "repeatedly under load" with real concurrency is not tested. |
| Idle large process population generates no generic tick write amplification | **Covered** — `test_phase3c_f01_idle_population_produces_zero_activation_claims` (Phase 4). "Large" is 10 instances in that test, not a scale test. |
| Ambiguous commit responses are idempotently recoverable | **Covered generically** by the `After`-fault class tests (storm recovers to completion after ambiguous/lost responses), not a named dedicated scenario. |
| All PostgreSQL integration tests run against a real test database in CI | **True locally** (`bpmn-lite-store-postgres` 98/98 against a real local Postgres this session) — **CI wiring itself was not verified this session** (no `.github/workflows` or CI config was inspected). |
| `cargo fmt`, focused tests, workspace tests, `cargo clippy`, repo CI checks pass | **fmt/clippy/tests verified locally, repeatedly, this session** — repo-specific CI checks beyond `cargo` commands not inspected. |
| Migration upgrade tested from last production schema; rollback documented | **Not done.** No "last production schema" baseline exists to test an upgrade from in this dev branch context, and no rollback strategy doc was written for the new `061_transition_lease_token.sql`/`062_workflow_activations.sql` migrations. |

## Recommendation — both done

Both options from the original recommendation are now closed. Option
(b), PostgreSQL connection-loss fault injection against
`PostgresWorkflowStore`, is done above. Option (a) — this doc initially
claimed the plan didn't enumerate I-1..I-10 anywhere it had read, which
was simply wrong: they're at `zed_agent_execution_lease_remediation_
plan.md` lines 28-46, missed on the first pass. Corrected and completed
as `docs/todo/PHASE5-invariant-audit.md` — every invariant mapped to a
specific, independently-verified-to-exist-and-pass test, with one real
gap stated plainly (I-10's drain-before-exit half has no test harness
that starts a real process and asserts what happens to in-flight work).

Phase 5 in full remains a multi-session effort beyond these two slices
(Wasmtime pool doesn't exist; SQL-transaction-internal fault injection
is a real design decision; multi-executor races are untested; CI wiring
and migration-rollback documentation weren't inspected/written this
session — see the release-criteria table above for the complete list).
