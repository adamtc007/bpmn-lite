# Phase 4 (partial) — graceful shutdown

Status: **done**. Closes the "Graceful shutdown" sub-section of Phase 4
(`docs/todo/zed_agent_execution_lease_remediation_plan.md`). Scope:
`bpmn-lite-server-runner/src/main.rs` only — the four background
`tokio::spawn` loops (job reclaim, activation reclaim, scheduler tick,
dedupe prune) and the gRPC server's own request drain. Metrics/
diagnostics and the full Gate 4 multi-replica bake test remain future
Phase 4 work.

## The gap

Before this change, `main()`'s gRPC server had proper graceful shutdown
(tonic's `serve_with_shutdown`, triggered by ctrl_c/SIGTERM) — in-flight
*requests* drained cleanly. But the four background loops were bare
`tokio::spawn(async move { loop { sleep(...); ...work...} })` blocks
with no cancellation path at all: on process exit (forced or otherwise)
whatever iteration was mid-flight was simply abandoned, and readiness
was never explicitly lowered before drain began.

## The fix

Implements the plan's six-step shutdown sequence:

1. **Readiness false first** — `shutdown_signal` (already the trigger
   `serve_with_shutdown` awaits) now calls `health_reporter.set_not_
   serving::<BpmnLiteServer<BpmnLiteService>>()` as its first action on
   ctrl_c/SIGTERM, before anything else — a load balancer stops routing
   new traffic here immediately, concurrently with request drain.
2. **Cancel acquisition loops** — a single `tokio::sync::watch::channel(bool)`
   (`shutdown_tx`/`shutdown_rx`) is created once and cloned into each of
   the four background loops. A new `sleep_or_shutdown(duration,
   &mut rx) -> bool` helper replaces every `tokio::time::sleep(...).await`
   with a `tokio::select!` between the sleep and `rx.wait_for(|v| *v)`:
   returns `true` (proceed) if the sleep elapses first, `false` (stop)
   the instant `shutdown_tx.send(true)` fires — including immediately,
   if a loop happens to be sitting in a 3600s sleep when shutdown is
   requested. Each loop checks the return value and `break`s its `loop`
   instead of starting another round.
3. **Stop accepting new requests** — unchanged, already `serve_with_
   shutdown`'s job; step 1 (readiness false) happens before this
   completes, not after.
4. **Drain in-flight commits for a bounded period** — the four loops'
   `JoinHandle`s are now captured (previously discarded/detached) and,
   after `serve_with_shutdown` returns, awaited via `tokio::time::
   timeout(BPMN_LITE_SHUTDOWN_DRAIN_SECS, join_all(handles))` (default
   10s).
5. **Release exact tokenised claims** — no new code needed: every loop's
   unit of work is already a single fenced claim → commit/act → release
   cycle (the same pattern as everywhere else in this codebase), so a
   loop that isn't mid-work when cancelled holds nothing to release. A
   loop forcibly cut off by the drain-bound timeout *while* mid-work
   leaves at most one claim to expire and reclaim via that work type's
   existing lease-expiry window (`reclaim_stale_jobs`, `reclaim_expired_
   activations`, or the transition lease itself) — the same recovery
   path a hard crash already goes through, not a new failure mode.
6. **Exit** — unchanged; the drain timeout's `Err` branch logs a warning
   and proceeds to exit rather than blocking indefinitely.

## Receipts

`sleep_or_shutdown` is the one piece of this sequence that's unit-
testable outside a running server process (the loops themselves live
inside `main()`). Three tests in `bpmn-lite-server-runner/src/main.rs`'s
`tests_owner` module (run via `--bin bpmn-lite-server`, not `--lib`,
since these live in the binary crate root):

```
sleep_or_shutdown_returns_true_when_the_sleep_elapses_first        ... ok
sleep_or_shutdown_returns_false_the_instant_shutdown_is_signalled  ... ok
sleep_or_shutdown_returns_false_if_the_sender_is_dropped           ... ok
```

The third case matters operationally: if `shutdown_tx` were ever
dropped without sending `true` (a bug, not the intended path — `main`
holds it until the end of the function), `watch::Receiver::wait_for`
returns `Err`, and the helper treats that identically to a real
shutdown signal (fail-safe stop) rather than looping forever on a
closed channel.

Full verification:

```
cargo build --workspace --tests                              → clean
(cd bpmn-lite-engine/fuzz && cargo build --tests)             → clean
cargo build -p bpmn-lite-server-runner --no-default-features  → clean (non-postgres feature path)
BPMN_LITE_TEST_DATABASE_URL=... cargo test \
  -p bpmn-lite-server-runner --bin bpmn-lite-server           → 7/7
BPMN_LITE_TEST_DATABASE_URL=... cargo test \
  -p bpmn-lite-server-runner --test integration               → 7/7
cargo clippy -p bpmn-lite-server-runner --lib --bins --tests --no-deps → clean
```

## Deliberately deferred (not this phase)

- **Metrics and diagnostics** (Phase 4's third sub-section) — activation
  lifecycle counters, claim-busy/renewal/lost-claim counters, queue age,
  kernel/artifact/transaction duration histograms, fence-vs-revision
  conflict breakdown, idle-population-vs-ready-count — none of this
  exists yet.
- **Gate 4 itself** — two-or-more-replica start/run/restart under load,
  and "forced termination recovers within the documented lease/reclaim
  bound" as an actually-measured property, not just an architectural
  claim. This session's verification is unit-level (the cancellation
  primitive is correct); it does not run two real server processes
  against one database and kill one under load.
- **`reclaim_stale_buffered_message_claims`/`prune_expired_messages`**
  — `bpmn-lite-server-runner` doesn't currently run these as background
  loops at all (only `recover_all_tenants` calls the first one, at
  startup); out of scope for a shutdown-sequencing fix, since there's
  nothing running to cancel.
