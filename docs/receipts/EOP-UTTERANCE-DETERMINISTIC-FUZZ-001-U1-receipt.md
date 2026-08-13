# EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001 — U1 receipt

Baseline: Gate U0 accepted, `c236272`/`ae3f2b7` (branch
`codex/bpmn-gameboard-refactor`). This tranche's revision: pending commit
(see below). **Tier: CAREFUL.**

- **Scope delivered:** extended `utterance-engine/fuzz/fuzz_targets/history_belief_state.rs`
  in place per the U0-ruled scope (no new `[[bin]]` entry). Net diff: 186
  insertions / 7 deletions in one file, plus 4 new named seeds. Delivered
  exactly the residual work item 2/4 tables in the (twice-corrected) U0
  receipt identified as genuinely new — nothing more:
  1. **P1** (`content_hash` helper + before/after assertion on both the
     valid-tape path and both hostile-axis early-return paths): the
     source `DesignerDag`'s content-derived identity
     (`DesignerDag::graph_state_hash(&dag.to_ir().unwrap())` — the same
     derivation `build_bpmn_design_position` itself uses internally, not
     a fuzz-target-local notion of "hash") is asserted unchanged before
     vs. after the whole board→...→disposition sequence.
  2. **`decide_bpmn_game_disposition` wired into the loop** (ported from
     `disposition_workbook_state.rs`'s pattern, ruled in U0's corrected
     work item 2, not redesigned): real call each iteration using the
     already-computed `board`/`position`/`fused.move_evidence`/`belief`,
     `disposition.validate_for_position(&position).unwrap()`, and an
     explicit assertion every `selected_moves()` entry is on
     `position.legal_moves()` (P4). A second, identical-input call is
     asserted to produce byte-identical serialized output (P2, for
     disposition specifically — the sub-case `history_belief_state.rs`
     didn't already have).
  3. **P5, the two axes U0's second correction confirmed genuinely
     new**: off-board-candidate injection (extends
     `evidence_fusion.rs`'s existing duplicate/omit malformed-ranking
     pattern with the one sub-case it doesn't cover — a foreign
     candidate id pushed onto the ranking) and foreign/stale board
     revision (`build_bpmn_design_position` called with a revision that
     disagrees with the board's own, asserting
     `Err(BpmnBoardError::StaleBoardRevision { .. })`, the exact
     `matches!` pattern already proven in `bpmn_board.rs`'s own unit test
     at line 1867). Both axes are selected by a new `data[3] % 3` selector
     (0 = valid tape, 1 = off-board candidate, 2 = stale revision) and
     short-circuit the rest of the iteration after their assertion, same
     discipline as `evidence_fusion.rs`'s own malformed-ranking early
     return.
  4. **P6 (reuse, not new design, per U0's corrected work item 2)**: a
     new `observed_intent(selector)` helper produces
     case/whitespace-decorated variants of the fixed canonical phrase
     (`"remind then escalate"` — content unchanged, so shape 2's
     `motif.reminder_then_escalate` completion assertion at the bottom of
     the file still fires correctly). When the tape selects a
     non-canonical variant, `finalize_bpmn_move_evidence` is called a
     second time with the literal canonical string and the two
     `move_evidence` results are asserted identical — the same
     normalisation-equivalence property `evidence_fusion.rs:327-341`
     already proved at this boundary, asserted inline here too since this
     target now varies the phrase.
  5. **P3 (reuse, not new design)**: two light inline assertions after
     `fused` is computed on the valid-tape path — `move_evidence.len() ==
     position.legal_moves().len()` and a probability-sum-to-1 check,
     epsilon `1e-12` (matching `evidence_fusion.rs:294`'s own value
     exactly, not independently re-derived — tightened from an initial
     `1e-9` after blind review flagged the unexplained discrepancy) —
     confirming the already-proven `evidence_fusion.rs:273-294` property
     holds for this target's own generated position too, without
     re-deriving the full exhaustiveness proof.
  6. Tape layout: `data[3]` (hostile-axis selector) and `data[4]`
     (text-mutation selector) inserted before the existing outcome loop,
     which now reads `data.iter().skip(5).take(65)` instead of
     `skip(3).take(65)`. Confirmed backward-compatible with the existing
     514-file corpus and 4 named seeds (see Focused checks below) — no
     seed needed re-authoring.

- **Target(s) and owner crate:** `utterance-engine/fuzz` ::
  `history_belief_state` (existing `[[bin]]`, unchanged). No new target.

- **Public API diff:** none. `python3 scripts/check-semantic-gameboard-boundaries.py`
  reports the same `utterance-engine` item counts/hashes as before this
  tranche across all four feature combinations — fuzz-target-only changes
  are invisible to this gate by construction (`utterance-engine/fuzz` is
  a `[[bin]]`-only, `publish = false` package, not a tracked library).

- **Input grammar, maximum size, and fixture catalogue:** unchanged from
  U0's ruling — the existing `graph(shape: u8)` three-way fixture (empty /
  linear / guarded, ≤5 nodes, within the 8-node envelope), 4 KiB implicit
  cap (libFuzzer default `-max_len`), and the existing, retained
  64-receipt (`MAX_HISTORY_ATTEMPTS`) cap.

- **Valid and hostile families reached:** confirmed via semantic-counter
  output on both the full existing 514-file corpus replay and the two new
  seeds — `motif_abandoned`/`motif_active`/`motif_completed` (shape
  families), all 8 `MoveAttemptOutcome` variants including `corrected`,
  `resource_bound`, and the two new hostile-axis counters
  `hostile_off_board_candidate` and `hostile_stale_board_revision` — both
  fire deterministically off their dedicated new seeds.

- **Invariants asserted:** P1 (new), P2 (residual — disposition
  determinism specifically), P3 (reused inline), P4 (ported), P5 (two
  genuinely-new axes only, per U0's corrected scope), P6 (reused inline).
  P7/P8 out of scope for U1 per the plan.

- **Seeds/regressions added or minimised:** 4 new named seeds —
  `hostile-off-board-candidate.seed`, `hostile-stale-board-revision.seed`
  (one byte tape each selecting the corresponding hostile axis),
  `text-mutation-uppercase.seed`, `text-mutation-whitespace.seed`
  (selecting mutation variants 1 and 2). All 4 pre-existing named seeds
  (`abandoned`, `active`, `completed`, `resource-bound`) retained
  unmodified and replay clean under the new tape layout. Zero regression
  inputs existed for this target before or after this tranche (nothing to
  minimise).

- **Focused checks and live-fuzz command/statistics:**
  - `cargo check --bin history_belief_state --locked`: clean, zero
    warnings.
  - `cargo check --manifest-path utterance-engine/fuzz/Cargo.toml --bins --locked`:
    clean — all 15 targets in the crate still build.
  - `cargo +nightly-2026-08-03 fuzz run history_belief_state corpus/history_belief_state -- -runs=0`:
    full existing 514-file corpus replayed, **zero crashes/assertion
    failures**, all pre-existing and both new semantic counters fired.
  - `cargo +nightly-2026-08-03 fuzz run history_belief_state seeds/history_belief_state -- -runs=0`:
    all 8 seeds (4 existing + 4 new) replayed clean.
  - `cargo +nightly-2026-08-03 fuzz run history_belief_state corpus/history_belief_state -- -max_len=4096 -max_total_time=60 -print_final_stats=1`:
    60-second bounded live run, **1878 executions, 0 crashes**, coverage
    grew `cov: 9480→9512`, `ft: 28838→29703`, corpus grew by 200 new
    interesting inputs (into the gitignored local `corpus/` dir — not
    committed, per `utterance-engine/fuzz/.gitignore`).
  - `cargo test -p bpmn-lite-compiler --lib dsl::frontend`-equivalent
    check not applicable here (no production code touched — this
    tranche only edits a fuzz target); instead `cargo check --workspace --all-targets`:
    clean, same 2 pre-existing unrelated `bpmn-lite-server-designer`
    warnings as every prior tranche.
  - `cargo run -p xtask -- fuzz regress` (plan's named neighboring-target
    regression replays: `phrase_index`, `evidence_fusion`,
    `game_turn_replay`, `disposition_workbook_state`,
    `preview_compilation`): `preview_compilation` and `model_boundary`
    are the only two targets in the whole fuzz suite with committed
    regression inputs (1 each, from before this session) — both replayed
    `ok`. The other named targets, and `history_belief_state` itself,
    have zero committed regression inputs, so there was nothing to
    replay for them (not a gap this tranche introduced — `xtask fuzz
    regress` has no `--target` filter despite accepting the flag; it
    always sweeps every discovered target with regression inputs).
  - `cargo fmt --check` on the touched file specifically produced no
    diff attributable to `history_belief_state.rs` — the same pre-existing,
    repo-wide `rustfmt` version-drift pattern H6's receipt already
    documented shows up in several *other*, untouched fuzz targets
    (`clarification_policy.rs`, `game_turn_replay.rs`,
    `legal_move_enumeration.rs`, `rule_explanation_decode.rs`); confirmed
    by diffing which files actually changed, not just running the check.

- **PR smoke and nightly-discovery result:** not run this tranche (no CI
  push). Confirmed via U0 that `history_belief_state` is not in PR-time
  smoke today (only `v3_route_admission`, `legal_move_enumeration`,
  `preview_compilation`, `evidence_fusion` are, of the utterance-engine
  targets) — adding it is explicitly U2's own work item 1, not U1's.
  Nightly discovery is automatic (`cargo run -p xtask -- fuzz list --json`
  walks `[[bin]]` entries) and needs no change for this target to be
  picked up.

- **Known deviations or explicitly parked work:**
  - `bpmn-lite-engine/fuzz`'s `xml_compile` target fails to **compile**
    (not a fuzzing crash) — `FaultStore` is missing three
    `AdminProjectionStore` trait method implementations
    (`open_dev_capture_session`, `append_dev_capture_record`,
    `load_dev_capture_session`). Confirmed via `git stash`/`git stash
    pop` that this is **pre-existing on the clean committed tree**, fully
    unrelated to this tranche — this session never touches
    `bpmn-lite-engine` or `AdminProjectionStore`/`FaultStore` anywhere.
    Flagged transparently, not fixed (out of this plan's scope; likely
    belongs to whichever in-flight initiative owns the dev-capture store
    interface — see CLAUDE.md's DIR-004/Q9-capture context). This caused
    every `cargo run -p xtask -- fuzz regress` invocation in this
    tranche to report "1 crash(es)" even though the actual crash is a
    compile error in an unrelated crate, not a fuzzing-discovered defect
    in anything this tranche touched.
  - Same `cargo fmt` repo-wide drift H6 already documented and declined
    to fix (unrelated to this plan, local rustfmt/CI toolchain version
    mismatch) — reconfirmed present, not newly introduced.

- **Blind peer-review findings and dispositions:** an independent
  reviewer (no prior context) re-derived every claim above directly
  against the live repo rather than trusting this receipt's prose:
  read `content_hash`'s three assertion sites and confirmed the
  derivation matches production's own `build_bpmn_design_position`
  internals; diffed the `decide_bpmn_game_disposition` wiring against
  `disposition_workbook_state.rs` line-by-line and confirmed the
  `attempt_id` is threaded consistently through all three calls in an
  iteration; independently verified the `wrapping_add(1)`/`{:064x}`
  stale-revision reasoning holds for every `u8` value including the 255
  edge case; confirmed the off-board candidate id format cannot collide
  with real board candidate ids by checking `semantic-decision-contracts`'s
  actual id format; reproduced every verification command live
  (corpus replay, seed replay, a 30s independent fuzz run, workspace
  check, public-API gate, `cargo fmt --check`, `xtask fuzz regress`) and
  got matching results, including the exact same four unrelated
  fmt-drift files and the exact same `xml_compile` false-crash;
  independently reproduced the `xml_compile` pre-existing-failure claim
  via its own `git stash`/`git stash pop`, and confirmed
  `utterance-engine/fuzz` has zero dependency on `bpmn-lite-engine`, so
  no accidental connection was possible. Confirmed scope discipline
  exactly (one file, 186/7 diff at review time, 4 new seeds, zero
  production code touched).
  - **One finding, disposed:** the P3 probability-sum epsilon was `1e-9`
    against `evidence_fusion.rs`'s already-proven `1e-12`, an unflagged
    inconsistency in a receipt that otherwise justifies every deviation
    from the ported pattern. Tightened to `1e-12` to match exactly;
    re-verified `cargo check` and a corpus replay pass clean after the
    change (see Files/packages changed note above — this receipt was
    updated in place to reflect the fix).
  - Verdict: **accept**. No other discrepancy found.

- **STOP-gate decision: blocked — awaiting peer review of this receipt.**

Per Gate U1's own text (plan §6): "Every valid seed satisfies P1–P6.
Every hostile seed reaches its intended refusal rather than a generic
early failure. The public API diff is empty." All three hold, both by
this tranche's own verification and by the independent reviewer's
reproduction. U2 (regression governance and CI — adding this target to
PR-time smoke, nightly-discovery confirmation, a fuzz-coverage receipt
entry) has not started.
