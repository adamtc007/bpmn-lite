use super::plan::{ExecutionNode, JoinMode, SplitMode, WorkflowExecutionPlan};
use crate::VerifiedWorkflow;
use bpmn_lite_types::{
    legacy_program, Addr, ArtifactEnvelope, ExecutableWorkflow, Instr, JoinPlanEntry,
    PayloadRouteBranch, Value,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendError {
    MissingNode(String),
    InvalidRoute(String),
    CyclicOrUnreachable(Vec<String>),
    UnsupportedLoop(String),
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
    let mut join_ids = BTreeMap::new();
    let mut next_join_id = 0u32;
    for node in plan.nodes.values() {
        if let ExecutionNode::Split(split) = node {
            if split.mode != SplitMode::Exclusive {
                join_ids.insert(split.join.clone(), next_join_id);
                next_join_id = next_join_id.saturating_add(1);
            }
        }
    }
    let mut join_plan = BTreeMap::new();

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
                        instructions.push(Instr::Fork {
                            targets: node
                                .flows
                                .iter()
                                .map(|flow| target(&addresses, &flow.next))
                                .collect::<Result<Vec<_>, _>>()?
                                .into_boxed_slice(),
                        });
                    }
                    SplitMode::Inclusive => {
                        let join_id = *join_ids
                            .get(&node.join)
                            .ok_or_else(|| FrontendError::MissingNode(node.join.clone()))?;
                        let (branches, default_target) = routes(node, &addresses)?;
                        instructions.push(Instr::ForkPayload {
                            branches: branches.into_boxed_slice(),
                            join_id,
                            default_target,
                        });
                    }
                }
            }
            ExecutionNode::Join(node) => match node.mode {
                JoinMode::Exclusive => instructions.push(Instr::Jump {
                    target: target(&addresses, &node.next)?,
                }),
                JoinMode::Parallel => {
                    let join_id = *join_ids
                        .get(&node.id)
                        .ok_or_else(|| FrontendError::MissingNode(node.id.clone()))?;
                    let expected = incoming_count(plan, &node.id);
                    let next = target(&addresses, &node.next)?;
                    join_plan.insert(
                        join_id,
                        JoinPlanEntry {
                            expected,
                            next,
                            reg_template: std::array::from_fn(|_| Value::Bool(false)),
                        },
                    );
                    instructions.push(Instr::Join {
                        id: join_id,
                        expected,
                        next,
                    });
                }
                JoinMode::Inclusive => {
                    let join_id = *join_ids
                        .get(&node.id)
                        .ok_or_else(|| FrontendError::MissingNode(node.id.clone()))?;
                    instructions.push(Instr::JoinDynamic {
                        id: join_id,
                        next: target(&addresses, &node.next)?,
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
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: task_manifest,
        error_route_map: BTreeMap::new(),
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
        ExecutionNode::Split(node) if node.routing_socket.is_some() => Ok(2),
        ExecutionNode::Loop(_) => Ok(3),
        _ => Ok(1),
    }
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

fn incoming_count(plan: &WorkflowExecutionPlan, target: &str) -> u16 {
    plan.nodes
        .values()
        .flat_map(outgoing)
        .filter(|node| *node == target)
        .count()
        .try_into()
        .unwrap_or(u16::MAX)
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
            compiled_bytecode: None,
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
}
