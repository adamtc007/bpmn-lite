# Gate 8 bullet 7 — third and final named target: legal_move_enumeration.rs

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification.

Entry authority: `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14 ("Gate 8" bullet
7). The prior receipt named `legal_move_enumeration.rs` as still open,
self-capped at 0-4 generated tasks — far below either new resource-limit cap —
and speculated that widening it "requires also widening the reference model."
That speculation turns out to be only half right, discovered by actually doing
the work rather than continuing to defer it.

## What the earlier speculation got wrong, and right

`ReferencePosition::candidates_at` (the fuzz target's own independent model) is
keyed purely by anchor *role* (`start`/`end`/task), not by graph size — every
generated task node already uses the identical branch regardless of
`task_count`, so the reference model generalizes to any graph size for free.
No reference-model change was needed.

What the earlier receipt didn't anticipate: this fixture's final loop
reconstructs a full board+position at **every** anchor, once per fuzz
execution, and does so for a graph that grows with a fuzzer-controlled task
count — a design that trades correctness-model precision for per-iteration
cost that scales worse than linearly. Measured directly: 459ms for a single
execution at 63 task nodes, ~4.9 seconds at 339. Reaching
`MAX_ENUMERATION_CANDIDATES` (4096) needs roughly 316 task nodes on this graph
family — squarely in the multi-second-per-iteration zone, which would make
sustained fuzzing here impractically slow, not incorrect.

## A real, independent bug found along the way

Raising the generated task count exposed a **pre-existing** collision in
`build_graph`'s own key allocation, invisible until now because `task_count`
was capped at ≤4: task node keys are `key(2 + index)` for `index in
0..task_count`, but the end node was hardcoded to `key(100)`. At `task_count`
≥ 99, `key(100)` collides with the task node at index 98 — `apply_production`
then panics on an internal `unwrap()`, a crash entirely in the fuzz fixture's
own scaffolding, not in production code. Fixed: `end_key = key(1_000_000)`,
comfortably clear of the task-key range at any task count this target could
plausibly reach.

## Two distinct resource limits, discovered to interact

A second, more interesting finding while first raising the graph size all the
way toward `MAX_ENUMERATION_CANDIDATES`: **`MAX_LEGAL_MOVES` (512, the
contract-layer cap on an admitted `DesignPosition`) binds long before
`MAX_ENUMERATION_CANDIDATES` (4096, `enumerate`'s own amplification cap) does**,
for this fixture. Its empty `PolicyFilter` never filters a raw candidate out of
admission, so admitted legal moves track considered candidates ~1:1 — meaning
`enumerate()` itself succeeds (never hits its own 4096-candidate counter) well
past 512 admitted moves, and it is `DesignPosition::new` that refuses first,
with the *other* typed error
(`GameboardContractError::ResourceLimitExceeded`, wrapped as
`BpmnBoardError::Gameboard(...)`, not `BpmnBoardError::ResourceLimit(...)`). A
first version of this change's assertion logic only modeled the enumeration
layer and crashed immediately on this — corrected by adding
`ExpectedWholeGraphOutcome` (`Admitted` / `EnumerationLimit` / `LegalMoveLimit`)
and asserting the *specific* variant expected at each zone, not "any error."

## Scope decision: reach MAX_LEGAL_MOVES, not MAX_ENUMERATION_CANDIDATES, here

Given the throughput cost of reaching `MAX_ENUMERATION_CANDIDATES` on this
fixture (~5s/iteration), and that `MAX_LEGAL_MOVES` is the practically-binding
constraint for it anyway, the fixture's task-count range was set to `% 64`
(needs ~40 tasks to cross 512 admitted moves) rather than the ~316+ needed to
reach `MAX_ENUMERATION_CANDIDATES`. `MAX_ENUMERATION_CANDIDATES` remains
unreached by this fuzz target — but it is already independently, deterministically
verified by
`legal_moves::tests::enumeration_amplification_beyond_the_limit_is_a_typed_resource_limit_refusal`
(Gate 8 bullet 4's own unit test: a single-construction 600-node fixture with
none of this target's per-anchor reconstruction multiplier), so the cap is not
untested — just not reached by *this specific* fuzz target's differential
methodology.

## What changed

- `utterance-engine/src/legal_moves.rs`: `MAX_ENUMERATION_CANDIDATES` widened
  from crate-private to `pub`, re-exported through `lib.rs` (same pattern as
  `MAX_HISTORY_ATTEMPTS`/`MAX_HISTORY_BYTES` in the prior receipt).
- `utterance-engine/fuzz/fuzz_targets/legal_move_enumeration.rs`:
  - `task_count` modulus raised from 5 to 64.
  - Fixed the `end_key` collision bug.
  - Added `whole_graph_candidate_count`, `ExpectedWholeGraphOutcome` and
    `expected_whole_graph_outcome`/`assert_matches_expected_outcome` helpers,
    replacing three blind `.unwrap()`s on `build_bpmn_design_position` (which
    would otherwise crash the fuzzer on every legitimate refusal) with
    zone-aware assertions that check the *specific* error variant expected.

## Verification

- `cargo +nightly check --all-targets`: clean.
- Reproduced the original `end_key` collision crash
  (`task_count=339`, single input, `-runs=1`) before the fix — confirmed it was
  the fixture bug, not a false positive.
- Reproduced the `MAX_LEGAL_MOVES`-vs-`MAX_ENUMERATION_CANDIDATES` assertion
  gap (`task_count=85`, single input, `-runs=1`) before correcting the
  assertion model — confirmed the fix resolves it and both zones now assert
  correctly.
- Both prior crash inputs rerun clean after the fixes (`-runs=1`, no crash).
- Two real fuzzing bursts at the final `% 64` range (21s/211 runs, then a
  fresh 31s/345 runs): 0 crashes in either, no lingering crash artifacts.

## Results

- `cargo test -p utterance-engine --all-features`: all passing, 0 failed.
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, against the
  updated baseline (1 new reviewed item:
  `pub const utterance_engine::MAX_ENUMERATION_CANDIDATES`).
- `python3 scripts/check_fuzz_regressions.py`: pass, unaffected.

## Gate 8 bullet 7 — final status

All three named-open decode/differential fuzz targets from the original survey
are now disposed of:
- `rule_explanation_decode.rs`: closed (widened, reaches `MAX_CONTRACT_TEXT_BYTES`).
- `correction_history.rs`: closed (widened, reaches `MAX_HISTORY_ATTEMPTS`).
- `legal_move_enumeration.rs`: closed for `MAX_LEGAL_MOVES`; deliberately does
  not reach `MAX_ENUMERATION_CANDIDATES` for a documented performance reason,
  with that cap independently covered by a unit test instead.
- `semantic_board_decode.rs`: still correctly out of scope — decodes an
  unrelated pre-gameboard contract type this session's caps never touch.

PostgreSQL fault tapes (closed earlier) and native/Wasm/Python differential
testing (ruled N/A, v0.10 amendment) complete Gate 8 bullet 7's own
sub-clauses. Bullet 7 is now closed in substance, with `semantic_board_decode.rs`'s
non-applicability the only remaining named exclusion — not an unaddressed gap.
