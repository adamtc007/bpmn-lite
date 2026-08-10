# Semantic gameboard Phase 8 gate — closure

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification.

Entry authority: `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14 ("Gate 8").
Supersedes the status (not the content) of
`docs/receipts/semantic-gameboard-phase8-gate-2026-08-10.md`, which closed
YELLOW with four bullets explicitly carried forward (3, 4, 5, 7). All four
close in this receipt, disposed of by five further receipts written the same
day. Re-verifying the original nine-bullet disposition in full, not just the
four that changed, so this is a complete gate record, not a diff against a
prior one.

## Disposition of every Gate 8 bullet

1. **"Every new fuzz target is discovered, independently sharded and
   receipted."** **CLOSED**, unchanged from the original gate receipt —
   `cargo xtask fuzz list` auto-discovers all 15 targets from
   `[[bin]]` entries in `utterance-engine/fuzz/Cargo.toml`.
2. **"No regression directory is empty after a finding is committed."**
   **CLOSED**, unchanged — 0 regressions across all targets; every crash found
   during development this session (the `end_key` collision in
   `legal_move_enumeration.rs`, the assertion-model gap in the same target,
   two earlier crash-hardening tranches from prior sessions) was a harness/test
   bug, fixed before any crash artifact needed to become a permanent
   regression case. `python3 scripts/check_fuzz_regressions.py` reconfirmed
   passing after every change this session (3 governed cases, unaffected).
3. **"P95 interactive latency meets the ratified budget on representative
   hardware."** **CLOSED.**
   `docs/receipts/semantic-gameboard-phase8-perf-budget-ratified-2026-08-10.md`.
   Adam ratified P95 ceilings (5ms enumeration, 1ms each for
   disposition/belief-update/rule-feedback-retrieval) against the measurement
   harness built in a prior session; wired as real `assert!`s in
   `gameboard_perf.rs` and a new CI step in `production-gates.yml`; red-green
   verified. Caveat named, not hidden: ratified on one development machine, not
   yet staging/production hardware — accepted via Adam's explicit choice of
   generous headroom.
4. **"Resource-limit failures are typed and leave the session usable."**
   **CLOSED.**
   `docs/receipts/semantic-gameboard-phase8-resource-limits-2026-08-10.md`.
   Added `GameboardContractError::ResourceLimitExceeded` and five new bounds to
   the pinned contract crate (`dsl` v0.4.0): contract-text length, move
   argument/applicability-fact counts, legal-move-set size, delta-operation
   count, an attempt-history backstop. Added a matching
   `BpmnBoardError::ResourceLimit` on the `bpmn-lite` side, a real
   enumeration-amplification cap (`MAX_ENUMERATION_CANDIDATES`, fires before
   expensive compiler work, not after), and an explicit, product-owned REST
   body limit. Every refusal proven, not just asserted, to leave the session
   usable — a legitimate call at or under each limit still succeeds
   immediately after a refusal, in every new test.
5. **"Expected wrong-move traffic cannot cause unbounded history, feedback
   recursion or repeated compiler work."** **CLOSED.**
   `docs/receipts/semantic-gameboard-phase8-wrong-move-traffic-2026-08-10.md`.
   400-turn end-to-end test through the real
   `update_bpmn_design_belief`/`decide_bpmn_game_disposition` chain, windowed
   the same way production does; proved flat per-turn cost and found the
   disposition policy's own repeated-failure loop breaker (escalation) is
   live, not dead code. "No repeated compiler work" recorded as
   architecturally guaranteed (a wrong-move disposition never reaches the
   compiler) rather than separately load-tested.
6. **"Every target has a completed receipt; semantic coverage includes every
   move kind, attempt outcome, disposition, disclosure class and correction
   lifecycle or records a reviewed unreachable justification."** **CLOSED**,
   unchanged from the original gate receipt (closed same-day by the coverage
   audit it references) — all 20 move kinds, 10 outcomes, 10 dispositions, 5
   disclosure classes and correction-lifecycle stages verified constructed
   across the suite; `SystemFailure`/`DisclosureSafeRefusal` named as
   reviewed-unreachable-by-construction, not a gap.
7. **"PostgreSQL fault tapes, native/Wasm differential packets and
   resource-abuse corpora pass their separately receipted lanes."**
   **CLOSED**, in three parts:
   - PostgreSQL fault tapes: closed in a prior session
     (`semantic-gameboard-phase8-postgres-fault-tapes-2026-08-10.md`).
   - Native/Wasm and Python/Candle differential: ruled N/A (v0.10 amendment) —
     no such runtime/binding exists in this product.
   - Resource-abuse corpora: closed across three receipts this session
     (`...resource-abuse-corpora...`, `...correction-history-limit...`,
     `...legal-move-enumeration-limit...`). First corrected a wrong assumption
     (this crate's `.gitignore` excludes `corpus` entirely; the only
     git-persisted fuzz mechanism is the regression manifest for confirmed
     crashes, not seed corpora) before doing anything. All three relevant
     decode/differential fuzz targets (`rule_explanation_decode.rs`,
     `correction_history.rs`, `legal_move_enumeration.rs`) were self-capped
     below every new resource limit and now reach at least one; the fourth
     candidate (`semantic_board_decode.rs`) correctly excluded as decoding an
     unrelated pre-gameboard contract type. `legal_move_enumeration.rs`
     deliberately does not reach `MAX_ENUMERATION_CANDIDATES` specifically
     (documented performance reason), with that cap covered instead by a fast,
     deterministic unit test from bullet 4's own work — named explicitly, not
     silently dropped.
8. **"Corpus minimization and regression-manifest validation run in CI without
   silently rewriting committed artifacts."** **CLOSED**, unchanged —
   `scripts/check_fuzz_regressions.py` wired into `nightly-fuzz.yml` and
   `production-gates.yml`; reconfirmed passing throughout this session.
9. **"Public-API snapshots and compile-fail boundary tests are unchanged
   except for separately reviewed facade/contract additions."** **CLOSED**,
   maintained throughout — every new `pub` item added this session
   (`ResourceLimitExceeded`, `BpmnBoardError::ResourceLimit`,
   `MAX_HISTORY_ATTEMPTS`, `MAX_HISTORY_BYTES`, `MAX_ENUMERATION_CANDIDATES`)
   was confirmed via `cargo public-api` before/after diffs to be exactly the
   deliberate addition and nothing else, and the baseline
   (`scripts/baselines/semantic-gameboard-public-api-v1.json`) updated
   reviewed, not silently.

## Results (aggregate, this session's Gate 8 work)

- `dsl` workspace (v0.4.0): `cargo build --workspace` and
  `cargo test --workspace` clean, 350+ tests, 0 failed.
- `bpmn-lite` workspace: `cargo check --workspace --all-targets --all-features`
  clean after every change.
- `cargo test -p utterance-engine --all-features`: all passing throughout,
  0 failed at every checkpoint.
- `cargo test -p bpmn-lite-server-designer --lib`: 77 passed, 0 failed.
- `cargo bench -p utterance-engine --bench gameboard_perf`: clean against
  ratified budgets; red-green verified.
- Three fuzz targets widened and stabilized (`rule_explanation_decode`,
  `correction_history`, `legal_move_enumeration`); one real fixture bug found
  and fixed along the way (`legal_move_enumeration.rs`'s `end_key` collision);
  multiple real fuzzing bursts across all three, 0 crashes after fixes.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass throughout,
  against a deliberately, incrementally updated baseline.
- `python3 scripts/check_fuzz_regressions.py`: pass throughout, unaffected (3
  governed regression cases, unchanged all session).

## What Gate 8 GREEN does not claim

- Bullet 3's ratification is on development hardware, not yet
  "representative" production/staging hardware — named in its own receipt.
- Bullet 7's `legal_move_enumeration.rs` coverage of
  `MAX_ENUMERATION_CANDIDATES` is by unit test, not this fuzz target —
  documented, not silently substituted.
- Gate 8 closing does not authorize Gate 6's learned-policy promotion (still
  RED per the v0.5 owner-authorized amendment) or start Phase 9's rollout —
  those are separate gates with their own, unmet entry conditions.
