# Gate 8 bullet 7 follow-on — correction_history.rs can now reach MAX_HISTORY_ATTEMPTS

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification.

Entry authority: `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14 ("Gate 8" bullet
7). `docs/receipts/semantic-gameboard-phase8-resource-abuse-corpora-2026-08-10.md`
closed one decode target (`rule_explanation_decode.rs`) and named
`correction_history.rs` as still open: capped at `MAX_STEPS = 24`, well under
`MAX_HISTORY_ATTEMPTS = 64`, and explicitly flagged that raising it "requires
teaching the harness's correctness model about the size dimension too, which
was judged out of scope for [that] pass rather than risking a rushed, subtly
wrong assertion under time pressure." Picked up here with the time to do it
properly.

## What changed

`utterance-engine/src/history.rs`: `MAX_HISTORY_ATTEMPTS`/`MAX_HISTORY_BYTES`
were `pub(super)` (crate-internal only) — a separate crate like
`utterance-engine-fuzz` cannot see them at all, so this fuzz target could only
have hardcoded `64` with no compiler-enforced link to the real constant.
Widened to `pub` and re-exported through `lib.rs`
(`pub use history::{MAX_HISTORY_ATTEMPTS, MAX_HISTORY_BYTES};`), the same
pattern already used for `resolver_comparison`'s `MAX_OFFLINE_*` constants
consumed by `model_boundary.rs`. Public-API baseline updated deliberately (2 new
reviewed items, `cargo public-api` diffed before/after to confirm nothing
else moved).

`utterance-engine/fuzz/fuzz_targets/correction_history.rs`:
- Raised `MAX_STEPS` from 24 to 96 — comfortably above
  `MAX_HISTORY_ATTEMPTS` (64) with margin, not pushed further since total
  per-iteration work is O(steps²) (`project_bpmn_attempt_history` revalidates
  the whole slice every step).
- The harness's own `reference_valid` function models acyclic correction-chain
  correctness only — it has no notion of tape size. Past
  `MAX_HISTORY_ATTEMPTS`, `history::project`'s size check runs **before**
  `validate_attempt_history`, so a tape can be simultaneously
  acyclic-correct-by-the-reference-model and still legitimately refused for
  being too large. The prior binary `Ok => expect_valid` / `Err =>
  expect_invalid` assertion structure could not express this — it would
  either falsely fail on a valid-but-oversized tape, or (worse) silently pass
  by coincidence without checking *which* error fired. Restructured into a
  three-way check: `over_resource_limit = attempts.len() >
  MAX_HISTORY_ATTEMPTS` is tracked alongside `expected_valid`; an `Ok` result
  now also asserts `!over_resource_limit`; an `Err` result asserts the error is
  specifically `BpmnBoardError::ResourceLimit(_)` when over the limit (not just
  "some error"), and falls back to the acyclic-correctness assertion only when
  under it.

## Verification

- `cargo +nightly check --all-targets` (fuzz sub-workspace): clean.
- Handcrafted a 96-byte all-zero input (every step: no correction, outcome
  `RejectedByUser`) and ran it directly (`-runs=1`): executed in 98ms, no
  assertion failure, no crash — proves steps 65-96 (past the resource limit)
  are reached and pass the new three-way check without a spurious "reference
  model disagreement" failure.
- `cargo +nightly fuzz run correction_history -- -max_total_time=15
  -max_len=96`: 710 real coverage-guided runs at the new max length, 0
  crashes.
- A separate 15-second run without an explicit `-max_len` (seeded from the
  existing small corpus): 7,070 runs, 0 crashes — confirms the raised
  `MAX_STEPS` doesn't destabilize the target at smaller, more common input
  sizes either.

## Results

- `cargo test -p utterance-engine --all-features`: all passing, 0 failed.
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, against the
  updated baseline (2 new reviewed items:
  `pub const utterance_engine::MAX_HISTORY_ATTEMPTS`,
  `pub const utterance_engine::MAX_HISTORY_BYTES`).
- `python3 scripts/check_fuzz_regressions.py`: pass, unaffected (3 governed
  regression cases, unchanged).

## Scope note

`legal_move_enumeration.rs` (capped at 0-4 tasks, can't reach
`MAX_ENUMERATION_CANDIDATES`/`MAX_LEGAL_MOVES`) and `semantic_board_decode.rs`
(decodes an unrelated pre-gameboard contract type) remain open, as named in the
prior receipt — not addressed here.
