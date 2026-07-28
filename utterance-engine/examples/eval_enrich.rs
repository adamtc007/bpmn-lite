//! DIR-002 Phase D, step 0 — eval-set board enrichment.
//!
//! `corpus_gen.rs` writes the held-out eval slice (`eval_*.json` banks)
//! straight through as raw labelled utterances — no board, no
//! context_projection, no tier1_list. That's fine for `corpus_gen.rs`'s
//! own job (keep it disjoint from training, per A3.3) but it means the
//! eval slice can't be SCORED by anything: no board to check candidates
//! against, no real tier-0 retrieval run, no way to compute recall@K —
//! the metric that answers "how good is tier-0 (Candle) alone before any
//! SLM re-ranking". This binary closes that substrate gap.
//!
//! Reuses the exact same board fixtures (`utterance_engine::fixtures`)
//! and record schema (`utterance_engine::corpus_schema`) as
//! `corpus_gen.rs` — never a second, drifting copy of either (A1's "one
//! serializer" principle, applied here to "one fixture set" and "one
//! record shape" too).
//!
//! Unlike `corpus_gen.rs`, this tool does NOT drop retrieval-misses,
//! overlap-cap breaches, or duplicates — eval "keeps the ugly cases,
//! that is its job" (corpus_gen.rs's own comment on the eval path).
//! Every eval entry becomes exactly one enriched record; `gold_in_tier1`
//! records whether the real tier-0 retriever's K-prefix actually
//! contained the true label — recall@K is exactly the fraction of
//! records where this is true, computed and printed in the card.
//!
//! Run: `cargo run -p utterance-engine --example eval_enrich --features embed --release`
//! Emits: seed/corpus_v2/<name>.eval_enriched.jsonl + .eval_enriched.card.json

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context, Result};
use designer_graph::positional::PositionalLegality;
use utterance_engine::board::{build_board, Board, EmptyUniverse, PolicyFilter};
use utterance_engine::context::{project_ir, ContextProjection};
use utterance_engine::contract::NONE_OF_THE_ABOVE;
use utterance_engine::corpus_schema::{BankEntry, BoardDump, CandDump, Example};
use utterance_engine::fixtures::{enumeration_classes, ClassState};
#[cfg(not(feature = "embed"))]
use utterance_engine::retrieval::LexicalTier0;
use utterance_engine::retrieval::{tier1_list, Tier0Retriever};

const K: usize = 8; // same K as corpus_gen.rs — the served list shape
const CORPUS_VERSION: &str = "synthetic-v2-beta";

fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let bank_dir = root.join("seed/banks");
    let out_dir = root.join("seed/corpus_v2");
    std::fs::create_dir_all(&out_dir)?;

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

    // Same eval_*.json convention as corpus_gen.rs.
    let mut eval_entries: Vec<BankEntry> = Vec::new();
    for f in std::fs::read_dir(&bank_dir).context("seed/banks missing")? {
        let path = f?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let is_eval = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("eval_"))
            .unwrap_or(false);
        if !is_eval {
            continue;
        }
        let bank: Vec<BankEntry> = serde_json::from_str(&std::fs::read_to_string(&path)?)
            .with_context(|| format!("{path:?}"))?;
        eval_entries.extend(bank);
    }
    if eval_entries.is_empty() {
        bail!("no eval_*.json banks found under seed/banks — nothing to enrich");
    }

    #[cfg(feature = "embed")]
    let retriever = utterance_engine::retrieval::embed::EmbedTier0::new()?;
    #[cfg(not(feature = "embed"))]
    let retriever = LexicalTier0;

    let mut examples: Vec<Example> = Vec::new();
    let mut bad_labels: Vec<String> = Vec::new();
    let mut gold_in_tier1_count = 0u32;
    let mut tier0_top1_count = 0u32;
    let mut per_class_recall: BTreeMap<String, (u32, u32)> = BTreeMap::new(); // (hits, total)

    for e in &eval_entries {
        let (board, proj, _c) = boards
            .get(e.class_id.as_str())
            .ok_or_else(|| anyhow!("eval bank names unknown class '{}'", e.class_id))?;

        // Board-membership legality is still a HALT-worthy defect even in
        // eval: a mislabelled eval entry silently teaches nothing (it's
        // eval, not training) but SCORES nothing meaningfully either —
        // better to catch it here than have Phase D quietly skip it.
        if e.label != NONE_OF_THE_ABOVE && !board.contains(&e.label) {
            bad_labels.push(format!(
                "'{}' not proposed by board '{}' ({:?})",
                e.label, e.class_id, e.text
            ));
            continue;
        }

        let result = retriever.retrieve(&e.text, board)?;
        let list = tier1_list(&result, K);
        let gold_in_tier1 = e.label == NONE_OF_THE_ABOVE || list.contains(&e.label);
        if gold_in_tier1 {
            gold_in_tier1_count += 1;
        }
        // tier1_list()'s first entry is the tier-0 retriever's OWN #1
        // pick (rank-ordered, per rank_canonically inside retrieve()) --
        // this is the real apples-to-apples "tier-0 alone" baseline for
        // any trained SLM's top-1 accuracy on the same eval set. Recall@K
        // (gold_in_tier1, above) is a much easier bar -- "is it in the
        // top 8 anywhere" -- and is NOT what an uplift comparison means.
        if list.first().map(|s| s.as_str()) == Some(e.label.as_str()) {
            tier0_top1_count += 1;
        }
        let slot = per_class_recall.entry(e.class_id.clone()).or_insert((0, 0));
        slot.1 += 1;
        if gold_in_tier1 {
            slot.0 += 1;
        }

        let family_id = format!("{}::{}", e.class_id, e.label);
        let mut pre = Vec::new();
        pre.extend_from_slice(board.board_hash.as_bytes());
        pre.extend_from_slice(e.label.as_bytes());
        pre.extend_from_slice(blake3::hash(e.text.as_bytes()).to_hex().as_bytes());
        pre.extend_from_slice(&e.paraphrase_seq.to_le_bytes());

        examples.push(Example {
            example_id: blake3::hash(&pre).to_hex().to_string(),
            provenance: format!("{CORPUS_VERSION}.eval_enriched"),
            board_hash: board.board_hash.clone(),
            context_projection: proj.serialize_canonical(),
            context_projection_hash: proj.hash(),
            board: BoardDump {
                candidates: board
                    .candidates
                    .iter()
                    .map(|c| CandDump {
                        canonical_id: c.canonical_id.to_owned(),
                        description: c.description.to_owned(),
                        schema_version: c.schema_version,
                    })
                    .collect(),
                anchor: board.context.anchor.clone(),
                graph_identity: board.context.graph_identity.clone().unwrap_or_default(),
                pack_identity: board.context.pack_identity.clone(),
                policy_denied: Vec::new(),
            },
            tier1_list: list,
            retrieved_subset_hash: result.retrieved_subset_hash.clone(),
            label: e.label.clone(),
            family_id,
            pair_group_id: e.pair_group.clone(),
            style_regime: e.regime.clone(),
            utterance: e.text.clone(),
            gold_in_tier1,
        });
    }

    if !bad_labels.is_empty() {
        bail!(
            "HALT: {} eval entries label candidates their boards do not propose:\n{}",
            bad_labels.len(),
            bad_labels.join("\n")
        );
    }

    let n = examples.len().max(1) as f64;
    let recall_at_k = gold_in_tier1_count as f64 / n;
    let tier0_top1_accuracy = tier0_top1_count as f64 / n;
    let per_class_recall_json: BTreeMap<String, serde_json::Value> = per_class_recall
        .iter()
        .map(|(k, (hits, total))| {
            (
                k.clone(),
                serde_json::json!({"hits": hits, "total": total, "recall": *hits as f64 / *total as f64}),
            )
        })
        .collect();

    let card = serde_json::json!({
        "corpus_version": format!("{CORPUS_VERSION}.eval_enriched"),
        "source": "seed/banks/eval_*.json",
        "retriever": retriever.bundle_identity(),
        "k": K,
        "totals": {
            "examples": examples.len(),
            "gold_in_tier1": gold_in_tier1_count,
            "recall_at_k": recall_at_k,
            "tier0_top1_correct": tier0_top1_count,
            "tier0_top1_accuracy": tier0_top1_accuracy,
        },
        "per_class_recall_at_k": per_class_recall_json,
        "note": "recall_at_k is the easy bar (gold anywhere in the top-K) -- NOT an uplift baseline. tier0_top1_accuracy (tier1_list[0], the retriever's own #1 rank-ordered pick) IS the apples-to-apples number any trained SLM's top1_end_to_end must be compared against to claim uplift. This card IS the plan's Phase D 'C5 baseline' receipt for whichever retriever this binary was built with. Board completeness should read 1.0 always (every board.candidates entry came from a real build_board call, never invented) -- if it doesn't, the generator is broken, not the eval set.",
    });

    let jsonl: String = examples
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        out_dir.join(format!("{CORPUS_VERSION}.eval_enriched.jsonl")),
        jsonl + "\n",
    )?;
    std::fs::write(
        out_dir.join(format!("{CORPUS_VERSION}.eval_enriched.card.json")),
        serde_json::to_string_pretty(&card)? + "\n",
    )?;

    println!(
        "EVAL-ENRICH: {} examples, recall@{K} = {:.4} ({}/{}), tier0_top1_accuracy (C5 baseline, {}) = {:.4} ({}/{})",
        examples.len(),
        recall_at_k,
        gold_in_tier1_count,
        examples.len(),
        retriever.bundle_identity(),
        tier0_top1_accuracy,
        tier0_top1_count,
        examples.len(),
    );
    Ok(())
}
