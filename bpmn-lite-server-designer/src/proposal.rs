//! Deterministic, resumable proposal workbooks.
//!
//! Inference selects a candidate; this module can only bind arguments declared
//! by that candidate's semantic contract. Missing semantic values remain typed
//! missing slots. Graph operations and fresh node identities are created only
//! after every required slot has resolved.

use bpmn_lite_compiler::{ConditionExpr, ConditionLiteral, ConditionOp, IRNode, TimerSpec};
use designer_graph::ops::{GuardTrigger, Operation};
#[cfg(test)]
use designer_graph::productions;
use designer_graph::schema::{DesignerDag, NodeKey};
#[cfg(test)]
use semantic_decision_contracts::CanonicalCandidateId;
use semantic_decision_contracts::{
    ArgumentKind, BindingProvenance, EvidenceRecordHash, ProposalStatus, ProposalWorkbook,
    SemanticDecisionBoard, SlotRequirement, SlotValue, SlotValueState, WorkbookId, WorkbookSlot,
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
    #[error("proposal graph resolution failed: {0}")]
    Graph(String),
    #[cfg(test)]
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

pub(crate) struct SelectedMove<'a> {
    pub(crate) position: &'a semantic_decision_contracts::DesignPosition,
    pub(crate) move_id: &'a semantic_decision_contracts::LegalMoveId,
}

/// The provenance that opened a workbook. A palette selection is explicit
/// user evidence, not a fabricated utterance-policy decision.
pub(crate) enum WorkbookEvidence<'a> {
    Decision(&'a utterance_engine::policy::DecisionRecord),
    PaletteSelection(EvidenceRecordHash),
}

#[cfg(test)]
fn fresh_key() -> NodeKey {
    NodeKey(Uuid::new_v4())
}

#[cfg(test)]
fn task(name: &str) -> IRNode {
    IRNode::ServiceTask {
        id: name.to_string(),
        name: name.to_string(),
        task_type: "noop".to_string(),
    }
}

#[cfg(test)]
fn end_node(id: String) -> IRNode {
    IRNode::End {
        id,
        terminate: false,
    }
}

pub(crate) fn quoted_names(text: &str) -> Vec<String> {
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

pub(crate) fn sanitize_identifier(value: &str) -> String {
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
pub(crate) struct ParsedDuration {
    millis: u64,
    interval: bool,
}

pub(crate) fn durations(text: &str) -> Vec<ParsedDuration> {
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

pub(crate) fn followed_count(text: &str, labels: &[&str]) -> Option<u32> {
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

pub(crate) fn bpmn_id_for_key(dag: &DesignerDag, key: NodeKey) -> Result<String, ProposalError> {
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

/// Inverse of `parse_condition`: renders a `ConditionExpr` back to the
/// `flag==literal` / `flag!=literal` / `flag>literal` / `flag<literal` text
/// grammar `parse_condition` accepts, byte-for-byte round-trippable.
fn render_condition(expr: &ConditionExpr) -> String {
    let op = match expr.op {
        ConditionOp::Eq => "==",
        ConditionOp::Neq => "!=",
        ConditionOp::Gt => ">",
        ConditionOp::Lt => "<",
    };
    let literal = match &expr.literal {
        ConditionLiteral::Bool(true) => "true".to_string(),
        ConditionLiteral::Bool(false) => "false".to_string(),
        ConditionLiteral::I64(n) => n.to_string(),
    };
    format!("{}{op}{literal}", expr.flag_name)
}

/// A raw direct-edit operation tape's best-effort recovered candidate shape:
/// which semantic candidate and anchor it MIGHT be, and which typed slot
/// answers to try. This is a guess, not a proof — the caller
/// (`resolve_direct_edit`) proves or refutes it by materializing the
/// recovered shape through the production path and comparing RESULTING
/// graph state, not this shape itself (v0.8 amendment,
/// `EOP-PLAN-BPMN-GAMEBOARD-001.md` Phase 2 item 9). One structural arm per
/// representable candidate, single- or multi-`Operation` (v0.9 amendment,
/// same doc, closing the deferred multi-op tranche); workbook-synthesized-
/// only fields (`key`, `edge_id`, `guard_id`, `fork_key`, `join_key`,
/// `entry_edge_id`, `in_edge_id`, `out_edge_id`) are never read here.
pub(crate) struct RecoveredShape {
    pub(crate) candidate_id: &'static str,
    pub(crate) anchor: NodeKey,
    pub(crate) answers: Vec<SlotAnswer>,
}

/// Why `recover_candidate_shape` could not produce a shape to try.
/// `Ambiguous` is distinguished from `NotProducible` because it names a real
/// defect class (fail closed rather than guess): a tape whose content
/// genuinely cannot distinguish which of two-or-more candidates produced it,
/// as opposed to a tape that simply matches no candidate at all.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ShapeRefusal {
    NotProducible,
    Ambiguous,
}

fn answer(name: &str, value: SlotValue) -> SlotAnswer {
    SlotAnswer { name: name.to_string(), value }
}

pub(crate) fn recover_candidate_shape(
    dag: &DesignerDag,
    operations: &[Operation],
) -> Result<RecoveredShape, ShapeRefusal> {
    match operations {
        [operation] => {
            recover_single_operation_shape(dag, operation).ok_or(ShapeRefusal::NotProducible)
        }
        [Operation::AttachGuard {
            host,
            key: guard_key,
            trigger: GuardTrigger::Timer(TimerSpec::Duration { ms }),
            ..
        }, Operation::AppendNode { anchor, node, .. }]
            if anchor == guard_key =>
        {
            Ok(RecoveredShape {
                candidate_id: "op.attach_guard",
                anchor: *host,
                answers: vec![
                    answer("escape", SlotValue::Identifier(node.id().to_string())),
                    answer("trigger", SlotValue::DurationMillis(*ms)),
                ],
            })
        }
        [Operation::AttachRearmingGuard {
            host,
            key: guard_key,
            trigger: GuardTrigger::Timer(TimerSpec::Cycle { interval_ms, max_fires }),
            ..
        }, Operation::AppendNode { anchor, node, .. }]
            if anchor == guard_key =>
        {
            Ok(RecoveredShape {
                candidate_id: "op.attach_rearming_guard",
                anchor: *host,
                answers: vec![
                    answer("escape", SlotValue::Identifier(node.id().to_string())),
                    answer("interval", SlotValue::DurationMillis(*interval_ms)),
                    answer("max_fires", SlotValue::Count(*max_fires)),
                ],
            })
        }
        [Operation::InsertAfter { anchor, key: send_key, node: send_node, .. }, Operation::InsertAfter {
            anchor: wait_anchor,
            node: IRNode::MessageWait { corr_key_source, .. },
            ..
        }] if wait_anchor == send_key => Ok(RecoveredShape {
            candidate_id: "prod.request_and_wait",
            anchor: *anchor,
            answers: vec![
                answer("request", SlotValue::Identifier(send_node.id().to_string())),
                answer(
                    "correlation_source",
                    SlotValue::DataReference(corr_key_source.clone()),
                ),
            ],
        }),
        [Operation::AttachGuard {
            host,
            key: guard_key,
            trigger: GuardTrigger::Timer(TimerSpec::Duration { ms }),
            ..
        }, Operation::AppendNode { anchor: mid_anchor, key: mid_key, node, .. }, Operation::AppendNode { anchor: end_anchor, node: IRNode::End { .. }, .. }]
            if mid_anchor == guard_key && end_anchor == mid_key =>
        {
            Ok(RecoveredShape {
                candidate_id: "prod.interrupting_timeout",
                anchor: *host,
                answers: vec![
                    answer("escape", SlotValue::Identifier(node.id().to_string())),
                    answer("duration", SlotValue::DurationMillis(*ms)),
                ],
            })
        }
        [Operation::AttachRearmingGuard {
            key: guard_key,
            trigger: GuardTrigger::Timer(TimerSpec::Cycle { .. }),
            ..
        }, Operation::AppendNode { anchor: mid_anchor, key: mid_key, .. }, Operation::AppendNode { anchor: end_anchor, node: IRNode::End { .. }, .. }]
            if mid_anchor == guard_key && end_anchor == mid_key =>
        {
            // prod.reminder_then_escalate and prod.non_interrupting_notification
            // (`legal_moves.rs` "prod.reminder_then_escalate" |
            // "prod.non_interrupting_notification" arm) materialize this exact
            // operation sequence — same AttachRearmingGuard/Cycle + AppendNode +
            // End — differing only in which workbook slot NAME supplied the
            // identical typed values. Nothing in the operation content
            // distinguishes them: fail closed rather than guess a label the
            // shape doesn't actually prove.
            Err(ShapeRefusal::Ambiguous)
        }
        _ => Err(ShapeRefusal::NotProducible),
    }
}

fn recover_single_operation_shape(dag: &DesignerDag, operation: &Operation) -> Option<RecoveredShape> {
    let (candidate_id, anchor, answers): (&'static str, NodeKey, Vec<SlotAnswer>) = match operation
    {
        Operation::AppendNode { anchor, node, .. } => (
            "op.append_node",
            *anchor,
            vec![answer("node", SlotValue::Identifier(node.id().to_string()))],
        ),
        Operation::InsertAfter { anchor, node, .. } => (
            "op.insert_after",
            *anchor,
            vec![answer("node", SlotValue::Identifier(node.id().to_string()))],
        ),
        Operation::InsertBefore { anchor, node, .. } => (
            "op.insert_before",
            *anchor,
            vec![answer("node", SlotValue::Identifier(node.id().to_string()))],
        ),
        Operation::ReplaceNode { target, node, .. } => (
            "op.replace_node",
            *target,
            vec![answer(
                "replacement",
                SlotValue::Identifier(node.id().to_string()),
            )],
        ),
        Operation::Connect { from, to, condition, .. } => {
            let to_id = bpmn_id_for_key(dag, *to).ok()?;
            let mut answers = vec![answer("to", SlotValue::NodeReference(to_id))];
            if let Some(condition) = condition {
                answers.push(answer(
                    "condition",
                    SlotValue::Condition(render_condition(condition)),
                ));
            }
            ("op.connect", *from, answers)
        }
        Operation::CreateBranch { gateway, target, condition, .. } => {
            let target_id = bpmn_id_for_key(dag, *target).ok()?;
            let condition = condition.as_ref()?;
            // The only shape `materialize_workbook` ever produces for this
            // candidate; anything else structurally cannot be an equivalent
            // create_branch move, so recovery itself refuses here.
            if condition.op != ConditionOp::Eq || condition.literal != ConditionLiteral::Bool(true)
            {
                return None;
            }
            (
                "op.create_branch",
                *gateway,
                vec![
                    answer("target", SlotValue::NodeReference(target_id)),
                    answer(
                        "outcome",
                        SlotValue::Identifier(condition.flag_name.clone()),
                    ),
                ],
            )
        }
        Operation::CreateParallelRegion { anchor, branches, .. } => (
            "op.create_parallel_region",
            *anchor,
            vec![answer(
                "branch_count",
                SlotValue::Count(branches.len() as u32),
            )],
        ),
        Operation::CreateInclusiveRegion { anchor, branches, .. } => {
            let mut conditions = Vec::with_capacity(branches.len());
            for branch in branches {
                conditions.push(render_condition(branch.condition.as_ref()?));
            }
            (
                "op.create_inclusive_region",
                *anchor,
                vec![
                    answer("branch_count", SlotValue::Count(branches.len() as u32)),
                    answer("conditions", SlotValue::Condition(conditions.join(","))),
                ],
            )
        }
        Operation::CreateMultiInstanceRegion { anchor, node, .. } => {
            let IRNode::MultiInstance { collection_flag_name, declared_max, .. } = node else {
                return None;
            };
            (
                "op.create_multi_instance_region",
                *anchor,
                vec![
                    answer(
                        "collection",
                        SlotValue::DataReference(collection_flag_name.clone()),
                    ),
                    answer("declared_max", SlotValue::Count(*declared_max)),
                ],
            )
        }
        Operation::SetGuardTrigger { guard, trigger } => {
            let GuardTrigger::Timer(TimerSpec::Duration { ms }) = trigger else {
                return None;
            };
            (
                "op.set_guard_trigger",
                *guard,
                vec![answer("trigger", SlotValue::DurationMillis(*ms))],
            )
        }
        Operation::SetGuardBudget { guard, failure_budget } => {
            let budget = (*failure_budget)?;
            (
                "op.set_guard_budget",
                *guard,
                vec![answer("failure_budget", SlotValue::Count(budget))],
            )
        }
        Operation::SetCorrelationSource { node, corr_key_source } => (
            "op.set_correlation_source",
            *node,
            vec![answer(
                "data_reference",
                SlotValue::DataReference(corr_key_source.clone()),
            )],
        ),
        Operation::DeleteNode { target } => ("op.delete_subgraph", *target, Vec::new()),
        _ => return None,
    };
    Some(RecoveredShape { candidate_id, anchor, answers })
}

/// Start a workbook from exactly the argument schema on the selected semantic
/// board. No undeclared convenience slot can enter this structure.
pub(crate) fn start_workbook(
    dag: &DesignerDag,
    anchor: Option<NodeKey>,
    board: &SemanticDecisionBoard,
    selected: SelectedMove<'_>,
    evidence: WorkbookEvidence<'_>,
    utterance: &str,
    source_utterance_seq: u64,
) -> Result<ProposalWorkbook, ProposalError> {
    let evidence_record_hash = match evidence {
        WorkbookEvidence::Decision(decision) => {
            if decision.board_hash != board.board_hash.as_str() {
                return Err(ProposalError::Contract(
                    "decision record and semantic board hashes differ".to_string(),
                ));
            }
            EvidenceRecordHash::new(decision.decision_record_hash.clone())
                .map_err(|error| ProposalError::Contract(error.to_string()))?
        }
        WorkbookEvidence::PaletteSelection(receipt_hash) => receipt_hash,
    };
    let legal_move = selected
        .position
        .legal_moves()
        .iter()
        .find(|legal_move| legal_move.move_id() == selected.move_id)
        .ok_or_else(|| {
            ProposalError::Contract("selected move is absent from the position".into())
        })?;
    let candidate_id = legal_move.candidate_id();
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

    ProposalWorkbook::new_position_bound(
        1,
        WorkbookId::new(Uuid::new_v4().to_string())
            .map_err(|error| ProposalError::Contract(error.to_string()))?,
        source_utterance_seq,
        board.board_hash.clone(),
        selected.position,
        selected.move_id.clone(),
        slots,
        evidence_record_hash,
    )
    .map_err(|error| ProposalError::Contract(error.to_string()))
}

pub(crate) fn parse_condition(value: &str) -> Result<ConditionExpr, ProposalError> {
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

/// Convert a complete workbook into the exhaustive Designer operation shape.
/// The capability facade owns deterministic identity derivation and exhaustive
/// mutation semantics; this application wrapper retains its existing internal
/// storage shape without becoming a second binder implementation.
#[cfg(test)]
pub(crate) fn materialize_operations(
    dag: &DesignerDag,
    workbook: &ProposalWorkbook,
) -> Result<BoundProposal, ProposalError> {
    let bound = utterance_engine::bpmn_board::materialize_bpmn_workbook(dag, workbook)
        .map_err(|error| ProposalError::Materialization(error.to_string()))?;
    Ok(BoundProposal {
        ops: bound.operations().to_vec(),
        description: bound.description().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_types::{DataObjectRole, DataObjectType, PrimitiveType};
    use designer_graph::schema::Provenance;
    use proptest::prelude::*;
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
    fn condition_render_round_trips_through_parse() {
        for text in ["approved==true", "approved==false", "attempts>2", "x<3", "y!=1"] {
            let expr = parse_condition(text).unwrap();
            assert_eq!(render_condition(&expr), text);
        }
    }

    fn tiny_dag() -> (DesignerDag, NodeKey, NodeKey) {
        let mut dag = DesignerDag::new("recover-shape-test");
        let start = NodeKey(Uuid::new_v4());
        dag.seed(start, IRNode::Start { id: "start".into() }, Provenance::default())
            .unwrap();
        let t1 = NodeKey(Uuid::new_v4());
        let staged = designer_graph::productions::apply_production(
            &dag,
            vec![Operation::AppendNode {
                anchor: start,
                key: t1,
                node: IRNode::ServiceTask {
                    id: "t1".into(),
                    name: "t1".into(),
                    task_type: "noop".into(),
                },
                edge_id: "f1".into(),
            }],
            Provenance::default(),
        )
        .unwrap();
        (staged.candidate, start, t1)
    }

    /// v0.8 regression: folding `op.delete_subgraph` into the general
    /// recover-synthesize-materialize-compare mechanism must not change what
    /// gets recovered for it — same candidate id, same anchor, no other
    /// required arguments, exactly as the pre-existing delete-only path.
    #[test]
    fn recover_candidate_shape_delete_node_is_unchanged() {
        let (dag, _start, t1) = tiny_dag();
        let shape = recover_candidate_shape(&dag, &[Operation::DeleteNode { target: t1 }])
            .expect("delete must recover a candidate shape");
        assert_eq!(shape.candidate_id, "op.delete_subgraph");
        assert_eq!(shape.anchor, t1);
        assert!(shape.answers.is_empty());
    }

    /// v0.9 (multi-op tranche): a 2-`Operation` `[AttachGuard, AppendNode]`
    /// tape chained on the guard's own minted key recovers `op.attach_guard`
    /// with the escape identifier and duration answers; `[AttachRearmingGuard,
    /// AppendNode]` with a `Cycle` trigger recovers `op.attach_rearming_guard`
    /// with escape/interval/max_fires. See the note in `rest.rs` on why these
    /// two arms are proven at this unit level rather than an HTTP round-trip.
    #[test]
    fn recover_candidate_shape_attach_guard_and_attach_rearming_guard() {
        let (dag, _start, t1) = tiny_dag();
        let guard_key = fresh_key();
        let escape_key = fresh_key();

        let shape = recover_candidate_shape(
            &dag,
            &[
                Operation::AttachGuard {
                    host: t1,
                    key: guard_key,
                    guard_id: "escape_guard".into(),
                    trigger: GuardTrigger::Timer(TimerSpec::Duration { ms: 900_000 }),
                },
                Operation::AppendNode {
                    anchor: guard_key,
                    key: escape_key,
                    node: task("escape"),
                    edge_id: "flow_escape".into(),
                },
            ],
        )
        .expect("attach_guard must recover a candidate shape");
        assert_eq!(shape.candidate_id, "op.attach_guard");
        assert_eq!(shape.anchor, t1);
        assert_eq!(shape.answers.len(), 2);
        assert!(matches!(&shape.answers[0].value, SlotValue::Identifier(v) if v == "escape"));
        assert!(matches!(shape.answers[1].value, SlotValue::DurationMillis(900_000)));

        let guard_key = fresh_key();
        let escape_key = fresh_key();
        let shape = recover_candidate_shape(
            &dag,
            &[
                Operation::AttachRearmingGuard {
                    host: t1,
                    key: guard_key,
                    guard_id: "escape_guard".into(),
                    trigger: GuardTrigger::Timer(TimerSpec::Cycle {
                        interval_ms: 60_000,
                        max_fires: 5,
                    }),
                },
                Operation::AppendNode {
                    anchor: guard_key,
                    key: escape_key,
                    node: task("escape"),
                    edge_id: "flow_escape".into(),
                },
            ],
        )
        .expect("attach_rearming_guard must recover a candidate shape");
        assert_eq!(shape.candidate_id, "op.attach_rearming_guard");
        assert_eq!(shape.anchor, t1);
        assert_eq!(shape.answers.len(), 3);
        assert!(matches!(&shape.answers[0].value, SlotValue::Identifier(v) if v == "escape"));
        assert!(matches!(shape.answers[1].value, SlotValue::DurationMillis(60_000)));
        assert!(matches!(shape.answers[2].value, SlotValue::Count(5)));
    }

    /// v0.9 (multi-op tranche): a 2-`Operation` chained-`InsertAfter` tape
    /// `[InsertAfter(anchor->send), InsertAfter(send_key->wait)]` recovers
    /// `prod.request_and_wait` with the request identifier and correlation
    /// source answers. See the note in `rest.rs` on why this arm is proven
    /// at this unit level rather than an HTTP round-trip (no `Operation`
    /// variant can seed the `IRNode::DataObject` the correlation source
    /// would need to reference for full compiler admission).
    #[test]
    fn recover_candidate_shape_request_and_wait_resolves_the_far_endpoint() {
        let (dag, _start, t1) = tiny_dag();
        let send_key = fresh_key();
        let shape = recover_candidate_shape(
            &dag,
            &[
                Operation::InsertAfter {
                    anchor: t1,
                    key: send_key,
                    node: task("request_quote"),
                    edge_id: "flow_request_quote".into(),
                },
                Operation::InsertAfter {
                    anchor: send_key,
                    key: fresh_key(),
                    node: IRNode::MessageWait {
                        id: "request_quote_response".into(),
                        name: "request_quote_response".into(),
                        corr_key_source: "quote_id".into(),
                    },
                    edge_id: "flow_request_quote_response".into(),
                },
            ],
        )
        .expect("request_and_wait must recover a candidate shape");
        assert_eq!(shape.candidate_id, "prod.request_and_wait");
        assert_eq!(shape.anchor, t1);
        assert_eq!(shape.answers.len(), 2);
        assert!(matches!(&shape.answers[0].value, SlotValue::Identifier(v) if v == "request_quote"));
        assert!(
            matches!(&shape.answers[1].value, SlotValue::DataReference(v) if v == "quote_id")
        );
    }

    /// v0.8: `Connect`'s non-anchor endpoint round-trips through
    /// `bpmn_id_for_key` into a `NodeReference` answer.
    #[test]
    fn recover_candidate_shape_connect_resolves_the_far_endpoint() {
        let (dag, start, t1) = tiny_dag();
        let shape = recover_candidate_shape(
            &dag,
            &[Operation::Connect { from: start, to: t1, edge_id: "f2".into(), condition: None }],
        )
        .expect("connect must recover a candidate shape");
        assert_eq!(shape.candidate_id, "op.connect");
        assert_eq!(shape.anchor, start);
        assert_eq!(shape.answers.len(), 1);
        assert_eq!(shape.answers[0].name, "to");
        assert!(matches!(&shape.answers[0].value, SlotValue::NodeReference(id) if id == "t1"));
    }

    /// v0.8: a `create_branch` condition shape `materialize_workbook` never
    /// produces (anything but `Eq`/`Bool(true)`) refuses recovery outright
    /// rather than guessing.
    #[test]
    fn recover_candidate_shape_create_branch_refuses_unproducible_condition() {
        let (dag, start, t1) = tiny_dag();
        let unproducible = Operation::CreateBranch {
            gateway: start,
            target: t1,
            edge_id: "f2".into(),
            condition: Some(ConditionExpr {
                flag_name: "outcome".into(),
                op: ConditionOp::Neq,
                literal: ConditionLiteral::Bool(true),
            }),
        };
        assert!(matches!(
            recover_candidate_shape(&dag, &[unproducible]),
            Err(ShapeRefusal::NotProducible)
        ));
    }

    /// v0.8: `set_guard_budget` always materializes `Some(budget)`; a raw
    /// `None` cannot have come from this candidate, so recovery refuses.
    #[test]
    fn recover_candidate_shape_set_guard_budget_refuses_none() {
        let (dag, _start, t1) = tiny_dag();
        let never_producible =
            Operation::SetGuardBudget { guard: t1, failure_budget: None };
        assert!(matches!(
            recover_candidate_shape(&dag, &[never_producible]),
            Err(ShapeRefusal::NotProducible)
        ));
    }

    #[test]
    fn bounded_extractors_do_not_panic_on_hostile_text() {
        let text = "\0 every 999999999999999999999999 weeks 999999999999 times \"\"";
        assert!(interval_ms(text).is_none());
        assert!(quoted_names(text).is_empty());
    }

    proptest! {
        #[test]
        fn arbitrary_slot_answers_cannot_bypass_declared_identifier_type(
            answer_name in ".{0,64}",
            text in ".{0,256}",
            number in any::<u64>(),
            flag in any::<bool>(),
            variant in 0u8..9,
        ) {
            let value = match variant {
                0 => SlotValue::Text(text),
                1 => SlotValue::Identifier(text),
                2 => SlotValue::NodeReference(text),
                3 => SlotValue::DataReference(text),
                4 => SlotValue::Count(number as u32),
                5 => SlotValue::DurationMillis(number),
                6 => SlotValue::Condition(text),
                7 => SlotValue::SubprocessReference(text),
                _ => SlotValue::Boolean(flag),
            };
            let workbook = ProposalWorkbook::new(
                1,
                WorkbookId::new("property-workbook").unwrap(),
                1,
                semantic_decision_contracts::BoardHash::new("b".repeat(64)).unwrap(),
                semantic_decision_contracts::GraphRevision::new("property-revision").unwrap(),
                CanonicalCandidateId::new("op.append_node").unwrap(),
                vec![WorkbookSlot {
                    name: "name".into(),
                    kind: ArgumentKind::Identifier,
                    requirement: SlotRequirement::Required,
                    value: SlotValueState::Missing,
                    provenance: None,
                    clarification_prompt: "Which name?".into(),
                }],
                EvidenceRecordHash::new("e".repeat(64)).unwrap(),
            )
            .unwrap();
            let result = apply_explicit_answers(
                &DesignerDag::new("property-answer"),
                workbook,
                vec![SlotAnswer { name: answer_name, value }],
            );
            if let Ok(workbook) = result {
                prop_assert_eq!(workbook.status(), ProposalStatus::ReadyForDryRun);
                let resolved = &workbook.slots()[0].value;
                prop_assert!(matches!(
                    resolved,
                    SlotValueState::Resolved(SlotValue::Identifier(identifier))
                        if valid_identifier(identifier)
                ));
            }
        }

        #[test]
        fn arbitrary_text_never_panics_deterministic_extractors(text in ".{0,4096}") {
            let _ = quoted_names(&text);
            let _ = durations(&text);
            let _ = followed_count(&text, &["times", "branches", "items"]);
            let _ = parse_condition(&text);
            let sanitized = sanitize_identifier(&text);
            let identifier_alphabet = sanitized.chars().all(|character| {
                character.is_alphanumeric() || character == '_'
            });
            prop_assert!(identifier_alphabet);
        }
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
        let position = utterance_engine::bpmn_board::build_bpmn_design_position(
            &dag,
            &board,
            "revision-1",
            &"a".repeat(64),
            "compiler-profile-test",
            &"b".repeat(64),
            semantic_decision_contracts::DesignFocus::element(
                semantic_decision_contracts::GraphElementRef::new("review").unwrap(),
            ),
            None,
        )
        .unwrap();
        let move_id = position
            .legal_moves()
            .iter()
            .find(|legal_move| legal_move.candidate_id() == &candidate_id)
            .unwrap()
            .move_id()
            .clone();
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
            board: Some(utterance_engine::corpus_schema::BoardDump::from_inference_board(&board)),
            action_span_producer_hash: utterance_engine::disposition::producer_hash(
                utterance_engine::disposition::NO_ACTION_SPAN_PRODUCER_ID,
            ),
            decision_record_hash: "d".repeat(64),
        };
        let workbook = start_workbook(
            &dag,
            Some(anchor),
            &board,
            SelectedMove {
                position: &position,
                move_id: &move_id,
            },
            WorkbookEvidence::Decision(&decision),
            "Send 'onboarding_request' and wait for its response",
            7,
        )
        .unwrap();
        assert_eq!(workbook.status(), ProposalStatus::NeedsArguments);
        let palette_hash = EvidenceRecordHash::new("e".repeat(64)).unwrap();
        let palette_workbook = start_workbook(
            &dag,
            Some(anchor),
            &board,
            SelectedMove {
                position: &position,
                move_id: &move_id,
            },
            WorkbookEvidence::PaletteSelection(palette_hash.clone()),
            "",
            7,
        )
        .unwrap();
        assert_eq!(palette_workbook.evidence_record_hash, palette_hash);
        assert_eq!(
            palette_workbook.position_binding().unwrap().move_id(),
            workbook.position_binding().unwrap().move_id()
        );
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
        let first = materialize_operations(&dag, &workbook).unwrap();
        let second = materialize_operations(&dag, &workbook).unwrap();
        assert_eq!(
            serde_json::to_value(&first.ops).unwrap(),
            serde_json::to_value(&second.ops).unwrap(),
            "workbook reconstruction must preserve every generated graph identity"
        );
        let preview = utterance_engine::bpmn_board::preview_bpmn_workbook(
            &dag,
            &workbook,
            workbook.graph_revision.as_str(),
            &"a".repeat(64),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(preview.bound().operations()).unwrap(),
            serde_json::to_value(&first.ops).unwrap()
        );
        let replayed_preview = utterance_engine::bpmn_board::preview_bpmn_workbook(
            &dag,
            &workbook,
            workbook.graph_revision.as_str(),
            &"a".repeat(64),
        )
        .unwrap();
        assert_eq!(preview.delta(), replayed_preview.delta());
        let dry = productions::apply_production(&dag, first.ops, Provenance::default()).unwrap();
        dry.candidate.admit().unwrap();
        assert_eq!(dag.node_count(), 4, "workbook processing mutates no graph");

        let delete_workbook = ProposalWorkbook::new(
            1,
            WorkbookId::new("compiler-refused-delete").unwrap(),
            8,
            board.board_hash.clone(),
            board.graph_revision.clone(),
            CanonicalCandidateId::new("op.delete_subgraph").unwrap(),
            vec![WorkbookSlot {
                name: "target".into(),
                kind: ArgumentKind::NodeReference,
                requirement: SlotRequirement::Required,
                value: SlotValueState::Resolved(SlotValue::NodeReference("review".into())),
                provenance: Some(BindingProvenance {
                    source: "test".into(),
                    detail: "known anchor".into(),
                }),
                clarification_prompt: "Which node should be deleted?".into(),
            }],
            EvidenceRecordHash::new("f".repeat(64)).unwrap(),
        )
        .unwrap();
        let refusal = utterance_engine::bpmn_board::preview_bpmn_workbook(
            &dag,
            &delete_workbook,
            delete_workbook.graph_revision.as_str(),
            &"a".repeat(64),
        )
        .unwrap_err();
        assert!(matches!(
            refusal,
            utterance_engine::bpmn_board::BpmnBoardError::CompilerRefused {
                diagnostics,
                ..
            } if !diagnostics.is_empty()
        ));
        assert_eq!(dag.node_count(), 4, "refused preview mutates no graph");
    }
}
