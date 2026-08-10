# EOP-VS-BPMN-CAPABILITY-FABRIC-004 — Typed task and effect capability fabric

- **Version:** v0.3
- **Status:** DRAFT FOR RATIFICATION
- **Date:** 2026-08-07
- **Owner:** Adam
- **Repository:** `/Users/adamtc007/dev/bpmn-lite`
- **Observed baseline:** `codex/bpmn-gameboard-refactor` at `1f4a130`, with an active concurrent refactor in the worktree
- **Companion documents:** `EOP-VS-BPMN-ISA-002`, `EOP-VS-BPMN-DESIGN-003`, `EOP-VS-BPMN-GAMEBOARD-001`, `bpmn-pack-plane-ledger.md`
- **Language companion:** `EOP-VS-BPMN-DSL-005.md`
- **Future companion plan:** `EOP-PLAN-BPMN-CAPABILITY-FABRIC-004.md` — not yet written

## Changelog

**v0.2 → v0.3 — forms are inert human-intervention boundaries.** A form is now
modelled as a governed human work item composed from a semantic interaction contract,
an independently versioned presentation bundle and an environment-bound provider. A
provider URL is delivery configuration, not form identity or workflow authority.
Forms may project data and preserve drafts/submissions, but may not contain business
logic, route the workflow or write systems of record. Submission creates an immutable
`FormSubmissionRef`; separate explicit capability verbs validate and apply it. Human
work must declare why straight-through processing stopped and expose that reason in
operational telemetry. Form admission and lifecycle are explicit fuzz targets.

**v0.1 → v0.2 — DSL source authority and keys-not-cargo ruling.** Every runtime
workflow must be completely hand-authorable in the S-expression DSL and diagnosable in
an IDE. Capability calls, chains, forms, data references, endpoint bindings, lifecycle
and child-workflow calls may not exist only in a graphical or generated representation.
The AST now has an explicit symbolic-reference and staged-resolution model. Workflow
state is restricted to typed identities/references and small control values; business
records, form bodies, documents and provider payloads remain in their systems of
record. Forms fetch by reference and name their submission/storage contracts. v0.3
tightens that boundary so only an explicit downstream capability may apply a
submission through a typed sink. This is a ratified architectural ruling, not an open
implementation choice.

**v0.1 — initial framing.** Defines everything an executable BPMN activity may call
outside the core control graph: typed DSL capability verbs, uniform durable invocation,
provider lifecycle, chained calls, DMN-as-a-call, forms and human work, messaging,
data access, endpoint/resource binding, child-workflow calls, connector plugins,
Camunda 8/Zeebe replacement scope, security, replay and fuzzability.

## 1. Executive decision

BPMN-Lite is to have one **typed capability-call fabric** between its verified core
execution graph and everything that performs, requests, observes or waits for work
outside that graph.

The core BPMN graph remains a small deterministic control machine. It owns:

- token and fibre movement;
- start/end and structured scopes;
- sequence, fork, join, race, guard and bounded repetition;
- durable suspension and resumption;
- typed routing through logic gates;
- cancellation, incidents and replay.

It does **not** know how to open a form, publish to RabbitMQ, call an HTTP endpoint,
store a passport scan, read a customer record, evaluate a DMN table, run a governed
function, or invoke a child workflow. Those are capability verbs supplied through the
same standard interface.

For this architecture, **effect** is deliberately the broad orchestration term:

> Any typed call made by a BPMN activity through the capability boundary, including a
> mathematically pure provider such as a pinned DMN decision.

DMN may be semantically pure and deterministic, but it is not privileged in the BPMN
core. It is another function that accepts typed inputs and returns a typed result used
by a logic gate. The common invocation protocol may optimize an in-process deterministic
provider, but the compiled contract and lifecycle do not gain a DMN-specific escape
hatch.

One activity may execute a bounded, compile-checked chain of capability verbs. Each
verb can have its own durable lifecycle. A call to another published BPMN workflow is
also a capability verb. New capabilities are added through admitted packs and provider
plugins without adding new core BPMN node or opcode semantics.

Every resulting runtime workflow must be expressible, reviewable, compilable and
maintainable by a human in an ordinary IDE using the DSL. The visual Designer, Sage and
AST mutator are authoring accelerators over that source model. They may not create an
executable construct that has no faithful DSL representation.

Workflow instances carry **keys, not cargo**. They may hold typed identities,
content-addressed references, correlation keys and small closed control outcomes. They
do not transport passports, form submissions, customer records, query bodies, asset
prices, arbitrary JSON documents or binary data between nodes.

## 2. Product vision

A workflow designer describes **what must happen** using typed DSL verbs:

- open this form for this subject;
- wait for its submission;
- store the submitted passport scan;
- obtain customer data;
- evaluate this pinned decision;
- publish this envelope to this queue endpoint;
- invoke this pinned workflow and await its declared outcome.

The compiler resolves those words against exact capability-pack pins, verifies every
input and output binding, proves that the chain is finite, and exposes one closed typed
result to the core graph. The graph routes that result through its ordinary logic gate.

The user need not understand workers, connectors, retries, callbacks, subscriptions,
leases or effect journals. The workflow remains honest about them: the Designer can
show what is called, what it waits for, what counts as completion, what may fail, what
external resources must be supplied, and what outcomes the graph must handle.

The resulting product is intended to replace the executable add-on surface that turns
portable BPMN diagrams into Camunda 8/Zeebe-specific applications, while retaining a
smaller and more strongly typed core.

## 3. Scope boundary

### 3.1 In scope

This capability owns the contracts and machinery for:

1. DSL declaration and invocation of capability verbs;
2. exact capability-pack and provider-bundle identity;
3. typed literals, data references, resource references and prior-result bindings;
4. task-local bounded chains of calls;
5. uniform request, progress, completion, rejection, failure and cancellation envelopes;
6. provider-defined lifecycle state under a common lifecycle protocol;
7. durable dispatch, wait, retry, deduplication and response application;
8. DMN and other decision/function providers;
9. forms, assisted human work and task lifecycle;
10. outbound messages, queues, HTTP/gRPC and general connectors;
11. inbound webhook, subscription, polling and message-correlation adapters;
12. typed reads and writes of external data;
13. documents and binary-content references;
14. child-workflow/subroutine invocation;
15. pre/post activity hooks expressed as visible chained verbs;
16. endpoint, environment, secret and credential bindings;
17. provider SDKs and application-facing facade APIs;
18. conformance accounting for the Camunda 8/Zeebe extension and connector surface;
19. canonical receipts, observability, replay and fuzz qualification;
20. complete IDE/DSL authoring, formatting, linting and symbolic-reference diagnostics;
21. typed external-data references and keys-only runtime state.

### 3.2 Explicitly outside this capability

This capability does not redesign:

- the core BPMN graph or its logic-gate semantics;
- SESE structure, fork/join, guard, race, timer or bounded-loop proof;
- graph authoring/gameboard inference except for exposing new typed activity moves;
- BPMN XML parsing itself;
- the vendor-neutral BPMN 2.0 importer;
- the later Zeebe XML extension importer;
- a complete Tasklist, Operate or Modeler user-interface clone;
- arbitrary ungoverned code execution;
- distributed ACID transactions across providers;
- an exactly-once network-delivery claim.

The later importers consume this capability. They map standard BPMN task intent or
Zeebe extension data into these capability contracts; they do not create a second
runtime model.

## 4. Normative separation

```text
Verified BPMN control graph
        |
        | enter activity with typed process state
        v
Compiled bounded CapabilityProgram
        |
        | one universal call protocol per word
        v
Capability fabric
        |
        +--> pinned DMN/function provider
        +--> form/human-task provider
        +--> external worker
        +--> HTTP/gRPC/queue/data/document connector
        +--> inbound subscription manager
        +--> child BPMN workflow
        +--> in-process or sandboxed implementation
        |
        v
Terminal typed activity outcome
        |
        v
Core BPMN logic gate selects the next legal route
```

The core sees no URL, broker client, form renderer, DMN engine, database driver or
child-engine special case. It sees a canonical capability call identity, durable state,
and a typed response.

## 5. Terms

| Term | Meaning |
|---|---|
| `CapabilityVerb` | One pack-declared callable word with a typed contract |
| `CapabilityContract` | Pinned declaration of inputs, outcomes, lifecycle, effect policy and authority |
| `ProviderBundle` | Versioned executable implementation of one or more compatible contracts |
| `CapabilityProgram` | A finite ordered chain of verb calls owned by one BPMN activity |
| `CapabilityCall` | One durable invocation of one word in one programme execution |
| `ResourceRef` | Typed reference to an installed or instance-bound external resource |
| `EndpointValue` | Typed endpoint argument, either registry-backed or a governed URI |
| `EntityRef<T>` | Typed identity of business data held by its system of record |
| `DocumentRef<T>` | Typed object-store reference plus integrity/version evidence, never bytes |
| `DataSinkRef<T>` | Typed destination contract identifying where an explicit data capability writes |
| `InteractionContractRef` | Pinned semantic purpose, allowed actions, schemas, authority and outcomes of human work |
| `PresentationBundleRef` | Pinned replaceable layout, labels, widgets, accessibility and channel presentation with no business authority |
| `FormProviderRef` | Environment binding for a renderer/task service; its URL is not persisted as form identity |
| `FormInstanceRef` | Stable identity of one governed human work item |
| `FormSubmissionRef` | Immutable, integrity-protected evidence captured by a form; not an applied domain mutation |
| `HumanInterventionReason` | Closed reason explaining why straight-through processing stopped or human judgment is mandatory |
| `SymbolRef<T>` | Authoring-time typed name that must resolve or become a declared binding slot |
| `CallReceipt` | Canonical evidence of dispatch, progress and terminal disposition |
| `ActivityOutcome` | Closed terminal value returned by the whole capability programme to the graph |
| `ProviderLifecycle` | Pack-declared provider states and legal transitions beneath one call |
| `Installation` | Environment-specific binding of contracts to providers and static resources |
| `Instantiation` | Per-run binding of instance resources and initial data |

## 6. One universal call contract

Every provider receives the semantic equivalent of:

```text
CapabilityRequest
  call_id
  capability_contract_id + exact content hash
  provider_bundle_hash
  template_artifact_hash
  process_instance_id
  activity_id
  chain_step_id
  attempt
  tenant / environment / authority context
  idempotency_key
  typed input record
  resolved resource bindings
  deadline and cancellation context
```

Every provider answers through the semantic equivalent of:

```text
CapabilityResponse
  Accepted(wait_handle, optional provider_state)
  Progress(provider_state, typed progress data, receipt)
  Completed(outcome_tag, typed outputs, receipt)
  BusinessRejected(outcome_tag, typed outputs, receipt)
  Failed(error_class, retry_advice, receipt)
  Cancelled(receipt)
```

`Accepted` and `Progress` never advance the BPMN token. Only a validated terminal
response can complete a word. Only completion of the final word can return an
`ActivityOutcome` to the graph.

The actual Rust types must use private fields and validating constructors. The shapes
above are semantic contracts, not permission to publish writable structs.

### 6.1 Result taxonomy

The protocol separates:

- **domain outcomes** — `approved`, `not_eligible`, `submitted`, `not_found`;
- **successful technical receipts** — broker accepted, record version written;
- **business rejection** — a governed negative result the workflow may route;
- **transient failure** — eligible for the declared retry policy;
- **contract violation** — provider, binding or response broke the pinned contract;
- **permanent technical failure** — retry cannot make the call valid;
- **cancellation/expiry** — the call ended without its normal domain result.

`done: true` is legal only for a contract whose output type is genuinely `Unit`. A
boolean must never collapse accepted, completed, rejected, failed and cancelled into
one ambiguous value.

## 7. Capability contracts are configuration

Domain and application meaning belongs in admitted YAML packs. Rust provides generic
parsers, validators, compilers, registries, protocols and adapters.

A contract must declare at least:

```yaml
id: queue.publish
schema_version: capability-contract.v1

inputs:
  endpoint: { type: QueueEndpoint, required: true }
  envelope: { type: MessageEnvelope, required: true }
  credential: { type: CredentialRef, required: false, secret: true }

outcomes:
  accepted:
    message: { type: MessageRef, required: true }

effect:
  class: external_write
  completion: broker_acknowledged
  idempotency: required
  must_complete: true
  cancellation: cooperative
  compensation: optional_explicit_verb

lifecycle:
  states: [prepared, dispatched, acknowledged, completed, failed, cancelled]
  terminal: [completed, failed, cancelled]

authority:
  required: messaging.publish
```

The schema must also support:

- input/output cardinality;
- closed enums and named-subset result types;
- structured records and typed references;
- maximum encoded sizes;
- redaction classification;
- allowed endpoint and resource binding phases;
- declared provider lifecycle transitions;
- retry, timeout, cancellation and compensation policies;
- deterministic/pure/provider hints without changing the universal protocol;
- audit and retention policy;
- compatibility requirements for provider bundles.

The compiler resolves a real exact pack pin and verifies the referenced word exists.
Checking only a namespace or domain name is forbidden.

## 8. Typed expression and binding model

The S-expression argument surface must compile into a small typed expression algebra:

- `Literal<T>`;
- `WorkflowInput<T>`;
- `DataRef<T>`;
- `ResourceRef<T>`;
- `ContextRef<T>`;
- `PriorResult<T>`;
- `CollectionElement<T>`;
- an admitted pure mapping expression, if separately specified and bounded.

Arbitrary `(String, String)` argument maps are not a sufficient execution contract.

Every binding is checked against the pinned verb signature. The compiler rejects:

- missing required inputs;
- duplicate inputs;
- unknown inputs;
- wrong types or cardinality;
- read-before-produce;
- a secret used as ordinary data;
- an installation-only resource supplied dynamically where policy forbids it;
- an output with no declared destination when the contract forbids discarding it;
- a prior-result reference that crosses an invalid scope or parallel ownership boundary.

### 8.1 The DSL is the complete source language

Every executable feature in this V&S must have an S-expression representation,
including:

- capability contract references and exact pins;
- ordered capability programmes;
- typed literals and references;
- form definition, view/projection and save-target bindings;
- endpoint and credential slots;
- provider lifecycle and completion policies selected by the contract;
- child-workflow calls;
- declared activity outcomes and their binding to a core logic gate;
- installation- and instance-time unresolved slots.

No `DesignerDag`, DTO, imported XML extension, REST payload or database row may carry
runtime semantics that cannot round-trip through the DSL AST without loss. Generated
DSL is formatted canonically. Reparse, compile and canonicalize must reproduce the same
artifact identity.

The graphical Designer may remain the authoritative interactive representation during
a session, but publication must be able to emit a complete canonical DSL source
receipt. A direct DSL author and a Designer author reach the same compiler and artifact
admission path.

### 8.2 Symbolic references and staged resolution

The authored AST represents names as typed symbols, never as undifferentiated strings:

```text
SymbolRef<FormDefinition>
SymbolRef<CapabilityContract>
SymbolRef<DecisionArtifact>
SymbolRef<DataStore>
SymbolRef<Endpoint>
SymbolRef<WorkflowTemplate>
SymbolRef<EntityType>
```

Each symbol has exactly one of four compile-visible dispositions:

1. **Resolved at authoring/publish** — exact pack/resource/artifact identity and hash;
2. **Declared installation slot** — deliberately supplied by the target environment;
3. **Declared instance slot** — deliberately supplied for each new process instance;
4. **Runtime-produced reference** — produced by one proved upstream word before use.

An ordinary unresolved name is a compiler error. A declared deferred slot is not an
error: it is a typed parameter in the template's installation/instantiation contract.
The distinction must be explicit in the AST and diagnostics; the compiler never guesses
that a misspelt name was intended to be a runtime parameter.

The IDE/LSP surface must provide:

- parse and structural diagnostics with exact source spans;
- unresolved, ambiguous, wrong-kind and stale-pin diagnostics;
- completions from the active admitted pack/resource registries;
- hover display of the complete capability and data contract;
- go-to-definition for DSL, YAML pack, form, decision and child-workflow symbols;
- find-references and safe rename for authoring identities;
- type/cardinality and producer/consumer dataflow diagnostics;
- code actions to declare an installation or instance binding slot;
- visibility of where every key is produced, stored and consumed;
- compile/admission status and canonical artifact identity;
- no dependency on a running Sage/model service for deterministic language tooling.

Tests and `xtask` must drive the same parser/compiler/LSP-facing facade and may not gain
a privileged AST construction path that makes textual DSL less capable than fixtures.

### 8.3 Keys, not cargo

The runtime value algebra is deliberately small. It may contain:

- typed UUID identities;
- `EntityRef<T>` and optional resolved entity version/hash;
- `DocumentRef<T>`/`ObjectRef<T>` containing store identity, object key, version/etag,
  content hash and bounded safe metadata;
- endpoint, credential, form, decision, schema and child-workflow references;
- correlation/idempotency/receipt identities;
- booleans, bounded integers, timestamps/durations and closed enums needed for control;
- bounded arrays/sets of the preceding reference or control values.

It may not contain:

- arbitrary JSON objects as business state;
- complete entity rows or API response bodies;
- form field bodies or query replies;
- passport/image/PDF bytes or base64 encodings;
- complete asset-price records or time series;
- provider-private lifecycle payloads;
- pre-signed URLs, bearer tokens or secret material;
- an unbounded string/array/map used as an escape from the type system.

Small routing facts are allowed only when their contract says they are control truth,
not a cached duplicate of the external business record. For example, a workflow may
carry `AssessmentOutcome::Refer`, but it carries a `PriceObservationRef` rather than the
price payload and asks a pinned decision capability to return the relevant closed
routing outcome.

This rule applies to start commands, job activations/completions, messages, child calls,
forms, journals, snapshots and inspection APIs. A compatibility envelope containing an
opaque `domain_payload` cannot be the target architecture.

### 8.4 Reference integrity and authority

A UUID alone is not a valid data contract. Each reference is bound to:

- semantic kind/schema;
- tenant and owning system/store;
- object/entity identity;
- optional immutable version, etag or content hash;
- provenance and producing call receipt;
- access/purpose policy where required.

Providers dereference only the inputs declared by their pinned contract and only under
the invocation's authority context. A valid UUID of the wrong kind, tenant, version or
purpose is rejected. This prevents the capability fabric becoming a confused-deputy
data-fetch service.

Read consistency is contract data: `latest-authorized`, `version-pinned`, or
`snapshot-pinned`. When a provider observes mutable external state, its terminal receipt
records the exact observed version/hash needed for audit and deterministic replay.

## 9. Endpoint, URL and environment variables

Endpoints are variables. They are not baked into the BPMN graph and they do not define
the semantic identity of a verb.

For example, the stable verb is `queue.publish`; its `endpoint` input determines where
this invocation publishes. The same compiled capability can be installed in development,
test and production against different resources.

`EndpointValue` must distinguish:

```text
RegistryEndpointRef   logical identity resolved by the installation registry
UriEndpoint           governed concrete URI supplied at installation or instantiation
DerivedEndpointRef    typed output of a previous admitted discovery call
```

A raw string that happens to contain `https://` or `amqp://` is not an endpoint.

Endpoint policy must cover:

- permitted schemes and ports;
- tenant/environment ownership;
- egress allowlists and private-network restrictions;
- canonical URI parsing;
- redirect policy;
- DNS and address revalidation policy;
- maximum lengths;
- credential separation;
- redaction in logs and receipts;
- whether runtime-derived endpoints are permitted for that verb;
- whether the endpoint is pinned for the whole instance or resolved per call.

Credentials are independent opaque `CredentialRef` values. User-info credentials in a
URL are rejected. Secrets are resolved inside the provider boundary and never stored in
the template artifact, instance payload, call input receipt or journal.

## 10. Bounded FORTH-like composition

One BPMN activity may contain a `CapabilityProgram`: a compile-time finite sequence of
typed words.

```lisp
(capability-task
  :id collect-and-assess-passport
  :program (
    (forms.show
      :reason missing-information
      :mode repair
      :contract $passport-request-contract
      :presentation $passport-request-web-v4
      :provider $forms-provider
      :subjects (@customer)
      :projection $passport-request-view
      :submission-schema $passport-request-input
      :submission-store $form-evidence-store
      -> @form-instance)
    (forms.await-submission
      :form @form-instance
      -> @submission)
    (customer.validate-passport-response
      :submission @submission
      -> @validated-response)
    (customer.apply-passport-response
      :validated @validated-response
      :target @customer
      -> @passport-ref)
    (dmn.evaluate
      :decision $passport-acceptance
      :inputs @passport-ref
      -> @assessment))
  :outcome @assessment
  :next passport-route)
```

This is FORTH-like in the useful sense: small typed words compose, earlier outputs feed
later words, and a larger capability does not require a new BPMN-core feature.

It is deliberately **not** a hidden second workflow language:

- programme order is finite and explicit;
- no unbounded recursion or loop exists inside a capability programme;
- no provider chooses an undeclared next word;
- business branching returns a typed outcome to a BPMN logic gate;
- parallelism, races, guards and repetition remain visible core-graph structures;
- failure policy is declared, not arbitrary control flow;
- the compiler proves every intermediate value's producer and consumer.

A Designer production may make a common chain appear as one high-level jigsaw piece,
but the compiled receipt exposes the constituent words.

### 10.1 Chain durability

Each word commits independently with a deterministic call ID and idempotency key. On
restart, the runtime resumes from the first non-terminal word; it never repeats a
terminal word merely because the process lost its response.

A chain is not a distributed transaction. If word three fails after words one and two
completed, their effects remain completed. Undo requires declared compensation verbs
and explicit orchestration policy.

### 10.2 Child BPMN workflows

`workflow.invoke` is a capability verb:

- input includes an exact published child-template/artifact reference;
- instance bindings are validated against the child's instantiation contract;
- invocation creates a child instance idempotently;
- the parent call waits for a declared child terminal outcome unless explicitly
  configured for durable handoff;
- cancellation and compensation propagation are declared;
- the returned child result is type-checked like any provider response.

Dynamic `latest` child selection is not stored in the compiled template. If accepted
for compatibility, it resolves at installation or activation into a recorded exact
artifact identity before the child starts.

Recursive child calls must be rejected unless a separate bounded-recursion design is
ratified and verifier-supported.

## 11. DMN and governed functions

DMN is invoked through the same interface:

```lisp
(dmn.evaluate
  :decision $credit-routing-decision
  :inputs @credit-context
  -> @route-outcome)
```

The contract declares the input schema and complete output domain. The provider may be
an in-process `dmn-lite` evaluator, Wasm module or remote decision service. The BPMN
core does not care.

A deterministic local provider may complete within the same engine transition only if
the universal call and receipt invariants remain observable and replay-equivalent. It
must not gain a private direct-write route into process state.

Inline scripting is not a default escape hatch. A Camunda script task imports as one
of:

1. an equivalent admitted typed DSL function;
2. a pinned sandboxed function provider with resource limits;
3. an unresolved capability requiring migration;
4. an explicit unsupported construct.

Arbitrary Groovy, JavaScript or Python execution inside the engine is outside scope.

## 12. Forms and assisted human work

Forms are a necessary human-intervention mechanism, but they are not the domain model
and not a miniature application platform. By default, a form signals that
straight-through processing has stopped because information is missing, data needs
repair or an exception needs resolution. A form can also represent deliberately
mandated human judgment, such as a regulated approval; that is not classified as an
automation defect, but it remains measured human work.

Every form task declares a closed `HumanInterventionReason`, such as
`missing-information`, `data-repair`, `exception-resolution`, `mandated-review` or
`discretionary-approval`. Packs may refine these values without replacing the common
operational categories.

Opening a governed human work item and waiting for its completion are separate
capabilities, commonly composed into one Designer production:

```text
forms.show -> forms.await-submission -> validate submission -> explicitly apply domain command -> activity outcome
```

The capability fabric must support:

- semantic interaction-contract identity and exact version/hash;
- independently pinned presentation/layout bundles;
- environment-bound form/task providers and channels;
- form instance creation;
- subject/case association;
- candidate users/groups and assignment policy;
- claim, release, reassign and delegation;
- due/follow-up dates and expiry;
- lifecycle listeners expressed as visible chained verbs;
- draft/save/submit/cancel outcomes;
- typed submission schema;
- document upload by external reference and content hash;
- authorization and disclosure-safe task queries;
- correlation and idempotent duplicate submission handling;
- complete lifecycle/audit receipts;
- custom UI/form providers as well as a native provider.

The workflow never stores an uploaded passport binary in ordinary instance variables.
It stores a governed `DocumentRef` with integrity and provenance.

### 12.1 Contract, presentation and delivery are separate

A form is assembled from independently governed parts:

| Part | Owns | Must not own |
|---|---|---|
| `InteractionContractRef` | purpose, intervention reason, subject kinds, input/output schemas, allowed actions, human outcomes and authority | screen layout, network address or business-data mutation |
| `PresentationBundleRef` | layout, widgets, copy, locale, accessibility and channel variants | routing, authorization decisions, database operations or domain rules |
| `FormProviderRef` | renderer/task-service compatibility and environment binding | semantic identity or workflow outcome policy |
| `FormProjectionRef` | exact data fields that may be fetched and displayed | undeclared traversal or mutation authority |
| `SubmissionSchemaRef` | typed values and references that may be captured | permission to apply those values to a system of record |
| `SubmissionStoreRef` | durable drafts, immutable submissions and evidence retention | business entities or workflow routing |

A concrete URL belongs to the installation's `FormProviderRef`. A provider may return
a short-lived launch locator to an application facade, but the workflow and journal
store the stable `FormInstanceRef`, never the URL, access token or presigned secret.
This permits the layout, renderer, hostname and delivery channel to change without
changing the semantic form contract or granting new authority.

`forms.show` is the DSL word for creating the governed interaction. It does not mean
that the engine opens a browser. `show`, `input`, `repair`, `review` and `approve` are
typed interaction modes or contract properties, not free-form implementation strings.

### 12.2 Showing data

A form does not receive an arbitrary workflow payload. `forms.show` receives:

- a pinned `InteractionContractRef`;
- a pinned `PresentationBundleRef` compatible with that contract;
- an environment-bound `FormProviderRef`;
- one or more typed subject/entity/document references;
- a pinned `FormProjectionRef` declaring the fields the provider may fetch and show;
- an actor/assignment context;
- optional small display/control values;
- a pinned `SubmissionSchemaRef` and `SubmissionStoreRef` where input is permitted.

The form provider resolves the references through governed data capabilities at render
time. A view projection is configuration and is independently versioned; changing a
screen does not widen the workflow's data authority implicitly.

### 12.3 Capturing data is not applying business data

Drafts and submissions are written only to the governed form/evidence store. Submission
freezes an immutable, integrity-protected `FormSubmissionRef`. The provider returns
only:

- the form-instance and submission references;
- separately stored upload/document references embedded in the submission;
- version/hash evidence;
- a small closed outcome such as `submitted`, `cancelled`, `expired` or
  `validation_rejected`.

The form provider does **not** update a customer, case, account or other system of
record. A subsequent visible capability word accepts the `FormSubmissionRef`, repeats
authoritative validation and returns either a typed rejection or a validated command.
Another explicit word applies that command through a declared `DataSinkRef`, including
schema, create/update semantics, authority, optimistic concurrency and retention. Its
receipt answers “where was this applied?” without relying on UI code.

For document fields, the UI streams bytes directly to the governed document provider
or quarantine/object store. A form submission contains `DocumentRef` values, not file
bytes. An explicit downstream capability admits, associates or rejects those documents
for domain use.

### 12.4 No hidden form application

Presentation bundles may contain non-authoritative usability hints such as required
markers, input masks, format checks, bounds and conditional visibility. The server
repeats structural validation against the pinned submission schema. Neither layer may
make a business decision.

The following are forbidden in form definitions, renderers and lifecycle callbacks:

- direct ORM/repository writes to business tables;
- hidden domain-data fetching beyond the declared projection;
- business validation or scoring treated as authoritative;
- workflow routing, task creation or message publication;
- authorization decisions inferred from hidden fields or presentation state;
- listeners that perform undeclared external effects;
- executable expressions capable of bypassing admitted capability contracts.

Any required validation, decision, write, notification or routing contribution is an
explicit typed DSL capability word with its own receipt. This keeps domain behavior
independently testable and prevents a Spring-controller/ORM unit of work from becoming
an invisible workflow engine.

### 12.5 Human work is observable automation debt

Receipts and operational projections must expose intervention reason, queue, wait,
claim, handling, resubmission, rejection and completion times. The product reports at
least straight-through-processing rate, human-intervention rate by reason, queue age,
touch time, repeat-submission rate and eventual automated-versus-human resolution.
Mandated reviews are reported separately from avoidable repair work.

This telemetry is product evidence, not routing authority. Learning systems may use
governed outcomes to improve utterance/move selection or propose automation, but they
may not silently change a ratified interaction contract.

## 13. Messaging and connectors

### 13.1 Outbound

Outbound calls include:

- HTTP/REST and SOAP;
- gRPC and GraphQL;
- queues and streams such as Kafka, RabbitMQ, SQS and SNS;
- email and collaboration messaging;
- database/data-service operations;
- document/object stores;
- SaaS and application adapters;
- AI/model providers when governed by an admitted contract.

`message.publish`, `queue.publish` and `http.request` are different contracts. A
connector template or domain-specific convenience verb may bind and constrain a generic
protocol capability without changing the runtime.

The completion point is mandatory contract data: outbox committed, transport accepted,
broker acknowledged, remote response received, or correlated business reply.

### 13.2 Inbound

Inbound providers may own long-lived webhook, subscription or polling lifecycles. They
convert external input into the existing typed start/message/signal boundary. They do
not mutate an instance directly.

The provider contract declares:

- subscription identity and lifecycle;
- event schema and source authentication;
- correlation-key derivation;
- deduplication identity and retention;
- buffering/TTL policy;
- start-new-instance versus correlate-existing disposition;
- acknowledgement semantics;
- poison/dead-letter handling;
- backpressure and rate limits.

The durable BPMN wait remains a core graph construct. The connector is the external
event source that satisfies it.

### 13.3 Internal message versus external publication

Publishing into BPMN-Lite's internal correlation buffer is not evidence that an
external broker accepted a message. The DSL, artifact and receipts must distinguish:

- internal instance correlation;
- durable application outbox handoff;
- external transport publication;
- request/reply correlation.

## 14. Provider model and extensibility

A provider bundle implements one stable adapter protocol. Supported execution owners
may include:

- in-process safe Rust;
- sandboxed Wasm with fuel and memory ceilings;
- external claimed workers;
- HTTP/gRPC provider bridges;
- managed connector runtimes;
- a child BPMN engine facade.

Provider registration is separate from semantic contract publication. An application
installation binds a pinned contract to a compatible pinned provider bundle and the
required static resources.

Adding a provider must not require:

- a new `IRNode` variant;
- a new kernel opcode;
- a new public compiler internal;
- a hard-coded task name in the engine;
- application-domain branching in shared crates.

Provider compatibility is proved from contract ID, schema version, input/output hashes,
lifecycle protocol version and declared operational features. Name equality is not
sufficient.

## 15. Installation and instantiation

Capability binding uses the template lifecycle established by the instance-linking
architecture:

```text
Parameterized template artifact
  -> install capability providers + environment resources
  -> create InstalledTemplate
  -> supply instance resources + initial variables
  -> create ExecutableInstanceSpec
  -> start
```

### 15.1 Installation-time bindings

Normally include:

- provider bundle;
- queue/service endpoint or endpoint registry namespace;
- form definition;
- decision artifact;
- child workflow artifact;
- schema registry;
- credential reference;
- egress/security policy.

### 15.2 Instance-time bindings

Normally include:

- subject/case/customer/document UUIDs;
- dynamic destination endpoint where the contract permits it;
- actor/assignee identity;
- form instance or external data identity;
- correlation values;
- initial typed process data.

### 15.3 Runtime-produced bindings

Only outputs explicitly declared by an earlier word may appear later. Examples include
a newly opened form ID, stored document reference, remote request ID or child instance
ID.

No call may discover an undeclared dependency after the instance was admitted.

## 16. Failure, retry, cancellation and compensation

### 16.1 Delivery guarantee

The baseline network claim is **at-least-once dispatch with idempotent completion and
deduplication**, not exactly-once external execution.

Every mutating provider contract declares its idempotency strategy. Retrying one
logical call reuses the same stable idempotency key. A provider that cannot safely
deduplicate must declare the weaker semantics and may be policy-ineligible for
high-risk workflows.

### 16.2 Retry

Retry policy is artifact-resident and bounded. It distinguishes transient failure from
business rejection and contract violation. Provider retry advice may narrow scheduling
within policy but cannot expand the compiled retry budget.

### 16.3 Cancellation

Cancellation has an explicit contract:

- not supported;
- cooperative request;
- confirmed cancellation;
- abandon-wait while effect may continue;
- cascade to child workflow;
- invoke compensation.

The runtime records late completions and applies the declared policy; it never silently
mutates an already-cancelled chain.

### 16.4 Compensation

Compensation is an explicit capability verb with typed input from the original call
receipt. Automatic distributed rollback is forbidden. The core graph decides when and
in what scope compensation runs; the provider implements the compensating effect.

## 17. Camunda 8/Zeebe replacement boundary

The target is **semantic replacement**, not XML spelling compatibility inside the core.
The later Zeebe importer translates extension declarations into this model.

The conformance baseline is Camunda 8.9 documentation as observed on 2026-08-07. A
machine-readable, versioned conformance ledger must ultimately enumerate every reviewed
feature and built-in connector with `equivalent`, `stronger`, `migration-required`,
`unsupported`, or `not-applicable` status.

| Camunda/Zeebe surface | Capability-fabric replacement |
|---|---|
| Service task job type | Pinned `CapabilityVerb` + provider binding |
| Job worker activation/completion | Claimed durable capability-call protocol |
| Task headers | Typed static or resource-bound inputs |
| Input/output variable mappings | Compile-checked binding expressions and declared mutation set |
| Job retries | Artifact-resident bounded retry policy |
| Outbound Connectors | Protocol/provider capability plugins |
| Inbound webhook/subscription/polling Connectors | Managed inbound providers feeding typed start/message boundaries |
| Connector templates | Pack-declared constrained verbs over generic providers |
| Connector secrets | Opaque `CredentialRef`, resolved only within provider boundary |
| User tasks | Human-task lifecycle capability |
| Camunda Forms/custom forms | Semantic interaction contract + independently pinned presentation + provider binding |
| Assignment/scheduling | Typed lifecycle inputs and provider states |
| User-task listeners | Explicit lifecycle-triggered chained capability verbs |
| Business-rule task/DMN | `dmn.evaluate` through the universal protocol |
| Script task/FEEL | Admitted typed function or sandboxed function provider |
| Send task/message throw | External publish capability; distinct from internal correlation |
| Receive task/message catch | Core durable wait + inbound provider/correlation boundary |
| Call activity | `workflow.invoke` with exact child artifact and typed contract |
| Resource binding `latest/deployment/versionTag` | Exact content pin after installation/activation resolution |
| Execution listeners | Visible ordered pre/post capability words |
| Errors/incidents | Typed domain outcomes and failure taxonomy mapped to core incidents/routes |
| Compensation handlers | Explicit compensation verbs orchestrated by core scopes |
| Variables/local mappings | Typed data slots, scope and canonical mutation sets |
| JSON process-variable cargo | Typed keys/references plus small closed control values; cargo rejected |
| Camunda document variable/reference | Typed `DocumentRef`/`ObjectRef`, bytes held by configured object store |
| Form variable field binding | Pinned projection + typed source refs + immutable submission; explicit downstream validate/apply verbs |
| Custom connectors | Provider SDK + admitted capability pack |
| Tasklist task APIs | Application facade over human-task provider state |
| Operate visibility | Canonical call/chain receipts and instance inspection; UI clone out of scope |

### 17.1 Catalogue parity versus framework parity

Two gates must not be conflated:

1. **Framework parity:** every Camunda connector/task behavior can be represented without
   changing the core graph or kernel.
2. **Catalogue parity:** every connector selected for the declared Camunda replacement
   release has a qualified provider implementation and migration mapping.

The final Camunda 8 replacement claim requires both for the published conformance
baseline. Framework parity alone is not marketed as complete replacement.

The provider catalogue can be delivered in tranches. At minimum, the first tranche
should prove REST/HTTP, Kafka or RabbitMQ, SQL/data access, document storage, forms,
DMN, inbound webhook/subscription and child workflow. The ledger—not prose—records the
remaining gap.

## 18. DSL and Designer surface

The DSL must expose semantic words, not vendor adapter mechanics:

- `forms.show`, not `zeebe:userTask` or a renderer URL;
- `queue.publish`, not a RabbitMQ XML extension;
- `dmn.evaluate`, not `zeebe:calledDecision`;
- `workflow.invoke`, not a Zeebe call-activity binding;
- `data.fetch`/`data.store`, not a hard-coded ORM operation;
- `http.request`, not a Camunda connector template payload.

Domain packs may define narrower words such as `kyc.request-passport` by composing or
constraining generic capability contracts. Those words remain YAML configuration and
exact pins; shared Rust crates do not acquire KYC, CBU or application-specific enums.

All examples, generated workflows and imported models must have a faithful canonical
DSL rendering. “Only constructible through the Designer” and “only representable in
imported XML” are release-blocking coverage failures, not acceptable alternate planes.

The Designer/gameboard must add legal moves for:

- selecting a capability verb;
- constructing or editing a capability programme;
- binding typed inputs and outputs;
- selecting completion semantics;
- adding a form/human lifecycle;
- binding a decision or child workflow;
- declaring endpoint/resource slots;
- exposing unresolved installation/instance requirements;
- previewing the complete chain and its external consequences.

Wrong bindings and unsupported providers are normal game outcomes with pack-grounded
feedback and legal alternatives, not raw Rust errors.

Form authoring must show the interaction contract, presentation bundle, provider slot,
projection, submission schema/store and intervention reason as distinct typed bindings.
The Designer may offer them as one ergonomic production, but canonical DSL must retain
the separation. It must visibly warn when a form introduces avoidable human repair and
must never generate an implicit business-data write from a writable field.

## 19. Rust capability and visibility boundary

The capability is implemented behind one deliberate application-facing facade.

Normative rules:

1. implementation modules default to private or `pub(crate)`;
2. fields of persistent/validated contracts are private;
3. only stable identity, request, outcome, receipt and facade types required by an
   application may be `pub`;
4. compiler IR, lowering, provider registries, lifecycle transition internals and
   storage rows are not public application APIs;
5. an application composes facades; it does not reach through crate layers;
6. provider SDK exposure is separate and narrower than application control exposure;
7. tests, examples, benches, fuzz targets, generators and `xtask` use the same facade
   or a crate-private test module;
8. no test-support need may widen production visibility;
9. new capability providers cannot create reverse dependencies into applications;
10. shared crates contain no application-specific code paths.

## 20. Determinism, replay and receipts

Each call and chain has canonical identity derived from recorded inputs, not wall-clock
arrival order. Receipts include:

- exact contract and provider hashes;
- instance/activity/step/call identity;
- canonical input and resource-binding hashes;
- idempotency key;
- attempt and lifecycle transitions;
- output/outcome hash;
- authority and policy decision identity;
- redacted provider receipt/provenance;
- timestamps as observed data, never as replay authority.

Replay never calls an external provider. It reapplies the recorded canonical terminal
response and verifies the same state transition and final hash.

Provider `Progress` is audit state. Unless a contract explicitly exposes it as process
data, it does not change the BPMN graph state.

## 21. Security and authority

Every invocation is authorized at two boundaries:

1. installation/instantiation proves the template may use the capability and resources;
2. dispatch proves the current tenant, actor and instance still hold the required
   authority.

Neither check replaces the other.

Standing controls include:

- no secrets in artifacts, process variables, prompts, logs or receipts;
- tenant isolation on providers, endpoints, resources and callbacks;
- egress policy for dynamic URLs;
- input/output size and depth limits;
- provider timeout, concurrency and rate budgets;
- Wasm fuel/memory ceilings where applicable;
- response-schema validation before any state mutation;
- output write allowlists rather than whole-payload replacement;
- signed/authenticated inbound events;
- replay/deduplication protection;
- disclosure-safe errors and operator diagnostics;
- auditable capability and policy identities.

## 22. Fuzzability is a design law

The capability fabric must expose a pure deterministic reference model for:

- chain position;
- call lifecycle;
- retries and exhaustion;
- cancellation and late completion;
- output binding;
- terminal outcome production;
- child-call ownership;
- inbound deduplication/correlation.

Time, identity, provider responses and registry revisions are controllable inputs.

Generated operation tapes must cover at least:

- missing, duplicate and wrongly typed inputs;
- hostile endpoint URIs and forbidden egress;
- missing/revoked providers and resources;
- stale contract/provider hashes;
- lost dispatch and completion responses;
- duplicate, reordered and late progress/completion events;
- transient retry followed by success or exhaustion;
- cancellation at every lifecycle state;
- provider output with wrong tag, field, type, size or mutation target;
- chain failure after earlier irreversible effects;
- idempotent restart at every word boundary;
- child workflow completion/cancellation races;
- form double-submit/reassignment/expiry races;
- hidden/extra-field injection, stale layouts, mismatched contract/presentation hashes
  and unauthorized projection expansion;
- wrong submission types, duplicate/stale application, optimistic-version conflicts
  and attempts to treat client-side validation as authority;
- forged upload references, cross-tenant references, oversized uploads and submission
  mutation after sealing;
- proof that rendering, draft and submission operations cannot mutate business data or
  route the core graph;
- inbound duplicate and correlation collisions;
- secret-redaction invariants;
- native/Wasm/provider differential execution where equivalent.

Permanent minimized regressions are committed and replayed in CI. A target with no
receipt or a regression gate with no cases fails closed once the first finding exists.

## 23. Performance and operational expectations

The fabric must support:

- O(1) lookup of installed contract/provider identity;
- no network call on replay;
- bounded canonical request/response sizes;
- streaming or referenced binary data rather than payload inflation;
- batched durable effect claiming;
- horizontally scalable providers;
- backpressure and per-provider concurrency limits;
- independent health/readiness for required provider bundles;
- startup refusal when an installed artifact references an unavailable mandatory
  provider;
- no global lock across unrelated process instances;
- bounded chain length and intermediate state.

Concrete latency, throughput, size, chain-length and lifecycle-state budgets belong in
the implementation plan and must be measured before release.

## 24. Current-source findings that motivate the capability

At the observed baseline, the repository contains valuable substrate but not the
complete contract described here:

1. `service-task`, `business-rule-task` and generic `task` parse into one `TaskAst`,
   erasing the semantic category before compilation.
2. DSL task arguments are `(String, String)` pairs and become string maps.
3. `BindingDecl` retains placeholder names and an effect-class string rather than the
   manifest's complete typed signature and outcome algebra.
4. the DSL frontend embeds `static_args` in `ExecDslTask`, but the kernel ignores that
   field while creating the job activation;
5. delivery mode is derived in the plan, while the DSL execution path always enqueues a
   job and parks;
6. job completion replaces a broad domain payload and flag map rather than applying a
   capability-declared typed mutation set;
7. `FfiServiceTask`/`V2AwaitEffect` already demonstrate durable typed invocation, but
   remain a parallel path rather than the universal DSL capability call;
8. `HumanWait` waits but does not create or govern a form/task lifecycle;
9. internal message publication and external queue publication are not one semantic
   guarantee;
10. the Designer semantic pack is substantially stronger on topology than on creating
    and binding executable capability programmes;
11. broad `domain_payload` JSON remains present in instance/job/completion paths and is
    incompatible with the keys-not-cargo target;
12. graph/IR/DTO constructs exist that the S-expression AST cannot faithfully author,
    creating a direct-IDE coverage gap.
13. no current form contract enforces the required separation between semantic
    interaction, presentation, delivery endpoint, immutable submission evidence and
    explicit domain application.

These findings must be re-audited after the concurrent gameboard refactor completes.
They are evidence for the V&S, not permission to patch the active worktree piecemeal.

## 25. Acceptance definition

This capability is complete only when all of the following are true:

1. one new admitted capability verb can be added without modifying BPMN core graph,
   kernel dispatch matching or public compiler internals;
2. DMN, form, outbound queue, inbound message, data/document and child-workflow calls
   all use the same canonical invocation lifecycle;
3. a BPMN activity can run a finite typed chain and restart safely at every word;
4. every input, intermediate output and final outcome is compile-checked;
5. endpoint URLs/resource identities are typed variables with installation,
   instantiation or runtime-produced provenance;
6. secrets never enter the workflow artifact or instance data;
7. terminal responses cannot mutate undeclared state;
8. the graph receives one closed typed activity outcome and routes it through ordinary
   logic-gate semantics;
9. retries, cancellations, late completions and lost responses have model-tested,
   durable behavior;
10. child workflows are pinned, typed and idempotently invoked;
11. provider-specific lifecycle is visible and auditable without leaking into core
    graph semantics;
12. the application and provider facades are narrow and production visibility was not
    widened for tests or tools;
13. property, fuzz, hostile-admission, recovery and differential suites pass with
    permanent regression governance;
14. the Camunda 8.9 conformance ledger has no unreviewed surface;
15. every claimed replacement connector has a qualified provider and migration fixture;
16. vanilla BPMN import and later Zeebe extension import both lower to this one model;
17. documentation distinguishes framework parity from delivered catalogue parity;
18. performance and resource-budget receipts satisfy the ratified implementation plan;
19. every runtime artifact can be emitted as canonical DSL and rebuilt to the identical
    admitted artifact identity;
20. the LSP/compiler distinguishes a misspelt unresolved symbol from a deliberately
    deferred installation/instance slot;
21. no production boundary carries arbitrary business JSON or binary cargo;
22. forms fetch only through typed projections and return immutable submission/document
    references plus small outcomes; they never apply business-data writes;
23. data/document references prove kind, tenant, store, version/integrity and
    provenance as required by their contract;
24. every human task declares a typed intervention reason and operational reporting
    separates mandated judgment from avoidable STP failure;
25. semantic interaction contract, presentation bundle and provider/URL binding are
    independently versioned, compatible and auditable;
26. form layouts, renderers, drafts, submissions and callbacks contain no business
    logic, routing or undeclared effects, and explicit downstream verbs own validation
    and domain application;
27. property/fuzz tests prove hostile form input cannot widen projection, forge
    authority, mutate a sealed submission or write business state through the form
    boundary.

## 26. Open owner decisions before the implementation plan

### Q1 — User-facing name

Options include `capability task`, `effect task`, or `capability programme`.

**Recommendation:** use **capability task** in the Designer and
`CapabilityProgram`/`CapabilityCall` in contracts. Retain “effect” for runtime delivery
and journalling. This accommodates DMN-as-universal-call without telling users that a
pure decision mutates the world.

### Q2 — Raw URI policy

Should production templates accept a concrete `UriEndpoint` instance input, or require
all production endpoints to resolve through a registry identity?

**Recommendation:** support both in the type system, default production policy to
registry references, and permit governed raw URIs only for explicitly eligible verbs
and tenants. This preserves the stated URL-variable requirement without making SSRF an
architectural default.

### Q3 — Chain branching

May a `CapabilityProgram` conditionally skip or select later words?

**Recommendation:** no business branching in v1. A chain is linear; failures use the
standard lifecycle and business outcomes return to the core graph. Use a child workflow
when internal orchestration is genuinely required. This preserves the boundary rather
than growing a second hidden BPMN.

### Q4 — Form service ownership

Should BPMN-Lite ship a native forms/human-task provider or define only the protocol?

**Recommendation:** ship a small native reference provider because Camunda replacement
cannot be proved with a protocol alone; keep rendering/UI replaceable through the same
provider facade.

### Q5 — Sandboxed functions

Is Wasm the only permitted user-code provider for imported script tasks?

**Recommendation:** yes for in-platform uploaded code. External workers may use any
implementation language behind the protocol, but arbitrary scripts do not execute in
the engine process.

### Q6 — Catalogue release target

Which Camunda built-in connector catalogue/version defines the first complete
replacement claim?

**Recommendation:** pin Camunda 8.9 as the audit baseline now, create the exhaustive
ledger, and define a smaller named first production tranche. Do not use “Camunda 8
replacement complete” until the ledger for the selected baseline closes.

### Q7 — Provider lifecycle exposure

May provider progress state be queried by ordinary workflow DSL, or only by operations
and UI surfaces?

**Recommendation:** operations/UI by default. A contract may explicitly project a
typed progress value into workflow state, but progress must not silently create routing
authority.

### Q8 — Completion atomicity for chained words

Should one transition apply the terminal response and immediately dispatch the next
word, or persist a transition boundary between them?

**Recommendation:** preserve a durable transition boundary per word in v1. It gives
clean replay, cancellation, receipts and fuzz schedules. Optimize later only with a
proved canonical-equivalence rule.

## 27. Required next documents

After ratification and after the concurrent refactor reaches a stable commit:

1. ratify `EOP-VS-BPMN-DSL-005.md` as the source-language and grammar boundary;
2. re-run the current-source drift audit;
3. write `EOP-PLAN-BPMN-CAPABILITY-FABRIC-004.md` with full-module replacement
   boundaries, red/green receipts and staged migration;
4. create the machine-readable Camunda 8.9 task/extension/connector conformance ledger;
5. write the vendor-neutral BPMN 2.0 XML importer V&S/plan;
6. write the separate Zeebe-extension importer V&S/plan against the conformance ledger;
7. define the template installation/instance-linking contract as a companion normative
   artifact if it has not already been ratified elsewhere;
8. write a provider SDK threat model and compatibility specification;
9. write fuzz target/oracle specifications before implementation begins;
10. write the canonical DSL/LSP coverage and round-trip specification;
11. write the typed reference/value algebra and data-authority specification;
12. add a migration plan for retiring broad `domain_payload` cargo from production
    boundaries.

## 28. Official Camunda baseline references

The replacement inventory in this v0.3 was checked against the following official
Camunda documentation on 2026-08-07:

- [BPMN task overview](https://docs.camunda.io/docs/components/modeler/bpmn/tasks/)
- [Service tasks and job-worker configuration](https://docs.camunda.io/docs/components/modeler/bpmn/service-tasks/)
- [Job workers](https://docs.camunda.io/docs/components/concepts/job-workers/)
- [User tasks, assignments, forms and listeners](https://docs.camunda.io/docs/components/modeler/bpmn/user-tasks/)
- [Camunda Forms](https://docs.camunda.io/docs/components/modeler/forms/utilizing-forms/)
- [Camunda form data binding](https://docs.camunda.io/docs/8.7/components/modeler/forms/configuration/forms-config-data-binding/)
- [Business rule tasks and called DMN decisions](https://docs.camunda.io/docs/components/modeler/bpmn/business-rule-tasks/)
- [Script tasks](https://docs.camunda.io/docs/components/modeler/bpmn/script-tasks/)
- [Call activities](https://docs.camunda.io/docs/components/modeler/bpmn/call-activities/)
- [Receive tasks](https://docs.camunda.io/docs/next/components/modeler/bpmn/receive-tasks/)
- [Send tasks](https://docs.camunda.io/docs/next/components/modeler/bpmn/send-tasks/)
- [Connector types](https://docs.camunda.io/docs/components/connectors/connector-types/)
- [Built-in connector catalogue](https://docs.camunda.io/docs/components/connectors/out-of-the-box-connectors/available-connectors-overview/)
- [Connector secrets](https://docs.camunda.io/docs/components/console/manage-clusters/manage-secrets/)
- [Execution listeners](https://docs.camunda.io/docs/components/concepts/execution-listeners/)
- [Resource binding types](https://docs.camunda.io/docs/components/best-practices/modeling/choosing-the-resource-binding-type/)
- [Process variables and mappings](https://docs.camunda.io/docs/components/concepts/variables/)
- [Camunda data-flow model](https://docs.camunda.io/docs/components/modeler/bpmn/data-flow/)
- [Camunda data-handling guidance](https://docs.camunda.io/docs/8.8/components/best-practices/development/handling-data-in-processes/)
- [Camunda document handling](https://docs.camunda.io/docs/components/document-handling/getting-started/)
- [Document upload and returned references](https://docs.camunda.io/docs/components/document-handling/upload-document-to-bpmn-process/)
- [Form Filepicker document references](https://docs.camunda.io/docs/components/modeler/forms/form-element-library/forms-element-library-filepicker/)
- [Compensation events](https://docs.camunda.io/docs/components/modeler/bpmn/compensation-events/)

The ledger required by §27 must pin the exact reviewed documentation/product version;
these live URLs alone are not a reproducible conformance artifact.
