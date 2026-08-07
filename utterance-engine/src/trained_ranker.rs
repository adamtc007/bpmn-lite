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
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams, TruncationStrategy};

use crate::contract::{rank_canonically, FiniteScore, RankedCandidate, SlmResult};
use crate::corpus_schema::{Example, SemanticCorpusClosure};

const MAX_LENGTH: usize = 256; // must match train_slm.py's MAX_LENGTH

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BundleInputMode {
    SemanticCandidatePairsV3,
}

impl BundleInputMode {
    fn from_admitted_card(card: &serde_json::Value) -> Result<Self> {
        if card["corpus_schema_id"].as_str() == Some(crate::corpus_schema::CORPUS_SCHEMA_ID)
            && card["pair_serializer_id"].as_str() == Some(crate::pair::PAIR_SERIALIZER_ID)
        {
            Ok(Self::SemanticCandidatePairsV3)
        } else {
            anyhow::bail!("bundle card does not declare an admitted scoring input mode")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScoringRoute {
    LegacyDescription,
    SemanticCandidatePairsV3,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum ScoringRouteAdmissionError {
    #[error("semantic-v3 bundle refused legacy utterance/context plus description textualisation")]
    V3LegacyDescriptionRefused,
}

fn admit_scoring_route(
    bundle_mode: BundleInputMode,
    route: ScoringRoute,
) -> std::result::Result<(), ScoringRouteAdmissionError> {
    match (bundle_mode, route) {
        (BundleInputMode::SemanticCandidatePairsV3, ScoringRoute::SemanticCandidatePairsV3) => {
            Ok(())
        }
        (BundleInputMode::SemanticCandidatePairsV3, ScoringRoute::LegacyDescription) => {
            Err(ScoringRouteAdmissionError::V3LegacyDescriptionRefused)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Base {
    GteModernbert,
    MsMarco,
    ModernbertBase,
    BgeReranker,
}

impl Base {
    pub const ALL: [Base; 4] = [
        Base::GteModernbert,
        Base::MsMarco,
        Base::ModernbertBase,
        Base::BgeReranker,
    ];

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
    input_mode: BundleInputMode,
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
        let card: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
            bundle_dir.join("training_card.json"),
        )?)
        .context("training_card.json")?;
        validate_bundle_card(&card, bundle_dir)?;
        let input_mode = BundleInputMode::from_admitted_card(&card)?;
        let pooling = card["pooling"]
            .as_str()
            .context("card.pooling")?
            .to_string();

        let (repo_id, revision) = base.repo_and_revision();
        let api = Api::new()?;
        let repo = api.repo(Repo::with_revision(
            repo_id.to_string(),
            RepoType::Model,
            revision.to_string(),
        ));
        let config_str =
            std::fs::read_to_string(repo.get("config.json").context("download config.json")?)?;

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
            .with_truncation(Some(TruncationParams {
                max_length: MAX_LENGTH,
                strategy: TruncationStrategy::LongestFirst,
                ..Default::default()
            }))
            .map_err(|e| anyhow!("with_truncation: {e}"))?;

        Ok(TrainedRanker {
            encoder,
            head,
            pooling,
            tokenizer,
            bundle_identity: format!(
                "slm.trained.{}@{}",
                base.key(),
                revision.get(..8).unwrap_or(revision)
            ),
            input_mode,
        })
    }

    pub fn bundle_identity(&self) -> &str {
        &self.bundle_identity
    }

    /// Scores the persisted semantic-v3 candidate pairs for the complete
    /// board. A legacy record without the v3 closure is refused before any
    /// textualisation or model work occurs.
    pub fn score(&self, record: &Example, device: &Device) -> Result<SlmResult> {
        admit_scoring_route(self.input_mode, ScoringRoute::SemanticCandidatePairsV3)?;
        let closure = admit_semantic_v3_record(record)?;
        let pairs = closure
            .candidate_pairs
            .iter()
            .map(|candidate| (candidate.pair.side_a.clone(), candidate.pair.side_b.clone()))
            .collect::<Vec<_>>();
        self.score_pairs(
            &pairs,
            &closure.full_served_list,
            &record.board_hash,
            device,
        )
    }

    /// Compatibility signature retained for existing callers. Every admitted
    /// v3 bundle refuses this legacy description-text route before scoring;
    /// callers must use [`Self::score`] with a complete semantic-v3 closure.
    pub fn score_list(
        &self,
        record: &Example,
        cand_ids: &[String],
        device: &Device,
    ) -> Result<SlmResult> {
        admit_scoring_route(self.input_mode, ScoringRoute::LegacyDescription)?;
        let query_text = format!("{}\n\n{}", record.utterance, record.context_projection);
        let desc: HashMap<&str, &str> = record
            .board
            .candidates
            .iter()
            .map(|c| (c.canonical_id.as_str(), c.description.as_str()))
            .collect();
        self.score_query(&query_text, cand_ids, &desc, &record.board_hash, device)
    }

    /// Compatibility signature for legacy thin boards. An admitted v3 bundle
    /// refuses this route; live graph-backed serving uses
    /// [`Tier1Ranker::rank_full_board`].
    pub fn score_serving(
        &self,
        utterance: &str,
        context_projection: &str,
        board: &dyn crate::board::InferenceBoard,
        cand_ids: &[String],
        device: &Device,
    ) -> Result<SlmResult> {
        admit_scoring_route(self.input_mode, ScoringRoute::LegacyDescription)?;
        let query_text = format!("{utterance}\n\n{context_projection}");
        let candidates = board.inference_candidates();
        let desc: HashMap<&str, &str> = candidates
            .iter()
            .map(|c| (c.canonical_id.as_str(), c.description.as_str()))
            .collect();
        self.score_query(&query_text, cand_ids, &desc, board.board_hash(), device)
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
        self.score_pairs(&pairs, cand_ids, board_hash, device)
    }

    fn score_pairs(
        &self,
        pairs: &[(String, String)],
        cand_ids: &[String],
        board_hash: &str,
        device: &Device,
    ) -> Result<SlmResult> {
        let encodings = self
            .tokenizer
            .encode_batch(pairs.to_vec(), true)
            .map_err(|e| anyhow!("encode_batch: {e}"))?;

        let k = encodings.len();
        let seq_len = encodings[0].get_ids().len();
        let mut ids = vec![0u32; k * seq_len];
        let mut mask = vec![0u32; k * seq_len];
        let mut ttype = vec![0u32; k * seq_len];
        for (i, enc) in encodings.iter().enumerate() {
            let e_ids = enc.get_ids();
            let e_mask = enc.get_attention_mask();
            let e_type = enc.get_type_ids();
            let n = e_ids.len();
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
                Ok(RankedCandidate {
                    candidate_id: id.clone(),
                    score: FiniteScore::new(score as f64)?,
                })
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
            evidence_trace: None,
            inference_evidence: None,
            move_evidence: Vec::new(),
        })
    }
}

fn validate_bundle_card(card: &serde_json::Value, bundle_dir: &std::path::Path) -> Result<()> {
    let required = |name: &str| {
        card[name]
            .as_str()
            .filter(|value| !value.is_empty())
            .with_context(|| format!("bundle card missing '{name}'"))
    };
    let equal = |name: &str, expected: &str| -> Result<()> {
        let actual = required(name)?;
        if actual != expected {
            anyhow::bail!("bundle card {name} mismatch: expected '{expected}', found '{actual}'");
        }
        Ok(())
    };
    equal("corpus_schema_id", crate::corpus_schema::CORPUS_SCHEMA_ID)?;
    equal(
        "corpus_schema_hash",
        &crate::corpus_schema::corpus_schema_hash(),
    )?;
    if card["semantic_board_schema_version"].as_u64() != Some(1) {
        anyhow::bail!("bundle card semantic_board_schema_version mismatch");
    }
    equal(
        "semantic_pack_identity",
        &crate::bpmn_board::bpmn_semantic_snapshot_identity(),
    )?;
    equal("turn_serializer_id", crate::pair::TURN_SERIALIZER_ID)?;
    equal("turn_serializer_hash", &crate::pair::turn_serializer_hash())?;
    equal(
        "candidate_serializer_id",
        crate::exact::CANDIDATE_SERIALIZER_ID,
    )?;
    equal(
        "candidate_serializer_hash",
        &crate::exact::candidate_serializer_hash(),
    )?;
    equal("pair_serializer_id", crate::pair::PAIR_SERIALIZER_ID)?;
    equal("pair_serializer_hash", &crate::pair::pair_serializer_hash())?;

    let budget = crate::pair::PairTokenBudget::default();
    if card["max_length"].as_u64() != Some(MAX_LENGTH as u64)
        || card["side_a_tokens"].as_u64() != Some(budget.side_a_tokens as u64)
        || card["side_b_tokens"].as_u64() != Some(budget.side_b_tokens as u64)
    {
        anyhow::bail!("bundle card token budgets do not match serving");
    }
    let tokenizer_path = bundle_dir.join("tokenizer.json");
    equal("tokenizer_hash", &file_hash(&tokenizer_path)?)?;
    let weights_path = bundle_dir.join("model.safetensors");
    equal("model_weights_hash", &file_hash(&weights_path)?)?;
    required("calibration_set_identity")?;
    required("training_split_manifest_hash")?;
    let temperature = card["temperature"]
        .as_f64()
        .context("bundle card missing 'temperature'")?;
    if !temperature.is_finite() || temperature <= 0.0 {
        anyhow::bail!("bundle card temperature must be finite and positive");
    }
    Ok(())
}

fn file_hash(path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(std::fs::read(path).with_context(|| format!("read {path:?}"))?)
    ))
}

/// Temperature-calibrated listwise probabilities (spec §10.8: the
/// calibration temperature is SEALED in the bundle's training card, never
/// caller-invented): softmax over `logit / t` across the served list.
/// Refuses a non-finite or non-positive temperature — a broken card must
/// fail the load, not silently serve uncalibrated scores.
pub fn calibrated_probabilities(logits: &[f64], temperature: f64) -> Result<Vec<f64>> {
    if !temperature.is_finite() || temperature <= 0.0 {
        return Err(anyhow!(
            "refused calibration temperature {temperature} — must be finite and > 0"
        ));
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
        let card: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
            bundle_dir.join("training_card.json"),
        )?)
        .context("training_card.json")?;
        let temperature = card["temperature"].as_f64().ok_or_else(|| {
            anyhow!("bundle card missing sealed calibration 'temperature' (§10.8)")
        })?;
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
        Ok(Tier1Ranker {
            ranker,
            temperature,
            model_bundle_hash,
            device,
        })
    }

    pub fn model_bundle_hash(&self) -> &str {
        &self.model_bundle_hash
    }

    pub fn temperature(&self) -> f64 {
        self.temperature
    }

    /// Legacy thin-board adapter retained as a compatibility boundary. Every
    /// currently admitted v3 bundle fails closed here because it lacks the
    /// semantic slices required by the candidate-pair serializer.
    pub fn rank(
        &self,
        tier0: &dyn crate::retrieval::Tier0Retriever,
        utterance: &str,
        context_projection: &str,
        board: &dyn crate::board::InferenceBoard,
    ) -> Result<SlmResult> {
        let tier0_evidence = tier0.retrieve(utterance, board)?;
        let list = crate::retrieval::tier1_list(&tier0_evidence, crate::retrieval::TIER1_K);
        let raw =
            self.ranker
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
                Ok(RankedCandidate {
                    candidate_id: rc.candidate_id.clone(),
                    score: FiniteScore::new(p)?,
                })
            })
            .collect::<Result<_>>()?;
        rank_canonically(&mut ranking);

        Ok(SlmResult {
            ranking,
            retrieved_subset_hash: raw.retrieved_subset_hash,
            board_hash: board.board_hash().to_string(),
            model_bundle_hash: self.model_bundle_hash.clone(),
            evidence_trace: None,
            inference_evidence: None,
            move_evidence: Vec::new(),
        })
    }

    /// BPMN production serving path: score every board candidate, including
    /// abstention, through the admitted semantic candidate-pair serializer.
    pub fn rank_full_board(
        &self,
        utterance: &str,
        context: &crate::context::ContextProjection,
        board: &semantic_decision_contracts::SemanticDecisionBoard,
    ) -> Result<SlmResult> {
        admit_scoring_route(
            self.ranker.input_mode,
            ScoringRoute::SemanticCandidatePairsV3,
        )?;
        let list = board
            .candidates
            .iter()
            .map(|candidate| candidate.canonical_id.as_str().to_string())
            .collect::<Vec<_>>();
        let pairs = board
            .candidates
            .iter()
            .map(|candidate| {
                let pair = crate::pair::serialize_candidate_pair(
                    utterance,
                    context,
                    candidate,
                    crate::pair::PairTokenBudget::default(),
                )?;
                // Tokenizer truncation is part of the sealed trained route.
                // A serving-only post-serialization sentinel veto would make
                // Candle refuse the exact pairs Python admitted during
                // training. The fixed Python/Candle parity packet therefore
                // owns this boundary; pair bytes and hashes remain validated
                // before scoring.
                Ok((pair.side_a, pair.side_b))
            })
            .collect::<Result<Vec<_>>>()?;
        let raw =
            self.ranker
                .score_pairs(&pairs, &list, board.board_hash.as_str(), &self.device)?;
        let logits: Vec<f64> = raw
            .ranking
            .iter()
            .map(|candidate| candidate.score.get())
            .collect();
        let probabilities = calibrated_probabilities(&logits, self.temperature)?;
        let mut ranking = raw
            .ranking
            .iter()
            .zip(probabilities.iter())
            .map(|(candidate, &probability)| {
                Ok(RankedCandidate {
                    candidate_id: candidate.candidate_id.clone(),
                    score: FiniteScore::new(probability)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        rank_canonically(&mut ranking);
        Ok(SlmResult {
            ranking,
            retrieved_subset_hash: raw.retrieved_subset_hash,
            board_hash: board.board_hash.as_str().to_string(),
            model_bundle_hash: self.model_bundle_hash.clone(),
            evidence_trace: None,
            inference_evidence: None,
            move_evidence: Vec::new(),
        })
    }
}

fn admit_semantic_v3_record(record: &Example) -> Result<&SemanticCorpusClosure> {
    let closure = record.semantic_v3.as_ref().ok_or_else(|| {
        anyhow!(
            "semantic-v3 bundle refused record '{}' without semantic_v3 candidate-pair closure",
            record.example_id
        )
    })?;
    let board = record.board.semantic_board.as_ref().ok_or_else(|| {
        anyhow!(
            "semantic-v3 bundle refused record '{}' without its semantic decision board",
            record.example_id
        )
    })?;
    if board.board_hash.as_str() != record.board_hash {
        anyhow::bail!("semantic-v3 record board hash does not match its embedded board");
    }
    if record.context_projection_hash
        != blake3::hash(record.context_projection.as_bytes())
            .to_hex()
            .to_string()
    {
        anyhow::bail!("semantic-v3 record context projection hash does not match its bytes");
    }
    if closure.corpus_schema_id != crate::corpus_schema::CORPUS_SCHEMA_ID
        || closure.corpus_schema_hash != crate::corpus_schema::corpus_schema_hash()
        || closure.semantic_board_schema_version != board.schema_version
        || closure.semantic_pack_identity != board.semantic_snapshot.as_str()
        || closure.turn_serializer_id != crate::pair::TURN_SERIALIZER_ID
        || closure.turn_serializer_hash != crate::pair::turn_serializer_hash()
        || closure.candidate_serializer_id != crate::exact::CANDIDATE_SERIALIZER_ID
        || closure.candidate_serializer_hash != crate::exact::candidate_serializer_hash()
        || closure.pair_serializer_id != crate::pair::PAIR_SERIALIZER_ID
        || closure.pair_serializer_hash != crate::pair::pair_serializer_hash()
    {
        anyhow::bail!("semantic-v3 record serializer or admitted-pack closure mismatch");
    }

    let board_ids = board
        .candidates
        .iter()
        .map(|candidate| candidate.canonical_id.as_str().to_string())
        .collect::<Vec<_>>();
    let pair_ids = closure
        .candidate_pairs
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect::<Vec<_>>();
    if closure.full_served_list != board_ids
        || pair_ids != board_ids
        || record.tier1_list != board_ids
    {
        anyhow::bail!(
            "semantic-v3 record must contain every current-board candidate exactly once in canonical order"
        );
    }

    for (stored, candidate) in closure.candidate_pairs.iter().zip(&board.candidates) {
        crate::pair::validate_serialized_candidate_pair(&stored.pair)
            .map_err(|error| anyhow!("candidate '{}': {error}", stored.candidate_id))?;
        if stored.pair.budget != crate::pair::PairTokenBudget::default() {
            anyhow::bail!(
                "candidate '{}' uses a non-serving pair budget",
                stored.candidate_id
            );
        }
        let semantic_text = crate::exact::serialize_candidate(candidate);
        if stored.candidate_semantic_text != semantic_text
            || stored.candidate_semantic_hash
                != blake3::hash(semantic_text.as_bytes()).to_hex().to_string()
        {
            anyhow::bail!(
                "candidate '{}' semantic text does not match the embedded board",
                stored.candidate_id
            );
        }
    }

    if closure.board_inclusion_truth != board_ids.contains(&record.label)
        || closure.exact_match != crate::exact::governed_exact(board, &record.utterance)
    {
        anyhow::bail!("semantic-v3 record label or exact-evidence closure mismatch");
    }
    let expected_bindings = board
        .candidates
        .iter()
        .find(|candidate| candidate.canonical_id.as_str() == record.label)
        .map(|candidate| candidate.arguments.as_slice())
        .unwrap_or_default();
    if closure.binding_requirements.as_slice() != expected_bindings {
        anyhow::bail!("semantic-v3 record binding requirements do not match its label");
    }
    Ok(closure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct ParityPacket {
        schema: String,
        bundle_base: String,
        pair_serializer_id: String,
        pair_serializer_hash: String,
        max_length: usize,
        absolute_tolerance: f64,
        pairs: Vec<ParityPair>,
        expected_python_logits: Vec<f64>,
        expected_ranking: Vec<String>,
    }

    #[derive(serde::Deserialize)]
    struct ParityPair {
        candidate_id: String,
        side_a: String,
        side_b: String,
    }

    fn semantic_record() -> Example {
        let dag = designer_graph::schema::DesignerDag::new("route-admission-test");
        let graph_identity = "route-admission-revision";
        let board = crate::bpmn_board::build_bpmn_semantic_board(
            &dag,
            None,
            graph_identity,
            &crate::board::PolicyFilter::default(),
        )
        .unwrap();
        let context = crate::context::project_ir(
            &dag.to_ir().unwrap(),
            None,
            board.semantic_snapshot.as_str(),
            graph_identity,
        )
        .unwrap();
        let label = board.candidates[0].canonical_id.as_str().to_string();
        let list = board
            .candidates
            .iter()
            .map(|candidate| candidate.canonical_id.as_str().to_string())
            .collect::<Vec<_>>();
        let closure = SemanticCorpusClosure::new(
            &board,
            "do something unknown",
            &context,
            &label,
            "route-family".to_string(),
            "route-test".to_string(),
            "route-split".to_string(),
        )
        .unwrap();
        Example {
            example_id: "route-record".to_string(),
            provenance: "route-test".to_string(),
            board_hash: board.board_hash.as_str().to_string(),
            context_projection: context.serialize_canonical(),
            context_projection_hash: context.hash(),
            board: crate::corpus_schema::BoardDump::from_inference_board(&board),
            tier1_list: list,
            retrieved_subset_hash: "full-board".to_string(),
            label,
            family_id: "route-family".to_string(),
            pair_group_id: None,
            style_regime: "route-test".to_string(),
            utterance: "do something unknown".to_string(),
            gold_in_tier1: true,
            semantic_v3: Some(closure),
        }
    }

    #[test]
    fn semantic_v3_bundle_refuses_every_legacy_textualisation_route() {
        assert!(admit_scoring_route(
            BundleInputMode::SemanticCandidatePairsV3,
            ScoringRoute::SemanticCandidatePairsV3,
        )
        .is_ok());
        let error = admit_scoring_route(
            BundleInputMode::SemanticCandidatePairsV3,
            ScoringRoute::LegacyDescription,
        )
        .unwrap_err();
        assert_eq!(
            error,
            ScoringRouteAdmissionError::V3LegacyDescriptionRefused
        );
    }

    #[test]
    fn semantic_v3_record_admission_checks_complete_pair_closure() {
        let record = semantic_record();
        admit_semantic_v3_record(&record).unwrap();

        let mut missing = semantic_record();
        missing.semantic_v3 = None;
        assert!(admit_semantic_v3_record(&missing)
            .unwrap_err()
            .to_string()
            .contains("without semantic_v3"));

        let mut tampered = semantic_record();
        tampered.semantic_v3.as_mut().unwrap().candidate_pairs[0]
            .pair
            .side_a
            .push_str(" tampered");
        assert!(admit_semantic_v3_record(&tampered)
            .unwrap_err()
            .to_string()
            .contains("content identities"));
    }

    #[test]
    #[ignore = "loads the frozen 568 MiB bundle; run explicitly for the Phase 0 parity receipt"]
    fn python_candle_v3_parity_packet() {
        let packet_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/v3_python_candle_parity.json");
        let packet: ParityPacket =
            serde_json::from_str(&std::fs::read_to_string(packet_path).unwrap()).unwrap();
        assert_eq!(packet.schema, "bpmn.python-candle-parity.v1");
        assert_eq!(packet.bundle_base, Base::ModernbertBase.key());
        assert_eq!(packet.pair_serializer_id, crate::pair::PAIR_SERIALIZER_ID);
        assert_eq!(
            packet.pair_serializer_hash,
            crate::pair::pair_serializer_hash()
        );
        assert_eq!(packet.max_length, MAX_LENGTH);
        assert_eq!(packet.pairs.len(), packet.expected_python_logits.len());

        let bundle_dir = std::env::var_os("SLM_BUNDLE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("train_py/bundles/modernbert-base")
            });
        let device = Device::Cpu;
        let ranker = TrainedRanker::load(Base::ModernbertBase, &bundle_dir, &device).unwrap();
        let candidate_ids = packet
            .pairs
            .iter()
            .map(|pair| pair.candidate_id.clone())
            .collect::<Vec<_>>();
        let pairs = packet
            .pairs
            .iter()
            .map(|pair| (pair.side_a.clone(), pair.side_b.clone()))
            .collect::<Vec<_>>();
        let candle = ranker
            .score_pairs(&pairs, &candidate_ids, "parity-board", &device)
            .unwrap();
        let candle_by_id = candle
            .ranking
            .iter()
            .map(|candidate| (candidate.candidate_id.as_str(), candidate.score.get()))
            .collect::<std::collections::BTreeMap<_, _>>();
        for (candidate_id, python) in candidate_ids.iter().zip(&packet.expected_python_logits) {
            let candle = candle_by_id[candidate_id.as_str()];
            assert!(
                (candle - python).abs() <= packet.absolute_tolerance,
                "{candidate_id}: Python={python}, Candle={candle}, tolerance={}",
                packet.absolute_tolerance
            );
        }
        assert_eq!(
            candle
                .ranking
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect::<Vec<_>>(),
            packet.expected_ranking
        );
    }

    #[test]
    fn bundle_with_old_candidate_serializer_is_refused_before_model_load() {
        let dir = std::env::temp_dir().join(format!("bpmn-bundle-card-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tokenizer.json"), b"tokenizer").unwrap();
        std::fs::write(dir.join("model.safetensors"), b"weights").unwrap();
        let budget = crate::pair::PairTokenBudget::default();
        let card = serde_json::json!({
            "corpus_schema_id": crate::corpus_schema::CORPUS_SCHEMA_ID,
            "corpus_schema_hash": crate::corpus_schema::corpus_schema_hash(),
            "semantic_board_schema_version": 1,
            "semantic_pack_identity": crate::bpmn_board::bpmn_semantic_snapshot_identity(),
            "turn_serializer_id": crate::pair::TURN_SERIALIZER_ID,
            "turn_serializer_hash": crate::pair::turn_serializer_hash(),
            "candidate_serializer_id": "legacy.description.v2",
            "candidate_serializer_hash": crate::exact::candidate_serializer_hash(),
            "pair_serializer_id": crate::pair::PAIR_SERIALIZER_ID,
            "pair_serializer_hash": crate::pair::pair_serializer_hash(),
            "tokenizer_hash": file_hash(&dir.join("tokenizer.json")).unwrap(),
            "max_length": MAX_LENGTH,
            "side_a_tokens": budget.side_a_tokens,
            "side_b_tokens": budget.side_b_tokens,
            "model_weights_hash": file_hash(&dir.join("model.safetensors")).unwrap(),
            "temperature": 1.0,
            "calibration_set_identity": "calibration-test",
            "training_split_manifest_hash": "split-test",
        });
        let error = validate_bundle_card(&card, &dir).unwrap_err();
        assert!(error
            .to_string()
            .contains("candidate_serializer_id mismatch"));
        std::fs::remove_dir_all(dir).unwrap();
    }

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
        assert!(
            p_t[0] < p1[0],
            "T>1 must FLATTEN the top probability: {} vs {}",
            p_t[0],
            p1[0]
        );
        assert!(p_t[2] > p1[2], "T>1 must lift the tail");
        assert!(
            p_t[0] > p_t[1] && p_t[1] > p_t[2],
            "monotone: order preserved"
        );
        assert!(
            calibrated_probabilities(&logits, 0.0).is_err(),
            "T=0 refused"
        );
        assert!(
            calibrated_probabilities(&logits, f64::NAN).is_err(),
            "NaN T refused"
        );
        assert!(
            calibrated_probabilities(&logits, -1.0).is_err(),
            "negative T refused"
        );
    }

    /// Real-bundle compatibility receipt: a semantic-v3 bundle refuses the
    /// legacy K-subset/description route. Requires the trained bundle on disk +
    /// hf-hub cache/network for the base's config.json, so it is #[ignore]d:
    /// run with
    ///   SLM_BUNDLE_DIR=utterance-engine/train_py/bundles/modernbert-base \
    ///   cargo test -p utterance-engine --features candle-probe --release -- --ignored tier1_serving
    #[test]
    #[ignore = "needs SLM_BUNDLE_DIR trained bundle + hf-hub config.json cache"]
    fn tier1_v3_bundle_refuses_legacy_k12_route() {
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
            t1.model_bundle_hash()
                .starts_with("slm.trained.modernbert-base@")
                && t1.model_bundle_hash().contains('#'),
            "bundle hash must carry identity#content-hash: {}",
            t1.model_bundle_hash()
        );
        assert!(t1.temperature() > 0.0);

        let board = build_board(
            &AllLegal,
            None,
            Some("rev0"),
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .unwrap();
        let ctx = crate::context::minimal("pack.none", "g-test");
        let error = t1
            .rank(
                &crate::retrieval::LexicalTier0,
                "chase them again",
                &ctx.serialize_canonical(),
                &board,
            )
            .unwrap_err();
        assert!(error.to_string().contains("refused legacy"));
    }
}
