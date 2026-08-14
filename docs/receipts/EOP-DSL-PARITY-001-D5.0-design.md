# Design note — EOP-PLAN-DSL-PARITY-001 Gate D5: XOR join oracle (fork B / P3)

**Status:** DRAFT — no code. STOP-gated per the plan doc's own instruction:
"D5.0 design note (no code): a traced join-inference design for
`GatewayXor`... Must trace the SESE/RPST settled decisions and state why
the chosen oracle can't mispair. STOP-gated as its own review."

## 0. Source-of-truth verification (read directly, not from memory)

- `ir.rs:87` `IRNode::GatewayXor { id, name }` — **no `direction` field**,
  unlike `GatewayAnd { id, name, direction: GatewayDirection }` (`ir.rs`
  near GatewayAnd) and `GatewayInclusive { id, name, direction }`.
  `GatewayDirection` (`ir.rs:24-28`) is `{ Diverging, Converging }`.
- `gateway_pairs` (`lowering.rs:1365-1388`, public API) pairs a node with
  its immediate post-dominator (`compute_post_dominators`,
  `lowering.rs:1060-1162`, standard iterative post-dominance) **only**
  when both sides are the same gateway kind AND opposite direction:
  ```
  (GatewayAnd{direction: Diverging,..}, GatewayAnd{direction: Converging,..}) => true
  (GatewayInclusive{direction: Diverging,..}, GatewayInclusive{direction: Converging,..}) => true
  _ => false
  ```
  (`lowering.rs:1372-1382`). This same-kind+direction guard exists
  specifically to prevent cross-kind mispairing (`lowering.rs:1341-1349`'s
  own comment calls it a "crossing hazard" guard) — it is not incidental,
  it is the mechanism that makes the oracle safe. `GatewayXor`, having no
  `direction` field, cannot participate in this match on either side
  without first gaining a discriminator.
- `compute_region_map` (`lowering.rs:1177-1191`) — the bytecode-layout
  helper, a **different** function from `gateway_pairs` — is
  direction-agnostic by design (out-degree-based), and its own doc
  comment (`lowering.rs:1232-1243`) confirms this was deliberate so XOR
  merges (which can reconverge on a bare `ServiceTask`, no gateway
  element at all) get correct layout for free. This function already
  handles XOR; it is not the oracle in question here — `gateway_pairs` is
  a *typed* pairing used for plan projection/emission, a stricter
  contract than raw layout.
- `project_ir` refuses `GatewayXor` via the generic catch-all
  (`ir_plan.rs:445-447`, `IrPlanError::UnsupportedNode`), confirmed by its
  own red test (`ir_plan.rs:642-658`).
- `emit_dsl` refuses `GatewayXor` via the out-of-core-kinds arm
  (`emit.rs:848`, `DslEmitError::UnsupportedNode`, test at
  `emit.rs:1641-1649`). For `GatewayAnd`, emission derives the printed
  `:join`/`:split` id from `gateway_pairs` at call time
  (`emit.rs:490,711-734`) plus a `join_to_split` reverse map that refuses
  (`UnmatchedGateway`) if one join is claimed by more than one split —
  IR's `GatewayAnd`/`GatewayInclusive` carry NO join-id field themselves;
  the oracle is the only source of pairing.
- DSL grammar: `split-xor`/`join-xor` **already exist** (`parser.rs:207`,
  `:216`; `refactor.rs:125`, `:163`; round-trip test `refactor.rs:587-597`)
  and use the exact same `SplitAst`/`JoinAst` structs as And/Or — only
  `SplitModeAst`/`JoinModeAst` gain an `Xor` variant. `SplitAst.join:
  String` and `JoinAst.split: String` (`ast.rs:184-192`, `:212-219`) are
  plain explicit AST fields, populated straight from parsed tokens.
  **DSL-side representability is done.** D5 closes only the graph-side
  gap: `project_ir` and `emit_dsl` (and the `gateway_pairs` oracle they
  both must share, per the plan's own binding rule — "one oracle, both
  projections consume it — never two pairings").
- RPST/SESE machinery is real, not aspirational: `dsl/rpst.rs`'s
  `verify_sese_nesting(plan: &WorkflowExecutionPlan)` — a DFS
  well-nestedness check — runs on DSL-compiled plans (`dsl/closure.rs:135`,
  `dsl/plan.rs:194`). Separately, `bpmn-lite-authoring/src/importer.rs`
  (XML→`IRGraph` import path) contains its own, independently-maintained
  XOR-capable pairing algorithm, `find_corresponding_join`
  (`importer.rs:390+`, used at `importer.rs:46-53` for XOR/And/Inclusive
  alike): a BFS-forward, per-branch-reachable-set intersection, refusing
  `"Non-SESE"` if branches never reconverge. **This is a second pairing
  algorithm, structurally different from `gateway_pairs`, already live in
  the codebase for a different entry path (XML import, not the
  designer-graph/DSL-bridge path this parity programme covers).** Noted
  as a cross-cutting observation below, not solved by this gate.
- Prior rulings: `EOP-VS-GRAPH-DSL-BRIDGE-001.md:219-228` (fork B, ruled)
  parked XOR pairing explicitly for "the separate parity planning phase"
  — this gate. `EOP-VS-DSL-PARITY-001.md:26` classifies XOR as P3
  ("structural design work") for the same reason. Neither prior ruling
  picked a mechanism — this note is the first to.

## 1. The three options, costed

**(a) Add `direction: GatewayDirection` to `GatewayXor`; extend
`gateway_pairs`' match arm to `(Xor{Diverging}, Xor{Converging}) => true`.**

Reuses the identical oracle AND/Inclusive already use, verbatim — the
same post-dominator computation, the same guard, the same
`join_to_split` dedup on the emission side. This is the literal reading
of the plan's "one oracle, both projections" rule: not "one oracle per
kind," one oracle, period, now covering three kinds via one extra match
arm. `gateway_pairs`'s post-dominator computation is topology-only — it
does not know or care whether a gateway is AND/OR/XOR semantically, only
whether the SAME kind+direction reoccurs at the immediate post-dominator.
That is exactly why AND and INCLUSIVE already share it despite having
different runtime routing semantics ("named-subset" vs "all branches") —
join-PAIRING is a graph-shape question, not a semantics question. XOR's
runtime semantics (exactly one branch taken) are irrelevant to whether
its structural join can be found the same way.

Cost: the same mechanical blast radius class D4 hit — `GatewayXor` is an
exhaustive struct literal in every construction site across the
workspace; adding a field breaks every one (`E0063`) until patched, same
"~N sites, all `direction: GatewayDirection::???`" mechanical sweep. Given
`GatewayXor` is used far less than `ServiceTask` (D4's ~90-site sweep),
this should be substantially smaller — to be counted exactly at
implementation time, not estimated here.

**(b) Infer diverging/converging from topology (out-degree/in-degree)
instead of an authored field — no new `GatewayXor` field at all.**

Avoids the mechanical field-storm entirely. But it trades a "fail closed
on authoring/edit inconsistency" property for a "silently reclassify"
one: today, `GatewayAnd`/`GatewayInclusive`'s authored `direction` lets a
future verifier check ("declared Diverging but out-degree is 1") name a
DEFECT explicitly; topology-inferred role has nothing independent to
check against — a malformed or mid-edit graph (e.g. a diverging XOR that
lost a branch down to one flow during an edit) would just silently stop
being treated as diverging, not surface as a refusal. This is the same
class of risk the working contract's "fail closed, no trap doors" rule
targets. It would also make `GatewayXor` the only gateway kind in the IR
whose role isn't authored — a real asymmetry with AND/Inclusive, not just
a style choice, since every other kind in the same enum encodes
direction explicitly.

**(c) An explicit join-id annotation carried on `GatewayXor` itself
(mirroring DSL's `SplitAst.join`/`JoinAst.split`), bypassing
`gateway_pairs` for XOR specifically.**

Directly violates the plan's own binding rule ("never two pairings") in
spirit even though it's a single kind: AND/Inclusive derive their pairing
structurally (no stored join reference — the graph IS the source of
truth); an authored join-id on XOR alone would make XOR the only kind
with a second, independently-driftable source of truth for the same
fact the graph topology already encodes — exactly the "five independent
declarations" anti-pattern CLAUDE.md warns against for lexicons,
recurring here at gateway-pairing scale. It would also NOT reuse
`gateway_pairs`, so `lowering.rs`'s runtime bytecode gen (which already
computes pairing via `compute_region_map`/`gateway_pairs` for AND/
Inclusive) would need a third, XOR-specific pairing path alongside the
two that already exist (`gateway_pairs` and `importer.rs`'s
`find_corresponding_join`) — three pairing algorithms in one codebase for
the same underlying question ("which node is my structural other half").

## 2. Recommendation: (a)

Add `direction: GatewayDirection` to `GatewayXor`, extend `gateway_pairs`'
match arm by one line. This is the option that:
- reuses the existing, already-trusted oracle exactly (zero new pairing
  logic beyond one match arm — the post-dominator machinery itself is
  untouched),
- keeps the IR's three gateway kinds structurally uniform (all three
  carry `direction`; none carry a stored join/split reference),
- preserves "fail closed on inconsistency" via an authored field a future
  verifier check can validate against topology,
- matches the plan's literal "one oracle, both projections" requirement
  without qualification.

## 3. Why the oracle "can't mispair" (the plan's required proof)

`gateway_pairs`'s guarantee, as it exists today for AND/Inclusive and
would extend identically to XOR under (a), is **"pair correctly, or
refuse — never pair incorrectly"**, not "always find a pair." Concretely:

1. The same-kind+direction match (`lowering.rs:1372-1382`) is the
   mispair guard: a `GatewayXor{Diverging}` can only ever be paired with
   a `GatewayXor{Converging}` — never with an `And`/`Inclusive` node that
   happens to sit at the same post-dominator position. This guard is
   unchanged by adding XOR; XOR simply becomes a third alternative in
   the same match, not a relaxation of it.
2. `emit.rs`'s `join_to_split` reverse-map dedup (`emit.rs:493-500`,
   `UnmatchedGateway` on collision) independently guards the OTHER
   mispair direction — two diverging gateways claiming the same
   converging node — again, kind-agnostic, so it protects XOR for free.
3. SESE well-nestedness is NOT a precondition the oracle depends on to
   avoid mispairing — it depends on it to avoid REFUSING correctly-formed
   graphs unnecessarily. A non-well-nested graph (e.g. a node reachable
   from two unrelated splits) simply won't have a clean same-kind
   post-dominator match and falls through to `UnmatchedGateway` — refusal,
   not silent mispairing. This is the same behavior AND/Inclusive already
   rely on for arbitrary designer-graph-edited (not just XML-imported)
   graphs today; XOR inherits the identical guarantee level, not a
   weaker one.

## 4. Surfaced, not solved by this gate

`importer.rs`'s `find_corresponding_join` (XML import path) is a second,
independently-maintained pairing algorithm for the same question
`gateway_pairs` answers on the designer-graph/DSL-bridge path. This gate
does not unify them — XML import is out of this parity programme's scope
entirely (unchanged by D0-D4). Flagging it here, not deciding it, per the
"surface forks, don't decide them" contract: a future observation for
whoever next touches the XML-import path, in case the two oracles could
ever disagree on the same imported-then-edited graph.

## 5. What D5 code would then look like, once ruled

Per the plan doc's own D5 §2: oracle lands in `lowering.rs` (already
does, gains one match arm) — one oracle, both projections consume it;
`project_ir` gains an XOR arm (mirroring the And/Inclusive arms exactly,
same split/join derivation via `gateway_pairs`); `emit_dsl` gains the XOR
arm (grammar already exists — `split-xor`/`join-xor`); conditions on XOR
flows are Eq-only both ways (matching the existing `ConditionAst`
machinery, no new condition shape); B2 fixtures per the established
G-series convention.

## 6. What this STOPs on

Adam rules: proceed with (a) as scoped in §2-3, or a different
disposition. No code has been touched — `ir.rs`, `lowering.rs`,
`ir_plan.rs`, `emit.rs` are all unmodified pending this ruling.
