#![no_main]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU32, Ordering};

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::Operation;
use designer_graph::productions::apply_production;
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    validate_attempt_history, ArgumentKind, BindingProvenance, BoardHash, CanonicalCandidateId,
    CorrectionKind, DesignStateId, EvidenceRecordHash, GraphContentHash, GraphRevision,
    MoveAttemptId, MoveAttemptOutcome, MoveAttemptReceipt, ProposalWorkbook, SlotRequirement,
    SlotValue, SlotValueState, WorkbookId, WorkbookSlot, GAMEBOARD_SCHEMA_VERSION,
};
use utterance_engine::bpmn_board::{
    materialize_bpmn_workbook, preview_bpmn_workbook, BpmnBoardError,
};
use uuid::Uuid;

const EXECUTABLE_CANDIDATES: &[&str] = &[
    "op.append_node",
    "op.insert_before",
    "op.insert_after",
    "op.replace_node",
    "op.connect",
    "op.create_branch",
    "op.create_parallel_region",
    "op.create_inclusive_region",
    "op.create_multi_instance_region",
    "op.attach_guard",
    "op.attach_rearming_guard",
    "op.set_guard_trigger",
    "op.set_guard_budget",
    "op.set_correlation_source",
    "op.delete_subgraph",
    "prod.request_and_wait",
    "prod.reminder_then_escalate",
    "prod.interrupting_timeout",
    "prod.non_interrupting_notification",
];

static OPERATION_COUNTERS: AtomicU32 = AtomicU32::new(0);
static ATTEMPT_COUNTERS: AtomicU32 = AtomicU32::new(0);

fn observe_operation(index: usize, candidate: &str) {
    let bit = 1_u32 << index;
    if OPERATION_COUNTERS.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter operation_kind={candidate}");
    }
}

fn observe_attempt(outcome: MoveAttemptOutcome) {
    let (index, label) = match outcome {
        MoveAttemptOutcome::Applied => (0, "applied"),
        MoveAttemptOutcome::Incomplete => (1, "incomplete"),
        MoveAttemptOutcome::Ambiguous => (2, "ambiguous"),
        MoveAttemptOutcome::Inapplicable => (3, "inapplicable"),
        MoveAttemptOutcome::DisclosureSafeRefusal => (4, "disclosure_safe_refusal"),
        MoveAttemptOutcome::Stale => (5, "stale"),
        MoveAttemptOutcome::CompilerRefused => (6, "compiler_refused"),
        MoveAttemptOutcome::RejectedByUser => (7, "rejected_by_user"),
        MoveAttemptOutcome::Corrected => (8, "corrected"),
        MoveAttemptOutcome::SystemFailure => (9, "system_failure"),
    };
    let bit = 1_u32 << index;
    if ATTEMPT_COUNTERS.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter attempt_outcome={label}");
    }
}

fn slot(name: &str, kind: ArgumentKind, value: SlotValue) -> WorkbookSlot {
    WorkbookSlot {
        name: name.to_string(),
        kind,
        requirement: SlotRequirement::Required,
        value: SlotValueState::Resolved(value),
        provenance: Some(BindingProvenance {
            source: "fuzz".into(),
            detail: "operation-tape".into(),
        }),
        clarification_prompt: format!("Supply {name}"),
    }
}

fn missing_slot(name: &str, kind: ArgumentKind) -> WorkbookSlot {
    WorkbookSlot {
        name: name.to_string(),
        kind,
        requirement: SlotRequirement::Required,
        value: SlotValueState::Missing,
        provenance: None,
        clarification_prompt: format!("Supply {name}"),
    }
}

fn revision(value: u64) -> String {
    format!("{value:064x}")
}

fn workbook(
    id: impl Into<String>,
    candidate: &str,
    graph_revision: &str,
    slots: Vec<WorkbookSlot>,
) -> ProposalWorkbook {
    ProposalWorkbook::new(
        GAMEBOARD_SCHEMA_VERSION,
        WorkbookId::new(id.into()).unwrap(),
        1,
        BoardHash::new("a".repeat(64)).unwrap(),
        GraphRevision::new(graph_revision).unwrap(),
        CanonicalCandidateId::new(candidate).unwrap(),
        slots,
        EvidenceRecordHash::new("e".repeat(64)).unwrap(),
    )
    .unwrap()
}

fn fixture() -> DesignerDag {
    let start = NodeKey(Uuid::from_u128(1));
    let end = NodeKey(Uuid::from_u128(2));
    let mut dag = DesignerDag::new("preview-fuzz");
    dag.seed(
        start,
        IRNode::Start { id: "start".into() },
        Provenance::default(),
    )
    .unwrap();
    apply_production(
        &dag,
        vec![Operation::AppendNode {
            anchor: start,
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
    .candidate
}

fn shape_slots(candidate: &str, suffix: u8) -> Vec<WorkbookSlot> {
    let identifier = |name: &str| {
        slot(
            name,
            ArgumentKind::Identifier,
            SlotValue::Identifier(format!("{name}_{suffix}")),
        )
    };
    let node = |name: &str, value: &str| {
        slot(
            name,
            ArgumentKind::NodeReference,
            SlotValue::NodeReference(value.to_string()),
        )
    };
    let count = |name: &str, value| slot(name, ArgumentKind::Count, SlotValue::Count(value));
    let duration = |name: &str| {
        slot(
            name,
            ArgumentKind::Duration,
            SlotValue::DurationMillis(1_000),
        )
    };
    let data = |name: &str| {
        slot(
            name,
            ArgumentKind::DataReference,
            SlotValue::DataReference(format!("{name}_{suffix}")),
        )
    };

    match candidate {
        "op.append_node" | "op.insert_before" | "op.insert_after" => {
            vec![node("anchor", "start"), identifier("node")]
        }
        "op.replace_node" => vec![node("target", "end"), identifier("replacement")],
        "op.connect" => vec![node("from", "start"), node("to", "end")],
        "op.create_branch" => vec![
            node("gateway", "start"),
            identifier("outcome"),
            node("target", "end"),
        ],
        "op.create_parallel_region" => {
            vec![node("anchor", "start"), count("branch_count", 2)]
        }
        "op.create_inclusive_region" => vec![
            node("anchor", "start"),
            count("branch_count", 2),
            slot(
                "conditions",
                ArgumentKind::Condition,
                SlotValue::Condition("left==true;right==true".into()),
            ),
        ],
        "op.create_multi_instance_region" => vec![
            node("anchor", "start"),
            data("collection"),
            count("declared_max", 2),
        ],
        "op.attach_guard" => vec![
            node("host", "start"),
            duration("trigger"),
            identifier("escape"),
        ],
        "op.attach_rearming_guard" => vec![
            node("host", "start"),
            duration("interval"),
            count("max_fires", 2),
            identifier("escape"),
        ],
        "op.set_guard_trigger" => vec![node("guard", "start"), duration("trigger")],
        "op.set_guard_budget" => {
            vec![node("guard", "start"), count("failure_budget", 2)]
        }
        "op.set_correlation_source" => vec![node("node", "start"), data("data_reference")],
        "op.delete_subgraph" => vec![node("target", "end")],
        "prod.request_and_wait" => vec![
            node("anchor", "start"),
            identifier("request"),
            data("correlation_source"),
        ],
        "prod.reminder_then_escalate" => vec![
            node("anchor", "start"),
            duration("interval"),
            count("max_reminders", 2),
            identifier("escalation"),
        ],
        "prod.interrupting_timeout" => vec![
            node("anchor", "start"),
            duration("duration"),
            identifier("escape"),
        ],
        "prod.non_interrupting_notification" => vec![
            node("anchor", "start"),
            duration("interval"),
            count("max_fires", 2),
            identifier("notification"),
        ],
        _ => unreachable!("candidate inventory is closed"),
    }
}

fn exercise_all_materializers(dag: &DesignerDag, suffix: u8) {
    let mut reached = BTreeSet::new();
    for (index, candidate) in EXECUTABLE_CANDIDATES.iter().enumerate() {
        let workbook = workbook(
            format!("shape-{index}-{suffix}"),
            candidate,
            &revision(0),
            shape_slots(candidate, suffix),
        );
        let first = materialize_bpmn_workbook(dag, &workbook).unwrap();
        let second = materialize_bpmn_workbook(dag, &workbook).unwrap();
        assert_eq!(
            serde_json::to_value(first.operations()).unwrap(),
            serde_json::to_value(second.operations()).unwrap()
        );
        assert!(!first.operations().is_empty());
        reached.insert(*candidate);
        observe_operation(index, candidate);
    }
    assert_eq!(reached.len(), EXECUTABLE_CANDIDATES.len());
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReferenceReceipt {
    id: String,
    outcome: MoveAttemptOutcome,
    correction_of: Option<String>,
}

/// Independent design-game model for the fuzz tape. It knows only that a
/// ratified insertion adds one node, refusals do not transition, and a
/// correction must link an earlier retained attempt.
struct ReferenceGame {
    node_count: usize,
    revision: u64,
    pending: bool,
    receipts: Vec<ReferenceReceipt>,
}

impl ReferenceGame {
    fn new() -> Self {
        Self {
            node_count: 2,
            revision: 0,
            pending: false,
            receipts: Vec::new(),
        }
    }

    fn record(&mut self, id: String, outcome: MoveAttemptOutcome, correction_of: Option<String>) {
        if let Some(target) = &correction_of {
            assert!(self.receipts.iter().any(|receipt| &receipt.id == target));
            assert_ne!(target, &id);
        }
        self.receipts.push(ReferenceReceipt {
            id,
            outcome,
            correction_of,
        });
    }

    fn assert_matches(
        &self,
        dag: &DesignerDag,
        actual_revision: u64,
        pending: &Option<Vec<Operation>>,
        receipts: &[MoveAttemptReceipt],
    ) {
        assert_eq!(dag.node_count(), self.node_count);
        assert_eq!(actual_revision, self.revision);
        assert_eq!(pending.is_some(), self.pending);
        assert_eq!(receipts.len(), self.receipts.len());
        for (actual, expected) in receipts.iter().zip(&self.receipts) {
            assert_eq!(actual.attempt_id().as_str(), expected.id);
            assert_eq!(actual.outcome(), expected.outcome);
            assert_eq!(
                actual.correction_of().map(|value| value.as_str()),
                expected.correction_of.as_deref()
            );
        }
        validate_attempt_history(receipts).unwrap();
    }
}

fn record_receipt(
    receipts: &mut Vec<MoveAttemptReceipt>,
    reference: &mut ReferenceGame,
    step: usize,
    outcome: MoveAttemptOutcome,
    correction_of: Option<MoveAttemptId>,
) -> MoveAttemptId {
    let id = MoveAttemptId::new(format!("attempt-{step}")).unwrap();
    let correction_kind = correction_of.as_ref().map(|_| CorrectionKind::Replacement);
    receipts.push(
        MoveAttemptReceipt::new(
            GAMEBOARD_SCHEMA_VERSION,
            id.clone(),
            DesignStateId::new("d".repeat(64)).unwrap(),
            None,
            GraphContentHash::new("f".repeat(64)).unwrap(),
            outcome,
            Vec::new(),
            Vec::new(),
            correction_of.clone(),
            correction_kind,
        )
        .unwrap(),
    );
    reference.record(
        id.as_str().to_string(),
        outcome,
        correction_of.map(|value| value.as_str().to_string()),
    );
    observe_attempt(outcome);
    id
}

fn insertion_workbook(step: usize, graph_revision: &str) -> ProposalWorkbook {
    workbook(
        format!("preview-{step}"),
        "op.insert_after",
        graph_revision,
        vec![
            slot(
                "anchor",
                ArgumentKind::NodeReference,
                SlotValue::NodeReference("start".into()),
            ),
            slot(
                "node",
                ArgumentKind::Identifier,
                SlotValue::Identifier(format!("generated_{step}")),
            ),
        ],
    )
}

fuzz_target!(|data: &[u8]| {
    let suffix = data.first().copied().unwrap_or_default();
    let mut dag = fixture();
    dag.admit().unwrap();
    exercise_all_materializers(&dag, suffix);

    let mut actual_revision = 0_u64;
    let mut pending: Option<Vec<Operation>> = None;
    let mut receipts = Vec::new();
    let mut last_applied = None;
    let mut reference = ReferenceGame::new();

    for (step, byte) in data.iter().copied().take(32).enumerate() {
        match byte % 8 {
            0 => {
                let current = revision(actual_revision);
                let workbook = insertion_workbook(step, &current);
                let before = dag.node_count();
                let preview =
                    preview_bpmn_workbook(&dag, &workbook, &current, &revision(100)).unwrap();
                let replay = apply_production(
                    &dag,
                    preview.bound().operations().to_vec(),
                    Provenance::default(),
                )
                .unwrap();
                replay.candidate.admit().unwrap();
                assert_eq!(dag.node_count(), before);
                pending = Some(preview.bound().operations().to_vec());
                reference.pending = true;
            }
            1 => {
                if let Some(operations) = pending.take() {
                    let applied =
                        apply_production(&dag, operations, Provenance::default()).unwrap();
                    applied.candidate.admit().unwrap();
                    dag = applied.candidate;
                    actual_revision += 1;
                    reference.pending = false;
                    reference.node_count += 1;
                    reference.revision += 1;
                    last_applied = Some(record_receipt(
                        &mut receipts,
                        &mut reference,
                        step,
                        MoveAttemptOutcome::Applied,
                        None,
                    ));
                }
            }
            2 => {
                if pending.take().is_some() {
                    reference.pending = false;
                    record_receipt(
                        &mut receipts,
                        &mut reference,
                        step,
                        MoveAttemptOutcome::RejectedByUser,
                        None,
                    );
                }
            }
            3 => {
                let current = revision(actual_revision);
                let stale = if actual_revision == 0 {
                    revision(99)
                } else {
                    revision(actual_revision - 1)
                };
                let workbook = insertion_workbook(step, &stale);
                assert!(matches!(
                    preview_bpmn_workbook(&dag, &workbook, &current, &revision(100)),
                    Err(BpmnBoardError::StaleWorkbook { .. })
                ));
                record_receipt(
                    &mut receipts,
                    &mut reference,
                    step,
                    MoveAttemptOutcome::Stale,
                    None,
                );
            }
            4 => {
                let current = revision(actual_revision);
                let workbook = workbook(
                    format!("refused-{step}"),
                    "op.delete_subgraph",
                    &current,
                    vec![slot(
                        "target",
                        ArgumentKind::NodeReference,
                        SlotValue::NodeReference("start".into()),
                    )],
                );
                assert!(matches!(
                    preview_bpmn_workbook(&dag, &workbook, &current, &revision(100)),
                    Err(BpmnBoardError::CompilerRefused { .. })
                ));
                record_receipt(
                    &mut receipts,
                    &mut reference,
                    step,
                    MoveAttemptOutcome::CompilerRefused,
                    None,
                );
            }
            5 => {
                if let Some(original) = last_applied.clone() {
                    let current = revision(actual_revision);
                    let workbook = insertion_workbook(step, &current);
                    let preview =
                        preview_bpmn_workbook(&dag, &workbook, &current, &revision(100)).unwrap();
                    let applied = apply_production(
                        &dag,
                        preview.bound().operations().to_vec(),
                        Provenance::default(),
                    )
                    .unwrap();
                    applied.candidate.admit().unwrap();
                    dag = applied.candidate;
                    actual_revision += 1;
                    reference.node_count += 1;
                    reference.revision += 1;
                    last_applied = Some(record_receipt(
                        &mut receipts,
                        &mut reference,
                        step,
                        MoveAttemptOutcome::Corrected,
                        Some(original),
                    ));
                }
            }
            6 => {
                let current = revision(actual_revision);
                let workbook = workbook(
                    format!("incomplete-{step}"),
                    "op.insert_after",
                    &current,
                    vec![
                        slot(
                            "anchor",
                            ArgumentKind::NodeReference,
                            SlotValue::NodeReference("start".into()),
                        ),
                        missing_slot("node", ArgumentKind::Identifier),
                    ],
                );
                assert!(matches!(
                    materialize_bpmn_workbook(&dag, &workbook),
                    Err(BpmnBoardError::WorkbookNotReady { .. })
                ));
                record_receipt(
                    &mut receipts,
                    &mut reference,
                    step,
                    MoveAttemptOutcome::Incomplete,
                    None,
                );
            }
            _ => {
                let current = revision(actual_revision);
                let workbook = workbook(
                    format!("inapplicable-{step}"),
                    "op.insert_after",
                    &current,
                    vec![
                        slot(
                            "anchor",
                            ArgumentKind::NodeReference,
                            SlotValue::NodeReference("missing".into()),
                        ),
                        slot(
                            "node",
                            ArgumentKind::Identifier,
                            SlotValue::Identifier(format!("generated_{step}")),
                        ),
                    ],
                );
                assert!(matches!(
                    materialize_bpmn_workbook(&dag, &workbook),
                    Err(BpmnBoardError::Binding(_))
                ));
                record_receipt(
                    &mut receipts,
                    &mut reference,
                    step,
                    MoveAttemptOutcome::Inapplicable,
                    None,
                );
            }
        }
        reference.assert_matches(&dag, actual_revision, &pending, &receipts);
    }
});
