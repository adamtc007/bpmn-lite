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
    build_bpmn_design_position, build_bpmn_semantic_board, decide_bpmn_game_disposition,
    finalize_bpmn_move_evidence, project_bpmn_attempt_history, record_bpmn_attempt,
    update_bpmn_design_belief, BpmnBoardError,
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

// P1: content-derived DAG identity, computed the same way
// `build_bpmn_design_position` derives its own `graph_state_hash` (D23/I34) --
// not a fuzz-target-local notion of "hash".
fn content_hash(dag: &DesignerDag) -> String {
    DesignerDag::graph_state_hash(&dag.to_ir().unwrap())
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

// U1 P6: case/whitespace decoration of the fixed governed phrase. The phrase
// content itself must not change -- shape 2's motif.reminder_then_escalate
// completion check (bottom of this file) depends on the same canonical
// phrase resolving every time, and normalisation-equivalent variants
// resolving identically is exactly what P6 asserts, mirroring the proven
// pattern in evidence_fusion.rs (`"insert after"` vs `"  INSERT   AFTER  "`).
const CANONICAL_INTENT: &str = "remind then escalate";

fn observed_intent(selector: u8) -> String {
    match selector % 4 {
        0 => CANONICAL_INTENT.to_string(),
        1 => CANONICAL_INTENT.to_uppercase(),
        2 => format!("  {}  ", CANONICAL_INTENT.replace(' ', "   ")),
        _ => {
            let mut mixed = String::new();
            for (index, ch) in CANONICAL_INTENT.chars().enumerate() {
                if index % 2 == 0 {
                    mixed.extend(ch.to_uppercase());
                } else {
                    mixed.push(ch);
                }
            }
            format!(" {mixed} ")
        }
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
    let hash_before = content_hash(&dag);
    let revision_byte = data.get(1).copied().unwrap_or_default();
    let revision = format!("{revision_byte:064x}");
    let board = build_bpmn_semantic_board(&dag, None, &revision, &PolicyFilter::default()).unwrap();

    // U1 P5, axis 2: foreign/stale board revision. `build_bpmn_design_position`
    // must refuse a caller-supplied revision that disagrees with the board's
    // own -- confirmed by reading bpmn_board.rs directly that no existing
    // fuzz target exercises this boundary. wrapping_add(1) guarantees a
    // distinct 64-hex-char string regardless of `revision_byte`'s value.
    let axis = data.get(3).copied().unwrap_or_default() % 3;
    if axis == 2 {
        observe(13, "hostile_stale_board_revision");
        let foreign_revision = format!("{:064x}", revision_byte.wrapping_add(1));
        let (empty_hash, _) = project_bpmn_attempt_history(&[]).unwrap();
        let result = build_bpmn_design_position(
            &dag,
            &board,
            &foreign_revision,
            &"b".repeat(64),
            "history-fuzz-v1",
            &empty_hash,
            DesignFocus::absent(FocusAbsenceReason::NotProvided),
            None,
        );
        assert!(matches!(
            result,
            Err(BpmnBoardError::StaleBoardRevision { .. })
        ));
        assert_eq!(content_hash(&dag), hash_before);
        return;
    }

    let (empty_hash, _) = project_bpmn_attempt_history(&[]).unwrap();
    let position = build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &"b".repeat(64),
        "history-fuzz-v1",
        &empty_hash,
        DesignFocus::absent(FocusAbsenceReason::NotProvided),
        None,
    )
    .unwrap();
    let legal_before = position
        .legal_moves()
        .iter()
        .map(|legal_move| legal_move.move_id().clone())
        .collect::<BTreeSet<_>>();

    let intent = observed_intent(data.get(4).copied().unwrap_or_default());
    let ranking_seed = data.get(2).copied().unwrap_or_default();

    // U1 P5, axis 1: off-board candidate injection. `evidence_fusion.rs`
    // already covers duplicate and omitted candidates against this same
    // `finalize_bpmn_move_evidence` boundary; a foreign candidate id was the
    // one malformed-ranking sub-case it never exercises.
    if axis == 1 {
        observe(14, "hostile_off_board_candidate");
        let mut malformed = raw(&board, ranking_seed);
        malformed.ranking.push(RankedCandidate {
            candidate_id: format!("fuzz-off-board-{ranking_seed}"),
            score: FiniteScore::new(0.5).unwrap(),
        });
        assert!(finalize_bpmn_move_evidence(
            &board,
            &position,
            &intent,
            malformed,
            EvidenceLane::Lexical,
            vec!["history.reference".into()],
            &[],
        )
        .is_err());
        assert_eq!(
            position
                .legal_moves()
                .iter()
                .map(|legal_move| legal_move.move_id().clone())
                .collect::<BTreeSet<_>>(),
            legal_before
        );
        assert_eq!(content_hash(&dag), hash_before);
        return;
    }

    // axis == 0: valid tape.
    let fused = finalize_bpmn_move_evidence(
        &board,
        &position,
        &intent,
        raw(&board, ranking_seed),
        EvidenceLane::Lexical,
        vec!["history.reference".into()],
        &[],
    )
    .unwrap();

    // U1 P6 (reuse, not new design): normalisation-equivalent decorations of
    // the canonical phrase must resolve to identical evidence. Already
    // proven at this same boundary in evidence_fusion.rs; asserted inline
    // here too since this target now varies the phrase.
    if data.get(4).copied().unwrap_or_default() % 4 != 0 {
        let canonical = finalize_bpmn_move_evidence(
            &board,
            &position,
            CANONICAL_INTENT,
            raw(&board, ranking_seed),
            EvidenceLane::Lexical,
            vec!["history.reference".into()],
            &[],
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&fused.move_evidence).unwrap(),
            serde_json::to_value(&canonical.move_evidence).unwrap()
        );
    }

    // U1 P3 (reuse, not new design): already proven against this same
    // function in evidence_fusion.rs; asserted inline here for this
    // target's own generated position too.
    assert_eq!(fused.move_evidence.len(), position.legal_moves().len());
    let probability_sum: f64 = fused
        .move_evidence
        .iter()
        .map(|evidence| evidence.probability().get())
        .sum();
    // Same epsilon evidence_fusion.rs already proved sufficient at this
    // boundary (fusion.rs:294) -- not independently re-derived here.
    assert!((probability_sum - 1.0).abs() < 1e-12);

    let mut receipts = Vec::<MoveAttemptReceipt>::new();
    let mut reference = ReferenceHistory::new();
    let mut previous = None;

    for (index, byte) in data.iter().copied().skip(5).take(65).enumerate() {
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
        let attempt_id = MoveAttemptId::new(format!("attempt-{index}")).unwrap();
        let receipt = record_bpmn_attempt(
            &position,
            attempt_id.clone(),
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

        // U1 P2+P4 (ported from disposition_workbook_state.rs's pattern, not
        // redesigned): decide_bpmn_game_disposition must validate against
        // this position and name only on-board moves, and repeat calls with
        // identical inputs must be deterministic.
        let disposition = decide_bpmn_game_disposition(
            &board,
            &position,
            &fused.move_evidence,
            &belief,
            &intent,
            attempt_id.clone(),
            &receipts,
            None,
        )
        .unwrap();
        disposition.validate_for_position(&position).unwrap();
        assert!(disposition.selected_moves().iter().all(|move_id| position
            .legal_moves()
            .iter()
            .any(|legal_move| legal_move.move_id() == move_id)));
        let disposition_replay = decide_bpmn_game_disposition(
            &board,
            &position,
            &fused.move_evidence,
            &belief,
            &intent,
            attempt_id,
            &receipts,
            None,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&disposition).unwrap(),
            serde_json::to_value(&disposition_replay).unwrap()
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

    // U1 P1: the source DAG's content-derived identity must be unchanged
    // across the whole board -> ... -> disposition sequence, valid-tape path.
    assert_eq!(content_hash(&dag), hash_before);
});
