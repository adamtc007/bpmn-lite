# Phase 5.5 Plan v0.5: Federated DSL Platform Demo Deployment

| Field | Value |
| --- | --- |
| Document ID | OB-POC-PHASE-5_5-PLAN-005 |
| Version | v0.5 |
| Status | DRAFT — federated DSL platform; gRPC bus; outbox pattern; stored-proc model; crate-disciplined; rip-and-replace by default |
| Author | Adam Cearns |
| Date | 2026-05-20 |
| Supersedes | v0.3 (had in-process tokio channels and shared-process assumption); v0.4 was an interim conversation draft, not landed |
| Position | Post-Phase 5 (`b7c5e5f1`); demo-driven; federated architecture is the production end state |
| Repo topology | bpmn-lite (separate repo, own Postgres, own engine instance); ob-poc (separate repo, own Postgres, own engine instance); shared infrastructure crates consumed by both |
| Deployment | Independent containers per domain; each domain's own Postgres; gRPC bus over HTTP/2 between domains |
| Instruction to Sonnet | Read end-to-end before any tranche work. v0.3 work is partially superseded; per-tranche disposition (§3) specifies what survives, what's mechanical update, what's rip-and-replace. Begin with T0 audit. |

---

## 0. Architectural commitments

Six load-bearing commitments. Every tranche serves them.

### Commitment A: bpmn-dsl source IS the workflow graph

The workflow definition language is bpmn-dsl: s-expression form, same language family as ob-poc DSL. BPMN XML and ob-poc DSL text are two parseable surfaces of the same language family. There is no separate "BPMN AST" lowered to a DAG.

Compilation pipeline: parse (NOM produces AST) → linter (placeholder allocation, type checks, reference resolution) → DAG pass (validate execution order, emit Populated Execution DAG) → ExecutablePlan.

### Commitment B: Two DAGs at two scopes

Long-lived BPMN workflow DAG (process-instance scoped, durable in Postgres, advances on lifecycle events) and short-lived inner DSL plan DAG (one ExecutionFrame, runs to outcome, emits event on commit). The BPMN workflow DAG is what people see "running." The inner DSL plan DAGs are short-running plans submitted across the bus to whichever domain owns the verb.

### Commitment C: Workflow surface declares structure; catalogue resolves bindings

bpmn-dsl declares topology (`:next`), verb/decision identities (`:verb`, `:decision`), and gateway predicates (`:condition`). Placeholder flow (`@cbu`, `@cbu-type`) is *inferred by the compiler* from catalogue declarations. Workflow author never writes binding-flow ceremony.

For demo, single-binding-per-type inference. Multi-binding-per-type (parent + subsidiary CBU via explicit naming) is out of scope.

### Commitment D: Async-by-default; sync is a latency case, not an implementation

One execution path. It is async. The demo exercises the fast case of it.

The BPMN executor never blocks waiting for a callback. All invocations are submitted asynchronously across the bus; the executor returns immediately after persistence; lifecycle events resume the process on arrival via subscriber tasks.

Mechanism: pending-call registry keyed by `execution_id` (the identity issued by the *receiving* domain's engine on plan submission and returned synchronously in the gRPC submission ack). Durable in Postgres. Survives runtime restart. Supports invocations of arbitrary duration (microseconds to days).

### Commitment E: Federated deployment is the architectural target

Each domain (bpmn-lite, ob-poc, future others) is **independently deployable** with its own Postgres database, its own DSL engine instance, its own catalogue, its own audit. Inter-domain calls go through a **gRPC over HTTP/2 bus** carrying typed protobuf messages.

Each domain runs its own Phase 5 engine code (consumed as a shared `dsl-engine-core` library). Domains do not share runtime state. The only inter-domain channel is the bus.

### Commitment F: Stored-procedure model — catalogue manifest is the public API

Each domain publishes a versioned catalogue manifest declaring its **public verb surface**: verb names, typed signatures, effect classes, resource dependencies, FSM applicability, authority requirements. The manifest is the API contract — analogous to a stored procedure registry exposed by a database.

Consumer domains import the manifest at build time. Their compilers validate every cross-domain verb reference against the imported manifest. Internal verbs (not in the manifest) are private to the owning domain.

Bus invocations are typed remote calls against the catalogue: caller has the signature; receiver has the implementation; the wire carries typed args and typed results.

---

## 1. What changed from v0.3 to v0.5

Explicit diff log for Sonnet:

| Topic | v0.3 | v0.5 |
|---|---|---|
| Deployment topology | Implied shared-process or shared-DB | Independent containers per domain; each own Postgres; **rip-and-replace** anything that assumes shared state |
| Inter-domain communication | tokio channels (in-process) | gRPC over HTTP/2 (tonic); typed protobuf |
| Verb namespacing | Implicit; no namespace prefix | Explicit: `ob-poc:cbu.create`, `bpmn-lite:process.start` |
| Catalogue federation | Implicit shared catalogue | Each domain publishes manifest; consumers import at build time |
| Lifecycle event delivery | Tokio mpsc channel from engine to subscriber | gRPC server endpoint on caller; receiver POSTs results back |
| Correlation identity | `execution_id` (issued by single shared engine) | `execution_id` (issued by *receiving* domain's engine; returned via gRPC ack) |
| Retry-safety identity | Conflated with execution_id | `idempotency_key` in gRPC metadata; distinct from `execution_id` |
| Delivery durability | Audit table + tokio channel buffer | Outbox-pattern transactional log in each domain's Postgres |
| Idempotent receive | Implicit via single-process semantics | Explicit inbox table keyed on `idempotency_key` |
| Authority propagation | Not addressed explicitly | Service identity at bus connection + per-call authority context |
| Snapshot pinning | Per-process, within single engine | Per-call across bus; caller pins consumed catalogue version |
| Crate topology | Inherited from existing structure | Explicit shared-infra crates + per-domain crates; **discipline gate per tranche** |
| Refactor posture | Implicit "edit existing code" | **Rip-and-replace by default**; mechanical updates only where explicitly specified |

---

## 2. Per-tranche disposition

Sonnet must read this section before assessing any prior work.

| Tranche | Prior state | Disposition | Notes |
|---|---|---|---|
| **T0** | Audit findings against v0.3 | **NEW** | T0 must be re-run against v0.5 architecture; prior audit answers different questions |
| **T1** | Compilation pipeline (parse/lint/DAG) | **SURVIVES** + **MECHANICAL UPDATE** | Pipeline shape correct. Mechanical update: linter must handle namespaced verb references (`ob-poc:cbu.create`). |
| **T2** | Event publisher + subscriber + pending-call registry (tokio-channel based) | **RIP-AND-REPLACE** | Architecture is wrong (in-process channels, shared-DB). Replaced by: protobuf schemas, gRPC client/server, outbox table, inbox table, manifest import, pending-call by execution_id. |
| **T3** | Async state machine executor | **RIP-AND-REPLACE** | State machine *shape* survives conceptually but submit-step transport changes fundamentally. Rebuild against v0.5 contract. |
| **T4** | Pre-coded demo BPMN model | **SURVIVES** + **MECHANICAL UPDATE** | Model survives. Update: namespaced verb references. |
| **T5** | Sage agentic integration | **PARTIALLY RIP-AND-REPLACE** | Sage internal logic survives. Sage's subscription/integration mechanism (in-process events → bus subscriber) is rip-and-replace. |
| **T6** | ob-poc UI repointing | **SURVIVES** | UI talks to bpmn-lite's local HTTP API. That contract is unchanged. Cross-domain transport is invisible to UI. |
| **T7** | Docker deployment integration | **RIP-AND-REPLACE** | Topology changes from "one container" to "multiple containers + multiple Postgres + gRPC networking". |
| **T8** | Demo polish + rehearsal | **NEW (re-rehearsal)** | Demo flow same; rehearsal must be redone against federated stack. |

**Rip-and-replace discipline:** for tranches marked RIP-AND-REPLACE, Sonnet must not attempt surgical edits of prior code. Delete the prior implementation cleanly (preserve learning in comments/notes for the close note) and rebuild against v0.5 contract.

**Mechanical update discipline:** for tranches marked MECHANICAL UPDATE, Sonnet performs only the explicitly specified textual transformation. No "while I'm here" cleanups.

---

## 3. Pre-locked decisions

1. **Workflow definition language** = s-expression bpmn-dsl. Same compiler as ob-poc DSL.
2. **Condition language** = s-expression predicate subset. Same machinery; same compiler.
3. **Binding convention** = service task `:verb` matches verb id in catalogue (namespaced: `ob-poc:cbu.create`). `:name` is human label only. Unresolved `:verb` = compile error.
4. **Placeholder mechanism** = single-binding-per-type inference (`@cbu` style). Workflow surface stays declaration-free.
5. **Bus transport** = gRPC over HTTP/2 via tonic (Rust crate). Production end state and demo.
6. **Wire format** = protobuf with schema files. Version-pinned per service.
7. **Correlation identity** = `execution_id` (engine-issued by *receiving* domain; returned in gRPC SubmissionAck synchronously; single identity for the call across both sides).
8. **Retry-safety** = `idempotency_key` (UUIDv7, caller-generated, transport-layer dedup; distinct from execution_id).
9. **Delivery durability** = outbox-pattern transactional log per domain. Atomic with business state. Single sender task per domain; not a queue; not a message broker.
10. **Idempotent receive** = inbox table keyed by `idempotency_key`. ON CONFLICT DO NOTHING.
11. **Catalogue manifest** = static YAML file. Generated by owning domain at build time. Imported by consuming domains as build artifact. Version-pinned.
12. **Authority model** = service identity at bus connection (shared secret for demo; OIDC for production); per-call authority context in gRPC metadata.
13. **Snapshot pinning** = caller declares consumed catalogue version in invocation request; receiver validates against current version (warn if drifted; reject if incompatible).
14. **Connection topology** = direct peer-to-peer gRPC, no broker. Domains know each other's endpoints via configuration.
15. **Subscriber model** = each domain owns its own pending-call/outbox/inbox tables. No shared cross-subscriber infrastructure.
16. **Crate discipline** = explicit per-tranche gate. No super-crates. `pub` requires justification. Internal types stay internal.
17. **Refactor discipline** = rip-and-replace by default for LLM-executed work. Mechanical updates permitted only where explicitly specified and bounded.
18. **STOP gates** between every tranche. Sonnet reports; Adam reviews diff; Adam approves commit.

---

## 4. Non-goals (explicit)

To prevent Sonnet drift:

- BPMN XML parser/loader (Stage 2; not this plan)
- Parallel or inclusive gateways
- Boundary events, event sub-processes, multi-instance markers, compensation events
- Timer scheduler, message correlator (sibling services; sized but not built)
- Multi-worker pool, lanes, admission controller (Phase 6 proper)
- Plan persistence (Q9), cross-snapshot replay (Q12), audit retention policy (Q15)
- KYC, UBO, screening, completeness — explicitly removed from demo model
- Multi-binding-per-type placeholder syntax
- Compensation primitive in engine; cancellation scopes; fan-out + WaitN
- Production-hardening of outbox: dead-letter queue, complex retry strategies, orphan reconciliation (basic backoff retry only in v0.5)
- Production authority: full OIDC, role propagation across domains, fine-grained per-verb permissions (service-identity + simple authority context only for demo)
- Verb catalogue extensions beyond what the demo model invokes
- Cross-domain distributed transactions (each invocation is its own transaction; no two-phase commit)
- gRPC streaming (unary calls only for demo; streaming deferred)
- LISTEN/NOTIFY for outbox wake-up (polling sender for demo; LISTEN/NOTIFY deferred)
- TLS / mTLS for gRPC (plaintext for demo; TLS for production deployment)
- NATS or any message broker (direct gRPC; broker is post-demo question)
- Any rework of Phase 5 engine internals — engine code (`b7c5e5f1`) is closed; consumed as library
- Existing ob-poc compiler internals beyond what's needed to expose catalogue and accept namespaced verb refs

If Sonnet's plan touches any of the above, STOP and report — do not implement.

---

## 5. Federated DSL platform architecture

This section is the architectural foundation. v0.5 tranches reference it; the demo Q&A pulls from it.

### 5.1 The stored-procedure analogy

Each domain in the federated platform is analogous to a database server hosting **stored procedures**. The procedures are the domain's DSL verbs. The catalogue manifest is the procedure registry — names, typed signatures, behavioural metadata.

Calling a stored proc across a network requires:
- The caller knowing the proc name (catalogue manifest)
- The caller knowing the proc signature (typed args, typed return)
- A wire protocol carrying typed call + typed result
- Authority propagation (who's calling, with what permissions)
- Idempotent retry semantics
- Result correlation to the original call

The federated DSL platform implements all of these. The architectural pattern is well-understood; this is not novel. What's novel is the application to verb-level DSL operations instead of SQL operations.

### 5.2 Domain as deployment unit

A **domain** is an independently deployable unit of:
- DSL engine (consumed as `dsl-engine-core` library; instantiated per domain)
- Catalogue (verbs the domain implements; published as manifest)
- Postgres (own database; not shared with other domains)
- Authority (own service identity; own user/role model if applicable)
- Audit (own audit trail in own Postgres)
- API (own HTTP/gRPC endpoints; UI access + bus access)

Domains do not share runtime state. The only inter-domain channel is the bus. This is structural failure isolation: a domain can crash, restart, be deployed independently, evolve its schema independently. Other domains continue functioning; their bus calls queue in their outboxes until the down domain recovers.

For the demo: bpmn-lite and ob-poc are two domains. dmn-lite is co-hosted within bpmn-lite (for the demo, simpler; could be its own domain in production).

### 5.3 Catalogue manifest as published contract

Each domain publishes a YAML catalogue manifest. Format (specified fully in §7):

```yaml
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.4.2"
generated_at: "2026-05-20T08:00:00Z"
generated_from_snapshot: "sha256:abc123..."

verbs:
  - id: "cbu.create"
    signature:
      inputs:
        - name: "name"
          type: "String"
        - name: "type"
          type: "CbuType"
      output:
        produces: "CBU"
    effect_class: "idempotent_ensure"
    resource_dependencies:
      - kind: "NaturalKey"
        from_input: "name"
    authority_required: "cbu.write"
    fsm_applicability:
      entity: "CBU"
      transitions: ["NonExistent → Created"]
    
  - id: "cbu.add_fund_product"
    signature:
      inputs:
        - name: "cbu"
          type: "CBU"
      output:
        produces: null
    effect_class: "read_modify_write"
    resource_dependencies:
      - kind: "EntityUuid"
        from_input: "cbu"
    authority_required: "cbu.write"

  # ... all public verbs ...

decisions:
  - id: "cbu_type_routing"
    inputs:
      - name: "cbu"
        type: "CBU"
    output:
      type: "String"
      values: ["fund", "corporate", "trust"]
```

The manifest is *generated*, not handwritten. ob-poc's build process exports its public catalogue surface to YAML. The manifest is checked into a known location (typically a `manifests/` directory in the consuming repo, or fetched from a manifest registry — for the demo, vendored as a build artifact).

Consumer domains' compilers import the manifest. The bpmn-lite compiler validates `ob-poc:cbu.create` against `ob-poc-manifest-v1.4.2.yaml`. Compile-time type checking; compile-time effect-class lookup; compile-time authority-requirement extraction.

### 5.4 Bus as wire protocol

The bus is **gRPC over HTTP/2 via tonic**, carrying typed protobuf messages. Each domain runs:
- gRPC client (initiating outbound invocations to other domains)
- gRPC server (receiving inbound invocations from other domains; receiving inbound results from other domains)

Wire-level message types fully specified in §6. Two service definitions per domain:
- `InvocationService` — receives inbound invocations from peer domains
- `ResultService` — receives inbound results for previously-submitted invocations

### 5.5 Identity model

Four identities from Phase 5 §10.3, applied to cross-domain context:

| Identity | Origin | Used for |
|---|---|---|
| `execution_id` | Receiver domain's engine, on `engine.submit()` | Correlation. Returned in gRPC SubmissionAck. Single identity used by both sides for the rest of the call's lifecycle. |
| `idempotency_key` | Caller domain, before bus call | Retry-safety. In gRPC metadata. Receiver dedupes on this. Distinct from execution_id. |
| `plan_id` | Receiver domain's compiler | Internal to receiver. Audit reference. |
| `attempt_id` | Receiver domain's engine | Internal to receiver. Retry attempts within an execution. |

The caller does NOT generate a separate "invocation_id." execution_id (returned in the SubmissionAck) is the correlation identity for the entire cross-domain call. idempotency_key handles the narrow case of retry-safety before the SubmissionAck completes.

### 5.6 Failure semantics

What happens when:

| Failure | Behaviour |
|---|---|
| Receiver domain down | Caller's outbox entry stays pending; sender task retries with backoff; resumes on reconnect |
| Caller domain down mid-call | Receiver completes work; queues result in receiver's outbox; delivers on caller's reconnect |
| Network partition | Each side keeps state durable; reconvene when network heals |
| Submission ack succeeds but call crashes | execution_id is recorded; receiver completes work; result delivery proceeds normally |
| Result delivery fails after work complete | Receiver's outbox retries result delivery; idempotent on caller side |
| Caller times out waiting for result | BPMN process marked Failed with TimeoutReason; no double-execution (receiver continues; result eventually discarded or logged) |
| Duplicate result delivery (network glitch) | Caller's inbox dedupes by idempotency_key; second delivery is no-op |
| Catalogue version skew (receiver evolved) | Receiver returns VersionMismatch outcome; caller fails the process or retries with newer manifest |

The outbox + inbox pattern handles all of these with single mechanism. Recovery is "read your own log on startup" — no distributed consensus, no two-phase commit, no broker dependency.

### 5.7 Audit federation

Each domain has its own audit trail in its own Postgres (`dsl_execution_audit` per Phase 5 T14). Cross-domain audit join is by `execution_id`:

```sql
-- on bpmn-lite side
SELECT * FROM bpmn_pending_invocation WHERE execution_id = $1;
-- gives target_domain and process_instance_id

-- on ob-poc side (queried via API or admin tool)  
SELECT * FROM dsl_execution_audit WHERE execution_id = $1;
-- gives full audit detail for the execution
```

End-to-end audit for a BPMN process instance:
- Query bpmn-lite's `bpmn_process_instance` and `bpmn_process_callout` tables
- For each callout, look up the corresponding `dsl_execution_audit` row in the *receiving domain's* Postgres
- Join by `execution_id`

For the demo: this is a manual join (or a simple federated query tool). For production: distributed tracing infrastructure (OpenTelemetry) would automate it. Out of scope for v0.5.

### 5.8 Crate topology

The federated platform has the following crate organisation. v0.5 tranches reference these crates by name.

#### Shared infrastructure crates (consumed by all domains)

- **`dsl-engine-core`** — Phase 5 engine code. ExecutionFrame, coordination, transaction policies, audit-as-commit-boundary, effect classes. Owned by Phase 5; closed at `b7c5e5f1`. Consumed as library by all domains.
- **`dsl-bus-protocol`** — protobuf definitions for all bus messages. `InvocationRequest`, `SubmissionAck`, `InvocationResult`, `AuthorityContext`, `LifecycleEvent`. gRPC service traits (tonic-generated). Pure types; no behaviour.
- **`dsl-bus-client`** — gRPC client side of the bus. Outbox sender task. Submission flow (write outbox + send + record execution_id). Used by any domain making outbound calls.
- **`dsl-bus-server`** — gRPC server side of the bus. Inbox receiver. Idempotent dispatch. Used by any domain receiving inbound calls.
- **`dsl-bus-storage`** — outbox and inbox table schemas. SQL migrations. Access layer (CRUD on outbox/inbox).
- **`dsl-manifest`** — catalogue manifest types. Loader. Validator. Used by compilers in any domain importing foreign manifests.

#### Per-domain crates (bpmn-lite)

- **`bpmn-lite-dsl-compiler`** — bpmn-dsl parser, linter, DAG pass. Consumes `dsl-engine-core` for plan emission; consumes `dsl-manifest` for foreign verb resolution.
- **`bpmn-lite-runtime`** — pause/persist/resume state machine. Process instance persistence. Callout dispatch via `dsl-bus-client`. Subscriber for inbound results via `dsl-bus-server`.
- **`bpmn-lite-api`** — HTTP endpoints for UI (process state, audit, lifecycle event stream). Separate from bus endpoints.
- **`bpmn-lite-app`** — binary crate. Wires runtime + api + bus server + bus client. Configuration loading. Startup recovery (outbox/inbox replay).

#### Per-domain crates (ob-poc)

- **`ob-poc-dsl-compiler`** — existing ob-poc compiler. Updated to export manifest at build time.
- **`ob-poc-engine`** — existing ob-poc engine wrapper (instance of `dsl-engine-core` with ob-poc catalogue).
- **`ob-poc-api`** — existing ob-poc HTTP endpoints. Add: manifest export endpoint (read by build pipeline; not runtime).
- **`ob-poc-bus-handler`** — NEW. Implements `InvocationService` gRPC server. Receives inbound invocations from peer domains. Dispatches to engine. Records to inbox. Sends results back via `dsl-bus-client`.
- **`ob-poc-app`** — existing binary. Wires existing components + new bus handler.

#### Crate discipline rules

1. **No super-crates.** A crate's purpose must be statable in one sentence. If it can't be, split it.
2. **Minimal public API per crate.** `pub` requires justification. Default to `pub(crate)` for internal helpers; `pub` only for the actual public interface.
3. **No convenience re-exports.** Don't re-export types from one crate via another crate to "make imports shorter." Each crate's public surface is its own.
4. **Explicit cross-crate deps.** Cargo.toml declares every dep used. No transitive reliance.
5. **No circular deps.** Crate A depending on crate B prohibits B from depending on A.
6. **Test code stays internal.** Test helpers, fixtures, mock implementations are `pub(crate)` or in test modules. They do not leak into the public API.
7. **No "expose for testing" exports.** If a test needs access to an internal type, the test goes in the same crate as the type (in a `tests` module).
8. **Each tranche reports new `pub` additions.** Sonnet's tranche DoD includes a line listing every new `pub` item added. Adam reviews; rejects any not justified.

These rules are enforceable by the gating in §9 (per-tranche execution conventions).

---

## 6. Bus protocol specification (inline)

protobuf v3. File: `dsl_bus.proto` (consumed by `dsl-bus-protocol` crate; tonic generates Rust code at build).

### 6.1 Common types

```protobuf
syntax = "proto3";
package dsl.bus.v1;

// Identities

message ExecutionId {
  bytes uuid = 1;  // UUIDv7 as bytes
}

message PlanId {
  bytes uuid = 1;
}

message IdempotencyKey {
  bytes uuid = 1;  // UUIDv7
}

message SnapshotId {
  bytes uuid = 1;
}

// Authority

message AuthorityContext {
  string service_identity = 1;    // calling service (e.g., "bpmn-lite")
  string user_identity = 2;       // end user (optional; demo uses "demo-user")
  repeated string roles = 3;
  bytes signed_token = 4;         // for production; empty for demo
}

// Bindings (typed args and resolved values)

message TypedValue {
  oneof value {
    string string_value = 1;
    int64 int_value = 2;
    double double_value = 3;
    bool bool_value = 4;
    bytes uuid_value = 5;
    bytes blob_value = 6;          // for complex serialised types
    NullValue null_value = 7;
  }
  string type_name = 10;            // type discriminant for validation
}

message NullValue {}

message ResolvedBinding {
  string name = 1;
  TypedValue value = 2;
}

// Outcomes (from Phase 5)

enum ExecutionOutcomeKind {
  OUTCOME_UNSPECIFIED = 0;
  COMMITTED = 1;
  IDEMPOTENT_REPLAY_RETURNED = 2;
  OPTIMISTIC_CONFLICT = 3;
  LOCK_TIMEOUT = 4;
  VERB_FAILED = 5;
  AUTHORITY_DENIED = 6;
  CANCELLED = 7;
  TIMED_OUT = 8;
  PANIC_RECOVERED = 9;
  REJECTED_BY_ADMISSION = 10;
  VERSION_MISMATCH = 11;            // catalogue version skew
}

message ExecutionOutcome {
  ExecutionOutcomeKind kind = 1;
  string detail = 2;                // structured detail per kind
  repeated ResolvedBinding bindings = 3;
}
```

### 6.2 InvocationService — receives inbound invocations

```protobuf
service InvocationService {
  // Submit an invocation. Returns synchronously with the execution_id
  // assigned by the receiving domain's engine. Actual execution proceeds async.
  // The result is delivered later via ResultService on the caller's side.
  rpc Submit(InvocationRequest) returns (SubmissionAck);
}

message InvocationRequest {
  // Identity
  IdempotencyKey idempotency_key = 1;   // caller-generated for retry safety
  
  // What to invoke
  string verb_id = 2;                    // e.g., "cbu.create"
  repeated ResolvedBinding inputs = 3;
  
  // Context
  AuthorityContext authority = 4;
  string source_domain = 5;              // e.g., "bpmn-lite"
  string catalogue_version = 6;          // version caller compiled against
  SnapshotId snapshot_pin = 7;           // caller's pinned snapshot if applicable
  
  // Delivery
  string result_callback_endpoint = 8;   // URL where result should be sent
  
  // Optional
  google.protobuf.Timestamp timeout_at = 9;
}

message SubmissionAck {
  ExecutionId execution_id = 1;          // engine-issued; the correlation identity
  SubmissionStatus status = 2;
  string detail = 3;                      // if status != ACCEPTED
}

enum SubmissionStatus {
  SUBMISSION_UNSPECIFIED = 0;
  ACCEPTED = 1;                           // invocation queued for execution
  DUPLICATE = 2;                          // idempotency_key already seen; existing execution_id returned
  REJECTED_VERB_UNKNOWN = 3;
  REJECTED_VERSION_INCOMPATIBLE = 4;
  REJECTED_AUTHORITY = 5;
  REJECTED_MALFORMED = 6;
}
```

### 6.3 ResultService — receives inbound results

```protobuf
service ResultService {
  // Deliver the result of a previously-submitted invocation.
  // Called by the domain that executed the verb, addressed to the caller.
  rpc DeliverResult(InvocationResult) returns (ResultAck);
}

message InvocationResult {
  // Correlation
  ExecutionId execution_id = 1;          // echoes the SubmissionAck's execution_id
  IdempotencyKey idempotency_key = 2;    // for caller-side dedup
  
  // Outcome
  ExecutionOutcome outcome = 3;
  
  // Provenance
  string source_domain = 4;              // domain that executed
  google.protobuf.Timestamp executed_at = 5;
  PlanId plan_id = 6;                    // for audit linkage
  
  // Optional details
  string audit_reference = 7;             // pointer into source's audit
}

message ResultAck {
  ReceiptStatus status = 1;
  string detail = 2;
}

enum ReceiptStatus {
  RECEIPT_UNSPECIFIED = 0;
  RECEIVED = 1;                           // first-time receipt; processing
  DUPLICATE_IGNORED = 2;                  // already received this idempotency_key
  REJECTED_UNKNOWN_EXECUTION = 3;         // no pending invocation for this execution_id
}
```

### 6.4 Wire-level conventions

- **Encoding:** protobuf binary
- **Transport:** HTTP/2 over TCP; gRPC framing via tonic
- **Compression:** gzip enabled at gRPC level
- **TLS:** disabled for demo (plaintext); enabled for production deployment
- **Timeouts:** Submit: 5 seconds. DeliverResult: 5 seconds. Caller-side timeout for full invocation: configurable per-call (default 60 seconds; demo uses 30 seconds).
- **Retries:** outbox-driven exponential backoff (1s, 2s, 4s, ..., capped at 60s). gRPC client does not retry internally; outbox handles all retries.
- **Authority verification:** `service_identity` field validated against allowlist on receiver. For demo, single allowlist entry (`bpmn-lite ↔ ob-poc`).
- **Catalogue version validation:** receiver compares `catalogue_version` against its current published manifest version. If exact match: proceed. If caller is older: proceed (forward compatibility). If caller is newer than receiver: reject with `REJECTED_VERSION_INCOMPATIBLE`.

---

## 7. Catalogue manifest specification (inline)

YAML format. Generated by owning domain at build time. Imported by consuming domains.

### 7.1 Top-level structure

```yaml
manifest_version: "1.0"               # manifest format version (not the domain's catalogue version)
domain: "ob-poc"                       # domain identifier
catalogue_version: "v1.4.2"            # semantic version of this domain's catalogue
generated_at: "2026-05-20T08:00:00Z"
generated_from_snapshot: "sha256:abc123..."  # which SemOS snapshot produced this manifest

# Compatibility
min_consumer_manifest_version: "1.0"
breaking_changes_since: []             # list of catalogue_versions where breaking changes occurred

# Public verb surface
verbs:
  - <verb entry>
  - ...

# Public decisions (DMN)
decisions:
  - <decision entry>
  - ...

# Type definitions (custom types referenced by verbs)
types:
  - <type entry>
  - ...
```

### 7.2 Verb entry

```yaml
verbs:
  - id: "cbu.create"
    
    # Signature
    signature:
      inputs:
        - name: "name"
          type: "String"
          required: true
        - name: "type"
          type: "CbuType"
          required: true
          enum_values: ["fund", "corporate", "trust"]
      output:
        produces: "CBU"             # type of binding produced, or null
    
    # Behavioural metadata
    effect_class: "idempotent_ensure"   # from Phase 5 effect class taxonomy
    coordination_policy: "UniqueInsert"  # implied by effect_class but explicit for clarity
    transaction_policy: "AtomicShort"
    
    # Resource dependencies
    resource_dependencies:
      - kind: "NaturalKey"
        from_input: "name"
        entity_type: "CBU"
    
    # FSM applicability
    fsm_applicability:
      entity: "CBU"
      preconditions: ["NotExists"]
      postconditions: ["Created"]
    
    # Authority
    authority_required: "cbu.write"      # permission scope required to invoke
    
    # Documentation
    description: "Create a new CBU entity, idempotent on natural key (name)."
    examples:
      - "(service-task :id create-cbu :verb ob-poc:cbu.create :inputs (name \"ACME-LTD\" type \"corporate\"))"
```

### 7.3 Decision entry (DMN)

```yaml
decisions:
  - id: "cbu_type_routing"
    
    inputs:
      - name: "cbu"
        type: "CBU"
        required: true
    
    output:
      type: "String"
      enum_values: ["fund", "corporate", "trust"]
    
    description: "Routes CBU to appropriate product attachment path based on entity type."
```

### 7.4 Type entry

```yaml
types:
  - name: "CBU"
    kind: "entity"
    description: "Custody Banking Unit — operating arrangement a client has on the street."
    uuid_type: "UUIDv7"
  
  - name: "CbuType"
    kind: "enum"
    values: ["fund", "corporate", "trust"]
```

### 7.5 Manifest lifecycle

- **Generation:** owning domain's build pipeline exports manifest from its catalogue. ob-poc has a `manifest-export` binary or build script that produces `ob-poc-manifest-v1.4.2.yaml`.
- **Publication:** for demo, manifest is vendored into the bpmn-lite repo at `bpmn-lite/manifests/ob-poc-v1.4.2.yaml`. For production, manifests would be published to a registry (artifactory, S3, etc.).
- **Import:** bpmn-lite's compiler loads the manifest at build time. Validates structure. Caches verb/decision lookups indexed by id.
- **Validation:** at compile time, every `ob-poc:cbu.create` reference in bpmn-dsl is validated against the imported manifest. Unknown verbs = compile error. Type mismatches = compile error.
- **Version pinning:** bpmn-lite records which manifest version it compiled against. The catalogue_version is carried in every InvocationRequest. Receiver validates.

### 7.6 Generation discipline

For ob-poc specifically: a build-time step generates the manifest from the SemOS catalogue. The generator reads `dsl_verb_catalogue` (or equivalent) tables, filters to public verbs (an explicit `is_public` flag or equivalent — to be confirmed by T0 audit), and emits the YAML.

For the demo: this generation must produce a valid manifest for the verbs in §8 (`cbu.create`, `cbu.add_fund_product`, `cbu.add_corporate_product`, `cbu.add_trust_product`, `cbu.add_instrument_matrix`) plus the `cbu_type_routing` DMN decision.

If T0 finds the catalogue doesn't have a public/private distinction today, the demo seed can mark all relevant verbs as public; full public/private discipline is a follow-up enhancement.

---

## 8. Outbox and inbox specifications (inline)

### 8.1 Outbox table (each domain has its own)

```sql
-- bpmn-lite side: outbox for outbound calls to ob-poc and result-acks
-- ob-poc side: outbox for inbound-call-results back to callers

CREATE TABLE outbox (
  id UUID PRIMARY KEY,                       -- internal entry id (UUIDv7)
  
  -- What's being sent
  target_domain TEXT NOT NULL,               -- e.g., "ob-poc"
  target_endpoint TEXT NOT NULL,             -- "invocation" | "result"
  payload BYTEA NOT NULL,                     -- protobuf-encoded message
  
  -- Correlation
  idempotency_key UUID NOT NULL,             -- caller-generated (for invocations)
  execution_id UUID,                          -- nullable; filled after SubmissionAck
  
  -- State
  status TEXT NOT NULL DEFAULT 'pending',    -- pending | submitted | failed
  attempt_count INT NOT NULL DEFAULT 0,
  next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_error TEXT,
  
  -- Lifecycle
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  submitted_at TIMESTAMPTZ,
  
  -- Indexing
  UNIQUE (idempotency_key, target_endpoint)
);

CREATE INDEX idx_outbox_pending 
  ON outbox(next_attempt_at) 
  WHERE status = 'pending';

CREATE INDEX idx_outbox_target 
  ON outbox(target_domain, status);
```

### 8.2 Inbox table (each domain has its own)

```sql
CREATE TABLE inbox (
  idempotency_key UUID PRIMARY KEY,          -- caller-generated; dedup key
  
  -- What was received
  source_domain TEXT NOT NULL,               -- which domain sent this
  endpoint TEXT NOT NULL,                     -- "invocation" | "result"
  
  -- Correlation
  execution_id UUID,                          -- engine-issued (for invocations we received)
  
  -- State
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  processed_at TIMESTAMPTZ,
  status TEXT NOT NULL DEFAULT 'received',   -- received | processed | failed
  
  -- Audit
  payload BYTEA                               -- optional; retain for audit/debug
);

CREATE INDEX idx_inbox_source 
  ON inbox(source_domain, received_at);
```

### 8.3 bpmn-lite pending-call table

```sql
CREATE TABLE bpmn_pending_invocation (
  execution_id UUID PRIMARY KEY,             -- engine-issued by receiving domain
  
  -- Where we are in the workflow
  process_instance_id UUID NOT NULL,
  node_id TEXT NOT NULL,
  
  -- What's running
  target_domain TEXT NOT NULL,               -- e.g., "ob-poc"
  verb_id TEXT NOT NULL,                      -- e.g., "ob-poc:cbu.create"
  
  -- Correlation
  idempotency_key UUID NOT NULL,             -- ours, generated when submitting
  
  -- Lifecycle
  submitted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  timeout_at TIMESTAMPTZ,
  
  UNIQUE (idempotency_key)
);

CREATE INDEX idx_pending_process 
  ON bpmn_pending_invocation(process_instance_id);

CREATE INDEX idx_pending_timeout 
  ON bpmn_pending_invocation(timeout_at) 
  WHERE timeout_at IS NOT NULL;
```

### 8.4 BPMN process instance table

```sql
CREATE TABLE bpmn_process_instance (
  id UUID PRIMARY KEY,
  workflow_id TEXT NOT NULL,                 -- which workflow definition
  
  -- State
  current_node TEXT NOT NULL,
  status TEXT NOT NULL,                       -- Created | Running | WaitingOnInvocation | Completed | Failed
  variables JSONB NOT NULL DEFAULT '{}',
  
  -- Pending-call linkage
  waiting_on_execution_id UUID,              -- nullable; set when status = WaitingOnInvocation
  
  -- Lifecycle
  started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_advanced_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  completed_at TIMESTAMPTZ,
  
  -- Result
  end_status TEXT,                            -- "CBU Operational" | "Failed" | etc.
  failure_reason TEXT
);
```

### 8.5 Outbox sender flow

```rust
// Pseudocode — runs as background task per domain
async fn outbox_sender_loop(
  pool: PgPool, 
  grpc_clients: HashMap<String, InvocationServiceClient>,
  result_clients: HashMap<String, ResultServiceClient>,
) {
  loop {
    // Pick up pending entries with next_attempt_at <= now
    let entries: Vec<OutboxEntry> = sqlx::query_as!(
      OutboxEntry,
      "SELECT * FROM outbox 
       WHERE status = 'pending' AND next_attempt_at <= now() 
       ORDER BY created_at 
       FOR UPDATE SKIP LOCKED 
       LIMIT 10"
    ).fetch_all(&pool).await.unwrap_or_default();
    
    for entry in entries {
      let result = match entry.target_endpoint.as_str() {
        "invocation" => {
          let req = InvocationRequest::decode(&entry.payload[..])?;
          let client = grpc_clients.get(&entry.target_domain).unwrap();
          client.submit(req).await
        }
        "result" => {
          let res = InvocationResult::decode(&entry.payload[..])?;
          let client = result_clients.get(&entry.target_domain).unwrap();
          client.deliver_result(res).await
        }
        _ => { /* error */ continue; }
      };
      
      match result {
        Ok(response) => {
          // Mark sent; for invocations, record the execution_id from the ack
          let execution_id = extract_execution_id(&response);
          sqlx::query!(
            "UPDATE outbox 
             SET status = 'submitted', submitted_at = now(), execution_id = $1 
             WHERE id = $2",
            execution_id, entry.id
          ).execute(&pool).await?;
          
          // If invocation, also insert the pending-call row
          if entry.target_endpoint == "invocation" {
            // ... insert bpmn_pending_invocation ...
          }
        }
        Err(e) => {
          // Backoff with exponential retry
          let backoff_secs = std::cmp::min(2_i32.pow(entry.attempt_count as u32), 60);
          sqlx::query!(
            "UPDATE outbox 
             SET attempt_count = attempt_count + 1, 
                 next_attempt_at = now() + ($1 || ' seconds')::interval,
                 last_error = $2
             WHERE id = $3",
            backoff_secs.to_string(), e.to_string(), entry.id
          ).execute(&pool).await?;
        }
      }
    }
    
    tokio::time::sleep(Duration::from_millis(100)).await;
  }
}
```

### 8.6 Inbox receive flow

```rust
// Pseudocode — gRPC handler in InvocationService
async fn handle_submit(req: InvocationRequest) -> Result<SubmissionAck> {
  let idem_key = uuid_from(&req.idempotency_key);
  
  // Idempotent receive: check inbox
  let existing: Option<InboxEntry> = sqlx::query_as!(
    InboxEntry,
    "SELECT * FROM inbox WHERE idempotency_key = $1",
    idem_key
  ).fetch_optional(&pool).await?;
  
  if let Some(existing) = existing {
    // Duplicate; return existing execution_id
    return Ok(SubmissionAck {
      execution_id: existing.execution_id.into(),
      status: SubmissionStatus::Duplicate as i32,
      detail: String::new(),
    });
  }
  
  // Validate verb, authority, version
  validate_invocation(&req)?;
  
  // Compile inner plan; submit to local engine
  let plan = compile_inner_plan(&req)?;
  let execution_id = engine.submit_async(plan).await?;
  
  // Atomically: record inbox + insert any necessary local state
  let mut tx = pool.begin().await?;
  sqlx::query!(
    "INSERT INTO inbox (idempotency_key, source_domain, endpoint, execution_id, payload) 
     VALUES ($1, $2, 'invocation', $3, $4)",
    idem_key, req.source_domain, execution_id, req.encode_to_vec()
  ).execute(&mut tx).await?;
  tx.commit().await?;
  
  Ok(SubmissionAck {
    execution_id: execution_id.into(),
    status: SubmissionStatus::Accepted as i32,
    detail: String::new(),
  })
}
```

### 8.7 Recovery on startup

```rust
async fn startup_recovery(pool: &PgPool) -> Result<()> {
  // 1. Outbox: nothing to do — the sender loop will pick up pending entries
  
  // 2. Inbox: identify any 'received' but not 'processed' entries
  //    These are invocations we accepted but didn't finish processing
  //    For each: re-trigger the local execution flow
  let stuck_inbox: Vec<InboxEntry> = sqlx::query_as!(
    InboxEntry,
    "SELECT * FROM inbox WHERE status = 'received' AND endpoint = 'invocation'"
  ).fetch_all(pool).await?;
  
  for entry in stuck_inbox {
    // Re-trigger execution; the engine handles its own restart recovery for the actual plan
    // If the engine had already completed this execution_id, the audit table has the record
    // and we just need to deliver the result; the outbox-side will handle that
    re_trigger_execution(entry.execution_id).await?;
  }
  
  // 3. Pending invocations (bpmn-lite only): identify processes that are WaitingOnInvocation
  //    Nothing to do here — they'll resume when their result arrives via gRPC
  //    Or timeout if their timeout_at passes
  
  Ok(())
}
```

---

## 9. Tranches

### T0 — Audit (NO CODE)

**Goal:** establish ground truth against v0.5 architecture.

**Disposition:** NEW. Prior T0 (v0.3) answered different questions; redo against v0.5.

**Crates touched:** none (audit only).

**Sonnet tasks:**

#### Repository and crate state

1. **bpmn-lite repo structure:** what crates exist; what's in each; what's in Cargo.toml
2. **ob-poc repo structure:** crate organisation; how engine code is currently packaged (is `dsl-engine-core` already a separate crate, or is engine code inside ob-poc as a module?)
3. **Shared workspace structure:** is there a shared workspace consumed by both repos? Or do they have independent Cargo workspaces with dependency relationships?
4. **Existing crate disciplines:** how strict is current `pub` usage? Any obvious super-crates? Any obvious leak points?

#### bpmn-dsl pipeline state

5. **Parser:** NOM-based parser exists? What AST does it produce? Test pack contents.
6. **Linter:** does any linter pass exist today? Does it handle binding flow / placeholder allocation at all?
7. **DAG pass:** does compilation produce a Populated Execution DAG today? Does Phase 5 validator accept it?
8. **Namespaced verb refs:** does the current parser/linter handle `ob-poc:cbu.create`-style namespaced ids? Or does it assume flat verb names?

#### Existing v0.3 work (to be ripped/replaced)

9. **T2 work landed:** what exactly was built in T2? List the crates, files, schemas. This identifies what gets ripped.
10. **T3 work landed:** what exactly was built in T3? List the crates, files, schemas, executor structure.
11. **In-process channel usage:** are there tokio channels between engine and bpmn-lite today? Where? How are they wired?
12. **Pending-call table (if exists):** what schema? What identity key?

#### Bus and federation prerequisites

13. **gRPC / tonic usage:** does either codebase already use tonic? Any existing gRPC services? Any protobuf files?
14. **Inter-domain communication today:** how does bpmn-lite invoke ob-poc functionality (if at all)? Function call? HTTP? Shared DB?
15. **Authority / service identity:** is there any existing service-identity infrastructure? Configuration patterns for cross-service auth?
16. **Catalogue export:** does ob-poc have any existing manifest/schema export? Or is this entirely new construction?

#### Engine integration

17. **`dsl-engine-core` extraction status:** is Phase 5 engine code already a separately-consumable crate? Or do bpmn-lite and ob-poc need it extracted into one before they can both consume it?
18. **Engine submit API:** does `engine.submit_async()` returning `execution_id` synchronously exist today? Or is the API blocking-on-completion?
19. **Lifecycle event emission:** does the engine emit events today on commit? Or is `dsl_execution_audit` the only artifact?

#### Demo verb catalogue

20. **CBU verbs in ob-poc catalogue:** `cbu.create`, `cbu.add_fund_product`, `cbu.add_corporate_product`, `cbu.add_trust_product`, `cbu.add_instrument_matrix`. Present? Effect classes appropriate per §5.3 manifest format? Any signature mismatches with what the demo model expects?
21. **DMN decision:** `cbu_type_routing` exists in dmn-lite? Input/output schema appropriate?
22. **Public/private verb distinction:** does the catalogue have any concept of "public vs internal" verbs today? Or are all verbs uniformly accessible?

#### Deployment

23. **Current Docker compose:** what services run, what networks, what volumes, what ports?
24. **Postgres topology:** one Postgres for both bpmn-lite and ob-poc today, or separate?
25. **Environment configuration:** how is per-service config managed (env vars, config files, secrets)?

#### UI integration

26. **ob-poc UI current state:** what APIs does it talk to? What components exist?
27. **T6 work landed:** what UI work was done against v0.3? Is it pointed at bpmn-lite already? What works, what doesn't?

**DoD:** structured findings report. Each of the 27 items has a clear answer. Particular attention to items 9-12 (what to rip) and items 17-19 (engine integration prerequisites). No code changes. Adam reviews; identifies gap-to-tranche mapping; locks in any final scope adjustments for T1 onwards.

**STOP gate.** Do not commit anything. Report findings and wait.

---

### T1 — bpmn-dsl compilation pipeline (parse / lint / DAG)

**Goal:** bpmn-dsl source compiles through the parse / lint / DAG pipeline, with `@cbu` placeholder inference and namespaced foreign verb resolution.

**Disposition:** SURVIVES + MECHANICAL UPDATE. Pipeline shape correct from v0.3 work. Update: linter must handle namespaced verb references against imported manifest.

**Crates touched:**
- `bpmn-lite-dsl-compiler` (existing; may need minor restructure if T0 reveals organisational issues)
- `dsl-manifest` (NEW shared crate — manifest types, loader, validator)

**Sonnet tasks:**

1. **Create `dsl-manifest` crate** (NEW shared crate):
   - Types matching §7 spec (Manifest, VerbEntry, DecisionEntry, TypeEntry)
   - YAML loader (serde_yaml)
   - Validator (checks manifest_version, structural correctness)
   - Lookup API: `manifest.lookup_verb(id) → Option<&VerbEntry>`
   - **Crate discipline:** pub surface is Manifest type + load function + lookup methods. Internal validation is `pub(crate)`. No re-exports of serde_yaml internals.

2. **bpmn-lite compiler updates:**
   - **Linter pass — namespaced verb resolution:**
     - Recognise `domain:verb` syntax (e.g., `ob-poc:cbu.create`)
     - Determine domain from prefix
     - For native domain (bpmn-lite's own verbs): resolve against local catalogue
     - For foreign domain: resolve against imported manifest (via `dsl-manifest`)
     - Imported manifests are loaded at compile time from a known location (`bpmn-lite/manifests/<domain>-<version>.yaml`)
   - **Linter pass — placeholder inference** (mechanical from v0.3):
     - Walk workflow; identify each service-task/business-rule-task by `:verb` or `:decision`
     - Look up each in catalogue/manifest; retrieve produces/consumes binding type declarations
     - Allocate placeholder slot per produced binding type (single-binding-per-type)
     - Thread placeholder slot to downstream consumers along DAG edges
     - Validate type consistency
   - **Linter pass — unresolved references:**
     - Every `:verb` resolves to a catalogue entry or manifest entry
     - Every `:decision` resolves to a dmn-lite catalogue entry
     - Every `:next` resolves to a defined node
     - Every gateway predicate references defined placeholder slots
   - **DAG pass** (mechanical from v0.3):
     - Topology from `:next` and gateway flows
     - Acyclic validation
     - Resource dependencies from catalogue/manifest declarations
     - Effect class from catalogue/manifest
     - Concurrency policy from effect class
   - **Compiler output:** ExecutablePlan submittable to local engine OR marked as cross-domain (target_domain populated) for the bus path

3. **Tests:**
   - Parse §10 demo model successfully
   - Linter resolves namespaced verbs against imported manifest
   - Linter rejects unknown verb (foreign or local)
   - Linter rejects type mismatch on placeholders
   - DAG pass produces valid DAG for §10 model
   - All five distinct end-to-end paths (fund × corporate × trust × instrument matrix) produce valid DAGs

4. **Crate discipline DoD:**
   - List every new `pub` item added. Justify each.
   - No new super-crate creation.
   - `dsl-manifest` has minimal pub surface (Manifest, ManifestError, load_manifest).

**DoD:** §10 model compiles cleanly to a Populated Execution DAG. Manifest import works. Namespaced verbs resolve correctly. Phase 5 validator accepts the DAG. Placeholder inference works without explicit declarations.

**STOP gate.**

---

### T2 — Bus infrastructure (RIP-AND-REPLACE)

**Goal:** gRPC bus, outbox/inbox tables, manifest publication, manifest import, all working end-to-end for a single test invocation.

**Disposition:** RIP-AND-REPLACE. All v0.3 T2 work (tokio channels, in-process subscriber, shared-DB pending-call) is deleted. Replaced by federated architecture.

**Crates touched (mostly NEW):**
- `dsl-bus-protocol` (NEW): protobuf definitions, tonic-generated traits
- `dsl-bus-client` (NEW): gRPC client, outbox sender
- `dsl-bus-server` (NEW): gRPC server, inbox handler
- `dsl-bus-storage` (NEW): outbox/inbox table schemas, access layer
- `ob-poc-bus-handler` (NEW per-domain): ob-poc's InvocationService implementation
- `bpmn-lite-runtime` (modify): use `dsl-bus-client` for outbound calls
- `bpmn-lite-app` (modify): wire bus client/server on startup
- `ob-poc-app` (modify): wire bus handler on startup

**Sonnet tasks:**

#### T2.1 — Rip prior v0.3 T2/T3 work

1. **Identify everything from prior T2/T3:** crates, files, schemas, tests. List exhaustively.
2. **Delete it cleanly:** no surgical preservation. Move to a `_deprecated/` directory or git-rm outright (Adam's call per the diff review).
3. **Document what was learned** (close note material): one-page note of "what we built in v0.3 T2/T3, what didn't work, what we kept conceptually." Goes into the Phase 5.5 close-note bucket.
4. **STOP gate before proceeding.** Adam reviews the rip; confirms nothing essential is in the deletion set.

#### T2.2 — Create `dsl-bus-protocol` crate

5. Write `dsl_bus.proto` per §6 spec exactly. All message types, both services.
6. Configure tonic build (`build.rs`) to generate Rust code from proto.
7. Pub surface: generated types and traits only. No additional handcrafted types.
8. Tests: verify generated code compiles; basic encode/decode round-trip per message type.

#### T2.3 — Create `dsl-bus-storage` crate

9. SQL migrations for outbox and inbox tables per §8.1, §8.2.
10. Rust types matching the table schemas (OutboxEntry, InboxEntry).
11. CRUD operations: `insert_outbox`, `select_pending_outbox`, `mark_outbox_submitted`, `insert_inbox` (with idempotent ON CONFLICT), `select_inbox`, `mark_inbox_processed`.
12. Pub surface: types and operations. Internal SQL is `pub(crate)`. No raw SQL exposed externally.
13. Tests: each CRUD operation; idempotent inbox insertion; outbox status transitions.

#### T2.4 — Create `dsl-bus-client` crate

14. gRPC client wrapper (tonic InvocationServiceClient, ResultServiceClient).
15. Outbox sender task: per §8.5. Background loop polling outbox, sending, marking submitted, backoff on failure.
16. Submission flow: `submit_invocation(target_domain, request) → Result<ExecutionId>`:
    - Insert outbox entry with idempotency_key, target_domain="ob-poc", target_endpoint="invocation", payload=request.encode_to_vec()
    - Sender task picks it up, calls Submit RPC, receives SubmissionAck with execution_id
    - Update outbox: status='submitted', execution_id=ack.execution_id
    - Return execution_id to caller
17. Result-send flow (for receiver-domain to send results back): `send_result(target_domain, result) → Result<()>`:
    - Insert outbox entry with target_endpoint="result"
    - Sender task delivers via DeliverResult RPC
18. Pub surface: `BusClient` (struct with submit_invocation, send_result methods), `BusClientConfig`, error types. Internal sender task is `pub(crate)`.
19. Tests: submission round-trip with mock gRPC server, sender backoff on error, idempotent submission (same idempotency_key returns same execution_id).

#### T2.5 — Create `dsl-bus-server` crate

20. gRPC server impl (tonic).
21. InvocationService implementation: per §8.6.
    - Idempotent check against inbox
    - Validate verb, authority, version
    - Compile inner plan (via consumer-provided callback)
    - Submit to local engine
    - Insert inbox row
    - Return SubmissionAck
22. ResultService implementation:
    - Idempotent check
    - Look up corresponding bpmn_pending_invocation by execution_id
    - Insert inbox row
    - Invoke consumer-provided callback (e.g., `bpmn-lite-runtime`'s advance() function)
    - Return ResultAck
23. Pub surface: `BusServer` struct with builder pattern for configuration. Service implementations are `pub(crate)` (consumed via the builder).
24. Tests: invocation round-trip with mock engine, idempotent receive, version mismatch handling, malformed request rejection.

#### T2.6 — Create `ob-poc-bus-handler` crate

25. Implements `dsl-bus-server`'s InvocationService trait for ob-poc:
    - Compile callback: invoke ob-poc compiler with verb_id and inputs
    - Engine callback: invoke ob-poc's `dsl-engine-core` instance
    - Result-sending callback: when engine completes, format InvocationResult and send back via `dsl-bus-client`
26. Subscribes to ob-poc engine's lifecycle events (whatever mechanism T0 reveals; if events don't exist, wire up via Phase 5 audit-table polling).
27. On engine completion: insert result into outbox for delivery to caller.
28. Pub surface: `ObPocBusHandler` struct + start() function. Internal callback wiring is `pub(crate)`.
29. Tests: invocation arrives, dispatches to mock engine, mock engine completes, result enqueued in outbox.

#### T2.7 — bpmn-lite-runtime integration

30. **bpmn_pending_invocation table:** per §8.3. Migration.
31. **bpmn_process_instance table:** per §8.4. Migration.
32. **Submission integration:** when bpmn-lite executor reaches a foreign-domain service-task, use `dsl-bus-client.submit_invocation(target_domain, request)`. Insert pending-call row keyed by returned execution_id.
33. **Result reception:** wire `dsl-bus-server`'s ResultService consumer callback to bpmn-lite executor's advance() function.
34. **Recovery:** on startup, query for in-flight pending invocations (they'll resume when results arrive or timeout).

#### T2.8 — ob-poc manifest export

35. ob-poc gets a `manifest-export` binary or build script (under `ob-poc-dsl-compiler` or a new `ob-poc-manifest-export` crate):
    - Reads ob-poc's verb catalogue
    - Filters to public verbs (or all verbs if no public/private distinction exists per T0)
    - Emits YAML per §7 spec
    - Output: `ob-poc-manifest-<version>.yaml`
36. Vendoring: the generated manifest is copied to `bpmn-lite/manifests/ob-poc-v1.0.0.yaml` for the demo. Build process documented.

#### T2.9 — bpmn-lite-app and ob-poc-app wiring

37. **bpmn-lite-app:** on startup, instantiate `BusClient` (for outbound calls) and `BusServer` (for inbound results). Wire to bpmn-lite-runtime. Outbox sender task started.
38. **ob-poc-app:** on startup, instantiate `BusServer` (for inbound calls) and `BusClient` (for sending results back). Wire to `ob-poc-bus-handler`. Outbox sender task started.
39. **Configuration:** each app reads a config file (TOML or YAML) declaring its own service identity, peer domain endpoints, Postgres connection, etc.

#### Tests (T2 master DoD)

40. **End-to-end single invocation test:** bpmn-lite-app submits a request for `ob-poc:cbu.create` via the bus; ob-poc-app receives, executes a mock engine response, returns result; bpmn-lite-app's advance() callback fires with the resolved binding.
41. **Idempotency test:** same idempotency_key submitted twice → ob-poc returns same execution_id; only one engine submission occurs.
42. **Recovery test:** bpmn-lite-app crashes mid-call; on restart, outbox sender resumes; invocation completes correctly.
43. **Version mismatch test:** caller declares unknown catalogue_version → receiver returns REJECTED_VERSION_INCOMPATIBLE.

#### Crate discipline DoD

44. List every new crate created. Justify each.
45. List every new `pub` item across all touched crates. Justify each.
46. Verify no circular dependencies between crates.
47. Verify cargo doc output shows minimal expected public API per crate.

**DoD:** end-to-end invocation works across the bus. All four test scenarios pass. Crate discipline maintained per §5.8.

**STOP gate.** Largest single tranche; expect Adam review to be substantial.

---

### T3 — BPMN executor as async state machine (RIP-AND-REPLACE)

**Goal:** BPMN process instances advance through pause/persist/resume state machine, with all invocations going through the bus.

**Disposition:** RIP-AND-REPLACE. State machine shape from v0.3 survives conceptually but submit-step changes fundamentally.

**Crates touched:**
- `bpmn-lite-runtime` (significant rebuild)

**Sonnet tasks:**

1. **Rip prior v0.3 T3 work:** identify, delete cleanly, document learning.
2. **Executor API** (rebuilt against v0.5):
   - `start_process(workflow_source, initial_variables) → Result<ProcessInstanceId>`
   - `advance(instance_id, outcome) → Result<()>` (called by bus result handler)
   - `cancel(instance_id) → Result<()>`
   - **All non-blocking.** Return after persistence; never await callout.
3. **advance_internal — synchronous walking slice:**
   - Walks through gateway evaluations and other non-callout nodes
   - Stops at next callout (service-task, business-rule-task) or end event
   - At callout: compile inner plan via T1 pipeline, submit via `dsl-bus-client.submit_invocation(...)`, persist pending-call linkage, return
   - At end event: mark process completed, emit BpmnInstanceCompleted event (or whatever the demo UI requires)
4. **Failure handling:**
   - VerbFailed → mark Failed; record reason
   - OptimisticConflict → single automatic retry by re-submitting (new idempotency_key); Failed if still conflicts
   - LockTimeout / TimedOut → Failed with explicit reason
   - VersionMismatch → Failed (catalogue evolved; manual intervention)
5. **State invariants:**
   - At every commit boundary, process_instance row reflects current truth
   - No in-memory state required to advance — load from DB sufficient
   - Restart mid-WaitingOnInvocation: subscriber on next event call advance() with loaded state
6. **Tests:**
   - Async-correctness test: start process, verify start_process() returns immediately, verify status=WaitingOnInvocation, verify no thread parked, wait 10s, deliver mock result, verify advance
   - Long-wait test: process in WaitingOnInvocation >10s without resources held
   - Restart-mid-callout test: start process, submit plan, restart bpmn-lite, on startup subscriber receives bus result, advances correctly
   - Full §10 demo test: end-to-end for all three CBU type paths
7. **Crate discipline DoD:** new pub items justified; bpmn-lite-runtime stays focused on workflow execution (no leakage of bus internals or compiler internals into its public API).

**DoD:** executor is genuinely async (long-wait test passes); §10 demo runs end-to-end for all three paths; restart recovery works.

**STOP gate.**

---

### T4 — Pre-coded demo BPMN model (SURVIVES + MECHANICAL UPDATE)

**Goal:** §10 model is one function call away; runs against the federated stack.

**Disposition:** SURVIVES + MECHANICAL UPDATE. Model survives. Update: namespaced verb references.

**Crates touched:**
- `bpmn-lite-demos` (existing or NEW): demo model constructors

**Sonnet tasks:**

1. **Mechanical update to demo model:** change verb references from `cbu.create` to `ob-poc:cbu.create` etc. Specified per §10.
2. **Demo seed:**
   - Verify ob-poc catalogue contains §10 verbs with correct effect classes (per T0)
   - Verify `cbu_type_routing` DMN decision exists (per T0)
   - Sample CBU input data for fund / corporate / trust types
3. **Integration test:**
   - Compile §10 model
   - start_process with fund-type input → wait for completion (via process status polling) → verify Completed with expected end state
   - Repeat for corporate-type, trust-type
   - Verify audit trail in both bpmn-lite's and ob-poc's audit tables
4. **Reset helper:** `reset_demo_state()` clearing process_instance, pending_invocation, outbox, inbox tables.

**DoD:** demo workflow constructible in one call; integration test verifies all three paths complete; reset helper works.

**STOP gate.**

---

### T5 — Sage agentic integration (PARTIALLY RIP-AND-REPLACE)

**Goal:** Sage routes through bus; reasoning persisted in audit.

**Disposition:** PARTIALLY RIP-AND-REPLACE. Sage internal logic survives. Integration mechanism (in-process events → bus subscriber) is rip-and-replace.

**Crates touched:**
- `sage-integration` (NEW per-domain): subscribes to bus events
- `bpmn-lite-runtime` (modify): one service-task routes through Sage

**Sonnet tasks:**

1. **Sage as bus subscriber:** registers via `dsl-bus-server`'s subscription API (or polls audit directly if T0 reveals simpler integration).
2. **Sage decision point in demo flow:** suggest `cbu.add_instrument_matrix` (post-gateway convergence). When BPMN executor reaches this node, marked as Sage-mediated.
   - Sage receives the lifecycle event for the prior node
   - Sage reads current process state
   - Sage walks Semantic Dependency Graph to confirm legal next-step
   - Sage submits the actual verb invocation via the bus (potentially to ob-poc or wherever the verb lives)
   - Sage's reasoning recorded in audit with structured form
3. **Tests:** Sage-mediated service task completes process correctly; reasoning captured; works for all three CBU type paths.

**Fallback:** if Sage cannot submit plans via bus (T0 reveals integration gap), T5 reduces to observation mode — Sage subscribes, observes, presents reasoning but doesn't submit. Less wow factor; still tells the story.

**DoD:** at least one service task goes through Sage; reasoning persisted; process completes for all three paths.

**STOP gate.**

---

### T6 — ob-poc UI repointing (SURVIVES)

**Goal:** existing ob-poc UI displays bpmn-lite process state, plan submissions, Sage reasoning, DMN results.

**Disposition:** SURVIVES. UI talks to bpmn-lite's local HTTP API. Cross-domain transport invisible to UI.

**Crates touched:**
- `bpmn-lite-api` (existing from T6 v0.3 work; minor adjustments)
- ob-poc UI (frontend; existing)

**Sonnet tasks:**

1. **Verify UI still works** against bpmn-lite-api after T2/T3 rebuilds.
2. **Update endpoints if needed:** any API contract changes from the runtime rebuild are surfaced and adjusted.
3. **Cross-domain visibility:** UI shows callout target_domain explicitly (e.g., "Calling ob-poc:cbu.create"). New small UI element.
4. **Tests:** manual walkthrough of demo flow; all four panels populate correctly.

**DoD:** UI displays demo process running across all panels for all three paths.

**STOP gate.**

---

### T7 — Docker deployment integration (RIP-AND-REPLACE)

**Goal:** federated stack runs via docker-compose.

**Disposition:** RIP-AND-REPLACE. Topology changes from "one container" to "multiple containers + multiple Postgres + gRPC network".

**Crates touched:**
- Docker configuration files (separate from Rust crates)

**Sonnet tasks:**

1. **Containers:**
   - `bpmn-lite-app` container (Rust binary)
   - `bpmn-lite-postgres` container
   - `ob-poc-app` container (Rust binary; includes ob-poc-bus-handler)
   - `ob-poc-postgres` container
   - `ob-poc-ui` container (existing frontend; pointed at bpmn-lite-app)
2. **Networking:**
   - All containers on a single docker-compose network
   - bpmn-lite-app and ob-poc-app know each other's hostnames (docker-compose service names)
   - UI knows bpmn-lite-app's HTTP endpoint
3. **Migrations:** each app runs its own migrations on startup.
4. **Demo seed:** loaded on startup or via separate seed container.
5. **Single command start:** `docker-compose up` brings entire stack live.
6. **Reset script:** `./demo-reset.sh` truncates relevant tables across both Postgres instances; clears any in-flight state.
7. **Async correctness in Docker:** verify the long-wait test (>10s WaitingOnInvocation) works across container boundaries.
8. **Tests:** stack starts cold; demo runs end-to-end dockerised; restart recovery works.

**DoD:** `docker-compose up` brings federated stack live; reset returns clean state; demo verified dockerised; async correctness holds across containers.

**STOP gate.**

---

### T8 — Demo polish + rehearsal (NEW re-rehearsal)

**Goal:** demo runs cleanly 5× in a row against federated stack.

**Disposition:** NEW (re-rehearsal). Demo flow same as planned; rehearsal must be redone against federated architecture.

**Sonnet tasks:**

1. **Scripted demo flow:** ordered user actions producing demo narrative.
2. **Speaker notes:** what to say at each step; what to point at; expected outcomes; **how to talk about the bus when foundational services ask** (use §13 prepared answers).
3. **Demo data variations:** 3 inputs producing fund / corporate / trust paths.
4. **Failure recovery documentation:** verb fails, DMN times out, Sage hangs, UI desyncs, restart mid-callout, bus partition simulated — documented for each.
5. **Rehearsal:** 5 consecutive runs; capture flakiness; fix; repeat until stable.
6. **Backup material:** screenshots of each beat.
7. **Q&A preparation:** §13 answers reviewed; practice delivery.

**DoD:** 5 consecutive clean runs; speaker notes complete; failure recovery documented; backup material prepared; Q&A answers internalised.

**STOP gate. Demo ready.**

---

## 10. Demo BPMN model (locked)

CBU lifecycle — sequential with one exclusive gateway routed by DMN. Federated: ob-poc owns the verbs and the DMN decision; bpmn-lite owns the workflow.

```scheme
(workflow custody-cbu-onboarding
  (start-event :id start :next create-cbu)
  (service-task :id create-cbu :verb ob-poc:cbu.create :next type-decision)
  (business-rule-task :id type-decision :decision ob-poc:cbu_type_routing :next type-gateway)
  (exclusive-gateway :id type-gateway
    (flow :condition (= @cbu-type "fund")      :next add-fund)
    (flow :condition (= @cbu-type "corporate") :next add-corp)
    (flow :condition (= @cbu-type "trust")     :next add-trust))
  (service-task :id add-fund  :verb ob-poc:cbu.add_fund_product      :next add-im)
  (service-task :id add-corp  :verb ob-poc:cbu.add_corporate_product :next add-im)
  (service-task :id add-trust :verb ob-poc:cbu.add_trust_product     :next add-im)
  (service-task :id add-im    :verb ob-poc:cbu.add_instrument_matrix :next end)
  (end-event :id end :status "Operational"))
```

Workflow surface declares structure only. All verbs are `ob-poc:*` namespaced — every callout crosses the bus.

5 service tasks (cross-domain), 1 business rule task (cross-domain), 1 exclusive gateway (local), 3 routing paths, 1 end state, 1 implicit `@cbu` placeholder, 1 implicit `@cbu-type` placeholder.

For demo: every callout is a real gRPC call to ob-poc. Foundational services see the federation in action on every node.

---

## 11. Master Demo DoD

Plan is complete when all simultaneously true:

1. bpmn-dsl source compiles via parse / lint / DAG pipeline to Populated Execution DAG
2. `@cbu` placeholder inference works without explicit declarations
3. Namespaced verb resolution against imported ob-poc manifest works
4. gRPC bus established between bpmn-lite-app and ob-poc-app
5. Outbox / inbox pattern handles delivery durability and idempotent receive
6. execution_id serves as the single correlation identity across both sides
7. BPMN executor is verifiably async (long-wait test passes)
8. §10 demo model runs end-to-end through all three CBU type paths
9. Both placeholder resolutions (`@cbu`, `@cbu-type`) work across cross-domain invocations
10. At least one service task routes through Sage with persisted reasoning
11. ob-poc UI displays workflow / plans / Sage / DMN in real time
12. Entire federated stack runs via `docker-compose up` from clean state
13. Async correctness verified across container boundaries (>10s WaitingOnInvocation works)
14. Restart recovery works: kill a container mid-call, restart, demo continues correctly
15. Demo runs cleanly 5× consecutively from scripted flow
16. Speaker notes complete; failure recovery documented; Q&A answers prepared
17. Crate discipline maintained per §5.8 — every new `pub` justified; no super-crates; no circular deps

---

## 12. Risk register

**R1: T0 reveals catalogue gaps for §10 verbs in ob-poc.**
Mitigation: T0 surfaces gaps; T2.8 includes catalogue extensions if needed; or model adjusts to verbs known to exist.

**R2: T0 reveals current binding-flow handling is explicit-only.**
Mitigation: T1 includes placeholder inference; extends Phase 5 T10 mechanism modestly.

**R3: T0 reveals `dsl-engine-core` isn't extracted as a shared crate.**
Mitigation: T2 includes the extraction. Adds work but unavoidable for federated deployment. Engine code itself is closed at `b7c5e5f1`; the extraction is mechanical (move files, adjust Cargo.toml).

**R4: T0 reveals existing inter-domain communication exists in non-bus form.**
Mitigation: T2 RIP-AND-REPLACE absorbs it. The federated bus is the only inter-domain channel.

**R5: T0 reveals Sage cannot subscribe to bus events.**
Mitigation: T5 reduces to observation mode.

**R6: gRPC + tonic introduces new operational complexity (TLS misconfiguration, port conflicts, etc.).**
Mitigation: demo uses plaintext gRPC; tonic defaults; minimal config. TLS deferred to production.

**R7: Outbox sender task has subtle ordering bugs under load.**
Mitigation: at-least-once delivery + idempotent inbox handles duplicate delivery. Tests in T2 cover this explicitly.

**R8: Async correctness verification fails — something parks a thread.**
Mitigation: long-wait test in T3 catches it; fix at design level rather than masking.

**R9: Manifest version skew between bpmn-lite's vendored manifest and ob-poc's actual catalogue.**
Mitigation: bus protocol carries catalogue_version; receiver validates; explicit rejection if incompatible. For demo, both pinned to single version.

**R10: T7 Docker deployment hits networking issues across containers.**
Mitigation: docker-compose default network usually just works; service names as hostnames. Fallback: explicit network configuration.

**R11: Crate discipline slips during execution (Sonnet adds `pub` without justification, creates super-crate).**
Mitigation: per-tranche DoD includes pub-additions report; Adam reviews; STOP gate triggers if violations detected.

**R12: T8 demo rehearsal reveals integration bugs.**
Mitigation: time-boxed fix-and-repeat; fallback to most stable subset + screenshots for residual.

---

## 13. Demo Q&A preparation

When foundational services ask the questions they'll ask, here are the prepared answers.

### Q: How does this handle long-running workflows?

> The DSL engine emits typed lifecycle events on every committed execution. Each domain in the federation is one peer among many; each subscribes to the events it cares about. Cross-domain invocations are recorded in a pending-call registry keyed by the engine's execution_id. When the result arrives — milliseconds later for fast verbs, hours later for human tasks, days later for external system callbacks — the subscriber looks up the pending invocation, binds the resolved values into the process variable scope, and advances the workflow. The executor never blocks. Process state is durable in Postgres at every step. If a runtime restarts mid-invocation, the subscriber on startup replays from the audit log; no invocations are lost. Same code path handles a 50-millisecond DMN evaluation and a 14-day user task.

### Q: How does this scale across multiple domains?

> Each domain is independently deployable with its own Postgres database, its own DSL engine instance, and its own catalogue. Domains communicate via gRPC over HTTP/2 — typed protobuf messages, no shared state, no broker dependency. Adding a new domain to the federation is: (1) the new domain implements the standard bus protocol, (2) other domains import its catalogue manifest at build time, (3) calls to the new domain's verbs work the same as any other federated call. Each domain owns its own data, its own audit, its own authority. Recovery is local to each domain — no distributed consensus, no two-phase commit.

### Q: What's the architectural model here?

> Each verb in the platform is analogous to a stored procedure. The catalogue manifest is the public API surface — names, typed signatures, behavioural metadata. Consumer domains import the manifest; their compilers validate every reference at build time, exactly like a SQL client validates queries against a known schema. The bus is the wire protocol — typed calls, typed results, authority-controlled. The mechanism is well-understood; what's novel is the application at the verb level rather than the SQL level.

### Q: What happens if a domain goes down?

> Outbox pattern handles it. Each domain has a transactional log of messages it intends to send. If a peer is unreachable, messages stay in the outbox; the sender task retries with exponential backoff. When the peer recovers, the outbox drains. Each side keeps state durable in its own Postgres; no shared infrastructure to lose. Network partitions, peer crashes, and runtime restarts all resolve via the same mechanism — read your own log, resume your own work.

### Q: How do you handle duplicate messages?

> Idempotency_key in protobuf metadata, distinct from the execution identity. Caller generates it per logical invocation. Receiver dedupes on it via a transactional inbox table. If the same idempotency_key arrives twice (network retry, sender restart), the receiver returns the same execution_id it returned the first time; only one actual execution occurs. At-least-once delivery with idempotent receive equals effectively-once execution from the application's perspective.

### Q: Why not use a message broker like Kafka or NATS?

> Direct peer-to-peer gRPC was the right call for our scale and architectural style. Brokers add operational complexity (HA clustering, partition management, schema registry) that's not justified for our throughput (tens to hundreds of messages per second between domain pairs). Each domain's outbox in its own Postgres gives us the durability properties without a broker dependency. Replay-on-restart, idempotent receive, exponential backoff are all in the application layer — we have full control over the semantics. If scale ever demands it, swapping the transport to NATS is a localised change; the protocol layer doesn't move.

### Q: How do you handle catalogue evolution?

> Catalogues are versioned. ob-poc publishes manifest v1.4.2; bpmn-lite imports v1.4.2 at build time. The catalogue_version is carried in every bus call. If ob-poc evolves to v1.5.0, bpmn-lite continues to invoke against v1.4.2 (forward compatibility) or rebuilds against v1.5.0. The bus protocol detects skew and rejects incompatible calls with structured diagnostic. This is the same pattern SQL clients use for schema migration — version-aware clients gracefully degrade.

### Q: What about transactions across domains?

> Each invocation is its own transaction within the receiving domain. Cross-domain distributed transactions (two-phase commit) are deliberately out of scope — the operational complexity outweighs the benefit for our use case. The architectural pattern is: design verbs to be idempotent where possible; use sagas (compensating actions) where transactions span domains; rely on the at-least-once-with-idempotent-receive semantics to handle most retry cases without needing distributed coordination. This is a mature pattern in distributed systems; well-documented in the literature.

### Q: How does Sage fit in?

> Sage is a subscriber peer in the federation — same status as bpmn-lite. It subscribes to lifecycle events from the bus, walks the Semantic Dependency Graph to reason about state, and submits its own plans via the bus when it makes decisions. Sage doesn't have privileged access to other domains' internals — it goes through the same catalogue-bound, authority-controlled invocation path as anything else. The agent is a peer, not a side-channel.

---

## 14. Execution conventions

- **One tranche per session.** Sonnet completes, reports, stops at STOP gate.
- **No commits without review.** Sonnet does not commit. Adam reviews diff, approves, commits.
- **Progress markers.** Sonnet reports % complete and current sub-step.
- **No improvisation outside tranche scope.** Hit something outside scope → STOP and report.
- **Phase 5 engine is closed.** No modifications to `b7c5e5f1` engine code. Consumed as `dsl-engine-core` library only.
- **Rip-and-replace by default.** Mechanical updates permitted only where explicitly specified and bounded.
- **No `block_on()` in async paths.** Async correctness is verifiable; verify it.
- **Crate discipline is non-negotiable.** Per-tranche DoD includes pub-additions report. Adam reviews; violations are STOP gates.
- **Replan from T0.** v0.1, v0.2, v0.3, v0.4 work is superseded by v0.5. Sonnet starts T0 fresh.

---

## 15. Hand-off to Sonnet

For each tranche, hand Sonnet:

1. v0.5 in full (Sonnet reads end-to-end before any work)
2. The relevant tranche section scoped explicitly
3. "Report findings at the STOP gate. Do not commit."

Begin with T0 (audit, no code). v0.1–v0.4 superseded; replan from T0 against v0.5.

---

## 16. Status tracking

```
Phase 5.5 v0.5 — federated DSL platform demo deployment
T0 ☐  T1 ☐  T2 ☐  T3 ☐  T4 ☐  T5 ☐  T6 ☐  T7 ☐  T8 ☐
Status: pre-execution; awaiting T0 audit from Sonnet
```

End of Phase 5.5 plan v0.5.
