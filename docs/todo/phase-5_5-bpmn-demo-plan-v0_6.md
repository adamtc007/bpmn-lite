# Phase 5.5 Plan v0.6: Federated DSL Platform Demo Deployment

| Field | Value |
| --- | --- |
| Document ID | OB-POC-PHASE-5_5-PLAN-006 |
| Version | v0.6 |
| Status | DRAFT — peer review corrections + T0 audit findings incorporated |
| Author | Adam Cearns |
| Date | 2026-05-20 |
| Supersedes | v0.5 (DMN ownership inconsistency; pre-ack durability gap; lexicon model implicit; outbox wording loose; T2 oversized); T0 audit findings (gaps A through J) absorbed |
| Position | Post-Phase 5 (`b7c5e5f1`); demo-driven; federated architecture is the production end state |
| Repo topology | bpmn-lite, ob-poc, dmn-lite — three independent repos, three independent Postgres, three independent engine instances |
| Deployment | Independent containers per domain; each domain's own Postgres; gRPC bus over HTTP/2 between domains |
| Engine sharing | NO new `dsl-engine-core` crate — each domain depends directly on ob-poc's existing `dsl-core` + `dsl-runtime` + `sem_os_postgres` as workspace deps |

---

## 0. What changed from v0.5 to v0.6

Five peer review corrections plus ten T0 audit findings, integrated:

**Peer review corrections:**
1. **DMN as its own federated domain.** dmn-lite is now a third independently-deployable domain alongside bpmn-lite and ob-poc.
2. **Local `callout_id` added for pre-SubmissionAck durability.** Caller-side identity that exists before the receiver returns execution_id. New process status `WaitingOnSubmission`; transition to `WaitingOnInvocation` after ack.
3. **Domain lexicon model made explicit.** New §5.4 listing what a domain contributes.
4. **Form.io as future domain example** demonstrating architectural generality.
5. **Outbox wording corrected** to "transactional delivery log" rather than "not a queue."
6. **T2 split into T2A (protocol + storage + manifest crate) and T2B (runtime bus path)** with STOP gate between.

**T0 audit findings absorbed:**

A. **No `dsl-engine-core` standalone crate.** Engine code distributed across `dsl-core`, `dsl-runtime`, `sem_os_postgres`. **Decision: skip extraction; each domain depends directly on these existing ob-poc crates.** Reduces T2 scope.

B. **Lexer doesn't support `:` in symbols.** T1 includes a one-line fix (`is_symbol_continue` extension).

C. **`WorkflowExecutionPlan` is bpmn-lite-specific; NOT a Phase 5 `ExecutablePlan`.** Clarified architectural distinction: BPMN workflow DAG remains `WorkflowExecutionPlan` (long-lived, bpmn-lite-specific); inner DSL plans submitted per callout ARE Phase 5 `ExecutablePlan`s (short-lived, run on receiver's engine).

D. **All v0.3 T2/T3 in-process tokio infrastructure is correctly classified as RIP-AND-REPLACE.** No change to v0.5 disposition.

E. **Three demo verbs (`cbu.add_fund_product`, `cbu.add_corporate_product`, `cbu.add_trust_product`) don't exist; `cbu.add_instrument_matrix` doesn't exist either.** Demo model in §10 corrected to use existing verbs: `cbu.add-product` (with product-type as arg) and `instrument-matrix.attach`. Gateway branching preserved for demo drama by routing to three service-tasks all calling `cbu.add-product` with different argument values.

F. **No public/private verb distinction in catalogue.** T2A manifest tooling uses explicit allowlist for demo. Adding `public: true` to verb YAML is a follow-up enhancement.

G. **No service identity / auth infra exists.** T2B includes env-var-based shared-secret config. Matches v0.5 decision.

H. **No manifest export pipeline exists.** T2B includes generator binaries for ob-poc and dmn-lite. Reads catalogue YAML + allowlist; produces v0.5 §7 format manifest.

I. **No shared bus crates exist.** T2A creates `dsl-bus-protocol`, `dsl-bus-storage`, `dsl-manifest`. T2B creates `dsl-bus-client`, `dsl-bus-server`.

J. **Old pending-call schema (migration 033) keyed on execution_id only.** T2A drops it; new migration adds callout_id + execution_id + idempotency_key + target_domain + verb_id per v0.6 schema.

The v0.5 spine survives intact. These are corrections, additions, and audit-informed scope adjustments.

---

## 1. Architectural commitments

Six load-bearing commitments. Every tranche serves them.

### Commitment A: bpmn-dsl source IS the workflow graph

The workflow definition language is bpmn-dsl: s-expression form, same language family as ob-poc DSL. The source is already structurally a graph; the compiler walks the source and emits the workflow DAG directly.

Compilation pipeline: parse (NOM produces AST) → linter (placeholder allocation, type checks, reference resolution, namespace resolution) → DAG pass (validate execution order, emit `WorkflowExecutionPlan`) → submission of per-callout inner plans as Phase 5 `ExecutablePlan`s.

### Commitment B: Two plan types at two scopes

| | BPMN workflow plan | Inner DSL plan |
|---|---|---|
| Type | `WorkflowExecutionPlan` (bpmn-lite-specific) | `ExecutablePlan` (Phase 5 contract) |
| Scope | Process-instance | One callout |
| Duration | Long-lived (seconds, hours, days) | Short-lived (one ExecutionFrame on receiver) |
| Persistence | Durable in Postgres between callouts | In-memory on receiver during execution |
| Advances on | Lifecycle event arrival via bus | Internal verb completion |
| Owned by | bpmn-lite | Receiver domain's engine |

The BPMN workflow plan describes the *workflow shape* — nodes, gateways, sequence flows, placeholder bindings. The inner DSL plans describe *individual verb invocations* — one per callout — compiled by bpmn-lite and submitted via bus to the receiving domain, which executes them on its own Phase 5 engine.

**Critical clarification (from T0 audit gap C):** `WorkflowExecutionPlan` is *not* an `ExecutablePlan` and is not validated by the Phase 5 validator. Phase 5 validates the *inner* plans, one per callout. bpmn-lite owns workflow-plan validation as a separate concern.

### Commitment C: Workflow surface declares structure; catalogue resolves bindings

bpmn-dsl declares topology (`:next`), verb/decision identities (`:verb`, `:decision`), optional inputs (`:inputs`), and gateway predicates (`:condition`). Placeholder flow (`@cbu`, `@cbu-type`) is *inferred by the compiler* from catalogue declarations. Workflow author never writes binding-flow ceremony unless overriding inferred behaviour.

For demo, single-binding-per-type inference. Multi-binding-per-type is out of scope.

### Commitment D: Async-by-default; sync is a latency case, not an implementation

One execution path. It is async. The demo exercises the fast case of it.

The BPMN executor never blocks waiting for a callback. All invocations are submitted asynchronously across the bus; the executor returns immediately after persistence; lifecycle events resume the process on arrival via subscriber tasks.

Mechanism: two-stage durable state.
- **Stage 1 (caller-side):** before bus call completes, caller has durable `callout_id` and outbox row. Process is in `WaitingOnSubmission` state.
- **Stage 2 (after SubmissionAck):** receiver returns `execution_id`. Caller transitions to `WaitingOnInvocation`, recording execution_id. Process state references execution_id for the remainder of the call.

Both stages survive crashes. Process recovery is "look at status; outbox sender resumes if WaitingOnSubmission; await result delivery if WaitingOnInvocation."

### Commitment E: Federated deployment is the architectural target

Each domain (bpmn-lite, ob-poc, dmn-lite, future others) is **independently deployable** with its own Postgres database, its own DSL engine instance (consumed as workspace deps on ob-poc's existing engine crates), its own catalogue, its own audit. Inter-domain calls go through a **gRPC over HTTP/2 bus** carrying typed protobuf messages.

Domains do not share runtime state. The only inter-domain channel is the bus.

### Commitment F: Stored-procedure model — catalogue manifest is the public API

Each domain publishes a versioned catalogue manifest declaring its **public verb surface**: verb names, typed signatures, effect classes, resource dependencies, FSM applicability, authority requirements. The manifest is the API contract.

Consumer domains import the manifest at build time. Their compilers validate every cross-domain verb reference at compile time. Internal verbs (not in the manifest) are private to the owning domain.

For demo: ob-poc publishes manifest containing `cbu.create`, `cbu.add-product`, `instrument-matrix.attach`. dmn-lite publishes manifest containing `cbu_type_routing`. bpmn-lite imports both at build time.

---

## 2. Per-tranche disposition

| Tranche | Disposition | Notes |
|---|---|---|
| **T0** | **COMPLETE** | Sonnet's audit findings absorbed into this v0.6 document. T0 closed. |
| **T1** | **MECHANICAL UPDATE** + **AUGMENT** | Lexer change (`:` in symbol_continue); namespace resolution in linter; inner-plan compilation to Phase 5 ExecutablePlan |
| **T2A** | **NEW** | `dsl-bus-protocol` + `dsl-bus-storage` + `dsl-manifest`; schemas + types; no runtime integration |
| **T2B** | **NEW** | `dsl-bus-client` + `dsl-bus-server`; ob-poc-bus-handler + dmn-lite-bus-handler; manifest export from both source domains; runtime integration in bpmn-lite |
| **T3** | **RIP-AND-REPLACE** | All in-process tokio T2/T3 work (lifecycle.rs, event_bus.rs, subscriber.rs, bpmn_executor.rs, demo_invoker.rs, sage_observer.rs, pending stores, workflow_instance stores, migrations 033/034, REST endpoints) is deleted. State machine rebuilt against v0.6 contract with two-stage callout_id + execution_id durability. |
| **T4** | **MECHANICAL UPDATE** | Demo model rewritten with corrected verb names (per §10); placeholder registry updated for namespaced verbs |
| **T5** | **PARTIALLY RIP-AND-REPLACE** | Sage internal logic survives; sage_observer.rs and EventBus subscription replaced with bus-subscriber pattern |
| **T6** | **SURVIVES** | UI talks to bpmn-lite's local HTTP API; REST endpoints rebuild against new types but contract preserved |
| **T7** | **RIP-AND-REPLACE** | Three app containers + three Postgres containers + UI; topology change |
| **T8** | **NEW** | Demo rehearsal against three-domain federation |

**Rip-and-replace discipline:** delete prior code cleanly. Do not attempt surgical edits. Move deleted code to `_deprecated/` directory or git-rm for review.

**Mechanical update discipline:** only the explicitly specified transformation. No "while I'm here" cleanups.

---

## 3. Pre-locked decisions

1. **Workflow definition language** = s-expression bpmn-dsl. Same compiler as ob-poc DSL.
2. **Condition language** = s-expression predicate subset.
3. **Binding convention** = service task `:verb` matches verb id in catalogue (namespaced: `ob-poc:cbu.create`, `dmn-lite:cbu_type_routing`). `:name` is human label only. Unresolved `:verb` = compile error.
4. **Verb argument convention** = `:inputs (key1 value1, key2 value2)` for explicit arg binding. Placeholders (`@cbu`) bind by type inference unless explicit. Constants quoted (`"fund"`).
5. **Placeholder mechanism** = single-binding-per-type inference (`@cbu` style). Workflow surface stays declaration-free for inferred bindings.
6. **Bus transport** = gRPC over HTTP/2 via tonic. Production end state and demo.
7. **Wire format** = protobuf with schema files. Version-pinned per service.
8. **Cross-domain correlation identity** = `execution_id` (engine-issued by *receiving* domain; returned in gRPC SubmissionAck synchronously).
9. **Caller-side pre-ack durability identity** = `callout_id` (UUIDv7, generated by caller before bus call; durable in caller's Postgres; NOT transmitted on wire as primary correlation).
10. **Retry-safety** = `idempotency_key` (UUIDv7, caller-generated, in gRPC metadata; distinct from execution_id and callout_id).
11. **Process states** = `Created`, `Running`, `WaitingOnSubmission`, `WaitingOnInvocation`, `Completed`, `Failed`.
12. **Delivery durability** = transactional delivery log per domain (outbox pattern); single sender task; atomic with business state.
13. **Idempotent receive** = inbox table keyed by `idempotency_key`. ON CONFLICT DO NOTHING.
14. **Catalogue manifest** = static YAML file. Generated by owning domain at build time. Imported by consuming domains as build artifact. Version-pinned.
15. **Manifest public-surface selection** = explicit allowlist file per domain for demo (T2B). Adding `public: true` flag to verb YAML is a follow-up.
16. **Authority model** = service identity at bus connection (shared secret in env var for demo); per-call authority context in gRPC metadata.
17. **Snapshot pinning** = caller declares consumed catalogue_version in invocation request; receiver validates.
18. **Connection topology** = direct peer-to-peer gRPC, no broker. Domains know each other's endpoints via env-var config.
19. **Subscriber model** = each domain owns its own pending-call/outbox/inbox tables.
20. **Crate discipline** = explicit per-tranche gate. No super-crates. `pub` requires justification.
21. **Refactor discipline** = rip-and-replace by default for LLM-executed work.
22. **DMN as federated domain** = dmn-lite is independently deployable with own engine, own Postgres, own manifest.
23. **Engine sharing** = NO `dsl-engine-core` extraction. Each domain depends directly on ob-poc's `dsl-core`, `dsl-runtime`, `sem_os_postgres` as workspace deps (git or path dependency).
24. **STOP gates** between every tranche.

---

## 4. Non-goals (explicit)

To prevent Sonnet drift:

- BPMN XML parser/loader (Stage 2; not this plan)
- Parallel or inclusive gateways
- Boundary events, event sub-processes, multi-instance markers, compensation events
- Timer scheduler, message correlator
- Multi-worker pool, lanes, admission controller (Phase 6 proper)
- Plan persistence, cross-snapshot replay, audit retention policy
- KYC, UBO, screening, completeness — explicitly removed from demo model
- Multi-binding-per-type placeholder syntax
- Compensation primitive, cancellation scopes, fan-out + WaitN
- Production-hardening of outbox: dead-letter queue, complex retry strategies, orphan reconciliation
- Production authority: full OIDC, fine-grained per-verb permissions
- Verb catalogue extensions beyond what the demo model invokes
- Cross-domain distributed transactions
- gRPC streaming (unary calls only for demo)
- LISTEN/NOTIFY for outbox wake-up (polling sender for demo)
- TLS / mTLS for gRPC (plaintext for demo)
- NATS or any message broker
- Adding `public: true` flag to verb YAML files (allowlist file used for demo manifest export)
- Form.io domain implementation (future-facing example only; not implementation)
- Multi-binding-per-type placeholder syntax (parent + subsidiary CBU)
- `dsl-engine-core` standalone crate extraction (each domain depends directly on ob-poc engine crates)
- Any rework of Phase 5 engine internals — engine code (`b7c5e5f1`) is closed; consumed as workspace deps only

If Sonnet's plan touches any of the above, STOP and report — do not implement.

---

## 5. Federated DSL platform architecture

### 5.1 The stored-procedure analogy

Each domain in the federated platform is analogous to a database server hosting **stored procedures**. The procedures are the domain's DSL verbs. The catalogue manifest is the procedure registry — names, typed signatures, behavioural metadata. The bus is the wire protocol; idempotency_key plus execution_id handle the call/retry/correlation semantics.

The architectural pattern is well-understood. What's novel is the application at the verb level rather than the SQL level.

### 5.2 Domain as deployment unit

A **domain** is an independently deployable unit of:
- DSL engine (instance of ob-poc's `dsl-core` + `dsl-runtime` + `sem_os_postgres`; not a separate `dsl-engine-core` crate)
- Catalogue (verbs the domain implements; published as manifest)
- Postgres (own database; not shared with other domains)
- Authority (own service identity)
- Audit (own audit trail in own Postgres)
- API (own HTTP/gRPC endpoints; UI access + bus access)

Domains do not share runtime state. The only inter-domain channel is the bus.

For the demo: three domains.
- **bpmn-lite**: workflow orchestration; owns BPMN process state and workflow execution
- **ob-poc**: business entity mutations; owns CBU catalogue and instrument matrix data
- **dmn-lite**: decision evaluation; owns DMN decision tables and routing logic

### 5.3 Catalogue manifest as published contract

Each domain publishes a YAML catalogue manifest. Format in §7.

The manifest is *generated*, not handwritten. Each domain has a manifest-export build step.

For the demo:
- `ob-poc` exports `ob-poc-manifest-v1.0.0.yaml` (from explicit allowlist; not all verbs)
- `dmn-lite` exports `dmn-lite-manifest-v1.0.0.yaml` (from explicit allowlist; not all decisions)
- `bpmn-lite` imports both at build time; compiler validates references

### 5.4 Domain lexicon model

**The shared DSL compiler and execution stack (ob-poc's `dsl-core` + `dsl-runtime`) are domain-neutral. A domain becomes executable by providing a domain lexicon.**

A domain lexicon contains:

1. **DSL verb catalogue** — Rust code implementing verb behaviours; consumed by the engine
2. **Manifest** — YAML declaring which verbs are public, their signatures, effect classes, dependencies, authority requirements
3. **Type definitions** — domain-specific types (e.g., `CBU`, `CbuType` for ob-poc); referenced by verb signatures
4. **Effect class mappings** — each verb's effect class declaration (per Phase 5 framework); determines coordination policy
5. **Authority requirements** — per-verb permission scopes
6. **Optional macro packs** — domain-specific s-expression macros (advanced; not in demo)
7. **Optional parser/linter extensions** — where allowed (advanced; not in demo)
8. **Runtime verb handlers** — Rust functions implementing verb behaviour against the domain's engine instance

The common stack owns:
- Parse / lint / DAG semantics (universal across domains)
- Execution model (ExecutionFrame, coordination, transaction policies — per Phase 5)
- Bus protocol (typed RPC, identity, authority, delivery)
- Audit, snapshot, manifest validation

The domain lexicon owns:
- Vocabulary (which verbs exist)
- Signatures (typed args and return)
- Bindings (what flows between verbs)
- Implementation (the Rust code behind each verb)
- Authority (what permission scopes apply)

This is the abstraction that makes the platform multi-domain. Each new domain provides items 1–8; the common stack accepts them; the platform federates.

### 5.5 Bus as wire protocol

The bus is **gRPC over HTTP/2 via tonic**, carrying typed protobuf messages.

Each domain runs:
- gRPC client (initiating outbound invocations)
- gRPC server (receiving inbound invocations + receiving inbound results)

Wire-level message types fully specified in §6.

Two service definitions per domain:
- `InvocationService` — receives inbound invocations
- `ResultService` — receives inbound results for previously-submitted invocations

### 5.6 Identity model

Five identities, each with a precise role:

| Identity | Origin | Role |
|---|---|---|
| `callout_id` | Caller domain, before bus call | **Caller-side durable identity before SubmissionAck.** Process state references this while WaitingOnSubmission. Local-only. Not on wire as primary correlation. |
| `idempotency_key` | Caller domain, before bus call | Retry-safety. In gRPC metadata. Receiver dedupes on this. |
| `execution_id` | Receiver domain's engine, on `engine.submit()` | **Cross-domain correlation identity.** Returned in SubmissionAck. Single identity used by both sides after ack. Carried in InvocationResult. |
| `plan_id` | Receiver domain's compiler | Internal to receiver. Audit reference. |
| `attempt_id` | Receiver domain's engine | Internal to receiver. Retry attempts within an execution. |

**Why callout_id exists** (addressing pre-ack durability): there's a window between "caller commits intent to invoke" and "receiver returns execution_id." During this window, the caller's bus might be retrying; the receiver might be down; the caller might crash. The BPMN process needs a durable local identity *throughout* this window, including before execution_id exists.

State transitions:
```
Process at service-task callout node
  → generate callout_id and idempotency_key
  → atomically: insert outbox row + insert pending-call row (callout_id only, execution_id null) + update process to WaitingOnSubmission(callout_id)
  → return from executor (no thread parked)

Outbox sender picks up entry → sends gRPC Submit → receives SubmissionAck(execution_id)
  → atomically: update outbox status='submitted' + update pending-call set execution_id + update process to WaitingOnInvocation(execution_id)

ResultService receives InvocationResult
  → lookup pending-call by execution_id
  → advance process
```

### 5.7 Failure semantics

| Failure | Behaviour |
|---|---|
| Receiver domain down | Caller's outbox stays pending; sender retries with backoff; resumes on reconnect. Process stays in WaitingOnSubmission. |
| Caller domain down between outbox-write and bus-send | On restart, outbox sender picks up the entry; sends; transitions process. |
| Caller domain down between SubmissionAck-received and pending-call-updated | On restart, query receiver by idempotency_key to recover execution_id; complete the state transition. |
| Caller domain down while in WaitingOnInvocation | On restart, process is durable; result arrival via bus advances correctly. |
| Network partition | Each side keeps state durable; reconvene when network heals. |
| Result delivery fails after work complete | Receiver's outbox retries result delivery; idempotent on caller side. |
| Caller times out waiting for result | BPMN process marked Failed; receiver continues; eventual result discarded. |
| Duplicate result delivery | Caller's inbox dedupes by idempotency_key; second is no-op. |
| Catalogue version skew | Receiver returns VersionMismatch outcome; caller fails the process or retries with newer manifest. |

### 5.8 Audit federation

Each domain has its own audit trail in its own Postgres (`dsl_execution_audit` per Phase 5 T14). Cross-domain audit join is by `execution_id`. End-to-end audit for a BPMN process instance:
- Query bpmn-lite's `bpmn_process_instance` and `bpmn_pending_invocation` tables
- For each callout, look up the corresponding `dsl_execution_audit` row in the *receiving domain's* Postgres
- Join by `execution_id`

For demo: manual join via shared query script. For production: distributed tracing (OpenTelemetry).

### 5.9 Crate topology

Shared infrastructure crates (consumed by all domains via workspace deps or git deps from a shared location):

- **`dsl-bus-protocol`** — protobuf definitions, tonic-generated traits. Pure types; no behaviour.
- **`dsl-bus-client`** — gRPC client wrapper; outbox sender task; submission flow.
- **`dsl-bus-server`** — gRPC server wrapper; inbox handler; idempotent dispatch.
- **`dsl-bus-storage`** — outbox/inbox table schemas; SQL migrations; access layer.
- **`dsl-manifest`** — catalogue manifest types; YAML loader; validator.

**NOT created (per T0 audit gap A):** `dsl-engine-core` extraction. Each domain depends directly on ob-poc's `dsl-core`, `dsl-runtime`, `sem_os_postgres` as workspace deps (existing pattern).

Per-domain crates (bpmn-lite):

- **`bpmn-lite-dsl-compiler`** — bpmn-dsl parser, linter, DAG pass. Already exists. Consumes `dsl-manifest`.
- **`bpmn-lite-runtime`** — pause/persist/resume state machine. Process instance persistence. Callout dispatch via `dsl-bus-client`. Already exists; rebuilt in T3.
- **`bpmn-lite-bus-handler`** — handles inbound result delivery for bpmn-lite's submitted invocations.
- **`bpmn-lite-api`** — HTTP endpoints for UI. Already exists; possibly minor updates.
- **`bpmn-lite-app`** — binary crate. Wires all components. Already exists.

Per-domain crates (ob-poc):

- **`ob-poc-bus-handler`** — NEW. Implements `InvocationService` for ob-poc. Receives inbound invocations; calls `dsl-runtime`'s `execute_verb` synchronously; enqueues result in outbox for delivery back.
- **`ob-poc-manifest-export`** — NEW. Build-time binary reading verb catalogue + allowlist; produces manifest YAML.
- Existing ob-poc crates consumed unchanged.

Per-domain crates (dmn-lite):

- **`dmn-lite-bus-handler`** — NEW. Implements `InvocationService` for dmn-lite. Receives inbound decision invocations; evaluates DMN; enqueues result.
- **`dmn-lite-manifest-export`** — NEW. Build-time binary for dmn-lite manifest.
- Existing dmn-lite crates consumed unchanged.

#### Crate discipline rules

1. **No super-crates.** A crate's purpose must be statable in one sentence.
2. **Minimal public API per crate.** `pub` requires justification. Default to `pub(crate)`.
3. **No convenience re-exports.**
4. **Explicit cross-crate deps.** No transitive reliance.
5. **No circular deps.**
6. **Test code stays internal.**
7. **No "expose for testing" exports.** Tests in same crate as types.
8. **Each tranche reports new `pub` additions.** Adam reviews; rejects unjustified ones.

### 5.10 Form.io as future federated domain (illustrative)

Demonstrating architectural generality (not implementation scope for v0.6):

A Form.io domain would expose a manifest:

```yaml
domain: "form-io"
catalogue_version: "v1.0.0"
verbs:
  - id: "form.render"
    signature:
      inputs:
        - name: "form_id"; type: "FormId"
        - name: "initial_data"; type: "JsonObject"
      output:
        produces: "FormSession"
    effect_class: "external_effect"
    authority_required: "form.read"
  
  - id: "form.submit"
    signature:
      inputs:
        - name: "session"; type: "FormSession"
      output:
        produces: "FormSubmission"
    effect_class: "append_fact"
    authority_required: "form.write"
  
  - id: "form.collect_evidence"
    signature:
      inputs:
        - name: "evidence_type"; type: "EvidenceType"
        - name: "context"; type: "JsonObject"
      output:
        produces: "EvidenceRecord"
    effect_class: "external_effect"
    authority_required: "evidence.collect"
```

BPMN workflows invoke `form-io:form.render` to start a user task. The user fills the form (over hours or days). Form.io eventually delivers a `FormSubmission` result via the bus. The BPMN process advances on result arrival.

Form.io is treated as a **human interaction domain** — not "just UI." Its verbs are typed; its bindings flow through workflows; its authority is enforced. It fits the same federated pattern as ob-poc and dmn-lite.

This confirms the architecture isn't OB-POC/DMN-specific. Adding new domain types is structural — each provides a lexicon (per §5.4) and connects to the bus.

---

## 6. Bus protocol specification (inline)

protobuf v3. File: `dsl_bus.proto`. Consumed by `dsl-bus-protocol` crate.

### 6.1 Common types

```protobuf
syntax = "proto3";
package dsl.bus.v1;

message Uuid { bytes value = 1; }    // UUIDv7 16 bytes

message AuthorityContext {
  string service_identity = 1;
  string user_identity = 2;
  repeated string roles = 3;
  bytes signed_token = 4;
}

message TypedValue {
  oneof value {
    string string_value = 1;
    int64 int_value = 2;
    double double_value = 3;
    bool bool_value = 4;
    Uuid uuid_value = 5;
    bytes blob_value = 6;
    bool null_value = 7;
  }
  string type_name = 10;
}

message ResolvedBinding {
  string name = 1;
  TypedValue value = 2;
}

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
  VERSION_MISMATCH = 11;
}

message ExecutionOutcome {
  ExecutionOutcomeKind kind = 1;
  string detail = 2;
  repeated ResolvedBinding bindings = 3;
}
```

### 6.2 InvocationService

```protobuf
service InvocationService {
  rpc Submit(InvocationRequest) returns (SubmissionAck);
}

message InvocationRequest {
  Uuid idempotency_key = 1;
  string verb_id = 2;           // e.g., "cbu.create" (domain prefix stripped before dispatch)
  repeated ResolvedBinding inputs = 3;
  AuthorityContext authority = 4;
  string source_domain = 5;
  string catalogue_version = 6;
  Uuid snapshot_pin = 7;
  string result_callback_endpoint = 8;
  google.protobuf.Timestamp timeout_at = 9;
}

message SubmissionAck {
  Uuid execution_id = 1;
  SubmissionStatus status = 2;
  string detail = 3;
}

enum SubmissionStatus {
  SUBMISSION_UNSPECIFIED = 0;
  ACCEPTED = 1;
  DUPLICATE = 2;
  REJECTED_VERB_UNKNOWN = 3;
  REJECTED_VERSION_INCOMPATIBLE = 4;
  REJECTED_AUTHORITY = 5;
  REJECTED_MALFORMED = 6;
}
```

### 6.3 ResultService

```protobuf
service ResultService {
  rpc DeliverResult(InvocationResult) returns (ResultAck);
}

message InvocationResult {
  Uuid execution_id = 1;
  Uuid idempotency_key = 2;
  ExecutionOutcome outcome = 3;
  string source_domain = 4;
  google.protobuf.Timestamp executed_at = 5;
  Uuid plan_id = 6;
  string audit_reference = 7;
}

message ResultAck {
  ReceiptStatus status = 1;
  string detail = 2;
}

enum ReceiptStatus {
  RECEIPT_UNSPECIFIED = 0;
  RECEIVED = 1;
  DUPLICATE_IGNORED = 2;
  REJECTED_UNKNOWN_EXECUTION = 3;
}
```

### 6.4 Wire-level conventions

- Encoding: protobuf binary
- Transport: HTTP/2 over TCP; gRPC framing via tonic
- Compression: gzip enabled
- TLS: disabled for demo (plaintext); enabled for production
- Timeouts: Submit 5s; DeliverResult 5s; caller full-invocation timeout configurable (demo 30s)
- Retries: outbox-driven exponential backoff (1s, 2s, 4s, ..., capped 60s)
- Authority verification: service_identity allowlist on receiver
- Catalogue version validation: receiver compares against current published manifest version

---

## 7. Catalogue manifest specification (inline)

YAML format. Generated by owning domain at build time.

### 7.1 Top-level structure

```yaml
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"
generated_from_snapshot: "sha256:abc123..."

min_consumer_manifest_version: "1.0"
breaking_changes_since: []

verbs: [<verb entry>, ...]
decisions: [<decision entry>, ...]
types: [<type entry>, ...]
```

### 7.2 Verb entry

```yaml
verbs:
  - id: "cbu.create"
    signature:
      inputs:
        - name: "name"
          type: "String"
          required: true
        - name: "client_type"
          type: "CbuClientType"
          required: true
      output:
        produces: "CBU"
    effect_class: "idempotent_ensure"
    coordination_policy: "UniqueInsert"
    transaction_policy: "AtomicShort"
    resource_dependencies:
      - kind: "NaturalKey"
        from_input: "name"
        entity_type: "CBU"
    authority_required: "cbu.write"
    description: "Create a new CBU entity."
```

### 7.3 Decision entry

```yaml
decisions:
  - id: "cbu_type_routing"
    inputs:
      - name: "cbu_client_type"
        type: "CbuClientType"
        required: true
    output:
      type: "CbuType"
      enum_values: ["fund", "corporate", "trust"]
    description: "Routes CBU to product attachment path based on client type."
```

### 7.4 Type entry

```yaml
types:
  - name: "CBU"
    kind: "entity"
    description: "Custody Banking Unit."
    uuid_type: "UUIDv7"
  - name: "CbuType"
    kind: "enum"
    values: ["fund", "corporate", "trust"]
  - name: "CbuClientType"
    kind: "enum"
    values: [...] # populated from actual catalogue
```

### 7.5 Manifest lifecycle

- **Generation:** each domain has a manifest-export binary that reads verb catalogue YAML files + an explicit allowlist file (`manifest-allowlist.yaml`) and emits the manifest.
- **Allowlist file format** (per-domain):
  ```yaml
  # ob-poc/manifest-allowlist.yaml
  public_verbs:
    - "cbu.create"
    - "cbu.add-product"
    - "instrument-matrix.attach"
  public_decisions: []  # ob-poc has no decisions
  ```
  ```yaml
  # dmn-lite/manifest-allowlist.yaml
  public_verbs: []
  public_decisions:
    - "cbu_type_routing"
  ```
- **Publication:** generated manifest is checked into a known location (`bpmn-lite/manifests/<domain>-<version>.yaml`) for the demo.
- **Import:** bpmn-lite's compiler loads manifests at build time; caches lookups indexed by namespaced id.
- **Validation:** every namespaced reference in bpmn-dsl is validated; unknown verbs = compile error.

---

## 8. Outbox / inbox / pending-call specifications (inline)

### 8.1 Outbox table (each domain has its own)

```sql
CREATE TABLE outbox (
  id UUID PRIMARY KEY,
  target_domain TEXT NOT NULL,
  target_endpoint TEXT NOT NULL,        -- 'invocation' | 'result'
  payload BYTEA NOT NULL,                -- protobuf-encoded
  idempotency_key UUID NOT NULL,
  execution_id UUID,                      -- nullable; filled after SubmissionAck
  callout_id UUID,                        -- nullable; bpmn-lite-side only for invocations
  status TEXT NOT NULL DEFAULT 'pending',
  attempt_count INT NOT NULL DEFAULT 0,
  next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  submitted_at TIMESTAMPTZ,
  UNIQUE (idempotency_key, target_endpoint)
);

CREATE INDEX idx_outbox_pending
  ON outbox(next_attempt_at) WHERE status = 'pending';
```

### 8.2 Inbox table (each domain has its own)

```sql
CREATE TABLE inbox (
  idempotency_key UUID PRIMARY KEY,
  source_domain TEXT NOT NULL,
  endpoint TEXT NOT NULL,
  execution_id UUID,                      -- engine-issued for invocations we received
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  processed_at TIMESTAMPTZ,
  status TEXT NOT NULL DEFAULT 'received',
  payload BYTEA
);

CREATE INDEX idx_inbox_source ON inbox(source_domain, received_at);
```

### 8.3 bpmn-lite pending-call table

```sql
CREATE TABLE bpmn_pending_invocation (
  callout_id UUID PRIMARY KEY,           -- caller-side identity (always present)
  
  process_instance_id UUID NOT NULL,
  node_id TEXT NOT NULL,
  
  target_domain TEXT NOT NULL,
  verb_id TEXT NOT NULL,
  
  idempotency_key UUID NOT NULL UNIQUE,
  execution_id UUID UNIQUE,              -- nullable; filled after SubmissionAck
  
  submitted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  ack_received_at TIMESTAMPTZ,           -- when execution_id was recorded
  timeout_at TIMESTAMPTZ
);

CREATE INDEX idx_pending_process ON bpmn_pending_invocation(process_instance_id);
CREATE INDEX idx_pending_execution ON bpmn_pending_invocation(execution_id) WHERE execution_id IS NOT NULL;
CREATE INDEX idx_pending_timeout ON bpmn_pending_invocation(timeout_at) WHERE timeout_at IS NOT NULL;
```

Pending-call lifecycle:
- **Stage 1 (caller commits intent):** row inserted with callout_id + idempotency_key set; execution_id = NULL; ack_received_at = NULL
- **Stage 2 (ack received):** row updated with execution_id + ack_received_at
- **Stage 3 (result received):** row deleted (atomic with process advance)

### 8.4 BPMN process instance table

```sql
CREATE TABLE bpmn_process_instance (
  id UUID PRIMARY KEY,
  workflow_id TEXT NOT NULL,
  
  current_node TEXT NOT NULL,
  status TEXT NOT NULL,                   -- Created | Running | WaitingOnSubmission | WaitingOnInvocation | Completed | Failed
  variables JSONB NOT NULL DEFAULT '{}',
  
  waiting_on_callout_id UUID,             -- set when status = WaitingOnSubmission
  waiting_on_execution_id UUID,           -- set when status = WaitingOnInvocation
  
  started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_advanced_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  completed_at TIMESTAMPTZ,
  
  end_status TEXT,
  failure_reason TEXT
);
```

### 8.5 Outbox sender flow

```rust
// Pseudocode — runs as background task per domain
async fn outbox_sender_loop(/* ... */) {
  loop {
    let entries = select_pending_outbox(10).await?;
    for entry in entries {
      let response = match entry.target_endpoint.as_str() {
        "invocation" => invocation_client.submit(entry.payload).await,
        "result" => result_client.deliver_result(entry.payload).await,
        _ => continue,
      };
      
      match response {
        Ok(ack) => {
          // For invocations: record execution_id from ack
          let exec_id = extract_execution_id(&ack);
          mark_outbox_submitted(entry.id, exec_id).await?;
          
          // For bpmn-lite invocations: ALSO update pending-call with execution_id
          if entry.target_endpoint == "invocation" && entry.callout_id.is_some() {
            update_pending_call_with_execution_id(entry.callout_id.unwrap(), exec_id).await?;
            transition_process_to_waiting_on_invocation(entry.callout_id.unwrap(), exec_id).await?;
          }
        }
        Err(e) => {
          let backoff = exp_backoff(entry.attempt_count);
          mark_outbox_retry(entry.id, backoff, e.to_string()).await?;
        }
      }
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
  }
}
```

### 8.6 Inbox receive flow (gRPC handler)

```rust
async fn handle_submit(req: InvocationRequest) -> Result<SubmissionAck> {
  let idem_key = uuid_from(&req.idempotency_key);
  
  // Idempotent receive
  if let Some(existing) = lookup_inbox(idem_key).await? {
    return Ok(SubmissionAck { execution_id: existing.execution_id, status: DUPLICATE });
  }
  
  // Validate verb/authority/version
  validate(&req)?;
  
  // Strip domain prefix from verb_id (e.g., "cbu.create" rather than "ob-poc:cbu.create")
  let local_verb_id = strip_domain_prefix(&req.verb_id);
  
  // Compile inner plan and submit to local engine SYNCHRONOUSLY (per T0 audit item 18)
  let result = execute_verb_sync(local_verb_id, &req.inputs).await?;
  let execution_id = result.execution_id;
  
  // Atomically: record inbox + enqueue result delivery
  let mut tx = pool.begin().await?;
  insert_inbox(&mut tx, idem_key, req.source_domain.clone(), execution_id, req.encode_to_vec()).await?;
  enqueue_result_to_outbox(&mut tx, req.source_domain.clone(), execution_id, idem_key, result.outcome).await?;
  tx.commit().await?;
  
  Ok(SubmissionAck { execution_id, status: ACCEPTED })
}
```

Note: synchronous execution inside `handle_submit` is fine. The receiver returns `SubmissionAck` after the verb completes locally (which may be milliseconds for ob-poc CBU operations). For longer-running verbs (e.g., future Form.io user tasks), `handle_submit` would return `ACCEPTED` immediately and the result would be enqueued when execution completes asynchronously. For demo: sync execution per the existing ob-poc engine API.

### 8.7 Recovery on startup

```rust
async fn startup_recovery() -> Result<()> {
  // 1. Outbox: sender loop picks up pending entries automatically
  
  // 2. Inbox: any 'received' but not 'processed' entries — for sync execution, this should be rare
  // (only if process crashed between handle_submit's tx commit and result enqueue)
  let stuck = select_stuck_inbox().await?;
  for entry in stuck {
    // Re-execute via engine; idempotency at engine layer prevents double-effect
    re_execute_inbox_entry(entry).await?;
  }
  
  // 3. bpmn-lite pending invocations: nothing to do — they'll resume on result delivery
  // Or timeout via separate sweep task
  
  Ok(())
}
```

---

## 9. Tranches

### T0 — COMPLETE

Sonnet's audit findings absorbed into this v0.6 document. T0 is closed. Proceed to T1.

---

### T1 — bpmn-dsl compilation pipeline updates

**Goal:** bpmn-dsl source compiles cleanly with namespaced verb resolution, inner-plan compilation to Phase 5 ExecutablePlan, and corrected demo verb references.

**Disposition:** MECHANICAL UPDATE + AUGMENT. Pipeline shape survives from v0.3; specific updates applied per T0 findings.

**Crates touched:**
- `bpmn-lite-compiler` (existing — modify): lexer, linter, DAG pass
- `dsl-manifest` (NEW shared crate): manifest types, loader, validator

**Sonnet tasks:**

1. **Lexer mechanical update (T0 gap B):**
   - In `bpmn-lite-compiler/src/dsl/lexer.rs`, extend `is_symbol_continue` to include `:` character
   - Tokens like `ob-poc:cbu.create` now lex as single symbols
   - Test: lexer produces single token for namespaced verb name

2. **Create `dsl-manifest` crate (NEW):**
   - Types matching §7 spec (`Manifest`, `VerbEntry`, `DecisionEntry`, `TypeEntry`)
   - YAML loader using `serde_yaml`
   - Validator for manifest structure
   - Lookup API: `manifest.lookup_verb(id) → Option<&VerbEntry>`, `manifest.lookup_decision(id) → Option<&DecisionEntry>`
   - Crate discipline: pub surface is `Manifest`, `ManifestError`, `load_manifest`. Internal validation is `pub(crate)`. No serde_yaml re-exports.

3. **Linter namespace resolution:**
   - Update `bpmn-lite-compiler/src/dsl/linter.rs` to recognise `domain:verb` references
   - Split namespaced ref at `:` to extract domain prefix and verb id
   - For native (no prefix or `bpmn-lite:`): resolve against local catalogue (current behaviour)
   - For foreign (`ob-poc:`, `dmn-lite:`): resolve against imported manifest via `dsl-manifest`
   - Manifests loaded at compile time from `bpmn-lite/manifests/<domain>-<version>.yaml`
   - Unknown domain → compile error
   - Unknown verb in known domain → compile error with manifest details

4. **Inner-plan compilation (T0 gap C):**
   - Clarify in code/comments that `WorkflowExecutionPlan` is bpmn-lite-specific (NOT Phase 5 ExecutablePlan)
   - For each service-task/business-rule-task node, the DAG pass annotates "this node compiles to a Phase 5 ExecutablePlan at runtime when callout is submitted"
   - Inner-plan compilation happens at submit-time, not at workflow-compile-time (workflow doesn't know which domain it'll dispatch to until inputs are bound)
   - Add explicit comment/doc: "WorkflowExecutionPlan describes workflow topology. Phase 5 ExecutablePlans are emitted per-callout at bus dispatch time."

5. **Placeholder inference (mechanical, from v0.3 work):**
   - Already exists per T0 item 6
   - Verify it works correctly with namespaced verbs (lookup uses imported manifest now)

6. **Tests:**
   - Lexer test: namespaced verb produces single symbol
   - Linter test: namespaced verb resolves against imported manifest
   - Linter test: unknown domain prefix produces structured error
   - Linter test: unknown verb in known domain produces structured error with available verbs
   - Compile §10 demo model (with corrected verb names): passes
   - DAG pass produces valid `WorkflowExecutionPlan`

7. **Crate discipline DoD:**
   - List every new `pub` item in `dsl-manifest`. Justify each.
   - `bpmn-lite-compiler` doesn't gain new pub items (mechanical updates only).
   - No circular deps.

**DoD:** §10 demo model compiles. Namespaced verbs resolve. Manifest import works. Placeholder inference works with namespaced verbs. No regression in existing 23 tests.

**STOP gate.**

---

### T2A — Bus protocol + storage + manifest types (NEW infrastructure)

**Goal:** typed foundation crates exist with their schemas, types, and unit tests. NO runtime integration.

**Disposition:** NEW. All net-new construction.

**Crates created:**
- `dsl-bus-protocol` (NEW)
- `dsl-bus-storage` (NEW)
- `dsl-manifest` (NEW — completed in T1 if not already)

**Sonnet tasks:**

#### T2A.1 — `dsl-bus-protocol` crate

1. Write `dsl_bus.proto` per §6 spec exactly. All message types; both services.
2. Configure tonic build (`build.rs`) to generate Rust code from proto.
3. Pub surface: generated types and traits only. No additional handcrafted types.
4. Tests: encode/decode round-trip for each message type.

#### T2A.2 — `dsl-bus-storage` crate

5. SQL migrations:
   - `outbox` table per §8.1
   - `inbox` table per §8.2
   - These migrations are applied per-domain (each domain runs them on its own Postgres)
6. Rust types: `OutboxEntry`, `InboxEntry`.
7. CRUD operations:
   - `insert_outbox(entry) → Result<()>`
   - `select_pending_outbox(limit) → Result<Vec<OutboxEntry>>` (uses `FOR UPDATE SKIP LOCKED`)
   - `mark_outbox_submitted(id, execution_id) → Result<()>`
   - `mark_outbox_retry(id, backoff_secs, error) → Result<()>`
   - `insert_inbox(entry) → Result<bool>` (returns false if duplicate via ON CONFLICT DO NOTHING)
   - `lookup_inbox(idempotency_key) → Result<Option<InboxEntry>>`
   - `mark_inbox_processed(idempotency_key) → Result<()>`
8. Pub surface: types and CRUD methods. SQL is `pub(crate)`. No raw SQL exposed.
9. Tests: each CRUD operation; idempotent inbox insertion; outbox status transitions; concurrent select with SKIP LOCKED.

#### T2A.3 — Verify `dsl-manifest` crate from T1

10. Per T1 task 2.
11. Add: manifest export validation (a manifest can be round-tripped through serde without loss).

#### Crate discipline DoD

12. List every new `pub` item across the three crates. Justify each.
13. No circular deps.
14. Each crate purpose is one sentence.

**DoD:** three crates compile. All unit tests pass. Schemas applied to test database. No runtime integration yet.

**STOP gate.**

---

### T2B — Runtime bus path

**Goal:** end-to-end bus invocation working between bpmn-lite, ob-poc, and dmn-lite. Manifest export pipelines in place. Demo verbs invokable across the bus.

**Disposition:** NEW. Runtime integration on top of T2A foundations.

**Crates touched:**
- `dsl-bus-client` (NEW)
- `dsl-bus-server` (NEW)
- `ob-poc-bus-handler` (NEW per-domain)
- `ob-poc-manifest-export` (NEW per-domain)
- `dmn-lite-bus-handler` (NEW per-domain)
- `dmn-lite-manifest-export` (NEW per-domain)
- `bpmn-lite-bus-handler` (NEW per-domain)
- `bpmn-lite-app` (modify): wire bus client/server
- `ob-poc-app` (modify): wire bus handler
- `dmn-lite-app` (modify): wire bus handler

**Sonnet tasks:**

#### T2B.1 — `dsl-bus-client` crate

1. gRPC client wrapper (tonic `InvocationServiceClient`, `ResultServiceClient`).
2. Outbox sender task per §8.5.
3. Submission API: `submit_invocation(target_domain, request) → Result<()>` (note: returns after outbox write; execution_id is populated asynchronously by the sender task)
4. Result-send API: `send_result(target_domain, result) → Result<()>` for receiver-side returning results.
5. Pub surface: `BusClient` struct with builder; `BusClientConfig`. Internal sender task is `pub(crate)`.
6. Tests: outbox-write-then-send round-trip with mock gRPC server; backoff on error; idempotent submission.

#### T2B.2 — `dsl-bus-server` crate

7. gRPC server impl (tonic).
8. `InvocationService` and `ResultService` traits with consumer-provided callbacks for verb dispatch and result handling.
9. Per §8.6: idempotent receive, verb dispatch via callback, atomic inbox+outbox-result.
10. Pub surface: `BusServer` builder pattern; service implementations are `pub(crate)`.
11. Tests: invocation round-trip with mock callbacks; idempotent receive; version mismatch handling.

#### T2B.3 — `ob-poc-manifest-export` binary

12. Reads ob-poc verb catalogue (existing YAML files).
13. Reads `ob-poc/manifest-allowlist.yaml` (per §7.5).
14. Emits `ob-poc-manifest-v1.0.0.yaml` to `bpmn-lite/manifests/` (or configurable output path).
15. For demo: allowlist contains `cbu.create`, `cbu.add-product`, `instrument-matrix.attach`.
16. Documentation: how to regenerate; how to bump version.

#### T2B.4 — `dmn-lite-manifest-export` binary

17. Reads dmn-lite decision catalogue.
18. Reads `dmn-lite/manifest-allowlist.yaml`.
19. Emits `dmn-lite-manifest-v1.0.0.yaml`.
20. For demo: allowlist contains `cbu_type_routing`.

#### T2B.5 — `ob-poc-bus-handler` crate

21. Implements `InvocationService` for ob-poc:
    - Receives `InvocationRequest`
    - Idempotent inbox check
    - Validates verb against current catalogue
    - Strips domain prefix (`ob-poc:cbu.create` → `cbu.create`)
    - Calls existing `dsl-runtime::execute_verb_sync` (per T0 item 18; verb is sync internally)
    - Receives `VerbExecutionResult` with execution_id and outcome
    - Atomically: insert inbox + enqueue result to outbox (target = caller domain)
    - Returns `SubmissionAck(execution_id)`
22. Pub surface: `ObPocBusHandler` struct + `start()` function.
23. Tests: invocation arrives, dispatches to verb, result enqueued.

#### T2B.6 — `dmn-lite-bus-handler` crate

24. Implements `InvocationService` for dmn-lite:
    - Receives `InvocationRequest`
    - Idempotent inbox check
    - Validates decision against current decision catalogue
    - Strips domain prefix
    - Calls existing dmn-lite evaluator
    - Atomically: insert inbox + enqueue result
    - Returns `SubmissionAck(execution_id)`
25. Pub surface: `DmnLiteBusHandler` struct + `start()` function.
26. Tests: decision invocation round-trip.

#### T2B.7 — bpmn-lite-bus-handler crate

27. Implements `ResultService` for bpmn-lite:
    - Receives `InvocationResult`
    - Idempotent inbox check
    - Looks up `bpmn_pending_invocation` by execution_id
    - Atomically: delete pending row + record inbox + call into `bpmn-lite-runtime::advance(process_instance_id, outcome)`
    - Returns `ResultAck`
28. Tests: result arrival → process advance.

#### T2B.8 — bpmn-lite-runtime integration

29. Add migrations for `bpmn_pending_invocation` (per §8.3) and `bpmn_process_instance` (per §8.4).
30. **Drop old migrations 033/034** (per T0 finding J).
31. Submission flow: when bpmn-lite executor reaches a foreign-domain service-task or business-rule-task:
    - Generate `callout_id` (UUIDv7) and `idempotency_key` (UUIDv7)
    - Atomically: insert outbox entry + insert pending-call row (with callout_id, no execution_id yet) + update process to `WaitingOnSubmission(callout_id)`
    - Executor returns; sender task handles transport asynchronously
32. State transition on SubmissionAck (handled by sender task per §8.5):
    - Update pending-call with execution_id + ack_received_at
    - Update process to `WaitingOnInvocation(execution_id)`
33. Result handling (via `bpmn-lite-bus-handler`):
    - Result arrives via `ResultService.DeliverResult`
    - Lookup pending-call by execution_id
    - Bind resolved values into process variable scope
    - Call `advance_internal` to continue walking the workflow

#### T2B.9 — App wiring

34. `bpmn-lite-app`: instantiate `BusClient` (outbound calls), `BusServer` with `ResultService` (inbound results). Start outbox sender task. Connection config via env vars (peer endpoints).
35. `ob-poc-app`: instantiate `BusServer` with `InvocationService` (via `ob-poc-bus-handler`). Instantiate `BusClient` (for sending results back). Start outbox sender task.
36. `dmn-lite-app`: instantiate `BusServer` with `InvocationService` (via `dmn-lite-bus-handler`). Instantiate `BusClient`. Start outbox sender task.
37. Config: env vars for peer endpoints (e.g., `OB_POC_BUS_ENDPOINT`, `DMN_LITE_BUS_ENDPOINT`, `BPMN_LITE_BUS_ENDPOINT`).

#### Manifest generation and import

38. Run `ob-poc-manifest-export`; emit YAML to `bpmn-lite/manifests/ob-poc-v1.0.0.yaml`.
39. Run `dmn-lite-manifest-export`; emit YAML to `bpmn-lite/manifests/dmn-lite-v1.0.0.yaml`.
40. Verify `bpmn-lite-compiler` imports both at build time.

#### Tests (T2B master DoD)

41. End-to-end test: bpmn-lite submits `ob-poc:cbu.create` via bus; ob-poc-app receives, executes verb, returns result; bpmn-lite's `ResultService` receives; process advances. With three real containers (or three local processes mocking containers).
42. End-to-end test: bpmn-lite submits `dmn-lite:cbu_type_routing`; dmn-lite-app receives, evaluates, returns result; bpmn-lite advances.
43. Idempotency test: same idempotency_key submitted twice → same execution_id returned; only one execution.
44. Recovery test: bpmn-lite-app crashes between outbox-write and SubmissionAck; on restart, outbox sender resumes; invocation completes correctly.
45. Recovery test: bpmn-lite-app crashes after SubmissionAck but before pending-call update; on restart, the pending row + outbox row are reconciled (this is the new pre-ack durability case).
46. Version mismatch test: caller declares unknown catalogue_version → receiver returns `REJECTED_VERSION_INCOMPATIBLE`.

#### Crate discipline DoD

47. List every new crate. Justify each.
48. List every new `pub` item across all touched crates. Justify each.
49. No circular deps.
50. cargo doc output shows minimal expected public API per crate.

**DoD:** end-to-end bus invocation works for both ob-poc and dmn-lite verbs. All six test scenarios pass. Manifest export and import work. Crate discipline maintained.

**STOP gate.** Largest tranche. Expect substantial Adam review.

---

### T3 — BPMN executor as async state machine (RIP-AND-REPLACE)

**Goal:** BPMN process instances advance through pause/persist/resume state machine with two-stage callout durability.

**Disposition:** RIP-AND-REPLACE.

**Crates touched:**
- `bpmn-lite-runtime` (significant rebuild)

**Sonnet tasks:**

1. **Rip prior v0.3 T3 work:**
   - `bpmn-lite-engine/src/bpmn_executor.rs` — DELETE
   - `bpmn-lite-engine/src/demo_invoker.rs` — DELETE
   - `bpmn-lite-engine/src/lifecycle.rs`, `event_bus.rs`, `subscriber.rs`, `sage_observer.rs` — DELETE (already absorbed into T2 rip)
   - Move to `_deprecated/` for review or git-rm per Adam's call
2. **Executor API** (rebuilt):
   - `start_process(workflow_source, initial_variables) → Result<ProcessInstanceId>`
   - `advance(instance_id, outcome) → Result<()>` — called by `bpmn-lite-bus-handler` on result arrival
   - `cancel(instance_id) → Result<()>`
   - All non-blocking.
3. **advance_internal** — synchronous walking slice:
   - Walks through gateway evaluations, intra-process arithmetic, anything not requiring callout
   - Stops at next callout (service-task, business-rule-task) or end event
   - At callout: generate callout_id + idempotency_key; submit via `bus_client.submit_invocation`; persist pending-call + outbox + process state; return
   - At end event: mark process Completed; emit BpmnInstanceCompleted
4. **Two-stage durability handling:**
   - On callout submission: insert pending-call with callout_id, idempotency_key set, execution_id NULL; insert outbox; set process to WaitingOnSubmission(callout_id)
   - Sender task (in `dsl-bus-client`) updates execution_id and transitions process to WaitingOnInvocation when SubmissionAck arrives
   - Result arrival deletes pending-call and calls advance()
5. **Failure handling:**
   - VerbFailed → mark process Failed
   - OptimisticConflict → single automatic retry with new idempotency_key; mark Failed if still conflicts
   - LockTimeout / TimedOut → Failed
   - VersionMismatch → Failed (manual intervention needed)
6. **State invariants:**
   - At every commit, process_instance row reflects truth
   - No in-memory state required to advance
   - Restart mid-WaitingOnSubmission: outbox sender resumes; state correct
   - Restart mid-WaitingOnInvocation: result delivery on reconnect; advances
7. **Tests:**
   - Async-correctness: start_process returns immediately; status=WaitingOnSubmission; no thread parked
   - Long-wait test: process in WaitingOnSubmission for >10s with bus down; no resources held
   - Restart-mid-WaitingOnSubmission: kill bpmn-lite, restart, verify outbox sender resumes
   - Restart-mid-WaitingOnInvocation: kill bpmn-lite, restart, simulate result delivery, verify advance
   - Full §10 demo: end-to-end for all three CBU type paths through real bus

**Crate discipline DoD:** new pub items justified. `bpmn-lite-runtime` doesn't leak bus internals or compiler internals.

**DoD:** executor genuinely async (long-wait passes); §10 demo runs end-to-end via real bus for all three paths; restart recovery works at both stages.

**STOP gate.**

---

### T4 — Pre-coded demo BPMN model (MECHANICAL UPDATE)

**Goal:** §10 model is one function call away; runs against federated stack.

**Disposition:** MECHANICAL UPDATE.

**Crates touched:**
- `bpmn-lite-engine/src/demo.rs` (or replacement)

**Sonnet tasks:**

1. Rewrite `custody_cbu_onboarding_source()` to match §10 model exactly (with corrected verb names — `ob-poc:cbu.add-product` not `ob-poc:cbu.add_fund_product`).
2. Update `demo_placeholder_registry()` to declare bindings for namespaced verbs against imported manifest.
3. Demo seed:
   - Verify ob-poc catalogue contains `cbu.create`, `cbu.add-product`, `instrument-matrix.attach`
   - Verify dmn-lite catalogue contains `cbu_type_routing`
   - Sample CBU input data for fund / corporate / trust types
4. Integration test:
   - Compile §10 model via T1 pipeline
   - `start_process` with fund-type CBU input → wait for completion → verify Completed with "Operational" end state
   - Repeat for corporate-type, trust-type
   - Verify audit trail in bpmn-lite, ob-poc, and dmn-lite Postgres instances (cross-domain audit)
5. Reset helper: `reset_demo_state()` truncating bpmn_process_instance, bpmn_pending_invocation, outbox, inbox.

**DoD:** demo workflow constructible; integration test verifies all three paths complete; reset helper works; cross-domain audit visible.

**STOP gate.**

---

### T5 — Sage agentic integration (PARTIALLY RIP-AND-REPLACE)

**Goal:** Sage routes through bus; reasoning persisted.

**Disposition:** PARTIALLY RIP-AND-REPLACE.

**Crates touched:**
- `sage-integration` (NEW or modify existing)
- `bpmn-lite-runtime` (modify): one service-task routes through Sage

**Sonnet tasks:**

1. Sage subscribes to lifecycle events from the bus (via `bus-server::register_subscriber` mechanism if available, or via direct audit-table polling — fallback per T0 item 19).
2. Sage decision point in demo flow: `instrument-matrix.attach` (post-convergence). When BPMN executor reaches this node, marked Sage-mediated.
3. Sage:
   - Receives lifecycle event for prior node
   - Reads current process state from durable storage
   - Walks Semantic Dependency Graph to confirm legal next-step (`ob-poc:instrument-matrix.attach`)
   - Submits the actual verb invocation via the bus (calling ob-poc)
   - Reasoning recorded in audit with `actor: Sage`
4. Tests: Sage-mediated service task completes process; reasoning captured; works for all three CBU type paths.

**Fallback:** if Sage cannot submit plans via bus, observation mode — subscribes, presents reasoning, doesn't submit.

**DoD:** at least one service task through Sage; reasoning persisted; process completes for all three paths.

**STOP gate.**

---

### T6 — ob-poc UI repointing (SURVIVES)

**Goal:** existing UI displays bpmn-lite process state through three-domain federation.

**Disposition:** SURVIVES.

**Sonnet tasks:**

1. Verify UI still works against `bpmn-lite-api` after T2/T3 rebuilds.
2. Update endpoints if needed: REST surface preserved per T0 item 27.
3. **Cross-domain visibility:** UI shows callout `target_domain` explicitly (e.g., "Calling `ob-poc:cbu.create`", "Evaluating `dmn-lite:cbu_type_routing`"). New small UI element showing the federation in action.
4. Tests: manual walkthrough; all four panels populate correctly.

**DoD:** UI displays demo running across all panels for all three paths; cross-domain calls visible.

**STOP gate.**

---

### T7 — Docker deployment integration (RIP-AND-REPLACE)

**Goal:** federated stack with three apps + three Postgres runs via docker-compose.

**Disposition:** RIP-AND-REPLACE.

**Sonnet tasks:**

1. Containers:
   - `bpmn-lite-app` (Rust binary)
   - `bpmn-lite-postgres` (16-bookworm)
   - `ob-poc-app` (Rust binary; includes ob-poc-bus-handler)
   - `ob-poc-postgres` (16-bookworm)
   - `dmn-lite-app` (Rust binary; includes dmn-lite-bus-handler)
   - `dmn-lite-postgres` (16-bookworm)
   - `ob-poc-ui` (existing frontend)
2. Networking: single docker-compose network; service-name DNS resolution
3. Env vars: each app knows peer endpoints (e.g., `bpmn-lite-app` knows `ob-poc-app:50051` and `dmn-lite-app:50051`)
4. Migrations on startup per domain
5. Demo seed loaded
6. Single command: `docker-compose up`
7. Reset script: clears all three Postgres tables
8. Async correctness verified across container boundaries (>10s wait)
9. Tests: cold start; demo runs end-to-end dockerised; recovery across container restart

**DoD:** `docker-compose up` brings federated stack live; reset works; demo verified in Docker.

**STOP gate.**

---

### T8 — Demo polish + rehearsal (NEW)

**Goal:** demo runs cleanly 5× consecutively against federated stack.

**Disposition:** NEW.

**Sonnet tasks:**

1. Scripted demo flow (with three-domain federation visible at each step)
2. Speaker notes:
   - What to say at each step
   - What to point at on UI
   - **How to talk about federation** when foundational services ask (use §13 prepared answers)
3. Demo data variations: 3 inputs producing fund / corporate / trust paths
4. Failure recovery: verb fails, DMN times out, bus partition (kill ob-poc-app mid-call), restart mid-call — documented
5. Rehearsal: 5 consecutive runs; capture flakiness; fix; repeat
6. Backup material: screenshots of each beat
7. Q&A prep: §13 answers reviewed

**DoD:** 5 consecutive clean runs; speaker notes complete; failure recovery documented; backup ready; Q&A internalised.

**STOP gate. Demo ready.**

---

## 10. Demo BPMN model (locked — corrected per T0 audit gap E)

CBU lifecycle — three-domain federation with one exclusive gateway routed by DMN. All cross-domain calls visible on every node.

```scheme
(workflow custody-cbu-onboarding
  (start-event :id start :next create-cbu)
  
  (service-task :id create-cbu 
                :verb ob-poc:cbu.create 
                :inputs (name @input-name, client_type @input-client-type)
                :next type-decision)
  
  (business-rule-task :id type-decision 
                      :decision dmn-lite:cbu_type_routing
                      :inputs (cbu_client_type @input-client-type)
                      :next type-gateway)
  
  (exclusive-gateway :id type-gateway
    (flow :condition (= @cbu-type "fund")      :next add-fund-product)
    (flow :condition (= @cbu-type "corporate") :next add-corp-product)
    (flow :condition (= @cbu-type "trust")     :next add-trust-product))
  
  (service-task :id add-fund-product  
                :verb ob-poc:cbu.add-product 
                :inputs (cbu @cbu, product_type "fund")
                :next attach-im)
  
  (service-task :id add-corp-product  
                :verb ob-poc:cbu.add-product 
                :inputs (cbu @cbu, product_type "corporate")
                :next attach-im)
  
  (service-task :id add-trust-product 
                :verb ob-poc:cbu.add-product 
                :inputs (cbu @cbu, product_type "trust")
                :next attach-im)
  
  (service-task :id attach-im 
                :verb ob-poc:instrument-matrix.attach 
                :inputs (cbu @cbu)
                :next end)
  
  (end-event :id end :status "Operational"))
```

Catalogue verbs used (all real):
- `ob-poc:cbu.create` (catalogued in cbu.yaml:11)
- `ob-poc:cbu.add-product` (existing in catalogue)
- `ob-poc:instrument-matrix.attach` (existing in catalogue)
- `dmn-lite:cbu_type_routing` (existing decision)

Implicit placeholders:
- `@cbu` — produced by `cbu.create`, consumed by `add-*-product` and `instrument-matrix.attach`
- `@cbu-type` — produced by `cbu_type_routing` decision, consumed by gateway predicates

Explicit inputs:
- `@input-name`, `@input-client-type` — provided by `start_process` initial_variables

Demo benefits from this model:
- Every callout crosses the bus (every service-task is `ob-poc:*`; business-rule-task is `dmn-lite:*`)
- Three distinct paths visible on UI (gateway routing visible)
- Same `cbu.add-product` verb called three ways (argument-driven, not verb-name-driven) — actually a cleaner architectural story
- Real catalogue verbs (no demo-only verb additions to ob-poc)

---

## 11. Master Demo DoD

Plan complete when all simultaneously true:

1. bpmn-dsl source compiles via parse / lint / DAG pipeline with namespaced verb resolution
2. `@cbu` placeholder inference works without explicit declarations
3. Manifests from ob-poc and dmn-lite imported correctly at build time
4. gRPC bus established between all three domains
5. Outbox/inbox pattern handles delivery durability and idempotent receive
6. execution_id is the cross-domain correlation identity; callout_id provides pre-ack durability
7. BPMN executor is verifiably async (long-wait test passes)
8. §10 demo runs end-to-end through all three CBU type paths
9. All cross-domain calls go through bus (no in-process shortcuts)
10. Both placeholder resolutions (`@cbu`, `@cbu-type`) work across cross-domain invocations
11. At least one service task routes through Sage with persisted reasoning
12. ob-poc UI displays workflow / plans / Sage / DMN with `target_domain` visibility
13. Three-container federated stack runs via `docker-compose up` from clean state
14. Async correctness verified across container boundaries (>10s WaitingOnSubmission works)
15. Restart recovery works at both durability stages (WaitingOnSubmission and WaitingOnInvocation)
16. Demo runs cleanly 5× consecutively
17. Speaker notes complete; failure recovery documented; Q&A prepared
18. Crate discipline maintained per §5.9 — every new `pub` justified; no super-crates

---

## 12. Risk register

**R1: Demo verbs adjusted per T0 audit gap E.** Mitigated by using existing `cbu.add-product` with type as argument; preserves gateway branching visually.

**R2: `WorkflowExecutionPlan` vs `ExecutablePlan` distinction may cause confusion.** Mitigated by §5.5 clarification; T1 task 4 makes the distinction explicit in code.

**R3: dmn-lite Postgres deployment — does dmn-lite already have a database layer or is it stateless?** T0 didn't fully cover; T2B/T7 may surface adjustment. Mitigation: if dmn-lite is stateless decision evaluator only, outbox/inbox added to it; Postgres added for that purpose; trivial scope.

**R4: gRPC + tonic across three containers may surface networking issues.** Mitigated by standard docker-compose patterns; tonic mature.

**R5: Outbox sender ordering bugs under load.** Mitigated by at-least-once + idempotent inbox; explicit tests in T2A and T2B.

**R6: Async correctness verification fails — something parks a thread.** Mitigated by long-wait test in T3.

**R7: Manifest version skew during demo (e.g., regenerated mid-run).** Mitigated by demo lockdown — manifests generated once before demo; not regenerated during.

**R8: Pre-ack crash window (caller crashes between outbox-write and SubmissionAck handling) leaves orphaned outbox entries.** Mitigated by outbox sender's resumption on startup; sender treats every pending entry as "may have been partially submitted; idempotency_key handles retry safety."

**R9: Crate discipline slips.** Mitigated by per-tranche pub-additions report; Adam reviews.

**R10: T8 rehearsal reveals integration bugs.** Mitigated by time-boxed fix-and-repeat; fallback most-stable-subset + screenshots.

---

## 13. Demo Q&A preparation

### Q: How does this handle long-running workflows?

The DSL engine in each domain emits typed lifecycle events on every committed execution. Each domain in the federation has its own bus subscriber. Cross-domain invocations are recorded in a two-stage durable pending-call mechanism — `callout_id` while the call is in flight to the receiver, `execution_id` once the receiver acknowledges. When the result arrives — milliseconds later for fast verbs, hours later for human tasks — the subscriber looks up the pending invocation, binds resolved values into the process variable scope, and advances the workflow. The executor never blocks. Process state is durable at every transition. If a runtime restarts mid-call, the outbox sender resumes from where it left off. Same code path handles a 50-millisecond DMN evaluation and a 14-day user task.

### Q: How does this scale across multiple domains?

Each domain is independently deployable with its own Postgres, its own engine instance, its own catalogue. Domains communicate via gRPC over HTTP/2 — typed protobuf, no shared state, no broker dependency. Adding a new domain is: (1) the new domain implements the standard bus protocol, (2) other domains import its catalogue manifest at build time, (3) calls to the new domain's verbs work the same as any other federated call. Each domain owns its own data, audit, and authority. Today we're demonstrating bpmn-lite, ob-poc, and dmn-lite — three domains. Form.io would slot in as a fourth in exactly the same shape.

### Q: What's the architectural model here?

Each verb is analogous to a stored procedure. The catalogue manifest is the public API surface — names, typed signatures, behavioural metadata. Consumer domains import the manifest; their compilers validate every reference at build time, exactly like a SQL client validates queries against a known schema. The bus is the wire protocol — typed calls, typed results, authority-controlled. The mechanism is well-understood; what's novel is the application at the verb level rather than the SQL level.

### Q: What happens if a domain goes down?

Outbox pattern handles it. Each domain has a transactional log of messages it intends to send. If a peer is unreachable, messages stay in the outbox; the sender retries with exponential backoff. When the peer recovers, the outbox drains. Each side keeps state durable in its own Postgres; no shared infrastructure to lose. Network partitions, peer crashes, runtime restarts all resolve via the same mechanism.

### Q: How do you handle duplicate messages?

idempotency_key in protobuf metadata, distinct from the execution identity. Caller generates it per logical invocation. Receiver dedupes via a transactional inbox table. If the same idempotency_key arrives twice (network retry, sender restart), the receiver returns the same execution_id it returned the first time; only one actual execution occurs. At-least-once delivery with idempotent receive = effectively-once execution.

### Q: Why three domains for the demo?

Each is a different *kind* of domain. bpmn-lite is orchestration. ob-poc is business entity mutations. dmn-lite is decision evaluation. The federation isn't just "BPMN calls a backend" — it's a genuine multi-role platform where workflow logic, business logic, and decision logic each have their own home, their own governance, their own deployment. Adding human-interaction (Form.io), analytics (a metrics domain), or compliance (a policy enforcement domain) follows the same pattern.

### Q: Why not use a message broker like Kafka or NATS?

Direct peer-to-peer gRPC was the right call for our scale and architectural style. Brokers add operational complexity (HA clustering, partition management) not justified for our throughput. Each domain's outbox in its own Postgres gives us durability without a broker dependency. The protocol layer is transport-agnostic — if scale ever demands NATS, the swap is localised; the protocol doesn't move.

### Q: How do you handle catalogue evolution?

Catalogues are versioned. Each domain publishes manifest at a known version; consumers import at build time pinned to that version. The catalogue_version is carried in every bus call. Receiver validates. Schema migration follows the same pattern SQL clients use — version-aware clients gracefully degrade.

### Q: What about transactions across domains?

Each invocation is its own transaction within the receiving domain. Cross-domain distributed transactions are deliberately out of scope. We use sagas (compensating actions) where atomicity spans domains; we rely on at-least-once-with-idempotent-receive for most retry cases. Standard distributed systems patterns.

### Q: How does Sage fit in?

Sage is a subscriber peer in the federation. Same status as bpmn-lite. Subscribes to lifecycle events from the bus, walks the Semantic Dependency Graph to reason about state, submits its own plans when it makes decisions. No privileged access to domain internals — same catalogue-bound, authority-controlled invocation path as anything else. The agent is a peer, not a side-channel.

---

## 14. Execution conventions

- One tranche per session
- No commits without review
- Progress markers per major step
- No improvisation outside tranche scope
- Phase 5 engine is closed (consumed as workspace deps only)
- Rip-and-replace by default; mechanical updates only where explicitly specified
- No `block_on()` in async paths
- Crate discipline non-negotiable; pub additions reported per tranche

---

## 15. Status tracking

```
Phase 5.5 v0.6 — federated DSL platform demo deployment
T0 ✅  T1 ☐  T2A ☐  T2B ☐  T3 ☐  T4 ☐  T5 ☐  T6 ☐  T7 ☐  T8 ☐
Status: T0 complete; awaiting T1
```

End of Phase 5.5 plan v0.6.
