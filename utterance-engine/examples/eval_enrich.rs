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
use utterance_engine::corpus_schema::{BankEntry, BoardDump, Example};
use utterance_engine::fixtures::{enumeration_classes, ClassState};
#[cfg(not(feature = "embed"))]
use utterance_engine::retrieval::LexicalTier0;
use utterance_engine::retrieval::{tier1_list, Tier0Retriever};

// K per EOP-DIR-BPMN-DESIGN-003-003 (Adam, ratified 2026-08-01): the ONE
// standing constant `retrieval::TIER1_K` (=12) -- the recall@K curve
// (2026-07-28 receipt) showed K=8's 4% ceiling was all four misses
// sitting at ranks 9-11, not lost; K=12 closes it on this eval set.
// The four currently-canonical bundles were TRAINED on K=8+NOTA-sized
// lists; serving/eval run the ratified wider list without retraining
// (measured in the bake-off report), and corpus_gen.rs generates at the
// same TIER1_K for the next retrain (corpus-v2).
const K: usize = utterance_engine::retrieval::TIER1_K;
const CORPUS_VERSION: &str = "synthetic-v2-beta";

/// A2.5 genuine-ambiguity item (seed/eval_ambiguity_v1.json): an
/// utterance CONSTRUCTED to be truly ambiguous between two boarded
/// candidates. These are never force-labelled — the suite tests that a
/// model's scores come out CLOSE (feeding the disposition policy's
/// clarification path), not which side it picks.
#[derive(serde::Deserialize)]
struct AmbiguityItem {
    class_id: String,
    candidate_a: String,
    candidate_b: String,
    text: String,
    #[allow(dead_code)]
    why_ambiguous: String,
}

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
    let mut gold_ranks: Vec<usize> = Vec::new(); // non-NOTA entries: gold's 1-based rank in the FULL board ranking
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
        // Recall@K curve input (close-out directive 2026-07-28): the
        // retriever ranks the ENTIRE board, so the gold label's exact
        // 1-based rank is already in hand — recording it costs nothing
        // and answers "would widening K past 8 close the recall ceiling"
        // directly (a K=8 miss at rank 9 is one free slot away; a miss
        // at rank 15 says the embedding is genuinely lost). NOTA labels
        // have no rank in the tier-0 ranking-proper sense (NOTA is
        // always appended to the served list); they're excluded here.
        if e.label != NONE_OF_THE_ABOVE {
            let rank = result
                .ranking
                .iter()
                .position(|rc| rc.candidate_id == e.label)
                .map(|p| p + 1)
                .ok_or_else(|| anyhow!("gold '{}' missing from full board ranking", e.label))?;
            gold_ranks.push(rank);
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
            board: BoardDump::from_board(board),
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

    // Recall@K curve over the same NOTA-counts-as-hit convention as
    // gold_in_tier1 (NOTA is always appended to the served list, so a
    // NOTA-labelled entry is served correctly at every K).
    let nota_count = examples.len() - gold_ranks.len();
    let curve: BTreeMap<String, serde_json::Value> = [1usize, 2, 4, 8, 12, 16]
        .iter()
        .map(|&k| {
            let hits = nota_count + gold_ranks.iter().filter(|&&r| r <= k).count();
            (
                format!("{k:02}"),
                serde_json::json!({"hits": hits, "recall": hits as f64 / n}),
            )
        })
        .collect();
    let mut miss_ranks: Vec<usize> = gold_ranks.iter().copied().filter(|&r| r > K).collect();
    miss_ranks.sort_unstable();
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
        "recall_curve": curve,
        "k8_miss_gold_ranks": miss_ranks,
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

    // A2.5 ambiguity-set enrichment: same board/projection machinery,
    // emitted as Example records so score_trained_bundle's scoring path
    // works unchanged. CONVENTION (documented, not hidden): `label` is
    // candidate_a and tier1_list is [candidate_a, candidate_b] — the
    // "label" is NOT gold (these items are deliberately unlabelable);
    // the suite only ever reads the PAIR and measures score closeness.
    let amb_path = root.join("seed/eval_ambiguity_v1.json");
    let amb_items: Vec<AmbiguityItem> =
        serde_json::from_str(&std::fs::read_to_string(&amb_path).context("eval_ambiguity_v1.json")?)?;
    let mut amb_examples: Vec<Example> = Vec::new();
    let mut amb_bad: Vec<String> = Vec::new();
    for item in &amb_items {
        let (board, proj, _c) = boards
            .get(item.class_id.as_str())
            .ok_or_else(|| anyhow!("ambiguity item names unknown class '{}'", item.class_id))?;
        for cand in [&item.candidate_a, &item.candidate_b] {
            if cand != NONE_OF_THE_ABOVE && !board.contains(cand) {
                amb_bad.push(format!(
                    "'{}' not proposed by board '{}' ({:?})",
                    cand, item.class_id, item.text
                ));
            }
        }
        let mut pre = Vec::new();
        pre.extend_from_slice(board.board_hash.as_bytes());
        pre.extend_from_slice(item.candidate_a.as_bytes());
        pre.extend_from_slice(item.candidate_b.as_bytes());
        pre.extend_from_slice(blake3::hash(item.text.as_bytes()).to_hex().as_bytes());
        amb_examples.push(Example {
            example_id: blake3::hash(&pre).to_hex().to_string(),
            provenance: format!("{CORPUS_VERSION}.ambiguity_enriched"),
            board_hash: board.board_hash.clone(),
            context_projection: proj.serialize_canonical(),
            context_projection_hash: proj.hash(),
            board: BoardDump::from_board(board),
            tier1_list: vec![item.candidate_a.clone(), item.candidate_b.clone()],
            retrieved_subset_hash: String::new(), // no retrieval ran; the pair IS the list
            label: item.candidate_a.clone(),
            family_id: format!("{}::ambiguity", item.class_id),
            pair_group_id: None,
            style_regime: "ambiguity".to_string(),
            utterance: item.text.clone(),
            gold_in_tier1: true,
        });
    }
    if !amb_bad.is_empty() {
        bail!("HALT: ambiguity-set defects:\n{}", amb_bad.join("\n"));
    }
    let amb_jsonl: String = amb_examples
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        out_dir.join(format!("{CORPUS_VERSION}.ambiguity_enriched.jsonl")),
        amb_jsonl + "\n",
    )?;
    println!("EVAL-ENRICH ambiguity set: {} pairs enriched", amb_examples.len());
    Ok(())
}
