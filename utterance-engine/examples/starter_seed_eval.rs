//! Corrected `starter-seed-v1` evaluator for Semantic Gameboard Phase 0.
//!
//! This is a read-only evaluation instrument. It builds the same semantic
//! board and context as graph-backed serving, scores the complete candidate
//! board with the admitted v3 pair route, finalises evidence, and invokes the
//! same deterministic disposition policy. It never rewrites the historical
//! v2 corpus/report files produced by the invalid legacy instrument.
//!
//! Run:
//! `cargo run -p utterance-engine --example starter_seed_eval --features candle-probe --release`
//!
//! The default output is a new Phase 0 receipt artifact. Override it with
//! `--report <path>`; use `--stdout-only` to perform no writes.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context, Result};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{bpmn_semantic_snapshot_identity, build_bpmn_semantic_board};
use utterance_engine::context::{project_ir, ContextProjection};
use utterance_engine::contract::NONE_OF_THE_ABOVE;
use utterance_engine::corpus_schema::SemanticCorpusClosure;
use utterance_engine::disposition::StrictCompoundSyntax;
use utterance_engine::exact::{finalize_semantic_evidence, EvidenceLane};
use utterance_engine::fixtures::{enumeration_classes, ClassState};
use utterance_engine::policy::{decide_with_action_spans, DispositionConfig, ProposalDisposition};
use utterance_engine::trained_ranker::{Base, Tier1Ranker};

const SUITE: &str = "starter-seed-v1";
const INSTRUMENT: &str = "semantic-gameboard.phase0.v3-serving-parity.v1";
const CANONICAL_BASE: Base = Base::ModernbertBase;

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

#[derive(serde::Deserialize)]
struct Adjudication {
    seq: u32,
    adjudicated_label: String,
}

struct EvaluationPosition<'a> {
    board: semantic_decision_contracts::SemanticDecisionBoard,
    context: ContextProjection,
    class: &'a ClassState,
}

enum Output {
    Report(std::path::PathBuf),
    StdoutOnly,
}

fn output_mode(root: &std::path::Path) -> Result<Output> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None => Ok(Output::Report(root.parent().unwrap().join(
            "docs/receipts/artifacts/semantic-gameboard-phase0-starter-evaluation.json",
        ))),
        Some("--stdout-only") if args.next().is_none() => Ok(Output::StdoutOnly),
        Some("--report") => {
            let path = args.next().context("--report requires a path")?;
            if args.next().is_some() {
                bail!("unexpected arguments after --report <path>");
            }
            Ok(Output::Report(path.into()))
        }
        Some(other) => bail!("unknown argument '{other}'"),
    }
}

fn disposition_name(disposition: &ProposalDisposition) -> &'static str {
    match disposition {
        ProposalDisposition::Candidate { .. } => "candidate",
        ProposalDisposition::Ambiguous { .. } => "ambiguous",
        ProposalDisposition::MissingArguments { .. } => "missing_arguments",
        ProposalDisposition::Compound { .. } => "compound",
        ProposalDisposition::OutOfScope => "out_of_scope",
        ProposalDisposition::EscalateToSage { .. } => "escalate_to_sage",
    }
}

fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = output_mode(root)?;
    let bank_path = root.join("seed/banks/starter_seed_v1.json");
    let items: Vec<StarterItem> = serde_json::from_str(&std::fs::read_to_string(&bank_path)?)
        .with_context(|| format!("{bank_path:?}"))?;

    let adj_path = root.join("seed/banks/starter_seed_v1_adjudications.json");
    let adjudications: BTreeMap<u32, String> = if adj_path.exists() {
        let list: Vec<Adjudication> = serde_json::from_str(&std::fs::read_to_string(&adj_path)?)
            .with_context(|| format!("{adj_path:?}"))?;
        list.into_iter()
            .map(|item| (item.seq, item.adjudicated_label))
            .collect()
    } else {
        BTreeMap::new()
    };

    let classes = enumeration_classes()?;
    let mut positions = BTreeMap::new();
    for class in &classes {
        let ir = class
            .dag
            .to_ir()
            .map_err(|error| anyhow!("{}: {error}", class.class_id))?;
        let graph_identity = format!("starter-seed-v1:class:{}", class.class_id);
        let anchor = class.anchor_key.zip(class.anchor_id);
        let board = build_bpmn_semantic_board(
            &class.dag,
            anchor,
            &graph_identity,
            &PolicyFilter::default(),
        )?;
        let context = project_ir(
            &ir,
            class.anchor_id,
            board.semantic_snapshot.as_str(),
            &graph_identity,
        )?;
        positions.insert(
            class.class_id,
            EvaluationPosition {
                board,
                context,
                class,
            },
        );
    }

    let bundle_dir = root.join("train_py/bundles").join(CANONICAL_BASE.key());
    let ranker = Tier1Ranker::load(CANONICAL_BASE, &bundle_dir)?;
    let policy = DispositionConfig::shadow_v2();
    let mut bad_labels = Vec::new();
    let mut rows = Vec::new();
    let mut per_category = BTreeMap::<String, (u32, u32, u32, u32)>::new();
    let mut dispositions = BTreeMap::<String, u32>::new();

    for item in &items {
        let position = positions
            .get(item.class_id.as_str())
            .ok_or_else(|| anyhow!("starter item names unknown class '{}'", item.class_id))?;
        let adjudicated_label = adjudications.get(&item.seq);
        let label = adjudicated_label
            .map(String::as_str)
            .unwrap_or(item.label.as_str());
        if label != NONE_OF_THE_ABOVE
            && !position
                .board
                .candidates
                .iter()
                .any(|candidate| candidate.canonical_id.as_str() == label)
        {
            bad_labels.push(format!(
                "seq {} '{}': hypothesis '{}' is not on semantic board '{}'",
                item.seq, item.text, label, item.class_id
            ));
            continue;
        }

        let closure = SemanticCorpusClosure::new(
            &position.board,
            &item.text,
            &position.context,
            label,
            format!("{}::{label}", item.class_id),
            SUITE.to_string(),
            format!("starter-seed-v1::{}", item.class_id),
        )?;
        let raw = ranker.rank_full_board(&item.text, &position.context, &position.board)?;
        let bundle = raw.model_bundle_hash.clone();
        let evidence = finalize_semantic_evidence(
            &position.board,
            &item.text,
            raw,
            vec![EvidenceLane::CandleCrossEncoder],
            vec![bundle],
        )?;
        let (disposition, decision) = decide_with_action_spans(
            &policy,
            &position.board,
            &evidence,
            &position.context,
            &item.text,
            &StrictCompoundSyntax,
        )?;
        let top = evidence
            .ranking
            .first()
            .map(|candidate| candidate.candidate_id.clone())
            .unwrap_or_default();
        let top_matches_hypothesis = top == label;
        let top3_contains_hypothesis = evidence
            .ranking
            .iter()
            .take(3)
            .any(|candidate| candidate.candidate_id == label);
        let nota_top1 = top == NONE_OF_THE_ABOVE;
        let category = per_category.entry(item.category.clone()).or_default();
        category.0 += 1;
        category.1 += u32::from(top_matches_hypothesis);
        category.2 += u32::from(top3_contains_hypothesis);
        category.3 += u32::from(nota_top1);
        *dispositions
            .entry(disposition_name(&disposition).to_string())
            .or_default() += 1;

        rows.push(serde_json::json!({
            "seq": item.seq,
            "category": item.category,
            "class_id": position.class.class_id,
            "text": item.text,
            "hypothesis_label": label,
            "original_hypothesis": item.label,
            "adjudicated": adjudicated_label.is_some(),
            "disputed": item.disputed && adjudicated_label.is_none(),
            "dispute_note": item.dispute_note,
            "board_hash": position.board.board_hash.as_str(),
            "context_projection_hash": position.context.hash(),
            "full_served_list": closure.full_served_list,
            "candidate_pair_hashes": closure.candidate_pairs.iter().map(|candidate| serde_json::json!({
                "candidate_id": candidate.candidate_id,
                "pair_hash": candidate.pair.pair_hash,
            })).collect::<Vec<_>>(),
            "exact_match": closure.exact_match,
            "top_candidate": top,
            "top_matches_hypothesis": top_matches_hypothesis,
            "top3_contains_hypothesis": top3_contains_hypothesis,
            "nota_top1": nota_top1,
            "ranking": evidence.ranking,
            "evidence_trace": evidence.evidence_trace,
            "disposition": disposition,
            "decision_record_hash": decision.decision_record_hash,
        }));
    }

    if !bad_labels.is_empty() {
        bail!(
            "HALT: {} starter hypotheses are absent from their semantic boards:\n{}",
            bad_labels.len(),
            bad_labels.join("\n")
        );
    }

    let category_report = per_category
        .into_iter()
        .map(|(name, (n, top1, top3, nota))| {
            (
                name,
                serde_json::json!({
                    "n": n,
                    "model_top1_matches_hypothesis": top1,
                    "model_top3_contains_hypothesis": top3,
                    "nota_top1": nota,
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let total_matches = rows
        .iter()
        .filter(|row| row["top_matches_hypothesis"] == true)
        .count();
    let total_top3 = rows
        .iter()
        .filter(|row| row["top3_contains_hypothesis"] == true)
        .count();
    let total_nota = rows.iter().filter(|row| row["nota_top1"] == true).count();
    let total_n = rows.len();
    let report = serde_json::json!({
        "instrument": INSTRUMENT,
        "suite": SUITE,
        "status": "corrected_measurement; evidence_not_acceptance_gate",
        "historical_comparison": "The prior legacy-textualisation result remains historical but is invalid for semantic-v3 serving and was not overwritten.",
        "semantic_pack_identity": bpmn_semantic_snapshot_identity(),
        "corpus_schema_id": utterance_engine::corpus_schema::CORPUS_SCHEMA_ID,
        "pair_serializer_id": utterance_engine::pair::PAIR_SERIALIZER_ID,
        "pair_serializer_hash": utterance_engine::pair::pair_serializer_hash(),
        "model_bundle_hash": ranker.model_bundle_hash(),
        "disposition_policy_hash": policy.policy_hash(),
        "adjudications_applied": adjudications.len(),
        "totals": {
            "n": total_n,
            "model_top1_matches_hypothesis": total_matches,
            "model_top3_contains_hypothesis": total_top3,
            "nota_top1": total_nota,
        },
        "per_category": category_report,
        "dispositions": dispositions,
        "rows": rows,
    });

    match output {
        Output::Report(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, serde_json::to_string_pretty(&report)? + "\n")?;
            println!("report: {}", path.display());
        }
        Output::StdoutOnly => println!("{}", serde_json::to_string_pretty(&report)?),
    }
    println!(
        "STARTER-SEED-EVAL ({SUITE}): {} items, v3 top1/hypothesis = {total_matches}, bundle = {}",
        total_n,
        ranker.model_bundle_hash()
    );
    Ok(())
}
