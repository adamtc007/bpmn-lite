# Phase 5.5 Plan v0.3: bpmn-lite Demo Deployment

| Field | Value |
| --- | --- |
| Document ID | OB-POC-PHASE-5_5-PLAN-003 |
| Version | v0.3 |
| Status | DRAFT — async-by-default mechanism explicit; pending-call registry pattern; T2/T3 rewritten |
| Author | Adam Cearns |
| Date | 2026-05-20 |
| Supersedes | v0.2 (had two-DAG model and pause/subscribe/resume but left async mechanism implicit) |
| Position | Post-Phase 5 (`b7c5e5f1`); demo-driven; not Phase 6 proper |
| Repo | bpmn-lite (separate from ob-poc); shared DSL engine consumed as workspace dependency |
| Deployment | Existing Docker; bpmn-lite + shared engine + ob-poc UI repointed |
| Instruction to Sonnet | Replan from T0. v0.1 and v0.2 work is superseded. The architectural commitments in §0 are load-bearing — do not deviate. |

---

## 0. Architectural commitments

Four load-bearing commitments. Every tranche serves them.

### Commitment A: bpmn-dsl source IS the workflow graph

The workflow definition language is bpmn-dsl: s-expression form, same language family as ob-poc DSL. BPMN XML and ob-poc DSL text are two parseable surfaces of the same language family. There is no separate "BPMN AST" lowered to a DAG. The source is already structurally a graph.

Compilation pipeline:
1. **Parse** bpmn-dsl source — NOM generates the AST against the graph-structured source
2. **Linter** does semantic resolution: unresolved reference checks, `@cbu` placeholder allocation and threading, type consistency checks against catalogue declarations
3. **DAG pass** validates runtime execution order; produces the BPMN-shaped Populated Execution DAG embedded in an ExecutablePlan
4. **Executor** runs the DAG in order, async, via the mechanism in Commitment D

This is single-pass compilation through three semantic phases (parse / lint / DAG), not multi-pass lowering between distinct representations.

### Commitment B: Two DAGs at two scopes

| | BPMN workflow DAG | Inner DSL plan DAG |
|---|---|---|
| Scope | Process-instance | One invocation |
| Duration | Long-lived (seconds, hours, days) | Short-lived (one ExecutionFrame) |
| Persistence | Durable in Postgres between invocations | In-memory inside ExecutionFrame |
| Advances on | Lifecycle event arrival (callback) | Internal verb completion |
| Outcome class | `BpmnInstanceCompleted` / `BpmnInstanceWaiting` / `BpmnInstanceFailed` | Phase 5 outcomes (Committed / OptimisticConflict / etc.) |
| Owns | bpmn-lite | Phase 5 engine (`b7c5e5f1`) |

The BPMN workflow DAG is what foundational services see "running." The inner DSL plan DAGs are short-running plans submitted to the Phase 5 engine for each invocation.

### Commitment C: Workflow surface declares structure; catalogue resolves bindings

The bpmn-dsl workflow surface declares topology and verb/decision identities only. Placeholder flow (`@cbu`, `@cbu-type`) is *inferred by the compiler* from catalogue declarations:

- Service task with `:verb cbu.create` → catalogue says it produces a CBU → compiler allocates `@cbu` placeholder slot
- Service task with `:verb cbu.add_fund_product` → catalogue says it consumes a CBU → compiler threads `@cbu` from the producer
- Gateway predicate references `@cbu-type` → compiler resolves it to the slot most recently produced of matching type

For the demo, **single-binding-per-type inference** is sufficient. Multi-binding-per-type (parent + subsidiary CBU via explicit aliasing) is out of scope.

### Commitment D: Async-by-default; sync is a latency case, not an implementation

**There is one execution path. It is async. The demo exercises the fast case of that path.**

The BPMN executor never blocks waiting for a callback. All invocations are submitted asynchronously; the executor returns immediately after persistence; lifecycle events resume the process on arrival via independent subscriber tasks.

Mechanism: **pending-call registry** keyed by `execution_id` (the identity issued by the Phase 5 engine on plan submission). Durable in Postgres. Survives runtime restart. Supports invocations of arbitrary duration.

End-to-end flow for one invocation:

**Phase 1 — submission (synchronous, fast, no blocking):**
```
Executor reaches service-task node
  ↓
Compiles inner ExecutablePlan via T1 pipeline
  ↓
Submits to engine: engine.submit(plan) → returns execution_id immediately
  ↓
Inserts into bpmn_pending_invocation table:
  - execution_id (PK)  ← same identity as Phase 5 §10.3
  - process_instance_id
  - node_id
  - submitted_at
  - timeout_at (optional)
  ↓
Updates bpmn_process_instance:
  - current_node = X
  - status = WaitingOnInvocation
  - waiting_on_execution_id = execution_id
  ↓
Returns from executor — no thread parked, no future awaited
```

**Phase 2 — engine executes (own threads, own time, arbitrary duration):**
```
Engine schedules plan on ExecutionFrame
  ↓
Frame runs to outcome (Committed / OptimisticConflict / VerbFailed / etc.)
  ↓
Engine commits audit record with execution_id (per Phase 5 T14)
  ↓
Engine publishes lifecycle event with execution_id and outcome
  ↓
Engine returns to its work; does not know who is listening
```

**Phase 3 — event arrival (asynchronous, independent task):**
```
bpmn-lite subscriber receives event on its own task
  ↓
Looks up bpmn_pending_invocation by execution_id
  ↓
If row found:
  - Loads associated bpmn_process_instance
  - Binds resolved values from outcome into process variable scope
  - Atomically marks pending invocation complete (DELETE or status update)
  - Calls executor.advance(process_instance_id, outcome)
  ↓
advance() walks through synchronous nodes (gateway predicates, internal arithmetic)
  until it reaches the next callout or end event
  ↓
At next callout: back to Phase 1 with new execution_id
At end event: marks process complete; emits BpmnInstanceCompleted
At end of slice (callout queued): persists; returns
  ↓
If no row found for execution_id: event is for a different subscriber
  or already-handled invocation. Ignore.
```

This is **the** mechanism. The demo's apparent synchronicity is just Phases 1-2-3 completing in tens of milliseconds. The same code path supports invocations spanning days.

**Identity coupling:** `invocation_id` is not a new identity. The pending-call table uses `execution_id` (already issued by the engine, already recorded in `dsl_execution_audit`, already carried in lifecycle events) as the key. No duplicate identity machinery.

**Subscriber discipline:**
- Publisher (engine) is non-blocking: emits event and returns regardless of subscriber state
- Subscriber processes events on independent task; subscriber failure does not affect engine
- Event delivery is at-least-once; subscriber is idempotent (`pending_invocation` lookup-and-delete is atomic; second delivery finds no row)
- Backpressure is queue-based (bounded channel or polled audit-table replay); never blocking on the engine side

**Persistence discipline:**
- `bpmn_pending_invocation` is durable; survives restart
- Optional in-memory cache layered on top for fast-path lookup
- Restart recovery: subscriber on startup replays any committed audit records since last processed offset; finds matching pending rows; advances processes

---

## 1. Pre-locked decisions

1. **Workflow definition language** = s-expression bpmn-dsl. Same compiler as ob-poc DSL.
2. **Condition language** = s-expression predicate subset. Same machinery; same compiler.
3. **Binding convention** = service task `:verb` matches verb id in SemOS catalogue (exact match). `:name` is human label only. Unresolved `:verb` = compile error.
4. **Placeholder mechanism** = single-binding-per-type inference (`@cbu` style). Workflow surface stays declaration-free.
5. **Callback mechanism** = pending-call registry keyed by `execution_id`. Durable in Postgres. Async by default.
6. **Identity coupling** = pending-call table uses `execution_id` (Phase 5 §10.3 identity). No new identity primitive.
7. **Demo BPMN model** = CBU lifecycle only (no KYC, no UBO). Pre-coded in Rust.
8. **Gateways in demo** = sequential + one exclusive gateway routed by DMN. No parallel / inclusive.
9. **Subscriber model** = bpmn-lite owns its own pending-call table. Multi-subscriber shared infrastructure deferred.
10. **No Phase 6 architectural work** in this plan.
11. **STOP gates** between every tranche. Sonnet reports; Adam reviews diff; Adam approves commit.

---

## 2. Non-goals (explicit)

To prevent Sonnet drift:

- BPMN XML parser/loader (Stage 2 — not this plan)
- Parallel or inclusive gateways
- Boundary events, event sub-processes, multi-instance markers, compensation
- Timer scheduler, message correlator (sibling services; sized but not built)
- Multi-worker pool, lanes, admission controller
- Plan persistence (Q9), cross-snapshot replay (Q12), audit retention (Q15)
- KYC, UBO, screening, completeness — explicitly removed from demo model
- Multi-binding-per-type placeholder syntax
- Phase 6 coordination primitives (fan-out, WaitN, cancellation scopes)
- Production-hardening of async mechanism: timeout sweep, dead-letter queue, orphan reconciliation (sized but deferred to Phase 5.6)
- Verb catalogue extensions beyond what the demo BPMN model invokes
- Shared cross-subscriber pending-call infrastructure (each subscriber owns its own table)
- Any rework of Phase 5 engine internals — engine is closed at `b7c5e5f1`

If Sonnet's plan touches any of the above, STOP and report — do not implement.

---

## 3. Demo BPMN model (locked)

**CBU lifecycle — sequential with one exclusive gateway routed by DMN:**

```
[Start]
   ↓
[Service: cbu.create]                ← produces @cbu
   ↓
[Business Rule: cbu_type_routing]    ← consumes @cbu; produces @cbu-type
   ↓
[Gateway: type-gateway]
   ├─ (= @cbu-type "fund")      → [Service: cbu.add_fund_product]
   ├─ (= @cbu-type "corporate") → [Service: cbu.add_corporate_product]
   └─ (= @cbu-type "trust")     → [Service: cbu.add_trust_product]
   ↓                              (all three converge)
[Service: cbu.add_instrument_matrix] ← consumes @cbu
   ↓
[End: CBU Operational]
```

**As bpmn-dsl s-expression source:**

```scheme
(workflow custody-cbu-onboarding
  (start-event :id start :next create-cbu)
  (service-task :id create-cbu :verb cbu.create :next type-decision)
  (business-rule-task :id type-decision :decision cbu_type_routing :next type-gateway)
  (exclusive-gateway :id type-gateway
    (flow :condition (= @cbu-type "fund")      :next add-fund)
    (flow :condition (= @cbu-type "corporate") :next add-corp)
    (flow :condition (= @cbu-type "trust")     :next add-trust))
  (service-task :id add-fund  :verb cbu.add_fund_product      :next add-im)
  (service-task :id add-corp  :verb cbu.add_corporate_product :next add-im)
  (service-task :id add-trust :verb cbu.add_trust_product     :next add-im)
  (service-task :id add-im    :verb cbu.add_instrument_matrix :next end)
  (end-event :id end :status "Operational"))
```

Workflow surface declares **structure only**: `:next` for topology, `:verb` for catalogue lookup, `:decision` for DMN lookup, `:condition` for gateway predicates. **No `:produces` or `:consumes` declarations.** Placeholder flow inferred by the compiler from catalogue.

5 service tasks, 1 business rule task, 1 exclusive gateway, 3 routing paths, 1 end state, 1 implicit `@cbu` placeholder, 1 implicit `@cbu-type` placeholder.

---

## 4. Tranches

### T0 — Pre-flight audit (NO CODE)

**Goal:** establish ground truth before any execution. Particular attention to async mechanism prerequisites.

**Sonnet tasks:**

1. **Inventory bpmn-lite repo structure:** directories, crates, Cargo.toml dependencies, workspace layout
2. **Inventory existing bpmn-dsl parser:**
   - NOM-based parsing in place?
   - AST shape (s-expression form expected)
   - Test pack contents
   - Surface adapter status (XML / direct construction / both)
3. **Inventory shared DSL engine integration:**
   - Is ob-poc declared as workspace dependency?
   - Can bpmn-lite invoke ob-poc compiler entry points?
   - Can it submit plans to the Phase 5 engine and receive `execution_id`?
4. **Inventory linter / semantic resolution layer:**
   - Existing placeholder/binding-flow handling?
   - Phase 5 T10 (`22d6821e`) typed binding slots reachable from bpmn-dsl path?
5. **Inventory DAG pass / execution order validation:**
   - Does compilation produce Populated Execution DAG passing Phase 5 validator?
6. **Inventory BPMN runtime execution model:**
   - **Critical:** is the executor structured as async pause/resume state machine, or as polling/blocking?
   - Does it persist process state between callouts?
   - Are there `block_on()` or synchronous `await` patterns in the call path that would prevent the async model?
7. **Inventory lifecycle event mechanism:**
   - Does Phase 5 engine emit typed events on commit?
   - If not, is there a publisher path from `dsl_execution_audit` to subscribers?
   - **Critical:** what would the event publisher look like — bounded channel, audit-table polling, both?
8. **Inventory subscriber/reactor mechanism in bpmn-lite:**
   - Existing subscriber pattern?
   - If absent: scope to build one as part of T2
9. **Inventory pending-call mechanism:**
   - **Critical:** is there any existing correlation/registry mechanism in bpmn-lite or shared infrastructure that could host pending invocations?
   - Or is this entirely new construction in T2/T3?
10. **Inventory catalogue for demo verbs:**
    - `cbu.create` — present? Effect class? Produces CBU?
    - `cbu.add_fund_product`, `cbu.add_corporate_product`, `cbu.add_trust_product` — same
    - `cbu.add_instrument_matrix` — same
11. **Inventory DMN decisions:**
    - `cbu_type_routing` — present? Input/output schema?
12. **Inventory Docker deployment:**
    - Services, networking, where bpmn-lite slots in
    - How ob-poc UI connects
13. **Inventory ob-poc UI:**
    - Current state; API endpoints; component reuse potential

**DoD:** structured findings report. Each of the 13 items has a clear answer. Critical items (6, 7, 9) explicitly assessed for async compatibility. No code changes. Adam reviews; identifies gap-to-tranche mapping.

**STOP gate.** Do not commit anything. Report findings and wait.

---

### T1 — bpmn-dsl compilation pipeline: parse / lint / DAG

**Goal:** bpmn-dsl source compiles to a Populated Execution DAG through the same compiler that handles ob-poc DSL, with `@cbu` placeholder inference working.

**Sonnet tasks (scope determined by T0 findings):**

1. **Parser path:** ensure NOM-based bpmn-dsl parser produces the s-expression form §3 expects
2. **AST representation:** the parsed s-expression IS the AST; no separate intermediate representation
3. **Linter pass — placeholder inference:**
   - Walk workflow; identify each service-task/business-rule-task by `:verb` or `:decision`
   - Look up each in catalogue; retrieve produces/consumes binding type declarations
   - Allocate placeholder slot per produced binding type (single-binding-per-type)
   - Thread placeholder slot to downstream consumers along DAG edges
   - Validate no consumer references a slot not produced upstream
   - Validate type consistency
4. **Linter pass — unresolved references:**
   - Every `:verb` resolves to a catalogue entry
   - Every `:decision` resolves to a dmn-lite catalogue entry
   - Every `:next` resolves to a defined node
   - Every gateway predicate references defined placeholder slots
5. **DAG pass:**
   - Topology from `:next` and gateway flows
   - Acyclic validation
   - Resource dependencies from catalogue declarations per node
   - Effect class from catalogue per node
   - Concurrency policy from effect class per Phase 5 framework
6. **Compiler output:** ExecutablePlan submittable to Phase 5 engine, with `sem_os_snapshot_id` populated, instructions complete, bindings frame schema populated
7. **Tests:**
   - Parse §3 demo model successfully
   - Linter resolves all placeholders correctly
   - Linter rejects ill-formed inputs (unresolved verb, type mismatch, undefined `:next`)
   - DAG pass produces valid DAG
   - All five distinct end-to-end paths produce valid DAGs

**DoD:** §3 model compiles cleanly to a Populated Execution DAG. Phase 5 validator accepts it. `@cbu` and `@cbu-type` inferred and threaded correctly without explicit declaration.

**STOP gate.**

---

### T2 — Async event publisher + bpmn-lite subscriber + pending-call registry

**Goal:** Phase 5 engine publishes lifecycle events asynchronously; bpmn-lite subscribes; pending-call registry mediates async RPC between executor and engine.

**Sonnet tasks (scope determined by T0 findings):**

#### T2a — Event publisher on the engine side

1. **Publisher infrastructure** (if not present per T0):
   - On `Committed` outcome (and other outcomes as relevant), publish typed event: `LifecycleEvent::VerbCompleted { execution_id, outcome, resolved_bindings, snapshot_id, attempted_at, committed_at, audit_sequence }`
   - Publisher dispatches to registered subscribers via bounded async channel (tokio mpsc or similar)
   - **Engine is non-blocking:** publisher uses `try_send` or equivalent; never blocks engine on subscriber backpressure
   - If channel is full: drop or buffer to durable replay; engine continues regardless
2. **Audit-as-source-of-truth backup:**
   - Events derive from `dsl_execution_audit` records (Phase 5 T14)
   - If the in-memory channel drops or subscriber is offline, the audit table is the durable record
   - Subscribers can replay from audit by `audit_sequence` after restart or backlog
3. **Subscriber registration API:**
   - `engine.register_subscriber(subscriber_id, channel_sender)` — subscriber registers and receives events
   - Subscribers identified by ID; engine doesn't know what they do with events
   - Multiple subscribers supported (bpmn-lite, Sage, UI, audit — each independent)

#### T2b — pending-call registry in bpmn-lite

1. **Schema:** `bpmn_pending_invocation` table
   ```sql
   CREATE TABLE bpmn_pending_invocation (
     execution_id UUID PRIMARY KEY,
     process_instance_id UUID NOT NULL,
     node_id TEXT NOT NULL,
     submitted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
     timeout_at TIMESTAMPTZ,
     INDEX idx_process_instance (process_instance_id)
   );
   ```
2. **Registry API:**
   - `registry.register(pending: PendingInvocation) → Result<()>` — INSERT row; non-blocking
   - `registry.complete(execution_id) → Result<Option<PendingInvocation>>` — atomic SELECT + DELETE; returns the registered invocation or None if not found
   - `registry.list_expired(now) → Result<Vec<PendingInvocation>>` — for timeout sweep (defer mechanism to Phase 5.6; expose the API in T2)
3. **In-memory cache** (optional, defer if T0 reveals time pressure):
   - HashMap<ExecutionId, PendingInvocation> in front of the table
   - Cache invalidation on register/complete
   - Fast path for sub-second invocations; correctness still in DB

#### T2c — bpmn-lite subscriber

1. **Subscriber task:**
   ```rust
   async fn run_subscriber(receiver: mpsc::Receiver<LifecycleEvent>, ...) {
     while let Some(event) = receiver.recv().await {
       if let Err(e) = handle_event(event, registry, executor).await {
         tracing::error!("subscriber error: {}", e);
         // continue; do not crash; do not propagate to engine
       }
     }
   }
   ```
2. **handle_event logic:**
   - Extract `execution_id` from event
   - `registry.complete(execution_id)` — atomic
   - If `Some(pending)`: call `executor.advance(pending.process_instance_id, event.outcome)`
   - If `None`: log debug; ignore (event for another subscriber or already handled)
3. **Restart recovery:**
   - On startup, query `dsl_execution_audit` for records with `audit_sequence > last_processed_sequence`
   - For each record with a matching pending invocation, dispatch as if event had arrived live
   - Mark `last_processed_sequence` after each successful handle
4. **Subscriber isolation:**
   - Subscriber error never propagates to engine
   - Subscriber crash does not affect engine commits
   - Subscriber restart resumes from `last_processed_sequence`; no events lost

5. **Tests:**
   - Plan completes → event emitted → subscriber receives → pending invocation completed → advance() called
   - Engine restart mid-execution → audit record persists → subscriber on restart replays → process advances
   - Subscriber error during event handling → engine continues; error logged; subsequent events still handled
   - Subscriber receives event with no matching pending row → ignored cleanly
   - Multiple events for same execution_id (at-least-once delivery) → only first triggers advance; second is no-op

**DoD:** engine publishes events non-blockingly; subscriber receives and dispatches; pending-call registry round-trips correctly; restart recovery works; isolation properties verified.

**STOP gate.**

---

### T3 — BPMN executor as async state machine

**Goal:** BPMN process instances advance through the pause/persist/resume state machine, never blocking on callouts.

**Sonnet tasks (scope determined by T0 findings):**

1. **Process instance persistence:**
   - `bpmn_process_instance` table:
     ```sql
     CREATE TABLE bpmn_process_instance (
       id UUID PRIMARY KEY,
       workflow_id TEXT NOT NULL,
       current_node TEXT NOT NULL,
       status TEXT NOT NULL, -- Created / Running / WaitingOnInvocation / Completed / Failed
       variables JSONB NOT NULL DEFAULT '{}',
       waiting_on_execution_id UUID, -- nullable; set when WaitingOnInvocation
       started_at TIMESTAMPTZ NOT NULL,
       last_advanced_at TIMESTAMPTZ NOT NULL,
       completed_at TIMESTAMPTZ
     );
     ```
2. **Executor API — all non-blocking:**
   - `executor.start_process(workflow_source, initial_variables) → Result<ProcessInstanceId>`
     - Compiles workflow via T1
     - Inserts new process_instance row
     - Calls advance_internal() to walk through synchronous nodes until first callout
     - Returns process_instance_id; does NOT wait for completion
   - `executor.advance(instance_id, outcome) → Result<()>`
     - Called by subscriber when event arrives
     - Loads process state from DB
     - Binds outcome's resolved values into variable scope
     - Calls advance_internal() to walk through synchronous nodes until next callout or end
     - Returns; does NOT wait for next callout
   - `executor.cancel(instance_id) → Result<()>`
     - Sets status to Cancelled
     - Deletes any pending_invocation rows for this instance
     - Returns immediately
3. **advance_internal — the synchronous walking slice:**
   ```
   loop:
     current_node = load current_node from process_instance
     match current_node:
       service_task or business_rule_task:
         compile inner plan
         submit to engine; get execution_id
         registry.register(pending invocation with execution_id)
         update process_instance: current_node = node, status = WaitingOnInvocation, waiting_on_execution_id = execution_id
         RETURN  -- waiting for subscriber to call advance() when event arrives
       exclusive_gateway:
         evaluate predicates against process variable scope
         pick matching flow (or default flow); error if none match
         update process_instance: current_node = next, status = Running
         continue loop  -- walks through immediately, no callout
       start_event:
         update process_instance: current_node = first non-start node, status = Running
         continue loop
       end_event:
         update process_instance: status = Completed, completed_at = now
         emit BpmnInstanceCompleted event
         RETURN
   ```
4. **Failure handling:**
   - `VerbFailed` outcome → mark process instance Failed; record reason; emit BpmnInstanceFailed
   - `OptimisticConflict` → single automatic retry by re-submitting the same plan; mark Failed if second attempt also conflicts
   - `LockTimeout` / `TimedOut` → mark Failed with explicit reason; emit BpmnInstanceFailed
5. **State invariants:**
   - At every commit boundary, process_instance row reflects current truth
   - No in-memory state required to advance — load from DB is always sufficient
   - Restart mid-WaitingOnInvocation: subscriber on next event call advance() with the loaded state; resumes correctly
6. **Tests:**
   - **Async-correctness test:** start a process; verify start_process() returns immediately; verify status is WaitingOnInvocation; verify no thread is parked anywhere; wait synthetic delay; deliver mock event; verify process advances
   - **Long-wait test:** start a process; wait >10 seconds without delivering event; verify no in-process resources are held; verify Postgres state is durable and correct
   - **Restart-during-callout test:** start process; submit plan; before event arrives, restart bpmn-lite; on startup, subscriber replays from audit; verify process advances correctly
   - **Full §3 demo test:** run end-to-end for all three CBU type paths; verify all reach correct end state
   - **Failure tests:** VerbFailed, OptimisticConflict (retry then fail), TimedOut — verify correct Failed state and reasoning

**DoD:** executor is genuinely async; verified by long-wait test (process in WaitingOnInvocation >10s without resources held); §3 demo runs end-to-end for all three paths; restart recovery works.

**STOP gate.**

---

### T4 — Pre-coded demo BPMN model

**Goal:** §3 model is one function call away; runs reliably with test fixtures.

**Sonnet tasks:**

1. **Constructor:** `fn custody_cbu_onboarding_workflow() -> WorkflowSource` returning the §3 model as bpmn-dsl s-expression value (constructed in Rust, not parsed from text)
2. **Demo seed:**
   - Verb catalogue entries for any `cbu.*` verbs not present (per T0)
   - DMN decision `cbu_type_routing` if not present
   - Sample CBU input data for fund / corporate / trust types
3. **Integration test:**
   - Construct workflow source via §1
   - Compile through T1 pipeline → ExecutablePlan
   - `executor.start_process()` with fund-type input → wait for completion via polling status — verify Completed
   - Repeat for corporate-type → verify
   - Repeat for trust-type → verify
   - Verify each end state has appropriate audit trail
4. **Reset helper:** `fn reset_demo_state()` truncating bpmn_process_instance, bpmn_pending_invocation, and any test-created entities

**DoD:** demo workflow constructible in one call; integration test verifies all three paths complete; reset helper restores clean state.

**STOP gate.**

---

### T5 — Sage agentic integration

**Goal:** at least one service task in the demo flow routes through Sage; Sage reasoning persisted and visible.

**Sonnet tasks (scope determined by T0 findings on Sage current state):**

1. **Sage as subscriber:** Sage registers with engine subscriber API; receives same lifecycle events as bpmn-lite
2. **Sage decision point** in demo flow: suggest `cbu.add_instrument_matrix` (post-gateway convergence point — visually clean for the demo)
   - When BPMN executor reaches this node, it submits to engine with metadata indicating "Sage-mediated"
   - Sage receives the corresponding `VerbCompleted` event in addition to bpmn-lite
   - For the Sage-mediated case: the original verb invocation is replaced by a Sage decision verb that consults the catalogue, walks the Semantic Dependency Graph for the verb being mediated, and submits the actual verb invocation
3. **Sage reasoning recording:** structured form in audit trail with `actor: Sage`, decision input, options considered, chosen, rationale
4. **Tests:**
   - Sage-mediated service task completes the process correctly
   - Reasoning captured in audit trail
   - Process completes for all three CBU type paths

**Fallback:** if T0 reveals Sage cannot submit plans to shared engine, T5 reduces to observation mode — Sage subscribes, observes, presents reasoning *about* what bpmn-lite did, but does not submit plans itself.

**DoD:** at least one service task goes through Sage; Sage reasoning persisted; process completes for all three demo paths.

**STOP gate.**

---

### T6 — ob-poc UI repointing

**Goal:** existing ob-poc UI displays bpmn-lite process state, plan submissions, Sage reasoning, DMN results in real time.

**Sonnet tasks:**

1. **API endpoints in bpmn-lite:** REST + SSE / WebSocket
   - Process instance state (current node, status, variables, history)
   - Lifecycle event stream (subscribe to live events for visualisation)
   - DMN decision results
   - Sage reasoning (from audit with `actor: Sage`)
2. **UI configuration:** point ob-poc UI at bpmn-lite endpoints (configuration; not a rebuild)
3. **UI panels:**
   - **Workflow panel:** BPMN process visualised; current node highlighted; completed nodes marked; gateway routing visible; WaitingOnInvocation state indicated clearly
   - **Plan feed:** live stream of plans submitted, with `execution_id` and outcome
   - **Sage panel:** reasoning when Sage active
   - **DMN panel:** decision invocations with inputs / table / outputs
4. **Manual test:** walk through full demo flow; verify all four panels populate correctly for each of three paths

**DoD:** UI displays demo process running across all four panels; all three demo paths verified visually; no console errors; sub-second event display.

**STOP gate.**

---

### T7 — Docker deployment integration

**Goal:** entire stack runs in existing Docker deployment with one command; reset script returns to clean state.

**Sonnet tasks:**

1. **bpmn-lite Docker image** builds with ob-poc shared engine dependency wired
2. **docker-compose** brings up Postgres, shared engine, bpmn-lite, ob-poc UI in correct order with health checks
3. **Service discovery:** UI knows bpmn-lite; bpmn-lite knows Postgres; subscribers register against engine on startup
4. **Migration on startup:** new tables created (`bpmn_process_instance`, `bpmn_pending_invocation`); demo seed loaded
5. **Single-command start:** `docker-compose up` brings stack live and ready
6. **Reset script:** `./demo-reset.sh` returns to clean state
7. **Async-correctness in docker:** verify the WaitingOnInvocation >10s test from T3 works inside the dockerised environment (event delivery across container boundaries; persistence across container restarts)
8. **Tests:** stack starts cleanly cold; demo runs end-to-end dockerised

**DoD:** `docker-compose up` brings stack live; reset returns clean state; full demo verified in dockerised environment; async correctness holds across container boundaries.

**STOP gate.**

---

### T8 — Demo polish + rehearsal

**Goal:** demo runs cleanly 5× in a row.

**Sonnet tasks:**

1. **Scripted demo flow:** ordered user actions producing the demo narrative
2. **Speaker notes:** what to say at each step; what to point at; expected outcomes; transitions between paths; *what to say about async mechanism when foundational services ask "how does this scale"*
3. **Demo data variations:** 3 inputs producing fund / corporate / trust paths
4. **Failure recovery documentation:** verb fails, DMN times out, Sage hangs, UI desyncs, engine restart mid-callout — documented recovery for each
5. **Rehearsal:** 5 consecutive runs; capture flakiness; fix; repeat until stable
6. **Backup material:** screenshots of each beat in case live demo fails partially
7. **Async story:** prepare 2-minute explanation of "pending-call registry / lifecycle events / restart recovery" for the question that will be asked

**DoD:** 5 consecutive clean runs documented; speaker notes complete; failure recovery documented; backup material prepared; async story prepared.

**STOP gate. Demo ready.**

---

## 5. Master Demo DoD

Plan is complete when all simultaneously true:

1. bpmn-dsl source compiles via parse / lint / DAG pipeline to Populated Execution DAG
2. `@cbu` placeholder inference works without explicit declarations
3. Phase 5 engine publishes lifecycle events asynchronously (non-blocking)
4. bpmn-lite subscriber receives events on independent task; restart recovery works
5. Pending-call registry round-trips correctly via `execution_id`
6. BPMN executor is verifiably async — passes >10s long-wait test
7. §3 demo model runs end-to-end through all three CBU type paths
8. Both placeholder resolutions (`@cbu`, `@cbu-type`) work across invocations
9. At least one service task routes through Sage with persisted reasoning
10. ob-poc UI displays workflow / plans / Sage / DMN in real time
11. Entire stack runs via `docker-compose up` from clean state
12. Async correctness verified across container boundaries
13. Demo runs cleanly 5× consecutively from scripted flow
14. Speaker notes complete; failure recovery documented; async story prepared

---

## 6. Tranche dependency graph

```
T0 (audit)
  ↓
T1 (parse / lint / DAG pipeline)
  ↓
T2 (event publisher + subscriber + registry) ← ASYNC FOUNDATION
  ↓
T3 (async state machine executor)             ← VERIFIED BY LONG-WAIT TEST
  ↓
T4 (pre-coded demo model)
  ├─→ T5 (Sage integration)
  └─→ T6 (UI repointing)
        ↓
       T7 (Docker)
        ↓
       T8 (polish + rehearsal)
```

T5 and T6 independent; can run in either order after T4. Everything else sequential. **T2 + T3 are the async foundation — get them right.**

---

## 7. Execution conventions

- **One tranche per session.** Sonnet completes, reports, stops at STOP gate.
- **No commits without review.** Sonnet does not commit. Adam reviews diff, approves, commits.
- **Progress markers.** Sonnet reports % complete and current sub-step.
- **No improvisation outside tranche scope.** Hit something outside scope → STOP and report.
- **Phase 5 engine is closed.** No modifications to `b7c5e5f1` engine code. If engine modification seems required → STOP, Adam decides.
- **No `block_on()` in async paths.** Any synchronous waiting in the executor or subscriber call path is a defect. Async correctness is verifiable; verify it.
- **Replan from T0.** v0.1 and v0.2 work superseded by architecture commitments in §0. Sonnet starts T0 fresh.

---

## 8. Risk register

**R1: T0 reveals catalogue gaps for §3 verbs.**
Mitigation: T0 surfaces gaps; T1 includes catalogue extensions if minor. Major: swap to verbs known to exist.

**R2: T0 reveals current binding-flow handling is explicit-only.**
Mitigation: T1 includes placeholder inference. Extends Phase 5 T10 mechanism modestly.

**R3: T0 reveals current bpmn-lite executor has synchronous patterns (`block_on`, parked threads).**
Mitigation: T3 includes refactor to async state machine. **Biggest possible T0 finding.** No fallback for the demo if this is large — the async commitment is non-negotiable. If refactor is too large, demo fallback is observation-mode only (Sage observes; no Sage-driven invocations).

**R4: T0 reveals lifecycle event emission doesn't exist; only audit records do.**
Mitigation: T2a includes audit-to-event publisher. Bounded work.

**R5: T0 reveals Sage cannot submit plans to shared engine.**
Mitigation: T5 reduces to observation mode.

**R6: T2 in-memory channel + audit-replay design has subtle ordering bugs.**
Mitigation: at-least-once delivery + idempotent subscriber (atomic complete-or-noop on pending row) is the discipline. Tests in T2 specifically cover this — duplicate event delivery is a tested invariant.

**R7: T3 long-wait test fails — something parks a thread or holds resources.**
Mitigation: this is the test that catches an async-incorrect implementation. If it fails, find what's blocking and fix; don't ship a sync-disguised-as-async architecture.

**R8: T8 reveals flakiness from prior tranches.**
Mitigation: time-boxed fix-and-repeat. Last resort: most stable subset demoed live; remaining as screenshots.

---

## 9. Hand-off to Sonnet

For each tranche, hand Sonnet:

1. This document, scoped to the relevant tranche
2. The pre-locked decisions (§1)
3. The non-goals (§2)
4. The execution conventions (§7)
5. "Report findings at the STOP gate. Do not commit."

Begin with T0. v0.1 and v0.2 superseded — Sonnet replans T0 fresh against this document.

---

## 10. Status tracking

```
Phase 5.5 v0.3 — bpmn-lite demo deployment, async-by-default
T0 ☑  T1 ☑  T2 ☑  T3 ☑  T4 ☑  T5 ☑  T6 ☑  T7 ☐  T8 ☐
Status: T6 complete (2026-05-20) — REST+SSE API (port 8080), DemoAppState, 4 React panels, /bpmn-demo route, Vite proxy; TS typecheck clean, server build clean
```

---

## 11. Async architectural note (for foundational services demo Q&A)

When foundational services ask "how does this work with long-running workflows," the answer is:

> The DSL engine emits typed lifecycle events on every committed execution. bpmn-lite is one subscriber among potentially many. Each invocation from BPMN to the engine is recorded in a pending-call registry keyed by the engine's `execution_id`. When the lifecycle event arrives — milliseconds later for fast verbs, hours later for human tasks, days later for external system callbacks — the subscriber looks up the pending invocation, binds the resolved values into the process variable scope, and advances the workflow. The executor never blocks. Process state is durable in Postgres at every step. If the runtime restarts mid-invocation, the subscriber on startup replays from the audit log; no invocations are lost. The same code path handles a 50-millisecond DMN evaluation and a 14-day user task. There is no separate "long-running mode" — the architecture is async at the mechanism level; the demo just happens to exercise the fast case.

This is the architectural story. It is the differentiator. Deliver it confidently.

End of Phase 5.5 plan v0.3.
