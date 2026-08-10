#![no_main]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::{apply, Operation};
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    CorrectionKind, DesignFocus, GraphElementRef, MoveAttemptId, MoveAttemptOutcome,
    MoveAttemptReceipt, ABSTENTION_CANDIDATE_ID,
};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, project_bpmn_attempt_history,
    record_bpmn_attempt,
};
use uuid::Uuid;

const MAX_STEPS: usize = 24;
static SCHEME_COUNTERS: AtomicU8 = AtomicU8::new(0);

fn observe(index: u8, label: &str) {
    let bit = 1_u8 << index;
    if SCHEME_COUNTERS.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter correction_scheme={label}");
    }
}

fn anchored_task() -> (DesignerDag, NodeKey) {
    let start = NodeKey(Uuid::from_u128(1));
    let task = NodeKey(Uuid::from_u128(2));
    let mut dag = DesignerDag::new("correction-history-fuzz");
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

fn correction_kind(selector: u8) -> CorrectionKind {
    match selector % 3 {
        0 => CorrectionKind::Undo,
        1 => CorrectionKind::Replacement,
        _ => CorrectionKind::FollowUp,
    }
}

fn attempt_id(index: usize) -> MoveAttemptId {
    MoveAttemptId::new(format!("attempt-{index}")).unwrap()
}

/// Compact, independent reference model for the same abstract invariant
/// `validate_attempt_history` enforces: every receipt's correction chain
/// (if any) must resolve to a real attempt somewhere in the *current* tape
/// and never revisit a receipt. `validate_attempt_history` is a pure
/// function of the whole current slice each call - it does not care about
/// append order, only about what exists *now* - so this model recomputes
/// from scratch every time rather than tracking incremental state. Walking
/// bounded by `len + 1` steps (a receipt cannot legally chain through more
/// distinct receipts than exist) is an independent way to express "acyclic
/// and resolvable," not a copy of the production traversal.
fn reference_valid(attempts: &[MoveAttemptReceipt]) -> bool {
    let by_id: BTreeMap<&str, &MoveAttemptReceipt> = attempts
        .iter()
        .map(|receipt| (receipt.attempt_id().as_str(), receipt))
        .collect();
    if by_id.len() != attempts.len() {
        return false;
    }
    attempts.iter().all(|receipt| {
        let mut cursor = receipt;
        for _ in 0..=attempts.len() {
            let Some(target) = cursor.correction_of() else {
                return true;
            };
            match by_id.get(target.as_str()) {
                Some(next) => cursor = next,
                None => return false,
            }
        }
        false // did not terminate within `len + 1` hops: a cycle.
    })
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() || data.len() > MAX_STEPS {
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

    let mut attempts: Vec<MoveAttemptReceipt> = Vec::new();

    for (index, &byte) in data.iter().enumerate() {
        let scheme = byte % 5;
        let correction_of = match scheme {
            0 => None,
            1 if index > 0 => Some(attempt_id((byte as usize / 5) % index)),
            2 => Some(attempt_id(index)), // self-reference
            3 => Some(attempt_id(index + 1 + (byte as usize % 4))), // forward, may resolve later
            _ => Some(MoveAttemptId::new(format!("phantom-{index}-{byte}")).unwrap()),
        };
        observe(
            scheme,
            match scheme {
                0 => "none",
                1 => "backward",
                2 => "self_cycle",
                3 => "forward",
                _ => "phantom",
            },
        );

        let (outcome, kind) = if correction_of.is_some() {
            (MoveAttemptOutcome::Corrected, Some(correction_kind(byte)))
        } else if byte % 2 == 0 {
            (MoveAttemptOutcome::RejectedByUser, None)
        } else {
            (MoveAttemptOutcome::Inapplicable, None)
        };

        let constructed = record_bpmn_attempt(
            &position,
            attempt_id(index),
            attempted_move.clone(),
            "fuzz observed intent",
            outcome,
            correction_of,
            kind,
        );
        if scheme == 2 {
            // Self-correction is refused at receipt construction, before it
            // can ever enter the tape - a stronger, earlier gate than the
            // history-level acyclic check this target otherwise fuzzes.
            assert!(constructed.is_err());
            continue;
        }
        attempts.push(constructed.unwrap());

        let expected_valid = reference_valid(&attempts);
        match project_bpmn_attempt_history(&attempts) {
            Ok((hash, projected)) => {
                assert!(
                    expected_valid,
                    "production accepted a tape the reference model rejects at step {index}"
                );
                assert_eq!(projected, attempts);
                let (hash_again, _) = project_bpmn_attempt_history(&attempts).unwrap();
                assert_eq!(hash, hash_again);
            }
            Err(_) => {
                assert!(
                    !expected_valid,
                    "production rejected a tape the reference model accepts at step {index}"
                );
                assert!(project_bpmn_attempt_history(&attempts).is_err());
            }
        }
    }
});
