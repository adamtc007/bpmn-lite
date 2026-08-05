# Shared-crates remediation Phase 4 blueprint

**Phase:** 4 — extract the pure semantic embedder
**Date:** 5 August 2026
**Shared base:** DSL `ca006a785e1545cf71e2870c4dffe9d7bb5147e8` on `feat/semantic-embedder`
**BPMN base:** `0bf5c058fdc8e2ea86825823816757261cd31b9b` on `refactor/semantic-embedder`
**ob-poc base:** `d76d8be9842c960e06841a4cc661d03ad44fbe73` on `refactor/semantic-embedder-adapter`

## Invariants and absolute boundaries

1. The extraction preserves BGE-small-en-v1.5 query prefixing, target handling, 512-token truncation, CLS pooling, L2 normalization, model identity, pinned model revision, embedding dimension and existing public matcher behaviour.
2. The shared crate has no SQLx, pgvector, PostgreSQL, UUID, Tokio, application schema, feedback, centroid, client-group, phonetic, population-binary, HTTP or host dependency.
3. `default = []`. Candle inference and Hugging Face retrieval are separate explicit features. A no-feature build compiles the trait, typed errors, model identity, bundle paths and deterministic fake without model/runtime dependencies.
4. Hugging Face is never ambient in the core API. Network/cache resolution is compiled only by `huggingface-download`; loading a caller-provided local bundle requires only `candle`.
5. Public reusable APIs return `EmbeddingError`, never `anyhow::Error`. Application adapters may translate this typed error into their existing error boundary.
6. The shared crate forbids unsafe code. Model weights are loaded through Candle's safe buffered safetensors API rather than the former unsafe memory-mapped loader.
7. The active `ob-poc` matcher remains an application-owned persistence/matching crate. It wraps and re-exports the shared inference capability while retaining its local fine-tuned-model discovery and existing `Embedder` facade.
8. BPMN imports `semantic-embedder` directly. Its `embed` feature remains off by default and preserves the existing download-capable behaviour when explicitly enabled.
9. No model weights, tokenizer, database schema, pack, trained bundle, threshold or deployment artifact changes. The model card in the pinned local Hugging Face snapshot declares MIT; the extraction moves code only.
10. The exact new DSL revision is pinned in both consumers before their gates run. Development path patches stay ignored and uncommitted.

## Shared module structure

```text
crates/semantic-embedder/
  Cargo.toml
  README.md
  src/
    lib.rs          # facade, constants, trait, fake, feature exports
    error.rs        # typed public failure taxonomy
    model.rs        # model identity and validated local bundle paths
    candle.rs       # feature-gated Candle BERT implementation
    download.rs     # feature-gated pinned Hugging Face resolver
```

Normative public surface:

```rust
pub trait Embedder: Send + Sync {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    fn embed_target(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    fn embed_batch_queries(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError>;
    fn embed_batch_targets(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError>;
    fn embedding_dim(&self) -> usize;
    fn model_identity(&self) -> &ModelIdentity;
}

pub struct ModelIdentity { /* repository, revision, dimension */ }
pub struct ModelBundle { /* config, tokenizer and weights paths */ }
pub enum EmbeddingError { /* path, config, tokenizer, weights, model, inference, download */ }
pub struct DeterministicFakeEmbedder { /* fixed dimension and identity */ }

#[cfg(feature = "candle")]
pub struct CandleEmbedder { /* BertModel, Tokenizer, Device, ModelIdentity */ }
```

`CandleEmbedder` provides `from_bundle` and `from_directory`. With `huggingface-download`, it also provides the compatibility constructors `new`, `with_model` and `with_model_and_revision`. The compatibility constructors resolve a directory locally or fetch an exact repository revision.

## Feature graph

```text
default = []
candle = candle-core + candle-nn + candle-transformers + tokenizers + serde_json + tracing
huggingface-download = candle + hf-hub/ureq
metal = candle + Candle metal features
```

The feature gate must prove that `cargo tree -p semantic-embedder --no-default-features` contains none of Candle, tokenizers or hf-hub; `candle` contains no hf-hub; and every feature closure contains neither SQLx nor pgvector.

## ob-poc adapter

`rust/crates/ob-semantic-matcher/src/embedder.rs` becomes a thin application facade around `semantic_embedder::CandleEmbedder`. It retains:

- `Embedder::new` search order for the three local fine-tuned model directories;
- remote fallback to the pinned default model;
- current method names and return shapes;
- the legacy target-defaulting methods for existing application callers.

All feedback, database, centroid, client-group, phonetic and population code remains in the application crate. Its direct Candle, tokenizer and Hugging Face dependencies are removed because it now consumes the shared crate.

## BPMN cutover

- Replace the optional `ob-semantic-matcher` Git dependency with `semantic-embedder` at the new exact DSL commit.
- Map `embed` to the dependency plus its `huggingface-download` feature, preserving current opt-in behaviour.
- Replace `ob_semantic_matcher::Embedder` with `semantic_embedder::CandleEmbedder` in `retrieval.rs`.
- Extend the shared pin guard and self-tests from seven to eight packages.
- Regenerate the root lockfile with the local patch disabled and run exact-Git gates.

## Verification design

- no-feature unit tests cover model identity validation and the deterministic fake;
- invalid/missing bundle files and invalid tokenizer JSON produce typed error variants;
- Candle local-bundle inference is deterministic across repeated calls;
- a gate-time harness compares old pinned and extracted BGE output for the same cached model and phrases, recording maximum absolute delta and cosine similarity;
- ob-poc focused tests prove its facade preserves local-directory selection and method forwarding without database access;
- BPMN retrieval tests compile and pass under `embed`;
- Cargo metadata/tree assertions prove database packages are absent from the embedding closure;
- full shared, focused ob-poc, and full BPMN consumer gates follow focused success.

## Expected commits

1. DSL: `feat: extract host-neutral semantic embedder`.
2. ob-poc: `refactor: consume shared semantic embedder`.
3. BPMN: `refactor: consume shared semantic embedder`.

Phase 4 stops after the receipt and these scoped commits. Phase 6 forensic cleanup does not begin until the gate is reviewed.
