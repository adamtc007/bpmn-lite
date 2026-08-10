# Semantic gameboard Phase 8 — gameboard property-test tranche

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification
(`docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14).

## Finding that reframed this item's scope

`utterance-engine/src/property_tests.rs` already existed with 6 `proptest!` cases, and
the earlier fuzz-tranche receipt characterized it as covering "~2 of the ~15" Phase 8
property bullets. On inspection, that overstated the overlap: all 6 existing cases test
the **pre-gameboard compatibility surface** (`crate::contract::FiniteScore`,
`SemanticDecisionBoard` from `semantic_decision_contracts` directly, the legacy
`ProposalWorkbook::new(...)` constructor) — not the gameboard's `DesignPosition` /
`GameDisposition` / `GameTurnRecord` model the Phase 8 bullets actually describe. None
of the 15-16 listed property bullets had genuine gameboard-level `proptest!` coverage
before this tranche.

## What this tranche adds

4 new `proptest!` cases in a new `property_tests::gameboard` submodule, each grounded
directly in `DesignPosition::new`'s own documented field lists (read from the
`semantic-decision-contracts` source, not re-derived) so the tests catch a future
accidental change to those lists rather than reimplementing the hashing:

| Case | Phase 8 bullet | Grounding |
|---|---|---|
| `legal_move_set_is_deterministic_and_canonically_ordered_by_move_id` | "legal move set is deterministic and canonically ordered" | `DesignPosition::new` sorts `legal_moves` by `move_id` before hashing; asserts two identical builds are equal and the returned order matches a fresh sort. |
| `move_set_hash_is_sensitive_to_focus_policy_revision_and_profile_drift` | "move-set hash changes with graph/focus/pack/policy drift" | `move_set_hash`'s preimage is `graph_revision, semantic_snapshot, focus_hash, policy_identity, compiler_profile` plus each move id, in that order. Drifts each of graph_revision/focus/policy/compiler_profile independently (single pack fixture, so "pack" drift itself is not exercised - named below as a residual) and asserts the hash changes; additionally proves focus drift changes the hash but never the legal moves themselves, since `legal_moves()` comes from the board's own anchor, not from `DesignFocus`. |
| `history_hash_never_changes_legal_moves_or_move_set_hash` | "history/belief cannot change legality" | `move_set_hash`'s preimage excludes `history_hash` entirely (only `state_id` includes it), and `build_bpmn_design_position` never takes a belief/attempt-history argument at all - legality is structurally independent. Proves `state_id` still differs (history is real content) while `legal_moves()`/`move_set_hash()` are byte-identical. |
| `off_board_duplicate_or_incomplete_evidence_is_always_refused` | "no off-board evidence or clarification survives validation" | Drives `decide_bpmn_game_disposition` with a complete evidence set, then applies exactly one of three mutations (drop an entry / duplicate an entry / append an evidence entry for a move id never on the position) and asserts refusal every time - exercising `disposition::validate_game_inputs`'s exact-cover check. |

## Verified as a real gate, not a tautology

Temporarily flipped `history_hash_never_changes_legal_moves_or_move_set_hash`'s
`legal_moves()` assertion from `prop_assert_eq!` to `prop_assert_ne!` and confirmed the
test failed red (proptest found and reported the counterexample immediately, saving a
regression seed), then reverted and re-confirmed green. Production code (the pinned
`semantic-decision-contracts` dependency) was never touched — flipping the assertion
direction inside our own test was the available red-check without mutating a pinned
external crate.

## Named residuals — genuinely open, not silently dropped

- "Pack" drift (one of the four move-set-hash inputs) is not independently exercised:
  the fixture in this repo compiles against a single fixed semantic pack
  (`compiled_semantic_pack()`), so there is no second pack version to swap in without
  building new pack-compilation infrastructure. Revisit if/when a second pack fixture
  exists.
- The remaining ~11 Phase 8 property bullets are still open: "every offered fully bound
  move previews and compiles", "previewed delta equals ratified delta", "evidence
  fusion is invariant to producer execution order", "every attempt reaches exactly one
  typed outcome", "non-transition outcomes preserve graph state", "correction links are
  acyclic and resolve to an earlier attempt", "feedback options resolve to legal moves
  or governed context/focus actions", "disclosure filtering never leaks a hidden
  candidate through explanation text", "removing statistical producers leaves the
  palette operational", "stale proposals never apply", "production and reference-model
  outcomes agree after every operation in a generated tape". Several of these already
  have *some* coverage as ordinary (non-`proptest!`) unit/integration tests or as
  libFuzzer targets from the prior tranche (`correction_history` differentially checks
  the acyclic-correction invariant; `rule_explanation_decode` and
  `move_attempt_feedback` cover disclosure filtering and typed-outcome coverage) - this
  receipt does not re-audit which; it only closes the 4 bullets above as genuine new
  `proptest!` coverage.

## Results

- `cargo test -p utterance-engine --all-features`: 113 passed (was 109), 0 failed, 5
  ignored, 0 regressions.
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, all surface hashes
  unchanged (the new module is `#[cfg(test)]`, no production `pub` surface added).
