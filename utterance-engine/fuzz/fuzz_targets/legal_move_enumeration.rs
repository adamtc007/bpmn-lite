#![no_main]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU8, Ordering};

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::Operation;
use designer_graph::productions::apply_production;
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    DesignFocus, DesignPosition, FocusAbsenceReason, GraphElementRef, MoveBindingState,
    ABSTENTION_CANDIDATE_ID,
};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{build_bpmn_design_position, build_bpmn_semantic_board};
use uuid::Uuid;

static ANCHOR_COUNTERS: AtomicU8 = AtomicU8::new(0);

fn observe_anchor(anchor: Option<&str>) {
    let (index, label) = match anchor {
        None => (0, "whole_graph"),
        Some("start") => (1, "start"),
        Some("end") => (2, "end"),
        Some(_) => (3, "task"),
    };
    let bit = 1_u8 << index;
    if ANCHOR_COUNTERS.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter anchor_shape={label}");
    }
}

fn key(value: u128) -> NodeKey {
    NodeKey(Uuid::from_u128(value))
}

fn build_graph(task_count: usize) -> (DesignerDag, Vec<(NodeKey, String)>) {
    let start = key(1);
    let mut dag = DesignerDag::new("fuzz-gameboard");
    dag.seed(
        start,
        IRNode::Start { id: "start".into() },
        Provenance::default(),
    )
    .unwrap();
    let mut elements = vec![(start, "start".to_string())];
    let mut anchor = start;
    for index in 0..task_count {
        let node_key = key(2 + index as u128);
        let id = format!("task_{index}");
        dag = apply_production(
            &dag,
            vec![Operation::AppendNode {
                anchor,
                key: node_key,
                node: IRNode::ServiceTask {
                    id: id.clone(),
                    name: id.clone(),
                    task_type: "noop".into(),
                },
                edge_id: format!("flow_{id}"),
            }],
            Provenance::default(),
        )
        .unwrap()
        .candidate;
        elements.push((node_key, id));
        anchor = node_key;
    }
    let end_key = key(100);
    dag = apply_production(
        &dag,
        vec![Operation::AppendNode {
            anchor,
            key: end_key,
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
    elements.push((end_key, "end".to_string()));
    (dag, elements)
}

/// Compact independent model for the deliberately small generated graph family.
/// It mirrors user-visible graph facts, not `PositionalLegality`, mutation, or
/// compiler internals.
struct ReferencePosition {
    revision: String,
    anchors: Vec<String>,
}

impl ReferencePosition {
    fn new(revision: String, elements: &[(NodeKey, String)], anchor: Option<&str>) -> Self {
        let anchors = match anchor {
            Some(anchor) => vec![anchor.to_string()],
            None => elements.iter().map(|(_, id)| id.clone()).collect(),
        };
        Self { revision, anchors }
    }

    fn candidates_at(anchor: &str) -> &'static [&'static str] {
        if anchor == "start" {
            &[
                "op.connect",
                "op.create_inclusive_region",
                "op.create_multi_instance_region",
                "op.create_parallel_region",
                "op.insert_after",
                "prod.request_and_wait",
            ]
        } else if anchor == "end" {
            &["op.insert_before", "op.replace_node"]
        } else {
            &[
                "op.attach_guard",
                "op.attach_rearming_guard",
                "op.connect",
                "op.create_inclusive_region",
                "op.create_multi_instance_region",
                "op.create_parallel_region",
                "op.insert_after",
                "op.insert_before",
                "op.replace_node",
                "prod.interrupting_timeout",
                "prod.non_interrupting_notification",
                "prod.reminder_then_escalate",
                "prod.request_and_wait",
            ]
        }
    }

    fn assert_matches(&self, position: &DesignPosition) {
        assert_eq!(position.graph_revision().as_str(), self.revision);
        let expected = self
            .anchors
            .iter()
            .flat_map(|anchor| {
                Self::candidates_at(anchor)
                    .iter()
                    .map(move |candidate| ((*candidate).to_string(), anchor.clone()))
            })
            .collect::<BTreeSet<_>>();
        let observed = position
            .legal_moves()
            .iter()
            .filter(|legal_move| legal_move.candidate_id().as_str() != ABSTENTION_CANDIDATE_ID)
            .map(|legal_move| {
                (
                    legal_move.candidate_id().as_str().to_string(),
                    legal_move.anchor().unwrap().as_str().to_string(),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(observed, expected);
        assert!(position.legal_moves().iter().all(|legal_move| {
            legal_move.candidate_id().as_str() == ABSTENTION_CANDIDATE_ID
                || matches!(
                    legal_move.binding_state(),
                    MoveBindingState::Incomplete { .. }
                )
        }));
    }
}

fuzz_target!(|data: &[u8]| {
    let task_count = data.first().copied().unwrap_or_default() as usize % 5;
    let (dag, elements) = build_graph(task_count);
    dag.admit().unwrap();
    let focus_selector = data.get(1).copied().unwrap_or_default() as usize;
    let anchored = focus_selector % (elements.len() + 1);
    let anchor = (anchored < elements.len()).then(|| &elements[anchored]);
    let revision = format!("{:064x}", data.get(2).copied().unwrap_or_default());
    let board = build_bpmn_semantic_board(
        &dag,
        anchor.map(|(key, id)| (*key, id.as_str())),
        &revision,
        &PolicyFilter::default(),
    )
    .unwrap();
    let focus = match anchor {
        Some((_, id)) => DesignFocus::element(GraphElementRef::new(id).unwrap()),
        None => DesignFocus::absent(FocusAbsenceReason::NotProvided, None).unwrap(),
    };
    let first = build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &"b".repeat(64),
        "compiler-fuzz-v1",
        &"c".repeat(64),
        focus.clone(),
        None,
    )
    .unwrap();
    let second = build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &"b".repeat(64),
        "compiler-fuzz-v1",
        &"c".repeat(64),
        focus,
        None,
    )
    .unwrap();

    ReferencePosition::new(
        revision.clone(),
        &elements,
        anchor.map(|(_, id)| id.as_str()),
    )
    .assert_matches(&first);

    assert_eq!(first, second);
    assert_eq!(dag.node_count(), elements.len());
    let wire = serde_json::to_vec(&first).unwrap();
    let decoded: DesignPosition = serde_json::from_slice(&wire).unwrap();
    assert_eq!(decoded, first);
    let ids = first
        .legal_moves()
        .iter()
        .map(|legal_move| legal_move.move_id().as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), first.legal_moves().len());
    assert_eq!(
        first
            .legal_moves()
            .iter()
            .filter(|legal_move| legal_move.candidate_id().as_str() == ABSTENTION_CANDIDATE_ID)
            .count(),
        1
    );
    if let Some((_, id)) = anchor {
        assert!(first.legal_moves().iter().all(|legal_move| {
            legal_move.candidate_id().as_str() == ABSTENTION_CANDIDATE_ID
                || legal_move
                    .anchor()
                    .is_some_and(|value| value.as_str() == id)
        }));
    }

    for index in 0..=elements.len() {
        let focus_anchor = (index < elements.len()).then(|| &elements[index]);
        let board = build_bpmn_semantic_board(
            &dag,
            focus_anchor.map(|(key, id)| (*key, id.as_str())),
            &revision,
            &PolicyFilter::default(),
        )
        .unwrap();
        let focus = match focus_anchor {
            Some((_, id)) => DesignFocus::element(GraphElementRef::new(id).unwrap()),
            None => DesignFocus::absent(FocusAbsenceReason::NotProvided, None).unwrap(),
        };
        let position = build_bpmn_design_position(
            &dag,
            &board,
            &revision,
            &"b".repeat(64),
            "compiler-fuzz-v1",
            &"c".repeat(64),
            focus,
            None,
        )
        .unwrap();
        ReferencePosition::new(
            revision.clone(),
            &elements,
            focus_anchor.map(|(_, id)| id.as_str()),
        )
        .assert_matches(&position);
        observe_anchor(focus_anchor.map(|(_, id)| id.as_str()));
    }
});
