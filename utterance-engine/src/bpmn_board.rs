//! Position-legal BPMN semantic decision-board construction.

use designer_graph::board_candidate::{CandidateId, LegalityOracle};
use designer_graph::ops::Operation;
use designer_graph::positional::PositionalLegality;
use designer_graph::schema::{DesignerDag, NodeKey};
use semantic_decision_contracts::{
    ApplicabilityState, BoardPath, CandidateSemanticSlice, DecisionBoardError, DesignBelief,
    DesignFocus, DesignPosition, DisclosureClass, DomainIdentity, EvidenceLane, FeedbackOption,
    GameDomainId, GameboardContractError, GraphContentHash, GraphDeltaPreview, GraphRevision,
    HistoryHash, LegalMoveId, MoveAttemptId, MoveAttemptOutcome, MoveAttemptReceipt,
    ProposalWorkbook, ResolvedPosition, RuleExplanation, SemanticDecisionBoard,
    GAMEBOARD_SCHEMA_VERSION,
};
use thiserror::Error;

use crate::board::PolicyFilter;
use crate::bpmn_pack::{
    candidate_spec, candidate_spec_by_canonical_id, compiled_semantic_pack, feedback_source,
    rule_source, semantic_snapshot_identity, BinderSupport, POLICY_HIDDEN_RULE_CODE,
};
use crate::game_state::BpmnGameState;
use crate::legal_moves::applicability_explanation;
use crate::legal_moves::{enumerate, materialize_workbook, preview_workbook};

const MAX_RECOVERY_OPTIONS: usize = 16;

/// Deterministically materialized operations for one fully bound workbook.
#[derive(Clone, Debug)]
pub struct BpmnBoundProposal {
    operations: Vec<Operation>,
    description: String,
}

impl BpmnBoundProposal {
    pub(crate) fn from_parts(operations: Vec<Operation>, description: String) -> Self {
        Self {
            operations,
            description,
        }
    }

    /// Exact operation tape supplied to preview and, after ratification, apply.
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }

    /// Adapter description of the proposed change.
    pub fn description(&self) -> &str {
        &self.description
    }
}

/// A compiler-admitted, non-mutating preview paired with its exact operation tape.
#[derive(Clone, Debug)]
pub struct BpmnWorkbookPreview {
    bound: BpmnBoundProposal,
    delta: GraphDeltaPreview,
}

impl BpmnWorkbookPreview {
    pub(crate) fn new(bound: BpmnBoundProposal, delta: GraphDeltaPreview) -> Self {
        Self { bound, delta }
    }

    /// Exact deterministic operations which produced the preview.
    pub fn bound(&self) -> &BpmnBoundProposal {
        &self.bound
    }

    /// Canonical graph delta admitted against a clone of the supplied graph.
    pub fn delta(&self) -> &GraphDeltaPreview {
        &self.delta
    }
}

/// Governed explanation and currently legal recovery choices for one requested
/// candidate shape.
#[derive(Clone, Debug)]
pub struct BpmnMoveGuidance {
    applicability: ApplicabilityState,
    explanation: RuleExplanation,
    recoveries: Vec<FeedbackOption>,
}

impl BpmnMoveGuidance {
    /// Deterministic classification of the requested shape.
    pub fn applicability(&self) -> ApplicabilityState {
        self.applicability
    }

    /// Pack-derived typed explanation; callers never parse an error string.
    pub fn explanation(&self) -> &RuleExplanation {
        &self.explanation
    }

    /// Bounded options linked only to the current legal move set or focus change.
    pub fn recoveries(&self) -> &[FeedbackOption] {
        &self.recoveries
    }
}

/// Errors returned while binding a shared decision board to a BPMN position.
#[derive(Debug, Error)]
pub enum BpmnBoardError {
    /// The public BPMN id and stable node key do not identify the same node.
    #[error("anchor '{bpmn_id}' does not resolve to the supplied Designer node key")]
    InvalidAnchor { bpmn_id: String },
    /// `PositionalLegality` returned a candidate the semantic registry has
    /// no spec for. Every `OperationKind`/`ProductionId` is required to
    /// have exactly one entry (`bpmn_pack::validate_registry_coverage`
    /// enforces this at snapshot construction), so reaching this state
    /// means the registry and the legality oracle have fallen out of sync
    /// — fail closed with the offending id rather than silently shrinking
    /// the board by one candidate.
    #[error(
        "legal candidate '{canonical_id}' has no BPMN semantic contract; the registry and PositionalLegality have drifted out of sync"
    )]
    MissingSemanticContract { canonical_id: &'static str },
    /// A shared semantic invariant rejected the adapter output.
    #[error(transparent)]
    Shared(#[from] DecisionBoardError),
    /// A reusable gameboard invariant rejected a compatibility projection.
    #[error(transparent)]
    Gameboard(#[from] GameboardContractError),
    /// A semantic board still names an anchor absent from the supplied graph.
    #[error("semantic board anchor '{bpmn_id}' is stale for the supplied graph")]
    StaleBoardAnchor { bpmn_id: String },
    /// The semantic board was enumerated at an earlier graph revision.
    #[error(
        "stale semantic board revision '{board_revision}'; current revision is '{current_revision}'"
    )]
    StaleBoardRevision {
        board_revision: String,
        current_revision: String,
    },
    /// The authoritative Designer graph could not be projected for enumeration.
    #[error("Designer graph projection failed: {0}")]
    GraphProjection(String),
    /// A fully bound executable candidate has no operation implementation.
    #[error("candidate '{canonical_id}' has no deterministic mutation implementation")]
    MissingMutationImplementation { canonical_id: String },
    /// A canonical preview payload could not be encoded.
    #[error("graph preview encoding failed: {0}")]
    PreviewEncoding(String),
    /// Bounded history or belief evidence could not be projected.
    #[error("gameboard continuity projection failed: {0}")]
    Continuity(String),
    /// A workbook has not completed its typed binding transition.
    #[error("proposal workbook is not ready for materialization: {status:?}")]
    WorkbookNotReady {
        status: semantic_decision_contracts::ProposalStatus,
    },
    /// A typed workbook binding is absent, stale or outside its admitted range.
    #[error("BPMN binding refused: {0}")]
    Binding(String),
    /// Staging or compiler admission refused a fully bound preview.
    #[error("compiler preview refused candidate '{candidate_id}': {diagnostics:?}")]
    CompilerRefused {
        candidate_id: String,
        diagnostics: Vec<String>,
    },
    /// The workbook was bound against a different authoritative graph revision.
    #[error(
        "stale workbook revision '{workbook_revision}'; current revision is '{current_revision}'"
    )]
    StaleWorkbook {
        workbook_revision: String,
        current_revision: String,
    },
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
        if let Some(semantic) = map_legal_candidate(raw.id, policy)? {
            candidates.push(semantic);
        }
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

/// Project the production semantic board into the reusable gameboard position
/// contract without changing legality, disposition, workbook or mutation flow.
///
/// Authority-bearing values absent from the legacy board are mandatory inputs;
/// this adapter never synthesizes them. The board domain is retained as one
/// opaque path segment so the shared mechanism does not interpret BPMN naming.
#[allow(clippy::too_many_arguments)]
pub fn project_design_position(
    board: &SemanticDecisionBoard,
    graph_hash: &str,
    compiler_profile: &str,
    history_hash: &str,
    focus: DesignFocus,
    current_proposal_hash: Option<&str>,
) -> Result<DesignPosition, BpmnBoardError> {
    DesignPosition::from_semantic_board(
        board,
        BoardPath::new(vec![board.domain.as_str().to_string()])?,
        GraphContentHash::new(graph_hash)?,
        compiler_profile,
        board.policy_fingerprint.clone(),
        HistoryHash::new(history_hash)?,
        focus,
        current_proposal_hash
            .map(GraphContentHash::new)
            .transpose()?,
    )
    .map_err(BpmnBoardError::from)
}

/// Build the concrete, position-bound gameboard from the same admitted semantic
/// board used by the language path. Enumeration is pure over explicit inputs;
/// complete moves are staged on a clone and compiler-admitted before inclusion.
#[allow(clippy::too_many_arguments)]
pub fn build_bpmn_design_position(
    dag: &DesignerDag,
    board: &SemanticDecisionBoard,
    current_graph_revision: &str,
    graph_hash: &str,
    compiler_profile: &str,
    history_hash: &str,
    focus: DesignFocus,
    current_proposal_hash: Option<&str>,
) -> Result<DesignPosition, BpmnBoardError> {
    if board.graph_revision.as_str() != current_graph_revision {
        return Err(BpmnBoardError::StaleBoardRevision {
            board_revision: board.graph_revision.as_str().to_string(),
            current_revision: current_graph_revision.to_string(),
        });
    }
    let graph_hash = GraphContentHash::new(graph_hash)?;
    let state = BpmnGameState::new(dag, board)?;
    let moves = enumerate(&state, &graph_hash)?;
    for refusal in &moves.compiler_refused {
        tracing::debug!(
            candidate_id = refusal.candidate_id,
            anchor = refusal.anchor,
            diagnostics = ?refusal.diagnostics,
            "compiler-refused concrete move excluded from the legal move set"
        );
    }
    DesignPosition::new(
        GAMEBOARD_SCHEMA_VERSION,
        GameDomainId::new(board.domain.as_str())?,
        BoardPath::new(vec![board.domain.as_str().to_string()])?,
        board.semantic_snapshot.clone(),
        board.graph_revision.clone(),
        graph_hash,
        compiler_profile,
        board.policy_fingerprint.clone(),
        current_proposal_hash
            .map(GraphContentHash::new)
            .transpose()?,
        focus,
        HistoryHash::new(history_hash)?,
        moves.admitted,
    )
    .map_err(BpmnBoardError::from)
}

/// Attach complete, finite evidence to every legal move and project the same
/// fused record back to the existing candidate-level policy input. Evidence can
/// rank a move but can neither add nor remove legal moves.
pub fn finalize_bpmn_move_evidence(
    board: &SemanticDecisionBoard,
    position: &DesignPosition,
    utterance: &str,
    result: crate::contract::SlmResult,
    active_lane: EvidenceLane,
    bundle_identities: Vec<String>,
    attempts: &[MoveAttemptReceipt],
) -> anyhow::Result<crate::contract::SlmResult> {
    crate::fusion::fuse_move_evidence(
        board,
        position,
        utterance,
        result,
        active_lane,
        bundle_identities,
        attempts,
    )
}

/// Validate and content-address the bounded attempt window supplied by the
/// application composition root. The returned receipts are the exact evidence
/// window used by ranking; neither the projection nor its hash grants authority.
pub fn project_bpmn_attempt_history(
    attempts: &[MoveAttemptReceipt],
) -> Result<(String, Vec<MoveAttemptReceipt>), BpmnBoardError> {
    let projection = crate::history::project(attempts)
        .map_err(|error| BpmnBoardError::Continuity(error.to_string()))?;
    Ok((
        projection.hash().as_str().to_string(),
        projection.attempts().to_vec(),
    ))
}

/// Record one terminal attempt using governed explanation and recovery
/// resources from the admitted BPMN semantic pack. No graph state is touched.
#[allow(clippy::too_many_arguments)]
pub fn record_bpmn_attempt(
    position: &DesignPosition,
    attempt_id: MoveAttemptId,
    attempted_move: Option<LegalMoveId>,
    observed_intent: &str,
    outcome: MoveAttemptOutcome,
    correction_of: Option<MoveAttemptId>,
    correction_kind: Option<semantic_decision_contracts::CorrectionKind>,
) -> Result<MoveAttemptReceipt, BpmnBoardError> {
    crate::history::receipt(
        position.semantic_snapshot().as_str(),
        attempt_id,
        position.state_id().clone(),
        attempted_move,
        observed_intent,
        outcome,
        correction_of,
        correction_kind,
    )
    .map_err(|error| BpmnBoardError::Continuity(error.to_string()))
}

/// Produce a bounded non-authoritative belief snapshot from the current legal
/// move evidence, governed motifs, graph facts and retained attempt window.
pub fn update_bpmn_design_belief(
    dag: &DesignerDag,
    position: &DesignPosition,
    evidence: &[semantic_decision_contracts::MoveEvidence],
    attempts: &[MoveAttemptReceipt],
    previous: Option<&DesignBelief>,
) -> Result<DesignBelief, BpmnBoardError> {
    crate::belief::update(dag, position, evidence, attempts, previous)
        .map_err(|error| BpmnBoardError::Continuity(error.to_string()))
}

/// Select one deterministic, position-bound game interaction from complete move
/// evidence and bounded belief. Models and belief remain evidence-only; the
/// returned packet cannot add a legal move or mutate the graph.
pub fn decide_bpmn_game_disposition(
    board: &SemanticDecisionBoard,
    position: &DesignPosition,
    evidence: &[semantic_decision_contracts::MoveEvidence],
    belief: &DesignBelief,
    observed_intent: &str,
    attempt_id: MoveAttemptId,
    attempts: &[MoveAttemptReceipt],
) -> Result<semantic_decision_contracts::GameDisposition, BpmnBoardError> {
    crate::disposition::decide_game(
        board,
        position,
        evidence,
        belief,
        observed_intent,
        attempt_id,
        attempts,
    )
    .map_err(|error| BpmnBoardError::Continuity(error.to_string()))
}

/// Render only admitted pack text referenced by a game disposition. The host
/// supplies layout; it does not invent domain wording, rules or recovery policy.
pub fn render_bpmn_game_disposition(
    disposition: &semantic_decision_contracts::GameDisposition,
    board: &SemanticDecisionBoard,
    position: &DesignPosition,
) -> Result<String, BpmnBoardError> {
    disposition.validate_for_position(position)?;
    let pack = compiled_semantic_pack();
    let mut messages = Vec::new();
    if let Some(prompt) = disposition.governed_prompt() {
        messages.push(prompt.to_string());
    }
    for move_id in disposition.selected_moves() {
        if let Some(legal_move) = position
            .legal_moves()
            .iter()
            .find(|legal_move| legal_move.move_id() == move_id)
        {
            if let Some(candidate) = board.candidate(legal_move.candidate_id().as_str()) {
                messages.push(candidate.effect.clone());
            }
        }
    }
    if let Some(receipt) = disposition.attempt_receipt() {
        for explanation_id in receipt.rule_explanations() {
            for source in pack.rule_explanations() {
                let explanation = RuleExplanation::new(
                    GAMEBOARD_SCHEMA_VERSION,
                    source.rule_code.clone(),
                    source.message_key.clone(),
                    Vec::new(),
                    board.semantic_snapshot.as_str(),
                    source.disclosure,
                )?;
                if explanation.explanation_id() == explanation_id {
                    messages.push(source.message.clone());
                }
            }
        }
        for option in receipt.feedback_options() {
            if let Some(source) = pack
                .feedback_options()
                .iter()
                .find(|source| source.prompt_key == *option.prompt_key())
            {
                messages.push(source.prompt.clone());
            }
        }
    }
    messages.retain(|message| !message.trim().is_empty());
    let mut seen = std::collections::BTreeSet::new();
    messages.retain(|message| seen.insert(message.clone()));
    if messages.is_empty() {
        return Err(BpmnBoardError::Continuity(
            "game disposition has no admitted renderable resource".to_string(),
        ));
    }
    Ok(messages.join("\n"))
}

/// Materialize a complete typed workbook with deterministic content-derived graph
/// identities. This function does not mutate the supplied graph.
pub fn materialize_bpmn_workbook(
    dag: &DesignerDag,
    workbook: &ProposalWorkbook,
) -> Result<BpmnBoundProposal, BpmnBoardError> {
    materialize_workbook(dag, workbook)
}

/// Materialize, dry-apply to a clone, and pass through the production compiler
/// admission boundary. The authoritative graph is never mutated.
pub fn preview_bpmn_workbook(
    dag: &DesignerDag,
    workbook: &ProposalWorkbook,
    current_graph_revision: &str,
    graph_hash: &str,
) -> Result<BpmnWorkbookPreview, BpmnBoardError> {
    preview_workbook(
        dag,
        workbook,
        current_graph_revision,
        &GraphContentHash::new(graph_hash)?,
    )
}

/// Resolve a direct graph operation to the same governed move identity when the
/// current position contains an exactly equivalent concrete move. Operations that
/// require additional semantic bindings return `None` until represented by a fully
/// bound move.
pub fn bpmn_legal_move_id_for_operation(
    dag: &DesignerDag,
    position: &DesignPosition,
    operation: &Operation,
) -> Option<LegalMoveId> {
    let (candidate_id, key) = match operation {
        Operation::DeleteNode { target } => ("op.delete_subgraph", *target),
        _ => return None,
    };
    let ir = dag.to_ir().ok()?;
    let node_id = ir.node_weights().find_map(|node| {
        (dag.key_for_bpmn_id(node.id()) == Some(key)).then(|| node.id().to_string())
    })?;
    position
        .legal_moves()
        .iter()
        .find(|legal_move| {
            legal_move.candidate_id().as_str() == candidate_id
                && legal_move
                    .anchor()
                    .is_some_and(|anchor| anchor.as_str() == node_id)
                && legal_move.binding_state().is_complete()
        })
        .map(|legal_move| legal_move.move_id().clone())
}

/// Explain why a known candidate does or does not fit this position and return
/// bounded recovery choices from the current legal move set. Policy-hidden
/// candidates receive a disclosure-safe explanation which does not name the piece.
pub fn explain_bpmn_candidate(
    board: &SemanticDecisionBoard,
    position: &DesignPosition,
    candidate_id: &str,
    policy: &PolicyFilter,
) -> Result<BpmnMoveGuidance, BpmnBoardError> {
    let spec = candidate_spec_by_canonical_id(candidate_id).ok_or_else(|| {
        BpmnBoardError::MissingMutationImplementation {
            canonical_id: candidate_id.to_string(),
        }
    })?;
    let policy_hidden = policy.denied.contains(candidate_id);
    let has_legal_shape = position
        .legal_moves()
        .iter()
        .any(|legal_move| legal_move.candidate_id().as_str() == candidate_id);
    let applicability = if policy_hidden {
        ApplicabilityState::PolicyHidden
    } else if has_legal_shape {
        if position.legal_moves().iter().any(|legal_move| {
            legal_move.candidate_id().as_str() == candidate_id
                && !legal_move.binding_state().is_complete()
        }) {
            ApplicabilityState::Incomplete
        } else {
            ApplicabilityState::Applicable
        }
    } else {
        ApplicabilityState::Inapplicable
    };
    let explanation = if policy_hidden {
        let source = rule_source(POLICY_HIDDEN_RULE_CODE);
        RuleExplanation::new(
            GAMEBOARD_SCHEMA_VERSION,
            source.rule_code.clone(),
            source.message_key.clone(),
            Vec::new(),
            board.semantic_snapshot.as_str(),
            source.disclosure,
        )?
    } else {
        applicability_explanation(&spec, board.semantic_snapshot.as_str())?
    };

    let mut recoveries = position
        .legal_moves()
        .iter()
        .filter(|legal_move| {
            legal_move.candidate_id().as_str()
                != semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
                && legal_move.candidate_id().as_str() != candidate_id
        })
        .take(MAX_RECOVERY_OPTIONS)
        .filter_map(|legal_move| {
            candidate_spec_by_canonical_id(legal_move.candidate_id().as_str())?;
            let source = feedback_source("recovery.select_alternative");
            Some(FeedbackOption::new(
                source.kind,
                Some(legal_move.move_id().clone()),
                source.prompt_key.clone(),
                Some(explanation.explanation_id().clone()),
                source.disclosure,
            ))
        })
        .collect::<Vec<_>>();
    if recoveries.is_empty() {
        let source = feedback_source("recovery.change_focus");
        recoveries.push(FeedbackOption::new(
            source.kind,
            None,
            source.prompt_key.clone(),
            Some(explanation.explanation_id().clone()),
            if policy_hidden {
                DisclosureClass::PolicyHidden
            } else {
                source.disclosure
            },
        ));
    }
    Ok(BpmnMoveGuidance {
        applicability,
        explanation,
        recoveries,
    })
}

/// Map one position-legal raw candidate id to its model-visible semantic
/// slice, or `Ok(None)` if it is deliberately excluded from the board.
///
/// Two distinct outcomes share this function's `None`-adjacent shape, and
/// they must not be conflated:
///
/// - **`Err(MissingSemanticContract)`**: the registry has no spec for this
///   id at all. `PositionalLegality` and the semantic registry are supposed
///   to be drawn from the exact same `designer-graph` catalogue
///   (`bpmn_pack::validate_registry_coverage` enforces 26/26 coverage at
///   snapshot construction), so this can only happen if the two have
///   drifted out of sync — a defect, not a legitimate exclusion. Fail
///   closed rather than silently shrinking the board by one candidate.
/// - **`Ok(None)`**: the id has a real spec, but is deliberately withheld —
///   either `BinderSupport::NotRepresentable` (invariant 13's ratified
///   deviation: the binder/engine cannot execute these seven actions yet,
///   so the mapper must not offer them even though they are position-legal
///   and semantically described; see the programme doc and CO-05) or
///   policy-denied for this position.
fn map_legal_candidate(
    id: CandidateId,
    policy: &PolicyFilter,
) -> Result<Option<CandidateSemanticSlice>, BpmnBoardError> {
    let spec = candidate_spec(id).ok_or(BpmnBoardError::MissingSemanticContract {
        canonical_id: id.canonical_id(),
    })?;
    if spec.binder_support == BinderSupport::NotRepresentable
        || policy.denied.contains(spec.semantic.canonical_id.as_str())
    {
        return Ok(None);
    }
    Ok(Some(spec.semantic))
}

/// Immutable identity required by model bundle compatibility cards.
pub fn bpmn_semantic_snapshot_identity() -> String {
    semantic_snapshot_identity().as_str().to_string()
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
    put(&mut hasher, &crate::disposition::game_policy_identity());
    hasher.finalize().to_hex().to_string()
}

fn put(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use bpmn_lite_compiler::IRNode;
    use bpmn_lite_types::{DataObjectRole, DataObjectType, PrimitiveType};
    use designer_graph::board_candidate::OperationKind;
    use designer_graph::ops::{apply, Operation};
    use designer_graph::schema::Provenance;
    use semantic_decision_contracts::{
        DesignBelief, FiniteScore, FocusAbsenceReason, GameDispositionKind, GraphElementRef,
        MoveAttemptId, MoveEvidence, ProducerIdentity, ABSTENTION_CANDIDATE_ID,
    };
    use uuid::Uuid;

    use super::*;
    use crate::board::{build_board, EmptyUniverse};
    use crate::policy::{decide, DispositionConfig};
    use crate::retrieval::{LexicalTier0, Tier0Retriever};

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

    fn controlled_evidence(
        position: &DesignPosition,
        scores: &std::collections::BTreeMap<LegalMoveId, f64>,
    ) -> Vec<MoveEvidence> {
        position
            .legal_moves()
            .iter()
            .map(|legal_move| {
                let score = scores.get(legal_move.move_id()).copied().unwrap_or(0.05);
                MoveEvidence::new(
                    GAMEBOARD_SCHEMA_VERSION,
                    legal_move.move_id().clone(),
                    Vec::new(),
                    FiniteScore::new(score).unwrap(),
                    FiniteScore::new(score).unwrap(),
                    Vec::new(),
                    ProducerIdentity::new("test.controlled-evidence.v1").unwrap(),
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
            ProducerIdentity::new("test.empty-belief.v1").unwrap(),
        )
        .unwrap()
    }

    fn anchored_game() -> (DesignerDag, SemanticDecisionBoard, DesignPosition) {
        let (dag, _, task) = fixture();
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
            "compiler-profile-v1",
            &"c".repeat(64),
            DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
            None,
        )
        .unwrap();
        (dag, board, position)
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
    fn game_disposition_is_permutation_invariant_and_requires_complete_evidence() {
        let (_, board, position) = anchored_game();
        let selected = position
            .legal_moves()
            .iter()
            .find(|legal_move| legal_move.candidate_id().as_str() != ABSTENTION_CANDIDATE_ID)
            .unwrap()
            .move_id()
            .clone();
        let scores = std::collections::BTreeMap::from([(selected, 0.9)]);
        let evidence = controlled_evidence(&position, &scores);
        let belief = empty_belief(&position);
        let first = decide_bpmn_game_disposition(
            &board,
            &position,
            &evidence,
            &belief,
            "controlled",
            MoveAttemptId::new("attempt-permutation").unwrap(),
            &[],
        )
        .unwrap();
        let mut reversed = evidence.clone();
        reversed.reverse();
        let second = decide_bpmn_game_disposition(
            &board,
            &position,
            &reversed,
            &belief,
            "controlled",
            MoveAttemptId::new("attempt-permutation").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(first, second);

        let mut incomplete = evidence;
        incomplete.pop();
        assert!(decide_bpmn_game_disposition(
            &board,
            &position,
            &incomplete,
            &belief,
            "controlled",
            MoveAttemptId::new("attempt-incomplete").unwrap(),
            &[],
        )
        .is_err());

        let (dag, _, task) = fixture();
        let stale_revision = "9".repeat(64);
        let current_board = build_bpmn_semantic_board(
            &dag,
            Some((task, "task-1")),
            &stale_revision,
            &PolicyFilter::default(),
        )
        .unwrap();
        let current_position = build_bpmn_design_position(
            &dag,
            &current_board,
            &stale_revision,
            &"8".repeat(64),
            "compiler-profile-v1",
            &"7".repeat(64),
            DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
            None,
        )
        .unwrap();
        assert!(decide_bpmn_game_disposition(
            &current_board,
            &current_position,
            &reversed,
            &belief,
            "stale evidence",
            MoveAttemptId::new("attempt-stale-evidence").unwrap(),
            &[],
        )
        .is_err());
    }

    #[test]
    fn governed_clarification_surfaces_the_third_ranked_legal_move() {
        let (_, board, position) = anchored_game();
        let top_three = position
            .legal_moves()
            .iter()
            .filter(|legal_move| legal_move.candidate_id().as_str() != ABSTENTION_CANDIDATE_ID)
            .take(3)
            .map(|legal_move| legal_move.move_id().clone())
            .collect::<Vec<_>>();
        assert_eq!(top_three.len(), 3);
        let scores = std::collections::BTreeMap::from([
            (top_three[0].clone(), 0.55),
            (top_three[1].clone(), 0.54),
            (top_three[2].clone(), 0.53),
        ]);
        let evidence = controlled_evidence(&position, &scores);
        let disposition = decide_bpmn_game_disposition(
            &board,
            &position,
            &evidence,
            &empty_belief(&position),
            "ambiguous fixture",
            MoveAttemptId::new("attempt-third-ranked").unwrap(),
            &[],
        )
        .unwrap();
        assert_eq!(disposition.kind(), GameDispositionKind::ClarifyMoves);
        assert_eq!(disposition.selected_moves().len(), 3);
        assert!(disposition.selected_moves().contains(&top_three[2]));
        assert!(disposition.selected_moves().iter().all(|move_id| {
            position
                .legal_moves()
                .iter()
                .any(|legal_move| legal_move.move_id() == move_id)
        }));
        assert!(disposition.governed_prompt().is_some());
    }

    #[test]
    fn policy_hidden_move_cannot_leak_through_scoring() {
        let (dag, _, task) = fixture();
        let mut filter = PolicyFilter::default();
        filter.denied.insert("op.insert_after".to_string());
        let revision = "d".repeat(64);
        let board =
            build_bpmn_semantic_board(&dag, Some((task, "task-1")), &revision, &filter).unwrap();
        let position = build_bpmn_design_position(
            &dag,
            &board,
            &revision,
            &"e".repeat(64),
            "compiler-profile-v1",
            &"f".repeat(64),
            DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
            None,
        )
        .unwrap();
        assert!(position
            .legal_moves()
            .iter()
            .all(|legal_move| { legal_move.candidate_id().as_str() != "op.insert_after" }));
        let evidence = controlled_evidence(&position, &std::collections::BTreeMap::new());
        let disposition = decide_bpmn_game_disposition(
            &board,
            &position,
            &evidence,
            &empty_belief(&position),
            "hidden move",
            MoveAttemptId::new("attempt-hidden").unwrap(),
            &[],
        )
        .unwrap();
        assert!(disposition.selected_moves().iter().all(|move_id| {
            position
                .legal_moves()
                .iter()
                .any(|legal_move| legal_move.move_id() == move_id)
        }));
    }

    #[test]
    fn legacy_board_projects_to_a_canonical_design_position() {
        let (dag, _, task) = fixture();
        let revision = "a".repeat(64);
        let board = build_bpmn_semantic_board(
            &dag,
            Some((task, "task-1")),
            &revision,
            &PolicyFilter::default(),
        )
        .unwrap();
        let focus = DesignFocus::element(GraphElementRef::new("task-1").unwrap());

        let position = project_design_position(
            &board,
            &"b".repeat(64),
            "compiler-profile-v1",
            &"c".repeat(64),
            focus,
            None,
        )
        .unwrap();

        assert_eq!(position.domain().as_str(), "bpmn.designer");
        assert_eq!(
            position.board_path().segments().collect::<Vec<_>>(),
            vec!["bpmn.designer"]
        );
        assert_eq!(position.graph_revision().as_str(), revision);
        assert_eq!(position.graph_hash().as_str(), "b".repeat(64));
        assert_eq!(position.history_hash().as_str(), "c".repeat(64));
        assert_eq!(position.compiler_profile(), "compiler-profile-v1");
        assert_eq!(position.policy_identity(), board.policy_fingerprint);
        assert_eq!(position.legal_moves().len(), board.candidates.len());
        assert!(position.current_proposal_hash().is_none());

        let wire = serde_json::to_value(&position).unwrap();
        let decoded: DesignPosition = serde_json::from_value(wire).unwrap();
        assert_eq!(decoded, position);
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
        assert_eq!(
            crate::exact::governed_exact(&policy, "insert after"),
            crate::exact::ExactMatch::None,
            "a phrase target removed by policy must not remain in collision analysis"
        );
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
    fn evidence_signed_for_the_legacy_thin_board_is_rejected() {
        let (dag, _, task) = fixture();
        let revision = "revision-1";
        let semantic = build_bpmn_semantic_board(
            &dag,
            Some((task, "task-1")),
            revision,
            &PolicyFilter::default(),
        )
        .unwrap();
        let oracle = PositionalLegality { dag: &dag };
        let legacy = build_board(
            &oracle,
            Some((&task, "task-1")),
            Some(revision),
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .unwrap();
        let evidence = LexicalTier0.retrieve("insert after", &legacy).unwrap();
        let context = crate::context::minimal(semantic.semantic_snapshot.as_str(), revision);

        let error = decide(
            &DispositionConfig::shadow_v1(),
            &semantic,
            &evidence,
            &context,
        )
        .unwrap_err();
        assert!(error.to_string().contains("different board"));
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

    #[test]
    fn missing_semantic_contract_fails_closed_instead_of_shrinking_the_board() {
        // `PositionalLegality` never actually emits `CandidateId::Abstain`
        // (it's owned by the board constructor, not any oracle), which
        // makes it the one real `CandidateId` guaranteed to have no
        // registry spec (`bpmn_pack::candidate_spec` returns `None` for it
        // by construction) -- exactly the "registry and oracle have
        // drifted" shape `map_legal_candidate` must refuse rather than
        // silently drop.
        let error = map_legal_candidate(CandidateId::Abstain, &PolicyFilter::default())
            .expect_err("a candidate id absent from the registry must error, not silently drop");
        assert!(matches!(
            error,
            BpmnBoardError::MissingSemanticContract { canonical_id } if canonical_id == "abstain.none_of_the_above"
        ));
    }

    #[test]
    fn complete_direct_delete_has_one_compiler_admitted_move_identity() {
        let (mut dag, _, task) = fixture();
        let end = NodeKey(Uuid::from_u128(3));
        dag = apply(
            &dag,
            Operation::AppendNode {
                anchor: task,
                key: end,
                node: IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "flow-end".into(),
            },
            Provenance::default(),
        )
        .unwrap()
        .candidate;
        let data = NodeKey(Uuid::from_u128(4));
        dag.seed(
            data,
            IRNode::DataObject {
                id: "case_data".into(),
                name: "Case data".into(),
                type_decl: DataObjectType::Primitive(PrimitiveType::String),
                role: DataObjectRole::Internal,
            },
            Provenance::default(),
        )
        .unwrap();
        dag.admit().unwrap();
        let revision = "a".repeat(64);
        let board =
            build_bpmn_semantic_board(&dag, None, &revision, &PolicyFilter::default()).unwrap();
        let position = build_bpmn_design_position(
            &dag,
            &board,
            &revision,
            &"b".repeat(64),
            "compiler-profile-v1",
            &"c".repeat(64),
            DesignFocus::absent(FocusAbsenceReason::NotProvided, None).unwrap(),
            None,
        )
        .unwrap();
        let deletion = position
            .legal_moves()
            .iter()
            .find(|legal_move| {
                legal_move.candidate_id().as_str() == "op.delete_subgraph"
                    && legal_move
                        .anchor()
                        .is_some_and(|anchor| anchor.as_str() == "case_data")
            })
            .expect("deleting an unreferenced data declaration must admit");
        assert!(deletion.binding_state().is_complete());
        assert!(deletion.preview().is_some());
        assert_eq!(
            bpmn_legal_move_id_for_operation(
                &dag,
                &position,
                &Operation::DeleteNode { target: data }
            )
            .as_ref(),
            Some(deletion.move_id())
        );
        assert_eq!(
            dag.node_count(),
            4,
            "enumeration and preview must not mutate"
        );

        let guidance = explain_bpmn_candidate(
            &board,
            &position,
            "op.create_branch",
            &PolicyFilter::default(),
        )
        .unwrap();
        assert_eq!(guidance.applicability(), ApplicabilityState::Inapplicable);
        assert!(!guidance.recoveries().is_empty());
        assert!(guidance.recoveries().iter().all(|recovery| {
            recovery.move_id().is_none_or(|move_id| {
                position
                    .legal_moves()
                    .iter()
                    .any(|legal_move| legal_move.move_id() == move_id)
            })
        }));

        let mut denied = PolicyFilter::default();
        denied.denied.insert("op.insert_after".to_string());
        let denied_board = build_bpmn_semantic_board(&dag, None, &revision, &denied).unwrap();
        let denied_position = build_bpmn_design_position(
            &dag,
            &denied_board,
            &revision,
            &"b".repeat(64),
            "compiler-profile-v1",
            &"c".repeat(64),
            DesignFocus::absent(FocusAbsenceReason::NotProvided, None).unwrap(),
            None,
        )
        .unwrap();
        let hidden =
            explain_bpmn_candidate(&denied_board, &denied_position, "op.insert_after", &denied)
                .unwrap();
        assert_eq!(hidden.applicability(), ApplicabilityState::PolicyHidden);
        assert_eq!(
            hidden.explanation().disclosure(),
            DisclosureClass::PolicyHidden
        );
        assert!(hidden.recoveries().iter().all(|recovery| {
            recovery.move_id().is_none_or(|move_id| {
                denied_position
                    .legal_moves()
                    .iter()
                    .any(|legal_move| legal_move.move_id() == move_id)
            })
        }));
    }

    #[test]
    fn concrete_positions_cover_empty_multiple_anchor_and_stale_shapes() {
        let empty = DesignerDag::new("empty-gameboard");
        let empty_board =
            build_bpmn_semantic_board(&empty, None, &"a".repeat(64), &PolicyFilter::default())
                .unwrap();
        let empty_position = build_bpmn_design_position(
            &empty,
            &empty_board,
            &"a".repeat(64),
            &"b".repeat(64),
            "compiler-profile-v1",
            &"c".repeat(64),
            DesignFocus::absent(FocusAbsenceReason::NotProvided, None).unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(empty_position.legal_moves().len(), 1);
        assert_eq!(
            empty_position.legal_moves()[0].candidate_id().as_str(),
            ABSTENTION_CANDIDATE_ID
        );

        let (dag, _, task) = fixture();
        let whole_board =
            build_bpmn_semantic_board(&dag, None, &"d".repeat(64), &PolicyFilter::default())
                .unwrap();
        let whole_position = build_bpmn_design_position(
            &dag,
            &whole_board,
            &"d".repeat(64),
            &"e".repeat(64),
            "compiler-profile-v1",
            &"f".repeat(64),
            DesignFocus::absent(FocusAbsenceReason::NotProvided, None).unwrap(),
            None,
        )
        .unwrap();
        let insert_after_anchors = whole_position
            .legal_moves()
            .iter()
            .filter(|legal_move| legal_move.candidate_id().as_str() == "op.insert_after")
            .map(|legal_move| legal_move.anchor().unwrap().as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(insert_after_anchors, BTreeSet::from(["start", "task-1"]));

        let anchored_board = build_bpmn_semantic_board(
            &dag,
            Some((task, "task-1")),
            &"1".repeat(64),
            &PolicyFilter::default(),
        )
        .unwrap();
        let error = build_bpmn_design_position(
            &empty,
            &anchored_board,
            &"1".repeat(64),
            &"2".repeat(64),
            "compiler-profile-v1",
            &"3".repeat(64),
            DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, BpmnBoardError::StaleBoardAnchor { .. }));

        let error = build_bpmn_design_position(
            &dag,
            &anchored_board,
            &"9".repeat(64),
            &"2".repeat(64),
            "compiler-profile-v1",
            &"3".repeat(64),
            DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, BpmnBoardError::StaleBoardRevision { .. }));
    }

    #[test]
    fn not_representable_and_policy_denied_are_silently_excluded_not_errors() {
        // Contrast with the fail-closed case above: a real, registered
        // candidate that is deliberately withheld (NotRepresentable, or
        // policy-denied) must return `Ok(None)`, not an error -- these are
        // the one ratified deviation from invariant 13, not a defect.
        let excluded = map_legal_candidate(
            CandidateId::Operation(OperationKind::CreateRace),
            &PolicyFilter::default(),
        )
        .expect("a NotRepresentable candidate must not error");
        assert!(excluded.is_none());

        let mut denied = PolicyFilter::default();
        denied.denied.insert("op.append_node".into());
        let excluded =
            map_legal_candidate(CandidateId::Operation(OperationKind::AppendNode), &denied)
                .expect("a policy-denied candidate must not error");
        assert!(excluded.is_none());
    }

    #[test]
    fn belief_replays_and_rejection_reduces_evidence_without_changing_legality() {
        use semantic_decision_contracts::{EvidenceLane, MoveAttemptId, MoveAttemptOutcome};

        let (dag, _, task) = fixture();
        let revision = "4".repeat(64);
        let board = build_bpmn_semantic_board(
            &dag,
            Some((task, "task-1")),
            &revision,
            &PolicyFilter::default(),
        )
        .unwrap();
        let (history_hash, history) = project_bpmn_attempt_history(&[]).unwrap();
        let position = build_bpmn_design_position(
            &dag,
            &board,
            &revision,
            &"5".repeat(64),
            "compiler-profile-v1",
            &history_hash,
            DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
            None,
        )
        .unwrap();
        let raw = LexicalTier0
            .retrieve("remind then escalate", &board)
            .unwrap();
        let evidence = finalize_bpmn_move_evidence(
            &board,
            &position,
            "remind then escalate",
            raw,
            EvidenceLane::Lexical,
            vec!["test.lexical".into()],
            &history,
        )
        .unwrap();
        let first =
            update_bpmn_design_belief(&dag, &position, &evidence.move_evidence, &history, None)
                .unwrap();
        let replay =
            update_bpmn_design_belief(&dag, &position, &evidence.move_evidence, &history, None)
                .unwrap();
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&replay).unwrap()
        );
        assert!(first
            .motifs()
            .iter()
            .any(|motif| motif.motif_id() == "motif.reminder_then_escalate"));

        let target = position
            .legal_moves()
            .iter()
            .find(|legal_move| legal_move.candidate_id().as_str() == "prod.reminder_then_escalate")
            .unwrap();
        let rejected = record_bpmn_attempt(
            &position,
            MoveAttemptId::new("rejected-reminder").unwrap(),
            Some(target.move_id().clone()),
            "not that",
            MoveAttemptOutcome::RejectedByUser,
            None,
            None,
        )
        .unwrap();
        let (_, history) = project_bpmn_attempt_history(&[rejected]).unwrap();
        let reduced = update_bpmn_design_belief(
            &dag,
            &position,
            &evidence.move_evidence,
            &history,
            Some(&first),
        )
        .unwrap();
        assert!(reduced
            .motifs()
            .iter()
            .any(|motif| motif.motif_id() == "motif.reminder_then_escalate"));
        let probability = |belief: &DesignBelief| {
            belief
                .likely_moves()
                .iter()
                .find(|probability| probability.move_id() == target.move_id())
                .unwrap()
                .probability()
                .get()
        };
        assert!(probability(&reduced) < probability(&first));
        let legal_before = position
            .legal_moves()
            .iter()
            .map(|legal_move| legal_move.move_id())
            .collect::<Vec<_>>();
        drop(reduced);
        assert_eq!(
            legal_before,
            position
                .legal_moves()
                .iter()
                .map(|legal_move| legal_move.move_id())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn legal_unwanted_replacement_and_correction_are_both_replayable() {
        use semantic_decision_contracts::{CorrectionKind, MoveAttemptId, MoveAttemptOutcome};

        let (dag, _, task) = fixture();
        let end = NodeKey(Uuid::from_u128(30));
        let original = apply(
            &dag,
            Operation::AppendNode {
                anchor: task,
                key: end,
                node: IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "flow-end".into(),
            },
            Provenance::default(),
        )
        .unwrap()
        .candidate;
        original.admit().unwrap();
        let revision = "6".repeat(64);
        let board = build_bpmn_semantic_board(
            &original,
            Some((task, "task-1")),
            &revision,
            &PolicyFilter::default(),
        )
        .unwrap();
        let (empty_history, _) = project_bpmn_attempt_history(&[]).unwrap();
        let before = build_bpmn_design_position(
            &original,
            &board,
            &revision,
            &"7".repeat(64),
            "compiler-profile-v1",
            &empty_history,
            DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
            None,
        )
        .unwrap();
        let replacement_move = before
            .legal_moves()
            .iter()
            .find(|legal_move| legal_move.candidate_id().as_str() == "op.replace_node")
            .unwrap()
            .move_id()
            .clone();
        let unwanted_key = NodeKey(Uuid::from_u128(31));
        let unwanted = apply(
            &original,
            Operation::ReplaceNode {
                target: task,
                key: unwanted_key,
                node: IRNode::ServiceTask {
                    id: "task-1".into(),
                    name: "Unwanted".into(),
                    task_type: "wrong".into(),
                },
            },
            Provenance::default(),
        )
        .unwrap()
        .candidate;
        unwanted.admit().unwrap();
        let original_attempt = MoveAttemptId::new("legal-unwanted").unwrap();
        let applied = record_bpmn_attempt(
            &before,
            original_attempt.clone(),
            Some(replacement_move),
            "replace review",
            MoveAttemptOutcome::Applied,
            None,
            None,
        )
        .unwrap();

        let after_board = build_bpmn_semantic_board(
            &unwanted,
            Some((unwanted_key, "task-1")),
            &"8".repeat(64),
            &PolicyFilter::default(),
        )
        .unwrap();
        let (after_history, _) =
            project_bpmn_attempt_history(std::slice::from_ref(&applied)).unwrap();
        let after = build_bpmn_design_position(
            &unwanted,
            &after_board,
            &"8".repeat(64),
            &"9".repeat(64),
            "compiler-profile-v1",
            &after_history,
            DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
            None,
        )
        .unwrap();
        let corrective_move = after
            .legal_moves()
            .iter()
            .find(|legal_move| legal_move.candidate_id().as_str() == "op.replace_node")
            .unwrap()
            .move_id()
            .clone();
        let corrected_graph = apply(
            &unwanted,
            Operation::ReplaceNode {
                target: unwanted_key,
                key: NodeKey(Uuid::from_u128(32)),
                node: IRNode::ServiceTask {
                    id: "task-1".into(),
                    name: "Review".into(),
                    task_type: "review".into(),
                },
            },
            Provenance::default(),
        )
        .unwrap()
        .candidate;
        corrected_graph.admit().unwrap();
        let correction = record_bpmn_attempt(
            &after,
            MoveAttemptId::new("correct-legal-unwanted").unwrap(),
            Some(corrective_move),
            "restore review",
            MoveAttemptOutcome::Corrected,
            Some(original_attempt),
            Some(CorrectionKind::Replacement),
        )
        .unwrap();
        let (_, replayable) = project_bpmn_attempt_history(&[applied, correction]).unwrap();
        assert_eq!(replayable.len(), 2);
        assert_eq!(replayable[0].outcome(), MoveAttemptOutcome::Applied);
        assert_eq!(replayable[1].outcome(), MoveAttemptOutcome::Corrected);
        assert_eq!(
            replayable[1].correction_of(),
            Some(replayable[0].attempt_id())
        );
        let final_task = corrected_graph
            .to_ir()
            .unwrap()
            .node_weights()
            .find_map(|node| match node {
                IRNode::ServiceTask {
                    id,
                    name,
                    task_type,
                } if id == "task-1" => Some((name.clone(), task_type.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(final_task, ("Review".into(), "review".into()));
    }
}
