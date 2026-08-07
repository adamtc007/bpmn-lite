# Semantic Gameboard Phase 3 Gate 3 Receipt — 2026-08-07

## Status

Gate 3 is green for Phase 3. This receipt covers complete per-move evidence and
governed deterministic fusion only. It does not claim that later session-kernel,
correction, fault-tape, Sage, evaluation or federation phases are complete. No model
was trained and no corpus or bundle was regenerated.

The prerequisite shared-contract change is the published `dsl` commit
`f3f781cc42c61066dfb2728c441389f4c34a595d` on
`refactor/sem-os-pack-policy`. BPMN-Lite resolves every shared DSL capability from
that single immutable revision.

## Architecture delivered

- The private `MoveEvidenceProducer` mechanism emits bounded typed observations for
  governed exact/explicit references, grammar and contrasts, typed arguments, graph
  locality, structural completion, the active lexical/embedding/Candle lane,
  history/correction and abstention.
- Extraction is evidence-only. It never binds a workbook or mutates a graph.
- Every compiler-admitted concrete legal move receives all eleven declared lanes
  exactly once. Evidence cannot add or remove a legal move.
- `EvidenceFusionPolicy` is content-identified from the admitted pack policy and the
  versioned fusion algorithm. Lane weights are hand-ratified YAML values; no learned
  weights exist in this phase.
- Policy v1 bounds each producer value before fusion and assigns one owner to each
  lane. Consequently the recorded `LaneScore` is both the raw producer value and the
  identity-normalized lane value; final score, probability, rule code and producer
  identity are recorded in shared `MoveEvidence`. This avoids a duplicate evidence
  DTO.
- Shared `CandidateEvidence` and `InferenceEvidence` now form the live policy input
  for graph-backed serving. Candle, embedding and lexical routes all enter the same
  finalisation path. The compatibility-only non-positional route remains explicit.
- Governed exact evidence has rule dominance, while rejection and correction remain
  negative context. Their adjustment is derived from admitted history/correction
  weights rather than a host-owned feedback constant.
- Required rule explanations and recovery choices render from admitted pack
  resources. Compiler diagnostic text is not parsed into user feedback.
- The server remains the composition root. The graph, compiler admission, explicit
  preview, workbook, human ratification and actual mutation paths are unchanged; no
  automatic apply route was added.

## Admitted pack and shared-contract checks

The BPMN pack declares every shared evidence lane exactly once, with non-zero bounded
weights, plus governed applicability, required-argument, compiler-refusal, evidence
and policy-disclosure explanations and their recovery resources. Its checked lock is:

```text
compiler: semantic-pack/0.2.2
source_sha256: fe1174906c5a0bdad127abaaeb1c1fee748b0c6edac79457066a0c01356faca7
artifact_sha256: 965381b5ec50a388977df9fd7a8a940587a0cb74b01bcd49a44026c9cdc4b963
```

The shared pack admission layer rejects unknown lanes, zero/out-of-range weights,
unknown candidate/lane/rule/recovery references, contradictory gates, cycles and
unbounded resource shapes. Its independent `semantic_pack_admission` fuzz receipt was
green for 256 runs (`cov=5354`, `ft=6449`, 35 corpus units, 117 MB peak RSS) before the
shared commit was published.

Commands passed:

```text
cargo run -p xtask -- pack-check bpmn
bash scripts/check-shared-pin.sh
bash scripts/check-layering.sh
python3 scripts/check-semantic-gameboard-boundaries.py
python3 scripts/check_fuzz_regressions.py
cargo metadata --locked --format-version 1 --no-deps
```

## Behaviour and feature verification

Passed:

```text
cargo test -p utterance-engine
cargo test -p utterance-engine --features candle-probe
cargo test -p utterance-engine --features embed,candle-probe
cargo test -p utterance-engine --features q9-capture
cargo test -p bpmn-lite-server-designer
cargo test -p bpmn-lite-server-designer --features candle-probe
cargo test -p bpmn-lite-server-designer --features embed,candle-probe
cargo test -p bpmn-lite-server-designer --features q9-capture
```

The focused cements prove complete finite vectors, deterministic candidate projection,
producer/bundle/candidate order independence, explicit candidate and node references,
duration/count separation, governed negative contrasts, stable move-set identity, and
negative rejection/correction evidence. Existing server tests prove suggestion does
not stage a workbook, graph mutation remains ratification-only, and stale graph
revisions fail closed.

Phase-owned new Rust files and edited hunks are rustfmt-formatted. A targeted check of
the current files passes, while the concurrent workspace-wide formatting delta is
deliberately excluded from the commit; the standing repository-wide formatting
baseline is therefore not misreported as repaired. Phase-owned Clippy surfaces pass
with warnings denied for `utterance-engine` and the server under
`embed,candle-probe`. Broader all-target/Q9 Clippy remains red only on unrelated
standing baseline diagnostics: `doc_lazy_continuation` in
`examples/score_trained_bundle.rs`, `enum_variant_names` in
`examples/candle_loadability_probe.rs`, Q9-only unused capture helpers, and the
pre-existing Q9 `needless_return` in server REST. None was changed to manufacture a
green result.

## Fuzz and regression verification

`cargo run -p xtask -- fuzz list` discovered 23 targets, including the new
`utterance-engine/evidence_fusion` target with seven named seeds. The permanent PR
smoke copies seeds to an isolated directory and runs every target invocation to
completion.

The final bounded Phase 3 run passed 256 executions with all semantic counters:

```text
cov=5736 ft=13883 corpus=70/1033b peak_rss_mb=455
exact duration count node_reference negative_contrast abstention
```

It covers duplicate/missing full-board inputs, non-finite construction refusal,
candidate and bundle reorderings, canonical-equivalent utterances, irrelevant
history, rejection/correction, finite score/probability invariants and legal-set
immutability. The canonical receipt is
`docs/receipts/artifacts/semantic-gameboard-phase3-fuzz-smoke.json`. The existing
governed regression manifest remains valid with one committed crash regression. No
fuzz command modified a committed seed, corpus, lockfile or generated artifact.

## Public API and dependency review

The feature-invariant production API change is exactly three items:

```text
utterance-engine default:             385 -> 388, sha256 8ddf64d0aafd6c403f1642239672fc37de1b3eb3bb7a97af696d96e744c14830
utterance-engine candle-probe:        411 -> 414, sha256 15104cce5b816bf656e59fd27d78f55d7d4632f193af2ea5b518773225dabd7a
utterance-engine embed,candle-probe:  421 -> 424, sha256 7916ba81386ff6c2282526a3246ae71b53b25ff46b28ea1cf8134d87af7af304
utterance-engine q9-capture:          439 -> 442, sha256 2f3e654a8ce51e0c5510e96e59e774528382b320362b05abde641fe8efdef164
server under every checked feature:     8 ->   8, sha256 8b3ea0f6f1762e702261e1fc8b4dc99dee2ff5fd8d9fb229f8d5a2402ae39576
```

The additions are the two shared evidence fields on the existing stable `SlmResult`
and the named `bpmn_board::finalize_bpmn_move_evidence` facade. The real external
consumer is `bpmn-lite-server-designer`; the owning facade is `bpmn_board`; the
stability contract is graph-position-bound, board-complete evidence with compiler
legality unchanged. No producer, fusion implementation module, unchecked constructor,
test hook, fuzz-only API, public module or glob export was added.

Dependency direction remains application-inward. The capability does not depend on
the server, fuzz project or `xtask`; the server composes the capability; the fuzzer
uses only the public facade; and `xtask` remains orchestration-only. Feature comparison
shows no feature-only authority or visibility expansion.

## Exact changed-file ledger

```text
.github/workflows/production-gates.yml
Cargo.lock
Cargo.toml
bpmn-lite-server-designer/src/rest.rs
docs/receipts/artifacts/semantic-gameboard-phase3-fuzz-smoke.json
docs/receipts/semantic-gameboard-phase3-gate3-2026-08-07.md
docs/receipts/semantic-gameboard-phase3-red-2026-08-07.md
scripts/baselines/semantic-gameboard-public-api-v1.json
utterance-engine/config/bpmn-semantic-pack.lock
utterance-engine/config/bpmn-semantic-pack.yaml
utterance-engine/fuzz/Cargo.lock
utterance-engine/fuzz/Cargo.toml
utterance-engine/fuzz/fuzz_targets/evidence_fusion.rs
utterance-engine/fuzz/seeds/evidence_fusion/all-shapes.bin
utterance-engine/fuzz/seeds/evidence_fusion/shape-abstention.seed
utterance-engine/fuzz/seeds/evidence_fusion/shape-count.seed
utterance-engine/fuzz/seeds/evidence_fusion/shape-duration.seed
utterance-engine/fuzz/seeds/evidence_fusion/shape-exact.seed
utterance-engine/fuzz/seeds/evidence_fusion/shape-negative-contrast.seed
utterance-engine/fuzz/seeds/evidence_fusion/shape-node-reference.seed
utterance-engine/src/argument_evidence.rs
utterance-engine/src/bpmn_board.rs
utterance-engine/src/bpmn_pack.rs
utterance-engine/src/contract.rs
utterance-engine/src/exact.rs
utterance-engine/src/fusion.rs
utterance-engine/src/graph_features.rs
utterance-engine/src/legal_moves.rs
utterance-engine/src/lib.rs
utterance-engine/src/policy.rs
utterance-engine/src/retrieval.rs
utterance-engine/src/trained_ranker.rs
utterance-engine/tests/evaluator_serving_packet_identity.rs
```

The phase-scoped commit message is
`feat(resolver): record and fuse complete per-move evidence`. The resulting commit
identity is reported in the handoff because a commit cannot contain its own hash.

## Protected concurrent work and remaining programme

All pre-existing `.DS_Store` files, runner edit, corpus/bundle outputs, deleted split
manifest, untracked normative documents, untracked v3 corpora and training logs remain
unstaged. A concurrent workspace-wide Rust formatting diff appeared during Phase 3;
it is not part of this phase and remains unstaged. Phase-owned staging is restricted to
the ledger above.

Phase 4 is next. Its entry conditions are the Phase 3 commit, this Gate 3 receipt,
the immutable shared pin, unchanged compiler/ratification authority and availability
of the session-state inputs required for typed attempt receipts and append-only
history. Phase 4 must start with its own red receipt and may not collapse later
learning, Sage or evaluation phases into the session kernel.
