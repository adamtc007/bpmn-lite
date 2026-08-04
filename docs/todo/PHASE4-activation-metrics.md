# Phase 4 (partial) — activation-queue lifecycle metrics

Status: **done, partial**. Closes a narrow, load-bearing slice of Phase
4's "Metrics and diagnostics" sub-section: durable activation-queue
lifecycle counters (claimed, consumed, released) from `tick_activated_
batch`, the scheduler's sole consumer of the queue as of Phase 3C. The
plan's fuller list (queue age, claim-to-start latency, kernel/artifact-
load/transaction duration histograms, fence-vs-revision conflict
breakdown, recovery lag, idle-population-vs-ready-count) is **not**
covered — see "Deliberately deferred" below for why and what it would
take.

## What's built

**`bpmn-lite-engine/src/engine.rs`**: `ActivationMetrics` — three
`AtomicU64` counters (`claimed_total`, `consumed_total`,
`released_total`) behind an `Arc`, sharing the exact pattern the
existing `in_flight` single-flight guard already uses: constructed once
in `BpmnLiteEngine::new_with_runtime_context`, cloned (not
reconstructed) into every `for_tenant` handle, so counts aggregate
across every tenant a given engine (or clone of it) has dispatched for.
`ActivationMetricsSnapshot` is the public, plain-data read of the three
counters via `BpmnLiteEngine::activation_metrics()`.

`tick_activated_batch` records:
- `claimed_total` += the batch size `claim_ready_activations` returned,
  once per batch (not once per item — this matches "number claimed", the
  plan's own terminology, explicitly distinct from "number successfully
  ticked").
- `consumed_total` += 1 per activation whose drain succeeded AND whose
  `consume_activation` call itself succeeded (a consume failure after a
  successful drain is logged but does not double-count — see the
  existing `tracing::warn!` there, unchanged by this phase).
- `released_total` += 1 per activation whose drain failed and was
  successfully released back to `ready`.

**`bpmn-lite-server-runner`**: the existing hand-rolled `Metrics` gRPC
RPC (`ServerMetrics`/`MetricsResponse`, the same pattern this codebase
already uses for `job_activations_total` etc. — no Prometheus/metrics
crate in this codebase to route Phase 4's proposed histograms through)
gained three new `uint64` fields (`activations_claimed_total`,
`activations_consumed_total`, `activations_released_total`) in the
proto, wired through `ServerMetrics::snapshot`, which now takes the
engine's `ActivationMetricsSnapshot` as a parameter — `ServerMetrics`
itself has no access to `BpmnLiteEngine`, so the `metrics()` RPC
handler (which holds `self.engine`) supplies it at call time rather
than `ServerMetrics` reaching into the engine on its own.

## Receipts

Two new engine tests
(`tick_activated_batch_records_claimed_and_consumed_metrics`,
`tick_activated_batch_records_released_metric_on_failure`) assert the
counters move by exactly the expected delta for a successful drain and
a failed one respectively, and that success never also increments
`released_total` (or vice versa) — `bpmn-lite-engine --lib` is 81/81
(was 79; +2).

```
cargo build --workspace --tests                                  → clean
(cd bpmn-lite-engine/fuzz && cargo build --tests)                 → clean
cargo test -p bpmn-lite-engine --lib                              → 81/81
BPMN_LITE_TEST_DATABASE_URL=... cargo test \
  -p bpmn-lite-server-runner --bin bpmn-lite-server               → 7/7
BPMN_LITE_TEST_DATABASE_URL=... cargo test \
  -p bpmn-lite-server-runner --test integration                   → 7/7
cargo clippy -p bpmn-lite-engine -p bpmn-lite-server-runner \
  --lib --bins --tests --no-deps                                  → clean
```

Not separately tested at the gRPC wire layer: `ServerMetrics::snapshot`
is a straight-line field copy from the engine's already-tested
`ActivationMetricsSnapshot` into the proto response — low enough risk,
and `test_grpc_smoke` (the one test that would exercise it end-to-end)
only runs against a live server process via `BPMN_LITE_URL`, skipped in
normal local/CI runs.

## Deliberately deferred (not this phase)

- **The rest of Phase 4's metrics list**: activation queue age,
  claim-to-start delay, kernel duration, artifact-load duration,
  transaction duration, fence-vs-revision conflict breakdown, claimed
  work that produced no business transition, expired claims by work
  type, recovery lag, idle-population-vs-ready-activation-count. Every
  one of these is either a duration/histogram (this codebase has no
  metrics/histogram crate — `ServerMetrics` is hand-rolled `AtomicU64`
  counters, adequate for totals, not for latency distributions) or
  requires instrumentation at a layer that currently has no metrics
  hook at all (`commit_transition` in the store crates, which by design
  don't depend on the server-runner's metrics types — the same
  layering reason Phase 3B's `enqueued` count isn't tracked either, per
  `ActivationMetrics`'s own doc comment).
- **`renew_activation_claim`, `dead_letter_activation`, `reclaim_
  expired_activations`** lifecycle events — none of these are called
  from anywhere in the engine yet (renewal isn't wired to a long-
  running tick; nothing currently dead-letters an activation), so
  there's nothing to instrument there until those call sites exist.
- **Choosing and adopting a real metrics/histogram library** (e.g.
  `metrics`, `prometheus`) to replace the hand-rolled `AtomicU64` +
  custom gRPC RPC pattern — a genuine architectural decision (new
  dependency, likely a new `/metrics` HTTP endpoint alongside or
  instead of the gRPC RPC) that should be surfaced and decided, not
  quietly introduced as a side effect of adding three counters.
