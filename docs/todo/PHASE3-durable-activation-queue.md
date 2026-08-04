# Phase 3A — durable activation queue (schema + store primitives)

Status: **done, unwired**. This closes the 3A slice only: a durable,
tenant-scoped, per-instance-serialised ready-work table plus store
primitives on every `RuntimeStore` implementer. Nothing in the engine
enqueues to or dequeues from it yet — `claim_running_instances`'s
full-population scan remains the live scheduler. F-01 (the scale defect
this queue exists to fix) is not yet remediated in production behavior;
this phase only proves the replacement mechanism works in isolation.

## Why 3A only

Phase 3 as scoped in `zed_agent_execution_lease_remediation_plan.md`
bundles schema + primitives + producer wiring + scheduler cutover +
removal of the old scan into one phase. That's too much surface for one
red→green slice, and cutting the scheduler over before the new path has
its own test receipts would mean debugging two unknowns (a new schema
*and* a new dispatch path) at once. Splitting:

- **3A (this phase):** schema, `RuntimeStore` trait methods, both
  implementations (`MemoryStore`, `PostgresWorkflowStore`), fault/test
  doubles, unit tests against each. Zero live behavior change.
- **3B (future):** dual-write — engine enqueues an activation alongside
  every existing scan-visible state change, shadow-verified against
  `claim_running_instances`'s output, still not consumed.
- **3C (future):** scheduler cutover — dispatch claims from
  `claim_ready_activations` instead of `claim_running_instances`.
- **3D (future):** remove the population-scan path once 3C has run
  clean for a bake period.

## What's built

### Schema — `bpmn-lite-store-postgres/migrations/062_workflow_activations.sql`

`workflow_activations`: one row per pending unit of ready work
(`command_id` identifies the durable command — a timer fire, a message
delivery, a job result — that produced it). Key invariants enforced by
the schema, not application code:

- **Idempotent enqueue:** `UNIQUE (tenant_id, command_id)` — the same
  command reported ready twice inserts once.
- **I-8, per-instance serialisation:** `workflow_activations_one_claimed_per_instance`,
  a partial unique index on `(tenant_id, instance_id) WHERE status =
  'claimed'` — at most one claimed activation per instance, enforced by
  the database even if every caller has a bug.
- **Claim identity is atomic-or-nothing:** `CHECK ((claim_owner IS NULL)
  = (claim_token IS NULL))` and the matching check for
  `claim_expires_at` — no half-claimed row.
- **RLS**, same `bpmn_lite_tenant_isolation` pattern as every other
  runtime table.
- Ready-selection index `(tenant_id, priority, available_at, seq) WHERE
  status = 'ready'` is the scan a future scheduler runs instead of
  `claim_running_instances`'s full-table sweep — the actual point of
  Phase 3, not yet exercised in anger.

### Trait — `bpmn-lite-store/src/store.rs`

Seven new `RuntimeStore` methods, mirroring the existing
`ClaimedTimer`/`ClaimedEffect` claim/renew/release/consume/dead-letter/
reclaim shape: `enqueue_activation`, `claim_ready_activations`,
`renew_activation_claim`, `release_activation_to_ready`,
`consume_activation`, `dead_letter_activation`,
`reclaim_expired_activations`.

### Types — `bpmn-lite-types/src/transition.rs`

`ClaimedActivation`: tenant, activation id, instance id, command id,
command kind, the `Command` itself, attempt count, claim token —
immutable value handed back by a claim, matching the existing
`ClaimedTimer`/`ClaimedEffect` pattern so call sites don't have to learn
a new shape.

### `MemoryStore` — `bpmn-lite-store/src/store_memory.rs`

In-process `HashMap<Uuid, MemoryActivation>` plus a `(tenant_id,
command_id) -> activation_id` dedupe index. `claim_ready_activations`
enforces per-instance-one-claimed both against already-claimed rows and
within the current batch (a `claimed_this_batch: HashSet<Uuid>`), since
nothing stops a caller asking for `limit=10` when eight ready rows
belong to the same instance.

### `PostgresWorkflowStore` — `bpmn-lite-store-postgres/src/store_postgres.rs`

The one genuinely tricky piece: `claim_ready_activations` needs
`FOR UPDATE SKIP LOCKED` (so concurrent claimers don't block each other)
*and* per-instance dedup ordered by priority — but PostgreSQL refuses to
combine a locking clause with `DISTINCT`/window functions in the same
`SELECT`. Solved with a three-CTE structure:

1. `locked` — plain `FOR UPDATE SKIP LOCKED` over-fetch, bounded by
   `(limit * 8).max(64)` rows computed in Rust. Over-fetching is
   necessary because the lock happens *before* per-instance dedup: a
   batch skewed toward one instance could lock ten ready rows and still
   yield only one distinct-instance result if the fetch window were
   exactly `limit`.
2. `deduped` — `ROW_NUMBER() OVER (PARTITION BY instance_id ORDER BY
   priority, available_at, seq)` over the already-locked rows (legal
   now — no locking clause in this SELECT).
3. `ranked` — filters `rn = 1`, re-orders, applies the real `LIMIT`.

Then a plain `UPDATE ... FROM ranked ... RETURNING`. `claim_token` is
`md5(random()::text || clock_timestamp()::text)`, matching the
Phase 2 `lease_token` generation pattern.

`reclaim_expired_activations` iterates tenants explicitly (mirroring
`reclaim_stale_buffered_message_claims`) rather than querying
`self.pool` directly — RLS scopes a transaction to one
`current_setting('app.current_tenant', ...)` at a time, so a global
query against the pool with no tenant context set matches zero rows
under `FORCE ROW LEVEL SECURITY`. Caught by the test suite (first
version of the reclaim test failed 0 == 1), not by the compiler — the
same class of RLS-scoping mistake previous phases have hit.

### Test/fault doubles

`ViolatingTestStore` (store_postgres.rs test module) and `FaultStore`
(bpmn-lite-engine/fuzz/src/fault.rs) both got the 7 methods —
`ViolatingTestStore` delegates straight through, `FaultStore` wraps each
in the existing `faulty!` fault-injection macro, same as every other
`RuntimeStore` method there.

## Receipts

**MemoryStore** (`cargo test -p bpmn-lite-store --lib test_phase3a`) — 5/5:

```
test_phase3a_memory_enqueue_activation_is_idempotent_on_command_id ... ok
test_phase3a_memory_claim_enforces_one_claimed_activation_per_instance ... ok
test_phase3a_memory_claim_then_consume_removes_from_ready_pool ... ok
test_phase3a_memory_release_returns_activation_to_ready_pool ... ok
test_phase3a_memory_dead_letter_activation_removes_it_from_ready_pool ... ok
```

**PostgresWorkflowStore** (`BPMN_LITE_TEST_DATABASE_URL=... cargo test -p
bpmn-lite-store-postgres --lib test_phase3a -- --test-threads=1`) — 8/8:

```
test_phase3a_enqueue_activation_is_idempotent_on_command_id ... ok
test_phase3a_claim_ready_activations_respects_available_at ... ok
test_phase3a_claim_enforces_one_claimed_activation_per_instance ... ok
test_phase3a_claim_then_consume_removes_from_ready_pool ... ok
test_phase3a_release_returns_activation_to_ready_pool ... ok
test_phase3a_renew_activation_claim_extends_expiry_for_live_token ... ok
test_phase3a_dead_letter_activation_removes_it_from_ready_pool ... ok
test_phase3a_reclaim_expired_activations_returns_stale_claims_to_ready ... ok
```

`test_phase3a_renew_activation_claim_extends_expiry_for_live_token` is
the negative half: it also asserts a `ClaimedActivation` reconstructed
with a wrong `claim_token` is rejected by `renew_activation_claim`
(returns `None`), not silently renewed.

**Full workspace**, including the fuzz crate:

```
cargo build --workspace --tests                         → clean
(cd bpmn-lite-engine/fuzz && cargo build --tests)        → clean
cargo test -p bpmn-lite-kernel --lib                     → 49/49
cargo test -p bpmn-lite-engine --lib                     → 77/77
cargo test -p bpmn-lite-store --lib                      → 34/34
BPMN_LITE_TEST_DATABASE_URL=... cargo test \
  -p bpmn-lite-store-postgres --lib -- --test-threads=1  → 93/94
```

The one Postgres failure,
`test_phase0_f03_active_active_recovery_aborts_on_busy_lease`, is the
pre-existing F-03 baseline red test (startup recovery aborting the whole
scan instead of skipping one busy-leased instance) — untouched by this
phase, explicitly Phase 4 territory per the remediation plan. Not a
regression.

`cargo clippy --lib --tests --no-deps` on every touched crate
(`bpmn-lite-types`, `bpmn-lite-store`, `bpmn-lite-store-postgres`,
`bpmn-lite-kernel`, `bpmn-lite-engine`, `bpmn-lite-engine/fuzz`): the
only warnings present are pre-existing and outside this phase's diff
(`store_memory.rs:2207`'s `sort_by` in the design-session summaries
path, two `collapsible_if` warnings in kernel `lib.rs` at 3811/4113
predating this phase's `apply_job_completion` edit).

## Deliberately deferred (not this phase)

- **3B/3C/3D** as above — no producer enqueues an activation yet, no
  consumer claims one, the old scan is untouched and still live.
- **`base_revision`** column exists in the schema (diagnostic) but no
  consumer checks it for staleness — that's a 3C concern once there's a
  real consumer to make the call against.
- **Renewal threading through the engine** — `renew_activation_claim`
  exists and is tested directly; nothing calls it from a long-running
  tick yet, same as the primitive existing without a scheduler wired to
  it.
- **Priority** is a plain `INTEGER NOT NULL DEFAULT 0` column, always 0
  today — no producer sets it. The ordering machinery (`ORDER BY
  priority, available_at, seq` throughout) is proven correct against a
  synthetic priority in none of these tests specifically, since nothing
  sets a non-zero value yet; worth a dedicated test once 3B gives a
  producer that can set it meaningfully.
