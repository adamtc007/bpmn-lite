# EOP-PLAN-GRAPH-DSL-BRIDGE-001 — B3 receipt

Baseline: Gate B2 accepted at `febdf5e` (branch
`codex/bpmn-gameboard-refactor`). **Tier: CAREFUL. Default: smallest
possible surface.** This is the plan's final tranche.

- **Scope delivered:** all three B3 work items — the read-only receipt
  endpoint, router-level tests for both response shapes plus
  non-mutation, and the boundary gates (no public-api baseline change
  needed — see below).

## Work item 1 — the endpoint

`GET /api/dsl/sessions/:id/dsl-receipt`
(`bpmn-lite-server-designer/src/rest.rs`, `session_dsl_receipt_endpoint`,
registered beside the runbook route). Response is always one of:

- `404` — unknown session (same shape as every sibling endpoint).
- `200` with `{ source, graph_state_hash, required_symbols,
  refused: null }` — the canonical DSL source (B1's emission, B2-proven
  recompile-equivalent), the content-derived identity witness, and the
  symbols the equivalence registry must declare.
- `200` with `{ source: null, …, refused: { stage, diagnostic } }` — a
  refusal is a valid, honest receipt response, never a 500. Stages:
  `session` (not graph-backed — its DSL text is authoritative already),
  `reconstruction`, `admission` (the receipt describes the ADMITTED
  artifact only — same discipline as the sibling graph endpoint),
  `emission` (a construct with no DSL surface, named in the diagnostic —
  the DSL-parity programme's backlog input).

Pipeline: `load_design_session` → `is_graph_backed` →
`reconstruct_designer_dag` → `dag.admit()` → `dag.emit_dsl(session_id)`.
Read-only on every path — no store write exists in the handler.

**Endpoint-shape decision (deferred to B3 start by the plan, decided
here, for ratification): `workflow_id` is the session UUID, not
`session.name`.** The name is free text (the seed helper's own is
"proposal session" — a space), which would refuse `UnrepresentableToken`
for essentially every real session; the UUID is the session's stable
identity and always lexes as a DSL Symbol token. Note the sibling graph
endpoint passes `session.name` to `project_ir` as its workflow id — a
pre-existing inconsistency this tranche does not touch; if the receipt's
plan-equality obligation is ever checked live against that endpoint's
projection, the ids must be aligned first (flagged, not fixed).

## Work item 2 — router tests (3, all through the public router only)

- `test_dsl_receipt_green_recompiles_and_never_mutates` — green session:
  `refused` null, source contains the seeded node, hash non-empty;
  **recompiled in-test** through `dsl::compile` with the derived
  empty-bindings registry built from `required_symbols` (the B0/B2
  contract, exercised end-to-end through HTTP); deterministic across two
  calls; graph endpoint's `source_hash` identical before/after (the
  established non-mutation pattern).
- `test_dsl_receipt_refuses_guard_graph_with_named_diagnostic` — a real
  guard attached via `graph-edit` (BoundaryTimer + escape End): `200`,
  `refused.stage == "emission"`, diagnostic names the guard node,
  `source` null, graph untouched. **Finding recorded:** the refusal is
  `UnreachableNode` naming `'timeout'`, not `UnsupportedNode
  (BoundaryTimer)` — a boundary node attaches via `attached_to`, not
  sequence flow, so the emitter's Stage-0 reachability pre-check fires
  first, per the frozen refusal ordering. Fail-closed and node-named
  either way; the first draft of this test assumed the per-node
  diagnostic and was corrected to the actual (correct) contract rather
  than reordering the frozen checks to flatter the test.
- `test_dsl_receipt_not_found_and_non_graph_backed` — 404 for unknown;
  `refused.stage == "session"` for a session with no graph edits.

## Work item 3 — gates

- **Public API diff: none.** The endpoint is internal routing; the
  crate's public surface remains exactly `DesignerState` +
  `designer_router`. `python3 scripts/check-semantic-gameboard-boundaries.py`:
  pass, no baseline change needed (verified, not assumed — the gate
  recomputes all surfaces).
- `python3 scripts/check-test-only-pub.py`: `ok: 0`.

## Verification

- `cargo test -p bpmn-lite-server-designer --lib`: **97 passed / 0
  failed, 1 ignored** (94 prior + 3 new; the ignored test is U3's
  benchmark, correctly).
- `cargo check --workspace --all-targets`: clean (same 2 pre-existing
  unrelated warnings, both in files this tranche does not touch).
- Boundary gates: both pass (above).

- **Refusal catalogue delta vs B0's frozen list: none.**

- **Known deviations or explicitly parked work:**
  - The `session.name`-vs-UUID workflow-id inconsistency with the
    sibling graph endpoint (work item 1) — flagged for a separate
    ruling, untouched.
  - Boundary-construct refusals surface as `UnreachableNode` rather than
    `UnsupportedNode` (work item 2 finding) — a diagnostic-precision
    note for the DSL-parity programme, not a defect: the frozen Stage-0
    ordering is behaving exactly as ratified.

- **Blind peer-review findings and dispositions:** an independent
  reviewer (no prior context) re-derived every claim: read-only proven by
  inspection down through both store impls (memory read-lock+clone;
  postgres single SELECT), `reconstruct_designer_dag`/`admit`/`emit_dsl`
  all side-effect-free (`&self`, no interior mutability in schema.rs);
  the 500-only-on-store-error claim traced through every path including
  a panic audit of the emitter's one guarded `expect`; the UUID
  workflow-id always lexes (dash is legal as both start and continue
  char — `UnrepresentableToken` on it is unreachable); the
  `session.name` inconsistency in the sibling graph endpoint confirmed
  at its exact line; and — the item most worth attacking — the
  UnreachableNode-before-UnsupportedNode explanation verified at the
  source: `AttachGuard` inserts NO edge (ops.rs — `attached_to_key` is a
  node field; `to_ir` synthesizes no host→boundary edge), so the guard
  node genuinely has no incoming flow edge and Stage-0 refuses it first.
  Gates re-run independently (boundary gate pass, 8 items, identical
  hashes; test-only-pub ok). All test counts reproduced. Verdict: **all
  six review items verified, no blocking findings.** Two scruples:
  1. The guard test's `contains("'timeout'")` was weaker than the
     property (satisfiable by `'timeout_end'` too) — disposed by
     tightening to an exact-prefix match on the full diagnostic.
  2. Same-process determinism doesn't prove cross-process canonicality —
     correct, and that is B2's harness's job, not this endpoint test's.
  The reviewer also confirmed the response shape's two additive deltas
  vs the plan's sketch (`required_symbols`; structured `refused`) are
  supersets of the promised shape, and that deciding the workflow-id at
  B3 was exactly what the plan authorized ("shape decided at B3 start").

- **STOP-gate decision: blocked — awaiting peer review of this receipt.**

Per Gate B3's own text: "Endpoint proven read-only; receipt shows both
response shapes live. Plan closes." Both shapes are live and tested
above; on acceptance, **EOP-PLAN-GRAPH-DSL-BRIDGE-001 closes**, and the
remaining work — the full-parity programme (new AST variants, XOR
oracle, IR-side loop provenance, process-level syntax), seeded with B1's
refusal catalogue — is handed to the separate DSL-parity planning phase
per the fork-A ruling. U4 of the fuzz plan is independently unblockable
since B2's acceptance.
