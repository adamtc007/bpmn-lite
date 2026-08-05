# Shared-crates remediation Phase 4 receipt

**Phase:** 4 — extract the pure semantic embedder
**Date:** 5 August 2026
**Result:** complete; shared extraction pushed and both consumer cutovers committed

## Delivered boundary

The shared DSL workspace now owns `semantic-embedder` version `0.2.0`, an MIT-licensed host-neutral crate with an empty default feature set. The default build exposes typed model identity and bundle contracts, `EmbeddingError`, the `Embedder` trait, and a deterministic fake. Local Candle inference is enabled by `candle`; exact Hugging Face resolution is a separate `huggingface-download` feature.

The shared crate contains no SQLx, pgvector, PostgreSQL, UUID, Tokio, host schema, feedback, centroid, client-group, phonetic, population binary, HTTP handler, or application dependency. It forbids unsafe code and loads safetensors through Candle's safe buffered API.

`ob-poc` retains its application-owned matcher, persistence, feedback, client-group, centroid, phonetic and population responsibilities. Its existing `Embedder` facade now delegates model loading and inference to `semantic-embedder` while preserving the three application-relative fine-tuned-model search paths and remote fallback.

BPMN's default-off `utterance-engine/embed` feature now imports `semantic-embedder` directly. The `ob-semantic-matcher` and `ob-poc-rust` dependency edge is absent from its manifest, lock and selected feature graph.

## Repository and commit ledger

| Repository | Branch | Starting HEAD | Ending commit | Publication state |
|---|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `feat/semantic-embedder` | `ca006a785e1545cf71e2870c4dffe9d7bb5147e8` | `5ac7da7a513744e907ca110484c3a6a9472ae985` — `feat: extract host-neutral semantic embedder` | pushed to `origin/feat/semantic-embedder` |
| `/Users/adamtc007/Developer/ob-poc` | `refactor/semantic-embedder-adapter` | `d76d8be9842c960e06841a4cc661d03ad44fbe73` | `333975b7c453758f5fabfdba76b2a0875df5da05` — `refactor: consume shared semantic embedder` | committed locally; not pushed by this phase |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/semantic-embedder` | `0bf5c058fdc8e2ea86825823816757261cd31b9b` | `2665c06ad42ef51a54e42c7739546edfc6ccbf49` — `refactor: consume shared semantic embedder` | committed locally; not pushed by this phase |

Rollback source revisions are the three starting HEADs above. No deployment or persistence migration occurred, so application-artifact rollback was not applicable or exercised.

## Public API and feature result

New shared public modules and types:

- `EmbeddingError` and `ModelArtifact` — typed failure categories;
- `ModelIdentity` and `ModelBundle` — validated model identity and caller-provided artifact paths;
- `Embedder` — host-neutral query/target and batch contract;
- `DeterministicFakeEmbedder` — dependency-free deterministic test provider;
- `CandleEmbedder` — optional safe local model loader and inference implementation;
- exact default BGE repository, revision, dimension, query-prefix and maximum-sequence constants.

Feature closures:

```text
default = []
candle = Candle + tokenizer + local JSON/bundle loading
huggingface-download = candle + hf-hub/ureq
metal = candle + Candle Metal dependencies
```

Metadata/tree assertions prove:

- the shared default closure contains no Candle, tokenizer, Hugging Face, SQLx or pgvector;
- the shared `candle` closure contains no Hugging Face, SQLx or pgvector;
- every shared feature closure contains no SQLx or pgvector;
- BPMN's default closure contains no model runtime, host matcher, SQLx or pgvector;
- BPMN's `embed` closure contains `semantic-embedder`, Candle and Hugging Face, but no `ob-semantic-matcher`, `ob-poc-rust`, SQLx or pgvector.

## Frozen inference compatibility

The extraction preserves:

- `BAAI/bge-small-en-v1.5` at revision `5c38ec7c405ec4b44b94cc5a9bb96e735b38267a`;
- 384 dimensions;
- the existing query instruction and unprefixed target contract;
- 512-token truncation;
- CLS position-zero pooling;
- L2 normalization with the existing clamp;
- CPU execution and existing batch behavior.

A standalone gate harness loaded the pre-extraction `ob-semantic-matcher` at `ff3f12c7` and the extracted implementation from the same cached pinned bundle. It compared two queries and two targets. Every 384-component vector was bit-for-bit identical: maximum absolute delta `0.0`; cosine similarity `1.0` within floating-point accumulation noise. The historical batch-versus-single divergence tripwire still passes, so the earlier decision not to batch serving inputs remains valid.

The BPMN evidence producer identity intentionally changes from `@ob-semantic-matcher:ff3f12c7` to `@semantic-embedder:5ac7da7`. This is accurate provenance for the moved implementation; model vectors, candidate scores and dispositions remain unchanged. Historical corpus cards retain the old identity because they record the producer actually used to generate them.

The cached pinned model card declares MIT. No model weights or tokenizer artifacts were copied into the shared repository.

## Verification receipt

### Shared DSL workspace

| Gate | Outcome |
|---|---|
| formatting | pass |
| locked workspace check, all targets/features | pass |
| workspace Clippy with `-D warnings` | pass |
| locked workspace tests, all targets/features | pass |
| rustdoc with `-D warnings` | pass |
| layering, dependency, domain-neutral and package gates | pass |
| `cargo-deny 0.20.2 check` | pass; existing warning-only `wit-bindgen` duplication remains |
| `semantic-embedder`, no features | pass — 4 unit tests and 1 doctest |
| `semantic-embedder`, Candle | pass — 5 passed, 1 real-bundle test ignored in the normal run |
| cached real-bundle deterministic inference | pass — ignored test explicitly executed |
| feature-tree assertions | pass |

### ob-poc adapter

| Gate | Outcome |
|---|---|
| exact-Git all-target/all-feature check | pass |
| focused Clippy with `-D warnings` | pass |
| focused tests | pass — 26 passed, 12 database tests ignored in the normal run |
| rustdoc with `-D warnings` | pass |
| ignored database/client-group suite | first combined run: 9 passed, 3 pool-acquisition timeouts; each timed-out test then passed independently on immediate retry |
| exact dependency tree | pass — persistence remains in the application matcher; inference is supplied by `semantic-embedder@5ac7da7` |

The combined database-suite timeouts were environmental connection-pool contention rather than semantic failures: the three tests reported only `pool timed out while waiting for an open connection`, and all three passed unchanged when isolated.

### BPMN consumer

| Gate | Outcome |
|---|---|
| shared-pin self-tests | pass |
| shared-pin real gate | pass — eight packages at exact revision `5ac7da7a513744e907ca110484c3a6a9472ae985` |
| locked full-workspace check, all targets/features | pass |
| locked full-workspace tests, all targets/features | pass |
| utterance-engine all-target/all-feature tests | pass — 65 passed, 4 ignored in normal run |
| real cached-model BPMN retrieval tests | pass — all 3 normally ignored embedding tests explicitly executed |
| default and embed feature-tree assertions | pass |
| utterance-engine fuzz-bin locked check | pass |
| designer-server fuzz-bin locked check | pass |
| `git diff --check` | pass |

The BPMN formatter remains red on pre-existing unrelated files. Strict scoped Clippy remains red only on the previously recorded unused `CapturePipeline` helpers in `utterance-engine/src/capture.rs`; this phase neither touched nor suppressed them. The shared crate and ob-poc adapter pass strict Clippy.

## Deployment and resource position

This phase introduced no service, database, schema migration, runtime network dependency in default builds, model download in default builds, model promotion, YAML change, threshold change or deployment. The opt-in BPMN feature retains its existing Hugging Face cache/download behavior.

Safe buffered safetensors loading temporarily holds the weights buffer during initialization, unlike the previous unsafe mmap loader. Inference output is identical, but initialization peak RSS should be measured before a production model-runtime promotion. That resource qualification is a carry-over, not a correctness defect in the extraction.

## Carry-overs

1. Measure `CandleEmbedder` initialization peak RSS and load latency under the production container limit; owner: model/runtime workstream; target: shared release qualification.
2. Resolve the existing BPMN formatting and `CapturePipeline` Clippy debt separately; owner: BPMN maintenance; target: before the next strict consumer gate.
3. Convert ob-poc's six older mutable DSL tag pins to the same exact shared revision in Phase 7; this phase added only the new embedder at an immutable revision.
4. Consider splitting ob-poc matcher persistence into narrower application crates only if the current application-owned package becomes a release/dependency problem; no reverse shared dependency remains.
5. Preserve the `sem_os_policy::decision_board` compatibility window and hash-v2 migration carry-overs from Phase 3.
6. Continue to the ruled Phase 6 forensic cleanup only after this Phase 4 receipt is accepted.

## User-work preservation

The coordinating BPMN checkout's concurrent DIR-002 changes were not modified, staged or reverted. The pre-existing `/Users/adamtc007/Developer/ob-poc/.cargo/config.toml.example` modification remains unstaged and was not included in the Phase 4 commit. Ignored repository-local patch files were restored after every exact-Git check. No unrelated file was committed.
