use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::{apply, Operation};
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use semantic_decision_contracts::{
    DesignBelief, DesignFocus, EvidenceLane, FiniteScore, GameClarificationDimension,
    GameDispositionKind, GraphElementRef, LaneScore, MoveAttemptId, MoveAttemptOutcome,
    MoveAttemptReceipt, MoveEvidence, ProducerIdentity, GAMEBOARD_SCHEMA_VERSION,
};
use serde::Deserialize;
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, decide_bpmn_game_disposition,
    update_bpmn_design_belief,
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
                loop_origin: None,
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
        None,
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

/// Gate 8 bullet 5: "Expected wrong-move traffic cannot cause unbounded
/// history, feedback recursion or repeated compiler work." The pre-existing
/// coverage only proved the storage-layer bound (`MAX_HISTORY_ATTEMPTS`/
/// `MAX_HISTORY_BYTES` reject an over-window `project()` call). This drives
/// realistic repeated wrong-move traffic — every turn is deliberately
/// ambiguous, never applies — through the real disposition/belief loop
/// end-to-end, mirroring the same bounded-window discipline
/// `design_history_projection` (rest.rs) applies in production, and proves
/// per-turn cost stays flat rather than growing with total historical
/// attempt count.
#[test]
fn repeated_wrong_move_traffic_keeps_the_disposition_loop_bounded() {
    const MAX_WINDOW: usize = 64;
    const TURNS: usize = 400;

    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/gameboard-top3.json")).unwrap();
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

    // Full history ever produced, growing without bound across turns — this
    // is what an unbounded caller would accumulate. What is actually passed
    // into each turn's belief/disposition call is the trailing MAX_WINDOW
    // slice, matching production windowing discipline.
    let mut full_tape: Vec<MoveAttemptReceipt> = Vec::new();
    let mut durations = Vec::with_capacity(TURNS);
    let mut previous_belief: Option<DesignBelief> = None;
    let mut escalated = false;

    for turn in 0..TURNS {
        let window_start = full_tape.len().saturating_sub(MAX_WINDOW);
        let window = &full_tape[window_start..];
        assert!(window.len() <= MAX_WINDOW);

        let started = Instant::now();
        let belief = update_bpmn_design_belief(
            &dag,
            &position,
            &evidence,
            window,
            previous_belief.as_ref(),
        )
        .unwrap();
        let disposition = decide_bpmn_game_disposition(
            &board,
            &position,
            &evidence,
            &belief,
            "fixture ambiguous request",
            MoveAttemptId::new(format!("wrong-move-{turn}")).unwrap(),
            window,
            None,
        )
        .unwrap();
        durations.push(started.elapsed());

        // Real, deliberate finding: the disposition policy is itself a loop
        // breaker. Early turns clarify; once repeated identical failure is
        // visible in the window, the policy escalates instead of clarifying
        // forever. Both are legitimate "did not apply" dispositions — what
        // must never happen is a silent apply on a fixture engineered to be
        // ambiguous on every turn.
        assert!(matches!(
            disposition.kind(),
            GameDispositionKind::ClarifyMoves | GameDispositionKind::Escalate
        ));
        if disposition.kind() == GameDispositionKind::Escalate {
            escalated = true;
        }
        let receipt = disposition
            .attempt_receipt()
            .expect("every non-apply disposition carries a terminal receipt");
        assert_ne!(receipt.outcome(), MoveAttemptOutcome::Applied);

        full_tape.push(receipt.clone());
        previous_belief = Some(belief);
    }

    assert_eq!(full_tape.len(), TURNS, "history is never silently dropped");
    assert!(
        escalated,
        "repeated identical wrong-move traffic never triggered the escalation \
         loop breaker within {TURNS} turns — either the fixture stopped being \
         ambiguous or the repeated-failure policy regressed"
    );

    // No growth: comparing the first and last quarter's total wall-clock
    // catches an O(total turns) or worse cost creeping in despite the
    // bounded window — e.g. a caller-side Vec that is filtered instead of
    // sliced, or a belief/disposition path that secretly reaches past the
    // window into `full_tape`. A generous multiplier absorbs run-to-run
    // noise; the point is "flat", not "identical".
    let quarter = TURNS / 4;
    let first_quarter: Duration = durations[..quarter].iter().sum();
    let last_quarter: Duration = durations[TURNS - quarter..].iter().sum();
    assert!(
        last_quarter <= first_quarter * 5 + Duration::from_millis(50),
        "disposition loop cost grew with total historical attempt count: \
         first quarter {first_quarter:?}, last quarter {last_quarter:?}"
    );
}
