//! DIR-002 Phase C, step 0 — T3.4a shortlist Candle loadability receipts.
//!
//! The plan (`EOP-PLAN-BPMN-DESIGN-003.md`) flags the four-base shortlist
//! as a "research receipt, candle support verified from candle-transformers
//! source; loadability receipts still owed at Phase C per 'verified not
//! assumed'". Source-reading confirms the right `candle_transformers::models`
//! module exists; it does NOT confirm a real HF checkpoint's weight-key
//! names line up with that module's `VarBuilder` prefixes, or that its
//! `config.json` deserializes into the module's `Config` struct. Training
//! a base that fails either check is wasted work discovered too late.
//!
//! This binary downloads each shortlisted base's real published weights
//! (pinned to an exact commit SHA — never a floor), loads them into the
//! matching Candle model struct, and runs one real forward pass on a
//! representative (query, candidate-description) pair. PASS means the
//! base is safe to spend Phase C training time on; FAIL means it drops
//! from the shortlist before any training starts, per the same
//! evidence-before-exclusion standard already applied to DeBERTa-v3.
//!
//! Run: `cargo run -p utterance-engine --example candle_loadability_probe
//! --features candle-probe --release -- all`
//! (or a single base key: gte-modernbert | ms-marco | modernbert-base | bge-reranker)

use anyhow::{anyhow, Context, Result};
use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::{linear, Module, VarBuilder};
use candle_transformers::models::{bert, modernbert, xlm_roberta};
use hf_hub::{api::sync::Api, Repo, RepoType};
use tokenizers::Tokenizer;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Base {
    GteRerankerModernBert,
    MsMarcoMiniLm,
    ModernBertBase,
    BgeRerankerBase,
}

impl Base {
    const ALL: [Base; 4] = [
        Base::GteRerankerModernBert,
        Base::MsMarcoMiniLm,
        Base::ModernBertBase,
        Base::BgeRerankerBase,
    ];

    fn key(self) -> &'static str {
        match self {
            Base::GteRerankerModernBert => "gte-modernbert",
            Base::MsMarcoMiniLm => "ms-marco",
            Base::ModernBertBase => "modernbert-base",
            Base::BgeRerankerBase => "bge-reranker",
        }
    }

    fn from_key(k: &str) -> Option<Base> {
        Base::ALL.into_iter().find(|b| b.key() == k)
    }

    /// (HF repo, exact commit SHA — fetched 2026-07-28, `HF api /models/<repo>`).
    fn repo_and_revision(self) -> (&'static str, &'static str) {
        match self {
            Base::GteRerankerModernBert => (
                "Alibaba-NLP/gte-reranker-modernbert-base",
                "f7481e6055501a30fb19d090657df9ec1f79ab2c",
            ),
            Base::MsMarcoMiniLm => (
                "cross-encoder/ms-marco-MiniLM-L6-v2",
                "c5ee24cb16019beea0893ab7796b1df96625c6b8",
            ),
            Base::ModernBertBase => (
                "answerdotai/ModernBERT-base",
                "8949b909ec900327062f0ebf497f51aef5e6f0c8",
            ),
            Base::BgeRerankerBase => (
                "BAAI/bge-reranker-base",
                "2cfc18c9415c912f9d8155881c133215df768a70",
            ),
        }
    }
}

/// Cross-encoder-shaped probe pair: representative of a real (utterance,
/// candidate-description) input, per the actual corpus this session built.
const PROBE_QUERY: &str = "chase them again";
const PROBE_CANDIDATE: &str =
    "Non-interrupting bounded reminder cycle with an escalation continuation";

fn encode_pair(tokenizer: &mut Tokenizer, device: &Device) -> Result<(Tensor, Tensor)> {
    // Some published tokenizer.json files bake in a fixed padding/
    // truncation strategy for the checkpoint's original long-document use
    // case (gte-reranker-modernbert-base pads every input to 8000 tokens
    // regardless of content — confirmed via its tokenizer.json's
    // `padding.strategy.Fixed`). Left in place, encoding this probe's
    // ~20-token pair still runs a full forward pass over 8000 positions —
    // real CPU minutes wasted on padding, not a loadability signal.
    // Disabling both here is representative of what Phase C training must
    // also do (dynamic per-batch padding on short board-candidate text,
    // never a repo-default built for a different workload).
    tokenizer.with_padding(None);
    tokenizer
        .with_truncation(None)
        .map_err(|e| anyhow!("with_truncation: {e}"))?;
    let enc = tokenizer
        .encode((PROBE_QUERY, PROBE_CANDIDATE), true)
        .map_err(|e| anyhow!("tokenizer.encode: {e}"))?;
    let ids: Vec<u32> = enc.get_ids().to_vec();
    let mask: Vec<u32> = enc.get_attention_mask().to_vec();
    let ids = Tensor::new(ids.as_slice(), device)?.unsqueeze(0)?;
    let mask = Tensor::new(mask.as_slice(), device)?.unsqueeze(0)?;
    Ok((ids, mask))
}

fn probe(base: Base) -> Result<String> {
    let (repo_id, revision) = base.repo_and_revision();
    let device = Device::Cpu;

    let api = Api::new().context("hf-hub API client")?;
    let repo = api.repo(Repo::with_revision(
        repo_id.to_string(),
        RepoType::Model,
        revision.to_string(),
    ));
    let config_path = repo.get("config.json").context("download config.json")?;
    let tokenizer_path = repo.get("tokenizer.json").context("download tokenizer.json")?;
    let weights_path = repo.get("model.safetensors").context("download model.safetensors")?;

    let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
        .map_err(|e| anyhow!("Tokenizer::from_file: {e}"))?;
    let vb = unsafe {
        VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)
            .context("VarBuilder::from_mmaped_safetensors")?
    };
    let config_str = std::fs::read_to_string(&config_path).context("read config.json")?;

    let (ids, mask) = encode_pair(&mut tokenizer, &device)?;

    match base {
        Base::GteRerankerModernBert => {
            // NOT ModernBertForSequenceClassification::load: this
            // checkpoint's real config.json has label2id values as
            // integers ({"LABEL_0": 0}), not the strings
            // ClassifierConfig::label2id (HashMap<String,String>)
            // requires — serde(flatten) silently swallows that mismatch
            // into `classifier_config: None` rather than erroring, which
            // then sizes the classifier Linear at 0 outputs and fails to
            // load the real [1,768] classifier.weight. Moot for Phase C
            // regardless: every base's PRETRAINED head gets discarded
            // and replaced with a freshly trained one on our corpus
            // (A4 — "identically trained... same recipe"), so the
            // receipt that actually matters is the base encoder, tested
            // identically to ModernBertBase below.
            let cfg: modernbert::Config =
                serde_json::from_str(&config_str).context("parse modernbert::Config")?;
            let model = modernbert::ModernBert::load(vb, &cfg).context("ModernBert::load")?;
            let hidden = model.forward(&ids, &mask).context("forward")?;
            Ok(format!("encoder hidden shape {:?} (pretrained head skipped — see comment)", hidden.shape()))
        }
        Base::ModernBertBase => {
            let cfg: modernbert::Config =
                serde_json::from_str(&config_str).context("parse modernbert::Config")?;
            let model = modernbert::ModernBert::load(vb, &cfg).context("ModernBert::load")?;
            let hidden = model.forward(&ids, &mask).context("forward")?;
            // Base encoder only (no head, no classifier) — Phase C trains a
            // fresh head on top of this; the receipt here is that the
            // encoder itself loads and produces sane-shaped hidden states.
            Ok(format!("encoder hidden shape {:?}", hidden.shape()))
        }
        Base::MsMarcoMiniLm => {
            let cfg: bert::Config = serde_json::from_str(&config_str).context("parse bert::Config")?;
            // Root VarBuilder: BertModel::load retries "{model_type}.*" on a
            // root-prefix miss, and this checkpoint's real keys are under
            // "bert.*" with config.model_type == "bert" (confirmed against
            // the real config.json before writing this probe).
            let model = bert::BertModel::load(vb.clone(), &cfg).context("BertModel::load")?;
            let token_type_ids = ids.zeros_like()?;
            let hidden = model
                .forward(&ids, &token_type_ids, Some(&mask))
                .context("bert forward")?;
            let cls = hidden.i((.., 0, ..)).context("CLS slice")?;
            // "classifier.weight"/"classifier.bias" sit at checkpoint root,
            // sibling to "bert.*" — a fresh Linear at vb.pp("classifier"),
            // not part of BertModel::load, matching the real key layout.
            let classifier = linear(cfg.hidden_size, 1, vb.pp("classifier"))
                .context("classifier Linear")?;
            let logits = classifier.forward(&cls).context("classifier forward")?;
            let v: Vec<Vec<f32>> = logits.to_vec2()?;
            Ok(format!("logits shape {:?}, value {:?}", logits.shape(), v))
        }
        Base::BgeRerankerBase => {
            let cfg: xlm_roberta::Config =
                serde_json::from_str(&config_str).context("parse xlm_roberta::Config")?;
            let model = xlm_roberta::XLMRobertaForSequenceClassification::new(1, &cfg, vb)
                .context("XLMRobertaForSequenceClassification::new")?;
            let token_type_ids = ids.zeros_like()?;
            let logits = model
                .forward(&ids, &mask, &token_type_ids)
                .context("xlm-roberta forward")?;
            let v: Vec<Vec<f32>> = logits.to_vec2()?;
            Ok(format!("logits shape {:?}, value {:?}", logits.shape(), v))
        }
    }
}

fn main() -> Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());
    let bases: Vec<Base> = if arg == "all" {
        Base::ALL.to_vec()
    } else {
        vec![Base::from_key(&arg)
            .ok_or_else(|| anyhow!("unknown base key '{arg}' (want one of: all, gte-modernbert, ms-marco, modernbert-base, bge-reranker)"))?]
    };

    let mut failures: Vec<(&'static str, String)> = Vec::new();
    for base in bases {
        print!("PROBE {} ({}) ... ", base.key(), base.repo_and_revision().0);
        match probe(base) {
            Ok(detail) => println!("PASS — {detail}"),
            Err(e) => {
                println!("FAIL — {e:#}");
                failures.push((base.key(), format!("{e:#}")));
            }
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "loadability probe FAILED for {} of {}: {:?}",
            failures.len(),
            Base::ALL.len(),
            failures.iter().map(|(k, _)| *k).collect::<Vec<_>>()
        );
    }
    println!("ALL {} bases PASS — safe to proceed to Phase C fine-tuning", Base::ALL.len());
    Ok(())
}
