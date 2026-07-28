//! DIR-002 Phase C's remaining step ("verify each [bundle] loads and
//! scores in Candle behind the SlmResult contract") + Phase D's real
//! uplift number: loads a fine-tuned bundle from `train_py/bundles/<key>/`
//! (produced by `train_py/train_slm.py`) into candle-transformers, and
//! scores it against the enriched eval set (`eval_enrich.rs`) so its
//! top-1 accuracy can sit next to the C5 baseline (tier-0 alone,
//! recall@8 = 0.9592 on this same eval slice — see
//! synthetic-v2-beta.eval_enriched.card.json).
//!
//! Key-layout note (why this ISN'T a copy of candle_loadability_probe.rs):
//! that probe loads a HF checkpoint's OWN weights via `ModernBert::load`/
//! `BertModel::load`/`XLMRobertaModel::new`, which expect each
//! architecture's real published key layout. `train_slm.py` exports a
//! DIFFERENT, uniform layout instead (`encoder.*` + `head.weight`/
//! `head.bias`, flat, chosen so one Python script serves four
//! architectures identically) — plain AutoModel-loaded encoders have no
//! "model."/"bert."/"roberta." prefix of their own, so:
//!   - ModernBert::load hardcodes an internal "model." segment regardless
//!     of the vb it's given -> strip "encoder." then ADD "model." back.
//!   - BertModel::load / XLMRobertaModel::new use the given vb's root
//!     directly (no hardcoded segment) -> strip "encoder.", add nothing.
//! Verified against the actual saved tensor names in a completed bundle
//! before writing this, not assumed.
//!
//! Run: `cargo run -p utterance-engine --example score_trained_bundle
//! --features candle-probe --release -- <base-key|all>`

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::{Linear, Module, VarBuilder};
use candle_transformers::models::{bert, modernbert, xlm_roberta};
use hf_hub::{api::sync::Api, Repo, RepoType};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer};
use utterance_engine::contract::{rank_canonically, FiniteScore, RankedCandidate};
use utterance_engine::corpus_schema::Example;

const MAX_LENGTH: usize = 256; // must match train_slm.py's MAX_LENGTH

#[derive(Clone, Copy, PartialEq, Eq)]
enum Base {
    GteModernbert,
    MsMarco,
    ModernbertBase,
    BgeReranker,
}

impl Base {
    const ALL: [Base; 4] = [Base::GteModernbert, Base::MsMarco, Base::ModernbertBase, Base::BgeReranker];

    fn key(self) -> &'static str {
        match self {
            Base::GteModernbert => "gte-modernbert",
            Base::MsMarco => "ms-marco",
            Base::ModernbertBase => "modernbert-base",
            Base::BgeReranker => "bge-reranker",
        }
    }

    fn from_key(k: &str) -> Option<Base> {
        Base::ALL.into_iter().find(|b| b.key() == k)
    }

    /// Same HF pins as candle_loadability_probe.rs -- architecture
    /// hyperparameters only (config.json). Weights come from the local
    /// trained bundle, never from this download.
    fn repo_and_revision(self) -> (&'static str, &'static str) {
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

struct TrainedRanker {
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
    fn load(base: Base, bundle_dir: &std::path::Path, device: &Device) -> Result<Self> {
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

    /// Scores every candidate in `record.tier1_list` -- the SAME list
    /// shape training/serving share (Adam's finding-5 ruling) -- never
    /// the full board.
    fn score(&self, record: &Example, device: &Device) -> Result<utterance_engine::contract::SlmResult> {
        self.score_list(record, &record.tier1_list, device)
    }

    /// Explicit-list variant: the accuracy path above always serves
    /// tier1_list; this exists for the latency-vs-K probe (timing the
    /// same forward pass at widened list sizes, e.g. the full board) and
    /// for scoring constructed candidate pairs (ambiguity-set suite).
    fn score_list(
        &self,
        record: &Example,
        cand_ids: &[String],
        device: &Device,
    ) -> Result<utterance_engine::contract::SlmResult> {
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
        Ok(utterance_engine::contract::SlmResult {
            ranking,
            retrieved_subset_hash: blake3::hash(&pre).to_hex().to_string(),
            board_hash: record.board_hash.clone(),
            model_bundle_hash: self.bundle_identity.clone(),
        })
    }
}

fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let arg = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());
    let bases: Vec<Base> = if arg == "all" {
        Base::ALL.to_vec()
    } else {
        vec![Base::from_key(&arg).ok_or_else(|| anyhow!("unknown base key '{arg}'"))?]
    };

    let eval_path = root.join("seed/corpus_v2/synthetic-v2-beta.eval_enriched.jsonl");
    let eval_records: Vec<Example> = std::fs::read_to_string(&eval_path)
        .with_context(|| format!("{eval_path:?} -- run eval_enrich first"))?
        .lines()
        .map(|l| serde_json::from_str(l).map_err(Into::into))
        .collect::<Result<_>>()?;

    // The apples-to-apples baseline (eval_enrich.rs's own receipt):
    // tier1_list[0] is the tier-0 retriever's OWN rank-ordered #1 pick.
    // recall@K is a much easier bar and is NEVER the uplift comparison.
    let card_path = root.join("seed/corpus_v2/synthetic-v2-beta.eval_enriched.card.json");
    let card: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&card_path)?)
        .with_context(|| format!("{card_path:?} -- run eval_enrich first"))?;
    let tier0_top1_accuracy = card["totals"]["tier0_top1_accuracy"]
        .as_f64()
        .context("card missing totals.tier0_top1_accuracy -- regenerate with the current eval_enrich.rs")?;
    println!("baseline: tier0_top1_accuracy (C5, tier-0 alone) = {tier0_top1_accuracy:.4}\n");

    let device = Device::Cpu;

    // Latency-vs-K probe (close-out suite): `<base-key> latency` times
    // the identical scoring path at three served-list sizes — the ruled
    // K=8+NOTA (9), the proposed K=12+NOTA (13), and the full board
    // (upper bound) — over the whole eval set. Widened lists take the
    // tier-0 ranking's next candidates in rank order (exactly what a
    // widened tier1_list would contain); the full-board list is the
    // board in ranking order. CPU device deliberately: serving latency
    // is the CPU story, per the T3.4a criteria.
    if std::env::args().nth(2).as_deref() == Some("latency") {
        let base = bases[0];
        let bundle_dir = root.join("train_py/bundles").join(base.key());
        let ranker = TrainedRanker::load(base, &bundle_dir, &device)?;
        for &(label, k) in &[("K=8+NOTA (ruled)", 9usize), ("K=12+NOTA (proposed)", 13), ("full board", usize::MAX)] {
            let mut times_ms: Vec<f64> = Vec::with_capacity(eval_records.len());
            for record in &eval_records {
                // Widen from the board's candidate list in board order —
                // the enriched record stores candidates in the retriever's
                // canonical order via tier1_list for the first 9; beyond
                // that, take remaining board candidates not already listed.
                let mut ids: Vec<String> = record.tier1_list.clone();
                for c in &record.board.candidates {
                    if ids.len() >= k {
                        break;
                    }
                    if !ids.contains(&c.canonical_id) {
                        ids.push(c.canonical_id.clone());
                    }
                }
                let t = std::time::Instant::now();
                let _ = ranker.score_list(record, &ids, &device)?;
                times_ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mean = times_ms.iter().sum::<f64>() / times_ms.len() as f64;
            let p95 = times_ms[(times_ms.len() as f64 * 0.95) as usize - 1];
            let actual_k = eval_records
                .first()
                .map(|r| {
                    let mut ids = r.tier1_list.len();
                    ids += r.board.candidates.iter().filter(|c| !r.tier1_list.contains(&c.canonical_id)).count();
                    ids.min(k)
                })
                .unwrap_or(0);
            println!(
                "  {} ({}): list_len={} mean={mean:.1}ms p95={p95:.1}ms over {} utterances",
                base.key(),
                label,
                actual_k,
                times_ms.len()
            );
        }
        return Ok(());
    }

    // Ambiguity-set score-separation suite (A2.5): `<base-key> ambiguity`.
    // For each constructed-ambiguous pair, softmax the two logits and
    // measure the winner-margin |p_a - p_b|; for the regular eval set,
    // softmax the full served list and measure top1-top2 margin. A model
    // that has learned real context-conditioning should show MARKEDLY
    // smaller margins on the constructed-ambiguous pairs (feeding the
    // disposition policy's clarification path) than on ordinary
    // examples — confidently splitting an unlabelable pair is the
    // failure mode this suite exists to catch. Raw logits, no
    // calibration: margins are compared within one model only.
    if std::env::args().nth(2).as_deref() == Some("ambiguity") {
        let base = bases[0];
        let bundle_dir = root.join("train_py/bundles").join(base.key());
        let ranker = TrainedRanker::load(base, &bundle_dir, &device)?;

        let amb_path = root.join("seed/corpus_v2/synthetic-v2-beta.ambiguity_enriched.jsonl");
        let amb_records: Vec<Example> = std::fs::read_to_string(&amb_path)
            .with_context(|| format!("{amb_path:?} -- run eval_enrich first"))?
            .lines()
            .map(|l| serde_json::from_str(l).map_err(Into::into))
            .collect::<Result<_>>()?;

        let softmax_margin = |logits: &[f64]| -> f64 {
            let max = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let exps: Vec<f64> = logits.iter().map(|l| (l - max).exp()).collect();
            let sum: f64 = exps.iter().sum();
            let mut probs: Vec<f64> = exps.iter().map(|e| e / sum).collect();
            probs.sort_by(|a, b| b.partial_cmp(a).unwrap());
            probs[0] - probs.get(1).copied().unwrap_or(0.0)
        };

        let mut amb_margins: Vec<f64> = Vec::new();
        for record in &amb_records {
            let result = ranker.score_list(record, &record.tier1_list, &device)?;
            let logits: Vec<f64> = result.ranking.iter().map(|rc| rc.score.get()).collect();
            amb_margins.push(softmax_margin(&logits));
        }
        let mut eval_margins: Vec<f64> = Vec::new();
        for record in &eval_records {
            let result = ranker.score(record, &device)?;
            let logits: Vec<f64> = result.ranking.iter().map(|rc| rc.score.get()).collect();
            eval_margins.push(softmax_margin(&logits));
        }
        let stats = |v: &mut Vec<f64>| -> (f64, f64) {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            (mean, v[v.len() / 2])
        };
        let (amb_mean, amb_median) = stats(&mut amb_margins);
        let (eval_mean, eval_median) = stats(&mut eval_margins);
        // Overlap check: what fraction of ambiguity pairs are MORE
        // confidently split than the median ordinary example?
        let confidently_split = amb_margins.iter().filter(|&&m| m > eval_median).count();
        println!("=== ambiguity-set score separation: {} ===", base.key());
        println!(
            "  ambiguity pairs (n={}): margin mean={amb_mean:.4} median={amb_median:.4}",
            amb_margins.len()
        );
        println!(
            "  ordinary eval  (n={}): margin mean={eval_mean:.4} median={eval_median:.4}",
            eval_margins.len()
        );
        println!(
            "  ambiguity pairs split more confidently than the ordinary-eval median: {}/{}",
            confidently_split,
            amb_margins.len()
        );
        let out = serde_json::json!({
            "base": base.key(),
            "bundle_identity": ranker.bundle_identity,
            "note": "margins are softmax(top1)-softmax(top2) within one model; ambiguity pairs use the 2-candidate constructed pair, ordinary eval uses the full served list. No calibration applied (A4 calibration is a separate close-out item); cross-model margin comparison is NOT valid from this file.",
            "ambiguity": {"n": amb_margins.len(), "margin_mean": amb_mean, "margin_median": amb_median},
            "ordinary_eval": {"n": eval_margins.len(), "margin_mean": eval_mean, "margin_median": eval_median},
            "ambiguity_pairs_split_more_confidently_than_eval_median": confidently_split,
        });
        let out_path = root.join(format!("train_py/bundles/{}/ambiguity_scores.json", base.key()));
        std::fs::write(&out_path, serde_json::to_string_pretty(&out)? + "\n")?;
        println!("written: {out_path:?}");
        return Ok(());
    }

    let mut summary = Vec::new();
    for base in bases {
        let bundle_dir = root.join("train_py/bundles").join(base.key());
        if !bundle_dir.join("model.safetensors").exists() {
            println!("SKIP {} -- no bundle yet (train_slm.py hasn't finished this base)", base.key());
            continue;
        }
        println!("=== scoring {} ===", base.key());
        let ranker = TrainedRanker::load(base, &bundle_dir, &device)?;

        let mut end_to_end_correct = 0u32;
        let mut given_inclusion_correct = 0u32;
        let mut given_inclusion_total = 0u32;
        // Per-class accuracy: the position-invariance proxy (§10.7).
        // family_id is "class::label"; the class IS the graph position,
        // so a label that scores well at one anchor kind and collapses
        // at another shows up here as class-level variance.
        let mut per_class: std::collections::BTreeMap<String, (u32, u32)> =
            std::collections::BTreeMap::new();
        for record in &eval_records {
            let result = ranker.score(record, &device)?;
            let top1 = &result.ranking[0].candidate_id;
            let class = record
                .family_id
                .split("::")
                .next()
                .unwrap_or("unknown")
                .to_string();
            let slot = per_class.entry(class).or_insert((0, 0));
            slot.1 += 1;
            if *top1 == record.label {
                end_to_end_correct += 1;
                slot.0 += 1;
            }
            if record.gold_in_tier1 {
                given_inclusion_total += 1;
                if *top1 == record.label {
                    given_inclusion_correct += 1;
                }
            }
        }
        let per_class_json: std::collections::BTreeMap<String, serde_json::Value> = per_class
            .iter()
            .map(|(k, (hits, total))| {
                (
                    k.clone(),
                    serde_json::json!({"correct": hits, "total": total, "accuracy": *hits as f64 / (*total).max(1) as f64}),
                )
            })
            .collect();
        let n = eval_records.len().max(1) as f64;
        let end_to_end = end_to_end_correct as f64 / n;
        let given_inclusion = given_inclusion_correct as f64 / given_inclusion_total.max(1) as f64;
        let uplift_pp = (end_to_end - tier0_top1_accuracy) * 100.0;
        println!(
            "  {} : top1_end_to_end={end_to_end:.4} ({end_to_end_correct}/{}) top1_given_inclusion={given_inclusion:.4} ({given_inclusion_correct}/{given_inclusion_total}) uplift_vs_tier0_top1={uplift_pp:+.1}pp",
            base.key(),
            eval_records.len(),
        );
        summary.push(serde_json::json!({
            "base": base.key(),
            "bundle_identity": ranker.bundle_identity,
            "n_eval": eval_records.len(),
            "tier0_top1_accuracy_baseline": tier0_top1_accuracy,
            "top1_end_to_end": end_to_end,
            "top1_end_to_end_correct": end_to_end_correct,
            "uplift_vs_tier0_top1_pp": uplift_pp,
            "top1_given_inclusion": given_inclusion,
            "top1_given_inclusion_correct": given_inclusion_correct,
            "top1_given_inclusion_total": given_inclusion_total,
            "per_class_accuracy": per_class_json,
        }));
    }

    let out_path = root.join("train_py/bundles/eval_scores.json");
    std::fs::write(&out_path, serde_json::to_string_pretty(&summary)? + "\n")?;
    println!("scores written: {out_path:?}");
    Ok(())
}
