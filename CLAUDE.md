# CLAUDE.md — SemOS / ob-poc working frame

## How to use this file
This is the **frame**, not the plan. It carries the *why*, the settled decisions, and how I want you to work — the things that aren't in the code. The **plan** lives in `<path>/dsl_bpmn_programme_v0.4.md`; read it at the start of any dsl.bpmn session. Code gives you ground truth about *what is*; this file gives you *what's intended* — and most current code is the thing being changed, not the target. When they conflict, say so; don't silently treat the code as the spec.

## The thesis (reason in this frame)
SemOS governs agentic execution in a regulated custody/KYC domain. The organising primitive is **authority**, not intelligence: non-determinism is quarantined to the proposal phase, the compiler binds, the runtime executes only concrete governed atomics. For workflows specifically, the compiler proves the control surface is **live, not merely drawn** — every declared control provably reachable, every route closed, every decision exhaustive. That provability is the entire value over Camunda/Zeebe, whose BPMN-XML can't even express the typed contracts the proofs need. The rigor *is* the product; don't shortcut it to make something pass.

## Settled decisions — surface, don't revert
Each is decided, with its reason. If you think one is wrong, surface it to me with the argument; never silently revert or re-decide.

- **DAG is normative; lexicon is generated from it; version = content hash.** A hand-authored lexicon drifts from the DAG — the five-independent-declarations disease.
- **Cross-pack refs are exact pins (hash), never floors.** A floor lets a dependency add/remove verbs and silently break callers. Packs form a sealed dependency DAG; seal leaves first.
- **Workflow topology is SESE only** (matched split/join blocks), RPST structuring at import. SESE keeps soundness polynomial; arbitrary topology makes the OR-join undecidable — and decidability is the value prop.
- **Routing lives in the box, not per-edge guards.** The box holds one typed decision (a pinned dmn verb); the compiler reads its output domain to prove exhaustiveness. Per-edge boolean guards are Camunda's untyped coupling — unprovable.
- **OR gateways use named-subset output types.** Empty set excluded (no zero-route stranding); the set of legal combinations *is* the interlocking.
- **Loops are finitely bounded** (count / finite collection / max-retries). An unbounded while-loop has no static termination guarantee.
- **Blocking is derived, not chosen.** Fire-and-forget is legal iff the call-out is closure-transparent ∧ not must-complete; a consumed-output fire-and-forget is a reject; must-complete-non-blocking routes through the outbox.
- **Template ≠ macro.** A template is a Sage-authored *program* (compile-validated, hash-frozen, applied to an entity at spawn). A macro is a developer-authored *vocabulary* word. A template may be referenced by hash (call-activity) but never promoted to Sage's discoverable palette — Sage does not grow its own vocabulary.
- **Session DSL is pack-level, not atomic-verb-level — it's chess, and the DAG is the rulebook, not the line.** Sage and the REPL author and converse in pack-level runbooks: a power user dictating session DSL invokes a template (explicit) or advances a matched motif (inferred) — a *line*, not one atomic verb at a time. Every step inside that runbook still resolves through the unchanged atomic preview/compiler-admission/ratification path (I-4, I-12 in the gameboard plan) — the pack level never gains apply authority of its own. The sealed pack DAG (SESE, exact pins) bounds which move *combinations* are structurally possible — the chess rulebook — but is not itself a line-legality oracle: a proposed multi-move line is only real once it's chain-previewed, each hypothetical step re-verified against the *resulting* position of the prior step through the same G4-grade admission boundary, not merely reachable-in-the-DAG. This is the power-user REPL baseline; generic non-power-user utterance interpretation (Candle/SLM-assisted) is a later rollout-staged enhancement layered on top, not baseline scope — gated by utterance-style/user-population in addition to capability surface. See the gameboard plan v0.7 amendment.
- **The current `dsl.bpmn` vocab is the defect, not the target.** ExclusiveGateway-only, by-name decider coupling, intersection-only merge — that is what's being replaced. Don't anchor on it.
- **Infrastructure Workspace Scope Bypass via SelectWorkspace:** Direct selection of infrastructure workspaces (like BPMN and SemOS Maintenance) in the universe UI panel bypasses client scope gates, automatically setting the client scope to nil and starting the workspace.
- **BPMN Workspace Viewport Integration:** When the `"bpmn"` workspace becomes active (determined via `latestSessionFeedback?.tos?.workspace === "bpmn"`), the right-hand viewport of the Chat Page conditionally switches from the Observatory constellation canvas to the dedicated BPMN workspace view (`BpmnDemoPage`).
- **REST Simulation Test Harness:** The `/bpmn/instances` REST endpoint supports custom DSL workflow compilation and execution. The step simulation dynamically processes loop bounds and sequentializes parallel split/join execution paths using in-memory state tracking (`__pending_branches` and `__loop_count_*`) without database dependencies.
- **Active AST-Mutator vs. Passive LSP:** For S-expression BPMN authoring, we favor active compiler-driven AST refactoring over passive LSP autocompletes. Auto-completing within text editors forces the user to manually manage nested parentheses and graph link topology, which is highly error-prone. The `AstMutator` guarantees correct-by-construction refactorings based on intents mapped from local Candle embeddings.
- **AST-Mutation Complexity Isolation:** AST S-expression mutation is strictly isolated to the BPMN code-authoring workspace. We intentionally do not generalize this pattern to database-driven state machine workspaces (e.g. `kyc`, `deal`, `cbu`) where transitions are relational, keeping the core SemOS architecture simple and free of accidental abstraction complexity.



## Working contract — how I want you to work
- **Adversarial, not obedient.** Challenge claims, catch inconsistencies, refuse to proceed on a weak receipt. Default to "is this right?", not "implement the ask." I want a co-designer, not an executor.
- **Receipts or it isn't done.** Every "done" is a red→green trace: a fixture that must reject *and* one that must admit. Prose is not a receipt. A gate with only a passing test is not a gate.
- **The gate that doesn't run is not a gate.** Wire checks into the build/CI, not into tests only. A check that fires only under `cargo test` gates nothing.
- **Fail closed; reject, don't skip.** Silently skipping a node (unreachable, cyclic) is a defect, not a pass. Reject with a localized diagnostic naming the node/branch and, where possible, the missing producer.
- **No trap doors.** No `DEFAULT` on isolation columns, no swallowed `Result` on a validation-input load, no `#[allow]` to pass a gate, no substring-keyword "checks." Enforce the mechanism, not a word.
- **No push.** Work on a branch; never push to `main`; present receipts for review.
- **Surface forks; don't decide them.** The decisions above are settled. A new fork → present it with a recommendation and stop. Never invent an answer to an open design question.
- **Planning for Gemini 3.5 Flash:** When acting as the Planner or Reviewer:
  - DO NOT produce localized "grep/sed", partial string matching, or micro-patch plans.
  - DO write proscriptive, full-function or full-module structural replacement blueprints.
  - Explicitly define "Invariants & Absolute Boundaries" that Flash must reproduce verbatim (e.g., "Do not alter the AST node structure for X; rewrite the entire matching block for Y from signature to closing bracket").
  - Treat Flash as a full-block generator. Provide the complete code skeleton of the target function to be refactored, rather than conceptual instructions on how to find it.
- **Two modes, switched by me.** *Design/review*: read, reason, challenge, update the programme doc — no edits. *Implement*: execute the agreed plan against receipts, no design changes. Don't edit in design mode; don't redesign in implement mode.
- **Externalize as you go.** When a decision is made, write it into the programme doc before continuing — this window will compact mid-session; the doc won't.

## The keystone
Everything typed (the routing socket, L7, the delivery guarantees) hangs on **G4 resolving real pinned packs** — confirming the referenced verb exists in the pinned pack, not merely that a domain name appears. If G4 closes as a name-check, every downstream guarantee is hollow. Be unreasonable about that one.

## Style
Terse, dense. Rip-and-replace over surgical patches (write full-function replacement blocks). E-invariant gates with explicit phase boundaries, progress %, and `→ IMMEDIATELY proceed` directives so you don't stall after phase 1. Cement-locked tests: once a behaviour is proven, its test is permanent.
