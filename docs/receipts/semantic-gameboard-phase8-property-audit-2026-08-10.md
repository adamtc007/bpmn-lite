# Semantic gameboard Phase 8 — remaining property-bullet audit and close-out

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification
(`docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14).

Continues `docs/receipts/semantic-gameboard-phase8-property-tests-2026-08-10.md`, which
closed 4 of the ~15 listed property bullets and left 11 open. Rather than writing 11 new
`proptest!` cases unconditionally (padding, per the working contract - "don't add tests
beyond what's needed"), this receipt audits each of the 11 for existing coverage first,
and adds new coverage only where a genuine gap was found.

## Audit

| # | Bullet | Disposition | Evidence |
|---|---|---|---|
| 1 | every offered fully bound move previews and compiles | **Already covered** | `utterance-engine/fuzz/fuzz_targets/preview_compilation.rs` calls `preview_bpmn_workbook` then actually applies and `dag.admit()`s the result across many scenarios. |
| 2 | previewed delta equals ratified delta | **Already covered** | Same target: `preview.bound().operations()` is asserted equal to what's actually applied at multiple points (e.g. lines ~512, ~604), and `project_bpmn_bound_game_turn`'s own preview-vs-delta-hash check is exercised. |
| 3 | evidence fusion is invariant to producer execution order | **Already covered** | `fuzz_targets/evidence_fusion.rs` explicitly builds a `reordered` case (`ranking.reverse()`) and asserts fused output is unchanged. |
| 4 | every attempt reaches exactly one typed outcome | **Type-guaranteed, no test needed** | `MoveAttemptReceipt::outcome()` is a single non-optional `MoveAttemptOutcome` enum field - there is no representable state with zero or multiple outcomes. Testing this would test the Rust type system, not product logic. |
| 5 | non-transition outcomes preserve graph state | **Already covered** | `disposition_workbook_state.rs` fuzz target asserts `(dag.node_count(), edge_count)` unchanged across every outcome scenario; Phase 7's fault-tape suite asserts the same at the HTTP/session level for refused attempts. |
| 6 | correction links are acyclic and resolve to an earlier attempt | **Already covered** | `correction_history.rs` fuzz target (this session's prior tranche) differentially checks this against an independent reference model, per-step, across 5 correction schemes including cycles and dangling/phantom targets. |
| 7 | feedback options resolve to legal moves or governed context/focus actions | **Gap - closed this pass** | Previously only one fixed example (`bpmn_board.rs`'s own test module). New: `property_tests::gameboard::feedback_recoveries_resolve_to_legal_moves_or_governed_focus_change`, fuzzing `explain_bpmn_candidate` across all 13 anchor candidates × all 8192 policy-denial combinations, asserting every recovery's `move_id()` is either a real legal move or `None` paired with `FeedbackOptionKind::ChangeFocus`. |
| 8 | disclosure filtering never leaks a hidden candidate through explanation text | **Gap - closed this pass** | `explain_bpmn_candidate`'s policy-hidden branch passes an empty `parameters` vec to `RuleExplanation::new` (structurally leak-proof for that channel), but nothing proved the *recovery* channel was equally clean, and nothing locked this in as a regression test. New: `property_tests::gameboard::policy_hidden_explanation_never_names_the_hidden_candidate`, fuzzing which candidate is hidden (holding others independently fuzzed-denied too) and asserting the serialized explanation+recoveries never contain the hidden candidate's id substring. |
| 9 | removing statistical producers leaves the palette operational | **Already covered, now stronger** | `bpmn_board.rs` has zero `#[cfg(feature = ...)]` gates - the entire legal-move/disposition/guidance pipeline is structurally independent of `candle-probe`/`embed`, which are off by default. `.github/workflows/production-gates.yml` already runs `cargo test -p utterance-engine property_tests` under default (no `candle-probe`) features as a real CI gate; the 6 new gameboard property tests from this and the prior receipt now run under that exact gate, verified locally: `cargo test -p utterance-engine property_tests` (no `--all-features`) → 12 passed, 0 failed. |
| 10 | stale proposals never apply | **Already covered** | Phase 7's `test_api_fault_tape_stale_client_preserves_new_revision` (`bpmn-lite-server-designer/src/rest.rs`) exercises this at the HTTP/session level, the layer where staleness is actually enforced (`build_bpmn_design_position`'s `StaleBoardRevision` check). |
| 11 | production and reference-model outcomes agree after every operation in a generated tape, not only at final state | **Methodology demonstrated, not universal** | `correction_history.rs` and `game_turn_replay.rs` both check production-vs-model agreement after *every* step of a generated tape, not only at the end - this is the differential-fuzzing pattern the bullet describes. It has not been applied to every subsystem (e.g. no reference model exists yet for the compiler-admission/preview pipeline itself); treating this as closed would overclaim. Left open, named explicitly rather than silently dropped. |

## Verified as real gates, not tautologies

Temporarily flipped `policy_hidden_explanation_never_names_the_hidden_candidate`'s
assertion (`!rendered.contains(...)` → `rendered.contains(...)`) and confirmed it failed
red with a concrete counterexample (`mask = 0, candidate_index = 0`) before reverting.

## Results

- `cargo test -p utterance-engine --all-features`: 115 passed (was 113), 0 failed, 5
  ignored, 0 regressions.
- `cargo test -p utterance-engine property_tests` (default features, matching the CI
  gate in `production-gates.yml` line 55): 12 passed, 0 failed - direct evidence for
  bullet 9.
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, surface hashes
  unchanged (test-only module).

## Phase 8 property-bullet status after this receipt

13 of ~15 bullets closed (4 from the prior receipt + 2 from this one + 7 cited as
already covered by existing fuzz/unit/CI evidence). 1 (#4) is type-guaranteed and needs
no test. 1 (#11) is explicitly left open as a named residual - the differential
methodology exists and is proven in two targets, but is not claimed to cover every
subsystem.

Performance budgets and PostgreSQL fault-tape replay remain the only completely
untouched Phase 8 work.
