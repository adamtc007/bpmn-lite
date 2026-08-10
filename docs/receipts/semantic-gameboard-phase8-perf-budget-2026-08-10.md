# Semantic gameboard Phase 8 — performance-budget measurement harness

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification
(`docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14, "Performance budgets").

## What this closes and what it doesn't

Phase 8's performance-budget section asks to "measure by graph size and move-board
size" against a *ratified* budget. No budget numbers are ratified anywhere in this repo
- there is nothing to gate against, and inventing thresholds unilaterally would be
deciding a fork that isn't mine to decide (per the working contract: surface forks,
don't decide them). This receipt closes the **measurement infrastructure** half of the
bullet only: a real, running harness that emits machine-readable latency/size numbers
for a human to compare against a budget once one exists. It does not close "P95
interactive latency meets the ratified budget" (Gate 8) - there is no ratified budget
to check against yet.

## What was built

`utterance-engine/benches/gameboard_perf.rs` (`harness = false`, matching the existing
convention in `bpmn-lite-types/benches/v2_perf.rs` — no `criterion` anywhere in the
workspace, so this doesn't introduce a new tooling dependency for one crate). Measures,
against the same anchored-task fixture used throughout this session's fuzz/property
tranches (5,000 iterations each):

- legal move enumeration (`build_bpmn_semantic_board` + `build_bpmn_design_position`)
- full disposition latency (`decide_bpmn_game_disposition`)
- belief update (`update_bpmn_design_belief`)
- rule/feedback retrieval (`explain_bpmn_candidate`)
- serialized `DesignPosition` and `MoveEvidence` size

Like `v2_perf.rs`, it asserts only machine-independent *shape* claims, never raw
latency: identical inputs must serialize to identical byte counts (trivial but real -
it would catch a non-determinism regression), and both the position and evidence
encodings must be non-empty. No `assert!(ns < N)` exists anywhere in the harness; doing
so would fabricate a budget.

## Deliberately not covered here

- **Preview-compilation latency**: needs realistic per-candidate argument wiring
  matching `fuzz_targets/preview_compilation.rs`'s own ~600-line setup (each candidate
  has a distinct argument shape) - not a cheap addition to this harness. Revisit by
  reusing that fuzz target's fixture machinery if/when this bullet needs closing.
- **Learned-lane (Candle) scoring latency**: requires the `candle-probe` feature and a
  network-downloaded model, off by default in this repo like `embed` - consistent with
  every other network-dependent measurement in this codebase.
- **"Deterministic feature calculation"**: no single named production function maps
  unambiguously to this bullet: it could mean `typed_argument_score` (private, in
  `disposition.rs`), the evidence-lane assembly inside `finalize_bpmn_move_evidence`, or
  something else. Left unaddressed rather than guessing which function the bullet
  means.

## Baseline numbers (informational only — not a gate)

Run on this development machine, 5,000 iterations, the anchored-task fixture (14 legal
moves, 1 policy-open candidate set):

```
legal_move_count=14
legal_move_enumeration_ns=407136
full_disposition_ns=7014
belief_update_ns=9182
rule_feedback_retrieval_ns=13082
design_position_bytes=15102
move_evidence_total_bytes=3961
move_evidence_bytes_per_move=282
```

Legal move enumeration dominates by roughly two orders of magnitude over disposition/
belief/guidance latency, since it's the only one of the four that reconstructs the
board and position from scratch each iteration (the others reuse a built position).
This is expected, not a finding - flagged here only so a future reader doesn't mistake
the asymmetry for a regression.

## Results

- `cargo bench -p utterance-engine --bench gameboard_perf`: runs clean, all shape
  assertions pass.
- `cargo test -p utterance-engine --all-features`: 115 passed, 0 failed, 5 ignored, 0
  regressions (bench addition doesn't touch test code).
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, surface hashes
  unchanged (bench target, no `pub` surface).

## Open fork for whoever ratifies budgets

Gate 8 ("P95 interactive latency meets the ratified budget on representative
hardware") cannot close until someone actually ratifies numbers against the metrics
this harness now emits. Not decided here.
