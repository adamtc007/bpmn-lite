# Receipt — EOP-PLAN-DSL-PARITY-001 Gate D2.1: `repeat_n_times` multi-predecessor fix

**Status:** awaiting acceptance
**Branch:** `codex/bpmn-gameboard-refactor`
**Scope:** fixes the multi-predecessor splice defect the D2 blind review surfaced
and explicitly declined to fix inline (recorded in
`docs/receipts/EOP-DSL-PARITY-001-D2-receipt.md`'s disposition table as a
separate fork). Ruled via plan approval ("accepted go" + the presented plan) to
fix now, before opening D3.

## The defect

`bpmn-lite-compiler/src/dsl/repeat.rs::repeat_n_times`, when the wrapped task has
more than one predecessor (a diamond: two branches both flowing directly into the
task), rewired every predecessor to `exit_next` first, then separately picked
ONE of them (the first found pointing at `exit_next`) as the `insert_after`
anchor — only that one predecessor ended up pointing at the new loop; every
other predecessor was left pointed at `exit_next`, silently bypassing the retry
for callers reaching the task via that branch. No error was raised; the result
compiled cleanly. Reachable via the REST `apply_dsl_macro` BoundedRetry path.

## The fix

Every predecessor of the wrapped task is now rewired directly to the loop's id
(not to `exit_next`), in the same pass that used to rewire them to `exit_next`.
The loop is then injected into the AST via `AstMutator::inject_into_same_scope`
(promoted from private to `pub(crate)`, reused rather than duplicated) at the
scope of one already-rewired predecessor — injection position is a text-layout
choice only, since flow is graph-driven via `next`/`id`, not source order.
`insert_after` is no longer used here: its own anchor-rewire step would be
redundant (every predecessor is already rewired) and semantically wrong to
reapply.

## Red→green trace

- `repeat_n_times_rewires_every_predecessor_in_a_diamond` (NEW): a split-and
  diamond (`branch-a`, `branch-b` both `:next charge`) — asserts BOTH branches
  now point at `charge-loop` (previously only one would), plus a full recompile
  of the rewired workflow.
- `repeat_n_times_rewires_a_guard_escape_predecessor_onto_the_loop` (STRENGTHENED,
  named cement update): the D2-review placeholder assertion `assert_ne!(g1.next,
  "charge")` — deliberately weak, with a "do not strengthen until ruled" note —
  is now `assert_eq!(g1.next, "charge-loop")`, matching every other predecessor
  kind.
- `repeat_n_times_rewires_a_timer_wait_predecessor`,
  `repeat_n_times_wraps_the_task_and_preserves_predecessor_and_exit_routing`,
  `repeat_n_times_refuses_a_missing_target` — unchanged, still green (confirms
  the fix is behavior-preserving for the single-predecessor case).

## Public-API impact

None. `inject_into_same_scope` is `pub(crate)`, not `pub` — invisible to
`cargo public-api`. Boundary gate confirmed clean (no drift reported).

## Verification sweep (all green before commit)

- `cargo test -p bpmn-lite-compiler` — 216 passed, 0 failed (was 215; +1 net:
  +1 new diamond test, 0 removed, 1 renamed+strengthened)
- `cargo test -p designer-graph` — 90 passed, 0 failed (untouched)
- `cargo test -p bpmn-lite-server-designer` — 98 passed, 0 failed, 1 ignored (untouched)
- `cargo test -p bpmn-lite-authoring` — 69 passed, 0 failed (untouched)
- `cargo check --workspace --all-targets` — 0 errors
- `scripts/check-semantic-gameboard-boundaries.py` — pass, no baseline change needed
- `scripts/check-test-only-pub.py` — pass

## STOP

D3.0 (MultiInstance freeze) begins only after Adam accepts this gate.
