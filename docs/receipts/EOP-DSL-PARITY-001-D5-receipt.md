# Receipt — EOP-PLAN-DSL-PARITY-001 Gate D5: XOR join oracle (fork B/P3)

**Status:** Blind review of commit `f998f3e` returned **ACCEPT**, no
corrections required (one cosmetic wording nit, fixed post-review — see
disposition below). Pending Adam's acceptance.
**Branch:** `codex/bpmn-gameboard-refactor`
**Design note:** `docs/receipts/EOP-DSL-PARITY-001-D5.0-design.md`, ratified
option (a) (Adam, 2026-08-14: "a").
**Follow-up ruling (mid-implementation):** Adam, 2026-08-14, "Always refuse
diverging (option 1)" — see "The defect this tranche caught in itself"
below.

## What was built

`IRNode::GatewayXor` gains `direction: GatewayDirection`, exactly mirroring
`GatewayAnd`/`GatewayInclusive`. `gateway_pairs` (`lowering.rs`) gains one
match arm pairing `GatewayXor{Diverging}` with `GatewayXor{Converging}` via
the identical same-kind+direction post-dominator match AND/Inclusive
already use — no new pairing logic, per the design note's "one oracle, both
projections" rule.

- `project_ir` (graph→plan): `GatewayXor` joined the existing combined
  `GatewayAnd | GatewayInclusive` match arm (now three-way), mapping to
  `SplitMode::Exclusive`/`JoinMode::Exclusive`. Full symmetry with AND —
  both diverging (conditioned flows, Eq-only) and converging directions
  work identically. **No scope reduction on this side.**
- `emit_dsl` (graph→DSL text): **asymmetric**, discovered empirically (see
  below). Converging (Join) works exactly like AND. Diverging (Split)
  **always refuses** via a new named `DslEmitError::GatewayXorSplitUnrepresentable`
  — never succeeds, regardless of graph content.

Two real (non-test) producers of `GatewayXor` outside the designer-graph
path — `bpmn-lite-authoring::dto_to_ir` and `bpmn-lite-compiler::parser`'s
raw BPMN-XML importer — have no authored direction data (`NodeDto::
ExclusiveGateway` and the `<exclusiveGateway>` XML element carry none,
unlike Parallel/Inclusive). Both infer `direction` from final out/in-degree
in a post-pass run after all edges are wired (edges aren't available at
node-construction time in either streaming/two-pass builder) — Adam's
ruling on the surfaced disposition (2026-08-14, "a" → disposition (1)):
infer locally at the two real producer sites rather than fabricate a
constant or extend the DTO/XML authoring surface (out of this gate's
scope).

## Two forks surfaced and ruled mid-tranche

### Fork 1 — direction inference for producers with no authored data

Surfaced before any code: `dto_to_ir.rs`'s `NodeDto::ExclusiveGateway`
and `parser.rs`'s `<exclusiveGateway>` element carry no direction, unlike
Parallel/Inclusive. Three options costed (infer locally / extend authoring
surface / fabricate a constant). Adam ruled: infer locally, scoped
narrowly to the two producer sites, no grammar/DTO schema changes.

### Fork 2 — GatewayXor diverging cannot satisfy the DSL grammar

**Found by this tranche's own B2 harness**, not guessed at. The first
implementation mirrored `GatewayAnd`'s diverging arm symmetrically
(condition-aware, `plug: None`) — fixture `g18` failed reparse-identity
immediately: `"expected ':plug', found ':join'"`.

Root cause, traced to `dsl/parser.rs::parse_split`:
```rust
let plug = if mode != SplitModeAst::And {
    Some(self.parse_kw_symbol("plug")?)   // MANDATORY for Xor/Or
} else {
    None
};
```
and `parse_split_flow(mode != SplitModeAst::And)` — every flow on a
`split-xor`/`split-or` requires a `:condition`; no default/unconditioned
branch is representable. `:plug` is a bound decision-verb reference
(`linter.rs` resolves it via `registry.decision_bindings`) — the grammar
requires an authored decision box for any named-subset split, matching
CLAUDE.md's "routing lives in the box" target architecture. But
`IRNode::GatewayXor` has **no plug/decision field at all** — it's
edge-condition-driven, with no way to ever supply one. This is
**structural, not case-dependent**: no graph-authored `GatewayXor`
diverging node can ever satisfy this grammar, regardless of content.

The D5.0 design note's §0 grammar check verified only that `SplitAst.join`/
`JoinAst.split` round-trip as plain fields — it never checked `:plug`/
per-flow-`:condition` mandatoriness, so this genuinely wasn't visible
until the harness ran a real fixture through it (verify, don't infer,
working as intended — same class of self-caught defect as D4's
`loop_origin`).

Surfaced to Adam with two options (always-refuse vs. loosen the grammar).
**Ruled: always refuse (option 1).** `project_ir` unaffected (no grammar
dependency); the Converging/Join side keeps working (harmless, likely
unreachable in a well-formed graph since the diverging refusal fires
first in canonical topological scan order, but implemented correctly and
symmetrically with AND for any future graph shape).

## Red→green trace

**Green (`ir_plan.rs`):**
- `matched_xor_gateway_pair_projects_to_split_join` — a matched
  diverging/converging `GatewayXor` pair (one branch Eq-conditioned, one
  default) projects to `Split{mode: Exclusive}`/`Join{mode: Exclusive}`
  via `project_ir`, replacing the now-obsolete
  `xor_gateway_is_refused_not_guessed` red test.

**Green (`b2_roundtrip_receipts.rs`):**
- `g18b_xor_split_still_projects_to_plan` — `project_ir` succeeds on a
  matched XOR pair; confirms the emission refusal (below) is specific to
  the unprintable direction, not a graph-admission defect.

**Red, exact-variant (never bare `is_err`):**
- `unmatched_xor_gateway_is_refused_not_guessed` (`ir_plan.rs`) — an
  unmatched diverging `GatewayXor` (no paired converging node) refuses
  `IrPlanError::UnmatchedGateway` — the oracle's mispair guard applies to
  XOR identically to AND/Inclusive.
- `g18_xor_split_refuses_at_emission` (`b2_roundtrip_receipts.rs`) — a
  fully matched, well-formed XOR split/join pair still refuses at
  `emit_dsl` (message asserts both the node id and "GatewayXor") — the
  structural, content-independent refusal from Fork 2.
- `g19_unmatched_converging_xor_gateway_refuses_at_emission`
  (`b2_roundtrip_receipts.rs`) — a converging `GatewayXor` with no
  diverging counterpart anywhere in the graph refuses `UnmatchedGateway`
  at emission (constructed with no diverging XOR node at all, since a
  diverging one would refuse first via Fork 2's disposition — this
  isolates the Converging arm's own pairing check).
- `red_service_task_...` and other pre-existing D1-D4 red tests: untouched,
  still green (no regressions).

No new refusal AXIS beyond the two new named variants — this isn't a new
grammar form (fork C doesn't apply), only the emission-arm logic and the
projection arm's kind-list extension.

## Mechanical blast radius

Adding `direction` to `IRNode::GatewayXor` broke every exhaustive struct
literal without `..` (`E0063`) and one match pattern (`E0027`,
`runbook.rs`). Smaller than D4's ~90-site sweep, as the D5.0 design note
predicted (`GatewayXor` is far less used than `ServiceTask`):

- Real (non-test) producers, direction inferred (not fabricated):
  `bpmn-lite-compiler/src/parser.rs` (BPMN-XML importer, post-pass),
  `bpmn-lite-authoring/src/dto_to_ir.rs` (DTO→IR, post-pass).
- Test/fixture construction sites, explicit `GatewayDirection::Diverging`/
  `Converging` per fixture's own shape:
  `bpmn-lite-compiler/src/lowering.rs` (2 sites),
  `bpmn-lite-compiler/src/verifier.rs` (2 sites),
  `designer-graph/src/ops.rs` (2 sites),
  `designer-graph/src/schema.rs` (4 sites),
  `utterance-engine/src/fixtures.rs` (1 site, new import added).
- One match-pattern site: `bpmn-lite-server-designer/src/runbook.rs` — the
  runbook-rendering `ir_node_sexpr` gained a `:direction` field in its
  printed form, mirroring `GatewayAnd`'s existing rendering (was `..`-free
  and would have silently dropped the new field otherwise — fixed to
  render it, not just pattern-match it away).

No blanket rustfmt run this time (learned from D4's reverted first pass) —
every touched file diffed individually against `git diff` before running
tests; the final diff is 13 files, 413 insertions(+), 46 deletions(-), all
attributable to this tranche.

## Public-API baseline

Diff: **+3 lines, −0 removals**:
- `pub bpmn_lite_compiler::IRNode::GatewayXor::direction:
  bpmn_lite_compiler::GatewayDirection`
- `pub bpmn_lite_compiler::dsl::DslEmitError::GatewayXorSplitUnrepresentable`
  (+1 field: `id`)

- **Consumer:** `designer-graph` (B2, `project_ir`/`emit_dsl` callers),
  `bpmn-lite-authoring` (`dto_to_ir`), `bpmn-lite-server-designer`
  (dsl-receipt endpoint, runbook rendering, transitively).
- **Owning facade:** `bpmn_lite_compiler::{IRNode, dsl}`, unchanged.
- **Stability contract:** additive-only; no emitted-source change for any
  existing graph (no prior graph carried a `GatewayXor` that projected or
  emitted anything, since it was previously entirely out of core on both
  paths).
- **Reason:** minimum public surface to admit `GatewayXor` into
  `project_ir`'s core and to name the Diverging-side structural refusal.

## Verification sweep (all green before commit)

- `cargo build --workspace --all-targets` — 0 errors
- `cargo test -p bpmn-lite-compiler` — 228 passed, 0 failed (unchanged
  count from post-D4: `ir_plan.rs`'s red `xor_gateway_is_refused_not_guessed`
  was replaced by two tests, `matched_xor_gateway_pair_projects_to_split_join`
  (green) and `unmatched_xor_gateway_is_refused_not_guessed` (red) — net
  +1 — while `emit.rs`'s "every out-of-core kind" red test lost one Vec
  entry (GatewayXor moved into core) without losing a whole `#[test]`,
  and one other emit.rs test's fixture kind changed (GatewayXor →
  GatewayInclusive) without changing the test count — net 0 there. Total:
  +1, i.e. 227→228.)
- `cargo test -p designer-graph --all-targets` — 96 passed, 0 failed (was
  94 before D5; +2 net: g18/g18b/g19 replace the originally-planned
  single g18, g19 rewritten mid-tranche per Fork 2's ruling)
- `cargo test -p bpmn-lite-authoring --all-targets` — 69 passed, 0 failed
  (untouched pass count; `dto_to_ir`'s new post-pass exercised
  transitively by existing importer/DTO round-trip tests, no regressions)
- `cargo test --workspace` — **124 `test result: ok` lines, 0 failures
  anywhere** (full sweep, not sampled)
- `scripts/check-semantic-gameboard-boundaries.py` — pass (after baseline
  regen; verified `EXIT:0` directly, not inferred from stdout content)
- `scripts/check-test-only-pub.py` — pass (0 items)

## Blind-review disposition

Independent authorship-blind review of commit `f998f3e` returned
**ACCEPT**. The reviewer independently re-derived every claim from
primary sources rather than trusting the receipt's prose: ran
`cargo build --workspace --all-targets` and `cargo test --workspace`
directly (confirmed 124 test-result lines, all "0 failed", not inferred
from a grep exit code); read `gateway_pairs`' new match arm and confirmed
it doesn't relax the existing mispair guard; read `project_ir`'s combined
match arm and confirmed GatewayXor gets fully symmetric treatment; read
`dsl/parser.rs::parse_split` directly and independently confirmed `:plug`
is unconditionally mandatory for Xor/Or (the highest-priority item — the
central asymmetric-refusal claim); read all three new/changed B2 fixtures
node-by-node and confirmed g18 tests a genuinely matched pair (not an
unmatched one), g18b calls `project_ir` (not `emit_dsl`), and g19's graph
contains zero diverging `GatewayXor` nodes so it isolates the Converging
arm's own `UnmatchedGateway` path rather than accidentally re-testing the
Diverging refusal; confirmed both direction-inference post-passes
(`parser.rs`, `dto_to_ir.rs`) run after all edges are wired and use the
identical inference rule; sampled 5 of the mechanical touch-up sites and
confirmed no smuggled logic; ran `cargo public-api` live and confirmed it
matches the committed baseline exactly; confirmed zero changes to
`bpmn-lite-authoring/src/importer.rs`'s `find_corresponding_join`
(out-of-scope per the design note).

One non-blocking wording nit: the `UnmatchedGateway` error message said
"only GatewayAnd/GatewayXor diverging/converging pairs are emittable,"
which overstates the XOR case — only its Converging half can ever reach
that message (Diverging refuses earlier, unconditionally, via
`GatewayXorSplitUnrepresentable`). Fixed post-review: the message now
names the asymmetry explicitly rather than implying XOR pairs are
emittable as pairs. Re-verified green after the fix (`cargo test -p
bpmn-lite-compiler -p designer-graph`: 228 + 96 passed, 0 failed).

## STOP

D5 is code-complete, blind-reviewed (ACCEPT + one cosmetic fix), pending
Adam's acceptance. D6 (Inclusive gateway alignment, the final tranche in
the ratified programme sequence) begins only after Adam accepts this
gate.
