# Receipt — EOP-PLAN-DSL-PARITY-001 Gate D4, Piece 1: loop provenance IR carriage (fork E)

**Status:** RATIFIED (Adam, 2026-08-14, "accepted"). Blind review of commit
`e2d6f98` returned **ACCEPT**, no corrections — see disposition below.
**Branch:** `codex/bpmn-gameboard-refactor`
**Design note:** `docs/receipts/EOP-DSL-PARITY-001-D4.0-design.md`, ratified
"proceed carrier only" (Adam, 2026-08-14) — fold-back explicitly deferred,
carrier-only scope confirmed.

## What was built

`IRNode::ServiceTask` gains a `loop_origin: Option<String>` field — the
IR-side twin of `dsl::ast::TaskAst::loop_origin` /
`dsl::plan::TaskExecNode::loop_origin`, which `unroll::unroll_loops`
already stamps on the DSL path. Before this tranche the field had no
IR-side carrier at all; `project_ir` and `emit_dsl` both hardcoded
`loop_origin: None` as a literal, not a read.

- `ir.rs`: new field, `#[serde(default)]` for the same forward-compat
  reason `TaskExecNode`'s twin uses.
- `ir_plan.rs` (`project_ir`, graph→plan): reads the field and passes it
  through honestly instead of a fabricated `None`.
- `emit.rs` (`emit_dsl`, graph→DSL text): **does not** pass it through.
  See "The defect this tranche caught in itself" below — it refuses via a
  new `DslEmitError::LoopOriginUnrepresentable` instead.

No graph-side operation sets this field today — `designer-graph`'s
`Operation` enum has no loop-authoring op, and `DSL->graph import` (the
only path that could ever produce one) stays out of this programme's
scope per the ruled V&S. Every existing graph still projects/emits
`loop_origin: None` exactly as before; this tranche is additive plumbing
with zero behavior change for any graph that exists today.

## The defect this tranche caught in itself

The D4.0 design note's Piece 1 scope, as ratified, was "pass through
honestly" on both `project_ir` and `emit_dsl`. The first implementation
did exactly that — and this tranche's own B2 plan-equality fixture (g17)
failed immediately:

```
left  (DSL-compiled plan): Task { ..., no loop_origin field ... }
right (project_ir plan):   Task { ..., loop_origin: "retry-loop" ... }
```

Root cause, verified against the primary source: `TaskAst.loop_origin`
has **no `ToSexpr` grammar surface** — `refactor.rs`'s `ToSexpr` impl for
`TaskAst` never prints it (confirmed: it was never meant to be
authorable; `unroll::unroll_loops` is the only writer, and it runs
*before* printing, within one compile pass, never after reparsing — see
`ast.rs`'s own doc comment on the field, unchanged by this tranche).
So a naive `emit_dsl` pass-through would silently print DSL source that,
recompiled, yields a plan **missing** the very provenance
`TaskExecNode.loop_origin` exists to carry (and that `closure.rs`'s L6
idempotency-inside-a-retried-task check depends on). That is exactly the
"lossy silent drop" the working contract's "no trap doors" rule forbids —
the field would have round-tripped `Some → printed-without-it → None` on
any graph→DSL→recompile cycle.

Fixed by NOT printing it: `emit_dsl`'s `ServiceTask` arm now refuses with
a new named `DslEmitError::LoopOriginUnrepresentable { id, loop_origin }`
when the field is `Some`, mirroring D3's `InputsUnrepresentable`
precedent (refuse rather than silently drop plan-relevant data with no
grammar surface). `project_ir`'s pass-through is unaffected — that
direction never goes through DSL text, so there is no printability
constraint on it.

This was caught by the tranche's own B2 fixture, not by review — the
"receipts or it isn't done" / "fail closed" disciplines working as
intended: the harness rejected a design that looked correct in isolation
the moment it was checked against the real four-proof contract.

## Red→green trace

**Green:**
- `dsl::ir_plan::tests::service_task_loop_origin_projects_through`
  (`ir_plan.rs`): a hand-built `IRNode::ServiceTask` with
  `loop_origin: Some("retry-loop")` projects to
  `TaskExecNode.loop_origin == Some("retry-loop")` — `project_ir` no
  longer hardcodes `None`.
- `g17b_service_task_loop_origin_still_projects_to_plan`
  (`b2_roundtrip_receipts.rs`): the same, through the real
  `DesignerDag::to_ir()` → `project_ir` path — confirms the refusal below
  is specific to the DSL-text direction, not a graph-admission defect.
- `dsl::closure::tests::test_l6_idempotency_check_survives_unrolling_via_loop_origin`
  (pre-existing, untouched): confirms L6 still reads `loop_origin` off the
  plan-side field, unaffected by the IR-side addition.

**Red (exact-variant, never bare `is_err`):**
- `dsl::emit::tests::red_service_task_loop_origin_unrepresentable`
  (`emit.rs`): a hand-built `IRNode::ServiceTask` with
  `loop_origin: Some("retry-loop")` refuses `emit_dsl` with exact
  `LoopOriginUnrepresentable { id: "t1", loop_origin: "retry-loop" }`.
- `g17_service_task_loop_origin_refuses_at_emission`
  (`b2_roundtrip_receipts.rs`): the same, through the real
  `DesignerDag::emit_dsl()` path — asserts the refusal message names both
  "loop provenance" and the node id.

No new refusal AXIS beyond the one new named variant — this isn't a new
grammar form (fork C doesn't apply; there is no new DSL head), so there's
no parse/lint layer to extend, only the emission-arm refusal itself.

## Mechanical blast radius

Adding a field to `IRNode::ServiceTask` breaks every exhaustive struct
literal across the workspace (no `..` rest pattern) — Rust's `E0063`.
~90 test/fixture construction sites across `bpmn-lite-compiler`,
`bpmn-lite-authoring`, `designer-graph`, `bpmn-lite-server-designer`,
`utterance-engine`, and `xtask` needed `loop_origin: None,` added; 3 match
patterns (`ir_to_dto.rs`, `bpmn_board.rs`, `runbook.rs`) needed a `..` rest
pattern added (`E0027`). Every site is a mechanical, single-field,
behavior-preserving addition — verified by full-workspace green, not by
inspection alone. (First pass ran a workspace-wide `rustfmt`, which
reflowed thousands of unrelated lines across `lowering.rs`/`verifier.rs`/
`rest.rs` — reverted; the final diff touches only the lines this tranche
actually changed.)

## Public-API baseline

Diff: **+4 lines, −0 removals**:
- `pub bpmn_lite_compiler::IRNode::ServiceTask::loop_origin:
  core::option::Option<alloc::string::String>`
- `pub bpmn_lite_compiler::dsl::DslEmitError::LoopOriginUnrepresentable`
  (+2 fields: `id`, `loop_origin`)

- **Consumer:** `designer-graph` (B2, `project_ir`/`emit_dsl` callers),
  `bpmn-lite-server-designer` (dsl-receipt endpoint, transitively).
- **Owning facade:** `bpmn_lite_compiler::{IRNode, dsl}`, unchanged.
- **Stability contract:** additive-only per the D4.0 design note's ruled
  scope; no emitted-source change for any existing graph.
- **Reason:** minimum public surface for the IR-side loop-provenance
  carrier plus its one new named refusal.

## Verification sweep (all green before commit)

- `cargo build --workspace --all-targets` — 0 errors
- `cargo test -p bpmn-lite-compiler` — 227 passed, 0 failed (was 225
  after D3; +2: `service_task_loop_origin_projects_through`,
  `red_service_task_loop_origin_unrepresentable` — the pre-existing L6
  idempotency test was already counted pre-D4, untouched by this tranche)
- `cargo test -p designer-graph --all-targets` — 93 passed, 0 failed (was
  91 after D3; +2: g17, g17b)
- `cargo test -p bpmn-lite-authoring --all-targets` — 69 passed, 0 failed
  (untouched)
- `cargo test -p bpmn-lite-server-designer --all-targets` — 99 passed, 0
  failed, 1 ignored (untouched)
- `cargo test -p utterance-engine --lib --tests` — 102 passed, 0 failed
  (untouched; `--bench gameboard_perf` fails identically under
  `cargo test`'s debug profile on the unmodified baseline too — confirmed
  via `git stash` — a pre-existing debug-vs-release budget mismatch, not a
  D4 regression; passes under `cargo bench --release`)
- `cargo test --workspace` — 0 failures anywhere
- `scripts/check-semantic-gameboard-boundaries.py` — pass (after baseline
  regen)
- `scripts/check-test-only-pub.py` — pass (0 items)

## Blind-review disposition

Independent authorship-blind review of commit `e2d6f98` returned
**ACCEPT**. The reviewer independently re-derived every claim rather than
trusting the receipt prose: re-ran `cargo build --workspace --all-targets`
and confirmed 0 errors; ran all four new tests directly and read their
actual assertions rather than trusting pass counts; read `refactor.rs`'s
real `impl ToSexpr for TaskAst` to independently confirm `loop_origin` has
no printable grammar surface — the reasoning behind the refusal, not just
its existence; diffed the live `cargo public-api` output against the
committed baseline (exact match, 531 lines each) and confirmed the
committed baseline's own diff from D3 is exactly the claimed +4/−0;
sampled 9 of the ~28 mechanically-touched files plus `lowering.rs`/
`verifier.rs` and confirmed every hunk is a single-field
`loop_origin: None,` addition or a `..` rest-pattern addition, no smuggled
logic changes; grepped the full commit for any `LoopAst`
construction/reconstruction code and found none — confirming fold-back
stayed out of scope as ratified. No discrepancies found; no corrections
made.

## STOP

D4 Piece 1 (carrier-only) is code-complete. Fold-back (folding
loop-provenance-marked copies back into `LoopAst` at emission) remains
explicitly OUT of this tranche per the ratified D4.0 design note — it has
no current producer to justify the added complexity, and the three
fidelity gaps identified there (non-Task loop-body members untagged,
nested-loop id collapse, inferred exit edge) are unresolved. D5 (XOR join
oracle design note) begins only after Adam accepts this gate.
