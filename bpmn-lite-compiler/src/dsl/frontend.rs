use super::plan::{ExecutionNode, JoinMode, SplitMode, WorkflowExecutionPlan};
use crate::VerifiedWorkflow;
use bpmn_lite_types::{
    legacy_program, Addr, ArtifactEnvelope, ExecutableWorkflow, Instr, PayloadRouteBranch,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendError {
    MissingNode(String),
    InvalidRoute(String),
    CyclicOrUnreachable(Vec<String>),
    UnsupportedLoop(String),
    /// WS-D D1: the plan FORMAT can now express guards and timer waits,
    /// but this frontend's opcode emission for them lands in WS-D D2 —
    /// until then a guard-bearing plan is refused by name, never lowered
    /// with its guards silently dropped (that would be exactly the
    /// BPMN_BOUNDARY_EVENT_BYPASS `analyze_safety` exists to catch).
    UnsupportedPlanConstruct(String),
    Artifact(String),
}

impl std::fmt::Display for FrontendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingNode(node) => write!(formatter, "DSL plan references missing node {node}"),
            Self::InvalidRoute(node) => write!(formatter, "DSL split {node} has an invalid route"),
            Self::CyclicOrUnreachable(nodes) => {
                write!(
                    formatter,
                    "DSL plan is cyclic or unreachable: {}",
                    nodes.join(", ")
                )
            }
            Self::UnsupportedLoop(node) => {
                write!(formatter, "DSL loop {node} requires bounded-loop lowering")
            }
            Self::UnsupportedPlanConstruct(message) => {
                write!(formatter, "DSL plan construct not yet lowerable: {message}")
            }
            Self::Artifact(message) => write!(formatter, "DSL artifact rejected: {message}"),
        }
    }
}

impl std::error::Error for FrontendError {}

/// A source language which lowers into the one verifier-admitted runtime form.
pub trait WorkflowFrontend {
    type Source: ?Sized;

    fn lower(source: &Self::Source) -> Result<VerifiedWorkflow, FrontendError>;
}

pub struct DslFrontend;

impl WorkflowFrontend for DslFrontend {
    type Source = WorkflowExecutionPlan;

    fn lower(plan: &WorkflowExecutionPlan) -> Result<VerifiedWorkflow, FrontendError> {
        lower_plan(plan)
    }
}

pub fn lower_plan(plan: &WorkflowExecutionPlan) -> Result<VerifiedWorkflow, FrontendError> {
    // WS-D D1 gate: guards and waits are representable in the plan but
    // their opcode emission is D2's step. Refuse up front with the node
    // named — a silent drop here would un-guard a task the author guarded.
    for node in plan.nodes.values() {
        match node {
            ExecutionNode::Wait(w) => {
                return Err(FrontendError::UnsupportedPlanConstruct(format!(
                    "wait node '{}' — plan-path timer lowering lands in WS-D D2",
                    w.id
                )));
            }
            ExecutionNode::Task(t) if !t.guards.is_empty() => {
                return Err(FrontendError::UnsupportedPlanConstruct(format!(
                    "task '{}' carries {} boundary guard(s) — plan-path guard lowering lands in WS-D D2",
                    t.id,
                    t.guards.len()
                )));
            }
            _ => {}
        }
    }

    let order = topological_order(plan)?;
    let mut addresses: BTreeMap<String, Addr> = BTreeMap::new();
    let mut address = Addr::new(0);
    for node_id in &order {
        let node = plan
            .nodes
            .get(node_id)
            .ok_or_else(|| FrontendError::MissingNode(node_id.clone()))?;
        addresses.insert(node_id.clone(), address);
        address = address.saturating_add(instruction_count(node)?);
    }

    let mut instructions = Vec::new();
    let mut debug_map = BTreeMap::new();
    let mut task_manifest = Vec::new();
    let mut task_ids = BTreeMap::new();
    let join_plan = BTreeMap::new();
    let mut fork_pairing: BTreeMap<String, Addr> = BTreeMap::new();

    for node_id in &order {
        let node = &plan.nodes[node_id];
        let base = addresses[node_id];
        debug_map.insert(base, node_id.clone());
        match node {
            ExecutionNode::Start(node) => {
                instructions.push(Instr::Jump {
                    target: target(&addresses, &node.next)?,
                });
            }
            ExecutionNode::Task(node) => {
                let task_type = intern_task(&node.plug, &mut task_ids, &mut task_manifest);
                instructions.push(Instr::ExecDslTask {
                    task_type,
                    static_args: node
                        .static_args
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                    produces_placeholder: node.produces_placeholder.clone(),
                });
                instructions.push(Instr::Jump {
                    target: target(&addresses, &node.next)?,
                });
            }
            ExecutionNode::Split(node) => {
                if let Some(operation) = &node.routing_socket {
                    let task_type = intern_task(operation, &mut task_ids, &mut task_manifest);
                    instructions.push(Instr::ExecDslTask {
                        task_type,
                        static_args: BTreeMap::new(),
                        produces_placeholder: node.produces_placeholder.clone(),
                    });
                }
                match node.mode {
                    SplitMode::Exclusive => {
                        let (branches, default_target) = routes(node, &addresses)?;
                        instructions.push(Instr::RoutePayload {
                            branches: branches.into_boxed_slice(),
                            default_target,
                        });
                    }
                    SplitMode::Parallel => {
                        let pairing = Addr::new(instructions.len() as u32);
                        instructions.push(Instr::V2Fork {
                            targets: node
                                .flows
                                .iter()
                                .map(|flow| target(&addresses, &flow.next))
                                .collect::<Result<Vec<_>, _>>()?
                                .into_boxed_slice(),
                            pairing,
                        });
                        fork_pairing.insert(node.join.clone(), pairing);
                    }
                    // V5 (§18 ruling H): DSL inclusive split lowers to
                    // `V2Fork` using the same dynamic-arity skip-to-join
                    // pattern as the XML frontend
                    // (`lowering::lower_inclusive_diverging_v2`) —
                    // unconditionally, not behind a `LoweringTarget` gate
                    // (unlike the XML frontend's boundary-timer/inclusive-
                    // gateway split): no test locks `lower_plan`'s v1
                    // `ForkPayload`/`JoinDynamic` shape for
                    // `SplitMode::Inclusive` (confirmed by grep across
                    // `bpmn-lite-compiler`/`bpmn-lite-engine`'s test
                    // suites — `bpmn_lite_authoring::publish::
                    // compile_program_from_dto`, which DOES lock
                    // `lowering::lower`'s v1 shape via T-AUTH-2, never
                    // routes through this DSL frontend at all), so this
                    // mirrors the precedent already set by `GatewayAnd`/
                    // standalone `TimerWait` (V5.1/5.2): replace directly.
                    SplitMode::Inclusive => {
                        let join_addr = target(&addresses, &node.join)?;
                        lower_dsl_inclusive_diverging_v2(
                            &node.flows,
                            Addr::new(instructions.len() as u32),
                            join_addr,
                            &addresses,
                            &mut instructions,
                        )?;
                    }
                }
            }
            ExecutionNode::Join(node) => match node.mode {
                JoinMode::Exclusive => instructions.push(Instr::Jump {
                    target: target(&addresses, &node.next)?,
                }),
                JoinMode::Parallel => {
                    let pairing = *fork_pairing
                        .get(&node.id)
                        .ok_or_else(|| FrontendError::MissingNode(node.id.clone()))?;
                    instructions.push(Instr::V2Join { pairing });
                    instructions.push(Instr::Jump {
                        target: target(&addresses, &node.next)?,
                    });
                }
                JoinMode::Inclusive => {
                    let ExecutionNode::Split(split) = &plan.nodes[&node.split] else {
                        return Err(FrontendError::MissingNode(node.split.clone()));
                    };
                    let routing_task_len: u32 = if split.routing_socket.is_some() { 1 } else { 0 };
                    let fork_block_base = addresses[&node.split] + routing_task_len;
                    let pairing = fork_block_base + dsl_inclusive_precheck_len(&split.flows);
                    instructions.push(Instr::V2Join { pairing });
                    // `V2Join` carries no `next` field (continuation is
                    // PC+1 on last arrival, per K-3) — same trailing-`Jump`
                    // pattern as `JoinMode::Parallel` above.
                    instructions.push(Instr::Jump {
                        target: target(&addresses, &node.next)?,
                    });
                }
            },
            ExecutionNode::Loop(node) => {
                let counter_id = u32::from_le_bytes(
                    blake3::hash(node.id.as_bytes()).as_bytes()[..4]
                        .try_into()
                        .map_err(|_| FrontendError::UnsupportedLoop(node.id.clone()))?,
                );
                instructions.push(Instr::IncCounter { counter_id });
                instructions.push(Instr::BrCounterLt {
                    counter_id,
                    limit: node.ceiling.saturating_add(1),
                    target: node
                        .body
                        .first()
                        .map(|body| target(&addresses, body))
                        .transpose()?
                        .unwrap_or(target(&addresses, &node.next)?),
                });
                instructions.push(Instr::Jump {
                    target: target(&addresses, &node.next)?,
                });
            }
            ExecutionNode::End(_) => instructions.push(Instr::End),
            // Unreachable behind the D1 gate at the top of this function;
            // kept as a hard refusal (not `unreachable!`) so a future
            // caller path that skips the gate still fails closed.
            ExecutionNode::Wait(w) => {
                return Err(FrontendError::UnsupportedPlanConstruct(format!(
                    "wait node '{}' — plan-path timer lowering lands in WS-D D2",
                    w.id
                )));
            }
        }
    }

    let instruction_bytes = serde_json::to_vec(&instructions)
        .map_err(|error| FrontendError::Artifact(error.to_string()))?;
    let bytecode_version = blake3::hash(&instruction_bytes).into();
    let legacy = legacy_program! {
        bytecode_version: bytecode_version,
        program: instructions,
        debug_map: debug_map,
        join_plan: join_plan,
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: task_manifest,
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    let envelope = ArtifactEnvelope::from_legacy_program(legacy, env!("CARGO_PKG_VERSION"))
        .map_err(|error| FrontendError::Artifact(error.to_string()))?;
    ExecutableWorkflow::from_verified_envelope(envelope)
        .map_err(|error| FrontendError::Artifact(error.to_string()))
}

fn instruction_count(node: &ExecutionNode) -> Result<u32, FrontendError> {
    match node {
        ExecutionNode::Task(_) => Ok(2),
        // V5 (§18 ruling H): an inclusive split's own v2 block (zero-match
        // precheck + `V2Fork` + per-branch headers — see
        // `dsl_inclusive_diverging_len`) is variable-length, unlike every
        // other split mode's fixed 1-or-2. The optional leading
        // `routing_socket` task (any split mode may carry one) still costs
        // exactly 1 instruction, ahead of the inclusive block.
        ExecutionNode::Split(node) if node.mode == SplitMode::Inclusive => {
            let routing_task_len: u32 = if node.routing_socket.is_some() { 1 } else { 0 };
            Ok(routing_task_len + dsl_inclusive_diverging_len(&node.flows))
        }
        ExecutionNode::Split(node) if node.routing_socket.is_some() => Ok(2),
        // V5.2 mechanical re-lowering: `V2Join` carries no `next` field
        // (kernel continuation is PC+1 on last arrival, per K-3) — an
        // explicit trailing `Jump` supplies what v1's embedded `next` used to.
        // V5 (§18 ruling H): `JoinMode::Inclusive` now lowers to the same
        // `V2Join` + `Jump` pair as `JoinMode::Parallel`.
        ExecutionNode::Join(node)
            if node.mode == JoinMode::Parallel || node.mode == JoinMode::Inclusive =>
        {
            Ok(2)
        }
        ExecutionNode::Loop(_) => Ok(3),
        _ => Ok(1),
    }
}

/// An inclusive-split flow is "always-live" (unconditional — always
/// included in the fork's target set, matching XML's `InclusiveBranch::
/// condition_flag: None` convention and the same code path as a DSL
/// `default_target`, see `lower_dsl_inclusive_diverging_v2`'s doc comment)
/// iff it carries neither a placeholder nor an expected value — the same
/// `(None, None)` shape `routes()` already recognises as a default flow
/// for `SplitMode::Exclusive`.
fn dsl_flow_is_always_live(flow: &super::plan::SplitExecFlow) -> bool {
    flow.placeholder.is_none() && flow.expected_value.is_none()
}

/// The zero-match precheck's own instruction count (§18 ruling J) — 0 when
/// an always-live flow exists, otherwise 2 per conditional flow
/// (`V2LoadPlaceholderMatch` + `BrIf`) plus 1 for `V2RouteZeroMatch`. The
/// DSL-side mirror of `lowering::inclusive_precheck_len`, shared by the
/// sizing pass (`dsl_inclusive_diverging_len`) and the `JoinMode::
/// Inclusive` emission arm, which needs to resolve its paired `V2Fork`'s
/// own address the same way.
fn dsl_inclusive_precheck_len(flows: &[super::plan::SplitExecFlow]) -> u32 {
    if flows.iter().any(dsl_flow_is_always_live) {
        0
    } else {
        let conditional_count = flows.iter().filter(|flow| !dsl_flow_is_always_live(flow)).count() as u32;
        conditional_count.saturating_mul(2) + 1
    }
}

/// An inclusive split's total v2 block length: the zero-match precheck,
/// `V2Fork` itself, and one per-flow header (3 instructions — see
/// `lower_dsl_inclusive_diverging_v2` — for a conditional flow, 1 for an
/// always-live one). The DSL-side mirror of
/// `lowering::inclusive_diverging_instr_count`.
fn dsl_inclusive_diverging_len(flows: &[super::plan::SplitExecFlow]) -> u32 {
    let fork = 1;
    let headers: u32 = flows
        .iter()
        .map(|flow| if dsl_flow_is_always_live(flow) { 1 } else { 3 })
        .sum();
    dsl_inclusive_precheck_len(flows) + fork + headers
}

/// V5 (§18 ruling H): lower a `SplitMode::Inclusive` node's flows to
/// `V2Fork` using the dynamic-arity skip-to-join pattern — the DSL-side
/// mirror of `lowering::lower_inclusive_diverging_v2`, differing only in
/// condition representation: XML's inclusive-gateway conditions are
/// flag-based (`LoadFlag`, reused as-is); the DSL's are
/// `(placeholder, expected_value)` string-equality checks
/// (`instance.placeholder_matches`), which have no existing operand-stack-
/// producing bytecode primitive — `Instr::ForkPayload`'s v1 kernel handler
/// evaluated them internally, never exposing a "load and check" step.
/// `V2LoadPlaceholderMatch` (the DSL-side analogue of `LoadFlag`, added by
/// this step) closes that gap; everything else about the shape — the
/// zero-match precheck omitted when an always-live flow exists (a
/// `(None, None)` flow, the same shape `routes()` already recognises as
/// `SplitMode::Exclusive`'s default — this is DSL's complete answer to the
/// brief's "default branch handling" requirement, structurally identical
/// to XML's "no separate default concept beyond unconditional," not a
/// partial one), the per-branch skip-check header, the shared `V2Join` —
/// is identical to the XML lowering, see its doc comment for the full
/// design rationale (zero-match-before-`V2Fork`, the two-new-opcode
/// decision).
fn lower_dsl_inclusive_diverging_v2(
    flows: &[super::plan::SplitExecFlow],
    fork_block_base: Addr,
    join_addr: Addr,
    addresses: &BTreeMap<String, Addr>,
    instructions: &mut Vec<Instr>,
) -> Result<(), FrontendError> {
    for flow in flows {
        if !(matches!((&flow.placeholder, &flow.expected_value), (Some(_), Some(_)))
            || dsl_flow_is_always_live(flow))
        {
            return Err(FrontendError::InvalidRoute(flow.next.clone()));
        }
    }

    let precheck_len = dsl_inclusive_precheck_len(flows);
    let fork_addr = fork_block_base + precheck_len;

    if precheck_len > 0 {
        for flow in flows {
            let placeholder = flow.placeholder.clone().expect("validated above");
            let expected_value = flow.expected_value.clone().expect("validated above");
            instructions.push(Instr::V2LoadPlaceholderMatch {
                placeholder,
                expected_value,
            });
            instructions.push(Instr::BrIf { target: fork_addr });
        }
        instructions.push(Instr::V2RouteZeroMatch);
    }

    let mut header_addr = fork_addr + 1u32;
    let mut headers: Vec<Addr> = Vec::with_capacity(flows.len());
    for flow in flows {
        headers.push(header_addr);
        header_addr += if dsl_flow_is_always_live(flow) { 1u32 } else { 3u32 };
    }

    instructions.push(Instr::V2Fork {
        targets: headers.into_boxed_slice(),
        pairing: fork_addr,
    });

    for flow in flows {
        let real_target = target(addresses, &flow.next)?;
        if dsl_flow_is_always_live(flow) {
            instructions.push(Instr::Jump { target: real_target });
        } else {
            let placeholder = flow.placeholder.clone().expect("validated above");
            let expected_value = flow.expected_value.clone().expect("validated above");
            instructions.push(Instr::V2LoadPlaceholderMatch {
                placeholder,
                expected_value,
            });
            instructions.push(Instr::BrIfNot { target: join_addr });
            instructions.push(Instr::Jump { target: real_target });
        }
    }
    Ok(())
}

fn target(addresses: &BTreeMap<String, Addr>, node: &str) -> Result<Addr, FrontendError> {
    addresses
        .get(node)
        .copied()
        .ok_or_else(|| FrontendError::MissingNode(node.to_string()))
}

fn intern_task(
    operation: &str,
    task_ids: &mut BTreeMap<String, u32>,
    manifest: &mut Vec<String>,
) -> u32 {
    if let Some(task_id) = task_ids.get(operation) {
        return *task_id;
    }
    let task_id = manifest.len() as u32;
    manifest.push(operation.to_string());
    task_ids.insert(operation.to_string(), task_id);
    task_id
}

fn routes(
    split: &super::plan::SplitExecNode,
    addresses: &BTreeMap<String, Addr>,
) -> Result<(Vec<PayloadRouteBranch>, Option<Addr>), FrontendError> {
    let mut branches = Vec::new();
    let mut default_target = None;
    for flow in &split.flows {
        match (&flow.placeholder, &flow.expected_value) {
            (Some(placeholder), Some(expected_value)) => branches.push(PayloadRouteBranch {
                placeholder: placeholder.clone(),
                expected_value: expected_value.clone(),
                target: target(addresses, &flow.next)?,
            }),
            (None, None) if default_target.is_none() => {
                default_target = Some(target(addresses, &flow.next)?);
            }
            _ => return Err(FrontendError::InvalidRoute(split.id.clone())),
        }
    }
    Ok((branches, default_target))
}

fn outgoing(node: &ExecutionNode) -> Vec<&str> {
    match node {
        ExecutionNode::Start(node) => vec![&node.next],
        ExecutionNode::Task(node) => vec![&node.next],
        ExecutionNode::Split(node) => node.flows.iter().map(|flow| flow.next.as_str()).collect(),
        ExecutionNode::Join(node) => vec![&node.next],
        ExecutionNode::Loop(node) => node
            .body
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(node.next.as_str()))
            .collect(),
        // D2 will extend this with guard escape entries when the guard
        // emission lands; until then the D1 gate refuses guard-bearing
        // plans before ordering runs.
        ExecutionNode::Wait(node) => vec![&node.next],
        ExecutionNode::End(_) => Vec::new(),
    }
}

fn topological_order(plan: &WorkflowExecutionPlan) -> Result<Vec<String>, FrontendError> {
    if !plan.nodes.contains_key(&plan.start_node) {
        return Err(FrontendError::MissingNode(plan.start_node.clone()));
    }
    let loop_bodies: BTreeSet<String> = plan
        .nodes
        .values()
        .filter_map(|node| match node {
            ExecutionNode::Loop(node) => Some(node.body.iter().cloned()),
            _ => None,
        })
        .flatten()
        .collect();
    let mut incoming: BTreeMap<String, usize> = plan
        .nodes
        .keys()
        .filter(|node| !loop_bodies.contains(*node))
        .map(|node| (node.clone(), 0))
        .collect();
    for node in plan.nodes.values() {
        if loop_bodies.contains(node.id()) {
            continue;
        }
        for successor in outgoing_for_order(node) {
            if loop_bodies.contains(successor) {
                continue;
            }
            let count = incoming
                .get_mut(successor)
                .ok_or_else(|| FrontendError::MissingNode(successor.to_string()))?;
            *count = count.saturating_add(1);
        }
    }
    let mut ready: BTreeSet<String> = incoming
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(node, _)| node.clone())
        .collect();
    if ready.remove(&plan.start_node) {
        ready.insert(plan.start_node.clone());
    }
    let mut order = Vec::with_capacity(plan.nodes.len());
    while !ready.is_empty() {
        let node_id = if order.is_empty() && ready.contains(&plan.start_node) {
            plan.start_node.clone()
        } else {
            ready
                .iter()
                .next()
                .cloned()
                .ok_or_else(|| FrontendError::CyclicOrUnreachable(Vec::new()))?
        };
        ready.remove(&node_id);
        order.push(node_id.clone());
        for successor in outgoing_for_order(&plan.nodes[&node_id]) {
            if loop_bodies.contains(successor) {
                continue;
            }
            let count = incoming
                .get_mut(successor)
                .ok_or_else(|| FrontendError::MissingNode(successor.to_string()))?;
            *count = count.saturating_sub(1);
            if *count == 0 {
                ready.insert(successor.to_string());
            }
        }
    }
    if order.len() + loop_bodies.len() != plan.nodes.len() {
        let seen: HashSet<_> = order.iter().cloned().collect();
        let missing = plan
            .nodes
            .keys()
            .filter(|node| !seen.contains(*node))
            .cloned()
            .collect();
        return Err(FrontendError::CyclicOrUnreachable(missing));
    }
    for node in plan.nodes.values() {
        let ExecutionNode::Loop(loop_node) = node else {
            continue;
        };
        let position = order
            .iter()
            .position(|node_id| node_id == &loop_node.id)
            .ok_or_else(|| FrontendError::MissingNode(loop_node.id.clone()))?;
        for (offset, body_node) in loop_node.body.iter().enumerate() {
            if !plan.nodes.contains_key(body_node) {
                return Err(FrontendError::MissingNode(body_node.clone()));
            }
            order.insert(position + offset, body_node.clone());
        }
    }
    Ok(order)
}

fn outgoing_for_order(node: &ExecutionNode) -> Vec<&str> {
    match node {
        ExecutionNode::Loop(node) => vec![node.next.as_str()],
        _ => outgoing(node),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::plan::{
        EndExecNode, JoinExecNode, LoopExecNode, PlaceholderSchema, SplitExecFlow, SplitExecNode,
        StartExecNode, TaskExecNode,
    };
    use crate::dsl::DeliveryMode;
    use std::collections::HashMap;

    fn routing_plan() -> WorkflowExecutionPlan {
        WorkflowExecutionPlan {
            workflow_id: "routing".to_string(),
            nodes: BTreeMap::from([
                (
                    "start".to_string(),
                    ExecutionNode::Start(StartExecNode {
                        id: "start".to_string(),
                        next: "decide".to_string(),
                        span: None,
                    }),
                ),
                (
                    "decide".to_string(),
                    ExecutionNode::Task(TaskExecNode {
                        id: "decide".to_string(),
                        plug: "dmn-lite:route".to_string(),
                        delivery_mode: DeliveryMode::GuaranteedAsync,
                        static_args: HashMap::from([("policy".to_string(), "v2".to_string())]),
                        next: "route".to_string(),
                        produces_placeholder: Some("@kind".to_string()),
                        consumes_placeholders: Vec::new(),
                        guards: Vec::new(),
                        span: None,
                    }),
                ),
                (
                    "route".to_string(),
                    ExecutionNode::Split(SplitExecNode {
                        id: "route".to_string(),
                        mode: SplitMode::Exclusive,
                        routing_socket: None,
                        flows: vec![
                            SplitExecFlow {
                                placeholder: Some("@kind".to_string()),
                                expected_value: Some("fund".to_string()),
                                next: "fund".to_string(),
                            },
                            SplitExecFlow {
                                placeholder: Some("@kind".to_string()),
                                expected_value: Some("trust".to_string()),
                                next: "trust".to_string(),
                            },
                        ],
                        join: "join".to_string(),
                        produces_placeholder: None,
                        span: None,
                    }),
                ),
                (
                    "fund".to_string(),
                    ExecutionNode::Task(TaskExecNode {
                        id: "fund".to_string(),
                        plug: "ob-poc:add-fund".to_string(),
                        delivery_mode: DeliveryMode::GuaranteedAsync,
                        static_args: HashMap::new(),
                        next: "join".to_string(),
                        produces_placeholder: None,
                        consumes_placeholders: Vec::new(),
                        guards: Vec::new(),
                        span: None,
                    }),
                ),
                (
                    "trust".to_string(),
                    ExecutionNode::Task(TaskExecNode {
                        id: "trust".to_string(),
                        plug: "ob-poc:add-trust".to_string(),
                        delivery_mode: DeliveryMode::GuaranteedAsync,
                        static_args: HashMap::new(),
                        next: "join".to_string(),
                        produces_placeholder: None,
                        consumes_placeholders: Vec::new(),
                        guards: Vec::new(),
                        span: None,
                    }),
                ),
                (
                    "join".to_string(),
                    ExecutionNode::Join(JoinExecNode {
                        id: "join".to_string(),
                        mode: JoinMode::Exclusive,
                        split: "route".to_string(),
                        next: "end".to_string(),
                        span: None,
                    }),
                ),
                (
                    "end".to_string(),
                    ExecutionNode::End(EndExecNode {
                        id: "end".to_string(),
                        status: "done".to_string(),
                        span: None,
                    }),
                ),
            ]),
            start_node: "start".to_string(),
            placeholder_schema: PlaceholderSchema::default(),
            closure_manifest: None,
            regime_version: None,
            mathematically_proved: true,
            unsafe_breeches: Vec::new(),
        }
    }

    #[test]
    fn dsl_frontend_is_deterministic_and_embeds_task_and_route_data() {
        let plan = routing_plan();
        let first = DslFrontend::lower(&plan).unwrap();
        let second = DslFrontend::lower(&plan).unwrap();
        assert_eq!(first.hash(), second.hash());

        let instructions = first.envelope().instructions();
        assert!(instructions.iter().any(|instruction| matches!(
            instruction,
            Instr::ExecDslTask {
                static_args,
                produces_placeholder: Some(placeholder),
                ..
            } if static_args.get("policy").map(String::as_str) == Some("v2")
                && placeholder == "@kind"
        )));
        assert!(instructions.iter().any(|instruction| matches!(
            instruction,
            Instr::RoutePayload { branches, .. }
                if branches.iter().any(|branch| branch.expected_value == "fund")
                    && branches.iter().any(|branch| branch.expected_value == "trust")
        )));
    }

    #[test]
    fn bounded_loop_lowers_to_verifier_admitted_counter_control_flow() {
        let mut plan = routing_plan();
        plan.nodes = BTreeMap::from([
            (
                "start".to_string(),
                ExecutionNode::Start(StartExecNode {
                    id: "start".to_string(),
                    next: "retry".to_string(),
                    span: None,
                }),
            ),
            (
                "retry".to_string(),
                ExecutionNode::Loop(LoopExecNode {
                    id: "retry".to_string(),
                    ceiling: 3,
                    body: vec!["body".to_string()],
                    next: "end".to_string(),
                    span: None,
                }),
            ),
            (
                "body".to_string(),
                ExecutionNode::Task(TaskExecNode {
                    id: "body".to_string(),
                    plug: "fixture:retry".to_string(),
                    delivery_mode: DeliveryMode::GuaranteedAsync,
                    static_args: HashMap::new(),
                    next: "retry".to_string(),
                    produces_placeholder: None,
                    consumes_placeholders: Vec::new(),
                    guards: Vec::new(),
                    span: None,
                }),
            ),
            (
                "end".to_string(),
                ExecutionNode::End(EndExecNode {
                    id: "end".to_string(),
                    status: "done".to_string(),
                    span: None,
                }),
            ),
        ]);
        plan.start_node = "start".to_string();
        let workflow = DslFrontend::lower(&plan).unwrap();
        assert!(workflow.envelope().instructions().iter().enumerate().any(
            |(address, instruction)| matches!(
                instruction,
                Instr::BrCounterLt { limit: 4, target, .. } if *target < Addr::from(address as u32)
            )
        ));
    }

    fn parallel_plan() -> WorkflowExecutionPlan {
        let mut plan = routing_plan();
        plan.nodes = BTreeMap::from([
            (
                "start".to_string(),
                ExecutionNode::Start(StartExecNode {
                    id: "start".to_string(),
                    next: "split".to_string(),
                    span: None,
                }),
            ),
            (
                "split".to_string(),
                ExecutionNode::Split(SplitExecNode {
                    id: "split".to_string(),
                    mode: SplitMode::Parallel,
                    routing_socket: None,
                    flows: vec![
                        SplitExecFlow {
                            placeholder: None,
                            expected_value: None,
                            next: "fund".to_string(),
                        },
                        SplitExecFlow {
                            placeholder: None,
                            expected_value: None,
                            next: "trust".to_string(),
                        },
                    ],
                    join: "join".to_string(),
                    produces_placeholder: None,
                    span: None,
                }),
            ),
            (
                "fund".to_string(),
                ExecutionNode::Task(TaskExecNode {
                    id: "fund".to_string(),
                    plug: "ob-poc:add-fund".to_string(),
                    delivery_mode: DeliveryMode::GuaranteedAsync,
                    static_args: HashMap::new(),
                    next: "join".to_string(),
                    produces_placeholder: None,
                    consumes_placeholders: Vec::new(),
                    guards: Vec::new(),
                    span: None,
                }),
            ),
            (
                "trust".to_string(),
                ExecutionNode::Task(TaskExecNode {
                    id: "trust".to_string(),
                    plug: "ob-poc:add-trust".to_string(),
                    delivery_mode: DeliveryMode::GuaranteedAsync,
                    static_args: HashMap::new(),
                    next: "join".to_string(),
                    produces_placeholder: None,
                    consumes_placeholders: Vec::new(),
                    guards: Vec::new(),
                    span: None,
                }),
            ),
            (
                "join".to_string(),
                ExecutionNode::Join(JoinExecNode {
                    id: "join".to_string(),
                    mode: JoinMode::Parallel,
                    split: "split".to_string(),
                    next: "end".to_string(),
                    span: None,
                }),
            ),
            (
                "end".to_string(),
                ExecutionNode::End(EndExecNode {
                    id: "end".to_string(),
                    status: "done".to_string(),
                    span: None,
                }),
            ),
        ]);
        plan.start_node = "start".to_string();
        plan
    }

    /// V5.2: DSL `Split`/`Join` mode `Parallel` lowers to `V2Fork`/`V2Join`,
    /// not v1 `Fork`/`Join` — proves the real V3 verifier (V-1..V-9,
    /// exercised via `ExecutableWorkflow::from_verified_envelope`, not a
    /// standalone call) admits the emitted pairing, and that the emitted
    /// shape is actually `V2Fork`/`V2Join` rather than the v1 words they
    /// replace.
    #[test]
    fn dsl_parallel_split_join_lowers_to_v2_fork_join_with_matching_pairing() {
        let plan = parallel_plan();
        let workflow = DslFrontend::lower(&plan).expect("v2 Fork/Join must be verifier-admitted");
        let instructions = workflow.envelope().instructions();

        let fork_pairing = instructions
            .iter()
            .enumerate()
            .find_map(|(address, instruction)| match instruction {
                Instr::V2Fork { pairing, .. } => {
                    assert_eq!(
                        pairing.index(),
                        address,
                        "V2Fork's pairing must be its own address"
                    );
                    Some(*pairing)
                }
                _ => None,
            })
            .expect("a V2Fork must be emitted");

        let join_count = instructions
            .iter()
            .filter(|instruction| match instruction {
                Instr::V2Join { pairing } => {
                    assert_eq!(*pairing, fork_pairing, "V2Join must reference the V2Fork's pairing");
                    true
                }
                _ => false,
            })
            .count();
        assert_eq!(join_count, 1, "one shared V2Join, arrived at by both branches");
    }

    // ═══════════════════════════════════════════════════════════
    //  V5 post-close (§18 ruling H) — DSL inclusive-split v2 lowering.
    // ═══════════════════════════════════════════════════════════

    /// `default_flow`: adds a third, unconditional (`(None, None)`) flow
    /// to `always` — exercises the always-live/default-branch precheck
    /// omission when `true`.
    fn inclusive_plan(default_flow: bool) -> WorkflowExecutionPlan {
        let mut plan = parallel_plan();
        let mut flows = vec![
            SplitExecFlow {
                placeholder: Some("@kind".to_string()),
                expected_value: Some("fund".to_string()),
                next: "fund".to_string(),
            },
            SplitExecFlow {
                placeholder: Some("@kind".to_string()),
                expected_value: Some("trust".to_string()),
                next: "trust".to_string(),
            },
        ];
        if default_flow {
            flows.push(SplitExecFlow {
                placeholder: None,
                expected_value: None,
                next: "always".to_string(),
            });
            plan.nodes.insert(
                "always".to_string(),
                ExecutionNode::Task(TaskExecNode {
                    id: "always".to_string(),
                    plug: "ob-poc:always".to_string(),
                    delivery_mode: DeliveryMode::GuaranteedAsync,
                    static_args: HashMap::new(),
                    next: "join".to_string(),
                    produces_placeholder: None,
                    consumes_placeholders: Vec::new(),
                    guards: Vec::new(),
                    span: None,
                }),
            );
        }
        plan.nodes.insert(
            "split".to_string(),
            ExecutionNode::Split(SplitExecNode {
                id: "split".to_string(),
                mode: SplitMode::Inclusive,
                routing_socket: None,
                flows,
                join: "join".to_string(),
                produces_placeholder: None,
                span: None,
            }),
        );
        plan.nodes.insert(
            "join".to_string(),
            ExecutionNode::Join(JoinExecNode {
                id: "join".to_string(),
                mode: JoinMode::Inclusive,
                split: "split".to_string(),
                next: "end".to_string(),
                span: None,
            }),
        );
        plan
    }

    /// A pure-conditional DSL inclusive split (no default/always-live
    /// flow) lowers to a zero-match precheck ahead of `V2Fork`, a header
    /// per branch, and a shared `V2Join` referencing the `V2Fork`'s own
    /// (post-precheck) address — not v1 `ForkPayload`/`JoinDynamic`.
    #[test]
    fn dsl_inclusive_split_pure_conditional_lowers_to_v2_fork_with_zero_match_precheck() {
        let plan = inclusive_plan(false);
        let workflow =
            DslFrontend::lower(&plan).expect("v2 inclusive split/join must be verifier-admitted");
        let instructions = workflow.envelope().instructions();

        assert!(
            instructions
                .iter()
                .any(|i| matches!(i, Instr::V2RouteZeroMatch)),
            "a pure-conditional DSL inclusive split must emit the zero-match precheck"
        );
        assert!(
            instructions
                .iter()
                .any(|i| matches!(i, Instr::V2LoadPlaceholderMatch { .. })),
            "DSL conditions must be evaluated via V2LoadPlaceholderMatch, not LoadFlag"
        );
        let fork_pairing = instructions
            .iter()
            .find_map(|i| match i {
                Instr::V2Fork { targets, pairing } => {
                    assert_eq!(targets.len(), 2, "two conditional flows → two V2Fork targets");
                    Some(*pairing)
                }
                _ => None,
            })
            .expect("a V2Fork must be emitted");
        let join_count = instructions
            .iter()
            .filter(|i| matches!(i, Instr::V2Join { pairing } if *pairing == fork_pairing))
            .count();
        assert_eq!(join_count, 1);
        // `Instr::ForkPayload`/`JoinDynamic` (v1) are deleted entirely as
        // of V5.3 (§18, landed 2026-07-23) — the negative assertions that
        // used to sit here are now vacuous (there is no variant left to
        // construct) and are removed rather than kept as dead code.
    }

    /// RED (WS-D D1 gate): the plan format can now CARRY guards and
    /// waits, but this frontend's emission for them is D2's step — until
    /// it lands, lowering a guard-bearing plan must refuse by name, never
    /// silently drop the guard (an un-guarded lowering of a guarded plan
    /// is the exact bypass analyze_safety flags on the Task-plug side).
    #[test]
    fn guard_bearing_plan_is_refused_until_d2_not_silently_unguarded() {
        let mut plan = routing_plan();
        if let Some(ExecutionNode::Task(t)) = plan.nodes.get_mut("decide") {
            t.guards.push(crate::dsl::plan::GuardExecSpec {
                guard_id: "bt".into(),
                trigger: crate::dsl::plan::GuardTriggerExec::Timer {
                    spec: crate::ir::TimerSpec::Duration { ms: 1000 },
                    interrupting: true,
                },
                failure_budget: None,
                escape_entry: "end".into(),
            });
        }
        let err = DslFrontend::lower(&plan).expect_err("guarded plan must refuse until D2");
        assert!(matches!(err, FrontendError::UnsupportedPlanConstruct(ref m) if m.contains("decide")));

        let mut plan = routing_plan();
        plan.nodes.insert(
            "w".into(),
            ExecutionNode::Wait(crate::dsl::plan::WaitExecNode {
                id: "w".into(),
                spec: crate::ir::TimerSpec::Duration { ms: 1000 },
                next: "end".into(),
                span: None,
            }),
        );
        let err = DslFrontend::lower(&plan).expect_err("wait-bearing plan must refuse until D2");
        assert!(matches!(err, FrontendError::UnsupportedPlanConstruct(ref m) if m.contains("'w'")));
    }

    /// An always-live (default) flow makes the DSL inclusive split's fork
    /// target set provably non-empty, so the zero-match precheck is
    /// omitted — DSL's answer to the brief's "default branch handling"
    /// requirement, structurally identical to the XML frontend's.
    #[test]
    fn dsl_inclusive_split_with_default_flow_omits_zero_match_precheck() {
        let plan = inclusive_plan(true);
        let workflow =
            DslFrontend::lower(&plan).expect("v2 inclusive split/join must be verifier-admitted");
        let instructions = workflow.envelope().instructions();

        assert!(
            !instructions
                .iter()
                .any(|i| matches!(i, Instr::V2RouteZeroMatch)),
            "a default/always-live flow must make the zero-match precheck unreachable, \
             so it must not be emitted at all"
        );
        let fork_targets = instructions
            .iter()
            .find_map(|i| match i {
                Instr::V2Fork { targets, .. } => Some(targets.len()),
                _ => None,
            })
            .expect("a V2Fork must be emitted");
        assert_eq!(fork_targets, 3, "two conditional + one default → three targets");
    }
}
