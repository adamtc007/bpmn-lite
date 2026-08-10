#![no_main]

use std::sync::atomic::{AtomicU16, Ordering};

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::{apply, Operation};
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    CorrectionKind, DesignFocus, GameDisposition, GraphElementRef, MoveAttemptId,
    MoveAttemptOutcome, MoveAttemptReceipt, ABSTENTION_CANDIDATE_ID,
};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, record_bpmn_attempt,
    render_bpmn_game_disposition,
};
use uuid::Uuid;

const MAX_INPUT_BYTES: usize = 256;
static OUTCOME_COUNTERS: AtomicU16 = AtomicU16::new(0);

fn observe(index: u8, label: &str) {
    let bit = 1_u16 << index;
    if OUTCOME_COUNTERS.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter outcome={label}");
    }
}

fn anchored_task() -> (DesignerDag, NodeKey) {
    let start = NodeKey(Uuid::from_u128(1));
    let task = NodeKey(Uuid::from_u128(2));
    let mut dag = DesignerDag::new("feedback-fuzz");
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

fn outcome(selector: u8) -> MoveAttemptOutcome {
    match selector % 10 {
        0 => MoveAttemptOutcome::Applied,
        1 => MoveAttemptOutcome::Incomplete,
        2 => MoveAttemptOutcome::Ambiguous,
        3 => MoveAttemptOutcome::Inapplicable,
        4 => MoveAttemptOutcome::DisclosureSafeRefusal,
        5 => MoveAttemptOutcome::Stale,
        6 => MoveAttemptOutcome::CompilerRefused,
        7 => MoveAttemptOutcome::RejectedByUser,
        8 => MoveAttemptOutcome::Corrected,
        _ => MoveAttemptOutcome::SystemFailure,
    }
}

/// Turn 32 fuzzed bytes into a well-formed-but-fuzzed lowercase hex digest,
/// distinct from any digest actually produced by the receipt's own content
/// hashing (which always hashes real struct fields, never raw fuzz bytes).
fn hostile_hex(data: &[u8]) -> String {
    let mut digest = String::with_capacity(64);
    for index in 0..64 {
        let byte = data.get(index % data.len().max(1)).copied().unwrap_or(0);
        digest.push(char::from_digit(((byte as u32) ^ (index as u32)) % 16, 16).unwrap());
    }
    digest
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 || data.len() > MAX_INPUT_BYTES {
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
    let attempted_move = position
        .legal_moves()
        .iter()
        .find(|legal_move| legal_move.candidate_id().as_str() != ABSTENTION_CANDIDATE_ID)
        .map(|legal_move| legal_move.move_id().clone());

    let outcome = outcome(data[0]);
    observe(data[0] % 10, &format!("{outcome:?}"));
    let (correction_of, correction_kind) = if outcome == MoveAttemptOutcome::Corrected {
        (
            Some(MoveAttemptId::new("retained-original").unwrap()),
            Some(CorrectionKind::FollowUp),
        )
    } else {
        (None, None)
    };
    let attempted_move = if data[1] % 2 == 0 {
        attempted_move
    } else {
        None
    };

    let receipt: MoveAttemptReceipt = record_bpmn_attempt(
        &position,
        MoveAttemptId::new("attempt-feedback-fuzz").unwrap(),
        attempted_move,
        "fuzz observed intent",
        outcome,
        correction_of,
        correction_kind,
    )
    .unwrap();

    if outcome == MoveAttemptOutcome::Applied {
        assert!(receipt.rule_explanations().is_empty());
        assert!(receipt.feedback_options().is_empty());
    } else {
        assert_eq!(receipt.rule_explanations().len(), 1);
        let explanation_id = &receipt.rule_explanations()[0];
        assert!(receipt
            .feedback_options()
            .iter()
            .all(|option| option.rule_explanation() == Some(explanation_id)));
    }

    // Round trip must be lossless.
    let encoded = serde_json::to_vec(&receipt).unwrap();
    let decoded: MoveAttemptReceipt = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, receipt);

    // Hostile explanation reference: tamper the encoded rule_explanations entry
    // with a fuzzed-but-well-formed digest. The receipt is content-addressed
    // (Deserialize recomputes and checks receipt_hash), so any such tamper must
    // be refused, never silently accepted and never panic.
    if !receipt.rule_explanations().is_empty() {
        let hostile = hostile_hex(&data[1..]);
        if hostile != receipt.rule_explanations()[0].as_str() {
            let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
            value["rule_explanations"][0] = serde_json::Value::String(hostile);
            let tampered = serde_json::to_vec(&value).unwrap();
            let result: Result<MoveAttemptReceipt, _> = serde_json::from_slice(&tampered);
            assert!(result.is_err());
        }
    }

    // Render must never leak a message unless a real, admitted explanation or
    // feedback source is present; Applied has none of either.
    let disposition = GameDisposition::explain_attempt(&position, receipt.clone()).unwrap();
    disposition.validate_for_position(&position).unwrap();
    let rendered = render_bpmn_game_disposition(&disposition, &board, &position);
    if outcome == MoveAttemptOutcome::Applied {
        assert!(rendered.is_err());
    } else {
        let message = rendered.unwrap();
        assert!(!message.trim().is_empty());
    }
});
