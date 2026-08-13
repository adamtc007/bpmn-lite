# EOP-PLAN-DSL-PARITY-001 — DSL parity, wave 1 (P1 + P1½ + P3-XOR)

| Field | Value |
| --- | --- |
| Status | **DRAFT — awaiting peer review. Begin D0 only after acceptance.** |
| Governing V&S | `EOP-VS-DSL-PARITY-001.md` — forks A–G ruled 2026-08-13 |
| Baseline | `76c22b7`, branch `codex/bpmn-gameboard-refactor` |
| Scope | Per fork-B ruling: P1 (guards, TimerWait, MultiInstance) + loop provenance + flow-order canonicalisation + P1½ Inclusive + P3-XOR. **P2 (HumanWait/SendTask/Ffi — plan-contract expansion) and DataObject are OUT — a future programme with its own V&S.** |
| Execution | One tranche per change set; STOP for review at every gate; receipts + authorship-blind review per the house discipline. The **B2 harness is the standing acceptance machine**: a kind is in parity exactly when its G-fixtures pass the four-proof round trip. |
| Does not authorise | Any P2 `ExecutionNode` addition; DSL→graph import; XML/`NodeDto` changes; Sage/palette exposure; removal of any B-series cement test. |

---

## 0. Binding constraints (the ruled forks, restated)

1. **Every tranche delivers the full vertical** (fork A): grammar +
   parser + `ToSexpr` + linter lowering + emission arm (deleting that
   kind from the `UnsupportedNode` `|`-pattern — the no-wildcard match
   forces this consciously) + `project_ir` alignment where needed + B2
   green fixtures + red fixtures for new failure axes + public-api
   baselines. No kind is "done" at less than the vertical.
2. **Grammar rules** (fork C): every new form uses a NEW head keyword —
   never overloading an existing one; attributes as keyword/symbol
   pairs, str-lits only for legitimate free text; each kind's concrete
   grammar is frozen at its own tranche's contract gate (a B0-style
   paper freeze **before** code); print→parse→print fixpoint is a
   mandatory cement test before the tranche closes. Existing sources
   must keep compiling unchanged (additive grammar only).
3. **Emission canonical-form changes are version bumps** (bridge
   contract): any tranche that changes the emitted source for a graph
   the bridge already emits (loop folding is the known case) must say so
   in its receipt and bump the bridge-contract version note in
   `emit.rs`'s module doc.
4. **Fail-closed inheritance**: each new kind inherits the frozen
   refusal ordering; new refusal axes extend the B0 catalogue via the
   amendment rule (recorded in the owning tranche's receipt AND the B0
   receipt).

---

## 1. Tranche map (strictly ordered unless noted)

```text
D0  Flow-order canonicalisation (fork F)     project_ir sorts flows by edge id; impact check; B2 tightens
D1  Boundary guards                          grammar+vertical for BoundaryTimer/BoundaryError (+ reachability redefinition)
D2  TimerWait                                vertical (small; may land with D1 review if trivial — gate decides)
D3  MultiInstance                            vertical incl. per-element inputs authoring surface question (gate D3.0)
D4  Loop provenance IR carriage (fork E)     IRNode carrier + fold-back ruling at the gate
D5  XOR join oracle (fork B / P3)            traced join-inference design note → gate → implementation; unblocks BOTH projections
D6  Inclusive alignment (fork D / P1½)       design note tracing routing-socket/OR-named-subset settled decisions → gate → implementation
```

D0 first: it is small, it sharpens the acceptance machine for everything
after (ordered equality instead of multiset), and it discharges the
replay-tape observation before more kinds pile onto the harness.
D5 and D6 each open with a **design note gate** (no code before the note
is accepted) — both sit on settled-decision territory (SESE pairing;
routing-lives-in-the-box) where implementation before design review is
exactly the failure mode the working contract forbids.

---

## D0 — `project_ir` flow-order canonicalisation

**Tier: CAREFUL. Touches a shipped projection's stored bytes.**

1. Sort `SplitExecNode.flows` by outgoing edge id in `project_ir`
   (matching emission's frozen rule) — the projection becomes
   content-canonical.
2. **Impact check, evidenced not assumed**: enumerate plan-store
   consumers of stored plan bytes/hashes (plan-store content addressing,
   template freezing, G6 replay artifacts); state per consumer whether a
   re-projection changes identity and what that means. If any consumer
   pins byte-identity of already-stored artifacts across re-projection,
   STOP and surface before landing.
3. Tighten B2's `normalize_plan` back to ordered flow equality (delete
   the multiset sort; the amendment reverses, recorded in both
   receipts). Add a cement test: two edit orders building
   `ir_graphs_equivalent` DAGs project byte-identical plans.
4. Kernel-side note in the receipt: fork target order is now
   content-derived — the replay-tape determinism claim this unblocks.

### Gate D0
Cement test green; impact table in the receipt; B2 tightened; all
existing suites green.

---

## D1 — Boundary guards (BoundaryTimer / BoundaryError)

**Tier: CAREFUL. The largest P1 tranche — includes a reachability
redefinition.**

1. **D1.0 contract freeze (paper, gated like B0):** grammar for guard
   forms (new heads, e.g. a top-level form referencing `:host`, or
   host-nested — decided at this freeze, not now); the emission
   reachability rule change — today a guard node refuses `UnreachableNode`
   because it has no incoming flow edge (B3 finding); the frozen Stage-0
   rule must be amended to: reachable = flow-reachable ∪ guard-attached
   (to a reachable host) ∪ reachable-from-a-guard's-escape-edge. New/
   changed refusal axes (guard on unsupported host; guard escape
   out-degree ≠ 1 — mirror `IrPlanError::GuardEscapeUnresolved`;
   `failure_budget` attribute shape; interrupting/rearming flag) frozen
   as B0-catalogue amendments.
2. Vertical per §0.1: parser/`NodeAst`/`ToSexpr`/linter lowering to
   `GuardExecSpec` on the host task (the DSL path must produce the SAME
   `GuardExecSpec` shape `project_ir` produces — that IS the equality
   proof) + emission arms + B2 green fixtures (interrupting timer guard;
   error guard; guard with failure budget; escape chain to its own End)
   + red fixtures per new axis.
3. The B3 endpoint's guard-graph test flips from refusing to emitting —
   rewrite it as a green receipt test in the same commit (cement update,
   named in the receipt, not silent).

### Gate D1
B2 four-proof round trip green for all guard fixtures; fixpoint cement;
existing guard runtime tests (WS-D, G5) untouched and green.

---

## D2 — TimerWait

Vertical per §0.1: new head (e.g. `timer-wait`) carrying the `TimerSpec`
(duration/date/cycle+max_fires — all three, or the freeze narrows with
reasons), linter lowering to `ExecutionNode::Wait`, emission arm,
fixtures. Small; its D2.0 freeze may be reviewed together with D1's gate
if the gate agrees.

---

## D3 — MultiInstance

1. **D3.0 freeze:** grammar for `multi-instance` (task_type, collection
   flag, declared_max) **and the per-element `inputs` question**: G4's
   `Vec<FfiInputBinding>` has no DSL story — either a bindings sub-form
   or an explicit documented exclusion (empty-inputs-only in wave 1,
   refusing a graph MI with non-empty inputs — fail closed, never drop).
   Surfaced at the freeze, not decided here.
2. Vertical per §0.1; fixtures include declared_max round-trip and the
   inputs disposition per the freeze.

---

## D4 — Loop provenance IR carriage (fork E)

1. Add the IR-side carrier (per-node `loop_origin` on `ServiceTask` — or
   the freeze's chosen shape), stamped through the DSL path's existing
   unroll provenance and through `project_ir` (replacing the hardcoded
   `None`).
2. **Gate ruling item:** whether emission folds provenance-marked copies
   back into `LoopAst`. Folding = bridge-contract version bump (§0.3)
   and a fold-correctness proof (folded source recompiles to the SAME
   unrolled plan — the B2 machine decides). Not folding = carrier lands
   for future use only. The tranche presents both costed; the gate
   rules.

---

## D5 — XOR join oracle (design note first)

1. **D5.0 design note (no code):** a traced join-inference design for
   `GatewayXor` — options include an explicit `direction`+pairing field
   on the IR node (aligning XOR with And/Inclusive's shape), extending
   `gateway_pairs`' post-dominator pairing to XOR, or an explicit join
   annotation carried from authoring. Must trace the SESE/RPST settled
   decisions and state why the chosen oracle can't mispair. STOP-gated
   as its own review.
2. On acceptance: oracle lands in `lowering.rs` (one oracle, both
   projections consume it — never two pairings); `project_ir` gains XOR
   projection; emission gains the XOR arm (grammar already exists:
   `split-xor`/`join-xor`); conditions on XOR flows are Eq-only both
   ways; fixtures.

---

## D6 — Inclusive alignment (design note first, fork D)

1. **D6.0 design note (no code):** trace "routing lives in the box" and
   "OR gateways use named-subset output types" against the current
   `split-or` grammar (mandatory `:plug` + conditions) and the graph
   side's plug-less `GatewayInclusive`. Expected recommendation
   direction (not pre-decided): the GRAPH side gains the decision-socket
   carriage rather than the grammar dropping it — but the note owns the
   argument. STOP-gated.
2. On acceptance: implement per the note; fixtures; the B0-era
   "Inclusive not in lossless core" verdict flips only when the B2
   machine says so.

---

## 2. Stop conditions

Inherited from the bridge plan §2, plus: any pressure to overload an
existing grammar head; any per-kind lowering that produces a
`GuardExecSpec`/`ExecutionNode` shape differing from `project_ir`'s for
the same construct (that is a convergence defect, its own gated fix);
D0's impact check finding a byte-identity-pinned consumer; any D5/D6
implementation urge before its design note is accepted.

## 3. Receipts

House template + the B-series extra line ("Refusal catalogue delta vs
B0's frozen list: …") + for grammar tranches: the frozen grammar block
quoted verbatim in the receipt.

## 4. Pre-execution review checklist

- [ ] Ratify the D0–D6 order and the D5/D6 design-note-first gates.
- [ ] Confirm P2 + DataObject stay out (fork-B ruling).
- [ ] Ratify §0's binding constraints (vertical, additive grammar,
      version-bump rule, amendment inheritance).
- [ ] Authorise D0.

**Status: no implementation approved. Begin D0 only after peer review.**
