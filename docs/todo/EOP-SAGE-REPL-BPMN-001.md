# EOP-SAGE-REPL-BPMN-001 — Sage↔REPL BPMN designer session programme v0.1

Status: **RATIFIED in outline (Adam, 2026-07-27: "crack on") — tranche
receipts to be appended in place as each closes.**

Goal: a standalone-deployable BPMN-Lite — Sage↔REPL AI-designer session
+ SemOS pack + engine — independent of the ob-poc main application, with
a server-built DAG rendered in a designer UI window, accepted by a
Chrome-MCP test harness running MULTIPLE concurrent design-review-save
sessions.

## T0 — Ratifications recorded (closed at drafting)

- **Parallel frontends RATIFIED** (implicit in the brief: the Sage↔REPL
  session is DSL-native end to end; XML stays a dormant import/export
  compatibility path, round-trip fidelity already cemented under F3).
- **Keyword-gate is the v1 REPL intent contract**; the embedding-driven
  mapping (Candle, per the settled AST-mutator decision) is a later
  upgrade that must not reshape the session loop.
- **Designer UI is STANDALONE** (served by/beside bpmn-lite-server),
  not an ob-poc panel.
- **WASM is NEXT PHASE** (Adam, 2026-07-27): wasm build/deploy of the
  server runtime engine, browser-wasm designer session, WASM-3.0
  docker-style deployment — all deferred until this phase works on
  current Rust patterns. Carried constraint: do not foreclose it —
  kernel stays pure/no-I/O; no gratuitous non-wasm-able dependencies in
  core crates; server-only concerns stay in server crates.
- **Dual taxonomy: STRUCK (Adam, 2026-07-27)** — unattributed legacy requirement, disowned on review; removed from the open-items list (was "deferred").

## T1 — Designer-session aggregate + persistence

The audit's hard gap: templates persist but the design SESSION does not
(dialogue state, in-progress DSL source/AST, undo history). Multi-session
concurrency is a requirement (N designers, isolated).

- Session aggregate: id, name, current DSL source, revision history
  (undo = revision walk, append-only), dialogue log (utterance +
  response + applied mutation), status (draft/reviewed/saved-as-template
  + template hash when saved).
- Store surface: MemoryStore + Postgres impls behind the existing store
  trait discipline (single write path, tenant-scoped).
- Endpoints: create / get / list / append-revision / append-utterance /
  save-as-template (delegates to the existing template store; template
  hash recorded back onto the session).
- Receipts: session round-trip + revision-walk undo test; two-session
  isolation test (mutations to A never visible in B); save-as-template
  end-to-end (session → template → plan compiles).

**T1 CLOSED 2026-07-27** (commits `fce546e` slice a, `e9f53e0` slice b):
- Aggregate + store surface: `DesignSessionRecord` (append-only event
  log, `current_source()` = last Revision, undo = revision walk) behind
  5 `AdminProjectionStore` methods; MemoryStore full impl + Postgres
  impl (`store_postgres.rs`, migration `059_design_sessions.sql`,
  events JSONB, RLS alignment deferred to T5 — noted in migration
  header).
- Endpoints (demo router): create / list / get / revision / utterance /
  save. Revision records ALWAYS (drafts may be broken) and returns
  compile diagnostics; save is fail-closed (uncompilable draft → 400 +
  diagnostics, catalog untouched) and pins `(name, version, plan_hash)`
  back onto the session.
- Receipts green: store-level `session_round_trip_and_undo_walk`,
  `concurrent_sessions_are_isolated`,
  `save_marks_pin_and_duplicate_create_rejects`; HTTP-level
  `test_design_session_round_trip_and_save` (end-to-end save v1 +
  Saved status + pin), `test_design_session_save_rejects_uncompilable_
  draft` (must-reject red), `test_design_session_unknown_id_is_404`.
- Postgres impl is compile-verified only (no live PG in this loop);
  its behavioural parity rides the T5 standalone-boot receipt.

## Programme reconciliation with EOP-PLAN-BPMN-DESIGN-003 v0.2 (ruled by Adam, 2026-07-27)

The two programmes claimed the same surface; the ruling merges them:

- **T1's sessions are WS-B's persistence substrate.** The session
  aggregate/store/endpoints (closed above) are what `designer-ui`
  builds on — not a parallel mechanism.
- **T4 (graph endpoint + designer UI) MERGES into WS-B** of
  EOP-PLAN-BPMN-DESIGN-003; it is no longer independently sequenced
  here. The WS-B day-one rule applies: its disposition path calls
  WS-C's deterministic disposition policy function from the first
  commit.
- **The keyword gate is DEMOTED**: T0's "keyword-gate is the v1 REPL
  intent contract" ratification is superseded by DESIGN-003 v0.6's
  canonical chain (board → tier-0 → disposition policy). The gate
  (`utterance_intent`, rest.rs) survives only as an *interim evidence
  producer* behind WS-C's disposition function, retired when tier-0
  matcher wiring lands. WS-C item 3/4 implies widening
  `DesignSessionEventKind::Utterance` toward the I28 record shape
  (board hash, scores, disposition closure).
- **T2 (macro hardening), T3 (pack seal), T5 (crate-split review),
  T6 (Chrome-MCP harness) stand unchanged** — they don't overlap
  WS-A/B/C content. T5's split review now also covers the three
  designer crates (GOV.2: bpmn-lite workspace, exact rev-pin
  consumption — pending Adam's confirm).

## T2 — Design-loop hardening (the never-reviewed layer)

- Authorship-blind review of `dsl/macros.rs`, `dsl/refactor.rs`
  (AstMutator), and the five authoring endpoints in `rest.rs` (CAREFUL
  tier — this layer landed in one commit, one review pass, one test).
- Tests for `plan.rs` and `ast.rs` (currently ZERO inline tests).
- Fuzz target `macro_apply` (bpmn-lite-compiler/fuzz): tape → source +
  macro/mutation choice → AstMutator → `to_sexpr` → re-`compile`.
  Oracle M-O1 no-panic; M-O2 correct-by-construction: every mutation
  output re-compiles (that is the AstMutator's core claim — make it an
  oracle, not a hope). Seeds from the macro fixtures.
- Receipts: review findings closed with red→green traces; fuzz target
  smoked clean into the fleet.

## T3 — Pack seal loop (the deployment artifact)

The SemOS pack becomes the deployable vocabulary unit, so the seal must
be live, not build-time theater:
- Runtime registry construction (`ManifestPlaceholderRegistry` sources)
  loads a SEALED pack closure and hash-verifies it (exact pin — G4
  discipline applied to the BPMN vocabulary itself); loose-YAML loading
  survives only behind an explicit dev flag.
- `xtask pack-build` output becomes the thing the server consumes.
- Receipts: tampered-pack red (bit-flipped closure → refuse to serve) +
  pinned-pack green; server boots from a sealed pack in the standalone
  profile.

## T4 — Server-built DAG graph + standalone designer UI

Render contract (Adam): the server builds the DAG — nodes, edges,
layout — the UI is a window. Assembly, mostly not invention:
`dsl::compile` → plan → `ir`/`dto` (`WorkflowGraphDto{nodes,edges}`) +
`topo_layout` (already computes coordinates for BPMN DI export).
- Endpoint: `GET /api/dsl/sessions/:id/graph` → graph DTO + layout for
  the session's CURRENT revision (recompiled server-side; compile errors
  surface as diagnostics, not a blank canvas).
- Standalone designer page (served static from bpmn-lite-server):
  REPL pane (utterance in, Sage/diagnostic feed out) + graph window
  (Camunda-8-editor-like rendering of the served layout) + session list
  + save-as-template action. Framework-light; it is a window, not an
  editor — all mutation goes through the REPL/macro endpoints.
- Receipts: graph endpoint golden test (known session → known
  nodes/edges/layout); UI smoke (page loads, renders a session graph).

## T5 — Deployment / crate-split review (native, this phase)

Design/review tranche — split proposal presented BEFORE implementation:
- Standalone deploy unit: bpmn-lite-server (designer endpoints + engine
  + sealed pack + static designer UI) with a `designer` build profile;
  prove ob-poc independence (no ob-poc references; runs against
  MemoryStore for demo and Postgres for durable).
- Audit crate edges for the split: what the designer session needs
  (compiler dsl, authoring dto/layout, store, engine-sim) vs what the
  runtime engine needs; feature-gate anything crossing.
- WASM-readiness NOTED per crate (pure/no-I/O vs tokio/net/pg) as an
  inventory only — implementation is next phase.
- Receipts: `cargo build` matrix for the profiles; a from-scratch
  standalone boot script (seal pack → boot server → create session →
  save template → spawn instance in simulator).

## T6 — Chrome-MCP multi-session acceptance harness

The phase gate: scripted browser sessions via the chrome-devtools MCP
driving the REAL standalone deployment:
- Script: open designer → create session → utter design steps (keyword
  gate) → macro-apply → diagnostics → graph render ASSERTED (nodes
  visible, layout sane) → review → save template → spawn in simulator →
  verify instance steps.
- MULTIPLE concurrent sessions (≥3 pages) with isolation assertions
  (A's graph never shows B's nodes; saves don't cross).
- Receipts: harness script in-repo (re-runnable), screenshots +
  assertion log per run; failure of any assertion is a finding, not a
  flake, per the usual discipline.

## Sequencing

T1 → T2 and T3 in either order (independent) → T4 (needs T1 sessions)
→ T5 (needs T3 pack + T4 UI to define the deploy unit) → T6 (needs all).
Commit+push per verified step; blind review at T2 and before T6 close;
receipts appended here in place.

---
*v0.1 drafted 2026-07-27 from EOP-DSL-AUTHORING-AUDIT-001 §4 + Adam's
deployment/UI/harness brief. Amend in place.*
