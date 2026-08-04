# Phase 4 (partial) — F-03: startup recovery must skip busy instances, not abort

Status: **done**. Closes F-03, the last remaining red baseline test from
the forensic review — `test_phase0_f03_active_active_recovery_aborts_on_busy_lease`
now passes. Scope is deliberately narrow: this is the "startup recovery"
sub-section of Phase 4 only (the specific defect the plan calls out —
*"a legitimately busy process is not startup failure"*), not graceful
shutdown, metrics/diagnostics, or the full Gate 4 (multi-replica
start/restart, forced-termination recovery bound). Those remain future
Phase 4 work.

## The defect

`BpmnLiteEngine::recover_all_tenants` (`bpmn-lite-engine/src/engine.rs`)
scans every `Running` instance per tenant at startup and calls
`claim_work_for_transition` to fence each one before verifying its
structural integrity (artifact present, fibers present, ABI current).
When a peer replica already holds a live transition lease on an
instance — the expected, normal state in an active-active deployment,
not corruption — `claim_work_for_transition` returns `None`. The old
code turned that into:

```rust
.ok_or_else(|| anyhow!("recovery could not fence tenant {tenant_id} instance {instance_id}"))?
```

propagating as an `Err` through `with_instance_guard(...).await?`,
which aborted the *entire* `recover_all_tenants` call — not just that
one instance. One busy instance anywhere in a tenant's population
failed startup recovery for every tenant on that replica.

A second, subtler instance of the same bug lived one level down: even
after fixing the busy-abort above, `scan_recoverable_inconsistencies`
(called once per tenant after the per-instance loop) unconditionally
inspects every `Running` instance's artifact/fibers/start-event —
including ones a peer is actively mid-flight mutating. Inspecting a
busy instance's structural state races the peer's own commits and can
produce false-positive `RecoveryIssue`s purely from timing, which
`recover_all_tenants` then also turns into a hard `Err` for the whole
tenant scan.

## The fix

Two changes in `bpmn-lite-engine/src/engine.rs`:

1. **`recover_all_tenants`'s per-instance loop**: the claim closure now
   returns `Result<bool>` (`true` = verified, `false` = busy) instead of
   erroring on a `None` claim. A `let...else` on the `claim_work_for_
   transition` result returns `Ok(false)` for the busy case; every other
   failure path (artifact/fiber verification, commit/release errors)
   still propagates as a real `Err`, unchanged. The busy instance IDs
   accumulate into a `HashSet<Uuid>` for step 2.
2. **`scan_recoverable_inconsistencies`** gained a `busy: &HashSet<Uuid>`
   parameter; instances in that set are skipped entirely rather than
   structurally inspected. `recover_all_tenants` passes the busy set
   built in step 1; the two direct test callers in `tests.rs` pass an
   empty set (unaffected — the underlying bug they test predates this
   fix).

`RecoveryReport` gained an `instances_busy_skipped: usize` field
alongside the existing `instances_verified` — a busy instance is
counted, not silently dropped, so an operator reading the recovery log
can distinguish "verified N, skipped M busy" from either N or M being
zero unexpectedly.

## Receipts

`test_phase0_f03_active_active_recovery_aborts_on_busy_lease`
(`bpmn-lite-store-postgres/src/store_postgres.rs`) — replica A holds a
live transition lease; replica B's `recover_all_tenants` must return
`Ok` with `instances_busy_skipped == 1` and `instances_verified == 0`,
not abort. **This test existed since Phase 0 as the documented red
baseline for F-03 — it is now green, with no other test's assertions
weakened to get there** (full receipts below).

Also added (unrelated defect, same investigation): `test_phase3c_f01_
idle_population_produces_zero_activation_claims` — the direct inversion
of `test_phase0_f01_idle_population_claims_regardless_of_readiness`'s
own forward-referencing doc comment (*"Phase 3's durable activation
queue removes `claim_running_instances` from the normal dispatch path
entirely, at which point this test's assertion should invert to 'an
idle population produces zero claims/writes'"*). Ten `Running` instances
with no runnable fiber (and therefore no Phase 3B dual-write) now
produce zero claims from `claim_ready_activations` — the direct, load-
bearing proof that F-01 is fixed in the live dispatch path, not just
that the mechanism exists.

Full verification:

```
cargo build --workspace --tests                         → clean
(cd bpmn-lite-engine/fuzz && cargo build --tests)        → clean
cargo test -p bpmn-lite-kernel --lib                     → 49/49
cargo test -p bpmn-lite-engine --lib                     → 79/79
cargo test -p bpmn-lite-store --lib                      → 37/37
BPMN_LITE_TEST_DATABASE_URL=... cargo test \
  -p bpmn-lite-store-postgres --lib -- --test-threads=1  → 98/98
BPMN_LITE_TEST_DATABASE_URL=... cargo test \
  -p bpmn-lite-server-runner --test integration          → 7/7
cargo clippy --lib --tests --no-deps (engine, store-postgres) → clean
```

**98/98 in `bpmn-lite-store-postgres` — every test in the crate is now
green, including the F-03 baseline that was red for the entire duration
of this remediation effort.**

## Deliberately deferred (not this phase)

- **3D**: removing `claim_running_instances`/`tick_claimed_batch` from
  `RuntimeStore`/`BpmnLiteEngine` now that nothing but tests calls them.
  Left in place pending a bake period for the 3C cutover, per the
  existing Phase 3 doc.
- **Graceful shutdown** (structured cancellation of scheduler/reclaimer/
  pruner tasks, bounded drain, exact-token release on shutdown) — Phase
  4's other sub-section, untouched.
- **Metrics and diagnostics** (activation lifecycle counters, claim-to-
  start latency, fence-vs-revision conflict breakdown, idle-population-
  vs-ready-count) — Phase 4's third sub-section, untouched.
- **Gate 4** itself (two-replica start/run/restart under load, forced-
  termination recovery bound) — requires a real multi-process harness
  this session hasn't built; the fix here is verified at the unit level
  (one contended lease, one replica) not at the gate's full scale.
