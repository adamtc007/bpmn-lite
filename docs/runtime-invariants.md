# BPMN-Lite Runtime Invariants

This document is the implementation contract for BPMN-Lite runtime hardening.
It describes behavior the code and tests must preserve before adding more BPMN
surface area.

## INV-001: Store Is The Source Of Truth

Process instance state, fibers, waits, payload history, job state, incidents,
dedupe records, and events are durable store state. In-memory engine state is
only a working copy inside a transition.

Acceptance:

- A process can be reconstructed from store rows after a process crash.
- No correctness-critical runtime state exists only inside a server process.
- `MemoryStore` remains a test and harness implementation, not the HA
  durability model.

## INV-002: One Writer Per Process Instance

No runtime path may mutate an instance unless it owns the instance mutation
right.

Acceptance:

- `start`, `tick`, `signal`, `complete_job`, `fail_job`, `cancel`, timer
  expiry, race resolution, join release, and incident transitions route through
  a guarded transition boundary.
- Concurrent RPC calls against the same instance cannot interleave state
  mutation.
- Scheduler work and load-balanced RPC work share the same mutation invariant.

## INV-003: Job Completion Is Atomic And Idempotent

A job completion must fully commit or be safely replayable.

Acceptance:

- Replaying the same `job_key` returns success without duplicate mutation.
- A crash cannot lose a job while leaving the instance unmutated.
- A crash cannot apply a completion payload twice.
- Completion/failure is accepted only from the worker claim owner once claim
  ownership is enabled.

## INV-004: Fiber PC And Wait State Are Persisted Together

A parked fiber must be persisted with the PC it should resume from when the wait
fires.

Acceptance:

- Timer waits resume after the wait instruction after restart.
- Message waits resume after the wait instruction after restart.
- Race and service-task waits keep their explicitly documented PC semantics.

## INV-005: Tenant Is Part Of Every External Identity Check

Instance IDs and job keys are not sufficient authorization.

Acceptance:

- Public API operations resolve tenant identity from trusted request context.
- Store reads/writes for public operations verify tenant ownership.
- Cross-tenant instance IDs and job keys are rejected.
- Worker activation, completion, and failure are tenant-scoped.

## INV-006: BPMN-Lite Is A Documented Executable Subset

BPMN-Lite is not a full BPMN 2.0.2, Camunda 8, DMN, FEEL, or scripting engine.

Acceptance:

- Supported, rejected, and externalized constructs are documented.
- Unsupported elements produce deterministic compile errors.
- Unsupported constructs do not silently lower to ambiguous runtime behavior.

## INV-007: Rules Do Not Live In The BPM Engine

The engine may route on simple orchestration flags. Rich decisions belong in
external service or decision workers.

Acceptance:

- Branch conditions are restricted to boolean switch semantics.
- Policy, decision tables, scripts, expressions, and numeric comparisons are
  externalized.
- Compiler diagnostics reject expressions that exceed the switch subset.
