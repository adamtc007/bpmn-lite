# Semantic gameboard Phase 8 — fuzz-target gap tranche

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification
(`docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14).

## Scope of this pass

Phase 8 is large: 11 named fuzz targets, ~15 property-test invariants, performance
budgets (no benches exist yet for any gameboard crate) and PostgreSQL fault-tape
replay. This pass closes the first, most concretely-scoped gap: of the 11 named fuzz
targets, 2 already existed exact-match (`legal_move_enumeration`, `evidence_fusion`)
and 4 had partial overlap under different names (`semantic_board_decode`,
`preview_compilation`, `history_belief_state`, `disposition_workbook_state` /
`workbook_transition`). The 5 genuinely missing targets are added here:
`clarification_policy`, `move_attempt_feedback`, `correction_history`,
`rule_explanation_decode`, `game_turn_replay`. Property-test invariant coverage,
performance budgets and PostgreSQL fault-tape replay remain open Phase 8 work, not
touched by this receipt.

Also recorded (plan doc v0.10 amendment): the differential-test bullets "native versus
Wasm compilation/admission" and "Python versus Candle learned-lane parity" are ruled
**N/A** for this product — no `wasm32` build target and no `pyo3` binding exist
anywhere in the workspace. Building either would be new product infrastructure adopted
solely to satisfy a checklist bullet, not a qualification pass over something that
exists.

## Grounding

Every target below drives a real, already-`pub` production function in
`utterance-engine/src/bpmn_board.rs` (or, for `rule_explanation_decode`, the
`semantic-decision-contracts` crate's own public constructors) — no new `pub` surface
was added to close any of these; `check-semantic-gameboard-boundaries.py` confirms.

| Target | Production surface | What it fuzzes |
|---|---|---|
| `clarification_policy` | `decide_bpmn_game_disposition` (routes through the private `crate::clarification::select`) | Fuzzed evidence scores across an anchored task's legal moves; when the disposition resolves to `ClarifyMoves`, asserts move-count bounds (2-3), moves ⊆ legal moves, non-empty governed prompt, attempt outcome is `Ambiguous`. All 3 clarification dimensions (Move/Focus/Argument) observed. |
| `move_attempt_feedback` | `record_bpmn_attempt`, `render_bpmn_game_disposition` | Every `MoveAttemptOutcome` variant; asserts `Applied` carries no explanations/feedback while every other outcome carries exactly one explanation; serde round-trip; a content-address tamper test (mutating an encoded `rule_explanations` entry) proves hostile explanation references are refused, never silently accepted; `render_bpmn_game_disposition` never leaks a message when nothing is admitted. |
| `correction_history` | `record_bpmn_attempt`, `project_bpmn_attempt_history` | A fuzzed tape of corrections (none / backward-valid / self-reference / forward / phantom) differentially checked against an independent reference model of the same abstract invariant (acyclic, resolvable correction graph over the *current* full tape — order-independent, matching `validate_attempt_history`'s own pure-function-of-the-slice semantics). Also proves self-correction is refused at receipt construction, before it can ever enter a tape. |
| `rule_explanation_decode` | `semantic_decision_contracts::RuleExplanation::new`, `filter_rule_explanations` | Raw hostile-byte JSON decode (never panics, round-trips losslessly); fuzzed rule code/message key/provenance/parameters (including duplicate parameter names) across every `DisclosureClass`; exhaustive allow-list filtering (32 masks over 5 classes) never leaks an explanation outside its allowed set. |
| `game_turn_replay` | `capture_bpmn_game_turn` (wraps `GameTurnRecord::new`'s ~8 cross-field consistency checks) | A replayed session of up to 8 turns. Each turn: the unmutated form is always admissible, deterministic (identical record hash on re-derivation), and round-trips through serde. A second, single-axis hostile mutation per turn (off-board chosen move; `Admitted` compiler result without a terminal attempt; `Refused` compiler result whose attempt outcome disagrees) is asserted to fail closed. |

## Fixture-design findings (fuzz-harness bugs, not product bugs)

Three real crashes were found and fixed during development — all harness defects, not
production defects; recorded because each reveals a genuine contract detail:

1. `move_attempt_feedback`: `1_u8 << index` overflowed for `MoveAttemptOutcome` index
   8/9 (10 variants, `u8` shift only valid to 7). Fixed by widening the observation
   counter to `u16`.
2. `correction_history` (first defect): assumed `record_bpmn_attempt` always succeeds
   and defers correction validity to the history-projection boundary. In fact
   `MoveAttemptReceipt::new` itself refuses self-correction
   (`"an attempt cannot correct itself"`) — a stronger, earlier gate than
   `validate_attempt_history`'s acyclic check. Fixed by special-casing self-reference as
   an expected construction-time `Err`.
3. `correction_history` (second defect): the reference model assumed corrections are
   only ever valid pointing to append-order-earlier attempts. `validate_attempt_history`
   is actually a pure function of the *current full slice* each call — it does not care
   about append order, so a forward reference becomes valid once its target is later
   appended. Fixed by rewriting the reference model to recompute fresh from the whole
   tape each step (bounded-hop acyclic walk) instead of tracking append-order state.
4. `game_turn_replay`: the hostile-mutation block re-derived `disposition` with a
   different `MoveAttemptId` than the `attempt` value it was reusing from the base
   scenario, so even the "no mutation" case tripped
   `GameTurnRecord::new`'s "disposition and turn name different terminal attempts"
   check for a reason unrelated to the axis under test. Fixed by using the same attempt
   id in both places.
5. `game_turn_replay`: the anchored-task fixture never produces a `MoveBindingState::
   Complete` legal move (confirmed against `legal_move_enumeration`'s own invariant —
   every non-abstention move here is always `Incomplete`), so the "`Admitted` compiler
   result without a terminal attempt" hostile axis (which requires
   `attempt.receipt().is_none()`, i.e. a `ProposeMove`/`CompoundPlan` disposition) was
   unreachable: `ProposeMove` never fired because `typed_argument_score` was always
   `0.0`. Fixed by adding a `TypedArgument` evidence lane to the fuzzed "propose" mode,
   matching how the real system justifies proposing despite unbound arguments.

## Results

- `cargo +nightly fuzz build` (all 15 targets in `utterance-engine/fuzz`): clean.
- Each of the 5 new targets, smoke-run bounded (`-max_total_time=15-30 -runs=100000-
  300000`) after its fix landed: 0 crashes/hangs/OOMs, no `artifacts/<target>/` entries
  left behind.

| Target | Executions | Distinct semantic counters observed |
|---|---|---|
| `clarification_policy` | 7,452 | 10 disposition kinds incl. `ClarifyMoves`; all 3 clarification dimensions |
| `move_attempt_feedback` | 7,839 | all 10 `MoveAttemptOutcome` variants |
| `correction_history` | 11,314 | all 5 correction schemes (none/backward/self/forward/phantom) |
| `rule_explanation_decode` | 300,000 | n/a (pure decode/construction fuzzing, no scenario counters) |
| `game_turn_replay` | 3,651 | all 4 hostile axes incl. the previously-unreachable `admitted_without_terminal_attempt` |

- `cargo check --workspace --all-targets --all-features`: clean (the fuzz crate is its
  own Cargo workspace root — `[workspace]` in `utterance-engine/fuzz/Cargo.toml` — so it
  is not part of this check; confirmed unaffected).
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, all surface hashes
  unchanged from the Phase 7 gate.
- `cargo test -p utterance-engine --all-features`: 109 passed, 0 failed, 5 ignored, 0
  regressions.

## Open Phase 8 work (not closed by this receipt)

- Property-test invariant coverage: `utterance-engine/src/property_tests.rs` covers 2
  of the ~15 listed invariants directly; the rest (move-set hash vs. drift, stale
  proposals never apply, disclosure filtering leak-freedom as a `proptest!`, history/
  belief cannot change legality, etc.) are unaudited against `proptest!` coverage
  specifically (some may already hold as ordinary unit tests — not confirmed here).
- Performance budgets: no `criterion` usage anywhere in the workspace; no `benches/`
  directory exists for any gameboard crate (`utterance-engine`, `designer-graph`,
  `bpmn-lite-server-designer`). No budget numbers are ratified to measure against.
- PostgreSQL fault-tape replay: substantial adjacent chaos-test infrastructure exists
  (`bpmn-lite-store-postgres`, `nightly-chaos.yml`) but no dedicated minimized-
  session/revision tape replay with two identities and connection-loss-before/after-
  commit, as Phase 8 specifically describes.
- Wasm/Python-Candle differential: ruled N/A this session (see Scope above); revisit
  only if a real product requirement for either surfaces.
