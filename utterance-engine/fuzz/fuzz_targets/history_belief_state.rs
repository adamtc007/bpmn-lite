#![no_main]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU16, Ordering};

use bpmn_lite_compiler::{IRNode, TimerSpec};
use designer_graph::ops::{GuardTrigger, Operation};
use designer_graph::productions::apply_production;
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    CorrectionKind, DesignFocus, EvidenceLane, FocusAbsenceReason, MoveAttemptId,
    MoveAttemptOutcome, MoveAttemptReceipt,
};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, finalize_bpmn_move_evidence,
    project_bpmn_attempt_history, record_bpmn_attempt, update_bpmn_design_belief,
};
use utterance_engine::contract::{FiniteScore, RankedCandidate, SlmResult};
use uuid::Uuid;

static COUNTERS: AtomicU16 = AtomicU16::new(0);

fn observe(index: u8, label: &str) {
    let bit = 1_u16 << index;
    if COUNTERS.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter history_belief={label}");
    }
}

fn key(value: u128) -> NodeKey {
    NodeKey(Uuid::from_u128(value))
}

fn graph(shape: u8) -> DesignerDag {
    if shape == 0 {
        return DesignerDag::new("history-empty");
    }
    let start = key(1);
    let task = key(2);
    let end = key(3);
    let mut dag = DesignerDag::new("history-active");
    dag.seed(
        start,
        IRNode::Start { id: "start".into() },
        Provenance::default(),
    )
    .unwrap();
    dag = apply_production(
        &dag,
        vec![Operation::AppendNode {
            anchor: start,
            key: task,
            node: IRNode::ServiceTask {
                id: "review".into(),
                name: "Review".into(),
                task_type: "review".into(),
            },
            edge_id: "flow_review".into(),
        }],
        Provenance::default(),
    )
    .unwrap()
    .candidate;
    dag = apply_production(
        &dag,
        vec![Operation::AppendNode {
            anchor: task,
            key: end,
            node: IRNode::End {
                id: "end".into(),
                terminate: false,
            },
            edge_id: "flow_end".into(),
        }],
        Provenance::default(),
    )
    .unwrap()
    .candidate;
    if shape == 2 {
        let guard = key(4);
        dag = apply_production(
            &dag,
            vec![Operation::AttachGuard {
                host: task,
                key: guard,
                guard_id: "timeout".into(),
                trigger: GuardTrigger::Timer(TimerSpec::Duration { ms: 60_000 }),
            }],
            Provenance::default(),
        )
        .unwrap()
        .candidate;
        dag = apply_production(
            &dag,
            vec![Operation::AppendNode {
                anchor: guard,
                key: key(5),
                node: IRNode::End {
                    id: "timeout_end".into(),
                    terminate: false,
                },
                edge_id: "flow_timeout".into(),
            }],
            Provenance::default(),
        )
        .unwrap()
        .candidate;
    }
    dag
}

fn raw(board: &semantic_decision_contracts::SemanticDecisionBoard, seed: u8) -> SlmResult {
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
        retrieved_subset_hash: "history-full-board".into(),
        board_hash: board.board_hash.as_str().to_string(),
        model_bundle_hash: "history.reference".into(),
        evidence_trace: None,
        inference_evidence: None,
        move_evidence: Vec::new(),
    }
}

struct ReferenceHistory {
    ids: BTreeSet<String>,
    corrections: usize,
}

impl ReferenceHistory {
    fn new() -> Self {
        Self {
            ids: BTreeSet::new(),
            corrections: 0,
        }
    }

    fn append(&mut self, receipt: &MoveAttemptReceipt) {
        assert!(self.ids.insert(receipt.attempt_id().as_str().to_string()));
        if let Some(target) = receipt.correction_of() {
            assert!(self.ids.contains(target.as_str()));
            self.corrections += 1;
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let shape = data.first().copied().unwrap_or_default() % 3;
    let dag = graph(shape);
    let revision = format!("{:064x}", data.get(1).copied().unwrap_or_default());
    let board = build_bpmn_semantic_board(&dag, None, &revision, &PolicyFilter::default()).unwrap();
    let (empty_hash, _) = project_bpmn_attempt_history(&[]).unwrap();
    let position = build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &"b".repeat(64),
        "history-fuzz-v1",
        &empty_hash,
        DesignFocus::absent(FocusAbsenceReason::NotProvided, None).unwrap(),
        None,
    )
    .unwrap();
    let legal_before = position
        .legal_moves()
        .iter()
        .map(|legal_move| legal_move.move_id().clone())
        .collect::<BTreeSet<_>>();
    let fused = finalize_bpmn_move_evidence(
        &board,
        &position,
        "remind then escalate",
        raw(&board, data.get(2).copied().unwrap_or_default()),
        EvidenceLane::Lexical,
        vec!["history.reference".into()],
        &[],
    )
    .unwrap();
    let mut receipts = Vec::<MoveAttemptReceipt>::new();
    let mut reference = ReferenceHistory::new();
    let mut previous = None;

    for (index, byte) in data.iter().copied().skip(3).take(65).enumerate() {
        let (outcome, correction) = match byte % 8 {
            0 => (MoveAttemptOutcome::Incomplete, None),
            1 => (MoveAttemptOutcome::Ambiguous, None),
            2 => (MoveAttemptOutcome::Inapplicable, None),
            3 => (MoveAttemptOutcome::CompilerRefused, None),
            4 => (MoveAttemptOutcome::RejectedByUser, None),
            5 => (MoveAttemptOutcome::Applied, None),
            6 if !receipts.is_empty() => {
                observe(8, "correction");
                (
                    MoveAttemptOutcome::Corrected,
                    Some(receipts.last().unwrap().attempt_id().clone()),
                )
            }
            _ => (MoveAttemptOutcome::Stale, None),
        };
        observe(
            byte % 8,
            match byte % 8 {
                0 => "incomplete",
                1 => "ambiguous",
                2 => "inapplicable",
                3 => "compiler_refused",
                4 => "rejected",
                5 => "applied",
                6 => "corrected",
                _ => "stale",
            },
        );
        let attempted_move = position
            .legal_moves()
            .first()
            .map(|item| item.move_id().clone());
        let receipt = record_bpmn_attempt(
            &position,
            MoveAttemptId::new(format!("attempt-{index}")).unwrap(),
            attempted_move,
            "fuzz observation",
            outcome,
            correction.clone(),
            correction.map(|_| CorrectionKind::FollowUp),
        )
        .unwrap();
        receipts.push(receipt.clone());
        if receipts.len() > 64 {
            observe(9, "resource_bound");
            assert!(project_bpmn_attempt_history(&receipts).is_err());
            receipts.pop();
            break;
        }
        reference.append(&receipt);
        let first = project_bpmn_attempt_history(&receipts).unwrap();
        let second = project_bpmn_attempt_history(&receipts).unwrap();
        assert_eq!(first.0, second.0);
        assert_eq!(
            serde_json::to_value(&first.1).unwrap(),
            serde_json::to_value(&second.1).unwrap()
        );
        let belief = update_bpmn_design_belief(
            &dag,
            &position,
            &fused.move_evidence,
            &first.1,
            previous.as_ref(),
        )
        .unwrap();
        let replay = update_bpmn_design_belief(
            &dag,
            &position,
            &fused.move_evidence,
            &first.1,
            previous.as_ref(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&belief).unwrap(),
            serde_json::to_value(&replay).unwrap()
        );
        previous = Some(belief);
        assert_eq!(
            legal_before,
            position
                .legal_moves()
                .iter()
                .map(|legal_move| legal_move.move_id().clone())
                .collect::<BTreeSet<_>>()
        );
    }

    match shape {
        0 => {
            observe(10, "motif_abandoned");
            assert!(previous
                .as_ref()
                .is_none_or(|belief| belief.motifs().is_empty()));
        }
        1 => {
            observe(11, "motif_active");
            assert!(previous
                .as_ref()
                .is_none_or(|belief| !belief.motifs().is_empty()));
        }
        _ => {
            observe(12, "motif_completed");
            assert!(previous.as_ref().is_none_or(|belief| belief
                .motifs()
                .iter()
                .all(|motif| motif.motif_id() != "motif.reminder_then_escalate")));
        }
    }
});
