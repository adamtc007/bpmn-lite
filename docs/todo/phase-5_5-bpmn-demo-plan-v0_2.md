# Phase 5.5 Plan v0.2: bpmn-lite Demo Deployment

| Field | Value |
| --- | --- |
| Document ID | OB-POC-PHASE-5_5-PLAN-002 |
| Version | v0.2 |
| Status | DRAFT — corrected for bpmn-dsl-as-workflow-definition architecture; two-DAG model; placeholder inference; callout/pause/subscribe/resume |
| Author | Adam Cearns |
| Date | 2026-05-20 |
| Supersedes | v0.1 (assumed two-pass AST→DAG; assumed bpmn-lite as polling orchestrator; demo model included KYC/UBO; explicit binding flow) |
| Position | Post-Phase 5 (`b7c5e5f1`); demo-driven; not Phase 6 proper |
| Repo | bpmn-lite (separate from ob-poc); shared DSL engine consumed as workspace dependency |
| Deployment | Existing Docker; bpmn-lite + shared engine + ob-poc UI repointed |
| Instruction to Sonnet | Replan from T0. v0.1 work is superseded. The architecture commitments in §0 are load-bearing — do not deviate. |

---

## 0. Architectural commitments

These three commitments are load-bearing. Every tranche serves them.

### Commitment A: bpmn-dsl source IS the workflow graph

The workflow definition language is bpmn-dsl: s-expression form, same language family as ob-poc DSL. BPMN XML and ob-poc DSL text are two parseable surfaces of the same language family. There is no separate "BPMN AST" that gets lowered to a DAG. The source is already structurally a graph.

The compilation pipeline:
1. **Parse** bpmn-dsl source — NOM generates the AST against the graph-structured source
2. **Linter** does semantic resolution: unresolved reference checks, `@cbu` placeholder allocation and threading, type consistency checks against catalogue declarations
3. **DAG pass** validates runtime execution order; produces the BPMN-shaped Populated Execution DAG embedded in an ExecutablePlan
4. **Executor** runs the DAG in order

This is single-pass compilation through three semantic phases (parse / lint / DAG), not multi-pass lowering between distinct representations.

### Commitment B: Two DAGs at two scopes

Runtime hosts two distinct DAGs operating at different scopes:

| | BPMN workflow DAG | Inner DSL plan DAG |
|---|---|---|
| Scope | Process-instance | One callout |
| Duration | Long-lived (days, weeks, months) | Short-lived (one ExecutionFrame) |
| Persistence | Durable in Postgres between callouts | In-memory inside ExecutionFrame |
| Advances on | Lifecycle event arrival (callback) | Internal verb completion |
| Outcome class | `BpmnInstanceCompleted` / `BpmnInstanceWaiting` / etc. | Phase 5 outcomes (Committed / OptimisticConflict / etc.) |
| Owns | bpmn-lite | Phase 5 engine (`b7c5e5f1`) |

The BPMN workflow DAG is what foundational services see "running." The inner DSL plan DAGs are short-running plans submitted to the Phase 5 engine for each callout.

### Commitment C: Callout / Pause / Subscribe / Resume

The BPMN executor is not a thread that polls or blocks. It's a state machine. At each service task or business rule task callout:

1. Resolve catalogue entry → produce inner ExecutablePlan
2. Submit plan to Phase 5 engine
3. Mark process instance as "waiting on callout for node X"
4. Persist BPMN process state to Postgres
5. Free the executor — no thread parked

When the lifecycle event arrives (DSL emits `VerbCompleted` with resolved binding values):

6. Subscriber receives event → identifies waiting process instance
7. Load instance state from Postgres
8. Bind resolved values into placeholder scope (`@cbu` now concrete)
9. Advance to next BPMN node (or another callout, or end event)

The DSL engine doesn't know bpmn-lite exists. It just emits lifecycle events on commit. bpmn-lite is one subscriber among N (Sage, UI, audit, any other reactor that wants the events).

### Placeholder inference (`@cbu` mechanism)

Workflow surface stays clean of binding ceremony. The compiler infers binding flow from catalogue declarations:

- Service task with `id cbu.create` → catalogue says it produces a CBU → compiler allocates `@cbu` placeholder slot
- Service task with `id cbu.add_fund_product` → catalogue says it consumes a CBU → compiler threads `@cbu` from the producer
- Gateway predicate references `@cbu-type` → compiler resolves it to the slot most recently produced of matching type

For the demo, **single-binding-per-type inference** is sufficient: `@cbu` resolves to "the most recently produced CBU in scope." If multi-binding-per-type is ever needed (parent + subsidiary CBU), explicit naming (`@parent-cbu`, `@subsidiary-cbu`) extends the mechanism — but that's out of scope for the demo.

---

## 1. Pre-locked decisions

1. **Workflow definition language** = s-expression bpmn-dsl. Same compiler as ob-poc DSL.
2. **Condition language** = s-expression predicate subset. Same machinery; same compiler.
3. **Binding convention** = service task `id` matches verb id in SemOS catalogue (exact match). `name` is human label only. Unresolved `id` = compile error.
4. **Placeholder mechanism** = single-binding-per-type inference (`@cbu` style). Workflow surface stays declaration-free; compiler infers binding flow from catalogue.
5. **Callback mechanism** = Phase 5 engine is synchronous; bpmn-lite owns the event loop. On plan completion, bpmn-lite emits a lifecycle event internally and dispatches to the BPMN instance reactor. The async callback mechanism (how bpmn-dsl land receives and routes events) is a T2 design-and-implement task.
6. **Demo BPMN model** = CBU lifecycle only (no KYC, no UBO). Pre-coded in Rust. XML loader deferred to Stage 2.
7. **Gateways in demo** = sequential + one exclusive gateway routed by DMN. No parallel / inclusive.
8. **Product verb** = single `cbu.add-product` verb with product-type arg. No type-specific product verbs. Three gateway branches each call `cbu.add-product` with a different `:product` arg.
9. **Instrument matrix verb** = `instrument-matrix.attach` (existing verb). Do not create `cbu.add-instrument-matrix` or any duplicate.
10. **No Phase 6 architectural work** in this plan.
11. **STOP gates** between every tranche. Sonnet reports; Adam reviews diff; Adam approves commit.

---

## 2. Non-goals (explicit)

To prevent Sonnet drift:

- BPMN XML parser/loader (Stage 2 — not this plan)
- Parallel or inclusive gateways
- Boundary events, event sub-processes, multi-instance markers, compensation
- Timer scheduler, message correlator
- Multi-worker pool, lanes, admission controller
- Plan persistence, cross-snapshot replay, audit retention
- KYC, UBO, screening, completeness — explicitly removed from demo model
- Multi-binding-per-type placeholder syntax (single-binding-per-type only)
- Phase 6 coordination primitives (fan-out, WaitN, cancellation scopes)
- Verb catalogue extensions beyond what the demo BPMN model invokes
- Any rework of Phase 5 engine internals — engine is closed at `b7c5e5f1`

If Sonnet's plan touches any of the above, STOP and report — do not implement.

---

## 3. Demo BPMN model (locked)

**CBU lifecycle — sequential with one exclusive gateway routed by DMN:**

```
[Start]
   ↓
[Service: cbu.create]                       ← produces @cbu
   ↓
[Business Rule: cbu_type_routing]           ← consumes @cbu; produces @cbu-type
   ↓
[Gateway: type-gateway]
   ├─ (= @cbu-type "fund")      → [Service: cbu.add-product :product CUSTODY_FUND]
   ├─ (= @cbu-type "corporate") → [Service: cbu.add-product :product CUSTODY_CORP]
   └─ (= @cbu-type "trust")     → [Service: cbu.add-product :product CUSTODY_TRUST]
   ↓                              (all three converge)
[Service: instrument-matrix.attach]         ← consumes @cbu
   ↓
[End: CBU Operational]
```

**As bpmn-dsl s-expression source (the workflow definition):**

```scheme
(workflow custody-cbu-onboarding
  (start-event :id start :next create-cbu)
  (service-task :id create-cbu :verb cbu.create :next type-decision)
  (business-rule-task :id type-decision :decision cbu_type_routing :next type-gateway)
  (exclusive-gateway :id type-gateway
    (flow :condition (= @cbu-type "fund")      :next add-fund)
    (flow :condition (= @cbu-type "corporate") :next add-corp)
    (flow :condition (= @cbu-type "trust")     :next add-trust))
  (service-task :id add-fund  :verb cbu.add-product :args (:product "CUSTODY_FUND")  :next add-im)
  (service-task :id add-corp  :verb cbu.add-product :args (:product "CUSTODY_CORP")  :next add-im)
  (service-task :id add-trust :verb cbu.add-product :args (:product "CUSTODY_TRUST") :next add-im)
  (service-task :id add-im    :verb instrument-matrix.attach :next end)
  (end-event :id end :status "Operational"))
```

Note: workflow surface declares **structure only** — `:next` for topology, `:verb` for catalogue lookup, `:decision` for DMN lookup, `:condition` for gateway predicates, `:args` for static arg overrides. **No `:produces` or `:consumes` declarations.** Placeholder flow is inferred by the compiler from catalogue declarations against verb/decision ids.

**Verb catalogue mapping (T0 reconciled, 2026-05-20):**

| Node | bpmn-dsl verb | Catalogue FQN | Status |
|------|---------------|---------------|--------|
| create-cbu | `cbu.create` | `cbu.create` | ✅ existing |
| add-fund/corp/trust | `cbu.add-product` | `cbu.add-product` | ✅ existing, product-type via `:args` |
| add-im | `instrument-matrix.attach` | `instrument-matrix.attach` | ✅ existing (added 2026-05-20) |

5 service tasks (3 share one verb), 1 business rule task, 1 exclusive gateway, 3 routing paths, 1 end state, 1 implicit `@cbu` placeholder, 1 implicit `@cbu-type` placeholder.

`cbu_type_routing` DMN decision does not yet exist — T1 seeds it as a fixture.

---

## 4. Tranches

### T0 — Pre-flight audit (NO CODE)

**Goal:** establish ground truth before any execution.

**Sonnet tasks:**

1. **Inventory bpmn-lite repo structure:** directories, crates, Cargo.toml dependencies, workspace layout
2. **Inventory existing bpmn-dsl parser:**
   - Does NOM-based parsing of bpmn-dsl s-expressions exist?
   - What AST does it produce?
   - Test pack contents: what fixtures cover what cases?
   - Is the parser already producing input the ob-poc compiler can consume, or is there a gap?
3. **Inventory shared DSL engine integration:**
   - Is bpmn-lite declaring ob-poc as a workspace dependency?
   - Can the bpmn-lite codebase invoke the ob-poc compiler entry points?
   - Can it submit plans to the Phase 5 engine?
4. **Inventory linter / semantic resolution layer:**
   - Does any linter pass currently handle `@cbu`-style placeholder allocation?
   - If not, what's the current binding-flow handling? Explicit declarations? Position-based? Nothing?
   - Phase 5 T10 (`22d6821e`) landed typed binding slots + BindingFrameSchema. Does the bpmn-dsl compilation path reach this mechanism, or does it bypass it?
5. **Inventory DAG pass / execution order validation:**
   - Does compilation today produce a Populated Execution DAG that passes the Phase 5 validator?
   - If not, what's the current output and what's the gap?
6. **Inventory BPMN runtime execution model in bpmn-lite:**
   - Is the executor structured as pause/subscribe/resume (state machine), or as polling/orchestrating (active loop)?
   - Is there a process_instance persistence layer in Postgres?
   - Does it persist state between callouts?
7. **Inventory lifecycle event emission from Phase 5 engine:**
   - Does the engine emit typed lifecycle events on plan commit (VerbCompleted, EntityCreated, FSMTransitioned)?
   - Or are commits only recorded in `dsl_execution_audit` without event dispatch?
   - If audit-only: is there a publisher path from audit records to subscribers, or does this need to be built?
8. **Inventory subscriber/reactor mechanism in bpmn-lite:**
   - Is there a subscriber that reads lifecycle events from the engine and dispatches to BPMN process instances?
   - If not, what would it look like to add one?
9. **Inventory catalogue for demo verbs:**
   - `cbu.create` — present? Effect class? Produces CBU binding?
   - `cbu.add_fund_product` — present? Effect class? Consumes CBU?
   - `cbu.add_corporate_product` — same questions
   - `cbu.add_trust_product` — same questions
   - `cbu.add_instrument_matrix` — same questions
   - For each missing or signature-mismatched: report what needs adding
10. **Inventory DMN decisions:**
    - `cbu_type_routing` — present in dmn-lite? Input/output schema appropriate?
    - If missing: what's needed to add it (seed SQL, dmn-lite catalogue entry)?
11. **Inventory Docker deployment:**
    - Services running, networking topology, how bpmn-lite would slot in
    - How ob-poc UI currently connects; what would change to repoint
12. **Inventory ob-poc UI:**
    - Current state; API endpoints it consumes
    - What components could display BPMN process state, plan submissions, Sage reasoning, DMN results

**DoD:** structured findings report. Each of the 12 inventory items has a clear answer. Gaps identified. No code changes. Adam reviews; identifies which gaps fold into which tranches.

**STOP gate.** Do not commit anything. Report findings and wait.

---

### T1 — bpmn-dsl compilation pipeline: parse / lint / DAG

**Goal:** bpmn-dsl source compiles to a Populated Execution DAG through the same compiler that handles ob-poc DSL, with `@cbu` placeholder inference working.

**Sonnet tasks (scope determined by T0 findings):**

1. **Parser path:** if NOM-based bpmn-dsl parser is in place, verify it produces the s-expression form §3 expects. If gaps exist, close them.
2. **AST representation:** the parsed s-expression IS the AST. No separate intermediate representation. Verify the compiler entry point accepts this form.
3. **Linter pass — placeholder inference:**
   - Walk workflow; identify each service-task/business-rule-task by `:verb` or `:decision` attribute
   - Look up each in catalogue; retrieve produces/consumes binding type declarations
   - Allocate placeholder slot per produced binding type (single-binding-per-type)
   - Thread placeholder slot to downstream consumers
   - Validate no consumer references a slot that hasn't been produced upstream
   - Validate type consistency: a CBU placeholder only flows to verbs that consume CBU
4. **Linter pass — unresolved references:**
   - Every `:verb` resolves to a catalogue entry
   - Every `:decision` resolves to a dmn-lite catalogue entry
   - Every `:next` resolves to a defined node
   - Every gateway predicate references defined placeholder slots
5. **DAG pass:**
   - Topology: walk `:next` and gateway flows; produce DAG edges
   - Validation: acyclic (cycles caught and reported)
   - Resource dependencies: derived from catalogue declarations for each node's verb/decision
   - Effect class: derived from catalogue per node
   - Concurrency policy: derived from effect class per Phase 5 framework
6. **Compiler output:** ExecutablePlan submittable to Phase 5 engine, with `sem_os_snapshot_id` populated, instructions complete, bindings frame schema populated.
7. **Tests:**
   - Parse §3 demo model successfully
   - Linter resolves all placeholders correctly
   - Linter rejects ill-formed examples (unresolved verb, type mismatch, undefined `:next`)
   - DAG pass produces valid DAG for §3 model
   - All five distinct end-to-end paths (fund / corporate / trust × instrument matrix) produce valid DAGs

**DoD:** §3 model expressed as bpmn-dsl s-expressions compiles cleanly to a Populated Execution DAG. The DAG passes the Phase 5 validator. `@cbu` and `@cbu-type` placeholders are inferred and threaded correctly through the workflow without explicit declaration in the source.

**STOP gate.**

---

### T2 — Lifecycle event emission + subscriber mechanism

**Goal:** Phase 5 engine emits typed lifecycle events on commit; bpmn-lite subscribes and dispatches to waiting process instances.

**Sonnet tasks (scope determined by T0 findings):**

1. **Event emission from engine** (if not already present per T0):
   - On `Committed` outcome, emit typed events: `VerbCompleted { verb_id, resolved_bindings, plan_id, execution_id, snapshot_id }`, and for verbs that mutate entity state, additional `EntityCreated` / `FSMTransitioned` events
   - Implementation: publisher reads from `dsl_execution_audit` (per Phase 5 T14 commit `06e59a96`) and dispatches to registered subscribers
   - Events carry resolved binding values (concrete UUIDs), not placeholders — placeholders are a compile-time concept
2. **Subscriber registration:**
   - A subscriber registry; bpmn-lite registers a reactor
   - Engine dispatches events to all registered subscribers
   - Sage and UI can register their own subscribers in later tranches without changes here
3. **Idempotent event delivery:**
   - Events carry sequence numbers; subscribers track last-processed; re-delivery after restart doesn't double-process
4. **bpmn-lite reactor:**
   - Receives `VerbCompleted` events
   - Looks up process instances waiting on this verb's completion (by `plan_id` or by `verb_id + correlation`)
   - Loads waiting process instance state from Postgres
   - Returns the resolved binding values to the process advancement code in T3
5. **Tests:**
   - Plan completes → event emitted → subscriber receives → correct values delivered
   - Engine restart → events not re-emitted to already-processed subscribers
   - Multiple subscribers receive same event independently

**DoD:** Phase 5 engine emits lifecycle events on commit; bpmn-lite reactor receives them; sequence-number idempotency works across restart.

**STOP gate.**

---

### T3 — BPMN executor: callout / pause / subscribe / resume state machine

**Goal:** BPMN process instances advance via the pause/subscribe/resume state machine, durable in Postgres between callouts.

**Sonnet tasks (scope determined by T0 findings):**

1. **Process instance persistence:**
   - `bpmn_process_instance` table: id, workflow_id, current_node, status (Created / Running / WaitingOnCallout / Completed / Failed), variables (JSONB), started_at, last_advanced_at
   - `bpmn_process_callout` table: process_instance_id, node_id, plan_id (the inner DSL plan submitted), submitted_at, completed_at, outcome
2. **Executor as state machine:**
   - `start_process(workflow_id, initial_variables)` — creates instance, advances to first node
   - `advance(instance_id, resolved_values)` — given the values returned from a completed callout, bind them into placeholder scope, evaluate gateway predicates, advance to next node
   - At each service-task / business-rule-task node:
     - Resolve catalogue entry; build inner ExecutablePlan via T1 compiler
     - Submit plan to Phase 5 engine
     - Record callout in `bpmn_process_callout`
     - Set instance status to `WaitingOnCallout`
     - Persist; return (no thread parked)
   - At each gateway:
     - Evaluate predicates against current placeholder scope
     - Pick matching flow; advance to next node
   - At end-event: mark instance Completed
3. **Reactor integration:**
   - When T2 reactor receives `VerbCompleted` event matching a waiting callout's plan_id:
     - Update `bpmn_process_callout` with outcome
     - Invoke `advance(instance_id, resolved_values)`
4. **Failure handling:**
   - Verb-level `VerbFailed` outcome → process instance marked Failed; reason recorded
   - `OptimisticConflict` → re-submit (single retry) then mark Failed if still conflicting
   - `LockTimeout` / `TimedOut` → mark process instance Failed with explicit reason
5. **Tests:**
   - Full §3 demo runs end-to-end with all three CBU type paths
   - Process instance state survives engine restart (resume from `WaitingOnCallout`)
   - Verb failure produces correct Failed state
   - Test fixtures drive all three DMN outcomes (fund / corporate / trust)

**DoD:** §3 demo model runs end-to-end via the state machine. All three end-to-end paths complete. Process state survives restart.

**STOP gate.**

---

### T4 — Pre-coded demo BPMN model

**Goal:** §3 model is one function call away; runs reliably with test fixtures.

**Sonnet tasks:**

1. **Constructor:** `fn custody_cbu_onboarding_workflow() -> WorkflowSource` returning the §3 model as a bpmn-dsl s-expression value (constructed in Rust, not parsed from text — the AST is the s-expression)
2. **Demo seed:**
   - Verb catalogue entries for any missing `cbu.*` verbs (per T0 findings)
   - DMN decision `cbu_type_routing` (if not already present)
   - Sample CBU input data for each of three types
3. **Integration test:**
   - Construct workflow source via §1 constructor
   - Compile through T1 pipeline → ExecutablePlan
   - Start a process instance with fund-type input → run to completion → assert end state
   - Repeat for corporate-type → assert
   - Repeat for trust-type → assert
4. **Reset helper:** `fn reset_demo_state()` clearing process_instance, process_callout, and any test-created entities for clean re-runs

**DoD:** demo workflow constructible in one call; integration test verifies all three paths complete; reset helper restores clean state.

**STOP gate.**

---

### T5 — Sage agentic integration

**Goal:** at least one service task in the demo flow goes through Sage; Sage reasoning persisted and visible.

**Sonnet tasks (scope determined by T0 findings on Sage current state):**

1. **Sage subscriber:** Sage registers as a subscriber to lifecycle events from the Phase 5 engine (same mechanism bpmn-lite uses)
2. **Sage decision point** in demo flow: one service task in §3 (suggest `cbu.add_instrument_matrix` since it follows the gateway branches and converges paths) routes through Sage
   - Sage receives the catalogue invocation request
   - Sage reads current process instance state from durable storage
   - Sage walks Semantic Dependency Graph to confirm legal next-step
   - Sage submits the actual plan against the Phase 5 engine (same engine bpmn-lite uses)
   - Sage's reasoning recorded in audit trail with structured form (decision input / options considered / chosen / rationale)
3. **Audit visibility:** Sage's reasoning surfaces in `dsl_execution_audit` (per Phase 5 T14) with `actor: Sage` and structured reasoning JSON
4. **Tests:**
   - Sage-mediated service task completes the process correctly
   - Reasoning captured in audit trail
   - Process completes normally for all three CBU type paths

**DoD:** at least one service task goes through Sage; Sage reasoning persisted in audit trail; process completes for all three demo paths.

**Fallback:** if T0 reveals Sage cannot yet submit plans against the shared engine, T5 reduces to "observation mode" — Sage subscribes, observes, presents reasoning *about* what bpmn-lite did, but does not itself submit plans. Less wow factor; still tells the agentic story.

**STOP gate.**

---

### T6 — ob-poc UI repointing

**Goal:** existing ob-poc UI displays bpmn-lite process state, plan submissions, Sage reasoning, DMN results in real time.

**Sonnet tasks:**

1. **API endpoints in bpmn-lite:** REST or SSE endpoints exposing:
   - Process instance state (current node, status, variables, history)
   - Lifecycle event stream (subscribe to live events)
   - DMN decision results (when business rule tasks fire)
   - Sage reasoning (from audit trail with `actor: Sage`)
2. **UI configuration:** point ob-poc UI at bpmn-lite endpoints (configuration setting; not a hard rebuild)
3. **UI panels:**
   - **Workflow panel:** BPMN process visualised; current node highlighted; completed nodes marked; gateway routing visible
   - **Plan feed:** live stream of plans submitted to engine, with outcomes
   - **Sage panel:** Sage reasoning when active in the flow
   - **DMN panel:** decision invocations with inputs, table evaluation, outputs
4. **Manual test:** walk through full demo flow; verify each of four panels populates correctly for each of three CBU type paths

**DoD:** UI displays demo process running across all four panels; all three demo paths verified visually; no console errors; reasonable refresh rate (sub-second event display).

**STOP gate.**

---

### T7 — Docker deployment integration

**Goal:** entire stack runs in existing Docker deployment with one command; reset script returns to clean state.

**Sonnet tasks:**

1. **bpmn-lite Docker image:** builds with ob-poc shared engine dependency wired correctly
2. **docker-compose (or equivalent):** brings up Postgres, shared engine, bpmn-lite, ob-poc UI in correct order with health checks
3. **Service discovery:** UI knows where bpmn-lite is; bpmn-lite knows where Postgres is; subscribers register against the engine on startup
4. **Migration on startup:** new tables (`bpmn_process_instance`, `bpmn_process_callout`) created; demo seed data loaded
5. **Single-command start:** `docker-compose up` brings the whole stack live and ready
6. **Reset script:** `./demo-reset.sh` returns to clean demo-ready state (fresh DB tables, no process instances, fresh seed data)
7. **Tests:** stack starts cleanly from cold; demo runs end-to-end in dockerised environment

**DoD:** `docker-compose up` brings stack live; `./demo-reset.sh` returns clean state; full demo verified in dockerised environment.

**STOP gate.**

---

### T8 — Demo polish + rehearsal

**Goal:** demo runs cleanly 5× in a row.

**Sonnet tasks:**

1. **Scripted demo flow:** ordered list of user actions producing the demo narrative
2. **Speaker notes:** what to say at each step; what to point at; expected outcomes; transitions between paths
3. **Demo data variations:** 3 different CBU inputs producing fund / corporate / trust paths
4. **Failure recovery documentation:** for each plausible failure mode (verb fails, DMN times out, Sage hangs, UI desyncs, engine restart mid-callout) — documented recovery path
5. **Rehearsal:** run demo 5× consecutively; capture flakiness; fix; repeat until stable
6. **Backup material:** screenshots of each demo beat in case live demo fails partially

**DoD:** 5 consecutive clean demo runs documented; speaker notes complete; failure recovery documented; backup material prepared.

**STOP gate. Demo ready.**

---

## 5. Master Demo DoD

The plan is complete when all of the following are true simultaneously:

1. bpmn-dsl source compiles via parse / lint / DAG pipeline to Populated Execution DAG
2. `@cbu` placeholder inference works without explicit binding declarations in source
3. Phase 5 engine emits lifecycle events; bpmn-lite reactor subscribes and dispatches
4. BPMN executor runs as pause/subscribe/resume state machine, durable across restarts
5. §3 demo model runs end-to-end through all three CBU type paths
6. Both placeholder resolutions (`@cbu`, `@cbu-type`) work correctly across callouts
7. At least one service task goes through Sage with persisted reasoning
8. ob-poc UI displays workflow, plans, Sage, DMN in real time
9. Entire stack runs via `docker-compose up` from clean state
10. Demo runs cleanly 5× consecutively from scripted flow
11. Speaker notes complete; failure recovery documented

---

## 6. Tranche dependency graph

```
T0 (audit)
  ↓
T1 (parse / lint / DAG pipeline)
  ↓
T2 (lifecycle events + subscriber)
  ↓
T3 (pause/subscribe/resume executor)
  ↓
T4 (pre-coded demo model)
  ├─→ T5 (Sage integration)
  └─→ T6 (UI repointing)
        ↓
       T7 (Docker)
        ↓
       T8 (polish + rehearsal)
```

T5 and T6 are independent of each other; can run in either order after T4. Everything else strictly sequential.

---

## 7. Execution conventions

- **One tranche per session.** Sonnet completes the tranche, reports, stops at the STOP gate.
- **No commits without review.** Sonnet does not commit. Adam reviews diff, approves, commits separately.
- **Progress markers.** Sonnet reports % complete and current sub-step at each major step within a tranche.
- **Explicit "do not improvise" posture.** If Sonnet hits something outside the tranche scope, STOP and report — do not extend the tranche.
- **Phase 5 engine is closed.** Sonnet does not modify ob-poc engine code from `b7c5e5f1`. If something appears to require engine modification, STOP — Adam decides.
- **Replan from T0.** v0.1 work is superseded by the architecture commitments in §0. Sonnet starts T0 fresh against this document.

---

## 8. Risk register

**R1: T0 reveals catalogue gaps for §3 verbs.**
Mitigation: T0 surfaces gaps up front; T1 includes catalogue extensions in scope if minor. If extensions are large, demo model swaps to verbs known to exist.

**R2: T0 reveals current binding-flow handling is explicit-only (no inference mechanism).**
Mitigation: T1 includes placeholder inference implementation. If extending Phase 5 T10 mechanism is required, scope expands modestly — single-binding-per-type inference is bounded work.

**R3: T0 reveals current bpmn-lite executor is polling/orchestrating, not state-machine.**
Mitigation: T3 includes refactor to state machine model. This is the biggest possible T0 finding — adds substantial work to T3. If demo deadline tight, fallback: hybrid mode where executor polls but persists state in pause/subscribe shape. Less elegant; still demonstrable.

**R4: T0 reveals lifecycle event emission doesn't exist; only audit records do.**
Mitigation: T2 includes audit-to-event publisher. Bounded work; publisher is a thin shim reading from `dsl_execution_audit` and dispatching.

**R5: T0 reveals Sage cannot submit plans to shared engine today.**
Mitigation: T5 reduces to observation mode. Less wow factor; architectural story still lands.

**R6: T8 reveals flakiness Sonnet introduced earlier.**
Mitigation: time-boxed fix-and-repeat; if 5-consecutive-clean cannot be achieved, demo runs with most stable subset and remaining variations are screenshots.

---

## 9. Hand-off to Sonnet

For each tranche:

1. This document, scoped to the relevant tranche
2. The pre-locked decisions (§1)
3. The non-goals (§2)
4. The execution conventions (§7)
5. "Report findings at the STOP gate. Do not commit."

Begin with T0. v0.1 work is superseded — Sonnet should replan T0 fresh against this document.

---

## 10. Status tracking

```
Phase 5.5 v0.2 — bpmn-lite demo deployment
T0 ☑  T1 ☑  T2 ☐  T3 ☐  T4 ☐  T5 ☐  T6 ☐  T7 ☐  T8 ☐
Status: T1 complete (2026-05-20) — 23 dsl pipeline tests passing, 55 compiler tests unaffected
```

End of Phase 5.5 plan v0.2.
