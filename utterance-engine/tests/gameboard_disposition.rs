use std::collections::BTreeMap;

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::{apply, Operation};
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use semantic_decision_contracts::{
    DesignBelief, DesignFocus, EvidenceLane, FiniteScore, GameClarificationDimension,
    GameDispositionKind, GraphElementRef, LaneScore, MoveAttemptId, MoveEvidence, ProducerIdentity,
    GAMEBOARD_SCHEMA_VERSION,
};
use serde::Deserialize;
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, decide_bpmn_game_disposition,
};
use uuid::Uuid;

#[derive(Deserialize)]
struct Fixture {
    fixture_id: String,
    ranking: Vec<RankedFixture>,
    gold_candidate_id: String,
    gold_rank: usize,
    expected_disposition: String,
    expected_dimension: String,
}

#[derive(Deserialize)]
struct RankedFixture {
    candidate_id: String,
    score: f64,
}

fn graph() -> (DesignerDag, NodeKey) {
    let start = NodeKey(Uuid::from_u128(101));
    let task = NodeKey(Uuid::from_u128(102));
    let end = NodeKey(Uuid::from_u128(103));
    let mut dag = DesignerDag::new("gameboard-top3-fixture");
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

#[test]
fn gold_third_ranked_move_is_surfaced_by_one_governed_clarification() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/gameboard-top3.json")).unwrap();
    assert_eq!(fixture.fixture_id, "bpmn-mid-sequence-third-ranked-v1");
    assert_eq!(fixture.gold_rank, 3);
    assert_eq!(fixture.expected_disposition, "clarify_moves");
    assert_eq!(fixture.expected_dimension, "argument");

    let (dag, task) = graph();
    let revision = "a".repeat(64);
    let board = build_bpmn_semantic_board(
        &dag,
        Some((task, "review")),
        &revision,
        &PolicyFilter::default(),
    )
    .unwrap();
    let position = build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &"b".repeat(64),
        "fixture-compiler-v1",
        &"c".repeat(64),
        DesignFocus::element(GraphElementRef::new("review").unwrap()),
        None,
    )
    .unwrap();
    let configured = fixture
        .ranking
        .iter()
        .map(|ranked| (ranked.candidate_id.as_str(), ranked.score))
        .collect::<BTreeMap<_, _>>();
    let evidence = position
        .legal_moves()
        .iter()
        .map(|legal_move| {
            let score = configured
                .get(legal_move.candidate_id().as_str())
                .copied()
                .unwrap_or(0.01);
            MoveEvidence::new(
                GAMEBOARD_SCHEMA_VERSION,
                legal_move.move_id().clone(),
                vec![LaneScore {
                    lane: EvidenceLane::TypedArgument,
                    score: FiniteScore::new(0.0).unwrap(),
                }],
                FiniteScore::new(score).unwrap(),
                FiniteScore::new(score).unwrap(),
                Vec::new(),
                ProducerIdentity::new("fixture.controlled-evidence.v1").unwrap(),
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
        ProducerIdentity::new("fixture.empty-belief.v1").unwrap(),
    )
    .unwrap();
    let disposition = decide_bpmn_game_disposition(
        &board,
        &position,
        &evidence,
        &belief,
        "fixture ambiguous request",
        MoveAttemptId::new("fixture-attempt").unwrap(),
        &[],
    )
    .unwrap();

    assert_eq!(disposition.kind(), GameDispositionKind::ClarifyMoves);
    assert_eq!(
        disposition.clarification_dimension(),
        Some(GameClarificationDimension::Argument)
    );
    assert_eq!(disposition.selected_moves().len(), 3);
    let selected_candidates = disposition
        .selected_moves()
        .iter()
        .map(|move_id| {
            position
                .legal_moves()
                .iter()
                .find(|legal_move| legal_move.move_id() == move_id)
                .unwrap()
                .candidate_id()
                .as_str()
        })
        .collect::<Vec<_>>();
    assert!(selected_candidates.contains(&fixture.gold_candidate_id.as_str()));
    assert!(disposition.governed_prompt().is_some());
    assert!(disposition
        .attempt_receipt()
        .is_some_and(|receipt| receipt.outcome()
            == semantic_decision_contracts::MoveAttemptOutcome::Ambiguous));
}
