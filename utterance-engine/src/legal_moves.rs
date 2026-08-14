//! Private deterministic legal-move enumeration and graph preview.

use bpmn_lite_compiler::{ConditionExpr, ConditionLiteral, ConditionOp, IRNode, TimerSpec};
use bpmn_lite_types::{DataObjectRole, DataObjectType, PrimitiveType};
use designer_graph::board_candidate::{CandidateId, LegalityOracle};
use designer_graph::ops::{GuardTrigger, Operation, RegionBranch};
use designer_graph::positional::PositionalLegality;
use designer_graph::productions::apply_production;
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
use semantic_decision_contracts::{
    ApplicabilityFact, ApplicabilityState, CanonicalCandidateId, ExplanationParameter,
    GraphContentHash, GraphDeltaOperation, GraphDeltaPreview, GraphElementRef, LegalMove,
    MoveArgument, ProposalStatus, ProposalWorkbook, RuleCode, RuleExplanation, SlotValue,
    SlotValueState, GAMEBOARD_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

use crate::bpmn_board::{BpmnBoardError, BpmnBoundProposal, BpmnWorkbookPreview, ResourceLimitExceeded};
use crate::bpmn_pack::{candidate_spec, rule_source, BpmnCandidateSpec, APPLICABILITY_RULE_CODE};
use crate::game_state::BpmnGameState;

const ANCHOR_PROVENANCE: &str = "bpmn.position.anchor.v1";

/// Maximum anchor x candidate pairs `enumerate` will consider before
/// refusing. Bounds enumeration amplification and the expensive per-candidate
/// compiler preview work that follows each one, independent of the tighter
/// `MAX_LEGAL_MOVES` cap the contract layer applies to the resulting set.
/// Public so fuzz targets (a separate crate) can drive a graph past this
/// bound deliberately instead of guessing/hardcoding it — see
/// `utterance-engine/fuzz/fuzz_targets/legal_move_enumeration.rs`.
pub const MAX_ENUMERATION_CANDIDATES: usize = 4096;

pub(crate) const MATERIALIZED_CANDIDATE_IDS: &[&str] = &[
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
    "op.create_data_object",
    "prod.request_and_wait",
    "prod.reminder_then_escalate",
    "prod.interrupting_timeout",
    "prod.non_interrupting_notification",
];

pub(crate) struct EnumeratedMoves {
    pub(crate) admitted: Vec<LegalMove>,
    pub(crate) compiler_refused: Vec<CompilerRefusal>,
}

pub(crate) struct CompilerRefusal {
    pub(crate) candidate_id: String,
    pub(crate) anchor: String,
    pub(crate) diagnostics: Vec<String>,
}

pub(crate) fn enumerate(
    state: &BpmnGameState<'_>,
    graph_hash: &GraphContentHash,
) -> Result<EnumeratedMoves, BpmnBoardError> {
    let oracle = PositionalLegality { dag: state.dag() };
    let mut admitted = Vec::new();
    let mut compiler_refused = Vec::new();
    let mut considered = 0_usize;

    for (key, bpmn_id) in state.anchors()? {
        for raw in oracle.legal_candidates(Some(&key)) {
            considered += 1;
            if considered > MAX_ENUMERATION_CANDIDATES {
                return Err(ResourceLimitExceeded {
                    field: "legal move enumeration candidates",
                    limit: MAX_ENUMERATION_CANDIDATES,
                    actual: considered,
                }
                .into());
            }
            if state.board().candidate(raw.id.canonical_id()).is_none() {
                continue;
            }
            let spec = candidate_spec(raw.id).ok_or(BpmnBoardError::MissingSemanticContract {
                canonical_id: raw.id.canonical_id(),
            })?;
            let move_result = position_bound_move(state, raw.id, &spec, key, &bpmn_id, graph_hash)?;
            match move_result {
                PreviewAdmission::Admitted(legal_move) => admitted.push(*legal_move),
                PreviewAdmission::CompilerRefused(refusal) => compiler_refused.push(refusal),
            }
        }
    }

    if let Some(abstain) = state
        .board()
        .candidate(semantic_decision_contracts::ABSTENTION_CANDIDATE_ID)
    {
        admitted.push(LegalMove::new(
            GAMEBOARD_SCHEMA_VERSION,
            abstain.canonical_id.clone(),
            state.board().graph_revision.clone(),
            false,
            None,
            Vec::new(),
            vec![ApplicabilityFact::new(
                RuleCode::new("semantic.gameboard.abstain")?,
                ApplicabilityState::Applicable,
                None,
                state.board().semantic_snapshot.as_str(),
            )?],
            None,
        )?);
    }

    compiler_refused.sort_by(|left, right| {
        (&left.candidate_id, &left.anchor).cmp(&(&right.candidate_id, &right.anchor))
    });
    Ok(EnumeratedMoves {
        admitted,
        compiler_refused,
    })
}

enum PreviewAdmission {
    Admitted(Box<LegalMove>),
    CompilerRefused(CompilerRefusal),
}

fn position_bound_move(
    state: &BpmnGameState<'_>,
    candidate: CandidateId,
    spec: &BpmnCandidateSpec,
    key: NodeKey,
    bpmn_id: &str,
    graph_hash: &GraphContentHash,
) -> Result<PreviewAdmission, BpmnBoardError> {
    let anchor_argument = spec
        .semantic
        .arguments
        .iter()
        .find(|argument| {
            argument.required
                && argument.kind == semantic_decision_contracts::ArgumentKind::NodeReference
        })
        .map(|argument| argument.name.as_str());
    let arguments = spec
        .semantic
        .arguments
        .iter()
        .map(|argument| {
            let binding = (Some(argument.name.as_str()) == anchor_argument).then(|| {
                (
                    SlotValue::NodeReference(bpmn_id.to_string()),
                    ANCHOR_PROVENANCE.to_string(),
                )
            });
            MoveArgument::new(
                &argument.name,
                argument.kind,
                argument.required,
                binding.as_ref().map(|(value, _)| value.clone()),
                binding.map(|(_, provenance)| provenance),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let explanation = applicability_explanation(spec, state.board().semantic_snapshot.as_str())?;
    let applicability = vec![ApplicabilityFact::new(
        explanation.rule_code().clone(),
        if arguments
            .iter()
            .any(|argument| argument.required() && argument.value().is_none())
        {
            ApplicabilityState::Incomplete
        } else {
            ApplicabilityState::Applicable
        },
        Some(explanation.explanation_id().clone()),
        state.board().semantic_snapshot.as_str(),
    )?];

    let preview = if arguments
        .iter()
        .all(|argument| !argument.required() || argument.value().is_some())
    {
        let operation = materialize_anchor_complete(candidate, key).ok_or_else(|| {
            BpmnBoardError::MissingMutationImplementation {
                canonical_id: candidate.canonical_id().to_string(),
            }
        })?;
        match preview_operations(
            state.dag(),
            candidate.canonical_id(),
            bpmn_id,
            graph_hash,
            vec![operation],
        )? {
            Ok(preview) => Some(preview),
            Err(diagnostics) => {
                return Ok(PreviewAdmission::CompilerRefused(CompilerRefusal {
                    candidate_id: candidate.canonical_id().to_string(),
                    anchor: bpmn_id.to_string(),
                    diagnostics,
                }));
            }
        }
    } else {
        None
    };

    Ok(PreviewAdmission::Admitted(Box::new(LegalMove::new(
        GAMEBOARD_SCHEMA_VERSION,
        CanonicalCandidateId::new(candidate.canonical_id())?,
        state.board().graph_revision.clone(),
        true,
        Some(GraphElementRef::new(bpmn_id)?),
        arguments,
        applicability,
        preview,
    )?)))
}

pub(crate) fn applicability_explanation(
    spec: &BpmnCandidateSpec,
    semantic_snapshot: &str,
) -> Result<RuleExplanation, BpmnBoardError> {
    let source = rule_source(APPLICABILITY_RULE_CODE);
    RuleExplanation::new(
        GAMEBOARD_SCHEMA_VERSION,
        source.rule_code.clone(),
        source.message_key.clone(),
        vec![
            ExplanationParameter::new("candidate", spec.semantic.canonical_id.as_str())?,
            ExplanationParameter::new("rule", spec.semantic.applicability.clone())?,
        ],
        semantic_snapshot,
        source.disclosure,
    )
    .map_err(BpmnBoardError::from)
}

/// Only candidates whose pack-declared required values are all supplied by the
/// current position reach this function. Other operation-specific materializers
/// are exercised after typed workbook binding, never with invented values.
fn materialize_anchor_complete(candidate: CandidateId, key: NodeKey) -> Option<Operation> {
    match candidate.canonical_id() {
        "op.delete_subgraph" => Some(Operation::DeleteNode { target: key }),
        _ => None,
    }
}

pub(crate) fn preview_operations(
    dag: &DesignerDag,
    candidate_id: &str,
    anchor: &str,
    graph_hash: &GraphContentHash,
    operations: Vec<Operation>,
) -> Result<Result<GraphDeltaPreview, Vec<String>>, BpmnBoardError> {
    let staged = match apply_production(dag, operations.clone(), Provenance::default()) {
        Ok(staged) => staged,
        Err(error) => return Ok(Err(vec![error.to_string()])),
    };
    if let Err(errors) = staged.candidate.admit() {
        return Ok(Err(errors.into_iter().map(|error| error.message).collect()));
    }

    let effects = operations
        .iter()
        .enumerate()
        .map(|(index, operation)| {
            let payload = serde_json::to_vec(operation)
                .map_err(|error| BpmnBoardError::PreviewEncoding(error.to_string()))?;
            let payload_hash = GraphContentHash::new(lower_hex(&Sha256::digest(payload)))?;
            GraphDeltaOperation::new(
                format!("{candidate_id}.{index}"),
                Some(GraphElementRef::new(anchor)?),
                payload_hash,
            )
            .map_err(BpmnBoardError::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    GraphDeltaPreview::new(GAMEBOARD_SCHEMA_VERSION, graph_hash.clone(), effects)
        .map(Ok)
        .map_err(BpmnBoardError::from)
}

struct DeterministicIdentities {
    seed: Vec<u8>,
    next: u64,
}

impl DeterministicIdentities {
    fn for_workbook(workbook: &ProposalWorkbook) -> Self {
        let mut seed = Vec::new();
        put_seed(&mut seed, workbook.workbook_id.as_str());
        put_seed(&mut seed, workbook.candidate_id.as_str());
        put_seed(&mut seed, workbook.graph_revision.as_str());
        for slot in workbook.slots() {
            put_seed(&mut seed, &slot.name);
            put_seed(
                &mut seed,
                &serde_json::to_string(&slot.value)
                    .expect("shared admitted slot values must serialize"),
            );
        }
        Self { seed, next: 0 }
    }

    fn next_key(&mut self) -> NodeKey {
        let mut hasher = Sha256::new();
        hasher.update(b"bpmn.gameboard.identity.v1");
        hasher.update((self.seed.len() as u64).to_be_bytes());
        hasher.update(&self.seed);
        hasher.update(self.next.to_be_bytes());
        self.next += 1;
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        NodeKey(uuid::Uuid::from_bytes(bytes))
    }
}

fn put_seed(buffer: &mut Vec<u8>, value: &str) {
    buffer.extend_from_slice(&(value.len() as u64).to_be_bytes());
    buffer.extend_from_slice(value.as_bytes());
}

fn task(name: &str) -> IRNode {
    IRNode::ServiceTask {
        id: name.to_string(),
        name: name.to_string(),
        task_type: "noop".to_string(),
        loop_origin: None,
    }
}

fn end_node(id: String) -> IRNode {
    IRNode::End {
        id,
        terminate: false,
    }
}

fn value<'a>(workbook: &'a ProposalWorkbook, name: &str) -> Result<&'a SlotValue, BpmnBoardError> {
    workbook
        .slots()
        .iter()
        .find(|slot| slot.name == name)
        .and_then(|slot| match &slot.value {
            SlotValueState::Resolved(value) => Some(value),
            SlotValueState::Missing => None,
        })
        .ok_or_else(|| BpmnBoardError::Binding(format!("slot '{name}' is unresolved")))
}

fn optional_value<'a>(workbook: &'a ProposalWorkbook, name: &str) -> Option<&'a SlotValue> {
    workbook
        .slots()
        .iter()
        .find(|slot| slot.name == name)
        .and_then(|slot| match &slot.value {
            SlotValueState::Resolved(value) => Some(value),
            SlotValueState::Missing => None,
        })
}

fn identifier(workbook: &ProposalWorkbook, name: &str) -> Result<String, BpmnBoardError> {
    match value(workbook, name)? {
        SlotValue::Identifier(value) => Ok(value.clone()),
        _ => Err(BpmnBoardError::Binding(format!(
            "slot '{name}' is not an identifier"
        ))),
    }
}

fn node_key(
    dag: &DesignerDag,
    workbook: &ProposalWorkbook,
    name: &str,
) -> Result<NodeKey, BpmnBoardError> {
    match value(workbook, name)? {
        SlotValue::NodeReference(value) => dag.key_for_bpmn_id(value).ok_or_else(|| {
            BpmnBoardError::Binding(format!("node reference '{value}' no longer exists"))
        }),
        _ => Err(BpmnBoardError::Binding(format!(
            "slot '{name}' is not a node reference"
        ))),
    }
}

fn data_reference(workbook: &ProposalWorkbook, name: &str) -> Result<String, BpmnBoardError> {
    match value(workbook, name)? {
        SlotValue::DataReference(value) => Ok(value.clone()),
        _ => Err(BpmnBoardError::Binding(format!(
            "slot '{name}' is not a data reference"
        ))),
    }
}

fn count(workbook: &ProposalWorkbook, name: &str) -> Result<u32, BpmnBoardError> {
    match value(workbook, name)? {
        SlotValue::Count(value) => Ok(*value),
        _ => Err(BpmnBoardError::Binding(format!(
            "slot '{name}' is not a count"
        ))),
    }
}

fn text(workbook: &ProposalWorkbook, name: &str) -> Result<String, BpmnBoardError> {
    match value(workbook, name)? {
        SlotValue::Text(value) => Ok(value.clone()),
        _ => Err(BpmnBoardError::Binding(format!(
            "slot '{name}' is not text"
        ))),
    }
}

/// G7.3 (F-G7a ruling): the four proposal.rs-canonicalised primitive-type
/// words; anything else is a bug upstream in `primitive_type_word`, not a
/// user-facing case (proposal.rs never resolves the slot to any other
/// string).
fn primitive_data_object_type(word: &str) -> Result<DataObjectType, BpmnBoardError> {
    match word {
        "bool" => Ok(DataObjectType::Primitive(PrimitiveType::Bool)),
        "integer" => Ok(DataObjectType::Primitive(PrimitiveType::I64)),
        "decimal" => Ok(DataObjectType::Primitive(PrimitiveType::F64)),
        "string" => Ok(DataObjectType::Primitive(PrimitiveType::String)),
        other => Err(BpmnBoardError::Binding(format!(
            "slot 'data_type' has an unrecognised primitive type word '{other}'"
        ))),
    }
}

fn duration(workbook: &ProposalWorkbook, name: &str) -> Result<u64, BpmnBoardError> {
    match value(workbook, name)? {
        SlotValue::DurationMillis(value) => Ok(*value),
        _ => Err(BpmnBoardError::Binding(format!(
            "slot '{name}' is not a duration"
        ))),
    }
}

fn parse_condition(value: &str) -> Result<ConditionExpr, BpmnBoardError> {
    let value = value.trim();
    for (operator_text, operator) in [
        ("!=", ConditionOp::Neq),
        ("==", ConditionOp::Eq),
        (">", ConditionOp::Gt),
        ("<", ConditionOp::Lt),
    ] {
        if let Some((flag, literal)) = value.split_once(operator_text) {
            let flag = flag.trim();
            let mut chars = flag.chars();
            let valid = matches!(chars.next(), Some(first) if first.is_alphabetic() || first == '_')
                && chars.all(|character| {
                    character.is_alphanumeric() || character == '_' || character == '-'
                });
            if !valid {
                return Err(BpmnBoardError::Binding(format!(
                    "condition flag '{flag}' is not an identifier"
                )));
            }
            let literal = match literal.trim() {
                "true" => ConditionLiteral::Bool(true),
                "false" => ConditionLiteral::Bool(false),
                integer => ConditionLiteral::I64(integer.parse().map_err(|_| {
                    BpmnBoardError::Binding(format!(
                        "condition literal '{integer}' is not boolean or integer"
                    ))
                })?),
            };
            return Ok(ConditionExpr {
                flag_name: flag.to_string(),
                op: operator,
                literal,
            });
        }
    }
    Err(BpmnBoardError::Binding(
        "condition must use flag==value, flag!=value, flag>integer or flag<integer".to_string(),
    ))
}

pub(crate) fn materialize_workbook(
    dag: &DesignerDag,
    workbook: &ProposalWorkbook,
) -> Result<BpmnBoundProposal, BpmnBoardError> {
    if !matches!(
        workbook.status(),
        ProposalStatus::ReadyForDryRun | ProposalStatus::ReadyForRatification
    ) {
        return Err(BpmnBoardError::WorkbookNotReady {
            status: workbook.status(),
        });
    }
    let mut ids = DeterministicIdentities::for_workbook(workbook);
    let candidate = workbook.candidate_id.as_str();
    if !MATERIALIZED_CANDIDATE_IDS.contains(&candidate) {
        return Err(BpmnBoardError::MissingMutationImplementation {
            canonical_id: candidate.to_string(),
        });
    }
    let (operations, description) = match candidate {
        "op.append_node" | "op.insert_after" | "op.insert_before" | "op.replace_node" => {
            let slot = if candidate == "op.replace_node" {
                "replacement"
            } else {
                "node"
            };
            let name = identifier(workbook, slot)?;
            let key = ids.next_key();
            let node = task(&name);
            let operation = match candidate {
                "op.append_node" => Operation::AppendNode {
                    anchor: node_key(dag, workbook, "anchor")?,
                    key,
                    node,
                    edge_id: format!("flow_{name}"),
                },
                "op.insert_after" => Operation::InsertAfter {
                    anchor: node_key(dag, workbook, "anchor")?,
                    key,
                    node,
                    edge_id: format!("flow_{name}"),
                },
                "op.insert_before" => Operation::InsertBefore {
                    anchor: node_key(dag, workbook, "anchor")?,
                    key,
                    node,
                    edge_id: format!("flow_{name}"),
                },
                _ => Operation::ReplaceNode {
                    target: node_key(dag, workbook, "target")?,
                    key,
                    node,
                },
            };
            let description = match candidate {
                "op.append_node" => format!("append service task '{name}'"),
                "op.insert_after" => format!("insert service task '{name}' after the selection"),
                "op.insert_before" => {
                    format!("insert service task '{name}' before the selection")
                }
                _ => format!("replace the selection with service task '{name}'"),
            };
            (vec![operation], description)
        }
        "op.connect" => {
            let condition = match optional_value(workbook, "condition") {
                Some(SlotValue::Condition(value)) => Some(parse_condition(value)?),
                Some(_) => {
                    return Err(BpmnBoardError::Binding(
                        "slot 'condition' has the wrong kind".to_string(),
                    ));
                }
                None => None,
            };
            (
                vec![Operation::Connect {
                    from: node_key(dag, workbook, "from")?,
                    to: node_key(dag, workbook, "to")?,
                    edge_id: format!("flow_{}", workbook.workbook_id.as_str()),
                    condition,
                }],
                "connect the two declared existing nodes".to_string(),
            )
        }
        "op.create_branch" => {
            let outcome = identifier(workbook, "outcome")?;
            (
                vec![Operation::CreateBranch {
                    gateway: node_key(dag, workbook, "gateway")?,
                    target: node_key(dag, workbook, "target")?,
                    edge_id: format!("flow_{outcome}"),
                    condition: Some(ConditionExpr {
                        flag_name: outcome.clone(),
                        op: ConditionOp::Eq,
                        literal: ConditionLiteral::Bool(true),
                    }),
                }],
                format!("create the '{outcome}' outcome branch"),
            )
        }
        "op.create_parallel_region" => {
            let branch_count = count(workbook, "branch_count")?;
            if !(2..=32).contains(&branch_count) {
                return Err(BpmnBoardError::Binding(
                    "parallel branch_count must be in 2..=32".to_string(),
                ));
            }
            let branches = (1..=branch_count)
                .map(|index| {
                    let name = format!("parallel_branch_{index}");
                    RegionBranch {
                        key: ids.next_key(),
                        node: task(&name),
                        in_edge_id: format!("flow_in_{name}"),
                        out_edge_id: format!("flow_out_{name}"),
                        condition: None,
                    }
                })
                .collect();
            let base = workbook.workbook_id.as_str();
            (
                vec![Operation::CreateParallelRegion {
                    anchor: node_key(dag, workbook, "anchor")?,
                    fork_key: ids.next_key(),
                    fork_node_id: format!("parallel_{base}_fork"),
                    join_key: ids.next_key(),
                    join_node_id: format!("parallel_{base}_join"),
                    entry_edge_id: format!("flow_parallel_{base}"),
                    branches,
                }],
                format!("create {branch_count} parallel branches"),
            )
        }
        "op.create_inclusive_region" => {
            let branch_count = count(workbook, "branch_count")?;
            if !(2..=32).contains(&branch_count) {
                return Err(BpmnBoardError::Binding(
                    "inclusive branch_count must be in 2..=32".to_string(),
                ));
            }
            let conditions = match value(workbook, "conditions")? {
                SlotValue::Condition(value) => value
                    .split([',', ';'])
                    .map(parse_condition)
                    .collect::<Result<Vec<_>, _>>()?,
                _ => {
                    return Err(BpmnBoardError::Binding(
                        "slot 'conditions' has the wrong kind".to_string(),
                    ));
                }
            };
            if conditions.len() != branch_count as usize {
                return Err(BpmnBoardError::Binding(format!(
                    "inclusive region declares {branch_count} branches but {} conditions",
                    conditions.len()
                )));
            }
            let branches = conditions
                .into_iter()
                .enumerate()
                .map(|(index, condition)| {
                    let name = format!("inclusive_{}_{}", condition.flag_name, index + 1);
                    RegionBranch {
                        key: ids.next_key(),
                        node: task(&name),
                        in_edge_id: format!("flow_in_{name}"),
                        out_edge_id: format!("flow_out_{name}"),
                        condition: Some(condition),
                    }
                })
                .collect();
            let base = workbook.workbook_id.as_str();
            (
                vec![Operation::CreateInclusiveRegion {
                    anchor: node_key(dag, workbook, "anchor")?,
                    fork_key: ids.next_key(),
                    fork_node_id: format!("inclusive_{base}_fork"),
                    join_key: ids.next_key(),
                    join_node_id: format!("inclusive_{base}_join"),
                    entry_edge_id: format!("flow_inclusive_{base}"),
                    branches,
                }],
                format!("create {branch_count} conditional inclusive branches"),
            )
        }
        "op.create_multi_instance_region" => {
            let collection = data_reference(workbook, "collection")?;
            let maximum = count(workbook, "declared_max")?;
            let name = format!("for_each_{collection}");
            (
                vec![Operation::CreateMultiInstanceRegion {
                    anchor: node_key(dag, workbook, "anchor")?,
                    key: ids.next_key(),
                    node: IRNode::MultiInstance {
                        id: name.clone(),
                        name: name.clone(),
                        task_type: "noop".to_string(),
                        collection_flag_name: collection.clone(),
                        declared_max: maximum,
                        inputs: Vec::new(),
                    },
                    edge_id: format!("flow_{name}"),
                }],
                format!("repeat over '{collection}' up to {maximum} times"),
            )
        }
        "op.create_data_object" => {
            let name = identifier(workbook, "name")?;
            let type_decl = primitive_data_object_type(&text(workbook, "data_type")?)?;
            (
                vec![Operation::CreateDataObject {
                    key: ids.next_key(),
                    id: name.clone(),
                    name: name.clone(),
                    type_decl,
                    role: DataObjectRole::Internal,
                }],
                format!("declare a new data object '{name}'"),
            )
        }
        "op.attach_guard" => {
            let escape = identifier(workbook, "escape")?;
            let guard_key = ids.next_key();
            (
                vec![
                    Operation::AttachGuard {
                        host: node_key(dag, workbook, "host")?,
                        key: guard_key,
                        guard_id: format!("{escape}_guard"),
                        trigger: GuardTrigger::Timer(TimerSpec::Duration {
                            ms: duration(workbook, "trigger")?,
                        }),
                    },
                    Operation::AppendNode {
                        anchor: guard_key,
                        key: ids.next_key(),
                        node: task(&escape),
                        edge_id: format!("flow_{escape}"),
                    },
                ],
                format!("attach an interrupting guard leading to '{escape}'"),
            )
        }
        "op.attach_rearming_guard" => {
            let escape = identifier(workbook, "escape")?;
            let guard_key = ids.next_key();
            (
                vec![
                    Operation::AttachRearmingGuard {
                        host: node_key(dag, workbook, "host")?,
                        key: guard_key,
                        guard_id: format!("{escape}_guard"),
                        trigger: GuardTrigger::Timer(TimerSpec::Cycle {
                            interval_ms: duration(workbook, "interval")?,
                            max_fires: count(workbook, "max_fires")?,
                        }),
                    },
                    Operation::AppendNode {
                        anchor: guard_key,
                        key: ids.next_key(),
                        node: task(&escape),
                        edge_id: format!("flow_{escape}"),
                    },
                ],
                format!("attach a bounded rearming guard leading to '{escape}'"),
            )
        }
        "op.set_guard_trigger" => (
            vec![Operation::SetGuardTrigger {
                guard: node_key(dag, workbook, "guard")?,
                trigger: GuardTrigger::Timer(TimerSpec::Duration {
                    ms: duration(workbook, "trigger")?,
                }),
            }],
            "set the selected guard timer".to_string(),
        ),
        "op.set_guard_budget" => {
            let budget = count(workbook, "failure_budget")?;
            (
                vec![Operation::SetGuardBudget {
                    guard: node_key(dag, workbook, "guard")?,
                    failure_budget: Some(budget),
                }],
                format!("set guard failure budget to {budget}"),
            )
        }
        "op.set_correlation_source" => {
            let data = data_reference(workbook, "data_reference")?;
            (
                vec![Operation::SetCorrelationSource {
                    node: node_key(dag, workbook, "node")?,
                    corr_key_source: data.clone(),
                }],
                format!("set correlation source to '{data}'"),
            )
        }
        "op.delete_subgraph" => (
            vec![Operation::DeleteNode {
                target: node_key(dag, workbook, "target")?,
            }],
            "delete the selected node or closed region".to_string(),
        ),
        "prod.request_and_wait" => {
            let request = identifier(workbook, "request")?;
            let wait = format!("{request}_response");
            let correlation = data_reference(workbook, "correlation_source")?;
            (
                designer_graph::productions::request_and_wait(
                    designer_graph::productions::RequestAndWaitBindings {
                        anchor: node_key(dag, workbook, "anchor")?,
                        send_key: ids.next_key(),
                        send_node: task(&request),
                        send_edge_id: format!("flow_{request}"),
                        wait_key: ids.next_key(),
                        wait_node: IRNode::MessageWait {
                            id: wait.clone(),
                            name: wait.clone(),
                            corr_key_source: correlation.clone(),
                        },
                        wait_edge_id: format!("flow_{wait}"),
                    },
                ),
                format!("send '{request}' and wait for a response correlated on '{correlation}'"),
            )
        }
        "prod.interrupting_timeout" => {
            let escape = identifier(workbook, "escape")?;
            (
                designer_graph::productions::interrupting_timeout(
                    designer_graph::productions::InterruptingTimeoutBindings {
                        anchor: node_key(dag, workbook, "anchor")?,
                        guard_key: ids.next_key(),
                        guard_id: format!("{escape}_guard"),
                        duration_ms: duration(workbook, "duration")?,
                        continuation_key: ids.next_key(),
                        continuation_node: task(&escape),
                        continuation_edge_id: format!("flow_{escape}"),
                        continuation_end_key: ids.next_key(),
                        continuation_end_node: end_node(format!("{escape}_end")),
                        continuation_end_edge_id: format!("flow_{escape}_end"),
                    },
                ),
                format!("interrupt on timeout and continue to '{escape}'"),
            )
        }
        "prod.reminder_then_escalate" | "prod.non_interrupting_notification" => {
            let (name_slot, count_slot) = if candidate == "prod.reminder_then_escalate" {
                ("escalation", "max_reminders")
            } else {
                ("notification", "max_fires")
            };
            let name = identifier(workbook, name_slot)?;
            let interval = duration(workbook, "interval")?;
            let maximum = count(workbook, count_slot)?;
            let operations = if candidate == "prod.reminder_then_escalate" {
                designer_graph::productions::reminder_then_escalate(
                    designer_graph::productions::ReminderThenEscalateBindings {
                        anchor: node_key(dag, workbook, "anchor")?,
                        guard_key: ids.next_key(),
                        guard_id: format!("{name}_guard"),
                        cycle: TimerSpec::Cycle {
                            interval_ms: interval,
                            max_fires: maximum,
                        },
                        escalation_key: ids.next_key(),
                        escalation_node: task(&name),
                        escalation_edge_id: format!("flow_{name}"),
                        escalation_end_key: ids.next_key(),
                        escalation_end_node: end_node(format!("{name}_end")),
                        escalation_end_edge_id: format!("flow_{name}_end"),
                    },
                )
            } else {
                designer_graph::productions::non_interrupting_notification(
                    designer_graph::productions::NonInterruptingNotificationBindings {
                        anchor: node_key(dag, workbook, "anchor")?,
                        guard_key: ids.next_key(),
                        guard_id: format!("{name}_guard"),
                        interval_ms: interval,
                        max_fires: maximum,
                        notification_key: ids.next_key(),
                        notification_node: task(&name),
                        notification_edge_id: format!("flow_{name}"),
                        notification_end_key: ids.next_key(),
                        notification_end_node: end_node(format!("{name}_end")),
                        notification_end_edge_id: format!("flow_{name}_end"),
                    },
                )
            };
            (
                operations,
                format!("run '{name}' every {interval}ms up to {maximum} times"),
            )
        }
        other => {
            return Err(BpmnBoardError::MissingMutationImplementation {
                canonical_id: other.to_string(),
            });
        }
    };
    Ok(BpmnBoundProposal::from_parts(operations, description))
}

pub(crate) fn preview_workbook(
    dag: &DesignerDag,
    workbook: &ProposalWorkbook,
    current_graph_revision: &str,
    graph_hash: &GraphContentHash,
) -> Result<BpmnWorkbookPreview, BpmnBoardError> {
    if workbook.graph_revision.as_str() != current_graph_revision {
        return Err(BpmnBoardError::StaleWorkbook {
            workbook_revision: workbook.graph_revision.as_str().to_string(),
            current_revision: current_graph_revision.to_string(),
        });
    }
    let bound = materialize_workbook(dag, workbook)?;
    let anchor = workbook
        .slots()
        .iter()
        .find_map(|slot| match &slot.value {
            SlotValueState::Resolved(SlotValue::NodeReference(value)) => Some(value.as_str()),
            _ => None,
        })
        .unwrap_or("whole_graph");
    let delta = match preview_operations(
        dag,
        workbook.candidate_id.as_str(),
        anchor,
        graph_hash,
        bound.operations().to_vec(),
    )? {
        Ok(delta) => delta,
        Err(diagnostics) => {
            return Err(BpmnBoardError::CompilerRefused {
                candidate_id: workbook.candidate_id.as_str().to_string(),
                diagnostics,
            });
        }
    };
    Ok(BpmnWorkbookPreview::new(bound, delta))
}

fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use bpmn_lite_compiler::IRNode;
    use designer_graph::ops::{apply, Operation};
    use designer_graph::schema::Provenance;
    use semantic_decision_contracts::{GraphContentHash, MoveBindingState};
    use uuid::Uuid;

    use super::*;
    use crate::board::PolicyFilter;
    use crate::bpmn_board::build_bpmn_semantic_board;

    fn fixture() -> (DesignerDag, NodeKey) {
        let start = NodeKey(Uuid::from_u128(1));
        let task = NodeKey(Uuid::from_u128(2));
        let mut dag = DesignerDag::new("phase2");
        dag.seed(
            start,
            IRNode::Start { id: "start".into() },
            Provenance::default(),
        )
        .unwrap();
        let dag = apply(
            &dag,
            Operation::AppendNode {
                anchor: start,
                key: task,
                node: IRNode::ServiceTask {
                    id: "task".into(),
                    name: "Task".into(),
                    task_type: "noop".into(),
                    loop_origin: None,
                },
                edge_id: "flow_task".into(),
            },
            Provenance::default(),
        )
        .unwrap()
        .candidate;
        (dag, task)
    }

    #[test]
    fn position_binds_known_anchor_and_leaves_semantic_values_missing() {
        let (dag, task) = fixture();
        let board = build_bpmn_semantic_board(
            &dag,
            Some((task, "task")),
            &"a".repeat(64),
            &PolicyFilter::default(),
        )
        .unwrap();
        let state = BpmnGameState::new(&dag, &board).unwrap();
        let moves = enumerate(&state, &GraphContentHash::new("b".repeat(64)).unwrap()).unwrap();
        let insert = moves
            .admitted
            .iter()
            .find(|legal_move| legal_move.candidate_id().as_str() == "op.insert_after")
            .unwrap();
        assert_eq!(insert.anchor().unwrap().as_str(), "task");
        assert!(matches!(
            insert.binding_state(),
            MoveBindingState::Incomplete { .. }
        ));
        assert_eq!(
            insert
                .arguments()
                .iter()
                .find(|argument| argument.name() == "anchor")
                .unwrap()
                .value(),
            Some(&SlotValue::NodeReference("task".to_string()))
        );
        assert!(insert.preview().is_none());
    }

    #[test]
    fn deterministic_reconstruction_preserves_move_ids() {
        let (dag, task) = fixture();
        let board = build_bpmn_semantic_board(
            &dag,
            Some((task, "task")),
            &"a".repeat(64),
            &PolicyFilter::default(),
        )
        .unwrap();
        let graph_hash = GraphContentHash::new("b".repeat(64)).unwrap();
        let first = enumerate(&BpmnGameState::new(&dag, &board).unwrap(), &graph_hash).unwrap();
        let second = enumerate(&BpmnGameState::new(&dag, &board).unwrap(), &graph_hash).unwrap();
        assert_eq!(first.admitted, second.admitted);
        assert_eq!(
            first
                .compiler_refused
                .iter()
                .map(|item| (&item.candidate_id, &item.anchor, &item.diagnostics))
                .collect::<Vec<_>>(),
            second
                .compiler_refused
                .iter()
                .map(|item| (&item.candidate_id, &item.anchor, &item.diagnostics))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn enumeration_amplification_beyond_the_limit_is_a_typed_resource_limit_refusal() {
        let start = NodeKey(Uuid::from_u128(1));
        let mut dag = DesignerDag::new("phase2-amplification");
        dag.seed(
            start,
            IRNode::Start { id: "start".into() },
            Provenance::default(),
        )
        .unwrap();
        let mut anchor = start;
        for index in 0..600_u128 {
            let key = NodeKey(Uuid::from_u128(1000 + index));
            dag = apply(
                &dag,
                Operation::AppendNode {
                    anchor,
                    key,
                    node: IRNode::ServiceTask {
                        id: format!("task_{index}"),
                        name: format!("Task {index}"),
                        task_type: "noop".into(),
                        loop_origin: None,
                    },
                    edge_id: format!("flow_{index}"),
                },
                Provenance::default(),
            )
            .unwrap()
            .candidate;
            anchor = key;
        }
        // Whole-graph (unanchored) board: enumerate walks every node, so this
        // large a graph is the amplification path the limit exists to bound.
        let board =
            build_bpmn_semantic_board(&dag, None, &"a".repeat(64), &PolicyFilter::default())
                .unwrap();
        let state = BpmnGameState::new(&dag, &board).unwrap();
        let result = enumerate(&state, &GraphContentHash::new("b".repeat(64)).unwrap());
        let error = match result {
            Ok(_) => panic!("600 nodes must exceed the enumeration amplification limit"),
            Err(error) => error,
        };
        assert!(matches!(
            &error,
            BpmnBoardError::ResourceLimit(limit)
                if limit.field == "legal move enumeration candidates"
                    && limit.limit == MAX_ENUMERATION_CANDIDATES
        ));

        // Session usable afterwards: a small, legitimate graph still enumerates.
        let (small_dag, task) = fixture();
        let small_board = build_bpmn_semantic_board(
            &small_dag,
            Some((task, "task")),
            &"a".repeat(64),
            &PolicyFilter::default(),
        )
        .unwrap();
        let small_state = BpmnGameState::new(&small_dag, &small_board).unwrap();
        assert!(
            enumerate(&small_state, &GraphContentHash::new("b".repeat(64)).unwrap()).is_ok()
        );
    }
}
