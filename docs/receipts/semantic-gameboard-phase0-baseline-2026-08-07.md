# Semantic Gameboard Phase 0 baseline receipt

**Date:** 2026-08-07
**Phase:** 0 — freeze claims and repair the measurement instrument
**Gate state at capture:** RED
**Normative inputs:** `EOP-VS-BPMN-GAMEBOARD-001` v0.4 and
`EOP-PLAN-BPMN-GAMEBOARD-001` v0.4, read completely before this receipt.

## Repository pre-flight

Commands:

```text
pwd
git branch --show-current
git rev-parse HEAD
git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}'
git status --short --branch --untracked-files=all
git status --porcelain=v2 --branch --untracked-files=all
git diff --name-status
git diff --cached --name-status
git ls-files --others --exclude-standard
git worktree list --porcelain
git branch --all --no-color
```

Captured before creating the programme branch:

- repository: `/Users/adamtc007/dev/bpmn-lite`;
- branch: `feat/dir-002-phase-c-slm-training`;
- HEAD: `22ba055966e49c15df2d296a470f690123799118`;
- upstream: `origin/feat/dir-002-phase-c-slm-training`;
- divergence: `+0 -0`;
- index: empty;
- programme branch subsequently created without changing files:
  `codex/bpmn-gameboard-refactor` at the same HEAD.

Other worktrees at capture:

- `/private/var/folders/rk/dvmhzg2557gghq46__h5mws40000gn/T/tmp.oRBr4NclEn/source`
  at `de48b8cfa1370bbad32b9c62d99a1e3c4086ba1a`, detached;
- `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` at
  `44afb933a1f70e7697163bc07d488867c4d8d846`, branch
  `refactor/bpmn-semantic-pack`.

No repository `AGENTS.md` exists. All relevant workspace, capability, application,
fuzz and `xtask` manifests were read before implementation.

## Protected pre-existing work

Every entry below existed before Phase 0 and is user/concurrent work. It must not be
staged, rewritten, regenerated, deleted or formatted by this phase.

```text
 M .DS_Store
 M bpmn-lite-server-runner/src/bus_runtime.rs
 M utterance-engine/seed/corpus_v2/starter-seed-v1.enriched.jsonl
 M utterance-engine/seed/corpus_v2/starter-seed-v1.report.json
 M utterance-engine/seed/corpus_v2/synthetic-v2-beta.ambiguity_enriched.jsonl
 M utterance-engine/seed/corpus_v2/synthetic-v2-beta.eval_enriched.card.json
 M utterance-engine/seed/corpus_v2/synthetic-v2-beta.eval_enriched.jsonl
 M utterance-engine/seed/corpus_v3/bpmn-semantic-v3-shadow.card.json
 M utterance-engine/train_py/bundles/eval_scores.json
 M utterance-engine/train_py/bundles/modernbert-base/training_card.json
 D utterance-engine/train_py/split_manifest.json
?? docs/.DS_Store
?? docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md
?? docs/todo/EOP-VS-BPMN-GAMEBOARD-001.md
?? utterance-engine/seed/corpus_v3/bpmn-semantic-v3-shadow.eval.jsonl
?? utterance-engine/seed/corpus_v3/bpmn-semantic-v3-shadow.jsonl
?? utterance-engine/train_py/augment_train.log
?? utterance-engine/train_py/control_train.log
?? utterance-engine/train_py/split_manifest_v3.json
?? utterance-engine/train_py/treatment_train.log
```

The only pre-existing source diff is a one-line import reorder in
`bpmn-lite-server-runner/src/bus_runtime.rs`; it does not overlap Phase 0. The two
normative v0.4 documents are untracked protected inputs and are not adopted into this
phase commit.

## Toolchain and shared revisions

```text
rustc 1.95.0 (59807616e 2026-04-14)
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
1.95-aarch64-apple-darwin (rust-toolchain.toml override)
cargo-public-api 0.52.0
cargo-semver-checks: not installed
```

The affected BPMN crates consume `sem_os_ontology`, `sem_os_policy`,
`semantic-decision-contracts`, `semantic-pack`, and optional `semantic-embedder` from
the exact DSL revision `a38eefe1e8d039bd8b52e52477ffd58ba39c3058` (v0.2.2).

## Production-path trace

For a graph-backed Designer session, the production path is:

```text
persisted append-only GraphEdit events
  -> reconstruct_designer_dag
  -> graph_identity_hash
  -> build_bpmn_semantic_board(PositionalLegality + admitted semantic pack + policy)
  -> context::project_ir
  -> DesignerState::retrieve_utterance_evidence
       -> Tier1Ranker::rank_full_board
       -> pair::serialize_candidate_pair for every board candidate
       -> exact::finalize_semantic_evidence
  -> policy::decide_with_action_spans
  -> proposal::start_workbook
  -> proposal::materialize_operations
  -> designer_graph::productions::apply_production on a clone
  -> DesignerDag::admit (production compiler/verifier)
  -> explicit ratification endpoint
       -> graph hash drift check
       -> reconstruct, re-stage and re-admit
       -> append authoritative GraphEdit only after admission
```

There is no automatic apply path. The graph is unchanged for evidence, disposition,
incomplete workbooks, rejected staging, stale ratification or compiler refusal.

## Red drift findings against v0.4

1. `starter_seed_eval` constructs the legacy thin `Board` through `build_board`, uses
   `pack.none`, stores `semantic_v3: None`, retrieves a K-subset and invokes
   `TrainedRanker::score`.
2. Production graph-backed serving constructs `SemanticDecisionBoard`, serializes
   every candidate pair, calls `Tier1Ranker::rank_full_board`, finalizes semantic
   evidence and calls `decide_with_action_spans`.
3. `TrainedRanker::load` validates that every currently loadable v3 bundle declares
   the candidate-pair serializer, but public legacy `score`, `score_list` and
   `score_serving` do not check the bundle input mode. A v3 bundle can therefore enter
   the legacy textualisation mechanically.
4. The starter evaluator records rankings only. It does not record pair hashes,
   full-board candidate identity, semantic board hash, final evidence, disposition or
   producer/policy closure.
5. The claimed 7/34 starter result is not evidence about the live v3 route. It remains
   historical evidence, but is invalid for the live-v3 comparison until the corrected
   instrument is run.
6. `eval_stored_pairs.py` is host-path-bound, evaluates only a split-selected corpus,
   emits aggregate accuracy rather than fixed per-pair logits, and has no Candle parity
   packet contract.
7. The current crate roots expose broad implementation module trees. This is existing
   API debt, not Phase 0 scope. It is classified below and must not grow during this
   phase.

## Fuzz baseline

Commands:

```text
cargo run --quiet -p xtask -- fuzz list --json
python3 scripts/check_fuzz_regressions.py
find .../fuzz/{seeds,regressions,corpus,artifacts} ...
```

Discovery returned 19 targets across seven fuzz projects:

| Project | Targets |
|---|---|
| `bpmn-lite-compiler` | `dsl_compile` |
| `bpmn-lite-engine` | `engine_commands`, `engine_graph`, `xml_compile`, `engine_recovery`, `engine_flagstorm` |
| `bpmn-lite-kernel` | `kernel_step`, `kernel_replay`, `kernel_replay_hostile`, `verifier_admission` |
| `bpmn-lite-server-designer` | `bpmn_binding_extract` |
| `bpmn-lite-server-runner` | `wire_decode` |
| `bpmn-lite-types` | `canonical_decode`, `canonical_decode_value`, `artifact_verify`, `envelope_decode` |
| `utterance-engine` | `semantic_board_decode`, `phrase_index`, `workbook_transition` |

Governed regression validation passed with exactly one committed case:
`bpmn-lite-engine/fuzz/regressions/xml_compile/f8-compiler-001-mi-no-successor.xml`.
The nightly workflow is discovery-driven and gives each target an independent matrix
job, corpus cache, completion receipt and crash-artifact upload; aggregation compares
all discovered targets with `completed-targets.txt`. Regression replay fails when no
committed case executes.

Seed inventory relevant to the active boundary:

- `utterance-engine`: one seed each for `semantic_board_decode`, `phrase_index`, and
  `workbook_transition`;
- `bpmn-lite-server-designer`: one `bpmn_binding_extract` seed;
- no route/admission target or seed exists.

Local evolved corpus inventory is 178,842 files. None belongs to the three current
`utterance-engine` targets or `bpmn_binding_extract`; these targets therefore lack
evolved-corpus evidence despite being discovered. Local crash-artifact inventory is
21 files. These local corpora/artifacts are uncommitted evidence and are not modified
by Phase 0.

## Public API baseline and disposition ownership

`cargo public-api -sss` was run with default and affected feature sets. Canonical
output is identified by item count and SHA-256:

| Package / features | Items | SHA-256 |
|---|---:|---|
| `utterance-engine` default | 347 | `29bbeeb847d3ebff7bfb2a8d93661417cfb24ada4c22b620c7c5db86b04dfa03` |
| `utterance-engine` `candle-probe` | 373 | `1b7728c76ad506cbddd210f9c3a4f8ea22acf5e2a329d14ebfc731a049ac4717` |
| `utterance-engine` `embed,candle-probe` | 383 | `632440593dc7a1df152f33363398a9a71578414c194add8bc9565b21c18e1628` |
| `utterance-engine` `q9-capture` | 401 | `e0bb39b4a04d89fe40605780c4d0f201b3326242d885833987dc21bc023cadd9` |
| `bpmn-lite-server-designer` default | 8 | `8b3ea0f6f1762e702261e1fc8b4dc99dee2ff5fd8d9fb229f8d5a2402ae39576` |
| server `candle-probe` | 8 | same as default |
| server `embed,candle-probe` | 8 | same as default |
| server `q9-capture` | 8 | same as default |

Existing `utterance-engine` public modules are classified by ownership:

- intended application/capability contract pending facade consolidation:
  `bpmn_board`, `context`, `contract`, `disposition`, `exact`, `pair`, `policy`;
- compatibility/legacy implementation with Phase 7 retirement owner: `board`,
  `retrieval`;
- corpus/evaluation/tool support exposed because examples currently compile as
  external consumers, remediation owner Phase 1/API facade: `corpus_schema`,
  `fixtures`, `trained_ranker`;
- application capture surface, remediation owner Phase 4/7 facade review:
  `dev_capture` and feature-gated `capture`/`funnel`;
- test-only `metrics` does not appear in non-test rustdoc output.

`designer-graph` exposes five implementation modules (`board_candidate`, `ops`,
`positional`, `productions`, `schema`) and public-field representations. Their
facade/private-representation remediation owner is Phase 2. Phase 0 does not widen or
clean this surface. `bpmn-lite-server-designer` exports only `rest`, `DesignerState`
constructors and `designer_router`; all workbook/binding and endpoint DTO machinery is
already `pub(crate)`. `xtask` is a binary and has no library public API.

There are no glob public re-exports in the affected source. The workspace already
defines `unreachable_pub = "deny"`, and the server opts into it. `utterance-engine`
and `designer-graph` do not yet opt into workspace lints; this is recorded debt, not a
reason to widen Phase 0 scope.

Feature-surface equality is currently not satisfied in `utterance-engine`: model and
Q9 features add public modules/items. Gate 0 requires Phase 0 to introduce no further
growth; facade consolidation and feature-surface equality remain owned by the later
API phase unless Phase 0 can remove exposure without changing consumers.

## Dependency-direction baseline

`cargo metadata --locked --format-version 1 --no-deps` and focused `cargo tree`
commands show:

- `bpmn-lite-server-designer` (application/composition root) depends on
  `utterance-engine`, `designer-graph`, compiler, engine, stores, authoring and shared
  contracts;
- `utterance-engine` depends inward on `designer-graph`, compiler/types, shared DSL
  contracts/pack, and optional model libraries;
- `designer-graph` depends inward on compiler/types only;
- no capability crate depends on `bpmn-lite-server-designer`, a fuzz project or
  `xtask`;
- fuzz projects depend on production facades/contracts, not the reverse;
- `xtask` depends on compiler, bus-handler and manifest crates and contains fuzz
  orchestration; it does not depend on `utterance-engine` or server internals.

The pre-existing `utterance-engine` dev-dependency on `sem_os_policy` is a compatibility
test edge, not a production edge. No Phase 0 dependency addition is approved unless it
preserves the direction above.

## Gate-0 entry conclusion

The measurement instrument is demonstrably red and the public/dependency/fuzz baselines
are frozen. Phase 0 may now add red tests and repair only the v3 routing, evaluator,
parity and route/admission-fuzz boundary. Model training, corpus regeneration, runtime
execution changes and unrelated worktree cleanup remain prohibited.
