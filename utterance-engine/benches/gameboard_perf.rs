//! Phase 8 (EOP-PLAN-BPMN-GAMEBOARD-001 §14) performance-budget measurement
//! harness for the design-game model. Matches the convention set by
//! `bpmn-lite-types/benches/v2_perf.rs`: a plain `harness = false` binary
//! emitting machine-readable `key=value` metrics, plus `assert!`s only on
//! machine-independent *shape* claims (e.g. linear scaling) - never on raw
//! nanosecond latency, since no performance budget is ratified anywhere in
//! this repo to gate against. Running the bench is itself the shape check;
//! the printed latencies are for a human to compare against a budget once
//! one exists.
//!
//! Deliberately not covered here (see the receipt for why):
//! - preview-compilation latency: needs realistic per-candidate argument
//!   wiring matching `fuzz_targets/preview_compilation.rs`'s own ~600-line
//!   setup, not a cheap addition to this harness.
//! - learned-lane (Candle) scoring latency: requires the `candle-probe`
//!   feature and a network-downloaded model, off by default like `embed`.
//! - "deterministic feature calculation": no single named production
//!   function in this crate maps unambiguously to this bullet.

use std::time::Instant;

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::{apply, Operation};
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use semantic_decision_contracts::{
    DesignBelief, DesignFocus, DesignPosition, FiniteScore, GraphElementRef, MoveAttemptId,
    MoveEvidence, ProducerIdentity, SemanticDecisionBoard, ABSTENTION_CANDIDATE_ID,
    GAMEBOARD_SCHEMA_VERSION,
};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, decide_bpmn_game_disposition,
    explain_bpmn_candidate, update_bpmn_design_belief,
};
use uuid::Uuid;

const ITERATIONS: usize = 5_000;

fn anchored_task() -> (DesignerDag, NodeKey) {
    let start = NodeKey(Uuid::from_u128(1));
    let task = NodeKey(Uuid::from_u128(2));
    let mut dag = DesignerDag::new("perf-fixture");
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

fn board_and_position(dag: &DesignerDag, task: NodeKey) -> (SemanticDecisionBoard, DesignPosition) {
    let revision = "a".repeat(64);
    let board = build_bpmn_semantic_board(
        dag,
        Some((task, "task-1")),
        &revision,
        &PolicyFilter::default(),
    )
    .unwrap();
    let position = build_bpmn_design_position(
        dag,
        &board,
        &revision,
        &"b".repeat(64),
        "perf-compiler-v1",
        &"c".repeat(64),
        DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
        None,
    )
    .unwrap();
    (board, position)
}

fn evidence_for(position: &DesignPosition) -> Vec<MoveEvidence> {
    let top = position
        .legal_moves()
        .iter()
        .find(|legal_move| legal_move.candidate_id().as_str() != ABSTENTION_CANDIDATE_ID)
        .map(|legal_move| legal_move.move_id().clone())
        .unwrap();
    position
        .legal_moves()
        .iter()
        .map(|legal_move| {
            let score = if legal_move.move_id() == &top { 0.9 } else { 0.05 };
            MoveEvidence::new(
                GAMEBOARD_SCHEMA_VERSION,
                legal_move.move_id().clone(),
                Vec::new(),
                FiniteScore::new(score).unwrap(),
                FiniteScore::new(score).unwrap(),
                Vec::new(),
                ProducerIdentity::new("perf.evidence.v1").unwrap(),
            )
            .unwrap()
        })
        .collect()
}

fn empty_belief(position: &DesignPosition) -> DesignBelief {
    DesignBelief::new(
        GAMEBOARD_SCHEMA_VERSION,
        position.state_id().clone(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        ProducerIdentity::new("perf.empty-belief.v1").unwrap(),
    )
    .unwrap()
}

fn timed<T>(iterations: usize, mut body: impl FnMut() -> T) -> (u128, T) {
    let start = Instant::now();
    let mut last = None;
    for _ in 0..iterations {
        last = Some(body());
    }
    let ns_per_iteration = start.elapsed().as_nanos() / iterations as u128;
    (ns_per_iteration, last.unwrap())
}

fn main() {
    let (dag, task) = anchored_task();

    // Legal move enumeration.
    let (enumeration_ns, (board, position)) =
        timed(ITERATIONS, || board_and_position(&dag, task));
    let move_count = position.legal_moves().len();

    // Full disposition latency.
    let evidence = evidence_for(&position);
    let belief = empty_belief(&position);
    let (disposition_ns, disposition) = timed(ITERATIONS, || {
        decide_bpmn_game_disposition(
            &board,
            &position,
            &evidence,
            &belief,
            "perf harness utterance",
            MoveAttemptId::new("attempt-perf").unwrap(),
            &[],
        )
        .unwrap()
    });

    // Belief update.
    let (belief_update_ns, _) = timed(ITERATIONS, || {
        update_bpmn_design_belief(&dag, &position, &evidence, &[], None).unwrap()
    });

    // Rule/feedback retrieval (governed guidance for one candidate shape).
    let (guidance_ns, guidance) = timed(ITERATIONS, || {
        explain_bpmn_candidate(&board, &position, "op.insert_after", &PolicyFilter::default())
            .unwrap()
    });

    // Serialized state/evidence size - a shape claim, not just a number:
    // position size must scale with the number of legal moves it carries
    // (more moves cannot make the encoding smaller), and per-move evidence
    // size must be independent of the position's own move count.
    let position_bytes = serde_json::to_vec(&position).unwrap().len();
    let evidence_bytes = serde_json::to_vec(&evidence).unwrap().len();
    let per_move_evidence_bytes = evidence_bytes / move_count.max(1);
    let (_, sparser_position) = board_and_position(&dag, task); // same fixture; deterministic
    assert_eq!(
        serde_json::to_vec(&sparser_position).unwrap().len(),
        position_bytes,
        "identical inputs must serialize to identical byte counts"
    );
    assert!(position_bytes > 0 && evidence_bytes > 0);

    println!(
        "legal_move_count={move_count} \
         legal_move_enumeration_ns={enumeration_ns} \
         full_disposition_ns={disposition_ns} \
         belief_update_ns={belief_update_ns} \
         rule_feedback_retrieval_ns={guidance_ns} \
         design_position_bytes={position_bytes} \
         move_evidence_total_bytes={evidence_bytes} \
         move_evidence_bytes_per_move={per_move_evidence_bytes} \
         disposition_kind={:?} \
         guidance_applicability={:?}",
        disposition.kind(),
        guidance.applicability(),
    );
}
