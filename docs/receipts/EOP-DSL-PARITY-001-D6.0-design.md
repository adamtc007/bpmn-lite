# Design note — EOP-PLAN-DSL-PARITY-001 Gate D6: Inclusive alignment (fork D / P1½)

**Status:** DRAFT — no code. STOP-gated per the plan doc's own instruction:
"D6.0 design note (no code): trace 'routing lives in the box' and 'OR
gateways use named-subset output types' against the current `split-or`
grammar ... and the graph side's plug-less `GatewayInclusive`. ... STOP-gated."

## 0. Source-of-truth verification (read directly, not from memory)

- `ir.rs:139-143` `IRNode::GatewayInclusive { id, name, direction:
  GatewayDirection }` — **already has `direction`**, added alongside
  `GatewayAnd` before this session; D5 gave `GatewayXor` the same field
  by analogy. Unlike Xor, Inclusive is **not** the gap here — pairing
  already works: `gateway_pairs` (`lowering.rs:1372-1382`) already
  matches `(Inclusive{Diverging}, Inclusive{Converging}) => true`, and
  `project_ir` (`ir_plan.rs:310-320`) already handles `GatewayInclusive`
  in the SAME match arm as And/Xor, projecting `SplitMode::Inclusive` —
  confirmed by the code being grouped as one shared arm with a `match
  node { ... }` dispatch on kind for `mode` only.
- The ONLY refusal left is `emit_dsl` (`emit.rs:890`): `GatewayInclusive`
  sits in the out-of-core catch-all, `DslEmitError::UnsupportedNode`,
  confirmed by its own red test (`emit.rs:1675-1684`).
- `emit.rs` refuses because of the exact same structural fact D5 found
  for Xor: `parser.rs::parse_split` (`parser.rs:403-427`) requires
  `:plug` unconditionally whenever `mode != SplitModeAst::And` — `Or`
  included (`parser.rs:210`, `"split-or" => ... parse_split(...,
  SplitModeAst::Or)`), and every flow requires `:condition`
  (`parser.rs:429-449`, `require_condition = mode != And`).
  `IRNode::GatewayInclusive` has no field to source a `:plug` value
  from — same shape of gap as Xor's Fork 2, not a new discovery.
- `SplitAst.plug: Option<String>` (`ast.rs:188`) is a bare symbol
  resolved at LINT time via `registry.decision_bindings(plug)`
  (`linter.rs:534-535`), becoming `ExecutionNode::Split.routing_socket:
  Option<String>` (`plan.rs:461`) — a **name**, not a value: the actual
  typed decision-verb resolution happens downstream of the DSL text, in
  the pinned-pack registry. The DSL grammar's `:plug` is a *reference*
  to a pinned dmn verb, matching CLAUDE.md's "the box holds one typed
  decision (a pinned dmn verb)".
- **`project_ir` never populates `routing_socket` for ANY graph-sourced
  gateway kind** — `ir_plan.rs:383` hardcodes `routing_socket: None`
  unconditionally for And/Inclusive/Xor alike. This is the load-bearing
  fact for this note: the graph→execution-plan path today carries **no
  decision-socket concept at all**, for any gateway kind, not just
  Inclusive. `GatewayAnd` never needs one (`SplitModeAst::And` is the
  one mode `parse_split` exempts from `:plug`). Xor was ruled (D5) to
  refuse rather than fabricate one. Inclusive is the third and last
  kind facing the identical absence.
- **Named-subset semantics do not exist in the DSL grammar today,
  independent of the `:plug` gap.** `ConditionAst` (`ast.rs`, consumed
  by `parse_condition`, `parser.rs:451-480`) has exactly one shape:
  `Eq { placeholder, value }` — a single-value equality test per flow,
  identical for `split-xor` and `split-or`. There is no multi-value
  "named subset" condition shape (e.g. "this flow fires iff the
  decision's output subset contains {A, B}") anywhere in `ast.rs` or
  `parser.rs`. CLAUDE.md's "OR gateways use named-subset output types"
  is the settled TARGET architecture; the CURRENT `split-or` grammar is
  structurally identical to `split-xor` — Eq-conditioned, first/only
  match semantics — which is itself one of the things CLAUDE.md flags
  as defect-not-target ("ExclusiveGateway-only, by-name decider
  coupling... is what's being replaced"). **This note's scope is
  graph↔DSL parity for the EXISTING grammar, not implementing
  named-subset condition semantics** — that would be a grammar redesign
  on the scale of D1's guard-form work, not a parity-closing tranche.
  Flagged in §4, not solved here.

## 1. The options, costed

**(a) Refuse Diverging `GatewayInclusive` emission unconditionally —
mirror D5's Xor ruling exactly.**

Same code shape as `DslEmitError::GatewayXorSplitUnrepresentable`: a new
`GatewayInclusiveSplitUnrepresentable` (or a shared variant covering
both kinds — naming decided at implementation) returned unconditionally
for the Diverging arm; Converging mirrors `GatewayAnd`'s join arm
exactly (no plug/condition fields on `JoinAst`, no grammar constraint).
`project_ir` is unaffected either way — no grammar dependency on that
direction, already proven true for Xor.

Cost: zero new IR fields, zero new mechanical blast radius, smallest
possible diff — likely the smallest D-gate this programme has run.
Consequence: Inclusive becomes permanently unemittable via DSL text,
identical to Xor's disposition. Two of the three gateway kinds end up
grammar-unrepresentable on the diverging side; only And ever emits.

**(b) Add a minimal carrier field (`plug: Option<String>` or reuse the
`routing_socket` name for symmetry with `plan.rs`) on
`IRNode::GatewayInclusive`, populate it `None` everywhere no real
producer authors one, refuse emission with a distinct "no decision
bound" error when `None`, emit `:plug <name>` when `Some`.**

This is the plan doc's stated expected direction ("the GRAPH side gains
the decision-socket carriage"). But unlike `direction` (D4/D5
precedent: mechanically inferable from edge degree at every producer
site), a decision-verb binding has **no structural inference** — it is
authored intent, not derivable from topology. Concretely, landing this
field today would leave it `None` at every real producer
(`dto_to_ir`, the XML importer, and the designer-graph `ops.rs`
gateway-creation op) because **none of them currently have any UI/XML
surface to author a plug reference on a graph-side Inclusive gateway** —
there is no `ExclusiveGateway`-style extension-element or DTO field
carrying one today (confirmed: grep of `dto_to_ir.rs` and
`parser.rs`'s XML gateway handling shows no decision/verb reference
parsed for gateways at all, unlike `ServiceTask`'s `task_type`). So (b)
as scoped would land a field that is **always `None` in every graph
this programme's fixtures can construct**, meaning emission still
always refuses in practice — the same observable behavior as (a),
except now behind a field that LOOKS wired but isn't populated by
anything. That is close to the working contract's "no trap doors"
concern (a mechanism that exists in name but is never actually
exercised end-to-end by anything real). Building an actual authoring
surface (a new `ops.rs` mutation, a designer-UI control, an XML
extension-element parse) is out of this parity programme's scope
entirely — it's a new feature, not a bridge-parity fix.

Cost: one new field, ~4-6 mechanical sites (Inclusive is used less than
Xor even), a second refusal variant, but **no functional gain over (a)
today** — the field would ship dark. Benefit: when/if an authoring
surface for Inclusive plugs is later built (a future, separately-ruled
tranche), the IR carrier already exists and doesn't require touching
`project_ir`'s match arm shape again.

**(c) Loosen the grammar — make `:plug` optional for `split-or`
specifically (Inclusive only, not Xor), matching the `And` exemption.**

Directly rejected: this is "the grammar dropping the requirement," the
option the plan doc explicitly contrasts against its expected direction,
and it contradicts CLAUDE.md's settled "routing lives in the box, not
per-edge guards" decision — a plug-less `split-or` with only Eq
conditions per flow IS per-edge guards, the exact anti-pattern the
architecture rejects. Not seriously considered; named here only because
the plan doc's own framing raises it as the alternative (a) is chosen
over.

## 2. Recommendation: (a), with (b) explicitly flagged as the plan's
stated alternative

Recommend (a) — refuse Diverging `GatewayInclusive` unconditionally,
identical in shape and reasoning to D5's ruling for Xor — because (b)
as buildable within this programme's scope would ship a field with no
real producer ever populating it, which is observably identical to (a)
for every fixture and every real graph this programme can construct
today, while adding surface area that looks load-bearing and isn't.
(a) keeps D6 a true parity-closing tranche (same size class as D5's
Fork 2 resolution); (b)'s actual value (unlocking real Inclusive-plug
authoring) depends on a future, separately-scoped authoring-surface
tranche this note is not positioned to design.

**This recommendation diverges from the plan doc's own stated expected
direction** ("the GRAPH side gains the decision-socket carriage rather
than the grammar dropping it"). I'm flagging that tension explicitly
rather than silently picking the plan's stated expectation over my own
read of the evidence, per "surface forks, don't decide them" — Adam
rules between (a) and (b) (or requests a fourth option) with the cost
argument above in view.

## 3. What D6 code would then look like, once ruled

**If (a):** `emit.rs`'s `GatewayInclusive` arm moves out of the
out-of-core catch-all into its own match arm mirroring `GatewayXor`'s
final shape exactly — Converging emits via `join_to_split`/
`shared_joins` (same dedup machinery), Diverging returns a new
unconditional-refusal error variant. `project_ir` is untouched (already
supports Inclusive). B2 fixtures: a converging-refusal-isolation
fixture (mirroring D5's rewritten g19) plus a diverging-refusal fixture
(mirroring g18) plus a `project_ir`-still-works fixture (mirroring
g18b). Public-API diff: +1 error variant, purely additive.

**If (b):** additionally, `ir.rs`'s `GatewayInclusive` gains the field
(mechanical sweep across ~4-6 sites per D5's pattern), `project_ir`'s
hardcoded `routing_socket: None` becomes conditional on the new field
for the Inclusive arm specifically (And/Xor unaffected — no field to
read), emission's Diverging arm branches on `Some`/`None` rather than
refusing unconditionally. All producers populate `None`. Fixtures add a
`Some`-populated round-trip case built directly at the `DesignerDag`
level (bypassing the nonexistent authoring surface, since B2 fixtures
already construct graphs programmatically) to prove the `Some` path
actually emits — this fixture would be the ONLY place in the codebase
exercising a populated Inclusive plug until a real authoring surface
exists.

## 4. Surfaced, not solved by this gate

- **Named-subset condition semantics** (§0's last point): the DSL
  grammar's `ConditionAst::Eq` is single-value equality for both
  `split-xor` and `split-or` — no multi-value named-subset shape
  exists. CLAUDE.md's "OR gateways use named-subset output types" is
  unimplemented in the DSL layer entirely, independent of this gate's
  `:plug` question. This is a grammar-redesign-scale gap, not a parity
  fix — flagged for a future, separately-scoped tranche.
- **No Inclusive-gateway plug authoring surface** exists anywhere
  (designer `ops.rs`, DTO import, XML import) — if (b) is ruled, the
  carrier field ships with zero real producers, a gap explicit in §1.

## 5. What this STOPs on

Adam rules: (a), (b), or a different disposition. No code has been
touched — `ir.rs`, `lowering.rs`, `ir_plan.rs`, `emit.rs` are all
unmodified pending this ruling.
