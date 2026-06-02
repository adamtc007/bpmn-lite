use anyhow::{anyhow, Result};
use std::collections::HashMap;
use petgraph::graph::NodeIndex;
use bpmn_lite_compiler::ir::{IRGraph, IRNode, GatewayDirection};
use bpmn_lite_compiler::dsl::plan::{
    WorkflowExecutionPlan, ExecutionNode, StartExecNode, EndExecNode, TaskExecNode,
    SplitExecNode, JoinExecNode, SplitMode, JoinMode, SplitExecFlow, PlaceholderSchema, DeliveryMode
};
use bpmn_lite_compiler::dsl::rpst::verify_sese_nesting;

/// Import a Zeebe BPMN 2.0 XML string and convert it into a SESE-structured `WorkflowExecutionPlan`.
/// Returns an error if the topology is non-SESE.
pub fn import_zeebe_bpmn(xml: &str, workflow_id: &str) -> Result<WorkflowExecutionPlan> {
    // 1. Parse BPMN XML into IRGraph
    let ir = bpmn_lite_compiler::parser::parse_bpmn(xml)?;

    // 2. Pair every Split with its corresponding Join
    let mut split_join_pairs: HashMap<NodeIndex, NodeIndex> = HashMap::new();
    let mut join_split_pairs: HashMap<NodeIndex, NodeIndex> = HashMap::new();

    for idx in ir.node_indices() {
        let node = &ir[idx];
        let is_split = match node {
            IRNode::GatewayXor { .. } => {
                let out_count = ir.neighbors_directed(idx, petgraph::Direction::Outgoing).count();
                out_count > 1
            }
            IRNode::GatewayAnd { direction: GatewayDirection::Diverging, .. } => true,
            IRNode::GatewayInclusive { direction: GatewayDirection::Diverging, .. } => true,
            _ => false,
        };

        if is_split {
            if let Some(join_idx) = find_corresponding_join(&ir, idx) {
                split_join_pairs.insert(idx, join_idx);
                join_split_pairs.insert(join_idx, idx);
            } else {
                return Err(anyhow!("Non-SESE: Split node '{}' does not have a corresponding merge/join node", node.id()));
            }
        }
    }

    // 3. Build WorkflowExecutionPlan nodes
    let mut nodes = HashMap::new();
    let mut start_node_id = None;

    for idx in ir.node_indices() {
        let node = &ir[idx];
        let node_id = node.id().to_string();

        let exec_node = match node {
            IRNode::Start { id } => {
                start_node_id = Some(id.clone());
                let next_idx = ir.neighbors_directed(idx, petgraph::Direction::Outgoing)
                    .next()
                    .ok_or_else(|| anyhow!("Start node has no outgoing edges"))?;
                ExecutionNode::Start(StartExecNode {
                    id: id.clone(),
                    next: ir[next_idx].id().to_string(),
                })
            }
            IRNode::End { id, .. } => {
                ExecutionNode::End(EndExecNode {
                    id: id.clone(),
                    status: "completed".to_string(),
                })
            }
            IRNode::ServiceTask { id, task_type, .. } => {
                let next_idx = ir.neighbors_directed(idx, petgraph::Direction::Outgoing)
                    .next()
                    .ok_or_else(|| anyhow!("ServiceTask node '{}' has no outgoing edges", id))?;
                ExecutionNode::Task(TaskExecNode {
                    id: id.clone(),
                    plug: task_type.clone(),
                    delivery_mode: DeliveryMode::Blocking,
                    static_args: HashMap::new(),
                    next: ir[next_idx].id().to_string(),
                    produces_placeholder: None,
                    consumes_placeholders: vec![],
                })
            }
            IRNode::HumanWait { id, task_kind, .. } => {
                let next_idx = ir.neighbors_directed(idx, petgraph::Direction::Outgoing)
                    .next()
                    .ok_or_else(|| anyhow!("HumanWait node '{}' has no outgoing edges", id))?;
                ExecutionNode::Task(TaskExecNode {
                    id: id.clone(),
                    plug: task_kind.clone(),
                    delivery_mode: DeliveryMode::Blocking,
                    static_args: HashMap::new(),
                    next: ir[next_idx].id().to_string(),
                    produces_placeholder: None,
                    consumes_placeholders: vec![],
                })
            }
            IRNode::GatewayXor { id, .. } => {
                if split_join_pairs.contains_key(&idx) {
                    let join_idx = split_join_pairs[&idx];
                    let join_id = ir[join_idx].id().to_string();
                    let flows = build_split_flows(&ir, idx)?;
                    ExecutionNode::Split(SplitExecNode {
                        id: id.clone(),
                        mode: SplitMode::Exclusive,
                        routing_socket: None,
                        flows,
                        join: join_id,
                        produces_placeholder: None,
                    })
                } else if join_split_pairs.contains_key(&idx) {
                    let split_idx = join_split_pairs[&idx];
                    let split_id = ir[split_idx].id().to_string();
                    let next_idx = ir.neighbors_directed(idx, petgraph::Direction::Outgoing)
                        .next()
                        .ok_or_else(|| anyhow!("Join node '{}' has no outgoing edges", id))?;
                    ExecutionNode::Join(JoinExecNode {
                        id: id.clone(),
                        mode: JoinMode::Exclusive,
                        split: split_id,
                        next: ir[next_idx].id().to_string(),
                    })
                } else {
                    return Err(anyhow!("Unpaired/Unbalanced XOR Gateway '{}'", id));
                }
            }
            IRNode::GatewayAnd { id, direction, .. } => {
                match direction {
                    GatewayDirection::Diverging => {
                        let join_idx = split_join_pairs.get(&idx)
                            .ok_or_else(|| anyhow!("Diverging Parallel Gateway '{}' has no paired Join", id))?;
                        let join_id = ir[*join_idx].id().to_string();
                        let flows = build_split_flows(&ir, idx)?;
                        ExecutionNode::Split(SplitExecNode {
                            id: id.clone(),
                            mode: SplitMode::Parallel,
                            routing_socket: None,
                            flows,
                            join: join_id,
                            produces_placeholder: None,
                        })
                    }
                    GatewayDirection::Converging => {
                        let split_idx = join_split_pairs.get(&idx)
                            .ok_or_else(|| anyhow!("Converging Parallel Gateway '{}' has no paired Split", id))?;
                        let split_id = ir[*split_idx].id().to_string();
                        let next_idx = ir.neighbors_directed(idx, petgraph::Direction::Outgoing)
                            .next()
                            .ok_or_else(|| anyhow!("Join node '{}' has no outgoing edges", id))?;
                        ExecutionNode::Join(JoinExecNode {
                            id: id.clone(),
                            mode: JoinMode::Parallel,
                            split: split_id,
                            next: ir[next_idx].id().to_string(),
                        })
                    }
                }
            }
            IRNode::GatewayInclusive { id, direction, .. } => {
                match direction {
                    GatewayDirection::Diverging => {
                        let join_idx = split_join_pairs.get(&idx)
                            .ok_or_else(|| anyhow!("Diverging Inclusive Gateway '{}' has no paired Join", id))?;
                        let join_id = ir[*join_idx].id().to_string();
                        let flows = build_split_flows(&ir, idx)?;
                        ExecutionNode::Split(SplitExecNode {
                            id: id.clone(),
                            mode: SplitMode::Inclusive,
                            routing_socket: None,
                            flows,
                            join: join_id,
                            produces_placeholder: None,
                        })
                    }
                    GatewayDirection::Converging => {
                        let split_idx = join_split_pairs.get(&idx)
                            .ok_or_else(|| anyhow!("Converging Inclusive Gateway '{}' has no paired Split", id))?;
                        let split_id = ir[*split_idx].id().to_string();
                        let next_idx = ir.neighbors_directed(idx, petgraph::Direction::Outgoing)
                            .next()
                            .ok_or_else(|| anyhow!("Join node '{}' has no outgoing edges", id))?;
                        ExecutionNode::Join(JoinExecNode {
                            id: id.clone(),
                            mode: JoinMode::Inclusive,
                            split: split_id,
                            next: ir[next_idx].id().to_string(),
                        })
                    }
                }
            }
            other => {
                return Err(anyhow!("BPMN node type '{:?}' not supported by SESE importer", other));
            }
        };

        nodes.insert(node_id, exec_node);
    }

    let start_node = start_node_id.ok_or_else(|| anyhow!("BPMN XML lacks Start node"))?;

    let plan = WorkflowExecutionPlan {
        workflow_id: workflow_id.to_string(),
        nodes,
        start_node,
        placeholder_schema: PlaceholderSchema { slots: HashMap::new() },
        closure_manifest: None,
        regime_version: std::env::var("BPMN_LITE_REGIME_VERSION").ok(),
    };

    // 4. Verify SESE structure (DFS stack walk)
    if let Err(err) = verify_sese_nesting(&plan) {
        return Err(anyhow!("Non-SESE topology verification failed: {}", err));
    }

    Ok(plan)
}

fn build_split_flows(ir: &IRGraph, split_idx: NodeIndex) -> Result<Vec<SplitExecFlow>> {
    use petgraph::visit::EdgeRef;
    let mut flows = Vec::new();
    for edge in ir.edges_directed(split_idx, petgraph::Direction::Outgoing) {
        let target = edge.target();
        let target_id = ir[target].id().to_string();
        let edge_weight = edge.weight();
        let (placeholder, expected_value) = if let Some(cond) = &edge_weight.condition {
            let val_str = match &cond.literal {
                bpmn_lite_compiler::ir::ConditionLiteral::Bool(b) => b.to_string(),
                bpmn_lite_compiler::ir::ConditionLiteral::I64(i) => i.to_string(),
            };
            (Some(format!("@{}", cond.flag_name)), Some(val_str))
        } else {
            (None, None)
        };
        flows.push(SplitExecFlow {
            placeholder,
            expected_value,
            next: target_id,
        });
    }
    Ok(flows)
}

fn find_corresponding_join(graph: &IRGraph, split_idx: NodeIndex) -> Option<NodeIndex> {
    let neighbors: Vec<NodeIndex> = graph.neighbors_directed(split_idx, petgraph::Direction::Outgoing).collect();
    if neighbors.is_empty() { return None; }
    let mut reachable_sets = Vec::new();
    for &nb in &neighbors {
        let mut visited = std::collections::HashSet::new();
        let mut dfs = petgraph::visit::Dfs::new(graph, nb);
        while let Some(nx) = dfs.next(graph) {
            visited.insert(nx);
        }
        reachable_sets.push(visited);
    }
    let mut intersection = reachable_sets[0].clone();
    for set in reachable_sets.iter().skip(1) {
        intersection = intersection.iter().copied().filter(|x| set.contains(x)).collect();
    }
    let mut queue = std::collections::VecDeque::new();
    let mut visited = std::collections::HashSet::new();
    queue.push_back(split_idx);
    visited.insert(split_idx);
    while let Some(curr) = queue.pop_front() {
        if curr != split_idx && intersection.contains(&curr) {
            let node = &graph[curr];
            if matches!(node, IRNode::GatewayXor { .. } | IRNode::GatewayAnd { direction: GatewayDirection::Converging, .. } | IRNode::GatewayInclusive { direction: GatewayDirection::Converging, .. }) {
                return Some(curr);
            }
        }
        for next in graph.neighbors_directed(curr, petgraph::Direction::Outgoing) {
            if visited.insert(next) {
                queue.push_back(next);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SESE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                      xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
      <bpmn:process id="process_sese" isExecutable="true">
        <bpmn:startEvent id="start" />
        <bpmn:exclusiveGateway id="split" name="Split" />
        <bpmn:serviceTask id="task1" name="Task 1">
          <bpmn:extensionElements>
            <zeebe:taskDefinition type="do_task1" />
          </bpmn:extensionElements>
        </bpmn:serviceTask>
        <bpmn:serviceTask id="task2" name="Task 2">
          <bpmn:extensionElements>
            <zeebe:taskDefinition type="do_task2" />
          </bpmn:extensionElements>
        </bpmn:serviceTask>
        <bpmn:exclusiveGateway id="join" name="Join" />
        <bpmn:endEvent id="end" />

        <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split" />
        <bpmn:sequenceFlow id="f2" sourceRef="split" targetRef="task1" />
        <bpmn:sequenceFlow id="f3" sourceRef="split" targetRef="task2" />
        <bpmn:sequenceFlow id="f4" sourceRef="task1" targetRef="join" />
        <bpmn:sequenceFlow id="f5" sourceRef="task2" targetRef="join" />
        <bpmn:sequenceFlow id="f6" sourceRef="join" targetRef="end" />
      </bpmn:process>
    </bpmn:definitions>"#;

    const INVALID_CROSSING_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                      xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
      <bpmn:process id="process_non_sese" isExecutable="true">
        <bpmn:startEvent id="start" />
        <bpmn:exclusiveGateway id="split1" name="Split 1" />
        <bpmn:exclusiveGateway id="split2" name="Split 2" />
        <bpmn:serviceTask id="task1" name="Task 1">
          <bpmn:extensionElements>
            <zeebe:taskDefinition type="do_task1" />
          </bpmn:extensionElements>
        </bpmn:serviceTask>
        <bpmn:serviceTask id="task2" name="Task 2">
          <bpmn:extensionElements>
            <zeebe:taskDefinition type="do_task2" />
          </bpmn:extensionElements>
        </bpmn:serviceTask>
        <bpmn:exclusiveGateway id="join1" name="Join 1" />
        <bpmn:exclusiveGateway id="join2" name="Join 2" />
        <bpmn:endEvent id="end" />

        <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="split1" />
        <bpmn:sequenceFlow id="f2" sourceRef="split1" targetRef="split2" />
        <bpmn:sequenceFlow id="f3" sourceRef="split1" targetRef="task1" />
        <bpmn:sequenceFlow id="f4" sourceRef="split2" targetRef="task2" />
        <bpmn:sequenceFlow id="f5" sourceRef="split2" targetRef="join1" />
        <bpmn:sequenceFlow id="f6" sourceRef="task1" targetRef="join1" />
        <bpmn:sequenceFlow id="f7" sourceRef="task2" targetRef="join2" />
        <bpmn:sequenceFlow id="f8" sourceRef="join1" targetRef="join2" />
        <bpmn:sequenceFlow id="f9" sourceRef="join2" targetRef="end" />
      </bpmn:process>
    </bpmn:definitions>"#;

    #[test]
    fn test_zeebe_import_rejections() {
        // 1. Green fixture (valid SESE) compiles successfully to an execution plan
        let res_green = import_zeebe_bpmn(VALID_SESE_XML, "green_wf");
        assert!(res_green.is_ok(), "Expected valid SESE to compile, got {:?}", res_green);
        let plan = res_green.unwrap();
        assert_eq!(plan.workflow_id, "green_wf");
        assert_eq!(plan.start_node, "start");
        assert!(plan.nodes.contains_key("split"));
        assert!(plan.nodes.contains_key("join"));

        // 2. Red fixture (invalid non-SESE with crossing boundaries) is rejected with error
        let res_red = import_zeebe_bpmn(INVALID_CROSSING_XML, "red_wf");
        assert!(res_red.is_err(), "Expected invalid crossing gateways to be rejected");
        let err_msg = res_red.unwrap_err().to_string();
        assert!(err_msg.contains("verification failed") || err_msg.contains("Crossing split-join boundaries"), "Got message: {}", err_msg);
    }

    #[test]
    fn test_zeebe_import_manifest_files() {
        let mut dir = std::path::PathBuf::from("manifests");
        if !dir.exists() {
            dir = std::path::PathBuf::from("../manifests");
        }
        let valid_path = dir.join("zeebe-valid-sese.xml");
        let invalid_path = dir.join("zeebe-crossing-boundaries.xml");

        // Validate that both files exist
        assert!(valid_path.exists(), "valid XML file missing");
        assert!(invalid_path.exists(), "invalid XML file missing");

        let xml_valid = std::fs::read_to_string(&valid_path).expect("read valid XML");
        let xml_invalid = std::fs::read_to_string(&invalid_path).expect("read invalid XML");

        // 1. Valid SESE file passes SESE verification
        let res_valid = import_zeebe_bpmn(&xml_valid, "zeebe_valid");
        assert!(res_valid.is_ok(), "Expected valid Zeebe SESE file to compile, got {:?}", res_valid);

        // 2. Invalid crossing boundaries file fails with SESE error
        let res_invalid = import_zeebe_bpmn(&xml_invalid, "zeebe_invalid");
        assert!(res_invalid.is_err(), "Expected non-SESE Zeebe file to fail import");
        let err = res_invalid.unwrap_err().to_string();
        assert!(
            err.contains("Crossing split-join boundaries"),
            "Expected crossing boundaries error, got: {}", err
        );
    }
}
