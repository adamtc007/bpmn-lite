# Semantic Gameboard Phase 6 implementation checkpoint

Date: 2026-08-07

Baseline commit: `1bbdf4b7ae670ff179a08b60ff0560491fc0c0f6`

Branch: `codex/bpmn-gameboard-refactor` (no upstream)

Implementation status: GREEN for the available Phase 6 mechanisms.

Gate 6 status: **RED**. This receipt does not authorize learned-policy promotion and
does not claim that Phase 6 or the wider programme is complete.

## Authority and architectural decisions

1. A completed workbook now projects to a new proposal-qualified `LegalMove` and
   `DesignPosition`. The new move contains the resolved typed arguments and exact
   compiler delta. The original unbound move remains unchanged in history.
2. Serving, ratification replay and game-turn capture use the same bound position,
   complete evidence, belief, deterministic disposition and delta. Exact projection
   mismatch fails closed before mutation.
3. Chartered game capture is a separate `*.game-turns.jsonl` stream. It records the
   complete shared `GameTurnRecord`; structured adjudications use a separate,
   append-only, restart-safe ledger. New capture envelopes have private
   representations, bounded admission and read-only accessors.
4. Corrections retain prior attempts through the shared `related_attempts` history.
   Exploratory and accidental interactions cannot become positive labels. Accepted
   and explicitly corrected on-board intent enters the structured baseline only through
   the shared adjudication gate.
5. Real turns freeze by whole session, latest observation time and semantic family.
   The Phase 6 constructor refuses fewer than 100 adjudicated turns and requires
   non-empty training, validation and test partitions. Training and calibration APIs
   require the matching frozen training or validation assignment respectively.
6. The structured baseline is deterministic conditional logit over every Phase 3
   evidence lane. Its identity includes the canonical training observations, weights
   and fit configuration. Temperature calibration is independently fitted by risk
   class and board-size bucket; an unseen stratum fails closed. Neither mechanism can
   add a legal move or issue a disposition.
7. Four-resolver comparison packets cover the identical position and move set exactly
   once for deterministic fusion, fusion plus Candle, structured choice and bounded
   offline prompt evidence. Partial, duplicate, foreign and non-finite output is
   rejected. Model refusal is a distinct typed outcome. Wilson 95% intervals and
   per-risk metrics are emitted only from adjudicated cases.
8. The game-level funnel reports representability, board inclusion, top-1/top-3,
   disposition, typed arguments, correction-free acceptance, delta, compiler
   admission, governed feedback, recovery cost, repeated failure and reversal. A
   field that the captured/adjudicated contract cannot prove is explicitly counted as
   not measured.
9. Statistical producers remain evidence-only. Graph authority, compiler admission,
   explicit preview and human ratification are unchanged. No automatic apply path was
   introduced.

The shared contract correction-history repair was committed and pushed separately in
`/Users/adamtc007/dev/dsl` as
`452342edffde74164719707a1174bc17fad0f493` (`fix(contracts): retain correction turn
history`). BPMN-Lite pins every affected DSL dependency and both fuzz lockfiles to that
exact revision.

## Gate 6 data receipt

```text
Q9_CAPTURE_DIR:                   unset
Q9_CHARTER_REF:                   unset
adjudicated real turns observed: 0
minimum required:                100
frozen split emitted:            no
structured model fitted:         no
four-resolver comparison run:    no
promotion authorized:            no
```

The fail-closed command was:

```text
cargo run -p utterance-engine --features q9-capture \
  --example freeze_real_turn_split -- <empty-temp-dir> 10 20 <output>

Error: real-turn split requires at least 100 adjudicated turns; observed 0
```

No model was trained or retrained. No corpus, bundle, tokenizer, model weight,
training log or pre-existing split manifest was regenerated or adopted. Synthetic
fixtures test mechanisms only and do not contribute promotion evidence.

## Verification

Green:

- `cargo fmt --all -- --check` after formatting the phase-owned Rust;
- `cargo test -p utterance-engine --features q9-capture --lib`: 105 tests;
- `cargo test -p utterance-engine --features candle-probe --lib`: 95 passed,
  two existing model-loading tests ignored;
- `cargo test -p bpmn-lite-server-designer --features q9-capture --lib`: 57 tests;
- warnings-denied, no-dependency Clippy for `utterance-engine` with `q9-capture`;
- warnings-denied, no-dependency library Clippy for Designer server with `q9-capture`;
- all three Phase 6 examples compile against public facades;
- public API, visibility, compile-pass/compile-fail and dependency-direction gate;
- 26 fuzz targets discovered, including `model_boundary`;
- three governed fuzz regressions validated and replayed;
- `preview_compilation`: 1,000-run isolated smoke, pass;
- `model_boundary`: 1,000-run isolated ASan smoke, all nine semantic counters,
  pass, 122 MiB peak RSS;
- changed fuzz target compilation and `git diff --check`.

The fuzz receipt is
`docs/receipts/artifacts/semantic-gameboard-phase6-fuzz-smoke.json`.

Known unrelated baseline failures, left unchanged:

- dependency-reaching Clippy reports existing warnings in
  `bpmn-lite-compiler/src/lowering.rs` and `bpmn-lite-kernel/src/lib.rs`;
- Candle all-target Clippy reaches existing `score_trained_bundle.rs`
  documentation warnings and `candle_loadability_probe.rs` enum-name warnings;
- Designer all-target no-dependency Clippy reaches the existing test-only
  `rest.rs` needless-return warning.

## Public API and dependency review

The application composition root consumes the reviewed BPMN capture/bound-projection
facade. Chartered evaluator examples consume the game-turn split, structured model,
calibration and funnel facade. The state-machine fuzzer consumes the same bounded
request and full-board packet constructors. No API exists solely under `cfg(fuzzing)`.

New root facade representations keep fields private and expose validated constructors
and read-only accessors. No new `pub mod`, glob re-export, unchecked constructor,
application dependency, fuzzer dependency or `xtask` dependency was introduced.

```text
utterance-engine default:             539 items, b3212ace212ca7de7985d2183edfb0def7b371b28256d5cef0b9a3450cece033
utterance-engine candle-probe:        565 items, e0b30b00dc1baa5305c68da7a32ce0a2f5f1619a2b7d006b4c3102a076929586
utterance-engine embed,candle-probe:  575 items, 832a6bf2c643c30da318d2ab595dae34f30491fcc480c2c92c2cd2bd635adba8
utterance-engine q9-capture:          678 items, aa350b18dce3ddc8f061fa101dd968ac7214d3d6b430c0f73f2d71007f0cf066
server all checked features:            8 items, 8b3ea0f6f1762e702261e1fc8b4dc99dee2ff5fd8d9fb229f8d5a2402ae39576
```

There are no visibility or dependency exceptions.

## Exact Phase 6 file ledger

```text
Cargo.lock
Cargo.toml
bpmn-lite-server-designer/src/rest.rs
docs/receipts/artifacts/semantic-gameboard-phase6-fuzz-smoke.json
docs/receipts/semantic-gameboard-phase6-checkpoint-2026-08-07.md
docs/receipts/semantic-gameboard-phase6-red-2026-08-07.md
fuzz-regressions.json
scripts/baselines/semantic-gameboard-public-api-v1.json
scripts/fixtures/gameboard_api/facade_consumer.rs
utterance-engine/Cargo.toml
utterance-engine/examples/adjudicate_game_turn.rs
utterance-engine/examples/fit_phase6_structured_baseline.rs
utterance-engine/examples/freeze_real_turn_split.rs
utterance-engine/fuzz/Cargo.lock
utterance-engine/fuzz/Cargo.toml
utterance-engine/fuzz/fuzz_targets/model_boundary.rs
utterance-engine/fuzz/fuzz_targets/preview_compilation.rs
utterance-engine/fuzz/regressions/model_boundary/phase6-model-boundary.seed
utterance-engine/fuzz/regressions/preview_compilation/phase6-bound-evidence-order.seed
utterance-engine/src/bpmn_board.rs
utterance-engine/src/capture.rs
utterance-engine/src/funnel.rs
utterance-engine/src/lib.rs
utterance-engine/src/resolver_comparison.rs
utterance-engine/src/structured_choice.rs
```

All pre-existing/concurrent formatting, runner, corpus, bundle, training-log,
`.DS_Store`, deleted split-manifest, generated v3 corpus and normative-document changes
remain outside the Phase 6 index.

## Gate and next phase

Gate 6 remains RED until at least 100 chartered, adjudicated real turns are available,
the frozen split is receipted, all four resolvers run on identical boards, confidence
intervals and per-risk results are published, and feedback/recovery promotion metrics
are evaluated. Phase 7 must not begin before that green Gate 6 receipt exists.
