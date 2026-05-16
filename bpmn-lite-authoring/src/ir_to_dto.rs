use super::dto::*;
use anyhow::{anyhow, Result};
use bpmn_lite_compiler::ir::*;
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use std::collections::HashMap;

/// Convert an IRGraph back to a WorkflowGraphDto.
///
/// This is the reverse of `dto_to_ir`. It enables IR → DTO round-trips
/// (e.g., after BPMN import → IR → DTO for editing, or for round-trip
/// verification in tests).
///
/// ## Limitations
/// - RaceWait cannot be reconstructed (IRGraph has no eventBasedGateway node).
/// - BoundaryError edges from `dto_to_ir` synthesized nodes are kept as direct
///   edges (not collapsed back to `on_error` EdgeDto fields) for simplicity.
/// - Gateway join pairing: parallel/inclusive converging gateways get `join: None`
///   (pairing is a higher-level concern handled by the verifier).
pub fn ir_to_dto(graph: &IRGraph, workflow_id: &str) -> Result<WorkflowGraphDto> {
    let mut nodes = Vec::new();
    let mut node_id_map: HashMap<petgraph::graph::NodeIndex, String> = HashMap::new();

    // 1. Map IR nodes → DTO nodes
    for idx in graph.node_indices() {
        let ir_node = &graph[idx];
        let dto_node = ir_node_to_dto(ir_node)?;
        let node_id = dto_node.id().to_string();
        node_id_map.insert(idx, node_id);
        nodes.push(dto_node);
    }

    // 2. Collect XOR gateway outgoing edges to detect defaults.
    //    In IR, XOR default = outgoing edge with condition=None from a GatewayXor node.
    //    But ONLY if the XOR has other conditional outgoing edges (otherwise it's just
    //    a merge gateway with a single outgoing edge, not a "default").
    let mut xor_ids: std::collections::HashSet<petgraph::graph::NodeIndex> =
        std::collections::HashSet::new();
    for idx in graph.node_indices() {
        if matches!(&graph[idx], IRNode::GatewayXor { .. }) {
            xor_ids.insert(idx);
        }
    }

    // For each XOR: check if it has at least one conditional outgoing edge
    let mut xor_has_conditions: std::collections::HashSet<petgraph::graph::NodeIndex> =
        std::collections::HashSet::new();
    for &xor_idx in &xor_ids {
        let has_cond = graph
            .edges_directed(xor_idx, Direction::Outgoing)
            .any(|e| e.weight().condition.is_some());
        if has_cond {
            xor_has_conditions.insert(xor_idx);
        }
    }

    // 3. Map IR edges → DTO edges
    let mut edges = Vec::new();
    for edge_ref in graph.edge_references() {
        let from_idx = edge_ref.source();
        let to_idx = edge_ref.target();
        let ir_edge = edge_ref.weight();

        let from_id = node_id_map
            .get(&from_idx)
            .ok_or_else(|| anyhow!("Missing node for edge source"))?
            .clone();
        let to_id = node_id_map
            .get(&to_idx)
            .ok_or_else(|| anyhow!("Missing node for edge target"))?
            .clone();

        let condition = ir_edge.condition.as_ref().map(condition_to_flag);

        // Detect default edge: unconditional outgoing edge from an XOR that has
        // other conditional edges.
        let is_default = ir_edge.condition.is_none() && xor_has_conditions.contains(&from_idx);

        edges.push(EdgeDto {
            from: from_id,
            to: to_id,
            condition,
            is_default,
            on_error: None,
        });
    }

    Ok(WorkflowGraphDto {
        id: workflow_id.to_string(),
        meta: None,
        nodes,
        edges,
    })
}

/// Convert an IR ConditionExpr back to a DTO FlagCondition.
fn condition_to_flag(cond: &ConditionExpr) -> FlagCondition {
    let op = match cond.op {
        ConditionOp::Eq => FlagOp::Eq,
        ConditionOp::Neq => FlagOp::Neq,
        ConditionOp::Lt => FlagOp::Lt,
        ConditionOp::Gt => FlagOp::Gt,
    };
    let value = match &cond.literal {
        ConditionLiteral::Bool(b) => FlagValue::Bool(*b),
        ConditionLiteral::I64(i) => FlagValue::I64(*i),
    };
    FlagCondition {
        flag: cond.flag_name.clone(),
        op,
        value,
    }
}

/// Convert a single IRNode to a NodeDto.
fn ir_node_to_dto(ir_node: &IRNode) -> Result<NodeDto> {
    let dto = match ir_node {
        IRNode::Start { id } => NodeDto::Start { id: id.clone() },

        IRNode::End { id, terminate } => NodeDto::End {
            id: id.clone(),
            terminate: *terminate,
        },

        IRNode::ServiceTask {
            id,
            name,
            task_type,
        } => {
            // Preserve bpmn_id if name differs from id (was explicitly set)
            let bpmn_id = if name != id { Some(name.clone()) } else { None };
            NodeDto::ServiceTask {
                id: id.clone(),
                task_type: task_type.clone(),
                bpmn_id,
            }
        }

        IRNode::GatewayXor { id, .. } => NodeDto::ExclusiveGateway { id: id.clone() },

        IRNode::GatewayAnd { id, direction, .. } => NodeDto::ParallelGateway {
            id: id.clone(),
            direction: direction.clone(),
            join: None,
        },

        IRNode::GatewayInclusive { id, direction, .. } => NodeDto::InclusiveGateway {
            id: id.clone(),
            direction: direction.clone(),
            join: None,
        },

        IRNode::TimerWait { id, spec } => {
            let (duration_ms, deadline_ms, cycle_ms, cycle_max) = timer_spec_to_fields(spec);
            NodeDto::TimerWait {
                id: id.clone(),
                duration_ms,
                deadline_ms,
                cycle_ms,
                cycle_max,
            }
        }

        IRNode::MessageWait {
            id,
            name,
            corr_key_source,
        } => NodeDto::MessageWait {
            id: id.clone(),
            name: name.clone(),
            corr_key_source: corr_key_source.clone(),
        },

        IRNode::HumanWait {
            id,
            task_kind,
            corr_key_source,
            ..
        } => NodeDto::HumanWait {
            id: id.clone(),
            task_kind: task_kind.clone(),
            corr_key_source: corr_key_source.clone(),
        },

        IRNode::BoundaryTimer {
            id,
            attached_to,
            spec,
            interrupting,
        } => {
            let (duration_ms, deadline_ms, cycle_ms, cycle_max) = timer_spec_to_fields(spec);
            NodeDto::BoundaryTimer {
                id: id.clone(),
                host: attached_to.clone(),
                duration_ms,
                deadline_ms,
                cycle_ms,
                cycle_max,
                interrupting: *interrupting,
            }
        }

        IRNode::BoundaryError {
            id,
            attached_to,
            error_code,
        } => NodeDto::BoundaryError {
            id: id.clone(),
            host: attached_to.clone(),
            error_code: error_code.clone(),
        },

        // DataObject and FfiServiceTask are not represented in the DTO/YAML
        // authoring layer (they are FFI-specific constructs). If encountered,
        // omit them by returning a minimal placeholder that the DTO round-trip
        // can skip. A future authoring-layer pass will handle these properly.
        IRNode::DataObject { id, .. } | IRNode::FfiServiceTask { id, .. } => {
            return Err(anyhow::anyhow!(
                "ir_to_dto: unsupported node type at id '{}' (DataObject / FfiServiceTask \
                 are not part of the YAML authoring DTO format)",
                id
            ))
        }
    };
    Ok(dto)
}

/// Convert a TimerSpec back to the DTO optional fields.
fn timer_spec_to_fields(spec: &TimerSpec) -> (Option<u64>, Option<u64>, Option<u64>, Option<u32>) {
    match spec {
        TimerSpec::Duration { ms } => (Some(*ms), None, None, None),
        TimerSpec::Date { deadline_ms } => (None, Some(*deadline_ms), None, None),
        TimerSpec::Cycle {
            interval_ms,
            max_fires,
        } => (None, None, Some(*interval_ms), Some(*max_fires)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto_to_ir::dto_to_ir;

    /// T-EXP-8: Linear IR round-trips through ir_to_dto.
    #[test]
    fn t_exp_8_linear_round_trip() {
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "task_a".to_string(),
                    task_type: "do_work".to_string(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "task_a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        let ir = dto_to_ir(&dto).unwrap();
        let dto2 = ir_to_dto(&ir, "test").unwrap();

        assert_eq!(dto2.nodes.len(), 3);
        assert_eq!(dto2.edges.len(), 2);
        assert_eq!(dto2.id, "test");

        // Check node types survive
        let has_start = dto2
            .nodes
            .iter()
            .any(|n| matches!(n, NodeDto::Start { .. }));
        let has_end = dto2.nodes.iter().any(|n| matches!(n, NodeDto::End { .. }));
        let has_task = dto2
            .nodes
            .iter()
            .any(|n| matches!(n, NodeDto::ServiceTask { task_type, .. } if task_type == "do_work"));
        assert!(has_start);
        assert!(has_end);
        assert!(has_task);
    }

    /// T-EXP-9: XOR with conditions preserves structure.
    #[test]
    fn t_exp_9_xor_conditions_preserved() {
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "check".to_string(),
                    task_type: "check_sanctions".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ExclusiveGateway {
                    id: "xor".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "clear".to_string(),
                    task_type: "proceed".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ServiceTask {
                    id: "escalate".to_string(),
                    task_type: "escalation".to_string(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "check".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "check".to_string(),
                    to: "xor".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".to_string(),
                    to: "clear".to_string(),
                    condition: Some(FlagCondition {
                        flag: "sanctions_clear".to_string(),
                        op: FlagOp::Eq,
                        value: FlagValue::Bool(true),
                    }),
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".to_string(),
                    to: "escalate".to_string(),
                    condition: None,
                    is_default: true,
                    on_error: None,
                },
                EdgeDto {
                    from: "clear".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "escalate".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        let ir = dto_to_ir(&dto).unwrap();
        let dto2 = ir_to_dto(&ir, "test").unwrap();

        // Same node count
        assert_eq!(dto2.nodes.len(), 6);

        // XOR gateway survives
        let has_xor = dto2
            .nodes
            .iter()
            .any(|n| matches!(n, NodeDto::ExclusiveGateway { id } if id == "xor"));
        assert!(has_xor, "XOR gateway should survive round-trip");

        // Condition edge should have the flag condition
        let cond_edges: Vec<_> = dto2
            .edges
            .iter()
            .filter(|e| e.condition.is_some())
            .collect();
        assert_eq!(
            cond_edges.len(),
            1,
            "Should have exactly one condition edge"
        );
        let cond = cond_edges[0].condition.as_ref().unwrap();
        assert_eq!(cond.flag, "sanctions_clear");

        // Default edge should be marked
        let default_edges: Vec<_> = dto2.edges.iter().filter(|e| e.is_default).collect();
        assert_eq!(
            default_edges.len(),
            1,
            "Should have exactly one default edge"
        );
        assert_eq!(default_edges[0].from, "xor");
    }

    /// T-EXP-10: BoundaryError nodes survive IR→DTO.
    #[test]
    fn t_exp_10_boundary_error_survives() {
        // Build a DTO with a BoundaryError node directly (not via on_error edge)
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "task_a".to_string(),
                    task_type: "do_work".to_string(),
                    bpmn_id: None,
                },
                NodeDto::BoundaryError {
                    id: "err_handler".to_string(),
                    host: "task_a".to_string(),
                    error_code: Some("BIZ_FAIL".to_string()),
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
                NodeDto::End {
                    id: "end_err".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "task_a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "err_handler".to_string(),
                    to: "end_err".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        let ir = dto_to_ir(&dto).unwrap();
        let dto2 = ir_to_dto(&ir, "test").unwrap();

        // BoundaryError should survive
        let boundary_errors: Vec<_> = dto2
            .nodes
            .iter()
            .filter(|n| matches!(n, NodeDto::BoundaryError { .. }))
            .collect();
        assert_eq!(boundary_errors.len(), 1, "BoundaryError should survive");

        if let NodeDto::BoundaryError {
            id,
            host,
            error_code,
        } = &boundary_errors[0]
        {
            assert_eq!(id, "err_handler");
            assert_eq!(host, "task_a");
            assert_eq!(error_code.as_deref(), Some("BIZ_FAIL"));
        } else {
            panic!("Expected BoundaryError");
        }
    }
}
