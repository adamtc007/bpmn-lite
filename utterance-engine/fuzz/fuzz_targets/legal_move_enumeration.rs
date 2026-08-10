#![no_main]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU8, Ordering};

use bpmn_lite_compiler::IRNode;
use designer_graph::ops::Operation;
use designer_graph::productions::apply_production;
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    DesignFocus, DesignPosition, FocusAbsenceReason, GameboardContractError, GraphElementRef,
    MoveBindingState, ABSTENTION_CANDIDATE_ID, MAX_LEGAL_MOVES,
};
use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::{
    build_bpmn_design_position, build_bpmn_semantic_board, BpmnBoardError,
};
use utterance_engine::MAX_ENUMERATION_CANDIDATES;
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
    // Task keys are `key(2 + index)` for `index in 0..task_count`; with
    // task_count now reaching up to 339 (see `% 340` below), a fixed
    // `key(100)` would collide with the task node at index 98 — a latent bug
    // in this fixture invisible while task_count was capped at ≤4. Well clear
    // of the task-key range regardless of how large task_count grows.
    let end_key = key(1_000_000);
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

/// The exact raw anchor x candidate count `enumerate`'s amplification counter
/// accumulates for a whole-graph (unanchored) position on this generated
/// graph family — derived from `ReferencePosition::candidates_at`, the same
/// source of truth the rest of this target already checks production output
/// against, rather than a second hardcoded magic-number formula that could
/// drift out of sync with it.
fn whole_graph_candidate_count(elements: &[(NodeKey, String)]) -> usize {
    elements
        .iter()
        .map(|(_, id)| ReferencePosition::candidates_at(id).len())
        .sum()
}

/// Two independent resource limits guard a whole-graph position, at two
/// different layers, and — for this fixture, whose empty `PolicyFilter`
/// never filters a raw candidate out of admission — two different graph
/// sizes: `enumerate` (`utterance-engine`) refuses once the raw candidates it
/// *considers* exceed `MAX_ENUMERATION_CANDIDATES`, before board-membership
/// filtering or admission; `DesignPosition::new` (the pinned contract crate)
/// separately refuses once the *admitted* legal-move set (raw candidates + 1
/// abstention, since nothing here is filtered) exceeds `MAX_LEGAL_MOVES`.
/// Because `MAX_LEGAL_MOVES` (512) is far tighter than
/// `MAX_ENUMERATION_CANDIDATES` (4096), it is what actually binds first as
/// this graph grows — discovered by this target crashing on an assertion
/// that only modeled the enumeration-layer cap. Both are real, reachable
/// zones the fixture's `% 340` task-count range now spans, and are asserted
/// as functionally distinct outcomes, not folded into "any resource limit".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedWholeGraphOutcome {
    Admitted,
    EnumerationLimit,
    LegalMoveLimit,
}

fn expected_whole_graph_outcome(elements: &[(NodeKey, String)]) -> ExpectedWholeGraphOutcome {
    let considered = whole_graph_candidate_count(elements);
    let admitted_estimate = considered + 1; // +1 framework abstention
    if considered > MAX_ENUMERATION_CANDIDATES {
        ExpectedWholeGraphOutcome::EnumerationLimit
    } else if admitted_estimate > MAX_LEGAL_MOVES {
        ExpectedWholeGraphOutcome::LegalMoveLimit
    } else {
        ExpectedWholeGraphOutcome::Admitted
    }
}

fn assert_matches_expected_outcome(
    result: Result<DesignPosition, BpmnBoardError>,
    expected: ExpectedWholeGraphOutcome,
) -> Option<DesignPosition> {
    match (result, expected) {
        (Ok(position), ExpectedWholeGraphOutcome::Admitted) => Some(position),
        (Ok(_), other) => panic!("expected {other:?} but the position was admitted"),
        (
            Err(BpmnBoardError::ResourceLimit(_)),
            ExpectedWholeGraphOutcome::EnumerationLimit,
        ) => None,
        (
            Err(BpmnBoardError::Gameboard(GameboardContractError::ResourceLimitExceeded {
                field: "design position legal moves",
                ..
            })),
            ExpectedWholeGraphOutcome::LegalMoveLimit,
        ) => None,
        (Err(error), expected) => {
            panic!("expected {expected:?} but got a different/unexpected refusal: {error:?}")
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // MAX_LEGAL_MOVES (512) is what actually binds for this fixture — its
    // empty PolicyFilter never filters a raw candidate before admission, so
    // admitted legal moves track raw candidates ~1:1, and MAX_LEGAL_MOVES is
    // far tighter than MAX_ENUMERATION_CANDIDATES (4096). `% 64` (needs ~40
    // task nodes to cross 512 admitted moves) reaches that zone with a
    // fast, single-byte-selected graph. MAX_ENUMERATION_CANDIDATES needs
    // ~316 task nodes to reach on its own — this fixture's per-iteration
    // cost is superlinear in node count (it reconstructs a board+position at
    // every anchor, once per fuzz call), making that zone impractically slow
    // for a fuzzing campaign (~5s/iteration at 339 nodes, measured). Left
    // unreached here; already independently, deterministically verified by
    // `legal_moves::tests::enumeration_amplification_beyond_the_limit_is_a_typed_resource_limit_refusal`
    // (a single-construction 600-node fixture with no such multiplier).
    let task_count = data.first().copied().unwrap_or_default() as usize % 64;
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
        None => DesignFocus::absent(FocusAbsenceReason::NotProvided),
    };

    // A whole-graph (unanchored) position walks every anchor's raw candidate
    // set; past either resource limit this is a legitimate, typed refusal
    // (Gate 8 bullet 4), not a bug to unwrap through. An anchored position
    // never risks this — `enumerate` only considers that one anchor.
    let expected_outcome = if anchor.is_none() {
        expected_whole_graph_outcome(&elements)
    } else {
        ExpectedWholeGraphOutcome::Admitted
    };

    let position_result = build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &"b".repeat(64),
        "compiler-fuzz-v1",
        &"c".repeat(64),
        focus.clone(),
        None,
    );
    let Some(first) = assert_matches_expected_outcome(position_result, expected_outcome) else {
        return;
    };
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

    let whole_graph_outcome = expected_whole_graph_outcome(&elements);
    for index in 0..=elements.len() {
        let focus_anchor = (index < elements.len()).then(|| &elements[index]);
        // Only the final (unanchored, whole-graph) iteration can hit either
        // resource limit; every anchored iteration considers exactly one
        // anchor's small, fixed candidate set.
        if focus_anchor.is_none() && whole_graph_outcome != ExpectedWholeGraphOutcome::Admitted {
            let board = build_bpmn_semantic_board(&dag, None, &revision, &PolicyFilter::default())
                .unwrap();
            let position_result = build_bpmn_design_position(
                &dag,
                &board,
                &revision,
                &"b".repeat(64),
                "compiler-fuzz-v1",
                &"c".repeat(64),
                DesignFocus::absent(FocusAbsenceReason::NotProvided),
                None,
            );
            assert!(
                assert_matches_expected_outcome(position_result, whole_graph_outcome).is_none(),
                "expected a refusal, not an admitted position"
            );
            continue;
        }
        let board = build_bpmn_semantic_board(
            &dag,
            focus_anchor.map(|(key, id)| (*key, id.as_str())),
            &revision,
            &PolicyFilter::default(),
        )
        .unwrap();
        let focus = match focus_anchor {
            Some((_, id)) => DesignFocus::element(GraphElementRef::new(id).unwrap()),
            None => DesignFocus::absent(FocusAbsenceReason::NotProvided),
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
