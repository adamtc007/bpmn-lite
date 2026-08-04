# Phase 3 — durable activation queue

Status: **3A + 3B done; 3C consumer built and tested, NOT yet wired
into the live scheduler.** 3A built a durable, tenant-scoped,
per-instance-serialised ready-work table plus store primitives on every
`RuntimeStore` implementer. 3B makes `commit_transition` itself the
producer: any commit that leaves a fiber in `WaitState::Running` writes
a matching activation atomically, in the same database transaction. 3C
adds `BpmnLiteEngine::tick_activated_batch`, a real consumer, tested
against `MemoryStore` — but `bpmn-lite-server-runner`'s live scheduler
loop (`main.rs`) still calls `tick_claimed_batch`/`claim_running_
instances`; the cutover swap itself is a pending decision (see "3C —
the cutover swap" below), not yet made. F-01 (the scale defect this
queue exists to fix) is therefore still not remediated in production
behavior.

## Why 3A only

Phase 3 as scoped in `zed_agent_execution_lease_remediation_plan.md`
bundles schema + primitives + producer wiring + scheduler cutover +
removal of the old scan into one phase. That's too much surface for one
red→green slice, and cutting the scheduler over before the new path has
its own test receipts would mean debugging two unknowns (a new schema
*and* a new dispatch path) at once. Splitting:

- **3A (done):** schema, `RuntimeStore` trait methods, both
  implementations (`MemoryStore`, `PostgresWorkflowStore`), fault/test
  doubles, unit tests against each. Zero live behavior change.
- **3B (done, this update):** dual-write — `commit_transition` enqueues
  an activation atomically with the commit that creates the runnable
  condition, for every command type (they all funnel through this one
  function). Still shadow: nothing consumes from the table.
- **3C (consumer built, cutover pending):** `tick_activated_batch`
  claims from `claim_ready_activations` and drains each named instance
  via the existing `tick_instance_as_owner`. Tested in isolation; the
  live scheduler in `bpmn-lite-server-runner/src/main.rs` has not been
  switched over to call it yet.
- **3D (future):** remove the population-scan path once 3C has run
  clean for a bake period.

## 3C — the consumer

`BpmnLiteEngine::tick_activated_batch(owner, limit, lease_ms)`
(`bpmn-lite-engine/src/engine.rs`) claims a bounded batch via
`claim_ready_activations`, then for each activation: runs the existing
`tick_instance_as_owner` (which already loops internally until the
instance is quiescent or parks again — so one activation claim maps to
one full drain, not one kernel step), then `consume_activation` on
success or `release_activation_to_ready` on failure. A failed drain
returns its activation to `ready` rather than dead-lettering it — the
same failure classes the old `tick_instance_ids_as_owner` already
treats as retryable-via-next-scheduler-pass (lease contention,
transient store errors) would otherwise permanently strand the
instance on the very first error.

Known, accepted inefficiency: `tick_instance_as_owner`'s internal drain
loop can itself trigger several intermediate `commit_transition` calls,
each of which (per 3B) enqueues its own activation for whatever's still
runnable. Only the ONE activation this consumer claimed going in gets
consumed; any intermediate ones the drain's own commits produced along
the way are not consumed here — they get claimed and drained on some
future pass, find the instance already quiescent, and return
immediately as cheap no-ops. This is wasted claim-cycles, not a
correctness bug (a stale activation trigger a real kernel step is never
possible — draining an already-quiescent instance is idempotent). Worth
optimizing (e.g. consuming every activation for the instance being
drained, not just the one that triggered it) before scale testing, not
before functional cutover.

Receipts: `tick_activated_batch_drains_an_instance_start_leaves_
runnable` — instance start's own 3B dual-write produces exactly one
ready activation with nothing having ticked yet; draining it via
`tick_activated_batch` reaches `ProcessState::Completed` in exactly one
activation and leaves nothing else claimable.
`tick_activated_batch_releases_to_ready_on_tick_failure` — an
activation whose drain fails (missing artifact) is still counted as
processed but goes back to `ready`, not dropped.

### 3C — the cutover swap (not yet decided)

The plan's literal instruction: *"Change the scheduler to claim
activations, not `ProcessState::Running` rows."* That's a real
production behavior change — `bpmn-lite-server-runner/src/main.rs`'s
scheduler loop would call `tick_activated_batch` instead of
`tick_claimed_batch` for every tenant, every 500ms, for every running
instance in the system. Two sub-questions to settle before touching
that call site, deliberately not decided here:

1. **Hard swap vs. dual-run behind a flag.** A hard swap is what the
   plan describes for 3C (3D is a *separate*, later phase that removes
   `claim_running_instances` itself) — but nothing in this codebase has
   run `tick_activated_batch` against a real multi-tenant Postgres
   workload yet, only `MemoryStore` unit tests. A short dual-run (both
   paths active, `tick_claimed_batch`'s old behavior kept as a safety
   net) trades a known inefficiency (both scanning and claiming) for a
   bake-in period with a proven fallback.
2. **3B's reconciliation query** (deferred in the 3B section above) —
   the plan positions it as a pre-cutover sanity check, not a
   post-cutover nice-to-have. Worth building before flipping the
   production switch, even in the hard-swap case.

Both are real decisions with production blast radius, not
implementation details — surfaced here rather than decided unilaterally.

## 3B — design fork and resolution

The plan says: *"In the same transaction that creates the condition,
enqueue the activation."* Two ways to get that atomicity were on the
table:

- **Option A (chosen):** thread the enqueue into `commit_transition`
  itself — the single function every command type (`Tick`, `TimerFired`,
  `MessageDelivered`, job completion, effect response, admin
  cancel/resume) already funnels through via `apply_and_commit_command`.
  `commit_transition` already computes a `has_running_fiber` boolean for
  an existing purpose (clearing the transition lease when nothing is
  left to do); 3B reuses that exact signal to decide whether to enqueue,
  in the same DB transaction as the rest of the commit.
- **Option B (rejected):** call `enqueue_activation` separately, right
  after `commit_transition` returns, from `apply_and_commit_command`.
  Smaller diff, but not atomic — a crash between the two calls would
  silently reproduce the exact "runnable-with-no-activation" gap this
  queue exists to close. Adam's call: *"option B is a frig and
  shortcut"* — rejected outright, no partial credit for "the gap is
  inert until 3C." Went with Option A.

### Why one hook covers most of the plan's producer list

3B's plan section lists several distinct "ready" conditions: new
instance/start continuation, a transition leaving a fiber runnable, due
timer delivery, correlated message delivery, job result, effect
response, admin cancellation/resume/retry. These are not independent
call sites to instrument one by one — every one of them is *resolved*
inside the kernel as a `Transition` whose `fibers_upsert()` contains a
fiber back in `WaitState::Running` (a timer firing sets the parked
fiber's wait back to `Running` before advancing its PC; so does a job
completion, a message delivery, a `V2Fork` spawning immediately-runnable
children, and instance start's root fiber). Checking `fibers_upsert()`
for `WaitState::Running` once, inside `commit_transition`, catches all
of them in one mechanism, because `commit_transition` is the one place
every one of those `Transition`s lands.

What's *not* covered by this single hook, and is honestly out of 3B's
tested scope: nothing currently distinguishes "this activation's
underlying command was a timer vs. a message vs. an admin action" — the
enqueued activation is always a generic `Command::Tick`. That's
sufficient for 3C's purpose (the consumer just needs to know "tick this
instance"), but if a future phase wants per-command-kind activations
(e.g., to skip re-deriving what's runnable), that's a deferred
refinement, not a 3B gap.

### Idempotent dedupe identity

`activation_command_id_for_runnable(instance_id, revision)`
(`bpmn-lite-types/src/transition.rs`) is a `blake3`-domain-separated
hash of `(instance_id, revision)`, mirroring the existing
`EffectId::for_transition` pattern. Since `commit_transition` computes
exactly one `new_revision` per successful commit, this key is
naturally unique per commit — `enqueue_activation`'s
`(tenant_id, command_id)` uniqueness (a `ON CONFLICT DO NOTHING` in
Postgres, a `HashMap` dedupe index in `MemoryStore`) is what actually
prevents a double-insert, not the hash's uniqueness alone; the hash just
makes retries of the *same* commit collapse onto the *same* row instead
of minting a distinct one every time.

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

### 3B

**MemoryStore** (`cargo test -p bpmn-lite-store --lib test_phase3b`) — 3/3:

```
test_phase3b_commit_leaving_a_running_fiber_enqueues_an_activation ... ok
test_phase3b_commit_leaving_no_running_fiber_enqueues_nothing ... ok
test_phase3b_repeated_commit_at_same_revision_does_not_double_enqueue ... ok
```

**PostgresWorkflowStore** (`BPMN_LITE_TEST_DATABASE_URL=... cargo test
-p bpmn-lite-store-postgres --lib test_phase3b -- --test-threads=1`) —
3/3, same three cases: a commit with an upserted `Running` fiber
enqueues exactly one activation claimable via `claim_ready_activations`;
a commit that only parks a fiber (`WaitState::Job`) enqueues nothing;
and a pre-seeded activation at the same `(instance_id, revision)`
dedupe key collapses the real commit's `ON CONFLICT DO NOTHING` insert
onto it rather than erroring or duplicating.

**Full workspace**, including the fuzz crate, after both 3A and 3B:

```
cargo build --workspace --tests                         → clean
(cd bpmn-lite-engine/fuzz && cargo build --tests)        → clean
cargo test -p bpmn-lite-kernel --lib                     → 49/49
cargo test -p bpmn-lite-engine --lib                     → 77/77
cargo test -p bpmn-lite-store --lib                      → 37/37
BPMN_LITE_TEST_DATABASE_URL=... cargo test \
  -p bpmn-lite-store-postgres --lib -- --test-threads=1  → 96/97
```

The one Postgres failure,
`test_phase0_f03_active_active_recovery_aborts_on_busy_lease`, is the
pre-existing F-03 baseline red test (startup recovery aborting the whole
scan instead of skipping one busy-leased instance) — untouched by this
work, explicitly Phase 4 territory per the remediation plan. Not a
regression, and unchanged in count from before 3B (still exactly one
red test, the same one).

`cargo clippy --lib --tests --no-deps` on every touched crate
(`bpmn-lite-types`, `bpmn-lite-store`, `bpmn-lite-store-postgres`,
`bpmn-lite-kernel`, `bpmn-lite-engine`, `bpmn-lite-engine/fuzz`): the
only warnings present are pre-existing and outside this phase's diff
(`store_memory.rs:2207`'s `sort_by` in the design-session summaries
path, two `collapsible_if` warnings in kernel `lib.rs` at 3811/4113
predating this phase's `apply_job_completion` edit).

## Deliberately deferred (not this phase)

- **3C/3D** as above — `commit_transition` now enqueues on every commit
  that leaves a fiber runnable, but no consumer claims one; the old
  `claim_running_instances` population scan is untouched and still the
  live scheduler.
- **3B's reconciliation query** (runnable state with no ready/claimed
  activation; ready activation whose instance is terminal/quarantined;
  multiple authoritative claims for an instance; expired claimed
  activations not reclaimable) — the plan calls for this as a
  divergence check against the old scheduler. With enqueue now atomic
  with the commit (Option A), the first two classes are structurally
  impossible rather than merely rare, which weakens the case for
  building the query now; still worth adding before 3C cutover as a
  pre-flight sanity check, not before.
- **`base_revision`** column exists in the schema (diagnostic) but no
  consumer checks it for staleness — that's a 3C concern once there's a
  real consumer to make the call against.
- **Renewal threading through the engine** — `renew_activation_claim`
  exists and is tested directly; nothing calls it from a long-running
  tick yet, same as the primitive existing without a scheduler wired to
  it.
- **Priority** is a plain `INTEGER NOT NULL DEFAULT 0` column, always 0
  today — 3B's enqueue always writes 0. The ordering machinery (`ORDER
  BY priority, available_at, seq` throughout) is proven correct against
  a synthetic priority in none of these tests specifically, since
  nothing sets a non-zero value yet; worth a dedicated test if a future
  phase gives a producer that can set it meaningfully.
- **Per-command-kind activations** — every 3B-enqueued activation is a
  generic `Command::Tick`, regardless of what actually made the fiber
  runnable (timer, message, job, admin action). Sufficient for 3C's
  purpose (the consumer re-derives what's runnable by ticking), but
  means the activation's `command_kind`/`command` fields don't carry the
  original triggering command — a deliberate simplification, not an
  oversight.
