//! DIR-003 Phase 3 — the `starter-seed-v1` permanent named suite.
//!
//! `seed/banks/starter_seed_v1.json` is a 34-utterance slice authored by
//! Adam OUTSIDE the generation pipeline (EOP-DIR-BPMN-DESIGN-003-003
//! Phase 3) -- the differently-generated slice per A3.3(i). It is NOT the
//! human-usage slice the spec calls for; its provenance is labelled
//! `starter-seed-v1` explicitly, it is kept out of training entirely, and
//! it is marked for supersession the moment real developer-session data
//! exists (see the plan's Phase 4 AFTER-items).
//!
//! Every label in the bank is a PROVISIONAL HYPOTHESIS, not a
//! label-by-construction gold value (these are free utterances, unlike
//! the synthetic corpus) -- pending Adam's adjudication at first live
//! testing, especially every entry the bank itself flags `disputed`.
//!
//! Reuses the same board fixtures / record schema / tier-0 retriever /
//! trained-bundle scorer as the rest of the pipeline (one fixture set,
//! one record shape, one scoring path -- never a second, drifting copy).
//!
//! Reports per-category evidence, NOT pass/fail (directive 3.1): this
//! slice exists to show where synthetic training meets unseen phrasing,
//! and surprises here are the product, not a red/green gate.
//!
//! Run: `cargo run -p utterance-engine --example starter_seed_eval
//! --features embed,candle-probe --release`
//! Emits: seed/corpus_v2/starter-seed-v1.enriched.jsonl +
//! seed/corpus_v2/starter-seed-v1.report.json

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context, Result};
use candle_core::Device;
use designer_graph::positional::PositionalLegality;
use utterance_engine::board::{build_board, Board, EmptyUniverse, PolicyFilter};
use utterance_engine::context::{project_ir, ContextProjection};
use utterance_engine::contract::NONE_OF_THE_ABOVE;
use utterance_engine::corpus_schema::{BoardDump, Example};
use utterance_engine::fixtures::{enumeration_classes, ClassState};
#[cfg(not(feature = "embed"))]
use utterance_engine::retrieval::LexicalTier0;
use utterance_engine::retrieval::{tier1_list, Tier0Retriever};
use utterance_engine::trained_ranker::{Base, TrainedRanker};

const K: usize = utterance_engine::retrieval::TIER1_K; // the ONE standing K (12; ratified 2026-08-01)
const SUITE: &str = "starter-seed-v1";
const CANONICAL_BASE: Base = Base::ModernbertBase; // Adam's ratification, EOP-DIR-BPMN-DESIGN-003-003

#[derive(serde::Deserialize)]
struct StarterItem {
    seq: u32,
    category: String,
    class_id: String,
    label: String,
    text: String,
    disputed: bool,
    dispute_note: String,
}

/// DIR-004 Phase 1.4: written only by `examples/adjudicate_starter_seed.rs`
/// (a real human verdict, never fabricated). Merged in here at run time so
/// the original authored bank stays untouched and this suite automatically
/// reflects adjudication as it accrues.
#[derive(serde::Deserialize)]
struct Adjudication {
    seq: u32,
    adjudicated_label: String,
}

fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let bank_path = root.join("seed/banks/starter_seed_v1.json");
    let out_dir = root.join("seed/corpus_v2");
    std::fs::create_dir_all(&out_dir)?;

    let items: Vec<StarterItem> = serde_json::from_str(&std::fs::read_to_string(&bank_path)?)
        .with_context(|| format!("{bank_path:?}"))?;

    let adj_path = root.join("seed/banks/starter_seed_v1_adjudications.json");
    let adjudications: BTreeMap<u32, String> = if adj_path.exists() {
        let list: Vec<Adjudication> = serde_json::from_str(&std::fs::read_to_string(&adj_path)?)
            .with_context(|| format!("{adj_path:?}"))?;
        list.into_iter().map(|a| (a.seq, a.adjudicated_label)).collect()
    } else {
        BTreeMap::new()
    };

    let classes = enumeration_classes()?;
    let mut boards: BTreeMap<&str, (Board, ContextProjection, &ClassState)> = BTreeMap::new();
    for c in &classes {
        let ir = c.dag.to_ir().map_err(|e| anyhow!("{}: {e}", c.class_id))?;
        let graph_identity = format!("class:{}", c.class_id);
        let proj = project_ir(&ir, c.anchor_id, "pack.none", &graph_identity)?;
        let oracle = PositionalLegality { dag: &c.dag };
        let anchor_pair = c.anchor_key.as_ref().map(|k| (k, c.anchor_id.unwrap()));
        let board = build_board(
            &oracle,
            anchor_pair,
            Some(&graph_identity),
            &EmptyUniverse,
            &PolicyFilter::default(),
        )?;
        boards.insert(c.class_id, (board, proj, c));
    }

    #[cfg(feature = "embed")]
    let retriever = utterance_engine::retrieval::embed::EmbedTier0::new()?;
    #[cfg(not(feature = "embed"))]
    let retriever = LexicalTier0;

    let device = Device::Cpu;
    let bundle_dir = root.join("train_py/bundles").join(CANONICAL_BASE.key());
    let ranker = TrainedRanker::load(CANONICAL_BASE, &bundle_dir, &device)?;

    let mut bad_labels: Vec<String> = Vec::new();
    let mut examples: Vec<Example> = Vec::new();
    // (category) -> Vec<row>
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut per_category: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new(); // (n, tier0_hits, tier1_hits)
    let mut disputed_rows: Vec<serde_json::Value> = Vec::new();

    for item in &items {
        let (board, proj, _c) = boards
            .get(item.class_id.as_str())
            .ok_or_else(|| anyhow!("starter item names unknown class '{}'", item.class_id))?;

        let adjudicated_label = adjudications.get(&item.seq);
        let label: &str = adjudicated_label.map(String::as_str).unwrap_or(item.label.as_str());
        let disputed = item.disputed && adjudicated_label.is_none();
        let provenance = if adjudicated_label.is_some() {
            "adam-adjudicated".to_string()
        } else {
            format!("{SUITE}.enriched")
        };

        if label != NONE_OF_THE_ABOVE && !board.contains(label) {
            let legal: Vec<&str> = board.candidates.iter().map(|c| c.canonical_id.as_str()).collect();
            bad_labels.push(format!(
                "seq {} '{}': label '{}' not proposed by board '{}' -- legal candidates: {:?}",
                item.seq, item.text, label, item.class_id, legal
            ));
            continue;
        }

        let result = retriever.retrieve(&item.text, board)?;
        let list = tier1_list(&result, K);
        let tier0_top1_hit = list.first().map(|s| s.as_str()) == Some(label);

        let family_id = format!("{}::{}", item.class_id, label);
        let mut pre = Vec::new();
        pre.extend_from_slice(board.board_hash.as_bytes());
        pre.extend_from_slice(label.as_bytes());
        pre.extend_from_slice(blake3::hash(item.text.as_bytes()).to_hex().as_bytes());
        pre.extend_from_slice(&item.seq.to_le_bytes());

        let example = Example {
            example_id: blake3::hash(&pre).to_hex().to_string(),
            provenance,
            board_hash: board.board_hash.clone(),
            context_projection: proj.serialize_canonical(),
            context_projection_hash: proj.hash(),
            board: BoardDump::from_board(board),
            tier1_list: list,
            retrieved_subset_hash: result.retrieved_subset_hash.clone(),
            label: label.to_string(),
            family_id,
            pair_group_id: None,
            style_regime: SUITE.to_string(),
            utterance: item.text.clone(),
            gold_in_tier1: true, // NOTA always served; non-NOTA labels validated above
            semantic_v3: None,
        };

        let tier1_result = ranker.score(&example, &device)?;
        let tier1_top1_hit = tier1_result
            .ranking
            .first()
            .map(|rc| rc.candidate_id.as_str())
            == Some(label);

        let slot = per_category.entry(item.category.clone()).or_insert((0, 0, 0));
        slot.0 += 1;
        if tier0_top1_hit {
            slot.1 += 1;
        }
        if tier1_top1_hit {
            slot.2 += 1;
        }

        let row = serde_json::json!({
            "seq": item.seq,
            "category": item.category,
            "class_id": item.class_id,
            "text": item.text,
            "hypothesis_label": label,
            "original_hypothesis": item.label,
            "adjudicated": adjudicated_label.is_some(),
            "disputed": disputed,
            "dispute_note": item.dispute_note,
            "tier0_top1": example.tier1_list.first().cloned().unwrap_or_default(),
            "tier0_top1_matches_hypothesis": tier0_top1_hit,
            "tier1_top1": tier1_result.ranking.first().map(|rc| rc.candidate_id.clone()).unwrap_or_default(),
            "tier1_top1_matches_hypothesis": tier1_top1_hit,
        });
        if disputed {
            disputed_rows.push(row.clone());
        }
        rows.push(row);
        examples.push(example);
    }

    if !bad_labels.is_empty() {
        bail!(
            "HALT: {} starter-seed-v1 entries label candidates their boards do not propose:\n{}",
            bad_labels.len(),
            bad_labels.join("\n")
        );
    }

    let per_category_json: BTreeMap<String, serde_json::Value> = per_category
        .iter()
        .map(|(k, (n, t0, t1))| {
            (
                k.clone(),
                serde_json::json!({
                    "n": n,
                    "tier0_top1_matches_hypothesis": t0,
                    "tier1_top1_matches_hypothesis": t1,
                }),
            )
        })
        .collect();

    let report = serde_json::json!({
        "suite": SUITE,
        "provenance": format!("{SUITE}.enriched"),
        "note": "Evidence, not pass/fail (directive 3.1): every label here is a PROVISIONAL HYPOTHESIS authored outside the generation pipeline, not gold-by-construction. Disputed entries are pending Adam's adjudication at first live testing -- a 'miss' against a disputed hypothesis is not necessarily a model error. Adjudicated items (via examples/adjudicate_starter_seed.rs, DIR-004 Phase 1.4) are merged in from seed/banks/starter_seed_v1_adjudications.json automatically -- see each row's 'adjudicated'/'original_hypothesis' fields. This slice is kept out of training entirely and is marked for supersession by real developer-session data.",
        "adjudications_applied": adjudications.len(),
        "canonical_base_scored": CANONICAL_BASE.key(),
        "k": K,
        "totals": {
            "n": items.len(),
        },
        "per_category": per_category_json,
        "disputed_items": disputed_rows,
        "rows": rows,
    });

    let jsonl: String = examples
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(out_dir.join(format!("{SUITE}.enriched.jsonl")), jsonl + "\n")?;
    std::fs::write(
        out_dir.join(format!("{SUITE}.report.json")),
        serde_json::to_string_pretty(&report)? + "\n",
    )?;

    println!("STARTER-SEED-EVAL ({SUITE}): {} items, canonical base = {}", items.len(), CANONICAL_BASE.key());
    for (cat, (n, t0, t1)) in &per_category {
        println!("  {cat:24} n={n:2}  tier0_top1_hits={t0}  tier1_top1_hits={t1}");
    }
    println!("  disputed items: {}", disputed_rows.len());
    Ok(())
}
