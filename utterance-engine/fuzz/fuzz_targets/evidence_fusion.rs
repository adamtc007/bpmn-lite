#![no_main]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU16, Ordering};

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::Operation;
use designer_graph::productions::apply_production;
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    CorrectionKind, DesignFocus, EvidenceLane, FocusAbsenceReason, GraphContentHash,
    GraphElementRef, MoveAttemptId, MoveAttemptOutcome, MoveAttemptReceipt,
    GAMEBOARD_SCHEMA_VERSION,
};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, finalize_bpmn_move_evidence,
};
use utterance_engine::contract::{FiniteScore, RankedCandidate, SlmResult};
use uuid::Uuid;

static SHAPES: AtomicU16 = AtomicU16::new(0);

fn observe(index: u8, label: &str) {
    let bit = 1_u16 << index;
    if SHAPES.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter evidence_shape={label}");
    }
}

fn key(value: u128) -> NodeKey {
    NodeKey(Uuid::from_u128(value))
}

fn graph(task_count: usize) -> (DesignerDag, Vec<(NodeKey, String)>) {
    let start = key(1);
    let mut dag = DesignerDag::new("evidence-fuzz");
    dag.seed(
        start,
        IRNode::Start { id: "start".into() },
        Provenance::default(),
    )
    .unwrap();
    let mut elements = vec![(start, "start".to_string())];
    let mut anchor = start;
    for index in 0..task_count {
        let node = key(index as u128 + 2);
        let id = format!("task_{index}");
        dag = apply_production(
            &dag,
            vec![Operation::AppendNode {
                anchor,
                key: node,
                node: IRNode::ServiceTask {
                    id: id.clone(),
                    name: id.clone(),
                    task_type: "noop".into(),
                },
                edge_id: format!("flow_{index}"),
            }],
            Provenance::default(),
        )
        .unwrap()
        .candidate;
        elements.push((node, id));
        anchor = node;
    }
    (dag, elements)
}

fn base(board: &semantic_decision_contracts::SemanticDecisionBoard, seed: u8) -> SlmResult {
    SlmResult {
        ranking: board
            .candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| RankedCandidate {
                candidate_id: candidate.canonical_id.as_str().to_string(),
                score: FiniteScore::new(
                    f64::from(seed.wrapping_add(index as u8)) / f64::from(u8::MAX),
                )
                .unwrap(),
            })
            .collect(),
        retrieved_subset_hash: "fuzz-full-board".into(),
        board_hash: board.board_hash.as_str().to_string(),
        model_bundle_hash: "fuzz.model".into(),
        evidence_trace: None,
        inference_evidence: None,
        move_evidence: Vec::new(),
    }
}

fn attempt(
    selector: u8,
    position: &semantic_decision_contracts::DesignPosition,
) -> Option<MoveAttemptReceipt> {
    let target = position.legal_moves().first()?;
    match selector % 4 {
        0 => None,
        1 => Some(
            MoveAttemptReceipt::new(
                GAMEBOARD_SCHEMA_VERSION,
                MoveAttemptId::new("fuzz-rejected").unwrap(),
                position.state_id().clone(),
                Some(target.move_id().clone()),
                GraphContentHash::new("d".repeat(64)).unwrap(),
                MoveAttemptOutcome::RejectedByUser,
                Vec::new(),
                Vec::new(),
                None,
                None,
            )
            .unwrap(),
        ),
        2 => Some(
            MoveAttemptReceipt::new(
                GAMEBOARD_SCHEMA_VERSION,
                MoveAttemptId::new("fuzz-corrected").unwrap(),
                position.state_id().clone(),
                Some(target.move_id().clone()),
                GraphContentHash::new("e".repeat(64)).unwrap(),
                MoveAttemptOutcome::Corrected,
                Vec::new(),
                Vec::new(),
                Some(MoveAttemptId::new("fuzz-original").unwrap()),
                Some(CorrectionKind::Replacement),
            )
            .unwrap(),
        ),
        _ => Some(
            MoveAttemptReceipt::new(
                GAMEBOARD_SCHEMA_VERSION,
                MoveAttemptId::new("fuzz-irrelevant").unwrap(),
                position.state_id().clone(),
                None,
                GraphContentHash::new("f".repeat(64)).unwrap(),
                MoveAttemptOutcome::Ambiguous,
                Vec::new(),
                Vec::new(),
                None,
                None,
            )
            .unwrap(),
        ),
    }
}

fuzz_target!(|data: &[u8]| {
    let task_count = usize::from(data.first().copied().unwrap_or_default() % 4) + 1;
    let (dag, elements) = graph(task_count);
    let anchor_index = usize::from(data.get(1).copied().unwrap_or_default()) % (elements.len() + 1);
    let anchor = (anchor_index < elements.len()).then(|| &elements[anchor_index]);
    let revision = format!("{:064x}", data.get(2).copied().unwrap_or_default());
    let board = build_bpmn_semantic_board(
        &dag,
        anchor.map(|(key, id)| (*key, id.as_str())),
        &revision,
        &PolicyFilter::default(),
    )
    .unwrap();
    let focus = anchor.map_or_else(
        || DesignFocus::absent(FocusAbsenceReason::NotProvided, None).unwrap(),
        |(_, id)| DesignFocus::element(GraphElementRef::new(id).unwrap()),
    );
    let position = build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &"b".repeat(64),
        "compiler-fuzz-v1",
        &"c".repeat(64),
        focus,
        None,
    )
    .unwrap();
    let before = position
        .legal_moves()
        .iter()
        .map(|legal_move| legal_move.move_id().clone())
        .collect::<BTreeSet<_>>();
    let utterance_selector = data.get(3).copied().unwrap_or_default() % 6;
    let anchor_name = anchor.map(|(_, id)| id.as_str()).unwrap_or("task_0");
    let utterance = match utterance_selector {
        0 => {
            observe(0, "exact");
            "insert after".to_string()
        }
        1 => {
            observe(1, "duration");
            format!("interrupt {anchor_name} after 5 minutes")
        }
        2 => {
            observe(2, "count");
            format!("create 3 parallel branches at {anchor_name}")
        }
        3 => {
            observe(3, "node_reference");
            format!("change {anchor_name}")
        }
        4 => {
            observe(4, "negative_contrast");
            "conditionally selected inclusive branches".to_string()
        }
        _ => {
            observe(5, "abstention");
            "unrelated xylophone request".to_string()
        }
    };
    let active_lane = match data.get(4).copied().unwrap_or_default() % 3 {
        0 => EvidenceLane::Lexical,
        1 => EvidenceLane::Embedding,
        _ => EvidenceLane::CandleCrossEncoder,
    };
    let history_selector = data.get(5).copied().unwrap_or_default();
    let history = attempt(history_selector, &position)
        .into_iter()
        .collect::<Vec<_>>();
    assert!(FiniteScore::new(f64::NAN).is_err());
    assert!(FiniteScore::new(f64::INFINITY).is_err());
    let mut base = base(&board, data.get(6).copied().unwrap_or_default());
    let malformed = data.get(7).copied().unwrap_or_default() % 3;
    if malformed == 1 && base.ranking.len() > 1 {
        base.ranking[1].candidate_id = base.ranking[0].candidate_id.clone();
    } else if malformed == 2 {
        base.ranking.pop();
    }
    if malformed != 0 {
        assert!(finalize_bpmn_move_evidence(
            &board,
            &position,
            &utterance,
            base,
            active_lane,
            vec!["fuzz.model".into()],
            &history,
        )
        .is_err());
        assert_eq!(
            position
                .legal_moves()
                .iter()
                .map(|legal_move| legal_move.move_id().clone())
                .collect::<BTreeSet<_>>(),
            before
        );
        return;
    }
    let first = finalize_bpmn_move_evidence(
        &board,
        &position,
        &utterance,
        base.clone(),
        active_lane,
        vec!["fuzz.model".into()],
        &history,
    )
    .unwrap();
    let mut reordered = base.clone();
    reordered.ranking.reverse();
    let second = finalize_bpmn_move_evidence(
        &board,
        &position,
        &utterance,
        reordered,
        active_lane,
        vec!["fuzz.model".into()],
        &history,
    )
    .unwrap();

    assert_eq!(first.move_evidence.len(), position.legal_moves().len());
    let observed = first
        .move_evidence
        .iter()
        .map(|evidence| evidence.move_id().clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(observed, before);
    assert!(first.move_evidence.iter().all(|evidence| {
        evidence.lanes().len() == EvidenceLane::ALL.len()
            && evidence
                .lanes()
                .iter()
                .all(|lane| lane.score.get().is_finite())
            && evidence.final_score().get().is_finite()
            && evidence.probability().get().is_finite()
    }));
    let probability_sum = first
        .move_evidence
        .iter()
        .map(|evidence| evidence.probability().get())
        .sum::<f64>();
    assert!((probability_sum - 1.0).abs() < 1e-12);
    assert_eq!(
        serde_json::to_value(&first.move_evidence).unwrap(),
        serde_json::to_value(&second.move_evidence).unwrap()
    );
    assert_eq!(
        first
            .inference_evidence
            .as_ref()
            .unwrap()
            .evidence_record_hash,
        second
            .inference_evidence
            .as_ref()
            .unwrap()
            .evidence_record_hash
    );
    if history_selector % 4 == 3 {
        let without_irrelevant_history = finalize_bpmn_move_evidence(
            &board,
            &position,
            &utterance,
            base.clone(),
            active_lane,
            vec!["fuzz.model".into()],
            &[],
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&first.move_evidence).unwrap(),
            serde_json::to_value(&without_irrelevant_history.move_evidence).unwrap()
        );
    }
    if utterance_selector == 0 {
        let canonical_equivalent = finalize_bpmn_move_evidence(
            &board,
            &position,
            "  INSERT   AFTER  ",
            base,
            active_lane,
            vec!["fuzz.model".into()],
            &history,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&first.move_evidence).unwrap(),
            serde_json::to_value(&canonical_equivalent.move_evidence).unwrap()
        );
    }
    assert_eq!(
        position
            .legal_moves()
            .iter()
            .map(|legal_move| legal_move.move_id().clone())
            .collect::<BTreeSet<_>>(),
        before
    );
});
