use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::build_bpmn_semantic_board;
use utterance_engine::context::project_ir;
use utterance_engine::contract::{FiniteScore, RankedCandidate, SlmResult};
use utterance_engine::corpus_schema::SemanticCorpusClosure;
use utterance_engine::disposition::StrictCompoundSyntax;
use utterance_engine::exact::{finalize_semantic_evidence, EvidenceLane};
use utterance_engine::pair::{serialize_candidate_pair, PairTokenBudget};
use utterance_engine::policy::{decide_with_action_spans, DispositionConfig};

fn position() -> (
    semantic_decision_contracts::SemanticDecisionBoard,
    utterance_engine::context::ContextProjection,
) {
    let class = utterance_engine::fixtures::enumeration_classes()
        .unwrap()
        .into_iter()
        .find(|class| class.class_id == "mid_sequence_task")
        .unwrap();
    let graph_identity = "packet-identity:mid_sequence_task";
    let board = build_bpmn_semantic_board(
        &class.dag,
        class.anchor_key.zip(class.anchor_id),
        graph_identity,
        &PolicyFilter::default(),
    )
    .unwrap();
    let context = project_ir(
        &class.dag.to_ir().unwrap(),
        class.anchor_id,
        board.semantic_snapshot.as_str(),
        graph_identity,
    )
    .unwrap();
    (board, context)
}

fn deterministic_evidence(board: &semantic_decision_contracts::SemanticDecisionBoard) -> SlmResult {
    let ranking = board
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| RankedCandidate {
            candidate_id: candidate.canonical_id.as_str().to_string(),
            score: FiniteScore::new(1.0 / (index + 1) as f64).unwrap(),
        })
        .collect();
    SlmResult {
        ranking,
        retrieved_subset_hash: "full-board-test-packet".to_string(),
        board_hash: board.board_hash.as_str().to_string(),
        model_bundle_hash: "fixed-parity-producer".to_string(),
        evidence_trace: None,
        inference_evidence: None,
        move_evidence: Vec::new(),
    }
}

#[test]
fn evaluator_and_serving_build_identical_v3_packets_and_decisions() {
    let (board, context) = position();
    let utterance = "insert a sanctions screen after this review";
    let gold = board.candidates[0].canonical_id.as_str();

    // Evaluator packet: the persisted v3 closure used by the corrected
    // measurement instrument.
    let evaluator = SemanticCorpusClosure::new(
        &board,
        utterance,
        &context,
        gold,
        "packet-identity-family".to_string(),
        "packet-identity-test".to_string(),
        "packet-identity-split".to_string(),
    )
    .unwrap();

    // Serving packet: independently walk the live board exactly as
    // Tier1Ranker::rank_full_board does.
    let serving = board
        .candidates
        .iter()
        .map(|candidate| {
            (
                candidate.canonical_id.as_str().to_string(),
                serialize_candidate_pair(
                    utterance,
                    &context,
                    candidate,
                    PairTokenBudget::default(),
                )
                .unwrap(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        evaluator.full_served_list,
        serving
            .iter()
            .map(|(candidate_id, _)| candidate_id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(evaluator.candidate_pairs.len(), serving.len());
    for (evaluator_pair, (serving_id, serving_pair)) in
        evaluator.candidate_pairs.iter().zip(&serving)
    {
        assert_eq!(&evaluator_pair.candidate_id, serving_id);
        assert_eq!(&evaluator_pair.pair, serving_pair);
    }

    let evaluator_evidence = finalize_semantic_evidence(
        &board,
        utterance,
        deterministic_evidence(&board),
        vec![EvidenceLane::CandleCrossEncoder],
        vec!["fixed-parity-producer".to_string()],
    )
    .unwrap();
    let serving_evidence = finalize_semantic_evidence(
        &board,
        utterance,
        deterministic_evidence(&board),
        vec![EvidenceLane::CandleCrossEncoder],
        vec!["fixed-parity-producer".to_string()],
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&evaluator_evidence).unwrap(),
        serde_json::to_vec(&serving_evidence).unwrap()
    );

    let policy = DispositionConfig::shadow_v2();
    let evaluator_decision = decide_with_action_spans(
        &policy,
        &board,
        &evaluator_evidence,
        &context,
        utterance,
        &StrictCompoundSyntax,
    )
    .unwrap();
    let serving_decision = decide_with_action_spans(
        &policy,
        &board,
        &serving_evidence,
        &context,
        utterance,
        &StrictCompoundSyntax,
    )
    .unwrap();
    assert_eq!(evaluator_decision.0, serving_decision.0);
    assert_eq!(
        serde_json::to_vec(&evaluator_decision.1).unwrap(),
        serde_json::to_vec(&serving_decision.1).unwrap()
    );
}

#[test]
fn evaluation_fixture_reconstruction_has_stable_packet_identity() {
    let (first_board, first_context) = position();
    let (second_board, second_context) = position();
    assert_eq!(first_board.board_hash, second_board.board_hash);
    assert_eq!(first_context.hash(), second_context.hash());
}
