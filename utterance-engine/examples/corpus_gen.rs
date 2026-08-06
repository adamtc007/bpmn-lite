//! DIR-002 Phase B — the corpus generator (EOP-SPEC-SLM-TRAIN-001 v0.3).
//!
//! Labels are correct by construction: this binary enumerates REAL board
//! states (seed → ops → PositionalLegality → build_board), attaches
//! authored utterances from the checked-in banks, and refuses — loudly —
//! anything the spec forbids: a label not on its board, a leakage-cap
//! breach, a duplicate, a retrieval-drop it wasn't allowed to hide.
//! The teacher's language lives in `seed/banks/*.json`; nothing here
//! invents a label.
//!
//! Run: `cargo run -p utterance-engine --example corpus_gen`
//! Emits: seed/corpus_v2/<name>.jsonl + <name>.card.json
//!
//! Lists follow Adam's ruling (spec finding 5): the real tier-0
//! retriever's K-prefix + NOTA always appended (`retrieval::tier1_list`).
//! Gold-not-retrieved examples are DROPPED and counted (they would teach
//! false abstention); the drop rate is the card's retrieval-miss line.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::{anyhow, bail, Context, Result};
use semantic_decision_contracts::SemanticDecisionBoard;
use utterance_engine::board::{InferenceBoard, PolicyFilter};
use utterance_engine::bpmn_board::build_bpmn_semantic_board;
use utterance_engine::context::{project_ir, ContextProjection};
use utterance_engine::contract::NONE_OF_THE_ABOVE;
use utterance_engine::corpus_schema::{
    BankEntry, BoardDump, Example, SemanticCorpusClosure,
};
use utterance_engine::fixtures::{enumeration_classes, ClassState};
#[cfg(not(feature = "embed"))]
use utterance_engine::retrieval::LexicalTier0;
use utterance_engine::retrieval::Tier0Retriever;

// Spec S5 recorded K=8 as the trained configuration (historical, not edited);
// Adam widened K 8->12 (ratified 2026-08-01) -- next corpus (corpus-v2 retrain)
// generates at the ONE standing constant. Recorded in the card.
const OVERLAP_CAP: f64 = 0.5; // spec S4 A3.1
const CORPUS_VERSION: &str = "bpmn-semantic-v3-shadow";


fn tokens(s: &str) -> BTreeSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_owned())
        .collect()
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn normalized(s: &str) -> String {
    tokens(s).into_iter().collect::<Vec<_>>().join(" ")
}

fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let bank_dir = root.join("seed/banks");
    let out_dir = root.join("seed/corpus_v3");
    std::fs::create_dir_all(&out_dir)?;

    // Build every class once; index by id.
    let classes = enumeration_classes()?;
    let mut boards: BTreeMap<&str, (SemanticDecisionBoard, ContextProjection, &ClassState)> =
        BTreeMap::new();
    for c in &classes {
        let ir = c.dag.to_ir().map_err(|e| anyhow!("{}: {e}", c.class_id))?;
        let graph_identity = format!("class:{}", c.class_id);
        let anchor_pair = c.anchor_key.zip(c.anchor_id);
        let board = build_bpmn_semantic_board(
            &c.dag,
            anchor_pair,
            &graph_identity,
            &PolicyFilter::default(),
        )?;
        let proj = project_ir(
            &ir,
            c.anchor_id,
            board.semantic_snapshot.as_str(),
            &graph_identity,
        )?;
        boards.insert(c.class_id, (board, proj, c));
    }

    // Load banks. Files named `eval_*.json` are the HELD-OUT slice
    // (A3.3 disjoint-regime eval) — routed to a separate eval corpus,
    // NEVER into training (split leakage would be silent and fatal).
    // `starter_seed_v1*.json` is a DIFFERENT schema entirely (seq/category/
    // label/text/disputed, not BankEntry's regime-tagged shape) — a
    // hypothesis-only, not-gold-by-construction slice owned by
    // `starter_seed_eval.rs`/`adjudicate_starter_seed.rs` and explicitly
    // "kept out of training entirely" per that file's own doc comment.
    // It happens to live in this same directory; skip it by name rather
    // than let it fail BankEntry parsing (the pre-existing failure mode —
    // corpus_gen could not run at all once this file landed).
    let mut entries: Vec<BankEntry> = Vec::new();
    let mut eval_entries: Vec<BankEntry> = Vec::new();
    for f in std::fs::read_dir(&bank_dir).context("seed/banks missing")? {
        let path = f?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .unwrap_or_default();
        if file_name.starts_with("starter_seed_v1") {
            continue;
        }
        let bank: Vec<BankEntry> = serde_json::from_str(&std::fs::read_to_string(&path)?)
            .with_context(|| format!("{path:?}"))?;
        if file_name.starts_with("eval_") {
            eval_entries.extend(bank);
        } else {
            entries.extend(bank);
        }
    }
    if entries.is_empty() {
        bail!("no bank entries — author seed/banks/*.json first");
    }

    // The retriever is the card-recorded generation parameter: embed
    // tier-0 (E3) when built with --features embed — required for full
    // synthetic-v2 (context pairs are lexically invisible by design) —
    // else the lexical baseline.
    #[cfg(feature = "embed")]
    let retriever = utterance_engine::retrieval::embed::EmbedTier0::new()?;
    #[cfg(not(feature = "embed"))]
    let retriever = LexicalTier0;
    let mut examples: Vec<Example> = Vec::new();
    let mut dropped_overlap = 0u32;
    let dropped_retrieval_miss = 0u32;
    let mut dropped_duplicate = 0u32;
    let mut seen_norm: HashSet<(String, String)> = HashSet::new(); // (class, normalized utterance)
    let mut bad_labels: Vec<String> = Vec::new();
    let mut regime_counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut label_counts: BTreeMap<String, u32> = BTreeMap::new();

    for e in &entries {
        let (board, proj, _c) = boards
            .get(e.class_id.as_str())
            .ok_or_else(|| anyhow!("bank names unknown class '{}'", e.class_id))?;

        // Label must be ON its board — the by-construction guarantee.
        // Collected (not first-fail) so ONE run surfaces every offender.
        if e.label != NONE_OF_THE_ABOVE && !board.contains(&e.label) {
            bad_labels.push(format!("'{}' not proposed by board '{}' ({:?})", e.label, e.class_id, e.text));
            continue;
        }

        // Leakage cap (spec S4): vs correct description, or — NOTA rule —
        // vs EVERY boarded description.
        let utoks = tokens(&e.text);
        let visible_candidates = board.inference_candidates();
        let breach = if e.label == NONE_OF_THE_ABOVE {
            visible_candidates
                .iter()
                .filter(|c| c.canonical_id != NONE_OF_THE_ABOVE)
                .any(|c| jaccard(&utoks, &tokens(&c.description)) > OVERLAP_CAP)
        } else {
            let desc = visible_candidates
                .iter()
                .find(|c| c.canonical_id == e.label)
                .expect("checked above")
                .description
                .clone();
            jaccard(&utoks, &tokens(&desc)) > OVERLAP_CAP
        };
        if breach {
            dropped_overlap += 1;
            continue;
        }

        // Near-duplicate removal (v1: normalized-token exact dup per class).
        if !seen_norm.insert((e.class_id.clone(), normalized(&e.text))) {
            dropped_duplicate += 1;
            continue;
        }

        // The ruled list: real tier-0 K-prefix + NOTA.
        let result = retriever.retrieve(&e.text, board)?;
        let list = board
            .candidates
            .iter()
            .map(|candidate| candidate.canonical_id.as_str().to_string())
            .collect::<Vec<_>>();
        if !list.contains(&e.label) {
            bail!("full-board corpus omitted legal gold candidate '{}'", e.label);
        }

        let family_id = format!("{}::{}", e.class_id, e.label);
        let mut pre = Vec::new();
        pre.extend_from_slice(board.board_hash.as_str().as_bytes());
        pre.extend_from_slice(e.label.as_bytes());
        pre.extend_from_slice(blake3::hash(e.text.as_bytes()).to_hex().as_bytes());
        pre.extend_from_slice(&e.paraphrase_seq.to_le_bytes());

        *regime_counts.entry(e.regime.clone()).or_insert(0) += 1;
        *label_counts.entry(e.label.clone()).or_insert(0) += 1;
        examples.push(Example {
            example_id: blake3::hash(&pre).to_hex().to_string(),
            provenance: CORPUS_VERSION.to_string(),
            board_hash: board.board_hash.as_str().to_string(),
            context_projection: proj.serialize_canonical(),
            context_projection_hash: proj.hash(),
            board: BoardDump::from_inference_board(board),
            tier1_list: list,
            retrieved_subset_hash: result.retrieved_subset_hash.clone(),
            label: e.label.clone(),
            family_id: family_id.clone(),
            pair_group_id: e.pair_group.clone(),
            style_regime: e.regime.clone(),
            utterance: e.text.clone(),
            // Always true for training records: the retrieval-miss check
            // above already dropped this example otherwise.
            gold_in_tier1: true,
            semantic_v3: Some(SemanticCorpusClosure::new(
                board,
                &e.text,
                proj,
                &e.label,
                family_id.clone(),
                e.regime.clone(),
                e.pair_group.clone().unwrap_or_else(|| family_id.clone()),
            )?),
        });
    }

    if !bad_labels.is_empty() {
        bail!(
            "HALT: {} bank entries label candidates their boards do not propose — either the banks are wrong or the legality oracle changed:\n{}",
            bad_labels.len(),
            bad_labels.join("\n")
        );
    }

    // Pair-group integrity: structural violations HALT (bank defects);
    // a pair whose side was dropped by hygiene/retrieval loses BOTH
    // sides — counted, never silently half-kept (a lone side is an
    // ordinary example wearing a pair label).
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, ex) in examples.iter().enumerate() {
        if let Some(g) = &ex.pair_group_id {
            groups.entry(g.clone()).or_default().push(i);
        }
    }
    let mut dropped_pair_break = 0u32;
    let mut remove: Vec<usize> = Vec::new();
    for (g, members) in &groups {
        if members.len() > 2 {
            bail!("HALT: pair group '{g}' has {} sides authored (max 2)", members.len());
        }
        if members.len() == 2 {
            let (a, b) = (&examples[members[0]], &examples[members[1]]);
            if a.utterance != b.utterance {
                bail!("HALT: pair group '{g}' sides differ in text");
            }
            if a.family_id == b.family_id {
                bail!("HALT: pair group '{g}' sides share a family — not a context pair");
            }
        } else {
            dropped_pair_break += 1;
            remove.extend(members.iter().copied());
        }
    }
    remove.sort_unstable();
    for i in remove.into_iter().rev() {
        let ex = examples.remove(i);
        *regime_counts.get_mut(&ex.style_regime).expect("counted at insert") -= 1;
        *label_counts.get_mut(&ex.label).expect("counted at insert") -= 1;
    }

    let nota = examples.iter().filter(|e| e.label == NONE_OF_THE_ABOVE).count();
    let paired = examples.iter().filter(|e| e.pair_group_id.is_some()).count();
    let card = serde_json::json!({
        "corpus_version": CORPUS_VERSION,
        "spec": "BPMN semantic mapper corpus v3 / Phase 6",
        "retriever": retriever.bundle_identity(),
        "list_rule": "complete position-legal semantic board plus abstention",
        "ctxproj_schema_version": utterance_engine::context::CONTEXT_PROJECTION_SCHEMA_VERSION,
        "overlap_cap": OVERLAP_CAP,
        "totals": {
            "examples": examples.len(),
            "nota": nota, "nota_pct": nota as f64 / examples.len().max(1) as f64,
            "context_pairs": paired, "pair_pct": paired as f64 / examples.len().max(1) as f64,
        },
        "dropped": {
            "overlap_cap": dropped_overlap,
            "retrieval_miss": dropped_retrieval_miss,
            "duplicate": dropped_duplicate,
            "pair_break": dropped_pair_break,
        },
        "per_regime": regime_counts,
        "per_label": label_counts,
        "floors": {
            "note": format!("{CORPUS_VERSION} is a shadow pipeline/authoring receipt; promotion still requires independently authored evaluation and owner-ratified thresholds"),
            "total_floor_met": examples.len() >= 5000,
        },
    });

    let jsonl: String = examples
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(out_dir.join(format!("{CORPUS_VERSION}.jsonl")), jsonl + "\n")?;
    std::fs::write(
        out_dir.join(format!("{CORPUS_VERSION}.card.json")),
        serde_json::to_string_pretty(&card)? + "\n",
    )?;
    if !eval_entries.is_empty() {
        // Held-out slice: same board/label validation discipline applies,
        // but no hygiene drops (eval keeps the ugly cases — that is its
        // job); emitted as raw labelled entries for the Phase-D harness.
        let mut eval_bad: Vec<String> = Vec::new();
        for e in &eval_entries {
            match boards.get(e.class_id.as_str()) {
                None => eval_bad.push(format!("eval entry names unknown class '{}'", e.class_id)),
                Some((board, _, _)) => {
                    if e.label != NONE_OF_THE_ABOVE && !board.contains(&e.label) {
                        eval_bad.push(format!(
                            "eval '{}' not proposed by board '{}' ({:?})",
                            e.label, e.class_id, e.text
                        ));
                    }
                }
            }
        }
        if !eval_bad.is_empty() {
            bail!("HALT: eval slice defects:\n{}", eval_bad.join("\n"));
        }
        let eval_jsonl: String = eval_entries
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(out_dir.join(format!("{CORPUS_VERSION}.eval.jsonl")), eval_jsonl + "\n")?;
        println!("CORPUS-GEN eval slice: {} held-out entries (never trained)", eval_entries.len());
    }
    println!(
        "CORPUS-GEN {CORPUS_VERSION}: {} examples ({nota} NOTA, {paired} paired), dropped: {dropped_overlap} overlap / {dropped_retrieval_miss} retrieval-miss / {dropped_duplicate} dup",
        examples.len()
    );
    Ok(())
}
