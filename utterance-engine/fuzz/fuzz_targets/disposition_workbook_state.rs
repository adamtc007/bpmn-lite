#![no_main]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::{apply, Operation};
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    ArgumentKind, BoardHash, CorrectionKind, DesignBelief, DesignFocus, EvidenceLane,
    EvidenceRecordHash, FiniteScore, GameDispositionKind, GraphElementRef, LaneScore, LegalMoveId,
    MoveAttemptId, MoveAttemptOutcome, MoveAttemptReceipt, MoveEvidence, ProducerIdentity,
    ProposalStatus, ProposalWorkbook, SlotRequirement, SlotValue, SlotValueState, WorkbookId,
    WorkbookSlot, GAMEBOARD_SCHEMA_VERSION,
};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, decide_bpmn_game_disposition,
    project_bpmn_attempt_history, record_bpmn_attempt,
};
use uuid::Uuid;

const MAX_INPUT_BYTES: usize = 4096;
static COUNTERS: AtomicU32 = AtomicU32::new(0);

fn observe(index: u8, family: &str, label: &str) {
    let bit = 1_u32 << index;
    if COUNTERS.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter {family}={label}");
    }
}

fn selector(byte: u8) -> u8 {
    if byte.is_ascii_digit() {
        byte - b'0'
    } else {
        byte % 10
    }
}

fn graph() -> (DesignerDag, NodeKey) {
    let start = NodeKey(Uuid::from_u128(1));
    let task = NodeKey(Uuid::from_u128(2));
    let end = NodeKey(Uuid::from_u128(3));
    let mut dag = DesignerDag::new("disposition-workbook-fuzz");
    dag.seed(
        start,
        IRNode::Start { id: "start".into() },
        Provenance::default(),
    )
    .unwrap();
    dag = apply(
        &dag,
        Operation::AppendNode {
            anchor: start,
            key: task,
            node: IRNode::ServiceTask {
                id: "review".into(),
                name: "Review".into(),
                task_type: "review".into(),
            },
            edge_id: "flow_review".into(),
        },
        Provenance::default(),
    )
    .unwrap()
    .candidate;
    dag = apply(
        &dag,
        Operation::AppendNode {
            anchor: task,
            key: end,
            node: IRNode::End {
                id: "end".into(),
                terminate: false,
            },
            edge_id: "flow_end".into(),
        },
        Provenance::default(),
    )
    .unwrap()
    .candidate;
    (dag, task)
}

fn evidence(
    position: &semantic_decision_contracts::DesignPosition,
    scores: &BTreeMap<LegalMoveId, (f64, f64, f64)>,
) -> Vec<MoveEvidence> {
    position
        .legal_moves()
        .iter()
        .map(|legal_move| {
            let (score, probability, typed) = scores
                .get(legal_move.move_id())
                .copied()
                .unwrap_or((0.01, 0.0, 0.0));
            MoveEvidence::new(
                GAMEBOARD_SCHEMA_VERSION,
                legal_move.move_id().clone(),
                vec![LaneScore {
                    lane: EvidenceLane::TypedArgument,
                    score: FiniteScore::new(typed).unwrap(),
                }],
                FiniteScore::new(score).unwrap(),
                FiniteScore::new(probability).unwrap(),
                Vec::new(),
                ProducerIdentity::new("fuzz.disposition-evidence.v1").unwrap(),
            )
            .unwrap()
        })
        .collect()
}

fn receipt(
    position: &semantic_decision_contracts::DesignPosition,
    index: usize,
    outcome: MoveAttemptOutcome,
) -> MoveAttemptReceipt {
    record_bpmn_attempt(
        position,
        MoveAttemptId::new(format!("history-{index}")).unwrap(),
        position
            .legal_moves()
            .first()
            .map(|item| item.move_id().clone()),
        "retained fuzz attempt",
        outcome,
        None,
        None,
    )
    .unwrap()
}

fn expected_transition(from: ProposalStatus, to: ProposalStatus) -> bool {
    matches!(
        (from, to),
        (
            ProposalStatus::NeedsArguments,
            ProposalStatus::ReadyForDryRun
        ) | (ProposalStatus::NeedsArguments, ProposalStatus::Rejected)
            | (ProposalStatus::NeedsArguments, ProposalStatus::Expired)
            | (
                ProposalStatus::ReadyForDryRun,
                ProposalStatus::DryRunRefused
            )
            | (
                ProposalStatus::ReadyForDryRun,
                ProposalStatus::ReadyForRatification
            )
            | (ProposalStatus::ReadyForDryRun, ProposalStatus::Expired)
            | (ProposalStatus::DryRunRefused, ProposalStatus::Rejected)
            | (ProposalStatus::DryRunRefused, ProposalStatus::Expired)
            | (
                ProposalStatus::ReadyForRatification,
                ProposalStatus::DryRunRefused
            )
            | (
                ProposalStatus::ReadyForRatification,
                ProposalStatus::Ratified
            )
            | (
                ProposalStatus::ReadyForRatification,
                ProposalStatus::Rejected
            )
            | (
                ProposalStatus::ReadyForRatification,
                ProposalStatus::Expired
            )
    )
}

fn status(value: u8) -> ProposalStatus {
    match value % 7 {
        0 => ProposalStatus::NeedsArguments,
        1 => ProposalStatus::ReadyForDryRun,
        2 => ProposalStatus::DryRunRefused,
        3 => ProposalStatus::ReadyForRatification,
        4 => ProposalStatus::Ratified,
        5 => ProposalStatus::Rejected,
        _ => ProposalStatus::Expired,
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let scenario = selector(data.first().copied().unwrap_or_default());
    let (dag, task) = graph();
    let graph_before = (dag.node_count(), dag.to_ir().unwrap().edge_count());
    let revision = "a".repeat(64);
    let unknown_focus = scenario == 7;
    let board = build_bpmn_semantic_board(
        &dag,
        (!unknown_focus).then_some((task, "review")),
        &revision,
        &PolicyFilter::default(),
    )
    .unwrap();
    let focus = if unknown_focus {
        DesignFocus::unknown(GraphElementRef::new("missing-focus").unwrap())
    } else {
        DesignFocus::element(GraphElementRef::new("review").unwrap())
    };
    let position = build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &"b".repeat(64),
        "fuzz-compiler-v1",
        &"c".repeat(64),
        focus,
        None,
    )
    .unwrap();
    let non_abstention = position
        .legal_moves()
        .iter()
        .filter(|legal_move| {
            legal_move.candidate_id().as_str()
                != semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
        })
        .take(3)
        .map(|legal_move| legal_move.move_id().clone())
        .collect::<Vec<_>>();
    let abstention = position
        .legal_moves()
        .iter()
        .find(|legal_move| {
            legal_move.candidate_id().as_str()
                == semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
        })
        .unwrap()
        .move_id()
        .clone();
    let mut scores = BTreeMap::new();
    let mut history = Vec::new();
    let utterance = match scenario {
        0 => {
            scores.insert(non_abstention[0].clone(), (0.90, 0.90, 1.0));
            "propose"
        }
        1 => {
            for (index, score) in [0.55, 0.54, 0.53].into_iter().enumerate() {
                scores.insert(non_abstention[index].clone(), (score, score, 0.0));
            }
            "clarify"
        }
        2 => {
            scores.insert(non_abstention[0].clone(), (0.90, 0.90, 0.0));
            "request arguments"
        }
        3 => {
            scores.insert(non_abstention[0].clone(), (0.40, 0.40, 0.0));
            "recover"
        }
        4 => {
            history.push(receipt(&position, 0, MoveAttemptOutcome::Applied));
            scores.insert(non_abstention[0].clone(), (0.30, 0.30, 0.0));
            "correct"
        }
        5 => {
            history.push(receipt(&position, 0, MoveAttemptOutcome::Inapplicable));
            scores.insert(abstention.clone(), (0.90, 0.90, 0.0));
            "explain"
        }
        6 => {
            history.extend(
                (0..3).map(|index| receipt(&position, index, MoveAttemptOutcome::Inapplicable)),
            );
            scores.insert(abstention.clone(), (0.90, 0.90, 0.0));
            "escalate"
        }
        7 => {
            scores.insert(non_abstention[0].clone(), (0.90, 0.90, 1.0));
            "change focus"
        }
        8 => {
            scores.insert(abstention.clone(), (0.90, 0.90, 0.0));
            "out of scope"
        }
        _ => {
            scores.insert(non_abstention[0].clone(), (0.90, 0.90, 1.0));
            scores.insert(non_abstention[1].clone(), (0.89, 0.89, 1.0));
            "insert before; insert after"
        }
    };
    let belief = DesignBelief::new(
        GAMEBOARD_SCHEMA_VERSION,
        position.state_id().clone(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        ProducerIdentity::new("fuzz.empty-belief.v1").unwrap(),
    )
    .unwrap();
    let evidence = evidence(&position, &scores);
    let disposition = decide_bpmn_game_disposition(
        &board,
        &position,
        &evidence,
        &belief,
        utterance,
        MoveAttemptId::new(format!("disposition-{scenario}")).unwrap(),
        &history,
        None,
    )
    .unwrap();
    disposition.validate_for_position(&position).unwrap();
    let kind_index = match disposition.kind() {
        GameDispositionKind::ProposeMove => 0,
        GameDispositionKind::ClarifyMoves => 1,
        GameDispositionKind::RequestMoveArguments => 2,
        GameDispositionKind::OfferRecoveryMoves => 3,
        GameDispositionKind::OfferCorrection => 4,
        GameDispositionKind::ExplainAttempt => 5,
        GameDispositionKind::Escalate => 6,
        GameDispositionKind::ChangeFocusOrContext => 7,
        GameDispositionKind::OutOfScope => 8,
        GameDispositionKind::CompoundPlan => 9,
    };
    observe(
        kind_index,
        "disposition",
        &format!("{:?}", disposition.kind()),
    );
    assert!(disposition.selected_moves().iter().all(|move_id| position
        .legal_moves()
        .iter()
        .any(|legal_move| legal_move.move_id() == move_id)));
    let encoded = serde_json::to_vec(&disposition).unwrap();
    let decoded: semantic_decision_contracts::GameDisposition =
        serde_json::from_slice(&encoded).unwrap();
    decoded.validate_for_position(&position).unwrap();

    let selected_move = non_abstention[0].clone();
    let mut workbook = ProposalWorkbook::new_position_bound(
        GAMEBOARD_SCHEMA_VERSION,
        WorkbookId::new(format!("workbook-{scenario}")).unwrap(),
        1,
        BoardHash::new(board.board_hash.as_str()).unwrap(),
        &position,
        selected_move,
        vec![WorkbookSlot {
            name: "value".into(),
            kind: ArgumentKind::Identifier,
            requirement: SlotRequirement::Required,
            value: SlotValueState::Missing,
            provenance: None,
            clarification_prompt: "governed fuzz value".into(),
        }],
        EvidenceRecordHash::new("d".repeat(64)).unwrap(),
    )
    .unwrap();
    workbook.validate_position(&position).unwrap();
    for byte in data.iter().copied().skip(1).take(64) {
        let operation = selector(byte);
        if operation == 0 && workbook.status() == ProposalStatus::NeedsArguments {
            let before = workbook.clone();
            let result = workbook.apply_answers(vec![(
                "value".into(),
                SlotValue::Identifier(format!("value_{byte}")),
            )]);
            assert!(result.is_ok());
            assert_ne!(workbook, before);
        } else {
            let before = workbook.clone();
            let from = workbook.status();
            let to = status(operation);
            let result = workbook.transition(to);
            assert_eq!(result.is_ok(), expected_transition(from, to));
            if result.is_err() {
                assert_eq!(workbook, before);
            }
        }
        workbook.validate_position(&position).unwrap();
        let round_trip: ProposalWorkbook =
            serde_json::from_slice(&serde_json::to_vec(&workbook).unwrap()).unwrap();
        assert_eq!(round_trip, workbook);
        assert_eq!(
            (dag.node_count(), dag.to_ir().unwrap().edge_count()),
            graph_before
        );
    }

    for (index, outcome) in [
        MoveAttemptOutcome::Applied,
        MoveAttemptOutcome::Incomplete,
        MoveAttemptOutcome::Ambiguous,
        MoveAttemptOutcome::Inapplicable,
        MoveAttemptOutcome::DisclosureSafeRefusal,
        MoveAttemptOutcome::Stale,
        MoveAttemptOutcome::CompilerRefused,
        MoveAttemptOutcome::RejectedByUser,
        MoveAttemptOutcome::Corrected,
        MoveAttemptOutcome::SystemFailure,
    ]
    .into_iter()
    .enumerate()
    {
        let correction = (outcome == MoveAttemptOutcome::Corrected)
            .then(|| MoveAttemptId::new("retained-original").unwrap());
        let attempt = record_bpmn_attempt(
            &position,
            MoveAttemptId::new(format!("outcome-{index}")).unwrap(),
            Some(non_abstention[0].clone()),
            "outcome tape",
            outcome,
            correction.clone(),
            correction.map(|_| CorrectionKind::FollowUp),
        )
        .unwrap();
        observe(10 + index as u8, "attempt_outcome", &format!("{outcome:?}"));
        assert_eq!(attempt.outcome(), outcome);
    }
    let projected = project_bpmn_attempt_history(&history).unwrap();
    assert_eq!(projected.1.len(), history.len());
    assert_eq!(
        (dag.node_count(), dag.to_ir().unwrap().edge_count()),
        graph_before
    );
});
