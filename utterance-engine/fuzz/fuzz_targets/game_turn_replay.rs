#![no_main]

use std::sync::atomic::{AtomicU8, Ordering};

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::{apply, Operation};
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    DesignBelief, DesignFocus, DesignPosition, DesignTurnId, EvidenceLane, FiniteScore,
    GameSessionId, GameTurnAnswer, GameTurnAnswerAbsenceReason, GameTurnAttempt,
    GameTurnCompilerResult, GameTurnRecord, GraphContentHash, GraphDeltaHash, GraphElementRef,
    LaneScore, LegalMoveId, MoveAttemptId, MoveAttemptOutcome, MoveEvidence, ProducerIdentity,
    SemanticDecisionBoard, ABSTENTION_CANDIDATE_ID, GAMEBOARD_SCHEMA_VERSION,
};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, capture_bpmn_game_turn,
    decide_bpmn_game_disposition,
};
use uuid::Uuid;

const MAX_TURNS: usize = 8;
static AXIS_COUNTERS: AtomicU8 = AtomicU8::new(0);

fn observe(index: u8, label: &str) {
    let bit = 1_u8 << index;
    if AXIS_COUNTERS.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter hostile_axis={label}");
    }
}

fn anchored_task() -> (DesignerDag, NodeKey) {
    let start = NodeKey(Uuid::from_u128(1));
    let task = NodeKey(Uuid::from_u128(2));
    let mut dag = DesignerDag::new("game-turn-replay-fuzz");
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

fn hex64(byte: u8, salt: u8) -> String {
    format!("{:02x}", byte ^ salt).repeat(32)
}

fn scored_evidence(
    position: &DesignPosition,
    top: &LegalMoveId,
    propose_mode: bool,
) -> Vec<MoveEvidence> {
    position
        .legal_moves()
        .iter()
        .map(|legal_move| {
            let is_top = legal_move.move_id() == top;
            let score = if propose_mode {
                if is_top { 0.95 } else { 0.01 }
            } else if is_top {
                0.55
            } else {
                0.05
            };
            // Every legal move at this anchor has at least one required,
            // unbound argument (`legal_move_enumeration`'s own invariant), so
            // routing to `ProposeMove` in propose_mode needs strong typed-
            // argument evidence, not just a high overall score - otherwise
            // the disposition always falls through to `RequestMoveArguments`
            // and `attempt.receipt()` is never `None`.
            let lanes = if propose_mode && is_top {
                vec![LaneScore {
                    lane: EvidenceLane::TypedArgument,
                    score: FiniteScore::new(1.0).unwrap(),
                }]
            } else {
                Vec::new()
            };
            MoveEvidence::new(
                GAMEBOARD_SCHEMA_VERSION,
                legal_move.move_id().clone(),
                lanes,
                FiniteScore::new(score).unwrap(),
                FiniteScore::new(score).unwrap(),
                Vec::new(),
                ProducerIdentity::new("fuzz.turn-evidence.v1").unwrap(),
            )
            .unwrap()
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_turn(
    board: &SemanticDecisionBoard,
    position: &DesignPosition,
    belief: &DesignBelief,
    top: &LegalMoveId,
    propose_mode: bool,
    index: usize,
    session_id: &GameSessionId,
    sequence: u64,
) -> (GameTurnAttempt, Result<GameTurnRecord, utterance_engine::bpmn_board::BpmnBoardError>) {
    let evidence = scored_evidence(position, top, propose_mode);
    let disposition = decide_bpmn_game_disposition(
        board,
        position,
        &evidence,
        belief,
        "fuzz observed intent, no compound syntax",
        MoveAttemptId::new(format!("attempt-turn-{index}")).unwrap(),
        &[],
        None,
    )
    .unwrap();
    disposition.validate_for_position(position).unwrap();
    let attempt = match disposition.attempt_receipt() {
        Some(receipt) => GameTurnAttempt::terminal(receipt.clone()),
        None => GameTurnAttempt::not_attempted(),
    };
    let turn_id = DesignTurnId::new(format!("turn-{index}")).unwrap();
    let record = capture_bpmn_game_turn(
        session_id.clone(),
        turn_id,
        sequence,
        1_700_000_000_000 + sequence,
        board,
        position.clone(),
        evidence,
        belief.clone(),
        disposition,
        "fuzz observed intent, no compound syntax",
        GameTurnAnswer::not_observed(GameTurnAnswerAbsenceReason::NotRequested),
        None,
        None,
        attempt.clone(),
        GameTurnCompilerResult::not_requested(),
        Vec::new(),
    );
    (attempt, record)
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
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
    let top = position
        .legal_moves()
        .iter()
        .find(|legal_move| legal_move.candidate_id().as_str() != ABSTENTION_CANDIDATE_ID)
        .map(|legal_move| legal_move.move_id().clone())
        .unwrap();
    let belief = DesignBelief::new(
        GAMEBOARD_SCHEMA_VERSION,
        position.state_id().clone(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        ProducerIdentity::new("fuzz.empty-belief.v1").unwrap(),
    )
    .unwrap();
    let session_id = GameSessionId::new("session-fuzz").unwrap();

    let steps = data.len().min(MAX_TURNS);
    for (index, &byte) in data.iter().take(steps).enumerate() {
        let sequence = (index + 1) as u64;
        let propose_mode = byte % 2 == 0;

        // Build the always-valid base turn once, so hostile axes below can
        // reuse its `attempt` without re-deriving the disposition.
        let (attempt, base_record) = build_turn(
            &board,
            &position,
            &belief,
            &top,
            propose_mode,
            index,
            &session_id,
            sequence,
        );
        let base_record = base_record.expect("the unmodified base turn is always admissible");
        assert_eq!(base_record.sequence(), sequence);

        // Determinism: re-deriving the identical base turn must produce an
        // identical content-addressed record.
        let (_, repeat_record) = build_turn(
            &board,
            &position,
            &belief,
            &top,
            propose_mode,
            index,
            &session_id,
            sequence,
        );
        assert_eq!(
            repeat_record.unwrap().record_hash(),
            base_record.record_hash()
        );

        let encoded = serde_json::to_vec(&base_record).unwrap();
        let decoded: GameTurnRecord = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, base_record);

        // Single-axis hostile mutation: reuse the same `attempt`/disposition
        // but corrupt exactly one cross-field relationship that
        // `GameTurnRecord::new` is documented to check, and prove it fails
        // closed rather than silently admitting an inconsistent turn.
        let axis = byte / 2 % 4;
        let turn_id = DesignTurnId::new(format!("turn-{index}-hostile")).unwrap();
        let evidence = scored_evidence(&position, &top, propose_mode);
        let disposition = decide_bpmn_game_disposition(
            &board,
            &position,
            &evidence,
            &belief,
            "fuzz observed intent, no compound syntax",
            // Same attempt id as `build_turn`'s base disposition above: this
            // regenerated disposition must name the exact same terminal
            // attempt as the reused `attempt` value, or the "axis 0 / no
            // mutation" case would itself trip GameTurnRecord::new's
            // "disposition and turn name different terminal attempts" check
            // for a reason unrelated to the axis under test.
            MoveAttemptId::new(format!("attempt-turn-{index}")).unwrap(),
            &[],
            None,
        )
        .unwrap();
        let (chosen_move, compiler_result, applicable) = match axis {
            1 => {
                observe(1, "chosen_move_off_board");
                (
                    Some(LegalMoveId::new(hex64(byte, 0xAA)).unwrap()),
                    GameTurnCompilerResult::not_requested(),
                    true,
                )
            }
            2 if attempt.receipt().is_none() => {
                observe(2, "admitted_without_terminal_attempt");
                (
                    None,
                    GameTurnCompilerResult::admitted(
                        GraphDeltaHash::new(hex64(byte, 0x11)).unwrap(),
                        GraphContentHash::new(hex64(byte, 0x22)).unwrap(),
                        GraphContentHash::new(hex64(byte, 0x33)).unwrap(),
                    )
                    .unwrap(),
                    true,
                )
            }
            3 if attempt
                .receipt()
                .is_some_and(|receipt| receipt.outcome() != MoveAttemptOutcome::CompilerRefused) =>
            {
                observe(3, "refused_outcome_mismatch");
                (
                    None,
                    GameTurnCompilerResult::refused(
                        GraphContentHash::new(hex64(byte, 0x44)).unwrap(),
                    ),
                    true,
                )
            }
            _ => {
                observe(0, "none");
                (None, GameTurnCompilerResult::not_requested(), false)
            }
        };

        let hostile_result = capture_bpmn_game_turn(
            session_id.clone(),
            turn_id,
            sequence,
            1_700_000_000_000 + sequence,
            &board,
            position.clone(),
            evidence,
            belief.clone(),
            disposition,
            "fuzz observed intent, no compound syntax",
            GameTurnAnswer::not_observed(GameTurnAnswerAbsenceReason::NotRequested),
            chosen_move,
            None,
            attempt,
            compiler_result,
            Vec::new(),
        );
        if applicable {
            assert!(
                hostile_result.is_err(),
                "hostile axis {axis} was wrongly admitted at turn {index}"
            );
        } else {
            hostile_result.expect("axis 0 (no mutation) is always admissible");
        }
    }
});
