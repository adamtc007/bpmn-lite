# Gate 8 bullet 5 — wrong-move-traffic / disposition-loop resource bound

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification.

Entry authority: `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14 ("Gate 8" bullet 5,
"Expected wrong-move traffic cannot cause unbounded history, feedback recursion or
repeated compiler work"). Carried forward by
`docs/receipts/semantic-gameboard-phase8-gate-2026-08-10.md` as "PARTIALLY
covered, not freshly verified" — the pre-existing coverage
(`MAX_HISTORY_ATTEMPTS`/`MAX_HISTORY_BYTES`, unit-tested in `history.rs`) only
proves the storage-layer bound; no test drove realistic repeated wrong-move
traffic through the disposition/recovery loop end-to-end.

## Test

`utterance-engine/tests/gameboard_disposition.rs::repeated_wrong_move_traffic_keeps_the_disposition_loop_bounded`,
alongside the existing golden-fixture test in the same file (reused its
graph/board/position/evidence construction, left that test untouched).

Drives 400 consecutive turns of a fixture deliberately engineered to be
ambiguous on every turn (same `gameboard-top3.json` evidence fixture the
existing golden test uses to force `ClarifyMoves`/`Ambiguous`), through the real
`update_bpmn_design_belief` -> `decide_bpmn_game_disposition` chain — the same
two calls the REST layer makes per utterance turn. Each turn passes only the
trailing 64-attempt window of the ever-growing tape, mirroring
`design_history_projection`'s (`rest.rs`) production windowing discipline rather
than assuming it.

Asserts, per turn: the disposition is never a silent apply (outcome is never
`MoveAttemptOutcome::Applied`); the disposition kind is one of the two
legitimate non-apply kinds this fixture can produce
(`ClarifyMoves`/`Escalate`). Across all 400 turns: history is never silently
dropped (`full_tape.len() == TURNS`); and total wall-clock for the last quarter
of turns is not more than 5x the first quarter plus a fixed noise floor — a
generous bound whose purpose is to catch a real cost regression (an
accidentally-unbounded caller-side Vec, or a belief/disposition path that
reaches past the passed-in window into the full tape) rather than to pin an
exact number.

## A real finding, not a test artifact

The first version of this test asserted every turn stays `ClarifyMoves`/
`Ambiguous` and failed at a mid-range turn with `left: Escalate, right:
ClarifyMoves`. This is not a bug — it is the disposition policy's own loop
breaker: once repeated identical failure is visible in the trailing window, the
policy stops offering the same clarification forever and escalates instead.
That is architecturally exactly the property Gate 8 bullet 5 is asking to be
proven exists, not merely assumed. The test now asserts escalation is actually
reached at least once within the 400 turns
(`assert!(escalated, ...)`) — turning what looked like a test bug into the
strongest single assertion in the file: the repeated-failure loop breaker is
live, not dead code.

## What this does not claim

- "No repeated compiler work" is not independently exercised by this test
  because it does not need to be: a wrong-move (`Ambiguous`/`Incomplete`/etc.)
  disposition never reaches `materialize_workbook`/`preview_workbook` — those
  are only invoked on a selected, fully-bound move proceeding toward
  ratification. The compiler is architecturally unreachable from this loop, not
  merely observed to be cheap in this run. Recorded here rather than silently
  assumed.
- This is a single-fixture, single-position loop (repeated identical failure on
  one static graph). It does not cover a *mixed* wrong-move workload (rotating
  through different ambiguous/incomplete/rejected/stale outcomes across turns)
  or a growing/mutating graph. The existing per-outcome coverage
  (`semantic-gameboard-phase8-coverage-audit-2026-08-10.md`) already proves every
  outcome kind is individually constructible and exercised; this test's
  contribution is specifically the *repeated-traffic* dimension, not outcome
  coverage.
- The 5x/noise-floor timing assertion is a regression guard, not a ratified
  performance budget — Gate 8 bullet 3 (ratified perf budget) remains separately
  open and is not touched by this receipt.

## Results

- `cargo test -p utterance-engine --test gameboard_disposition`: 2 passed, 0
  failed (existing golden test unchanged; new test green, reconfirmed stable
  across 3 repeated runs).
- `cargo test -p utterance-engine --all-features`: all passing, 0 failed.
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, unchanged (test
  files add no public library surface).
