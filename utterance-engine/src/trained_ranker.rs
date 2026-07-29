//! Trained-bundle loading and scoring — extracted from
//! `examples/score_trained_bundle.rs` 2026-07-29 so `starter_seed_eval.rs`
//! (DIR-003 Phase 3) shares the exact same load/score path rather than a
//! second, drifting copy (A1's "one serializer" principle, applied here
//! to "one trained-scoring path" as it already was to "one fixture set").
//!
//! Key-layout note: `train_slm.py` exports a uniform `encoder.*` +
//! `head.weight`/`head.bias` layout across four architectures. Loading it
//! back into candle-transformers needs a per-architecture remap, verified
//! against the actual saved tensor names, not assumed:
//!   - `ModernBert::load` hardcodes an internal "model." segment
//!     regardless of the vb it's given -> strip "encoder.", add "model."
//!     back.
//!   - `BertModel::load` / `XLMRobertaModel::new` use the given vb's root
//!     directly (no hardcoded segment) -> strip "encoder.", add nothing.

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::{Linear, Module, VarBuilder};
use candle_transformers::models::{bert, modernbert, xlm_roberta};
use hf_hub::{api::sync::Api, Repo, RepoType};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer};

use crate::contract::{rank_canonically, FiniteScore, RankedCandidate, SlmResult};
use crate::corpus_schema::Example;

pub const MAX_LENGTH: usize = 256; // must match train_slm.py's MAX_LENGTH

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Base {
    GteModernbert,
    MsMarco,
    ModernbertBase,
    BgeReranker,
}

impl Base {
    pub const ALL: [Base; 4] = [Base::GteModernbert, Base::MsMarco, Base::ModernbertBase, Base::BgeReranker];

    pub fn key(self) -> &'static str {
        match self {
            Base::GteModernbert => "gte-modernbert",
            Base::MsMarco => "ms-marco",
            Base::ModernbertBase => "modernbert-base",
            Base::BgeReranker => "bge-reranker",
        }
    }

    pub fn from_key(k: &str) -> Option<Base> {
        Base::ALL.into_iter().find(|b| b.key() == k)
    }

    /// Same HF pins as candle_loadability_probe.rs -- architecture
    /// hyperparameters only (config.json). Weights come from the local
    /// trained bundle, never from this download.
    pub fn repo_and_revision(self) -> (&'static str, &'static str) {
        match self {
            Base::GteModernbert => (
                "Alibaba-NLP/gte-reranker-modernbert-base",
                "f7481e6055501a30fb19d090657df9ec1f79ab2c",
            ),
            Base::MsMarco => (
                "cross-encoder/ms-marco-MiniLM-L6-v2",
                "c5ee24cb16019beea0893ab7796b1df96625c6b8",
            ),
            Base::ModernbertBase => (
                "answerdotai/ModernBERT-base",
                "8949b909ec900327062f0ebf497f51aef5e6f0c8",
            ),
            Base::BgeReranker => (
                "BAAI/bge-reranker-base",
                "2cfc18c9415c912f9d8155881c133215df768a70",
            ),
        }
    }
}

/// Remap `train_slm.py`'s bundle key layout to what each candle-transformers
/// loader expects, per the module doc's derivation. Returns (encoder_tensors,
/// head_weight, head_bias).
fn remap_bundle_tensors(
    base: Base,
    raw: HashMap<String, Tensor>,
) -> Result<(HashMap<String, Tensor>, Tensor, Tensor)> {
    let mut encoder = HashMap::new();
    let mut head_weight = None;
    let mut head_bias = None;
    for (k, v) in raw {
        if let Some(rest) = k.strip_prefix("encoder.") {
            let new_key = match base {
                Base::GteModernbert | Base::ModernbertBase => format!("model.{rest}"),
                Base::MsMarco | Base::BgeReranker => rest.to_string(),
            };
            encoder.insert(new_key, v);
        } else if k == "head.weight" {
            head_weight = Some(v);
        } else if k == "head.bias" {
            head_bias = Some(v);
        }
    }
    let head_weight = head_weight.ok_or_else(|| anyhow!("bundle missing head.weight"))?;
    let head_bias = head_bias.ok_or_else(|| anyhow!("bundle missing head.bias"))?;
    Ok((encoder, head_weight, head_bias))
}

enum Encoder {
    ModernBert(modernbert::ModernBert),
    Bert(bert::BertModel),
    XlmRoberta(xlm_roberta::XLMRobertaModel),
}

impl Encoder {
    /// (K, seq, H) hidden states -- uniform across the three
    /// architectures despite their different native forward signatures.
    fn forward(&self, ids: &Tensor, mask: &Tensor, token_type_ids: &Tensor) -> Result<Tensor> {
        Ok(match self {
            Encoder::ModernBert(m) => m.forward(ids, mask)?,
            Encoder::Bert(m) => m.forward(ids, token_type_ids, Some(mask))?,
            Encoder::XlmRoberta(m) => m.forward(ids, mask, token_type_ids, None, None, None)?,
        })
    }
}

pub struct TrainedRanker {
    encoder: Encoder,
    head: Linear,
    pooling: String, // "cls" | "mean", read from the bundle's training_card.json
    tokenizer: Tokenizer,
    bundle_identity: String,
}

fn pool(hidden: &Tensor, mask: &Tensor, pooling: &str) -> Result<Tensor> {
    Ok(match pooling {
        "cls" => hidden.i((.., 0, ..))?,
        "mean" => {
            let m = mask.unsqueeze(2)?.to_dtype(hidden.dtype())?;
            let summed = hidden.broadcast_mul(&m)?.sum(1)?;
            let counts = m.sum(1)?.clamp(1e-6, f64::MAX)?;
            summed.broadcast_div(&counts)?
        }
        other => anyhow::bail!("unknown pooling '{other}'"),
    })
}

impl TrainedRanker {
    pub fn load(base: Base, bundle_dir: &std::path::Path, device: &Device) -> Result<Self> {
        let card: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(bundle_dir.join("training_card.json"))?)
                .context("training_card.json")?;
        let pooling = card["pooling"].as_str().context("card.pooling")?.to_string();

        let (repo_id, revision) = base.repo_and_revision();
        let api = Api::new()?;
        let repo = api.repo(Repo::with_revision(repo_id.to_string(), RepoType::Model, revision.to_string()));
        let config_str = std::fs::read_to_string(repo.get("config.json").context("download config.json")?)?;

        let raw = candle_core::safetensors::load(bundle_dir.join("model.safetensors"), device)
            .context("load bundle safetensors")?;
        let (encoder_tensors, head_weight, head_bias) = remap_bundle_tensors(base, raw)?;
        let vb = VarBuilder::from_tensors(encoder_tensors, DType::F32, device);

        let encoder = match base {
            Base::GteModernbert | Base::ModernbertBase => {
                let cfg: modernbert::Config = serde_json::from_str(&config_str)?;
                Encoder::ModernBert(modernbert::ModernBert::load(vb, &cfg)?)
            }
            Base::MsMarco => {
                let cfg: bert::Config = serde_json::from_str(&config_str)?;
                Encoder::Bert(bert::BertModel::load(vb, &cfg)?)
            }
            Base::BgeReranker => {
                let cfg: xlm_roberta::Config = serde_json::from_str(&config_str)?;
                Encoder::XlmRoberta(xlm_roberta::XLMRobertaModel::new(&cfg, vb)?)
            }
        };
        let head = Linear::new(head_weight, Some(head_bias));

        let mut tokenizer = Tokenizer::from_file(bundle_dir.join("tokenizer.json"))
            .map_err(|e| anyhow!("Tokenizer::from_file: {e}"))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
        tokenizer
            .with_truncation(None)
            .map_err(|e| anyhow!("with_truncation: {e}"))?;

        Ok(TrainedRanker {
            encoder,
            head,
            pooling,
            tokenizer,
            bundle_identity: format!("slm.trained.{}@{}", base.key(), revision.get(..8).unwrap_or(revision)),
        })
    }

    pub fn bundle_identity(&self) -> &str {
        &self.bundle_identity
    }

    /// Scores every candidate in `record.tier1_list` -- the SAME list
    /// shape training/serving share (Adam's finding-5 ruling) -- never
    /// the full board.
    pub fn score(&self, record: &Example, device: &Device) -> Result<SlmResult> {
        self.score_list(record, &record.tier1_list, device)
    }

    /// Explicit-list variant: the accuracy path above always serves
    /// tier1_list; this exists for the latency-vs-K probe (timing the
    /// same forward pass at widened list sizes, e.g. the full board) and
    /// for scoring constructed candidate pairs (ambiguity-set suite).
    pub fn score_list(&self, record: &Example, cand_ids: &[String], device: &Device) -> Result<SlmResult> {
        let query_text = format!("{}\n\n{}", record.utterance, record.context_projection);
        let desc: HashMap<&str, &str> = record
            .board
            .candidates
            .iter()
            .map(|c| (c.canonical_id.as_str(), c.description.as_str()))
            .collect();
        let cand_texts: Vec<&str> = cand_ids
            .iter()
            .map(|id| {
                desc.get(id.as_str())
                    .copied()
                    .ok_or_else(|| anyhow!("candidate id '{id}' not on board"))
            })
            .collect::<Result<_>>()?;

        let pairs: Vec<(String, String)> = cand_texts
            .iter()
            .map(|c| (query_text.clone(), c.to_string()))
            .collect();
        let encodings = self
            .tokenizer
            .encode_batch(pairs, true)
            .map_err(|e| anyhow!("encode_batch: {e}"))?;

        let k = encodings.len();
        let seq_len = encodings[0].get_ids().len().min(MAX_LENGTH);
        let mut ids = vec![0u32; k * seq_len];
        let mut mask = vec![0u32; k * seq_len];
        let mut ttype = vec![0u32; k * seq_len];
        for (i, enc) in encodings.iter().enumerate() {
            let e_ids = enc.get_ids();
            let e_mask = enc.get_attention_mask();
            let e_type = enc.get_type_ids();
            let n = e_ids.len().min(seq_len);
            ids[i * seq_len..i * seq_len + n].copy_from_slice(&e_ids[..n]);
            mask[i * seq_len..i * seq_len + n].copy_from_slice(&e_mask[..n]);
            ttype[i * seq_len..i * seq_len + n].copy_from_slice(&e_type[..n]);
        }
        let ids = Tensor::from_vec(ids, (k, seq_len), device)?;
        let mask_t = Tensor::from_vec(mask, (k, seq_len), device)?;
        let ttype = Tensor::from_vec(ttype, (k, seq_len), device)?;

        let hidden = self.encoder.forward(&ids, &mask_t, &ttype)?;
        let pooled = pool(&hidden, &mask_t, &self.pooling)?;
        let logits = self.head.forward(&pooled)?.squeeze(1)?; // (K,)
        let logits: Vec<f32> = logits.to_vec1()?;

        let mut ranking: Vec<RankedCandidate> = cand_ids
            .iter()
            .zip(logits.iter())
            .map(|(id, &score)| {
                Ok(RankedCandidate { candidate_id: id.clone(), score: FiniteScore::new(score as f64)? })
            })
            .collect::<Result<_>>()?;
        rank_canonically(&mut ranking);

        let mut pre = Vec::new();
        pre.extend_from_slice(b"subset.v1:");
        for rc in &ranking {
            pre.extend_from_slice(rc.candidate_id.len().to_string().as_bytes());
            pre.push(b':');
            pre.extend_from_slice(rc.candidate_id.as_bytes());
        }
        Ok(SlmResult {
            ranking,
            retrieved_subset_hash: blake3::hash(&pre).to_hex().to_string(),
            board_hash: record.board_hash.clone(),
            model_bundle_hash: self.bundle_identity.clone(),
        })
    }
}
