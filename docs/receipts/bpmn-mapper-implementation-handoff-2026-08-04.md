# BPMN semantic mapper implementation handoff

**Date:** 4 August 2026
**Branch:** `feature/bpmn-semantic-decision-board`
**BPMN base:** `b5b2844d9352b7f2ff56696ac34ce8e09813af98`
**Implementation code tip:** `6e9ebdd7113350bff39aef35d3511b800223dc17`
**Shared DSL base/final:** `a043e7f3d40262b78b367a6c18ac4a937c7498c6` → `fa51217ffd2218edea82c175e45ffa11d9eb7cf9` (`v0.1.6`)
**ob-poc base/final:** `d76d8be9842c960e06841a4cc661d03ad44fbe73` → `342fdd374032619c78b1461fe8ccdb030e413926`
**Promotion:** shadow; human ratification always required

The receipt itself follows the implementation code tip, so the repository HEAD
containing this document is reported at final handoff rather than recursively
embedded here. The three reviewed implementation branches have since been
published; no pull request or merge was created as part of the handoff.

## Phase commits

| phase | commit | outcome |
|---|---|---|
| 0 | `be0aa48` | invocation/authoring pack truth |
| 1 | `a3a47a5` | drift review, executable programme and baseline |
| 2 | shared `fa51217` | shared SemOS board/evidence/workbook contracts |
| 3 | `7d3c34f` | exhaustive BPMN semantic profile and legal board |
| 4–5 | `c75a65e` | graph cutover, exact evidence and full-board serving |
| 6 | `7f91236` | v3 pair/corpus/bundle admission; weights remain blocked |
| fuzz critical | `599a326` | complete nightly matrix and real regression gate |
| 7 | `9ce4a56` | typed resumable proposal workbooks |
| 8 | `e2f48c2` | ambiguity/compound closure and durable proposal audit |
| 9 | `a251c31` | property/fuzz qualification, CI and performance |
| 10 | `6e9ebdd` | conservative shadow/suggest/workbook rollout |

## Source inventory

Shared DSL/SemOS:

- `crates/sem_os_policy/src/decision_board.rs`, `domain_pack.rs`, `lib.rs`;
- `crates/dsl_types/src/constellation_map_def.rs`;
- `docs/decision-board-contract-mapping.md` and the layering guard.

BPMN semantic mapper and serving:

- new `utterance-engine/src/{bpmn_pack,bpmn_board,exact,pair,disposition}.rs`;
- changed board, contract, context/corpus, retrieval, policy, trained-ranker,
  capture, examples and Python corpus/bundle validators under
  `utterance-engine/`;
- `bpmn-lite-server-designer/src/{rest,proposal}.rs` for graph-backed serving,
  rollout, typed answers, dry staging, audit and ratification;
- shared-revision pins in root/Cargo manifests and lockfiles.

Pack and wait truth:

- BPMN invocation manifest and compiler/authoring execution-plan files;
- server-runner/store message-wait projections;
- the ob-poc BPMN pack, DAG/constellation/domain seeds, operation registry and
  infrastructure split listed by commit `342fdd37`.

Assurance and governance:

- production/nightly workflows, xtask fuzz runner, governed regression
  manifest/checker and F8 reproducer;
- mapper property tests and two new fuzz projects/four targets;
- programme, plane ledger, drift review and phase receipts under `docs/`.

The complete machine inventory is reproducible with:

```text
git diff --name-status b5b2844d9352b7f2ff56696ac34ce8e09813af98..6e9ebdd7113350bff39aef35d3511b800223dc17
git -C /Users/adamtc007/dev/dsl-sem-os-decision-board diff --name-status a043e7f3d40262b78b367a6c18ac4a937c7498c6..fa51217ffd2218edea82c175e45ffa11d9eb7cf9
git -C /Users/adamtc007/Developer/ob-poc-bpmn-pack-truth diff --name-status d76d8be9842c960e06841a4cc661d03ad44fbe73..342fdd374032619c78b1461fe8ccdb030e413926
```

## Generated artifacts and commands

- checked BPMN invocation manifest/closure: `cargo run -p xtask -- pack-check bpmn`;
- v3 corpus/card: `cargo run -p utterance-engine --example corpus_gen` (large
  JSONL outputs remain uncommitted; the small shadow card is committed);
- semantic-board fuzz seed: `cargo run -p utterance-engine --example semantic_board_seed`;
- native performance Markdown: `cargo run --release -p utterance-engine --example semantic_perf_receipt`;
- target discovery: `cargo run -p xtask -- fuzz list --json`;
- committed regression replay: `cargo run -p xtask -- fuzz regress`.

Fuzz locks were regenerated through `cargo +nightly fuzz build --fuzz-dir
<project>/fuzz`. The runner now refuses any execution that rewrites them.

## Receipts

- drift/base: `docs/reviews/bpmn-utterance-mapper-drift-review-2026-08-04.md`;
- Phase 3: `docs/receipts/bpmn-semantic-board-phase3.md`;
- Phases 4–5: `docs/receipts/bpmn-semantic-serving-phase4-5.md`;
- Phase 6: `docs/receipts/bpmn-candidate-pair-phase6.md`;
- fuzz critical: `docs/receipts/fuzz-critical-gates-2026-08-04.md`;
- Phase 7: `docs/receipts/bpmn-proposal-workbook-phase7.md`;
- Phase 8: `docs/receipts/bpmn-disposition-audit-phase8.md`;
- Phase 9: `docs/receipts/bpmn-mapper-phase9.md` and the performance receipt;
- Phase 10: `docs/receipts/bpmn-mapper-shadow-report-2026-08-04.md`.

## Gate summary

Passed:

- pack truth, layering and Q9 self/live guards;
- shared contract/workspace and ob-poc narrow pack compatibility gates;
- mapper contract/property/serving/proposal tests;
- all-feature workspace build, serial tests and documentation;
- seven cargo-fuzz project builds, 19-target discovery, four seeded mapper
  probes and non-mutating committed regression replay;
- changed-source formatting and changed-package Clippy with `-D warnings`.

Historical workspace exceptions:

- workspace formatting reports unrelated pre-existing DMN drift;
- full dependency Clippy reports two unchanged kernel and two compiler lints;
- rustdoc succeeds with unrelated existing link warnings.

Ignored/external tests:

- three embedding tests: pinned BGE download/cache required;
- tier-1 serving and designer context-pair test: admitted `SLM_BUNDLE_DIR` v3
  bundle and BGE cache required;
- graph-authored spawn integration: foreign database migration history;
- v3 training/evaluation: Python 3.14 host has no compatible PyTorch, and no
  independent evaluation/owner thresholds were supplied.

## Known gaps

The governed [carry-over register](../todo/bpmn-semantic-mapper-carry-overs.md)
assigns an ID, priority, owner role and objective completion condition to every
item summarised here.

- no admitted v3 model weights, independent confusion matrix, NOTA/ambiguity
  metrics, confident-wrong review or owner-ratified promotion thresholds;
- no Candle cold/warm/full-board latency or request-memory receipt;
- seven semantically described candidates remain binder-unrepresentable and
  are excluded from legal production boards;
- fuzz review FT-03–FT-06 and FT-09 remain separate execution-authority work:
  controlled-clock lease/job models, PostgreSQL crash cuts, native/Wasm corpus
  differential and durable resource budgets/telemetry;
- the isolated ob-poc worktree contains unrelated uncommitted generated DSL
  changes outside commit `342fdd37`; they were not modified, staged or folded
  into this implementation.

The correct operational decision is to keep `BPMN_MAPPER_ROLLOUT` unset (or
`shadow`) until the independent evidence and owner decisions exist.
