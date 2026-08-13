# EOP-VS-GRAPH-DSL-BRIDGE-001 — Graph-Repl → canonical DSL bridge (vision & scope)

| Field | Value |
| --- | --- |
| Status | **All forks ruled (Adam, 2026-08-13 — A/B/E explicitly, C/D/F/G accepted as recommended; see §5). Implementation plan: `EOP-PLAN-GRAPH-DSL-BRIDGE-001.md`.** |
| Baseline researched | `1b3d390` (2026-08-13), branch `codex/bpmn-gameboard-refactor` |
| Purpose | Satisfy the U4 precondition of `EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001` and §8.1/§27-item-10 of `EOP-VS-BPMN-CAPABILITY-FABRIC-004`: define the requirement, the ground truth, and the forks a bridge implementation plan must rule before it can be written. |
| Decides | Nothing. Every fork in §5 is surfaced with a recommendation; peer review rules them. |

---

## 1. The requirement

Two prior ratified documents demand this capability without designing it:

1. **U4 precondition** (`EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001` §U4): a
   separately reviewed plan must land "a genuine production capability that
   projects an admitted `DesignerDag` or operation-applied graph to canonical
   `bpmn-dsl` source or a stable DSL AST," stating owner crate, public entry
   point, canonicalisation and identifier/order rules, the representability
   catalogue with typed refusals, the equivalence relation, and round-trip
   compatibility/diagnostic ownership. It explicitly warns: `DesignerDag::to_ir()`
   is IR projection, not DSL emission — do not conflate them.
2. **Capability-fabric V&S** (`EOP-VS-BPMN-CAPABILITY-FABRIC-004` §8.1, line
   1007-1009, acceptance criterion 19): "No `DesignerDag` … may carry runtime
   semantics that cannot round-trip through the DSL AST without loss …
   publication must be able to emit a complete canonical DSL source receipt …
   'Only constructible through the Designer' is a release-blocking coverage
   failure." §27 item 10 flags the round-trip specification as a required
   document not yet written. **This document is that specification's V&S
   stage** — the concrete spec follows once §5's forks are ruled.

The product value: an admitted graph becomes auditable, diffable,
version-controllable *source* — the canonical DSL receipt is the human-readable
witness of what the Designer built, and the DSL compiler re-admitting it to the
same artifact identity is the proof the receipt is faithful. Without the
bridge, Designer-built workflows exist only as graph state + edit logs, a plane
the fabric V&S names release-blocking.

---

## 2. Ground truth (verified against code at `1b3d390`, not assumed)

### 2.1 What already exists

| Component | Location | What it does | What it does NOT do |
| --- | --- | --- | --- |
| `ToSexpr` | `bpmn-lite-compiler/src/dsl/refactor.rs:1-177` | Full AST→text printer for all 7 `NodeAst` variants (used by `AstMutator`; its tests exercise task/loop shapes, not per-variant print coverage — a B2 harness must close that). | Never touches `IRNode`/`IRGraph` — solves AST→text only. |
| `project_ir` | `bpmn-lite-compiler/src/dsl/ir_plan.rs:159` | `IRGraph → WorkflowExecutionPlan`, fail-closed: typed `IrPlanError` refusals for every unsupported construct. | Goes to the *plan*, not to DSL source/AST. But it is the house precedent for partial-representability discipline. |
| `DesignerDag::to_ir()` | `designer-graph/src/schema.rs:242-275` | Structural clone to `IRGraph` — payload *is* `IRNode`, so no per-node field can be dropped. | Excludes process-level `default_guard_budget`/`default_retry_policy` (schema.rs:88-103 — "no IRNode home"); a bridge reading only `IRGraph` silently loses these. |
| Missing entirely | — | — | No graph→AST or graph→text code exists anywhere (grepped: `unparse|to_dsl|graph_to_dsl|emit_dsl|render_dsl` — zero production hits). No abandoned attempts either. |

### 2.2 The representability gap — the actual obstacle

`IRNode` has **15** variants (ir.rs:62-187 — the count was independently
re-verified after a first-draft "13" miscount was caught by blind review);
`NodeAst` has 7. Intersecting "writable in DSL source" with "projectable by
`project_ir`" leaves a lossless core of:

**`Start`, `End` (incl. `terminate:true`, via the existing `"terminated"`
string-sentinel convention — set in ir_plan.rs:231-237, read back in
frontend.rs:380-393), `ServiceTask`↔`TaskAst`, `MessageWait`, matched
`GatewayAnd` pairs.**

That is a core of 5; the refusal catalogue for a narrow bridge is the
remaining **10** kinds (`GatewayXor`, `GatewayInclusive`, `TimerWait`,
`HumanWait`, `BoundaryTimer`, `BoundaryError`, `DataObject`,
`FfiServiceTask`, `SendTask`, `MultiInstance`), and any
per-`IRNode`-kind error enum must enumerate all 15.

Everything else is deficient in at least one direction:

| Construct | DSL AST surface | `project_ir` support | Verdict today |
| --- | --- | --- | --- |
| `GatewayXor` | `SplitAst{mode:Xor}` exists | **Refused** (falls into the `UnsupportedNode` catch-all, ir_plan.rs:407-409, cement test :577-593 — no direction field, no join-pairing oracle) | Round-trips in *neither* direction |
| `GatewayInclusive` | `SplitAst{mode:Or}` exists (`split-or`) — but the grammar REQUIRES `:plug` + a `:condition` on every flow (parser.rs:393-406), and the DSL path lowers plug → `routing_socket: Some`, while `project_ir` emits `routing_socket: None` | supported | Plan-equality unreachable → **not** in the lossless core (B0 finding — refines this row's original "none/DSL-unwritable", which was imprecise about the surface but right about the verdict) |
| `MultiInstance` (+ per-element `inputs`, G4) | none | supported | DSL-unwritable |
| `FfiServiceTask`, `SendTask`, `HumanWait` | none | **refused** | Both directions missing |
| `BoundaryTimer`/`BoundaryError` (guards, budgets) | none | supported (as `GuardExecSpec` on host) | DSL-unwritable |
| `DataObject` (G7) | none | placeholder wiring only | DSL-unwritable |
| `TimerWait` | none directly | supported (`Wait`) | DSL-unwritable |
| Loops | `LoopAst{ceiling}` exists | n/a — **unrolled before the graph exists** (`unroll.rs:1-16`, G3) | See §2.3 |
| Process-level guard/retry defaults (G5) | none | carried by `admit()`, not `to_ir()` | Lost by any IRGraph-only bridge |

### 2.3 Three structural problems, not just missing variants

1. **Loops are compile-time sugar with no graph-side marker.** `unroll.rs`
   expands `(loop :ceiling N)` into N forward-chained copies before any graph
   exists; per-copy ids derive from loop-id + index (I33). The DSL path
   **already stamps loop provenance**: `unroll.rs:260` writes
   `loop_origin` into each unrolled `TaskAst`, and the linter carries it
   through to `TaskExecNode.loop_origin` in the plan (linter.rs:440,
   plan.rs:384). What is missing is only the **IR-side carrier**: `IRNode`
   has no loop field, `DesignerNode` has none, and `project_ir` hardcodes
   `loop_origin: None` when projecting graph-authored tasks
   (ir_plan.rs:254). So graph-authored copies carry no marker, and
   reconstructing `LoopAst` from a graph is a pattern-recognition fold,
   not an inverse function — but fork E option (3) is smaller than "build
   provenance from scratch": stamp and plan-side field both exist; only IR
   carriage is absent. (First draft understated this; corrected per blind
   review.)
2. **`GatewayXor` has no join-pairing oracle** in the compiler's exposed
   surface; `ir_plan.rs:31-37` already refuses it. A bridge cannot emit
   `SplitAst{mode:Xor, join: …}` without knowing which join closes which
   split. (The `GatewayAnd`/`GatewayInclusive` pairing oracle exists —
   `gateway_pairs` — but is not defined for XOR.)
3. **No canonical order or naming exists on the graph side.** BPMN string ids
   are opaque user text, uniqueness-only (`schema.rs:148-150`); petgraph
   arena order is semantically meaningless; `WorkflowSource.nodes` order *is*
   the source declaration order that `ToSexpr` reproduces.
   `graph_state_hash`/`ir_graphs_equivalent` do define a sort (nodes by BPMN
   id, edges by from/to/condition) — but that is a hash-canonicalisation,
   not a valid program emission order (id-sorted output would interleave
   unrelated branches). Any deterministic emission order (e.g. topological,
   ties by BPMN id) is a **new invention** this bridge must define and
   freeze — it is the "canonicalisation and identifier/order rules" clause
   of the U4 precondition.

### 2.4 Adjacent facts a spec must not trip over

- Two different "graph identity" notions already coexist: `graph_state_hash`
  (content-derived, `schema.rs:363`) vs the server's
  `graph_identity_hash`/`graph_content_hash` (route-derived — edit-log hash,
  despite the name; warned in `schema.rs:352-362`). The bridge's equivalence
  relation must name which one it means. Recommendation in §5 fork D.
- A third representation exists: `bpmn-lite-authoring`'s `NodeDto` (BPMN-XML
  import path), converging at `IRGraph` via `dto_to_ir` (`rest.rs:2206-2222`,
  "no YAML/DSL text round-trip"). Out of scope here, but the spec must say so
  explicitly to stop scope creep.
- Stale-count discrepancy: `bpmn_board.rs`'s comment says "the binder/engine
  cannot execute these seven actions yet," but the shipped pack has only 2
  `not_representable` capabilities (+5 `needs_workbook`). Surfaced for
  correction; nothing in this V&S depends on it.
- Stale module header: `ir_plan.rs`'s doc comment (lines 13, 38-39) still
  lists `MessageWait` and `MultiInstance` as unsupported, but the code
  projects both (:278-294, :381-400 — G5.4a landed the MI projection). The
  tables above follow the code. The header should be fixed (trivial doc
  commit) before this V&S is spot-checked against it, or a reviewer will
  derive a false refutation from the prose.
- The codebase already tried DSL-native REPL editing once
  (`EOP-SAGE-REPL-BPMN-001` T0/T4: `AstMutator`/`ToSexpr` as the mutation
  surface, graph as read-only rendering) and superseded it with the
  `DesignerDag` architecture. The bridge is *not* a return to that — the graph
  stays normative for authoring; DSL emission is a projection/receipt, exactly
  parallel to how `project_ir` is a projection to the plan store.

---

## 3. Proposed capability shape (the thing peer review is ratifying in outline)

```text
DesignerDag ──(existing to_ir + process-level fields)──▶ bridge input
  bridge: emit_dsl(&DesignerDag) -> Result<CanonicalDsl, DslEmitError>
    1. representability check   — typed refusal per unsupported construct
                                  (extends the IrPlanError catalogue pattern)
    2. structure recovery       — gateway block pairing via the existing
                                  gateway_pairs oracle (And/Inclusive);
                                  XOR per fork B ruling
    3. canonical ordering       — deterministic node order + formatting
                                  (fork C ruling)
    4. AST construction         — WorkflowSource/NodeAst values
    5. text emission            — existing ToSexpr (reused, not rewritten)

proof obligation (the U4/P8 contract):
  dsl::compile(emit_dsl(dag)) admits, and its artifact is equivalent to the
  graph's own admitted artifact under the fork-D equivalence relation.
  A refusal preserves the DAG and emits no partial DSL artifact.
```

- **Fail-closed is non-negotiable** (house rule + `project_ir` precedent): a
  construct outside the ruled scope refuses with a diagnostic naming the node
  and the missing DSL surface. No silent dropping, no lossy encoding, no
  `ServiceTask`-faking of unrepresented kinds (`ops.rs:36-40` precedent).
- The bridge is **one-directional as a capability** (graph → DSL). The reverse
  direction already exists (`dsl::compile`); "round-trip" means
  emit-then-recompile-then-compare, not a new DSL→graph importer.
- `ToSexpr` is reused as the final printer; the new work is steps 1–4.

---

## 4. What this V&S excludes (scope boundary, any ruling)

- No change to the DSL grammar/lexer/parser except where a fork-A ruling
  explicitly adds AST variants (full-parity option only).
- No DSL→`DesignerDag` importer. No change to the XML/`NodeDto` path.
- No change to `project_ir`, `unroll.rs`, or the plan store, except the
  optional loop-provenance stamp under fork E.
- No fuzz target — that is U4 itself, which opens only after this bridge lands
  and is accepted.
- No Sage/palette exposure: emission is a governed projection, not a new verb.

---

## 5. Forks to rule (each with recommendation; none decided here)

**A. Scope: narrow-core v1 vs full parity.**
Narrow v1 = the §2.2 lossless core only (Start/End/ServiceTask/MessageWait/
matched-And); all 10 other `IRNode` kinds refuse with typed diagnostics.
Full parity = extend `NodeAst` (+lexer/parser/`ToSexpr`/linter) with variants
for MultiInstance, guards, Inclusive, Timer/Human/Send/Ffi tasks, DataObject —
a second compiler-surface expansion programme in its own right.
*Recommendation: narrow v1 first.* It is honest (most real Designer graphs
will refuse today — the refusal diagnostics become the prioritised backlog for
parity work), it lands the entire bridge skeleton + equivalence proof + gate
machinery on a small surface, and each later AST variant becomes its own
reviewable tranche extending a proven frame rather than a big-bang grammar
change. Fabric-V&S criterion 19 is met *incrementally with a visible gap
list*, not claimed early.

**RULED (Adam, 2026-08-13): full parity is the required end state — "it's
simply not done otherwise"; a narrow bridge alone does not discharge the
fabric-V&S criterion. The DSL-parity surface (new AST variants + grammar +
printer + `project_ir` alignment) is to be scheduled as its own separate
planning phase/document, not folded into this bridge plan's tranches. The
narrow v1 bridge proceeds only as the staging skeleton whose refusal
catalogue feeds that parity plan; the capability is not "done" until the
parity plan closes.**

**B. `GatewayXor` policy.**
Options: (1) refuse XOR in v1 (consistent with `project_ir`, which also
refuses it — the two projections stay aligned); (2) build the XOR
join-pairing oracle now (new compiler capability, benefits `project_ir` too).
*Recommendation: refuse in v1; raise the oracle as its own follow-up
capability since it unblocks two projections at once.*

**RULED (Adam, 2026-08-13, "see my answer to fork A"): the XOR
join-pairing oracle belongs to the separate parity planning phase ruled
under fork A. v1 refuses XOR; the parity plan owns the oracle.**

**C. Canonicalisation rules.**
Emission order must be deterministic and content-derived: proposed —
topological order from the unique Start, ties broken by BPMN id (lexicographic);
gateway blocks emitted as nested `SplitAst` flows in edge-id order; formatting
fixed by `ToSexpr`'s existing indentation. BPMN ids pass through verbatim
(they are already uniqueness-enforced user text; inventing a renaming scheme
would break the "same artifact identity" proof).
*Recommendation: ratify the above as the frozen v1 rule; any change is a
version bump of the bridge contract.*

**D. Equivalence relation for the round-trip proof.**
Options: (1) `ir_graphs_equivalent(to_ir(dag), ir_of(compiled_dsl))` — but the
DSL path produces a `WorkflowExecutionPlan`, not an `IRGraph`, so this needs
an IR extraction the DSL path may not expose; (2) compare at
`WorkflowExecutionPlan` level: `project_ir(to_ir(dag)) ≡ dsl::compile(emitted)`
under a defined plan-equality (both paths already converge at the plan — the
DSL-authoring audit confirmed the plan is the only convergence point).
*Recommendation: (2), plan-level equality, with `graph_state_hash` (the
content-derived hash, NOT the route-derived server hashes) recorded alongside
as the graph-side identity witness.* This makes the proof use only existing,
already-trusted convergence machinery.

**E. Loop policy.**
Options: (1) v1 refuses nothing (graphs have no loop marker, so unrolled
copies emit as plain tasks — *silently losing the loop abstraction*, violating
"without loss" for authoring intent though not for runtime semantics);
(2) refuse graphs whose provenance shows loop-template origin until a
provenance stamp exists; (3) teach `unroll.rs` to stamp loop provenance into
the graph (e.g. via `TaskAst.loop_origin`'s IR-side counterpart) so emission
can fold copies back into `LoopAst`.
*Recommendation: (1) for v1, explicitly documented as "runtime-faithful,
authoring-abstraction-lossy," with (3) raised as a named follow-up.* Runtime
semantics are preserved exactly (the unrolled form IS the admitted semantics
per G3); only the sugar is lost, and the equivalence relation (fork D) is
unaffected. Peer review may rule (2) if authoring-intent loss is deemed
release-blocking now.

**RULED (Adam, 2026-08-13): option (1) — v1 emits unrolled copies as plain
tasks. IR-side provenance carriage joins the parity planning phase's
candidate list.**

**F. Owner crate and entry point.**
Options: (1) `bpmn-lite-compiler::dsl` (owns the AST, the printer, and
`project_ir` — the bridge is a third projection alongside them);
(2) `designer-graph` (owns the input); (3) a new crate.
*Recommendation: (1) `bpmn-lite-compiler`,* entry point
`pub fn emit_dsl(ir: &IRGraph, process_decls: &ProcessDecls) -> Result<…>`
plus a thin `DesignerDag`-aware wrapper — keeping AST construction where the
AST lives, mirroring `project_ir`'s placement exactly. `designer-graph` must
not depend on printing; the compiler already sits below both.
Note the input must include the process-level guard/retry defaults `to_ir()`
excludes (§2.1) — hence the explicit second parameter, not a bare `IRGraph`.

**G. Process-level declarations carrier.**
Resolved by direct grammar audit (blind review): the DSL parser/lexer has
**no** process-level syntax for a default guard budget or default retry
policy — zero hits for retry/budget/guard forms in `parser.rs`/`lexer.rs`;
the `(workflow name …)` form accepts no process-level attributes at all
(`parser.rs:144-172`). So these two G5-era declarations join the fork-A
parity backlog, and *v1 must refuse any DAG that sets them — never silently
drop them*. No longer an open audit item; the ruling needed is only whether
peer review accepts that refusal posture (recommended) or prioritises the
grammar extension into v1.

---

## 6. Sketch tranche map (illustrative — final map belongs to the implementation plan, written only after §5 is ruled)

```text
Bridge plan (this document's successor — the staging skeleton):
B0  Contract & representability catalogue    frozen fork rulings → spec doc; no code
B1  emit_dsl skeleton + refusal catalogue    typed DslEmitError for all 15 IRNode kinds; core-5 emission
B2  Round-trip proof harness                 emit → compile → plan-equality gate; red fixtures (must-refuse)
                                             and green fixtures (must-admit-equivalent), CI-wired
B3  Canonical receipt integration            server-side "DSL source receipt" surface (read-only endpoint
                                             or artifact), public-api reviewed

DSL-parity plan (SEPARATE planning phase, per fork-A ruling — its own V&S/
plan document, own gates; owns the new AST variants, the XOR join oracle
(fork B), IR-side loop provenance (fork E follow-up), and process-level
guard/retry syntax (fork G)). The capability is not "done" until it closes.

U4  (fuzz plan)                              opens per its own precondition once B2's gate is green
```

Every tranche: receipt, blind review, STOP-gate — the established discipline.

---

## 7. Stop conditions for the eventual implementation

- Any pressure to emit a construct lossily instead of refusing.
- Any need to widen `designer-graph` or server visibility solely for emission.
- The plan-level equivalence (fork D) turning out to require comparing
  constructs `project_ir` refuses — that means the fork-A scope was drawn
  wrong; return to review, don't special-case.
- Discovery that `ToSexpr` output does not re-parse to an identical AST for
  any core-5 form (printer/parser desync) — fix as its own gated defect first.

---

## 8. Peer-review checklist

- [ ] Ratify the capability shape (§3): projection + typed refusal + recompile-equivalence proof.
- [ ] Rule fork A (scope), B (XOR), C (canonical order), D (equivalence relation), E (loops), F (owner crate), G (process-level decls).
- [ ] Confirm the §4 exclusions.
- [ ] Confirm the §2.2/§2.3 ground-truth tables (spot-check against code).
- [ ] Authorise drafting of the implementation plan (B0–B3) against the ruled forks.

**Status: no implementation approved. This document decides nothing; it surfaces.**
