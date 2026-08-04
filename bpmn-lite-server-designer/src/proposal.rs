//! Deterministic, resumable proposal workbooks.
//!
//! Inference selects a candidate; this module can only bind arguments declared
//! by that candidate's semantic contract. Missing semantic values remain typed
//! missing slots. Graph operations and fresh node identities are created only
//! after every required slot has resolved.

use bpmn_lite_compiler::{ConditionExpr, ConditionLiteral, ConditionOp, IRNode, TimerSpec};
use designer_graph::ops::{GuardTrigger, Operation, RegionBranch};
use designer_graph::productions;
use designer_graph::schema::{DesignerDag, NodeKey};
use sem_os_policy::decision_board::{
    ArgumentKind, BindingProvenance, CanonicalCandidateId, EvidenceRecordHash, ProposalStatus,
    ProposalWorkbook, SemanticDecisionBoard, SlotRequirement, SlotValue, SlotValueState,
    WorkbookId, WorkbookSlot,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

const MAX_DECLARED_COUNT: u32 = 10_000;
const MAX_DURATION_MS: u64 = 10 * 365 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Error)]
pub(crate) enum ProposalError {
    #[error("proposal contract error: {0}")]
    Contract(String),
    #[error("invalid slot answer: {0}")]
    InvalidAnswer(String),
    #[error("proposal workbook is not ready for materialization: {0:?}")]
    NotReady(ProposalStatus),
    #[error("proposal graph resolution failed: {0}")]
    Graph(String),
    #[error("proposal materialization failed: {0}")]
    Materialization(String),
}

/// One explicitly typed answer. The tagged `SlotValue` shape prevents a text
/// value from being smuggled into a node/count/duration slot.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SlotAnswer {
    pub name: String,
    pub value: SlotValue,
}

/// Concrete operations exist only once the workbook is complete.
#[derive(Clone, Debug)]
pub(crate) struct BoundProposal {
    pub ops: Vec<Operation>,
    pub description: String,
}

fn fresh_key() -> NodeKey {
    NodeKey(Uuid::new_v4())
}

fn task(name: &str) -> IRNode {
    IRNode::ServiceTask {
        id: name.to_string(),
        name: name.to_string(),
        task_type: "noop".to_string(),
    }
}

fn end_node(id: String) -> IRNode {
    IRNode::End {
        id,
        terminate: false,
    }
}

fn quoted_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\'' || character == '"' {
            let mut span = String::new();
            for next in chars.by_ref() {
                if next == character {
                    break;
                }
                span.push(next);
            }
            let value = sanitize_identifier(&span);
            if !value.is_empty() {
                out.push(value);
            }
        }
    }
    out
}

fn sanitize_identifier(value: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = true;
    for character in value.to_lowercase().chars() {
        if character.is_alphanumeric() {
            out.push(character);
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

fn valid_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_alphabetic() || first == '_')
        && characters
            .all(|character| character.is_alphanumeric() || character == '_' || character == '-')
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|character: char| !character.is_alphanumeric())
                .to_string()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn unit_ms(unit: &str) -> Option<u64> {
    match unit {
        "ms" | "millisecond" | "milliseconds" => Some(1),
        "second" | "seconds" | "sec" | "secs" => Some(1_000),
        "minute" | "minutes" | "min" | "mins" => Some(60_000),
        "hour" | "hours" | "hr" | "hrs" => Some(3_600_000),
        "day" | "days" => Some(86_400_000),
        "week" | "weeks" => Some(604_800_000),
        _ => None,
    }
}

fn iso8601_ms(word: &str) -> Option<u64> {
    let upper = word.to_uppercase();
    let rest = upper.strip_prefix('P')?;
    let (date, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut millis = 0_u64;
    let mut found = false;
    let mut parse = |part: &str, units: &[(char, u64)]| -> Option<()> {
        let mut number = String::new();
        for character in part.chars() {
            if character.is_ascii_digit() {
                number.push(character);
                continue;
            }
            let multiplier = units
                .iter()
                .find_map(|(unit, multiplier)| (*unit == character).then_some(*multiplier))?;
            let value: u64 = number.parse().ok()?;
            millis = millis.checked_add(value.checked_mul(multiplier)?)?;
            found = true;
            number.clear();
        }
        number.is_empty().then_some(())
    };
    parse(date, &[('W', 604_800_000), ('D', 86_400_000)])?;
    parse(time, &[('H', 3_600_000), ('M', 60_000), ('S', 1_000)])?;
    found.then_some(millis)
}

#[derive(Clone, Copy)]
struct ParsedDuration {
    millis: u64,
    interval: bool,
}

fn durations(text: &str) -> Vec<ParsedDuration> {
    let words = words(text);
    let mut out = Vec::new();
    for (index, word) in words.iter().enumerate() {
        let interval = index > 0 && words[index - 1] == "every";
        if let Some(millis) = iso8601_ms(word) {
            out.push(ParsedDuration { millis, interval });
            continue;
        }
        if let Ok(value) = word.parse::<u64>() {
            if let Some(multiplier) = words.get(index + 1).and_then(|unit| unit_ms(unit)) {
                if let Some(millis) = value.checked_mul(multiplier) {
                    out.push(ParsedDuration { millis, interval });
                }
            }
        }
    }
    out
}

fn plain_duration_ms(text: &str) -> Option<u64> {
    durations(text)
        .into_iter()
        .find(|duration| !duration.interval)
        .map(|duration| duration.millis)
}

fn interval_ms(text: &str) -> Option<u64> {
    durations(text)
        .into_iter()
        .find(|duration| duration.interval)
        .map(|duration| duration.millis)
}

fn followed_count(text: &str, labels: &[&str]) -> Option<u32> {
    let words = words(text);
    words.iter().enumerate().find_map(|(index, word)| {
        let value = word.parse::<u32>().ok()?;
        labels
            .contains(&words.get(index + 1)?.as_str())
            .then_some(value)
    })
}

fn bare_integer(text: &str) -> Option<u32> {
    let words = words(text);
    words.iter().enumerate().find_map(|(index, word)| {
        let value = word.parse::<u32>().ok()?;
        let next = words.get(index + 1).map(String::as_str);
        let reserved = next.is_some_and(|unit| unit_ms(unit).is_some())
            || matches!(next, Some("time" | "times" | "branch" | "branches"));
        (!reserved).then_some(value)
    })
}

fn count_for_slot(name: &str, utterance: &str, quoted: &[String]) -> Option<u32> {
    match name {
        "max_fires" | "max_reminders" => followed_count(utterance, &["time", "times"]),
        "branch_count" => followed_count(utterance, &["branch", "branches"])
            .or_else(|| (quoted.len() >= 2).then_some(quoted.len() as u32)),
        _ => bare_integer(utterance),
    }
}

fn data_object_ids(dag: &DesignerDag) -> Result<Vec<String>, ProposalError> {
    let graph = dag
        .to_ir()
        .map_err(|error| ProposalError::Graph(error.to_string()))?;
    Ok(graph
        .node_indices()
        .filter_map(|index| match &graph[index] {
            IRNode::DataObject { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect())
}

fn graph_node_ids(dag: &DesignerDag) -> Result<Vec<String>, ProposalError> {
    let graph = dag
        .to_ir()
        .map_err(|error| ProposalError::Graph(error.to_string()))?;
    Ok(graph
        .node_indices()
        .map(|index| graph[index].id().to_string())
        .collect())
}

fn mentioned_id(ids: &[String], text: &str, used: &[String]) -> Option<String> {
    let lower = text.to_lowercase();
    ids.iter()
        .filter(|id| !used.contains(id))
        .find(|id| {
            lower
                .split(|character: char| !(character.is_alphanumeric() || character == '_'))
                .any(|token| token == id.to_lowercase())
        })
        .cloned()
}

fn bpmn_id_for_key(dag: &DesignerDag, key: NodeKey) -> Result<String, ProposalError> {
    let graph = dag
        .to_ir()
        .map_err(|error| ProposalError::Graph(error.to_string()))?;
    graph
        .node_indices()
        .find_map(|index| {
            (dag.key_for_bpmn_id(graph[index].id()) == Some(key))
                .then(|| graph[index].id().to_string())
        })
        .ok_or_else(|| ProposalError::Graph("anchor key resolves to no node".to_string()))
}

fn anchor_slot(candidate_id: &str) -> Option<&'static str> {
    match candidate_id {
        "op.append_node"
        | "op.insert_before"
        | "op.insert_after"
        | "op.create_parallel_region"
        | "op.create_inclusive_region"
        | "op.create_multi_instance_region"
        | "prod.request_and_wait"
        | "prod.reminder_then_escalate"
        | "prod.interrupting_timeout"
        | "prod.non_interrupting_notification" => Some("anchor"),
        "op.replace_node" | "op.delete_subgraph" => Some("target"),
        "op.connect" => Some("from"),
        "op.create_branch" => Some("gateway"),
        "op.attach_guard" | "op.attach_rearming_guard" => Some("host"),
        "op.set_guard_trigger" | "op.set_guard_budget" => Some("guard"),
        "op.set_correlation_source" => Some("node"),
        _ => None,
    }
}

fn extracted_provenance(detail: impl Into<String>) -> Option<BindingProvenance> {
    Some(BindingProvenance {
        source: "deterministic_extraction".to_string(),
        detail: detail.into(),
    })
}

/// Start a workbook from exactly the argument schema on the selected semantic
/// board. No undeclared convenience slot can enter this structure.
pub(crate) fn start_workbook(
    dag: &DesignerDag,
    anchor: Option<NodeKey>,
    board: &SemanticDecisionBoard,
    decision: &utterance_engine::policy::DecisionRecord,
    candidate_id: &CanonicalCandidateId,
    utterance: &str,
    source_utterance_seq: u64,
) -> Result<ProposalWorkbook, ProposalError> {
    if decision.board_hash != board.board_hash.as_str() {
        return Err(ProposalError::Contract(
            "decision record and semantic board hashes differ".to_string(),
        ));
    }
    match &decision.disposition {
        utterance_engine::policy::ProposalDisposition::Candidate {
            candidate_id: selected,
        } if selected == candidate_id.as_str() => {}
        _ => {
            return Err(ProposalError::Contract(
                "workbook candidate does not match the inference disposition".to_string(),
            ));
        }
    }
    let candidate = board
        .candidates
        .iter()
        .find(|candidate| candidate.canonical_id == *candidate_id)
        .ok_or_else(|| ProposalError::Contract("candidate is absent from the board".to_string()))?;

    let quoted = quoted_names(utterance);
    let data_ids = data_object_ids(dag)?;
    let node_ids = graph_node_ids(dag)?;
    let anchor_id = anchor.map(|key| bpmn_id_for_key(dag, key)).transpose()?;
    let positional_slot = anchor_slot(candidate_id.as_str());
    let mut identifier_index = 0_usize;
    let mut used_node_ids = anchor_id.iter().cloned().collect::<Vec<_>>();
    let mut slots = Vec::with_capacity(candidate.arguments.len());

    for argument in &candidate.arguments {
        let mut value = SlotValueState::Missing;
        let mut provenance = None;
        if positional_slot == Some(argument.name.as_str()) {
            if let Some(id) = &anchor_id {
                value = SlotValueState::Resolved(SlotValue::NodeReference(id.clone()));
                provenance = extracted_provenance("resolved endpoint anchor");
            }
        } else {
            let extracted = match argument.kind {
                ArgumentKind::Identifier => {
                    let name = quoted.get(identifier_index).cloned();
                    if name.is_some() {
                        identifier_index += 1;
                    }
                    name.filter(|name| valid_identifier(name))
                        .map(SlotValue::Identifier)
                }
                ArgumentKind::NodeReference => mentioned_id(&node_ids, utterance, &used_node_ids)
                    .map(|id| {
                        used_node_ids.push(id.clone());
                        SlotValue::NodeReference(id)
                    }),
                ArgumentKind::DataReference => {
                    mentioned_id(&data_ids, utterance, &[]).map(SlotValue::DataReference)
                }
                ArgumentKind::Count => count_for_slot(&argument.name, utterance, &quoted)
                    .filter(|value| *value > 0 && *value <= MAX_DECLARED_COUNT)
                    .map(SlotValue::Count),
                ArgumentKind::Duration => {
                    let millis = if argument.name == "interval" {
                        interval_ms(utterance)
                    } else {
                        plain_duration_ms(utterance).or_else(|| interval_ms(utterance))
                    };
                    millis
                        .filter(|value| *value > 0 && *value <= MAX_DURATION_MS)
                        .map(SlotValue::DurationMillis)
                }
                ArgumentKind::Text => quoted.get(identifier_index).cloned().map(|text| {
                    identifier_index += 1;
                    SlotValue::Text(text)
                }),
                ArgumentKind::Boolean => {
                    words(utterance)
                        .into_iter()
                        .find_map(|word| match word.as_str() {
                            "true" | "yes" => Some(SlotValue::Boolean(true)),
                            "false" | "no" => Some(SlotValue::Boolean(false)),
                            _ => None,
                        })
                }
                // Conditions and subprocess pins require explicit typed answers.
                ArgumentKind::Condition | ArgumentKind::SubprocessReference => None,
            };
            if let Some(extracted) = extracted {
                value = SlotValueState::Resolved(extracted);
                provenance = extracted_provenance("bounded utterance extraction");
            }
        }
        slots.push(WorkbookSlot {
            name: argument.name.clone(),
            kind: argument.kind,
            requirement: if argument.required {
                SlotRequirement::Required
            } else {
                SlotRequirement::Optional
            },
            value,
            provenance,
            clarification_prompt: argument.clarification_prompt.clone(),
        });
    }

    ProposalWorkbook::new(
        1,
        WorkbookId::new(Uuid::new_v4().to_string())
            .map_err(|error| ProposalError::Contract(error.to_string()))?,
        source_utterance_seq,
        board.board_hash.clone(),
        board.graph_revision.clone(),
        candidate_id.clone(),
        slots,
        EvidenceRecordHash::new(decision.decision_record_hash.clone())
            .map_err(|error| ProposalError::Contract(error.to_string()))?,
    )
    .map_err(|error| ProposalError::Contract(error.to_string()))
}

fn parse_condition(value: &str) -> Result<ConditionExpr, ProposalError> {
    let value = value.trim();
    for (operator_text, operator) in [
        ("!=", ConditionOp::Neq),
        ("==", ConditionOp::Eq),
        (">", ConditionOp::Gt),
        ("<", ConditionOp::Lt),
    ] {
        if let Some((flag, literal)) = value.split_once(operator_text) {
            let flag = flag.trim();
            if !valid_identifier(flag) {
                return Err(ProposalError::InvalidAnswer(format!(
                    "condition flag '{flag}' is not an identifier"
                )));
            }
            let literal = match literal.trim() {
                "true" => ConditionLiteral::Bool(true),
                "false" => ConditionLiteral::Bool(false),
                integer => ConditionLiteral::I64(integer.parse().map_err(|_| {
                    ProposalError::InvalidAnswer(format!(
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
    Err(ProposalError::InvalidAnswer(
        "condition must use flag==value, flag!=value, flag>integer or flag<integer".to_string(),
    ))
}

fn validate_answer(dag: &DesignerDag, answer: &SlotAnswer) -> Result<(), ProposalError> {
    match &answer.value {
        SlotValue::Identifier(value) if !valid_identifier(value) => Err(
            ProposalError::InvalidAnswer(format!("'{}' is not a valid identifier", value)),
        ),
        SlotValue::NodeReference(value) if dag.key_for_bpmn_id(value).is_none() => Err(
            ProposalError::InvalidAnswer(format!("node reference '{value}' does not exist")),
        ),
        SlotValue::DataReference(value) if !data_object_ids(dag)?.contains(value) => Err(
            ProposalError::InvalidAnswer(format!("data reference '{value}' does not exist")),
        ),
        SlotValue::Count(value) if *value == 0 || *value > MAX_DECLARED_COUNT => Err(
            ProposalError::InvalidAnswer(format!("count must be in 1..={MAX_DECLARED_COUNT}")),
        ),
        SlotValue::DurationMillis(value) if *value == 0 || *value > MAX_DURATION_MS => {
            Err(ProposalError::InvalidAnswer(format!(
                "duration must be in 1..={MAX_DURATION_MS} milliseconds"
            )))
        }
        SlotValue::Condition(value) if answer.name == "conditions" => value
            .split([',', ';'])
            .map(parse_condition)
            .collect::<Result<Vec<_>, _>>()
            .map(|_| ()),
        SlotValue::Condition(value) => parse_condition(value).map(|_| ()),
        SlotValue::SubprocessReference(value) if !value.contains('@') => {
            Err(ProposalError::InvalidAnswer(
                "subprocess reference must be a pinned name@revision".to_string(),
            ))
        }
        SlotValue::Text(value) if value.trim().is_empty() => Err(ProposalError::InvalidAnswer(
            "text answer must not be empty".to_string(),
        )),
        _ => Ok(()),
    }
}

/// Validate and apply a batch atomically. The owned input lets the caller keep
/// its prior workbook unchanged whenever validation fails.
pub(crate) fn apply_explicit_answers(
    dag: &DesignerDag,
    mut workbook: ProposalWorkbook,
    answers: Vec<SlotAnswer>,
) -> Result<ProposalWorkbook, ProposalError> {
    if workbook.status() != ProposalStatus::NeedsArguments {
        return Err(ProposalError::InvalidAnswer(format!(
            "workbook in {:?} does not accept answers",
            workbook.status()
        )));
    }
    for answer in &answers {
        validate_answer(dag, answer)?;
    }
    workbook
        .apply_answers(
            answers
                .into_iter()
                .map(|answer| (answer.name, answer.value))
                .collect(),
        )
        .map_err(|error| ProposalError::InvalidAnswer(error.to_string()))?;
    Ok(workbook)
}

fn value<'a>(workbook: &'a ProposalWorkbook, name: &str) -> Result<&'a SlotValue, ProposalError> {
    workbook
        .slots()
        .iter()
        .find(|slot| slot.name == name)
        .and_then(|slot| match &slot.value {
            SlotValueState::Resolved(value) => Some(value),
            SlotValueState::Missing => None,
        })
        .ok_or_else(|| ProposalError::Materialization(format!("slot '{name}' is unresolved")))
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

fn identifier(workbook: &ProposalWorkbook, name: &str) -> Result<String, ProposalError> {
    match value(workbook, name)? {
        SlotValue::Identifier(value) => Ok(value.clone()),
        _ => Err(ProposalError::Materialization(format!(
            "slot '{name}' is not an identifier"
        ))),
    }
}

fn node_key(
    dag: &DesignerDag,
    workbook: &ProposalWorkbook,
    name: &str,
) -> Result<NodeKey, ProposalError> {
    match value(workbook, name)? {
        SlotValue::NodeReference(value) => dag.key_for_bpmn_id(value).ok_or_else(|| {
            ProposalError::Graph(format!("node reference '{value}' no longer exists"))
        }),
        _ => Err(ProposalError::Materialization(format!(
            "slot '{name}' is not a node reference"
        ))),
    }
}

fn data_reference(workbook: &ProposalWorkbook, name: &str) -> Result<String, ProposalError> {
    match value(workbook, name)? {
        SlotValue::DataReference(value) => Ok(value.clone()),
        _ => Err(ProposalError::Materialization(format!(
            "slot '{name}' is not a data reference"
        ))),
    }
}

fn count(workbook: &ProposalWorkbook, name: &str) -> Result<u32, ProposalError> {
    match value(workbook, name)? {
        SlotValue::Count(value) => Ok(*value),
        _ => Err(ProposalError::Materialization(format!(
            "slot '{name}' is not a count"
        ))),
    }
}

fn duration(workbook: &ProposalWorkbook, name: &str) -> Result<u64, ProposalError> {
    match value(workbook, name)? {
        SlotValue::DurationMillis(value) => Ok(*value),
        _ => Err(ProposalError::Materialization(format!(
            "slot '{name}' is not a duration"
        ))),
    }
}

/// Convert a complete workbook into the exhaustive Designer operation shape.
/// This is the only function in the proposal path that mints graph identities.
pub(crate) fn materialize_operations(
    dag: &DesignerDag,
    workbook: &ProposalWorkbook,
) -> Result<BoundProposal, ProposalError> {
    if workbook.status() != ProposalStatus::ReadyForDryRun {
        return Err(ProposalError::NotReady(workbook.status()));
    }
    let candidate = workbook.candidate_id.as_str();
    let bound = match candidate {
        "op.append_node" | "op.insert_after" | "op.insert_before" | "op.replace_node" => {
            let slot = if candidate == "op.replace_node" {
                "replacement"
            } else {
                "node"
            };
            let name = identifier(workbook, slot)?;
            let key = fresh_key();
            let node = task(&name);
            let (operation, description) = match candidate {
                "op.append_node" => (
                    Operation::AppendNode {
                        anchor: node_key(dag, workbook, "anchor")?,
                        key,
                        node,
                        edge_id: format!("flow_{name}"),
                    },
                    format!("append service task '{name}'"),
                ),
                "op.insert_after" => (
                    Operation::InsertAfter {
                        anchor: node_key(dag, workbook, "anchor")?,
                        key,
                        node,
                        edge_id: format!("flow_{name}"),
                    },
                    format!("insert service task '{name}' after the selection"),
                ),
                "op.insert_before" => (
                    Operation::InsertBefore {
                        anchor: node_key(dag, workbook, "anchor")?,
                        key,
                        node,
                        edge_id: format!("flow_{name}"),
                    },
                    format!("insert service task '{name}' before the selection"),
                ),
                _ => (
                    Operation::ReplaceNode {
                        target: node_key(dag, workbook, "target")?,
                        key,
                        node,
                    },
                    format!("replace the selection with service task '{name}'"),
                ),
            };
            BoundProposal {
                ops: vec![operation],
                description,
            }
        }
        "op.connect" => {
            let condition = match optional_value(workbook, "condition") {
                Some(SlotValue::Condition(value)) => Some(parse_condition(value)?),
                Some(_) => {
                    return Err(ProposalError::Materialization(
                        "slot 'condition' has the wrong kind".to_string(),
                    ));
                }
                None => None,
            };
            BoundProposal {
                ops: vec![Operation::Connect {
                    from: node_key(dag, workbook, "from")?,
                    to: node_key(dag, workbook, "to")?,
                    edge_id: format!("flow_{}", workbook.workbook_id.as_str()),
                    condition,
                }],
                description: "connect the two declared existing nodes".to_string(),
            }
        }
        "op.create_branch" => {
            let outcome = identifier(workbook, "outcome")?;
            BoundProposal {
                ops: vec![Operation::CreateBranch {
                    gateway: node_key(dag, workbook, "gateway")?,
                    target: node_key(dag, workbook, "target")?,
                    edge_id: format!("flow_{outcome}"),
                    condition: Some(ConditionExpr {
                        flag_name: outcome.clone(),
                        op: ConditionOp::Eq,
                        literal: ConditionLiteral::Bool(true),
                    }),
                }],
                description: format!("create the '{outcome}' outcome branch"),
            }
        }
        "op.create_parallel_region" => {
            let branch_count = count(workbook, "branch_count")?;
            if !(2..=32).contains(&branch_count) {
                return Err(ProposalError::Materialization(
                    "parallel branch_count must be in 2..=32".to_string(),
                ));
            }
            let branches = (1..=branch_count)
                .map(|index| {
                    let name = format!("parallel_branch_{index}");
                    RegionBranch {
                        key: fresh_key(),
                        node: task(&name),
                        in_edge_id: format!("flow_in_{name}"),
                        out_edge_id: format!("flow_out_{name}"),
                        condition: None,
                    }
                })
                .collect();
            let base = workbook.workbook_id.as_str();
            BoundProposal {
                ops: vec![Operation::CreateParallelRegion {
                    anchor: node_key(dag, workbook, "anchor")?,
                    fork_key: fresh_key(),
                    fork_node_id: format!("parallel_{base}_fork"),
                    join_key: fresh_key(),
                    join_node_id: format!("parallel_{base}_join"),
                    entry_edge_id: format!("flow_parallel_{base}"),
                    branches,
                }],
                description: format!("create {branch_count} parallel branches"),
            }
        }
        "op.create_inclusive_region" => {
            let branch_count = count(workbook, "branch_count")?;
            if !(2..=32).contains(&branch_count) {
                return Err(ProposalError::Materialization(
                    "inclusive branch_count must be in 2..=32".to_string(),
                ));
            }
            let conditions = match value(workbook, "conditions")? {
                SlotValue::Condition(value) => value
                    .split([',', ';'])
                    .map(parse_condition)
                    .collect::<Result<Vec<_>, _>>()?,
                _ => {
                    return Err(ProposalError::Materialization(
                        "slot 'conditions' has the wrong kind".to_string(),
                    ));
                }
            };
            if conditions.len() != branch_count as usize {
                return Err(ProposalError::Materialization(format!(
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
                        key: fresh_key(),
                        node: task(&name),
                        in_edge_id: format!("flow_in_{name}"),
                        out_edge_id: format!("flow_out_{name}"),
                        condition: Some(condition),
                    }
                })
                .collect();
            let base = workbook.workbook_id.as_str();
            BoundProposal {
                ops: vec![Operation::CreateInclusiveRegion {
                    anchor: node_key(dag, workbook, "anchor")?,
                    fork_key: fresh_key(),
                    fork_node_id: format!("inclusive_{base}_fork"),
                    join_key: fresh_key(),
                    join_node_id: format!("inclusive_{base}_join"),
                    entry_edge_id: format!("flow_inclusive_{base}"),
                    branches,
                }],
                description: format!("create {branch_count} conditional inclusive branches"),
            }
        }
        "op.create_multi_instance_region" => {
            let collection = data_reference(workbook, "collection")?;
            let maximum = count(workbook, "declared_max")?;
            let name = format!("for_each_{collection}");
            BoundProposal {
                ops: vec![Operation::CreateMultiInstanceRegion {
                    anchor: node_key(dag, workbook, "anchor")?,
                    key: fresh_key(),
                    node: IRNode::MultiInstance {
                        id: name.clone(),
                        name: name.clone(),
                        task_type: "noop".to_string(),
                        collection_flag_name: collection.clone(),
                        declared_max: maximum,
                    },
                    edge_id: format!("flow_{name}"),
                }],
                description: format!("repeat over '{collection}' up to {maximum} times"),
            }
        }
        "op.attach_guard" => {
            let escape = identifier(workbook, "escape")?;
            let guard_key = fresh_key();
            BoundProposal {
                ops: vec![
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
                        key: fresh_key(),
                        node: task(&escape),
                        edge_id: format!("flow_{escape}"),
                    },
                ],
                description: format!("attach an interrupting guard leading to '{escape}'"),
            }
        }
        "op.attach_rearming_guard" => {
            let escape = identifier(workbook, "escape")?;
            let guard_key = fresh_key();
            BoundProposal {
                ops: vec![
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
                        key: fresh_key(),
                        node: task(&escape),
                        edge_id: format!("flow_{escape}"),
                    },
                ],
                description: format!("attach a bounded rearming guard leading to '{escape}'"),
            }
        }
        "op.set_guard_trigger" => BoundProposal {
            ops: vec![Operation::SetGuardTrigger {
                guard: node_key(dag, workbook, "guard")?,
                trigger: GuardTrigger::Timer(TimerSpec::Duration {
                    ms: duration(workbook, "trigger")?,
                }),
            }],
            description: "set the selected guard timer".to_string(),
        },
        "op.set_guard_budget" => {
            let budget = count(workbook, "failure_budget")?;
            BoundProposal {
                ops: vec![Operation::SetGuardBudget {
                    guard: node_key(dag, workbook, "guard")?,
                    failure_budget: Some(budget),
                }],
                description: format!("set guard failure budget to {budget}"),
            }
        }
        "op.set_correlation_source" => {
            let data = data_reference(workbook, "data_reference")?;
            BoundProposal {
                ops: vec![Operation::SetCorrelationSource {
                    node: node_key(dag, workbook, "node")?,
                    corr_key_source: data.clone(),
                }],
                description: format!("set correlation source to '{data}'"),
            }
        }
        "op.delete_subgraph" => BoundProposal {
            ops: vec![Operation::DeleteNode {
                target: node_key(dag, workbook, "target")?,
            }],
            description: "delete the selected node or closed region".to_string(),
        },
        "prod.request_and_wait" => {
            let request = identifier(workbook, "request")?;
            let wait = format!("{request}_response");
            let correlation = data_reference(workbook, "correlation_source")?;
            BoundProposal {
                ops: productions::request_and_wait(productions::RequestAndWaitBindings {
                    anchor: node_key(dag, workbook, "anchor")?,
                    send_key: fresh_key(),
                    send_node: task(&request),
                    send_edge_id: format!("flow_{request}"),
                    wait_key: fresh_key(),
                    wait_node: IRNode::MessageWait {
                        id: wait.clone(),
                        name: wait.clone(),
                        corr_key_source: correlation.clone(),
                    },
                    wait_edge_id: format!("flow_{wait}"),
                }),
                description: format!(
                    "send '{request}' and wait for a response correlated on '{correlation}'"
                ),
            }
        }
        "prod.interrupting_timeout" => {
            let escape = identifier(workbook, "escape")?;
            BoundProposal {
                ops: productions::interrupting_timeout(productions::InterruptingTimeoutBindings {
                    anchor: node_key(dag, workbook, "anchor")?,
                    guard_key: fresh_key(),
                    guard_id: format!("{escape}_guard"),
                    duration_ms: duration(workbook, "duration")?,
                    continuation_key: fresh_key(),
                    continuation_node: task(&escape),
                    continuation_edge_id: format!("flow_{escape}"),
                    continuation_end_key: fresh_key(),
                    continuation_end_node: end_node(format!("{escape}_end")),
                    continuation_end_edge_id: format!("flow_{escape}_end"),
                }),
                description: format!("interrupt on timeout and continue to '{escape}'"),
            }
        }
        "prod.reminder_then_escalate" | "prod.non_interrupting_notification" => {
            let name_slot = if candidate == "prod.reminder_then_escalate" {
                "escalation"
            } else {
                "notification"
            };
            let count_slot = if candidate == "prod.reminder_then_escalate" {
                "max_reminders"
            } else {
                "max_fires"
            };
            let name = identifier(workbook, name_slot)?;
            let interval = duration(workbook, "interval")?;
            let maximum = count(workbook, count_slot)?;
            let ops = if candidate == "prod.reminder_then_escalate" {
                productions::reminder_then_escalate(productions::ReminderThenEscalateBindings {
                    anchor: node_key(dag, workbook, "anchor")?,
                    guard_key: fresh_key(),
                    guard_id: format!("{name}_guard"),
                    cycle: TimerSpec::Cycle {
                        interval_ms: interval,
                        max_fires: maximum,
                    },
                    escalation_key: fresh_key(),
                    escalation_node: task(&name),
                    escalation_edge_id: format!("flow_{name}"),
                    escalation_end_key: fresh_key(),
                    escalation_end_node: end_node(format!("{name}_end")),
                    escalation_end_edge_id: format!("flow_{name}_end"),
                })
            } else {
                productions::non_interrupting_notification(
                    productions::NonInterruptingNotificationBindings {
                        anchor: node_key(dag, workbook, "anchor")?,
                        guard_key: fresh_key(),
                        guard_id: format!("{name}_guard"),
                        interval_ms: interval,
                        max_fires: maximum,
                        notification_key: fresh_key(),
                        notification_node: task(&name),
                        notification_edge_id: format!("flow_{name}"),
                        notification_end_key: fresh_key(),
                        notification_end_node: end_node(format!("{name}_end")),
                        notification_end_edge_id: format!("flow_{name}_end"),
                    },
                )
            };
            BoundProposal {
                ops,
                description: format!("run '{name}' every {interval}ms up to {maximum} times"),
            }
        }
        // Every board-visible id is listed above. Reaching this arm means the
        // semantic board admitted a candidate without adapter coverage.
        other => {
            return Err(ProposalError::Contract(format!(
                "board-visible candidate '{other}' has no materialization implementation"
            )));
        }
    };
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_types::{DataObjectRole, DataObjectType, PrimitiveType};
    use designer_graph::schema::Provenance;
    use utterance_engine::board::PolicyFilter;
    use utterance_engine::policy::{DecisionRecord, ProposalDisposition};

    #[test]
    fn condition_parser_is_strict() {
        assert!(parse_condition("approved==true").is_ok());
        assert!(parse_condition("attempts>2").is_ok());
        assert!(parse_condition("approved").is_err());
        assert!(parse_condition("bad flag==true").is_err());
    }

    #[test]
    fn bounded_extractors_do_not_panic_on_hostile_text() {
        let text = "\0 every 999999999999999999999999 weeks 999999999999 times \"\"";
        assert!(interval_ms(text).is_none());
        assert!(quoted_names(text).is_empty());
    }

    #[test]
    fn request_and_wait_resumes_with_typed_data_answer_and_dry_admits() {
        let mut dag = DesignerDag::new("request workbook");
        let start = fresh_key();
        let data = fresh_key();
        dag.seed(
            start,
            IRNode::Start {
                id: "start".to_string(),
            },
            Provenance::default(),
        )
        .unwrap();
        dag.seed(
            data,
            IRNode::DataObject {
                id: "application_reference".to_string(),
                name: "Application reference".to_string(),
                type_decl: DataObjectType::Primitive(PrimitiveType::String),
                role: DataObjectRole::Internal,
            },
            Provenance::default(),
        )
        .unwrap();
        let anchor = fresh_key();
        let staged = productions::apply_production(
            &dag,
            vec![
                Operation::AppendNode {
                    anchor: start,
                    key: anchor,
                    node: task("review"),
                    edge_id: "flow_review".to_string(),
                },
                Operation::AppendNode {
                    anchor,
                    key: fresh_key(),
                    node: end_node("end".to_string()),
                    edge_id: "flow_end".to_string(),
                },
            ],
            Provenance::default(),
        )
        .unwrap();
        let dag = staged.candidate;
        let board = utterance_engine::bpmn_board::build_bpmn_semantic_board(
            &dag,
            Some((anchor, "review")),
            "revision-1",
            &PolicyFilter::default(),
        )
        .unwrap();
        let candidate_id = CanonicalCandidateId::new("prod.request_and_wait").unwrap();
        assert!(board
            .candidates
            .iter()
            .any(|candidate| candidate.canonical_id == candidate_id));
        let decision = DecisionRecord {
            board_hash: board.board_hash.as_str().to_string(),
            retrieved_subset_hash: "subset".to_string(),
            model_bundle_hash: "tier0.test".to_string(),
            disposition_policy_hash: "policy".to_string(),
            context_projection_hash: "context".to_string(),
            ranking: vec![],
            disposition: ProposalDisposition::Candidate {
                candidate_id: candidate_id.as_str().to_string(),
            },
            evidence_trace: None,
            decision_record_hash: "d".repeat(64),
        };
        let workbook = start_workbook(
            &dag,
            Some(anchor),
            &board,
            &decision,
            &candidate_id,
            "Send 'onboarding_request' and wait for its response",
            7,
        )
        .unwrap();
        assert_eq!(workbook.status(), ProposalStatus::NeedsArguments);
        assert_eq!(
            workbook
                .slots()
                .iter()
                .filter(|slot| matches!(slot.value, SlotValueState::Missing))
                .map(|slot| slot.name.as_str())
                .collect::<Vec<_>>(),
            vec!["correlation_source"]
        );

        let workbook = apply_explicit_answers(
            &dag,
            workbook,
            vec![SlotAnswer {
                name: "correlation_source".to_string(),
                value: SlotValue::DataReference("application_reference".to_string()),
            }],
        )
        .unwrap();
        assert_eq!(workbook.status(), ProposalStatus::ReadyForDryRun);
        let bound = materialize_operations(&dag, &workbook).unwrap();
        let dry = productions::apply_production(&dag, bound.ops, Provenance::default()).unwrap();
        dry.candidate.admit().unwrap();
        assert_eq!(dag.node_count(), 4, "workbook processing mutates no graph");
    }
}
