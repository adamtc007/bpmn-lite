# Phase 5.5 v0.6 Addendum: Federated Services, Validation, Dispatch Transparency

| Field | Value |
| --- | --- |
| Document ID | OB-POC-PHASE-5_5-ADDENDUM-003 |
| Version | v0.6-A3 |
| Status | ADDENDUM to v0.6 plan; in addition to v0.6-A2 (signal/cargo/transport separation) |
| Author | Adam Cearns |
| Date | 2026-05-20 |
| Applies to | Phase 5.5 plan v0.6 — affects §5 architecture, T2A.1 protocol, T2B.5 ob-poc-bus-handler, T2B.6 dmn-lite-bus-handler, T2B.9 app wiring; introduces concepts referenced by future T3+ work |
| Instruction to Sonnet | Drop point: after A2 implementation lands and T2B.8 manifest export/import completes. Insert *before* T2B.9 finishes. Adds three RPCs to bus protocol (Validate, EntityService.Resolve, SemOsService.FetchDagPacks) with stub implementations sufficient for protocol shape and wiring. Full implementations land in engine V&S v0.4 post-demo. |

---

## 0. What this addendum establishes

This addendum names four architectural principles that complete the federated DSL platform's identity. Each is implemented in v0.6 to whatever degree the demo requires; each is documented as a first-class architectural principle for engine V&S v0.4 and beyond.

The principles:

1. **Three federated services per domain.** Each domain exposes (potentially) three RPC surfaces: verb execution (InvocationService — already in v0.6), entity resolution (EntityService — new), and SemOS DAG pack retrieval (SemOsService — new). Capability-based: a domain implements what it has.

2. **Validation is a service.** A separate `Validate` RPC on InvocationService lets callers ask "would this verb succeed if invoked now?" without side effects. Used by REPL during authoring, by workflows at start-of-execution, by Sage during reasoning.

3. **Dispatch transparency.** Above the dispatch layer, code does not distinguish local from remote verb invocation. A unified `VerbDispatcher` abstraction routes per catalogue declaration. The REPL, workflow executors, and Sage all interact uniformly with verbs regardless of where they execute.

4. **Multi-surface authoring.** REPL, BPMN, and Sage are peer authoring surfaces on the same federation. Each uses the same dispatcher, the same validation, the same audit. The user experience is invariant under deployment topology and uniform across authoring choice.

These four principles together complete what was started in v0.6 and refined in A2: a federated platform whose architecture is invariant under deployment, transparent above the transport, and complete in its inter-domain communication surfaces.

---

## 1. Architectural principles (the cuts that matter)

### Principle 1: Three federated services per domain

A domain in this platform may expose:

- **InvocationService** — verb execution and validation. Universal; every domain has verbs to invoke.
- **EntityService** — entity resolution: existence checks, natural-key-to-UUID resolution, FSM state queries, cascade reference semantics. Present for domains that hold entity state (e.g., ob-poc). Absent for stateless domains (e.g., dmn-lite, where decision tables are pure functions).
- **SemOsService** — SemOS DAG pack retrieval: semantic graphs, constellation maps, derivation chains, FSM applicability descriptors for verbs in this domain. Present for domains that publish semantic grounding for their verbs.

Capability-based: each domain implements what its content warrants. The bus protocol defines all three; the manifest declares which a domain provides; consumers query only what's declared.

For demo configuration:
- ob-poc implements all three (verbs + entities + SemOS DAG packs)
- dmn-lite implements only InvocationService (decisions are stateless; no entities; no DAG packs)
- bpmn-lite implements ResultService (per v0.6) plus InvocationService for any verbs it exposes (likely none in demo scope)

### Principle 2: Validation is a service

Validation answers: "would this verb succeed if invoked now?" It does not execute. It does not produce side effects. It resolves references, checks FSM preconditions, verifies authority, confirms catalogue version compatibility.

Validation lives on the receiver (only the receiver has the data needed to validate against). Validation is exposed as `InvocationService.Validate` — same request shape as Submit, distinct semantics.

Callers use Validate before composing or committing:
- REPL calls Validate during authoring; user iterates until "would succeed"
- Workflows call Validate at start-of-execution; if all callouts validate, transition to Running; if any fail, fail-fast with structured detail
- Sage calls Validate before submitting; decides whether to proceed or rethink

Validation is *not* a guarantee of success (TOCTOU between Validate and Submit means state can change). It is structured information that lets callers reason about likely success and surface failures before side effects.

### Principle 3: Dispatch transparency

The REPL, workflow executors, and Sage hold a `VerbDispatcher` abstraction. They invoke verbs through it. They do not know whether the verb is local (in-process engine) or remote (via bus to another domain).

```rust
pub trait VerbDispatcher {
    async fn validate(&self, call: ResolvedCall) -> Result<ValidationOutcome>;
    async fn dispatch(&self, call: ResolvedCall) -> Result<InvocationOutcome>;
}
```

A `DispatchRouter` implements `VerbDispatcher` by routing per catalogue declaration:
- Local verbs → in-process engine
- Remote verbs → bus client → gRPC to target domain

The dispatch detail is the only place that knows about transport. Everything above operates on a unified API.

**Why this matters:**
- Refactor stability: verbs moving between domains, or domains being co-deployed/distributed, are configuration changes, not code changes
- Compositional integrity: every authoring surface inherits transparency automatically
- Cognitive integrity: the abstraction lives at the right level (verbs, not transport)

### Principle 4: Multi-surface authoring

The federation supports multiple authoring surfaces, each with the same architectural privileges:

| Surface | Authoring style | Use case |
|---|---|---|
| **REPL** | Interactive / programmatic; one call at a time | Development, exploration, runbook composition |
| **BPMN** | Declarative workflow; long-running with durable state | Production workflows with visible structure, recovery |
| **Sage** | Autonomous agent; reasons and composes calls | Agentic automation, decision support |
| **Future** | Scheduled batch, event-driven scripts, etc. | TBD |

All use the same `VerbDispatcher`. All use the same `Validate` RPC for pre-commit checks. All produce audit records in the same shape. All flow through the same bus.

**User experience invariance:** a Sage/REPL session in BPMN behaves the same as a native ob-poc DSL session. The user (or agent) is not aware of distribution — verbs resolve, dispatch, return outcomes. The mechanism is plumbing.

---

## 2. Protocol additions

### 2.1 InvocationService.Validate

```protobuf
service InvocationService {
  rpc Submit(InvocationRequest) returns (SubmissionAck);
  rpc Validate(InvocationRequest) returns (ValidationResult);   // NEW
}

message ValidationResult {
  ValidationOutcome outcome = 1;
  repeated ValidationIssue issues = 2;
  // execution_id only populated if outcome == NOT_IMPLEMENTED (for audit tracing)
  Uuid validation_id = 3;
}

enum ValidationOutcome {
  VALIDATION_UNSPECIFIED = 0;
  WOULD_SUCCEED = 1;
  WOULD_FAIL = 2;
  NOT_IMPLEMENTED = 3;          // stub for v0.6 demo
}

message ValidationIssue {
  string field_name = 1;        // which input has the issue, or "" for verb-level
  string issue_kind = 2;        // "unknown_reference" | "fsm_precondition" | "authority" | "version_skew" | "verb_unknown"
  string detail = 3;            // human-readable detail
  // Optional structured payloads for richer detail:
  oneof specific {
    UnknownReferenceDetail unknown_reference = 10;
    FsmPreconditionDetail fsm_precondition = 11;
    AuthorityDetail authority = 12;
  }
}

message UnknownReferenceDetail {
  string expected_type = 1;
  bytes provided_uuid = 2;      // empty if natural key was used
  string provided_natural_key = 3;
}

message FsmPreconditionDetail {
  string entity_uuid = 1;
  string current_state = 2;
  repeated string allowed_states = 3;
}

message AuthorityDetail {
  string required_scope = 1;
  repeated string provided_scopes = 2;
}
```

### 2.2 EntityService (new)

```protobuf
service EntityService {
  rpc Resolve(EntityResolutionRequest) returns (EntityResolutionResult);
}

message EntityResolutionRequest {
  AuthorityContext authority = 1;
  repeated EntityQuery queries = 2;
  string catalogue_version = 3;
}

message EntityQuery {
  string entity_type = 1;        // e.g., "CBU"
  oneof lookup_by {
    bytes uuid = 2;
    string natural_key = 3;
  }
  bool include_state = 4;        // if true, return FSM state alongside resolution
  bool include_audit_pointer = 5; // if true, return latest audit reference
}

message EntityResolutionResult {
  repeated EntityResolution resolutions = 1;
  Uuid resolution_id = 2;        // for audit tracing
}

message EntityResolution {
  ResolutionOutcome outcome = 1;
  bytes resolved_uuid = 2;       // populated on Resolved
  string entity_type = 3;
  string current_state = 4;      // populated if include_state was true
  string audit_reference = 5;    // populated if include_audit_pointer was true
  string detail = 6;             // populated on NotFound or other outcomes
}

enum ResolutionOutcome {
  RESOLUTION_UNSPECIFIED = 0;
  RESOLVED = 1;
  NOT_FOUND = 2;
  AMBIGUOUS = 3;                 // multiple matches for natural key
  AUTHORITY_DENIED = 4;
  NOT_IMPLEMENTED = 5;           // stub for v0.6 demo
}
```

### 2.3 SemOsService (new)

```protobuf
service SemOsService {
  rpc FetchDagPacks(DagPackRequest) returns (DagPackResponse);
}

message DagPackRequest {
  AuthorityContext authority = 1;
  repeated string dag_pack_ids = 2;     // specific DAG packs requested
  repeated string verb_ids = 3;          // OR: fetch DAG packs grounding these verbs
  bool include_constellation_maps = 4;
  bool include_derivation_chains = 5;
  bool include_fsm_applicability = 6;
  string catalogue_version = 7;
}

message DagPackResponse {
  repeated DagPack packs = 1;
  Uuid response_id = 2;
}

message DagPack {
  string pack_id = 1;
  string domain = 2;
  string version = 3;
  bytes semantic_graph = 4;              // serialised semantic graph (format TBD; for stub, empty)
  repeated ConstellationMapRef constellation_maps = 5;
  repeated DerivationChainRef derivation_chains = 6;
  FsmApplicabilityMatrix fsm_applicability = 7;
  string detail = 8;                     // for stub: explains NOT_IMPLEMENTED
  DagPackOutcome outcome = 9;
}

message ConstellationMapRef {
  string map_id = 1;
  bytes map_payload = 2;
}

message DerivationChainRef {
  string chain_id = 1;
  bytes chain_payload = 2;
}

message FsmApplicabilityMatrix {
  bytes matrix_payload = 1;
}

enum DagPackOutcome {
  DAG_PACK_UNSPECIFIED = 0;
  AVAILABLE = 1;
  NOT_FOUND = 2;
  AUTHORITY_DENIED = 3;
  NOT_IMPLEMENTED = 4;                   // stub for v0.6 demo
}
```

### 2.4 Service discovery in manifest

The catalogue manifest declares which services a domain implements:

```yaml
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"

# NEW: declared services
services:
  - kind: "InvocationService"
    available: true
    capabilities: ["Submit", "Validate"]    # Validate is stubbed in demo
  - kind: "EntityService"
    available: true
    capabilities: ["Resolve"]                # stubbed in demo
  - kind: "SemOsService"
    available: true
    capabilities: ["FetchDagPacks"]          # stubbed in demo

# Existing fields unchanged
verbs: [...]
decisions: [...]
types: [...]
```

For dmn-lite:

```yaml
manifest_version: "1.0"
domain: "dmn-lite"
catalogue_version: "v1.0.0"

services:
  - kind: "InvocationService"
    available: true
    capabilities: ["Submit", "Validate"]    # Validate is stubbed
  # No EntityService — dmn-lite is stateless
  # No SemOsService — decisions are self-contained

verbs: []
decisions:
  - id: "cbu_type_routing"
    ...
```

This is **capability-based**. Consumers know what's available; they don't probe blindly.

---

## 3. Implementation in v0.6

### 3.1 T2A.1 — dsl-bus-protocol (additions)

Add the proto definitions per §2 above. Compile via tonic build script. Generated Rust types and traits become available to all downstream crates.

**Estimated work:** ~200 LOC of protobuf + tonic-generated code.

### 3.2 T2A.3 — dsl-manifest (additions)

Extend the `Manifest` type to include `services: Vec<ServiceDeclaration>`. Add types:

```rust
pub struct ServiceDeclaration {
    pub kind: ServiceKind,
    pub available: bool,
    pub capabilities: Vec<String>,
}

pub enum ServiceKind {
    InvocationService,
    EntityService,
    SemOsService,
}
```

The manifest loader parses these from YAML. The validator accepts them. The lookup API adds `manifest.declared_services() -> &[ServiceDeclaration]`.

**Estimated work:** ~100 LOC.

### 3.3 T2B.5 — ob-poc-bus-handler (stub additions)

Implement `EntityService` and `SemOsService` as stub services on ob-poc's bus server.

```rust
// In ob-poc-bus-handler
pub struct ObPocEntityServiceImpl { /* ... */ }
pub struct ObPocSemOsServiceImpl { /* ... */ }

#[tonic::async_trait]
impl EntityService for ObPocEntityServiceImpl {
    async fn resolve(
        &self,
        _request: Request<EntityResolutionRequest>,
    ) -> Result<Response<EntityResolutionResult>, Status> {
        // Stub: return NOT_IMPLEMENTED for all queries
        Ok(Response::new(EntityResolutionResult {
            resolutions: vec![EntityResolution {
                outcome: ResolutionOutcome::NotImplemented as i32,
                detail: "EntityService stub; full implementation in engine V&S v0.4".into(),
                ..Default::default()
            }],
            resolution_id: Some(Uuid { value: uuid_v7().as_bytes().to_vec() }),
        }))
    }
}

#[tonic::async_trait]
impl SemOsService for ObPocSemOsServiceImpl {
    async fn fetch_dag_packs(
        &self,
        _request: Request<DagPackRequest>,
    ) -> Result<Response<DagPackResponse>, Status> {
        // Stub: return NOT_IMPLEMENTED
        Ok(Response::new(DagPackResponse {
            packs: vec![DagPack {
                outcome: DagPackOutcome::NotImplemented as i32,
                detail: "SemOsService stub; full implementation in engine V&S v0.4".into(),
                ..Default::default()
            }],
            response_id: Some(Uuid { value: uuid_v7().as_bytes().to_vec() }),
        }))
    }
}
```

Also implement `InvocationService.Validate` as stub:

```rust
async fn validate(
    &self,
    _request: Request<InvocationRequest>,
) -> Result<Response<ValidationResult>, Status> {
    Ok(Response::new(ValidationResult {
        outcome: ValidationOutcome::NotImplemented as i32,
        issues: vec![ValidationIssue {
            issue_kind: "not_implemented".into(),
            detail: "Validate stub; full implementation in engine V&S v0.4".into(),
            field_name: String::new(),
            specific: None,
        }],
        validation_id: Some(Uuid { value: uuid_v7().as_bytes().to_vec() }),
    }))
}
```

**Estimated work:** ~150 LOC across the stubs.

### 3.4 T2B.6 — dmn-lite-bus-handler (stub additions)

Implement only `InvocationService.Validate` as stub. **Does not implement** EntityService or SemOsService — dmn-lite is stateless; no entities; no DAG packs to publish.

This is the capability-based pattern in action. dmn-lite's manifest declares only InvocationService; consumers know not to query EntityService or SemOsService against dmn-lite.

**Estimated work:** ~30 LOC for the Validate stub.

### 3.5 T2B.9 — app wiring (additions)

Each domain's main() instantiates the bus server with all the services it declares:

```rust
// ob-poc-app main()
let bus_server = BusServer::builder()
    .invocation_service(invocation_impl)
    .result_service(result_impl)         // for results from peers (e.g., bpmn-lite acks)
    .entity_service(entity_impl)          // NEW: stubbed
    .sem_os_service(sem_os_impl)         // NEW: stubbed
    .outbox_notifier(notifier.clone())
    .listen_addr("0.0.0.0:50051".parse()?)
    .build()?;

bus_server.serve().await?;
```

```rust
// dmn-lite-app main()
let bus_server = BusServer::builder()
    .invocation_service(invocation_impl)
    .result_service(result_impl)
    .outbox_notifier(notifier.clone())
    .listen_addr("0.0.0.0:50052".parse()?)
    .build()?;
// No entity_service / sem_os_service — dmn-lite doesn't expose these
```

```rust
// bpmn-lite-app main()
let bus_server = BusServer::builder()
    .result_service(result_impl)          // primary: receive results from peers
    .invocation_service(invocation_impl) // optional: bpmn-lite verbs (e.g., process.start)
    .outbox_notifier(notifier.clone())
    .listen_addr("0.0.0.0:50053".parse()?)
    .build()?;
```

The `BusServer` builder accepts optional service implementations. Missing services result in gRPC returning UNIMPLEMENTED for those routes — which is correct behaviour (consumers should consult the manifest before calling).

**Estimated work:** ~100 LOC across the three apps' main() wiring.

### 3.6 Manifest generation (T2B.3 and T2B.4)

The manifest exporters need to emit the new `services` section:

- `ob-poc-manifest-export`: emits all three services with their declared capabilities
- `dmn-lite-manifest-export`: emits only InvocationService

**Estimated work:** ~50 LOC across both exporters.

### 3.7 Test additions

For each new RPC, basic stub tests:

```rust
#[tokio::test]
async fn validate_stub_returns_not_implemented() {
    let server = test_bus_server().await;
    let response = server.invocation_client.validate(test_request()).await.unwrap();
    assert_eq!(response.outcome, ValidationOutcome::NotImplemented as i32);
}

#[tokio::test]
async fn entity_resolve_stub_returns_not_implemented() {
    // ... similar
}

#[tokio::test]
async fn sem_os_fetch_dag_packs_stub_returns_not_implemented() {
    // ... similar
}

#[tokio::test]
async fn dmn_lite_does_not_expose_entity_service() {
    // ... verify gRPC UNIMPLEMENTED response when calling EntityService against dmn-lite
}
```

**Estimated work:** ~80 LOC of test code.

### 3.8 Total implementation cost

Sum: roughly 700 LOC across the affected crates. About one full day of Sonnet work, give or take depending on test depth.

This is *small* relative to the architectural integrity it preserves.

---

## 4. What is NOT in scope for v0.6

These remain for engine V&S v0.4 / post-demo work:

- **Real implementation of `InvocationService.Validate`** — actual entity resolution, FSM state checks, authority validation against the receiver's stores
- **Real implementation of `EntityService.Resolve`** — natural-key-to-UUID resolution, existence checks, state queries against ob-poc's entity store
- **Real implementation of `SemOsService.FetchDagPacks`** — DAG pack retrieval, constellation maps, derivation chains against SemOS infrastructure
- **`DispatchRouter` abstraction layer** — unified `VerbDispatcher` with local and remote routing. Belongs in V&S v0.4 / T3+ refactoring. For v0.6 demo, bpmn-lite executor calls `dsl-bus-client` directly; refactor to use `DispatchRouter` later.
- **REPL extended to use DispatchRouter** — ob-poc REPL gaining the ability to compose remote calls. Pure post-demo work.
- **Sage integration via DispatchRouter primitives** — Sage refactored to use the unified dispatcher rather than domain-specific shortcuts.
- **Workflow-start validation phase** — BPMN workflows running Validate on all callouts before transitioning to Running. Belongs in T3 or V&S v0.4.

These additions, when they land, slot into the protocol shape established in v0.6. No protocol changes required.

---

## 5. v0.6 plan changes

### §4 Non-goals (updates)

**Add to non-goals:**
- "Real implementation of `InvocationService.Validate` (stubbed in demo; full implementation in engine V&S v0.4)"
- "Real implementation of `EntityService.Resolve` (stubbed in demo; full implementation post-demo)"
- "Real implementation of `SemOsService.FetchDagPacks` (stubbed in demo; full implementation post-demo)"
- "`DispatchRouter` abstraction layer (post-T3 refactoring; v0.6 has bpmn-lite executor calling `dsl-bus-client` directly)"
- "REPL extended for distributed verbs (post-demo work)"
- "Sage integration via `VerbDispatcher` (post-demo work)"
- "Workflow-start validation phase (post-T3 work)"

**Remove from non-goals:**
- "Validation as a separate concern from execution" — partially addressed by stub Validate RPC
- "Entity resolution as a federated service" — addressed by stub EntityService
- "SemOS DAG packs as federated content" — addressed by stub SemOsService

### §5 Architecture (new subsections)

**Add §5.11 "Three federated services per domain"** with the content from §1 Principle 1.

**Add §5.12 "Validation as a service"** with the content from §1 Principle 2.

**Add §5.13 "Dispatch transparency"** with the content from §1 Principle 3.

**Add §5.14 "Multi-surface authoring"** with the content from §1 Principle 4.

### §6 Bus protocol specification (additions)

Insert the protocol definitions from §2 of this addendum into §6 of v0.6 as new subsections §6.5, §6.6, §6.7.

### §7 Catalogue manifest specification (additions)

Add `services:` field to manifest top-level structure per §2.4 of this addendum.

### §9 T2A.1, T2A.3, T2B.3, T2B.4, T2B.5, T2B.6, T2B.9 (tranche updates)

For each affected tranche, add the implementation tasks specified in §3 of this addendum.

### §13 Demo Q&A (new prepared answers)

**Add:**

> **Q: How does authoring work? Is BPMN the only way to compose distributed DSL?**
>
> BPMN is one composition surface. The platform also supports distributed REPL — programmatic and interactive composition of cross-domain calls through the same bus, with the same authority, the same audit, the same validation surface. Anyone authoring DSL against the federation uses the REPL primitives; BPMN exists for cases where declarative workflow structure pays off. Sage uses the REPL surface natively. The architecture is multi-surface; BPMN is one application of it.

> **Q: How do you handle entity references and existence checks across domains?**
>
> Federated entity resolution. Each domain that holds entity state exposes an EntityService — a gRPC RPC that resolves natural keys to UUIDs, checks existence, queries FSM state. The REPL calls it during authoring; workflows call it at start-of-execution; Sage calls it during reasoning. Failures surface synchronously with structured detail: "entity X not found", "entity X in state Y, verb requires Z", etc. This is in the v0.6 protocol; full implementation lands in engine V&S v0.4.

> **Q: How does Sage reason about cross-domain composition?**
>
> Federated semantic grounding. Each domain that publishes verbs also publishes its SemOS DAG packs — semantic graphs, constellation maps, derivation chains, FSM applicability descriptors. Sage fetches the relevant DAG packs from each target domain via the SemOsService gRPC. Sage's reasoning is grounded in the same semantic substrate the verb implementations were authored against. There's no second-class "agent only" semantic model — Sage uses what the catalogue and SemOS publish.

> **Q: What's the architectural relationship between local and remote DSL?**
>
> Dispatch transparency. Above the dispatch layer, the REPL and authoring tools don't distinguish between local in-process verbs and remote bus-dispatched verbs. The same composition produces the same outcome regardless of where the verb implementation lives. This means refactoring (moving verbs between domains, co-deploying or distributing services) is a configuration change, not a code change. It also means Sage, REPL, and workflow execution all use the same primitives — they're peer applications of the same federated substrate.

### §12 Risk register (additions)

**Add R11:** "Stubbed RPCs in v0.6 may behave unexpectedly if exercised. Mitigation: stubs return NOT_IMPLEMENTED with explanatory detail; demo flow doesn't exercise them; foundational services see stub behaviour transparently. Production deployment requires full implementation per engine V&S v0.4."

**Add R12:** "Bus protocol locks in shape before implementations exist. Mitigation: stub shapes match anticipated full implementations; protocol designed with extension fields where future evolution likely. If unforeseen requirements surface, additive changes (new fields, new RPCs) are safe; breaking changes require version bump."

---

## 6. Discipline notes for Sonnet implementation

These are the discipline points that protect this architecture from drift:

1. **Stubs return NOT_IMPLEMENTED consistently.** Don't let stubs return success or hardcoded fake data. The point is to surface "this exists but isn't real yet." Hardcoded fake data invites callers to depend on it.

2. **No conditional dispatching based on "is this stubbed?"** Callers (when they exist) check the manifest's `services` declaration to know what's available. They don't probe with calls to see what's stubbed.

3. **Service declarations in manifest are authoritative.** A domain that declares EntityService in its manifest must implement it (even as stub returning NOT_IMPLEMENTED). A domain that doesn't declare it must not register the service in the gRPC server (so gRPC returns UNIMPLEMENTED, not a confused stub).

4. **Don't conflate stub with absence.** EntityService stubbed (declared but returns NOT_IMPLEMENTED) is *different* from EntityService absent (not declared, gRPC returns UNIMPLEMENTED). The first means "we will implement this; for now it's pending"; the second means "this domain doesn't have entities."

5. **Tests prove the protocol shape, not the implementation.** Stub tests verify the wire format works and the right outcomes are returned. They don't test actual entity resolution or DAG pack fetching (because there isn't any yet).

6. **No `if let Some(real_impl) = ...` patterns inside service handlers.** Either the service is implemented (returning real results or NOT_IMPLEMENTED) or it isn't (not in the gRPC server). The handler doesn't conditionally do real work vs stub work.

7. **The `services:` manifest field is part of the contract.** Generators emit it; loaders parse it; consumers (eventually) honour it. For now, capability discovery is "read the manifest; act accordingly." Future consumers (REPL, Sage, workflow validation phase) will all consult this field.

8. **DispatchRouter is not in this addendum's scope.** Don't preemptively build it. v0.6 demo: bpmn-lite executor uses `dsl-bus-client` directly. V&S v0.4 refactoring introduces DispatchRouter. Building it now without the multi-surface consumers to justify it is YAGNI.

---

## 7. Demo behaviour with stubs

The demo flow doesn't change. Specifically:

- bpmn-lite compiles workflows against imported manifests (no change)
- bpmn-lite executor reaches a callout; submits via bus (no change)
- Receiver runs the verb via existing engine (no change)
- Result returned via bus (no change)
- Process advances (no change)

The new RPCs (Validate, EntityService.Resolve, SemOsService.FetchDagPacks) are present on the wire but **the demo flow doesn't call them**. They exist for the architecture story, not for demo execution.

If a foundational services member asks "show me what happens when EntityService.Resolve is called" — that's a "this is stubbed for the demo; here's what the response shape looks like" answer. Show the protobuf; show the NOT_IMPLEMENTED outcome; explain it lands in V&S v0.4.

The demo story is *unchanged* but the architectural story is *complete*.

---

## 8. Timing and sequencing

**Drop point:** after A2 implementation lands and T2B.8 manifest export/import completes. Insert before T2B.9 finishes.

**Why this point:**
- A2 patches need to land first (signal/cargo/transport separation is foundational)
- T2A.1 protocol can be extended once foundational schemas are stable
- T2A.3 manifest extension follows
- T2B.5 and T2B.6 receive their stub implementations
- T2B.9 wiring includes the new services in main() bootstrap

**Sequence:**
1. Sonnet completes A2 implementation patches (currently in progress)
2. STOP gate; Adam reviews diff
3. Sonnet completes T2B.7 (bpmn-lite-bus-handler) per A2
4. Sonnet completes T2B.8 (manifest generation and import for ob-poc and dmn-lite)
5. **STOP gate; drop v0.6-A3.** Instruction: "Read A3. Implement protocol additions (T2A.1), manifest type extensions (T2A.3), stub handlers (T2B.5, T2B.6), and wiring (T2B.9). Validate stub behaviour with the test set per §3.7. Do not commit. Report findings."
6. Sonnet implements A3.
7. STOP gate; Adam reviews diff.
8. T2B.9 wiring completes with all services bootstrapped.
9. T3 begins per v0.6 §9.

This insertion adds approximately one Sonnet session of work between T2B.8 and T2B.9 completion.

---

## 9. Why this is the architecture

The platform's identity is now complete:

| Layer | Mechanism | Property |
|---|---|---|
| **Cargo** | Postgres tables | Durable; transactional with business state |
| **Signal (in-process)** | `tokio::sync::Notify` | Lossy by design; safety-net timer covers |
| **Transport** | TCP/IP via gRPC | TCP assured delivery; HTTP/2 framing; protobuf payloads |
| **Verb execution** | InvocationService (Submit + Validate) | Federated; typed; auditable |
| **Entity resolution** | EntityService (Resolve) | Federated; capability-based; structured outcomes |
| **Semantic grounding** | SemOsService (FetchDagPacks) | Federated; per-domain DAG packs |
| **Catalogue** | YAML manifests | Versioned; explicit; allowlist-published |
| **Dispatch** | DispatchRouter (future) | Transparent; routes per catalogue |

Six federated services per domain (potentially): three RPCs on InvocationService, plus EntityService, SemOsService, ResultService. Capability-based: each domain implements what its content warrants. The bus carries all of them with the same authority, the same retry semantics, the same audit, the same identity model.

Above the dispatch layer: REPL, BPMN, Sage. All peers. All using the same primitives. All composing the same DSL.

This is what "federated DSL platform" actually means. Not "BPMN talks to a backend." A platform where every domain is independently deployable, every inter-domain interaction is typed and federated, every authoring surface is uniform, and every layer earns its place.

For BNY's regulatory context: this is the architecture that holds up under scrutiny. Each layer has a stated purpose. Each layer has bounded failure modes. Each layer is auditable at the wire level. The platform's behaviour is invariant under deployment topology. Sage uses the same surfaces a human would use. The story is complete.

End of v0.6-A3.
