# EOP-PLAN-GRAPH-DSL-BRIDGE-001 — Narrow-core graph→DSL bridge (staging skeleton)

| Field | Value |
| --- | --- |
| Status | **CLOSED — B0 `2cbd450`, B1 `9d79cb3`, B2 `febdf5e`, B3 `1fca5dd`, all accepted 2026-08-13. Successor: the DSL-parity planning phase (fork-A ruling), seeded by B1's refusal catalogue.** |
| Governing V&S | `EOP-VS-GRAPH-DSL-BRIDGE-001.md` — all seven forks ruled 2026-08-13 |
| Baseline | `54f5dbb`, branch `codex/bpmn-gameboard-refactor` |
| Scope | `emit_dsl`: admitted `IRGraph` + process-level decls → canonical `bpmn-dsl` source for the core-5 kinds; typed refusal for everything else; recompile-equivalence proof; server receipt surface. |
| Execution | One tranche per change set; STOP for review at every gate. Receipts per the house template. |
| Does not authorise | Any DSL grammar/lexer/parser change; any new `NodeAst` variant; the XOR oracle; IR-side loop provenance; DSL→graph import; XML/`NodeDto` path changes; Sage/palette exposure. All of those belong to the separate DSL-parity planning phase (fork-A ruling) or are excluded outright (V&S §4). |

---

## 0. Frozen contract (the ruled forks, restated as binding constraints)

1. **Scope (A):** v1 emits exactly the core-5: `Start`, `End` (incl.
   terminate sentinel), `ServiceTask`, `MessageWait`, matched `GatewayAnd`
   pairs. The other 10 `IRNode` kinds refuse. This plan is the **staging
   skeleton only** — the capability is not "done" until the separate
   DSL-parity plan closes (Adam's ruling: "it's simply not done otherwise").
   The refusal catalogue this plan lands is that parity plan's input backlog.
2. **XOR (B):** refused in v1, same posture as `project_ir`. No oracle work
   here.
3. **Canonical order (C):** emission order is topological from the unique
   `Start`, ties broken by BPMN id (lexicographic, byte-wise on the raw id
   string). Matched `GatewayAnd` blocks emit as one `SplitAst` whose flows
   are ordered by outgoing edge id (lexicographic). BPMN ids pass through
   verbatim — no renaming, ever (renaming would break artifact-identity
   equality). Formatting is whatever `ToSexpr` produces; `ToSexpr` is the
   only printer. Any change to these rules after B0 closes is a version bump
   of the bridge contract, not a patch.
4. **Equivalence relation (D):** plan-level equality —
   `project_ir(to_ir(dag), wf_id)` ≡ `dsl::compile(emitted_source)` under a
   defined `WorkflowExecutionPlan` comparison (B0 freezes exactly which
   fields; see B0 work item 3 — fields that legitimately differ by
   construction, e.g. provenance-ish metadata, must be enumerated and
   excluded *by name*, never by "ignore the rest"). `graph_state_hash`
   (content-derived — NOT the server's route-derived hashes) is recorded
   alongside as the graph-side identity witness.
5. **Loops (E):** unrolled copies emit as plain tasks. Documented in the
   emitted receipt header as "runtime-faithful; loop authoring sugar not
   reconstructed." No fold heuristic, no refusal.
6. **Owner (F):** `bpmn-lite-compiler::dsl`, new module `emit.rs`, sitting
   beside `ir_plan.rs` as the second sanctioned projection off `IRGraph`.
   Entry point:
   `pub fn emit_dsl(ir: &IRGraph, workflow_id: &str, decls: &ProcessLevelDecls) -> Result<EmittedDsl, DslEmitError>`
   — the `decls` parameter is mandatory because `to_ir()` drops
   `default_guard_budget`/`default_retry_policy` (V&S §2.1); a bare-`IRGraph`
   signature would silently lose them. A thin
   `DesignerDag::emit_dsl(&self, workflow_id)` wrapper lives in
   `designer-graph` (which already depends on the compiler) and passes its
   own process-level fields — the wrapper contains no logic beyond field
   plumbing.
7. **Process-level decls (G):** the DSL grammar has no syntax for them
   (verified — parser accepts no process-level attributes). Therefore: a DAG
   with either field **set** refuses (`DslEmitError::ProcessDeclUnrepresentable`,
   naming the field); a DAG with both unset emits. Never silently dropped.

**Fail-closed, always:** every refusal is a typed `DslEmitError` variant
naming the offending node/edge/field and the missing DSL surface. No lossy
encoding, no silent skipping, no `ServiceTask`-faking. A refusal emits no
partial artifact and touches nothing.

---

## 1. Tranche map

```text
B0  Contract freeze + fixtures + prep     spec section, fixture catalogue, ir_plan.rs header fix
B1  emit_dsl + DslEmitError               core-5 emission; full 15-kind + structural refusal catalogue
B2  Round-trip proof harness + CI gate    red/green fixtures; plan-equality; printer/parser identity
B3  Canonical receipt surface             read-only server endpoint; public-api reviewed
```

B0 → B1 → B2 strictly ordered. B3 after B2 (no receipt surface before the
proof gate exists — an unproven receipt is prose, not a receipt). U4 of the
fuzz plan opens only after B2's gate is green and accepted, per its own
precondition.

---

## B0 — Contract freeze, fixtures, prep

**Tier: CAREFUL. Production code changes: one trivial doc-comment fix only.**

### Work

1. Fix `ir_plan.rs`'s stale module header (still lists `MessageWait`/
   `MultiInstance` as unsupported; the code projects both). Doc-comment-only
   commit. Flagged in the V&S; do it first so every later spot-check reads
   true prose.
2. Freeze the `DslEmitError` catalogue on paper before code: one variant per
   unsupported `IRNode` kind (10), plus structural refusals mirroring
   `IrPlanError`'s axes where they apply to emission — at minimum:
   unmatched/malformed `GatewayAnd` pairing, non-`Eq` edge condition,
   condition on an edge whose DSL target can't carry one, multiple `Start`s
   / no `Start` / unreachable nodes, duplicate-id impossibility (defence in
   depth; `DesignerDag` already prevents it), and
   `ProcessDeclUnrepresentable`. Enumerate exhaustively in the receipt; B1
   implements exactly this list — additions found during B1 go back into the
   B0 receipt as amendments, not silently into code.
3. Freeze the plan-equality comparison for fork D: enumerate every
   `WorkflowExecutionPlan`/`ExecutionNode` field, and for each rule
   "compared" or "excluded-by-name-with-reason." The V&S stop condition
   applies: if equality turns out to require comparing constructs
   `project_ir` refuses, the scope was drawn wrong — STOP, return to review.
4. Fixture catalogue: green fixtures (must-emit-and-admit-equivalent) —
   minimum: linear start→task→end; task chain with `MessageWait`;
   matched-And block with 2 and 3 branches; terminate-end; nested And blocks.
   Red fixtures (must-refuse, one per refusal variant) — every `DslEmitError`
   variant from item 2 gets at least one fixture that provably triggers it.
   Fixtures are `DesignerDag`-constructed in test code (same discipline as
   `ir_plan.rs`'s own cement tests), not hand-authored IR dumps.

### Gate B0

Peer review ratifies the frozen error catalogue, the field-by-field equality
definition, and the fixture list. Receipt records all three tables in full.

---

## B1 — `emit_dsl` and the refusal catalogue

**Tier: CAREFUL.**

### Work

1. `bpmn-lite-compiler/src/dsl/emit.rs`: `DslEmitError` (exactly B0's frozen
   catalogue), `ProcessLevelDecls`, `EmittedDsl { source: String, ast: WorkflowSource, graph_state_hash: String }`,
   and `emit_dsl` per §0.6. Internal passes, in order: representability
   check (whole-graph, all errors collected? No — **first refusal wins,
   deterministically**: scan in canonical node order so the same graph
   always yields the same refusal); structure recovery (reuse the existing
   `gateway_pairs` oracle for And-pairing — do not reimplement pairing);
   canonical ordering (§0.3); `NodeAst` construction; text via `ToSexpr`.
2. `DesignerDag::emit_dsl` wrapper in `designer-graph` (field plumbing only).
3. Unit tests in `emit.rs`: every green fixture emits; every red fixture
   refuses with its exact variant (not just `is_err()` — house rule).
4. Module doc comment stating the projection's contract and its relationship
   to `ir_plan.rs` (sibling projection, aligned refusal posture), so the
   next reader doesn't re-derive it.

### Public-surface rule

New public surface is exactly: `emit_dsl`, `DslEmitError`,
`ProcessLevelDecls`, `EmittedDsl`, and the `DesignerDag` wrapper method.
Nothing else widens — no `ir_plan.rs` internals exposed, no `ToSexpr` change.
`cargo public-api` diff must show exactly these additions and be recorded in
the receipt (the workspace-wide gate from H6 will catch drift regardless —
update its baseline in the same commit, per that gate's own discipline).

### Gate B1

All green fixtures emit; all red fixtures hit their named variant; public
API diff is exactly the enumerated additions; workspace check + boundary
gates clean.

---

## B2 — Round-trip proof harness, CI-wired

**Tier: CAREFUL. This is the keystone tranche — the equivalence proof is the
entire value of the bridge. Be unreasonable about it.**

### Work

1. For every green fixture: `emit_dsl` → `dsl::compile(source)` →
   compare against `project_ir(to_ir(dag), wf_id)` under B0's frozen
   field-by-field equality. Also assert `graph_state_hash` matches the
   emitted record's witness.
2. Printer/parser identity for the core-5: re-parse the emitted source and
   assert the re-parsed `WorkflowSource` equals the emitted AST
   (`ToSexpr`-desync detector — V&S stop condition; also closes the V&S
   note that `ToSexpr`'s own tests don't cover all variants' printing).
3. Idempotence: `emit_dsl` twice on the same DAG yields byte-identical
   source (canonical means canonical).
4. Red-side proof: for every red fixture, the refusal emits no artifact and
   the DAG's `graph_state_hash` is unchanged after the attempt.
5. **CI wiring, not tests-only** (the gate that doesn't run is not a gate):
   the round-trip suite runs in the production-gates workflow. If runtime is
   trivial (expected — fixtures are tiny), it rides the existing test job;
   if a dedicated step is needed, follow the established gate-script pattern
   (`scripts/check-*.py` precedent) rather than inventing a new mechanism.

### Gate B2

Red→green trace in the receipt: at least one fixture that must refuse and
one that must admit-equivalent, both shown failing when the invariant is
broken (mutation-test the harness once: deliberately corrupt the emitter in
a scratch branch, show the gate goes red, revert — record the trace).
Cement-locked thereafter. **U4 of the fuzz plan becomes unblockable once
this gate is accepted** — note it in the receipt but do not open U4.

---

## B3 — Canonical receipt surface

**Tier: CAREFUL. Default: smallest possible surface.**

### Work

1. Read-only endpoint on the designer server (shape decided at B3 start,
   surfaced-not-decided here: likely
   `GET /api/dsl/sessions/{id}/dsl-receipt` returning
   `{ source, graph_state_hash, refused: Option<diagnostic> }` — a refusal
   is a valid, honest receipt response, not a 500).
2. Router-level tests: green session returns source that recompiles;
   refusing session returns the typed diagnostic; endpoint never mutates
   (graph hash unchanged — same pattern as
   `test_utterance_proposal_stages_without_mutating_graph`).
3. Public-api baselines updated; boundary gates clean.

### Gate B3

Endpoint proven read-only; receipt shows both response shapes live. Plan
closes. Remaining work — the full-parity programme (new AST variants, XOR
oracle, IR-side loop provenance, process-level syntax) — is handed to the
separate DSL-parity planning phase per the fork-A ruling, seeded with B1's
refusal catalogue and B2's gap list.

---

## 2. Stop conditions

Inherited from the V&S (§7) plus:

- B0 item 3 discovers the plan-equality needs constructs `project_ir`
  refuses → STOP (scope mis-drawn).
- Any green fixture fails plan-equality for a reason that implicates
  `project_ir` or `dsl::compile` themselves (not the emitter) → STOP; that
  is a pre-existing convergence defect, its own gated fix, not something to
  absorb silently here.
- `ToSexpr` output fails re-parse identity for any core-5 form → STOP; fix
  the printer/parser desync as its own gated defect first.
- Any pressure to widen visibility beyond B1's enumerated additions.

---

## 3. Receipt template

House template (as `EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001` §8), plus a
mandatory line per tranche: "Refusal catalogue delta vs B0's frozen list:
none / amended (list)."

---

## 4. Pre-execution review checklist

- [ ] Ratify §0's restatement of the seven fork rulings as binding.
- [ ] Ratify the B0→B3 tranche boundaries and the B2 keystone framing.
- [ ] Confirm the parity work stays out (separate planning phase, fork A).
- [ ] Confirm B3's endpoint shape is decided at B3, not now.
- [ ] Authorise B0.

**Status: no implementation approved. Begin B0 only after peer review.**
