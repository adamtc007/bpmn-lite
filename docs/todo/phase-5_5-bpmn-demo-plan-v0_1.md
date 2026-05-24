# Phase 5.5 Plan: bpmn-lite Demo Deployment

| Field | Value |
| --- | --- |
| Document ID | OB-POC-PHASE-5_5-PLAN-001 |
| Version | v0.2 |
| Status | DRAFT — execution plan for foundational services demo |
| Author | Adam Cearns |
| Date | 2026-05-20 |
| Position | Post-Phase 5 (`b7c5e5f1`); demo-driven; not Phase 6 proper |
| Repo | bpmn-lite (separate from ob-poc); shared DSL engine consumed as workspace dependency |
| Deployment | Existing Docker; bpmn-lite + shared engine + ob-poc UI repointed |

---

## 0. Goal

Foundational services demo: end-to-end pre-coded BPMN process running on shared Phase 5 DSL engine, with DMN decisions invoked inline, Sage agentic integration visible, all surfaced through ob-poc UI repointed at bpmn-lite app.

Demo runs cleanly 5× in a row from a scripted flow.

## 1. Pre-locked decisions

These are decided. No further discussion in tranches.

1. **Condition language** for BPMN sequence flows = ob-poc s-expression DSL, predicate subset.
2. **Binding convention** = service task `id` matches verb id in SemOS catalogue (exact match). `name` is human label only. Unresolved `id` = compile error.
3. **Demo BPMN model** = pre-coded in Rust (Stage 1 only). XML loader deferred to Stage 2 / Phase 5.6.
4. **Gateways in demo** = sequential + exclusive gateways only. No parallel / inclusive. Keeps Phase 6 coordination primitives deferred.
5. **No Phase 6 architectural work** in this plan: no multi-worker pool, no lanes, no admission controller, no cancellation scopes, no fan-out/WaitN.
6. **STOP gates** between every tranche. Sonnet reports; Adam reviews diff; Adam approves commit.

## 2. Non-goals (explicit)

To prevent Sonnet drift:

- BPMN XML parser/loader (Stage 2 — not this plan)
- Parallel or inclusive gateways
- Boundary events, event sub-processes, multi-instance markers, compensation
- Timer scheduler, message correlator
- Multi-worker pool, lanes, admission controller
- Plan persistence (Q9), cross-snapshot replay (Q12), audit retention (Q15)
- Verb catalogue extensions beyond what the demo BPMN model invokes
- Any rework of Phase 5 engine internals — engine is closed at `b7c5e5f1`

If Sonnet's plan touches any of the above, STOP and report — do not implement.

## 3. Demo BPMN model (locked, v0.2)

**Luxembourg SICAV onboarding — sequential, no gateways:**

```
[Start]
   ↓
[Service: cbu.ensure]                 ← create/upsert CBU (Lux SICAV, FUND_MANDATE)
   ↓
[Service: cbu.add-product]            ← subscribe CBU to CUSTODY product
   ↓
[Service: instrument-matrix.attach]   ← bootstrap instrument matrix (draft trading profile)
   ↓
[End: Onboarded]
```

3 service tasks, 0 gateways, 1 end state. All three verbs confirmed present in the SemOS catalogue. `instrument-matrix.attach` is a new verb added in T0 execution (2026-05-20): creates a DRAFT trading profile idempotently.

KYC and UBO branches removed. KYC work is handled by the UBO workspace separately; not in scope for this demo.

**Catalogue FQN mapping (verified):**

| Demo step | Catalogue FQN | Status |
|-----------|---------------|--------|
| Create CBU | `cbu.ensure` | ✅ existing |
| Add product | `cbu.add-product` | ✅ existing |
| Attach IM | `instrument-matrix.attach` | ✅ new verb added 2026-05-20 |

## 4. Tranches

### T0 — Pre-flight audit (NO CODE)

**Goal:** establish ground truth before any execution.

**Sonnet tasks:**
1. Inventory bpmn-lite repo: directory structure, Cargo.toml, dependencies, existing crates
2. Inventory existing BPMN parser: what types it produces (raw XML / typed AST), what test pack covers, what's wired
3. Inventory shared DSL engine dependency: is bpmn-lite already declaring ob-poc as workspace dep? If not, what's required to wire it?
4. Inventory existing bpmn-lite runtime glue: process instance state? Service task dispatch? Sequence flow eval? What's there, what's not?
5. Inventory Docker deployment: what services run, how they connect, where bpmn-lite sits
6. Inventory ob-poc UI: how it currently talks to ob-poc; what would change to repoint at bpmn-lite
7. Inventory SemOS verb catalogue for the verbs in §3 (existence check): `ensure_cbu`, `kyc.initiate`, `ubo.resolve`, `attach_instrument_matrix`, `add_product_subscription`. Are they all present with appropriate effect classes? Any missing or signature-mismatched?
8. Inventory DMN decisions: do `kyc_screening` and `ubo_completeness` decisions exist? If not, what's needed to add them?

**DoD:** structured findings report. No code changes. Adam reviews; identifies gaps that need filling before T1.

**STOP gate.**

---

### T1 — Wire bpmn-lite to shared engine + condition language

**Goal:** bpmn-lite can submit plans to the shared DSL engine and resolve verbs through the catalogue.

**Sonnet tasks:**
1. Add ob-poc as workspace dependency in bpmn-lite (if not present)
2. Implement verb resolution from BPMN service task `id` → catalogue lookup → verb invocation plan
3. Implement s-expression predicate subset parser for sequence flow conditions (reuse ob-poc DSL grammar; restrict to boolean-producing expressions)
4. Implement condition evaluation against process instance variable scope
5. Tests: catalogue resolution success/failure paths; predicate parser; predicate evaluation

**DoD:** unit tests pass; bpmn-lite can resolve a verb id to a catalogue entry and submit a plan against the shared engine; predicate parser handles the conditions §3 will use (`= "clear"`, `= "complete"`, etc.).

**STOP gate.**

---

### T2 — BPMN AST → Populated Execution DAG lowering

**Goal:** typed BPMN process value → Populated Execution DAG submittable to the Phase 5 engine.

**Sonnet tasks:**
1. Define typed BPMN AST if not already present from existing parser (`BpmnProcess`, `BpmnNode { ServiceTask, BusinessRuleTask, ExclusiveGateway, StartEvent, EndEvent }`, `BpmnFlow { source, target, condition: Option<Predicate> }`)
2. Lowering pass: walk BPMN AST, emit Populated Execution DAG with correct edge types from Phase 5 (per T02 commit `493473da`)
3. Service tasks → verb invocation DAG nodes; business rule tasks → DMN evaluation DAG nodes; gateways → branching DAG nodes; sequence flows → DAG edges with predicate guards
4. Resolve service task `id` → catalogue entry → effect class → ExecutablePlan node
5. Tests: lowering produces valid DAG for §3 model; DAG passes Phase 5 validator

**DoD:** can take a typed `BpmnProcess` value and produce a valid Populated Execution DAG that compiles to an ExecutablePlan accepted by the shared engine.

**STOP gate.**

---

### T3 — BPMN runtime glue

**Goal:** process instances run end-to-end through the shared engine.

**Sonnet tasks:**
1. Process instance state machine: `process_instance` table; `current_node`, `variables`, `status`, `started_at`, `completed_at`
2. Process advancement loop: on node completion, evaluate outgoing flows, advance to next node, submit next plan
3. Service task dispatch: submit verb invocation plan to shared engine; await outcome; route based on `Committed` / `OptimisticConflict` / `VerbFailed`
4. Business rule task dispatch: invoke DMN decision via verb; capture result into process variable scope
5. Gateway evaluation: evaluate outgoing flow conditions; pick matching flow; error if none match (or default flow handling per BPMN spec)
6. End event handling: mark process instance complete with end state
7. Persistence: process instance state durable in Postgres; survives runtime restart
8. Tests: full §3 model runs end-to-end with mocked verb responses; reaches all four end states correctly

**DoD:** §3 model runs end-to-end against the shared engine; each of the four end states reachable via test fixtures driving DMN outcomes.

**STOP gate.**

---

### T4 — Pre-coded demo BPMN model

**Goal:** §3 model exists as a constructible Rust value, ready to invoke.

**Sonnet tasks:**
1. Constructor function `fn custody_onboarding_process() -> BpmnProcess` building the §3 model in code
2. Test fixtures: realistic CBU input data, KYC entity, UBO data
3. Demo seed: SQL or Rust-side fixtures populating catalogue with required verb mappings and DMN decisions (if T0 surfaced gaps)
4. Integration test: invoke `custody_onboarding_process()`, lower to plan, run through engine, assert end state reached for each KYC × UBO combination

**DoD:** demo process is one function call away; integration test runs the full pipeline; all four end states verified.

**STOP gate.**

---

### T5 — Sage agentic integration

**Goal:** Sage participates in the running BPMN process; reasoning visible.

**Sonnet tasks:**
1. Sage integration point: one of the service tasks (suggest `kyc.initiate` or `ubo.resolve`) invokes Sage to make a decision rather than calling a fixed verb
2. Sage reads process instance state from shared catalogue
3. Sage walks Semantic Dependency Graph to determine next-step options
4. Sage submits chosen plan against shared engine
5. Sage reasoning recorded in audit trail (visible to UI via T6)
6. Tests: Sage-mediated service task completes process; reasoning captured

**DoD:** at least one service task in the demo flow goes through Sage; Sage reasoning persisted in audit trail; process completes normally.

**STOP gate.**

---

### T6 — ob-poc UI repointing

**Goal:** existing ob-poc UI displays bpmn-lite process state, plan submissions, Sage reasoning.

**Sonnet tasks:**
1. Configure UI to talk to bpmn-lite API endpoints (in addition to or replacing ob-poc endpoints)
2. Add BPMN process visualisation: current node highlighted, completed nodes marked, gateway routing visible
3. Add plan submission feed: each plan submitted to engine appears in real-time
4. Add Sage reasoning panel: when Sage makes a decision, reasoning visible
5. Add DMN decision results panel: when business rule task fires, decision + inputs + output visible
6. Tests: UI manual walkthrough of demo flow; each demo beat displays correctly

**DoD:** UI shows the demo process running; all four UI panels (process, plans, Sage, DMN) populate correctly in real time.

**STOP gate.**

---

### T7 — Docker deployment integration

**Goal:** entire stack runs in existing Docker deployment with one command.

**Sonnet tasks:**
1. bpmn-lite Docker image builds with shared engine dependency
2. docker-compose (or equivalent) brings up: Postgres, shared engine, bpmn-lite, ob-poc UI
3. Service discovery: UI knows where bpmn-lite is, bpmn-lite knows where Postgres is, etc.
4. Health checks pass on all services
5. Single-command demo start: `docker-compose up` brings the whole stack live and ready
6. Reset script: returns to clean demo-ready state (fresh DB, fresh process instances)

**DoD:** `docker-compose up` brings stack live; `./demo-reset.sh` returns to clean state; ready to run demo flow.

**STOP gate.**

---

### T8 — Demo polish + rehearsal

**Goal:** demo runs cleanly 5× in a row.

**Sonnet tasks:**
1. Scripted demo flow: ordered list of user actions producing the demo narrative
2. Speaker notes: what to say at each step; what to point at in the UI; expected outcomes
3. Failure recovery: documented path for each plausible failure mode (verb fails, DMN times out, Sage hangs, UI desyncs)
4. Run demo 5× consecutively: capture any flakiness; fix; repeat until stable
5. Demo data variations: 2-3 different input CBUs producing different end states, to show the gateways routing

**DoD:** 5 consecutive clean demo runs; speaker notes complete; failure recovery documented.

**STOP gate. Demo ready.**

---

## 5. Master Demo DoD

The plan is complete when all of the following are true simultaneously:

1. Pre-coded BPMN process (§3 model) runs end-to-end through shared Phase 5 engine
2. All four end states reachable via input data variation
3. Both DMN decisions invoked successfully during the run
4. At least one service task goes through Sage with visible reasoning
5. ob-poc UI shows process state, plan feed, Sage reasoning, DMN results in real time
6. Entire stack runs via `docker-compose up` from clean state
7. Demo runs cleanly 5× consecutively from scripted flow
8. Speaker notes complete; failure recovery documented

## 6. Tranche dependency graph

```
T0 (audit)
  ↓
T1 (engine wiring + condition lang)
  ↓
T2 (lowering)
  ↓
T3 (runtime glue)
  ↓
T4 (demo model constructible)
  ├─→ T5 (Sage integration)
  └─→ T6 (UI repointing)
        ↓
       T7 (Docker)
        ↓
       T8 (polish + rehearsal)
```

T5 and T6 are independent of each other; can run in either order after T4. Everything else is strictly sequential.

## 7. Execution conventions

Per established Claude Code conventions:

- **One tranche per session.** Sonnet completes the tranche, reports, stops at the STOP gate.
- **No commits without review.** Sonnet does not commit. Adam reviews diff, approves, commits separately.
- **Progress markers.** Sonnet reports % complete and current sub-step at each major step within a tranche.
- **Explicit "do not improvise" posture.** If Sonnet hits something outside the tranche scope, it stops and reports — does not extend the tranche on its own.
- **Phase 5 engine is closed.** Sonnet does not modify ob-poc engine code from `b7c5e5f1`. If something appears broken in the engine, STOP — Adam decides.

## 8. Risk register

**R1: T0 reveals catalogue gaps that block the demo model.**
Mitigation: T0 surfaces them up front; T1 can include catalogue extensions in scope if needed. If extensions are large, demo model swaps to a different set of verbs known to exist.

**R2: Existing BPMN parser produces something different from what T2 expects.**
Mitigation: T0 reports parser output shape; T2 adapts to it. Worst case: T2 includes a thin adapter.

**R3: Sage integration in T5 reveals Sage doesn't yet submit plans to the shared engine.**
Mitigation: T5 scope reduces to "observation mode" — Sage reads process state and presents reasoning, doesn't submit plans. Less wow factor but still tells the agentic story.

**R4: Docker deployment integration in T7 hits networking / service discovery issues.**
Mitigation: existing Docker deployment is the starting point; T7 extends rather than rebuilds. If extension breaks, fallback is `cargo run` against local Postgres for the demo.

**R5: Demo rehearsal in T8 reveals flakiness Sonnet introduced.**
Mitigation: time-boxed fix-and-repeat; if 5-consecutive-clean cannot be achieved, demo runs with the most stable subset and remaining variations are screenshots.

## 9. What you hand to Sonnet

For each tranche:

1. This document, scoped to the relevant tranche (T0, T1, etc.)
2. The pre-locked decisions (§1)
3. The non-goals (§2)
4. The execution conventions (§7)
5. "Report findings at the STOP gate. Do not commit."

Open Sonnet, paste T0, let it audit. Review findings. Then T1, then T2, and so on.

## 10. Status tracking

Suggest a single tracker line you keep updated:

```
Phase 5.5 — bpmn-lite demo deployment
T0 ☐  T1 ☐  T2 ☐  T3 ☐  T4 ☐  T5 ☐  T6 ☐  T7 ☐  T8 ☐
Status: pre-execution; awaiting T0
```

Tick each as Sonnet completes and you commit.

End of Phase 5.5 plan v0.1.
