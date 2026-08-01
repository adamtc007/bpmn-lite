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

const MAX_LENGTH: usize = 256; // must match train_slm.py's MAX_LENGTH

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
        self.score_query(&query_text, cand_ids, &desc, &record.board_hash, device)
    }

    /// Serving-side variant (DIR-002 serving integration, 2026-08-01):
    /// same encoding path as `score_list` — query text is
    /// `utterance \n\n context_projection`, EXACTLY the corpus records'
    /// preimage (A1: one textualisation) — but sourced from a live
    /// `Board` rather than a corpus `Example`.
    pub fn score_serving(
        &self,
        utterance: &str,
        context_projection: &str,
        board: &crate::board::Board,
        cand_ids: &[String],
        device: &Device,
    ) -> Result<SlmResult> {
        let query_text = format!("{utterance}\n\n{context_projection}");
        let desc: HashMap<&str, &str> = board
            .candidates
            .iter()
            .map(|c| (c.canonical_id.as_str(), c.description.as_str()))
            .collect();
        self.score_query(&query_text, cand_ids, &desc, &board.board_hash, device)
    }

    /// The ONE forward-pass core both `score_list` (corpus/eval) and
    /// `score_serving` (live endpoint) share — extracted so the serving
    /// path cannot drift from the mechanics the bake-off measured.
    fn score_query(
        &self,
        query_text: &str,
        cand_ids: &[String],
        desc: &HashMap<&str, &str>,
        board_hash: &str,
        device: &Device,
    ) -> Result<SlmResult> {
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
            .map(|c| (query_text.to_string(), c.to_string()))
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
            board_hash: board_hash.to_owned(),
            model_bundle_hash: self.bundle_identity.clone(),
        })
    }
}

/// Temperature-calibrated listwise probabilities (spec §10.8: the
/// calibration temperature is SEALED in the bundle's training card, never
/// caller-invented): softmax over `logit / t` across the served list.
/// Refuses a non-finite or non-positive temperature — a broken card must
/// fail the load, not silently serve uncalibrated scores.
pub fn calibrated_probabilities(logits: &[f64], temperature: f64) -> Result<Vec<f64>> {
    if !temperature.is_finite() || temperature <= 0.0 {
        return Err(anyhow!("refused calibration temperature {temperature} — must be finite and > 0"));
    }
    let scaled: Vec<f64> = logits.iter().map(|l| l / temperature).collect();
    let max = scaled.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let exps: Vec<f64> = scaled.iter().map(|l| (l - max).exp()).collect();
    let sum: f64 = exps.iter().sum();
    if !(sum.is_finite() && sum > 0.0) {
        return Err(anyhow!("calibrated softmax degenerate (sum {sum})"));
    }
    Ok(exps.iter().map(|e| e / sum).collect())
}

/// The tier-1 SERVING producer (DIR-002 serving integration, ruled
/// 2026-08-01): tier-0 retrieve → `tier1_list` K-subset (+NOTA) →
/// trained-bundle listwise scoring → calibrated probabilities. Output is
/// the same `SlmResult` EVIDENCE shape every tier-0 emits; `policy::decide`
/// stays the only disposition issuer (I27) and accepts the K-subset
/// ranking unchanged.
pub struct Tier1Ranker {
    ranker: TrainedRanker,
    /// Sealed calibration temperature from the bundle's `training_card.json`.
    temperature: f64,
    /// `<identity>#<content hash>` — identity string plus the bundle's own
    /// recorded `model.safetensors` hash (the card's `ratification.bundle_hash`
    /// when present, else blake3 of the file bytes), so the I28
    /// `model_bundle_hash` names the actual weights, not just a label.
    model_bundle_hash: String,
    device: Device,
}

impl Tier1Ranker {
    /// Loads the ranker, the sealed temperature, and the bundle content
    /// hash from `bundle_dir`. CPU device (T3.4a: serving latency is the
    /// CPU story). Fails closed on a missing/unsealed temperature.
    pub fn load(base: Base, bundle_dir: &std::path::Path) -> Result<Self> {
        let device = Device::Cpu;
        let card: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(bundle_dir.join("training_card.json"))?)
                .context("training_card.json")?;
        let temperature = card["temperature"]
            .as_f64()
            .ok_or_else(|| anyhow!("bundle card missing sealed calibration 'temperature' (§10.8)"))?;
        if !temperature.is_finite() || temperature <= 0.0 {
            return Err(anyhow!("bundle card temperature {temperature} refused"));
        }
        let content_hash = match card["ratification"]["bundle_hash"].as_str() {
            Some(h) if !h.is_empty() => h.to_owned(),
            _ => blake3::hash(&std::fs::read(bundle_dir.join("model.safetensors"))?)
                .to_hex()
                .to_string(),
        };
        let ranker = TrainedRanker::load(base, bundle_dir, &device)?;
        let model_bundle_hash = format!("{}#{content_hash}", ranker.bundle_identity());
        Ok(Tier1Ranker { ranker, temperature, model_bundle_hash, device })
    }

    pub fn model_bundle_hash(&self) -> &str {
        &self.model_bundle_hash
    }

    pub fn temperature(&self) -> f64 {
        self.temperature
    }

    /// The full tier-0 → tier-1 serving pass. `tier0` supplies the
    /// high-recall ranking; the served list is `tier1_list` at the ONE
    /// standing `TIER1_K` (training-list shape = serving-list shape);
    /// scores are calibrated probabilities over that list.
    pub fn rank(
        &self,
        tier0: &dyn crate::retrieval::Tier0Retriever,
        utterance: &str,
        context_projection: &str,
        board: &crate::board::Board,
    ) -> Result<SlmResult> {
        let tier0_evidence = tier0.retrieve(utterance, board)?;
        let list = crate::retrieval::tier1_list(&tier0_evidence, crate::retrieval::TIER1_K);
        let raw = self
            .ranker
            .score_serving(utterance, context_projection, board, &list, &self.device)?;

        // Calibrate over the ranking as scored (order-preserving: softmax
        // is monotone, so the canonical rank order is unchanged).
        let logits: Vec<f64> = raw.ranking.iter().map(|rc| rc.score.get()).collect();
        let probs = calibrated_probabilities(&logits, self.temperature)?;
        let mut ranking: Vec<RankedCandidate> = raw
            .ranking
            .iter()
            .zip(probs.iter())
            .map(|(rc, &p)| {
                Ok(RankedCandidate { candidate_id: rc.candidate_id.clone(), score: FiniteScore::new(p)? })
            })
            .collect::<Result<_>>()?;
        rank_canonically(&mut ranking);

        Ok(SlmResult {
            ranking,
            retrieved_subset_hash: raw.retrieved_subset_hash,
            board_hash: board.board_hash.clone(),
            model_bundle_hash: self.model_bundle_hash.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hermetic calibration receipts: temperature actually rescales the
    /// distribution (T>1 flattens), output is a probability simplex, and
    /// broken temperatures are refused (red half).
    #[test]
    fn calibration_rescales_and_fails_closed() {
        let logits = [2.0, 0.5, -1.0];
        let p1 = calibrated_probabilities(&logits, 1.0).unwrap();
        let p_t = calibrated_probabilities(&logits, 1.4303200528314213).unwrap();
        assert!((p1.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!((p_t.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(p_t[0] < p1[0], "T>1 must FLATTEN the top probability: {} vs {}", p_t[0], p1[0]);
        assert!(p_t[2] > p1[2], "T>1 must lift the tail");
        assert!(p_t[0] > p_t[1] && p_t[1] > p_t[2], "monotone: order preserved");
        assert!(calibrated_probabilities(&logits, 0.0).is_err(), "T=0 refused");
        assert!(calibrated_probabilities(&logits, f64::NAN).is_err(), "NaN T refused");
        assert!(calibrated_probabilities(&logits, -1.0).is_err(), "negative T refused");
    }

    /// Real-bundle serving receipt (K=12 list, sealed temperature, SlmResult
    /// shape, populated bundle hash). Requires the trained bundle on disk +
    /// hf-hub cache/network for the base's config.json, so it is #[ignore]d:
    /// run with
    ///   SLM_BUNDLE_DIR=utterance-engine/train_py/bundles/modernbert-base \
    ///   cargo test -p utterance-engine --features candle-probe --release -- --ignored tier1_serving
    #[test]
    #[ignore = "needs SLM_BUNDLE_DIR trained bundle + hf-hub config.json cache"]
    fn tier1_serving_k12_calibrated_evidence_shape() {
        use crate::board::{build_board, EmptyUniverse, PolicyFilter};
        use designer_graph::board_candidate::{LegalityOracle, OperationKind, ProductionId};

        struct AllLegal;
        impl LegalityOracle for AllLegal {
            type NodeKey = ();
            fn legal_operations(&self, _: Option<&()>) -> Vec<OperationKind> {
                OperationKind::ALL.to_vec()
            }
            fn legal_productions(&self, _: Option<&()>) -> Vec<ProductionId> {
                ProductionId::ALL.to_vec()
            }
        }

        let bundle_dir = std::path::PathBuf::from(
            std::env::var("SLM_BUNDLE_DIR").expect("set SLM_BUNDLE_DIR to a trained bundle dir"),
        );
        let t1 = Tier1Ranker::load(Base::ModernbertBase, &bundle_dir).expect("bundle load");
        assert!(
            t1.model_bundle_hash().starts_with("slm.trained.modernbert-base@") && t1.model_bundle_hash().contains('#'),
            "bundle hash must carry identity#content-hash: {}",
            t1.model_bundle_hash()
        );
        assert!(t1.temperature() > 0.0);

        let board = build_board(&AllLegal, None, Some("rev0"), &EmptyUniverse, &PolicyFilter::default()).unwrap();
        let ctx = crate::context::minimal("pack.none", "g-test");
        let ev = t1
            .rank(&crate::retrieval::LexicalTier0, "chase them again", &ctx.serialize_canonical(), &board)
            .expect("tier-1 rank");

        // K=12 list construction: the TIER1_K prefix, +1 only when NOTA
        // was cut by it (tier1_list's contract). The full board is
        // larger — the subset rule is doing real work here.
        assert!(
            ev.ranking.len() == crate::retrieval::TIER1_K
                || ev.ranking.len() == crate::retrieval::TIER1_K + 1,
            "K-subset (+NOTA when cut): got {}",
            ev.ranking.len()
        );
        assert!(board.candidates.len() > ev.ranking.len(), "subset, not the board");
        assert!(ev.ranking.iter().any(|rc| rc.candidate_id == crate::contract::NONE_OF_THE_ABOVE));
        for rc in &ev.ranking {
            assert!(board.contains(&rc.candidate_id), "off-board: {}", rc.candidate_id);
        }
        // Temperature applied → probability simplex, not raw logits.
        let sum: f64 = ev.ranking.iter().map(|rc| rc.score.get()).sum();
        assert!((sum - 1.0).abs() < 1e-9, "calibrated probabilities must sum to 1, got {sum}");
        assert_eq!(ev.board_hash, board.board_hash);
        assert_eq!(ev.model_bundle_hash, t1.model_bundle_hash());
    }
}
