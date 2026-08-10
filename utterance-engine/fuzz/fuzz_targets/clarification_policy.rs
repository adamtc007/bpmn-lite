#![no_main]

use std::sync::atomic::{AtomicU8, Ordering};

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::{apply, Operation};
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    DesignBelief, DesignFocus, FiniteScore, GameClarificationDimension, GameDispositionKind,
    GraphElementRef, MoveAttemptId, MoveAttemptOutcome, MoveEvidence, ProducerIdentity,
    ABSTENTION_CANDIDATE_ID, GAMEBOARD_SCHEMA_VERSION,
};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, decide_bpmn_game_disposition,
};
use uuid::Uuid;

const MAX_INPUT_BYTES: usize = 256;
static KIND_COUNTERS: AtomicU8 = AtomicU8::new(0);
static DIMENSION_COUNTERS: AtomicU8 = AtomicU8::new(0);

fn observe(counters: &AtomicU8, index: u8, family: &str, label: &str) {
    let bit = 1_u8 << index;
    if counters.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter {family}={label}");
    }
}

/// Anchored task with many admitted candidates (mirrors `anchored_game()` in
/// `bpmn_board.rs`'s own test module), so contrastive `negative_contrasts` pairs
/// exist for the clarification policy's move dimension to select from.
fn anchored_task() -> (DesignerDag, NodeKey) {
    let start = NodeKey(Uuid::from_u128(1));
    let task = NodeKey(Uuid::from_u128(2));
    let mut dag = DesignerDag::new("clarification-fuzz");
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
                id: "task-1".into(),
                name: "Review".into(),
                task_type: "review".into(),
            },
            edge_id: "flow-1".into(),
        },
        Provenance::default(),
    )
    .unwrap()
    .candidate;
    (dag, task)
}

fn unit_score(byte: u8) -> f64 {
    // Bias toward the [0.30, 0.62) band: below PROPOSAL_SCORE_FLOOR but usually
    // above WEAK_EVIDENCE_FLOOR, where clarification actually has a chance to
    // fire, while still letting the fuzzer explore the full unit range.
    0.30 + (byte as f64 / 255.0) * 0.40
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() || data.len() > MAX_INPUT_BYTES {
        return;
    }
    let (dag, task) = anchored_task();
    let revision = "a".repeat(64);
    let board = build_bpmn_semantic_board(
        &dag,
        Some((task, "task-1")),
        &revision,
        &PolicyFilter::default(),
    )
    .unwrap();
    let position = build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &"b".repeat(64),
        "fuzz-compiler-v1",
        &"c".repeat(64),
        DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
        None,
    )
    .unwrap();
    let non_abstention = position
        .legal_moves()
        .iter()
        .filter(|legal_move| legal_move.candidate_id().as_str() != ABSTENTION_CANDIDATE_ID)
        .map(|legal_move| legal_move.move_id().clone())
        .collect::<Vec<_>>();
    if non_abstention.len() < 3 {
        return;
    }
    let active = 2 + (data[0] as usize % 2); // 2 or 3 scored alternatives
    let scores = data
        .iter()
        .skip(1)
        .take(active)
        .copied()
        .map(unit_score)
        .collect::<Vec<_>>();
    if scores.len() < active {
        return;
    }
    let evidence = position
        .legal_moves()
        .iter()
        .map(|legal_move| {
            let score = non_abstention
                .iter()
                .position(|move_id| move_id == legal_move.move_id())
                .filter(|index| *index < active)
                .map_or(0.01, |index| scores[index]);
            MoveEvidence::new(
                GAMEBOARD_SCHEMA_VERSION,
                legal_move.move_id().clone(),
                Vec::new(),
                FiniteScore::new(score).unwrap(),
                FiniteScore::new(score).unwrap(),
                Vec::new(),
                ProducerIdentity::new("fuzz.clarification-evidence.v1").unwrap(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let belief = DesignBelief::new(
        GAMEBOARD_SCHEMA_VERSION,
        position.state_id().clone(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        ProducerIdentity::new("fuzz.empty-belief.v1").unwrap(),
    )
    .unwrap();
    let disposition = decide_bpmn_game_disposition(
        &board,
        &position,
        &evidence,
        &belief,
        "fuzz utterance no compound syntax",
        MoveAttemptId::new("attempt-clarification-fuzz").unwrap(),
        &[],
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
        &KIND_COUNTERS,
        kind_index,
        "disposition_kind",
        &format!("{:?}", disposition.kind()),
    );

    if disposition.kind() == GameDispositionKind::ClarifyMoves {
        assert!((2..=3).contains(&disposition.selected_moves().len()));
        assert!(disposition.selected_moves().iter().all(|move_id| {
            position
                .legal_moves()
                .iter()
                .any(|legal_move| legal_move.move_id() == move_id
                    && legal_move.candidate_id().as_str() != ABSTENTION_CANDIDATE_ID)
        }));
        assert!(disposition
            .governed_prompt()
            .is_some_and(|prompt| !prompt.trim().is_empty()));
        assert_eq!(
            disposition
                .attempt_receipt()
                .map(|receipt| receipt.outcome()),
            Some(MoveAttemptOutcome::Ambiguous)
        );
        let dimension_index = match disposition.clarification_dimension() {
            Some(GameClarificationDimension::Move) => Some(0),
            Some(GameClarificationDimension::Focus) => Some(1),
            Some(GameClarificationDimension::Argument) => Some(2),
            None => None,
        };
        if let Some(index) = dimension_index {
            observe(
                &DIMENSION_COUNTERS,
                index,
                "clarification_dimension",
                &format!("{:?}", disposition.clarification_dimension()),
            );
        }
    }

    let encoded = serde_json::to_vec(&disposition).unwrap();
    let decoded: semantic_decision_contracts::GameDisposition =
        serde_json::from_slice(&encoded).unwrap();
    decoded.validate_for_position(&position).unwrap();
    assert_eq!(decoded, disposition);
});
