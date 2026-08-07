# Semantic Gameboard Phase 0 Gate 0 receipt

**Date:** 2026-08-07
**Phase:** 0 — freeze claims and repair the measurement instrument
**Gate:** GREEN
**Scope:** measurement, v3 route admission, parity, fuzz/API/dependency baseline only

This receipt does not claim that the Semantic Gameboard programme is complete. It
closes only Phase 0 and establishes the entry evidence for Phase 1.

## Architectural decisions

1. `TrainedRanker` now derives a private `BundleInputMode` from the already sealed v3
   corpus and pair-serializer identities. The retained legacy public signatures fail
   with a private typed route-admission outcome before textualisation, tokenization or
   model work. No public item was added to support a test or fuzzer.
2. Persisted v3 records are admitted only with an embedded semantic board, matching
   board/context identities, current serializer/snapshot closure, complete canonical
   full-board lists, untampered pair hashes, semantic candidate text, label truth,
   exact evidence and binding requirements.
3. The starter evaluator now uses the graph-position BPMN semantic board, its admitted
   snapshot and context, `Tier1Ranker::rank_full_board`, semantic evidence finalisation,
   `DispositionConfig::shadow_v2` and `decide_with_action_spans`. It writes a new
   receipt artifact and has no path to the old v2 corpus/report files.
4. Evaluation fixture node identities are now supplied by a deterministic local
   sequence. They no longer depend on random UUID generation or global mutable state.
5. The first corrected run exposed a serving-only sentinel-survival veto. Python
   training admitted the same pair and applied the bundle's sealed longest-first
   256-token truncation; Candle alone rejected it later. The veto was removed as route
   skew. Raw pair serialization/hashes and bundle admission remain enforced, and the
   fixed Python/Candle packet proves tokenizer/model numerical parity. A future change
   that requires sentinels to survive tokenization must bump the serializer/bundle and
   retrain; Phase 0 did not pretend the existing bundle had that property.
6. The application remains the composition root. Graph authority, compiler/verifier
   admission, preview, workbook state, explicit ratification and mutation semantics
   were not changed. Models still emit evidence only.

## Corrected frozen evaluation

Command:

```text
cargo run -p utterance-engine --features candle-probe --release \
  --example starter_seed_eval
```

Artifact:
`docs/receipts/artifacts/semantic-gameboard-phase0-starter-evaluation.json`

- artifact SHA-256:
  `62411bafea5b4b6c56015848de7372bb073797a683a104491140e55f91aca5db`;
- sample count: 34;
- top-1 matching provisional/adjudicated hypothesis: 22/34;
- top-3 containing hypothesis: 28/34;
- NOTA ranked first: 10/34;
- dispositions: 16 candidate, 8 escalate-to-Sage, 10 out-of-scope;
- rows carrying board hash, full candidate list, pair hashes, exact evidence,
  evidence-producer identities, disposition and decision-record hash: 34/34.

This is evidence rather than a promotion gate. The former 15/34 and 7–8/34 results
are invalid for live semantic-v3 trend claims. The peer review and bake-off report now
carry explicit invalidation addenda; their historical statements remain visible.

Protected historical outputs were byte-identical before and after every corrected run:

```text
996da3bb81b9522df3e72e7aad4ce73ce4c6b97ee94c0711a57e6bce8dbec9f3
  utterance-engine/seed/corpus_v2/starter-seed-v1.enriched.jsonl
0a55a578a6151f87b0ec5b0e2d786bea633a331411a5c3397d3bb70092601a59
  utterance-engine/seed/corpus_v2/starter-seed-v1.report.json
```

No corpus, bundle, model weights, tokenizer or split manifest belongs to this phase.

## Evaluator/serving and Python/Candle parity

`tests/evaluator_serving_packet_identity.rs` independently constructs evaluator and
live-serving packets over the same fixed graph position. It proves identical board,
candidate order, pair bytes/hashes, final evidence bytes, disposition and decision
record, and reconstructs the fixture twice with identical board/context identities.

The fixed `bpmn.python-candle-parity.v1` packet contains three stored v3 pairs.
Python/Torch 2.13.0 and Candle 0.8.4 agreed on ranking and logits within absolute
tolerance `0.0002`:

```text
candidate                              Python logit
op.connect                             4.396847724914551
op.create_inclusive_region             1.3561034202575684
op.create_multi_instance_region        1.2450464963912964
ranking: connect > inclusive-region > multi-instance-region
```

Both the Python packet command and the ignored real-bundle Candle test passed after
the final implementation.

## Fuzz baseline and route/admission target

Discovery now reports 20 targets. The 19-target baseline is unchanged and
`utterance-engine/v3_route_admission` is added. The target consumes a bounded,
shrinkable operation tape selecting one of three hostile route states:

- legacy candidate textualisation declared by a v3 bundle;
- legacy pair textualisation declared by a v3 bundle;
- an unsealed pair-serializer identity.

The independent reference state names the expected refusal diagnostic. Every input is
bounded at 1,024 bytes and is guaranteed to refuse through the existing public
`TrainedRanker::load` admission facade before tokenizer/weights reads, model creation,
HF cache/network access or scoring. No fuzz-only production API or `cfg(fuzzing)`
visibility/authority bypass was introduced.

Local smoke receipt:

```text
toolchain: nightly 1.99.0-nightly (2026-08-04)
seed files: 1 committed v3/legacy mismatch
runs: 64
max_len: 1024
executed units: 64
peak RSS: 112 MiB
result: PASS
fuzz Cargo.lock SHA-256 before/after:
7808a635bb2e7b763fa744d24bc9d65f670e7f40dc615fc64698fe1cfcc4d6d4
```

The earlier 256-run smoke also passed (256 units, 200 MiB peak RSS). It was initially
pointed at the seed directory and generated 46 local corpus entries; those
Phase-0-created entries were immediately moved out of the repository to
`/tmp/bpmn-v3-route-fuzz-generated.XaduCx`. The standing PR smoke now copies the seed
to a temporary corpus, so committed seed/regression directories are not mutated.

`python3 scripts/check_fuzz_regressions.py` passed with exactly one governed committed
regression. The discovery-driven nightly matrix automatically gives the new target an
independent completion receipt and the existing aggregate missing-receipt check.

The standalone fuzz lockfile was deliberately refreshed to add the target's Candle
bundle-admission dependency closure. The workspace lockfile was not changed.

## Public API, visibility and dependency gate

`scripts/check-semantic-gameboard-boundaries.py` is a standing CI gate. It checks:

- all eight default/feature public API snapshots;
- exact approved `pub mod` sets and absence of public glob re-exports;
- inward capability/application/xtask dependency directions;
- a compile-pass external facade consumer;
- compile-fail private-module and unchecked-constructor consumers.

All eight item counts and hashes are byte-identical to the pre-implementation
baseline. Phase 0 added or removed **zero** public items. `utterance-engine` now opts
into the workspace `unreachable_pub = "deny"` lint. There are no visibility or
dependency exceptions.

Existing feature-surface inequality in `utterance-engine` remains recorded debt; the
gate freezes each feature surface independently and prevents Phase 0 growth. The
server surface remains identical across the affected features. Shared-release
semver-difference tooling remains a later shared-capability release gate because
`cargo-semver-checks` is not installed in this repository.

## Verification performed

Green:

- changed/new Rust files formatted and checked (the pre-existing unformatted portions
  of `fixtures.rs` were not mechanically reformatted);
- `cargo test -p utterance-engine`: 61 unit + integration/doc tests green;
- `cargo test -p utterance-engine --features candle-probe`: 65 unit tests green,
  two explicitly model-loading tests ignored, integration/doc tests green;
- evaluator/serving packet identity: 2/2 green;
- `cargo test -p bpmn-lite-server-designer --features candle-probe`: 57/57 green;
- explicit real-bundle Python/Candle parity test: 1/1 green;
- Python bytecode compile and parity command: green;
- changed libraries, evaluator, packet integration test and designer targets:
  Clippy warnings denied, green;
- public API/visibility/dependency boundary script: green;
- fuzz discovery: 20 targets, new target exactly once;
- governed regression validation: one non-empty case, green;
- isolated route-admission fuzz smoke: green;
- `git diff --check`: green.

Known unrelated baseline failures, not changed:

- broad changed-package Clippy with dependencies reaches two existing
  `bpmn-lite-compiler/src/lowering.rs` warnings (`match_like_matches_macro` and
  `too_many_arguments`);
- `utterance-engine --all-targets` Clippy reaches existing warnings in
  `score_trained_bundle.rs` (`doc_lazy_continuation`) and
  `candle_loadability_probe.rs` (`enum_variant_names`);
- workspace `cargo fmt --all -- --check` has pre-existing formatting drift across
  unrelated examples/modules. Phase 0 did not format those files.

## Exact Phase 0 file ledger

Changed tracked files:

```text
.github/workflows/production-gates.yml
docs/reviews/utterance-resolver-findings-peer-review-2026-08-07.md
docs/todo/EOP-REPORT-SLM-BAKEOFF-001.md
utterance-engine/Cargo.toml
utterance-engine/examples/starter_seed_eval.rs
utterance-engine/fuzz/Cargo.lock
utterance-engine/fuzz/Cargo.toml
utterance-engine/src/fixtures.rs
utterance-engine/src/pair.rs
utterance-engine/src/trained_ranker.rs
utterance-engine/train_py/eval_stored_pairs.py
```

Added files:

```text
docs/receipts/artifacts/semantic-gameboard-phase0-starter-evaluation.json
docs/receipts/semantic-gameboard-phase0-baseline-2026-08-07.md
docs/receipts/semantic-gameboard-phase0-red-2026-08-07.md
docs/receipts/semantic-gameboard-phase0-gate0-2026-08-07.md
scripts/baselines/semantic-gameboard-public-api-v1.json
scripts/check-semantic-gameboard-boundaries.py
scripts/fixtures/gameboard_api/facade_consumer.rs
scripts/fixtures/gameboard_api/internal_module_import.rs
scripts/fixtures/gameboard_api/unchecked_constructor.rs
utterance-engine/fuzz/fuzz_targets/v3_route_admission.rs
utterance-engine/fuzz/seeds/v3_route_admission/v3-card-legacy-candidate.txt
utterance-engine/tests/evaluator_serving_packet_identity.rs
utterance-engine/tests/fixtures/v3_python_candle_parity.json
```

The commit containing this receipt is recorded in the external phase handoff because a
file cannot contain the immutable hash of the commit that contains itself.

## Phase 1 entry conditions

Gate 0 is green. Phase 1 may begin with the shared design-position, move-envelope,
attempt-outcome, correction-link and capability contracts only if it preserves the
existing graph/compiler/ratification authority path, uses no BPMN vocabulary in generic
shared Rust contracts, and extends the generators/reference model/operation tapes in
the same phase. No Phase 1 implementation is included here.
