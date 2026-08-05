# Shared-crates remediation Phase 4 baseline

**Date:** 5 August 2026
**Status:** pre-edit dependency and compatibility receipt

## Repository ledger

| Repository | Phase branch | Starting HEAD | Pre-existing dirty state |
|---|---|---|---|
| `/Users/adamtc007/dev/dsl` | `feat/semantic-embedder` | `ca006a785e1545cf71e2870c4dffe9d7bb5147e8` | clean |
| `/Users/adamtc007/dev/bpmn-lite-semantic-decision-board` | `refactor/semantic-embedder` | `0bf5c058fdc8e2ea86825823816757261cd31b9b` | clean; ignored local `.cargo/config.toml` present |
| `/Users/adamtc007/Developer/ob-poc` | `refactor/semantic-embedder-adapter` | `d76d8be9842c960e06841a4cc661d03ad44fbe73` | `.cargo/config.toml.example` modified by the earlier standalone-boundary phase |
| `/Users/adamtc007/dev/bpmn-lite` | `feat/dir-002-phase-c-slm-training` | `ddd143e8258b17593ab6282742fa84e5795cdb30` | concurrent application/model work plus programme documents; preserved |

Additional worktrees remain `/Users/adamtc007/dev/dsl-sem-os-decision-board` at `edded43` and `/Users/adamtc007/Developer/ob-poc-bpmn-pack-truth` at `d2afc0c4`. Neither is selected for Phase 4.

Shared and BPMN use Rust/Cargo 1.95; the active ob-poc checkout resolves Rust/Cargo 1.96.1. The shared MSRV remains 1.95.

## Existing implementation

BPMN's default-off `utterance-engine/embed` feature depends on `ob-semantic-matcher` from `https://github.com/adamtc007/ob-poc-rust` at exact revision `ff3f12c7c0dfa4ac9c8a7bc086162fc2bcecb67e`, with matcher default features disabled. BPMN imports only `ob_semantic_matcher::Embedder` and calls `new`, `embed_query`, `embed_target`, the two batch methods, `embedding_dim`, and `model_name`.

The pinned matcher successfully excludes its optional `pg` feature, so SQLx and pgvector are already absent from the BPMN embedding closure. It nevertheless brings many unrelated unconditional application-matcher dependencies, exposes `anyhow` from the reusable embedder API, combines host-neutral and host modules in one package, and loads weights through an unsafe memory map.

The active ob-poc matcher is application-owned and currently contains:

- Candle/Hugging Face BGE embedding;
- PostgreSQL and pgvector repositories;
- client-group resolution and host SQL;
- feedback analysis, learning and promotion;
- centroid and phonetic matching;
- the `populate_embeddings` application binary.

Its current `Embedder::new` additionally probes three application-relative fine-tuned model directories before falling back to the pinned Hugging Face model. That host-specific search behaviour must remain in the ob-poc adapter rather than move into the shared crate.

## Frozen model contract

- model repository: `BAAI/bge-small-en-v1.5`;
- revision: `5c38ec7c405ec4b44b94cc5a9bb96e735b38267a`;
- dimension: 384;
- query prefix: `Represent this sentence for searching relevant passages: `;
- target prefix: none;
- maximum input length: 512 tokens;
- pooling: CLS position zero;
- normalization: L2 with `1e-12` lower clamp;
- device: CPU;
- model licence: MIT, as declared in the cached pinned model card.

The pinned model snapshot is locally available, allowing a no-download native inference comparison at the Phase 4 gate.

## Baseline risks and controls

1. Safe buffered safetensors loading uses more resident memory during initialization than the prior mmap path. It is selected to keep the shared core safe; initialization time and inference equality will be recorded.
2. A direct type move would lose ob-poc's local fine-tuned path search. The application retains a wrapper facade.
3. Enabling Hugging Face in the shared crate's default feature would introduce an ambient network/cache behaviour. Defaults remain empty and BPMN explicitly opts into the download feature only through its existing `embed` feature.
4. The BPMN workspace has unrelated formatting and Clippy baseline debt documented in Phase 3. Phase 4 will not format or lint-fix unrelated files.
5. The active ob-poc branch's `.cargo/config.toml.example` modification is user/earlier-phase work and will not be included in the Phase 4 adapter commit.
