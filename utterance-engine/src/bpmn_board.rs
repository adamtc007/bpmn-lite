//! Position-legal BPMN semantic decision-board construction.

use designer_graph::board_candidate::LegalityOracle;
use designer_graph::positional::PositionalLegality;
use designer_graph::schema::{DesignerDag, NodeKey};
use sem_os_policy::decision_board::{
    DecisionBoardError, DomainIdentity, GraphRevision, ResolvedPosition, SemanticDecisionBoard,
};
use thiserror::Error;

use crate::board::PolicyFilter;
use crate::bpmn_pack::{candidate_spec, semantic_snapshot_identity, BinderSupport};

/// Errors returned while binding a shared decision board to a BPMN position.
#[derive(Debug, Error)]
pub enum BpmnBoardError {
    /// The public BPMN id and stable node key do not identify the same node.
    #[error("anchor '{bpmn_id}' does not resolve to the supplied Designer node key")]
    InvalidAnchor { bpmn_id: String },
    /// A shared semantic invariant rejected the adapter output.
    #[error(transparent)]
    Shared(#[from] DecisionBoardError),
}

/// Build the complete model-visible semantic board at a resolved BPMN position.
///
/// The function applies positional legality and policy before semantic content
/// becomes model-visible, excludes catalogue entries without a deterministic
/// representation, and delegates canonicalization and hashing to the shared
/// SemOS board constructor.
///
/// # Examples
///
/// ```
/// use designer_graph::schema::DesignerDag;
/// use utterance_engine::board::PolicyFilter;
/// use utterance_engine::bpmn_board::build_bpmn_semantic_board;
///
/// let dag = DesignerDag::new("empty");
/// let board = build_bpmn_semantic_board(&dag, None, "revision-1", &PolicyFilter::default())
///     .unwrap();
/// assert_eq!(board.candidates.len(), 1); // framework abstention
/// ```
pub fn build_bpmn_semantic_board(
    dag: &DesignerDag,
    anchor: Option<(NodeKey, &str)>,
    graph_revision: &str,
    policy: &PolicyFilter,
) -> Result<SemanticDecisionBoard, BpmnBoardError> {
    if let Some((key, bpmn_id)) = anchor {
        if dag.key_for_bpmn_id(bpmn_id) != Some(key) {
            return Err(BpmnBoardError::InvalidAnchor {
                bpmn_id: bpmn_id.to_string(),
            });
        }
    }

    let oracle = PositionalLegality { dag };
    let mut candidates = Vec::new();
    for raw in oracle.legal_candidates(anchor.as_ref().map(|(key, _)| key)) {
        let Some(spec) = candidate_spec(raw.id) else {
            continue;
        };
        if spec.binder_support == BinderSupport::NotRepresentable
            || policy.denied.contains(spec.semantic.canonical_id.as_str())
        {
            continue;
        }
        candidates.push(spec.semantic);
    }

    SemanticDecisionBoard::new(
        1,
        DomainIdentity::new("bpmn.designer")?,
        semantic_snapshot_identity(),
        GraphRevision::new(graph_revision)?,
        ResolvedPosition {
            anchor: anchor.map(|(_, bpmn_id)| bpmn_id.to_string()),
            context_hash: position_context_hash(anchor),
        },
        candidates,
        policy_fingerprint(policy),
    )
    .map_err(BpmnBoardError::from)
}

fn position_context_hash(anchor: Option<(NodeKey, &str)>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"bpmn-position-v1");
    match anchor {
        Some((key, bpmn_id)) => {
            hasher.update(b"some");
            hasher.update(key.0.as_bytes());
            put(&mut hasher, bpmn_id);
        }
        None => {
            hasher.update(b"none");
        }
    }
    hasher.finalize().to_hex().to_string()
}

fn policy_fingerprint(policy: &PolicyFilter) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"bpmn-policy-filter-v1");
    hasher.update(&(policy.denied.len() as u64).to_le_bytes());
    for denied in &policy.denied {
        put(&mut hasher, denied);
    }
    hasher.finalize().to_hex().to_string()
}

fn put(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use bpmn_lite_compiler::IRNode;
    use designer_graph::ops::{apply, Operation};
    use designer_graph::schema::Provenance;
    use sem_os_policy::decision_board::ABSTENTION_CANDIDATE_ID;
    use uuid::Uuid;

    use super::*;

    fn fixture() -> (DesignerDag, NodeKey, NodeKey) {
        let start = NodeKey(Uuid::from_u128(1));
        let task = NodeKey(Uuid::from_u128(2));
        let mut dag = DesignerDag::new("semantic-board-fixture");
        dag.seed(
            start,
            IRNode::Start { id: "start".into() },
            Provenance::default(),
        )
        .unwrap();
        let staged = apply(
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
        .unwrap();
        (staged.candidate, start, task)
    }

    #[test]
    fn same_inputs_produce_the_same_board_hash() {
        let (dag, _, task) = fixture();
        let policy = PolicyFilter::default();
        let first =
            build_bpmn_semantic_board(&dag, Some((task, "task-1")), "rev-1", &policy).unwrap();
        let second =
            build_bpmn_semantic_board(&dag, Some((task, "task-1")), "rev-1", &policy).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.board_hash, second.board_hash);
        assert_eq!(
            first.candidates.last().unwrap().canonical_id.as_str(),
            ABSTENTION_CANDIDATE_ID
        );
    }

    #[test]
    fn revision_anchor_and_policy_each_move_the_board_hash() {
        let (dag, start, task) = fixture();
        let base = build_bpmn_semantic_board(
            &dag,
            Some((task, "task-1")),
            "rev-1",
            &PolicyFilter::default(),
        )
        .unwrap();
        let revision = build_bpmn_semantic_board(
            &dag,
            Some((task, "task-1")),
            "rev-2",
            &PolicyFilter::default(),
        )
        .unwrap();
        let anchor = build_bpmn_semantic_board(
            &dag,
            Some((start, "start")),
            "rev-1",
            &PolicyFilter::default(),
        )
        .unwrap();
        let mut denied = PolicyFilter::default();
        denied.denied.insert("op.insert_after".into());
        let policy =
            build_bpmn_semantic_board(&dag, Some((task, "task-1")), "rev-1", &denied).unwrap();

        assert_ne!(base.board_hash, revision.board_hash);
        assert_ne!(base.board_hash, anchor.board_hash);
        assert_ne!(base.board_hash, policy.board_hash);
        assert!(policy.candidate("op.insert_after").is_none());
    }

    #[test]
    fn invalid_anchor_pair_is_refused_instead_of_becoming_whole_graph() {
        let (dag, start, _) = fixture();
        let error = build_bpmn_semantic_board(
            &dag,
            Some((start, "task-1")),
            "rev-1",
            &PolicyFilter::default(),
        )
        .unwrap_err();
        assert!(matches!(error, BpmnBoardError::InvalidAnchor { .. }));
    }

    #[test]
    fn production_board_never_contains_unrepresentable_candidates() {
        let (dag, _, task) = fixture();
        let board = build_bpmn_semantic_board(
            &dag,
            Some((task, "task-1")),
            "rev-1",
            &PolicyFilter::default(),
        )
        .unwrap();
        for id in [
            "op.create_race",
            "op.close_parallel_region",
            "op.attach_rollback_guard",
            "op.call_subprocess",
            "prod.timer_message_race",
            "prod.human_review_with_rework",
            "prod.call_durable_subprocess",
        ] {
            assert!(board.candidate(id).is_none(), "{id} leaked onto the board");
        }
    }
}
