# Design note — EOP-PLAN-DSL-PARITY-001 Gate D4: loop provenance IR carriage (fork E)

**Status:** DRAFT — presenting the gate's own ruling item, costed, per the
plan doc's D4 text ("The tranche presents both costed; the gate rules.").
No code written. STOP for Adam's ruling before any implementation.

## 0. Source-of-truth verification (read directly, not from memory)

- `bpmn-lite-compiler/src/dsl/ast.rs:107-120` `TaskAst.loop_origin:
  Option<String>` — stamped ONLY by `unroll.rs::clone_node_iteration`'s
  `NodeAst::Task` arm (`unroll.rs:272-284`), never by the parser
  (`parser.rs:288/321/351` all construct it as `None`).
- `bpmn-lite-compiler/src/dsl/plan.rs:379-384` `TaskExecNode.loop_origin:
  Option<String>` — the plan-side twin, populated from `n.loop_origin` in
  `linter.rs:451` when lowering `NodeAst::Task`.
- `bpmn-lite-compiler/src/ir.rs:70-74` `IRNode::ServiceTask { id, name,
  task_type }` — **no `loop_origin` field**. This is the missing carrier
  fork-E names.
- `bpmn-lite-compiler/src/dsl/ir_plan.rs:254-271` (`project_ir`, graph→plan)
  hardcodes `loop_origin: None` at line 269 because there is nothing on
  `IRNode::ServiceTask` to read.
- `bpmn-lite-compiler/src/dsl/emit.rs:587-604` (`emit_dsl`, graph→DSL)
  hardcodes `loop_origin: None` at line 602, same reason, reverse direction.
- `bpmn-lite-compiler/src/dsl/closure.rs:526-551`, specifically line 539
  (`let is_inside_loop = t.loop_origin.is_some();`) — the ONE consumer of
  this field today (L6, the idempotency-inside-a-retried-task check). It
  needs only a boolean ("is this task inside *a* loop"), never `ceiling`,
  sibling order, or the loop's original exit edge.

## 1. Does the graph side have anything to carry loop provenance FROM, today?

No. `designer-graph/src/ops.rs`'s `Operation` enum has no loop-shaped
variant and no unrolling mechanism exists on the graph/REST-authored path
at all (verified: no "loop" hits anywhere in `designer-graph/src/*.rs`).
The nearest graph-side construct, `CreateMultiInstanceRegion` →
`IRNode::MultiInstance`, is a single node representing "repeat this one
activity N times" — structurally nothing like unrolling's N
provenance-linked sibling copies, and irrelevant to this carrier (D3
closed that vertical separately).

`DSL→graph import` is explicitly excluded from this whole programme
(`EOP-VS-DSL-PARITY-001.md` §3). That is the only path that could ever
produce a graph containing loop-unrolled, `loop_origin`-stamped
`ServiceTask` copies. **Consequence: adding the IR-side carrier has no
current producer.** It is honest plumbing for a future path, not something
that changes any live graph's behavior today — which bears directly on
the fold-back question below.

## 2. Piece 1 — the carrier itself (uncontroversial; proceeding on this regardless of the fold ruling)

- Add `loop_origin: Option<String>` to `IRNode::ServiceTask` (`ir.rs`),
  `#[serde(default)]` for the same forward-compat reason `TaskExecNode`'s
  copy already uses.
- `project_ir` (`ir_plan.rs:269`): read `task_type`'s node for the field
  instead of hardcoding `None` — becomes a pass-through, not a fabricated
  literal. No behavior change today (every existing graph has no producer,
  so every read is still `None`) — this closes a latent gap, not a live bug.
- `emit_dsl` (`emit.rs:602`): read the same field into `TaskAst.loop_origin`
  instead of hardcoding `None` — closes the reverse latent gap: if a future
  graph-side producer ever DOES stamp this field, re-emitted DSL will no
  longer silently discard it (today it would, since `emit.rs` throws it
  away even if the IR carried it — except the IR can't carry it yet, hence
  "latent").
- This is a **pure additive plumbing change**: no `NodeAst` variant, no
  grammar, no new refusal axis, no B2 fixture behavior change (every
  existing green/red fixture still round-trips `None → None`). Public-API
  diff: `IRNode::ServiceTask` gains one field (breaking for exhaustive
  external constructors, but `IRNode` is `#[non_exhaustive]`-equivalent in
  practice per the existing baseline convention — will confirm against the
  baseline gate at implementation time).

This piece is low-risk and delivers real (if currently dormant) value. I
see no reason to gate it — flagging it here for visibility, not as an open
question.

## 3. The gate's actual ruling item — fold copies back into `LoopAst`?

**Recommendation: NOT NOW — carrier lands for future use only. Do not
attempt folding in this tranche.**

### Why folding is more than a version bump — it is currently lossy/ambiguous by construction, verified against `unroll.rs`

`LoopAst` (`ast.rs:222-228`) is `{ id, ceiling, body: Vec<NodeAst>, next,
span }`. Reconstructing it from a flat, `loop_origin`-stamped plan/IR
would need, per copy: the shared `loop_id` (recoverable — that's exactly
what `loop_origin` is), the original unqualified body-node id (in
principle recoverable by stripping `unroll.rs::qualified_id`'s
`format!("{base_id}__{loop_id}_{index}")` suffix, `unroll.rs:226-228`),
the `ceiling` (recoverable by counting distinct index suffixes — assumes
no gaps), and the loop's own `next` (the post-loop exit target).

Three concrete gaps, not stylistic concerns:

1. **Only `Task` nodes are stamped.** `clone_node_iteration`
   (`unroll.rs:264-371`) clones `TimerWait`, `MultiInstance`,
   `MessageWait`, `Split`, `Join`, `BoundaryTimer`, `BoundaryError` bodies
   too, but none of those `NodeAst` variants carry a `loop_origin` field —
   only `TaskAst` does. A loop body containing anything other than a bare
   chain of tasks cannot be refolded even with the IR-side carrier in
   place, because the non-Task siblings have no provenance tag to
   associate back to the same `loop_id`/index. Fixing this is itself
   several more verticals (one per node kind), not a detail.
2. **Nested loops lose their inner id.** `clone_node_iteration`'s Task arm
   (`unroll.rs:283`, comment 279-282) deliberately preserves only the
   OUTERMOST loop's id: `t.loop_origin.clone().or_else(|| Some(loop_id))`.
   A nested-loop structure folds, at best, into one flat outer loop —
   silently discarding the inner loop's own `ceiling` and boundary. Any
   fold implementation that didn't special-case this would produce a
   `LoopAst` that recompiles to a DIFFERENT unrolled plan than the
   original — exactly the "folded source recompiles to the SAME unrolled
   plan" proof the plan doc's own §D4 item 2 demands, and exactly the kind
   of thing that proof is supposed to catch (correctly conservative
   design), but it means the fold logic itself would need real new
   machinery, not a formatting exercise.
3. **The exit edge is inferred, not tagged.** `remap_next`
   (`unroll.rs:230-246`) substitutes the loop's own `next` (`exit_target`)
   inline wherever a body node's `next` pointed at the loop id — nothing
   marks which rewritten edge WAS that substitution versus an ordinary
   in-body edge. Recovering `LoopAst.next` requires inferring it from
   which copy's `next` doesn't resolve to another same-`loop_id` sibling,
   which is fragile against the very unqualified-id-collision case
   `qualified_id`'s suffix scheme already flags as an edge case.

None of these are unsolvable. They are real design work — a second
freeze-worthy tranche in their own right, not a "gate ruling item" to
settle inline here. And per §1 above, there is currently **no producer**
that would ever hand emission a graph worth folding — so there is no
practical loss in deferring, only a hypothetical one.

### The costed alternative (not recommended, presented per the plan's instruction to cost both)

Folding now would require: (a) extending the loop_origin carrier to every
`NodeAst` variant clonable inside a loop body, not just `Task`; (b)
preserving nested-loop identity (a second provenance field, or a stack/path
encoding, not a single `Option<String>`); (c) an explicit exit-edge tag
rather than inferred substitution; (d) the B2 fold-correctness proof itself
(folded source recompiles to the same unrolled plan) as a new B2 harness
capability; (e) the bridge-contract version-bump note in `emit.rs`'s module
doc header (prose-only today — no machine-checked version field exists,
confirmed by grep) plus a receipt entry per `EOP-PLAN-DSL-PARITY-001.md`
§0 item 3. This is realistically its own multi-piece tranche, not an
increment on D4.

## 4. Proposed D4 scope, pending ruling

1. Piece 1 (§2 above): add the carrier, wire `project_ir` and `emit_dsl`
   to pass it through honestly instead of hardcoding `None`. B2/red-green
   fixtures assert the pass-through is field-identical to today's `None`
   default (no behavior change for any existing graph) plus a
   hand-built-`IRGraph` fixture (same convention as D1-D3's G13/G16-style
   fixtures) proving a `loop_origin`-stamped `IRNode::ServiceTask` DOES
   survive a `project_ir`/`emit_dsl` round trip once one exists.
2. Fold-back: NOT in this tranche. Carrier lands for future use only, per
   the plan doc's own explicit fallback wording ("Not folding = carrier
   lands for future use only."). Re-open as its own gate (own freeze doc)
   only once a real producer of loop-provenance-bearing graphs exists
   (i.e., not before `DSL→graph import` — currently excluded — enters some
   future programme's scope) AND the three gaps in §3 are addressed as
   their own design.

## 5. What this STOPs on

Adam rules:
(a) proceed with Piece 1 as scoped in §4.1 (carrier + honest pass-through,
no folding), or
(b) some other disposition (e.g., skip D4 entirely for now, given it has
no current producer and therefore no current payoff beyond closing a
latent gap).

No code changes have been made. Awaiting ruling before touching `ir.rs`,
`ir_plan.rs`, or `emit.rs`.
