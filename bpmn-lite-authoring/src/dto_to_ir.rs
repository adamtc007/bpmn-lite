use super::dto::*;
use super::validate::validate_dto;
use anyhow::{anyhow, Result};
use bpmn_lite_compiler::ir::*;
use std::collections::HashMap;

/// Convert a validated WorkflowGraphDto to an IRGraph suitable for the existing
/// verifier + lowering pipeline.
///
/// RaceWait is not supported in Phase A — returns an error if encountered.
/// Error edge retries are deferred; on_error edges create BoundaryError IR nodes
/// which provide routing without retry counters.
pub fn dto_to_ir(dto: &WorkflowGraphDto) -> Result<IRGraph> {
    // 1. Validate
    let errors = validate_dto(dto);
    if !errors.is_empty() {
        let msgs: Vec<String> = errors
            .iter()
            .map(|e| format!("[{}] {}", e.rule, e.message))
            .collect();
        return Err(anyhow!("DTO validation failed:\n{}", msgs.join("\n")));
    }

    let mut graph = IRGraph::new();
    let mut node_index_map: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

    // 2. Create IR nodes
    for node in &dto.nodes {
        let ir_node = match node {
            NodeDto::Start { id } => IRNode::Start { id: id.clone() },

            NodeDto::End { id, terminate } => IRNode::End {
                id: id.clone(),
                terminate: *terminate,
            },

            NodeDto::ServiceTask {
                id,
                task_type,
                bpmn_id,
            } => IRNode::ServiceTask {
                id: id.clone(),
                name: bpmn_id.as_ref().cloned().unwrap_or_else(|| id.clone()),
                task_type: task_type.clone(),
            },

            NodeDto::ExclusiveGateway { id } => IRNode::GatewayXor {
                id: id.clone(),
                name: String::new(),
            },

            NodeDto::ParallelGateway { id, direction, .. } => IRNode::GatewayAnd {
                id: id.clone(),
                name: String::new(),
                direction: direction.clone(),
            },

            NodeDto::InclusiveGateway { id, direction, .. } => IRNode::GatewayInclusive {
                id: id.clone(),
                name: String::new(),
                direction: direction.clone(),
            },

            NodeDto::TimerWait {
                id,
                duration_ms,
                deadline_ms,
                cycle_ms,
                cycle_max,
            } => {
                let spec =
                    timer_spec_from_fields(*duration_ms, *deadline_ms, *cycle_ms, *cycle_max)?;
                IRNode::TimerWait {
                    id: id.clone(),
                    spec,
                }
            }

            NodeDto::MessageWait {
                id,
                name,
                corr_key_source,
            } => IRNode::MessageWait {
                id: id.clone(),
                name: name.clone(),
                corr_key_source: corr_key_source.clone(),
            },

            NodeDto::HumanWait {
                id,
                task_kind,
                corr_key_source,
            } => IRNode::HumanWait {
                id: id.clone(),
                name: format!("human.{}", task_kind),
                task_kind: task_kind.clone(),
                corr_key_source: corr_key_source.clone(),
            },

            NodeDto::RaceWait { id, .. } => {
                return Err(anyhow!(
                    "RaceWait '{}' not supported in Phase A. Use BoundaryTimer instead.",
                    id
                ));
            }

            NodeDto::BoundaryTimer {
                id,
                host,
                duration_ms,
                deadline_ms,
                cycle_ms,
                cycle_max,
                interrupting,
            } => {
                let spec =
                    timer_spec_from_fields(*duration_ms, *deadline_ms, *cycle_ms, *cycle_max)?;
                IRNode::BoundaryTimer {
                    id: id.clone(),
                    attached_to: host.clone(),
                    spec,
                    interrupting: *interrupting,
                }
            }

            NodeDto::BoundaryError {
                id,
                host,
                error_code,
            } => IRNode::BoundaryError {
                id: id.clone(),
                attached_to: host.clone(),
                error_code: error_code.clone(),
            },
        };

        let idx = graph.add_node(ir_node);
        node_index_map.insert(node.id().to_string(), idx);
    }

    // 3. Synthesize BoundaryError nodes from on_error edges
    // For each on_error edge, create a synthetic BoundaryError IR node and wire it.
    let mut synthetic_error_nodes: Vec<(String, String, String, Option<String>)> = Vec::new(); // (synth_id, from_id, to_id, error_code)
    for edge in &dto.edges {
        if let Some(on_error) = &edge.on_error {
            let synth_id = format!("err_{}_{}", edge.from, on_error.error_code);
            synthetic_error_nodes.push((
                synth_id.clone(),
                edge.from.clone(),
                edge.to.clone(),
                Some(on_error.error_code.clone()),
            ));
        }
    }

    for (synth_id, from_id, _to_id, error_code) in &synthetic_error_nodes {
        let ir_node = IRNode::BoundaryError {
            id: synth_id.clone(),
            attached_to: from_id.clone(),
            error_code: error_code.clone(),
        };
        let idx = graph.add_node(ir_node);
        node_index_map.insert(synth_id.clone(), idx);
    }

    // 4. Create edges
    let mut edge_counter = 0u32;
    for edge in &dto.edges {
        if edge.on_error.is_some() {
            // on_error edges are wired through synthetic BoundaryError nodes, not as direct edges
            continue;
        }

        // Resolve from/to (dot-notation not supported in Phase A since RaceWait is deferred)
        let from_idx = node_index_map
            .get(&edge.from)
            .ok_or_else(|| anyhow!("Edge from '{}': node not found", edge.from))?;
        let to_idx = node_index_map
            .get(&edge.to)
            .ok_or_else(|| anyhow!("Edge to '{}': node not found", edge.to))?;

        let condition = if edge.is_default {
            // Default edges have no condition in IR (the lowering treats condition=None as default)
            None
        } else {
            edge.condition.as_ref().map(flag_to_condition)
        };

        let ir_edge = IREdge {
            id: format!("e{}", edge_counter),
            condition,
        };
        edge_counter += 1;

        graph.add_edge(*from_idx, *to_idx, ir_edge);
    }

    // 5. Wire synthetic BoundaryError nodes: synth_node → to_target
    for (synth_id, _from_id, to_id, _error_code) in &synthetic_error_nodes {
        let synth_idx = node_index_map[synth_id.as_str()];
        let to_idx = node_index_map
            .get(to_id.as_str())
            .ok_or_else(|| anyhow!("Error edge to '{}': node not found", to_id))?;

        let ir_edge = IREdge {
            id: format!("e{}", edge_counter),
            condition: None,
        };
        edge_counter += 1;

        graph.add_edge(synth_idx, *to_idx, ir_edge);
    }

    Ok(graph)
}

/// Convert DTO FlagCondition → IR ConditionExpr.
fn flag_to_condition(fc: &FlagCondition) -> ConditionExpr {
    let op = match fc.op {
        FlagOp::Eq => ConditionOp::Eq,
        FlagOp::Neq => ConditionOp::Neq,
        FlagOp::Lt => ConditionOp::Lt,
        FlagOp::Gt => ConditionOp::Gt,
    };
    let literal = match &fc.value {
        FlagValue::Bool(b) => ConditionLiteral::Bool(*b),
        FlagValue::I64(i) => ConditionLiteral::I64(*i),
    };
    ConditionExpr {
        flag_name: fc.flag.clone(),
        op,
        literal,
    }
}

/// Construct a TimerSpec from optional fields. Exactly one must be set.
fn timer_spec_from_fields(
    duration_ms: Option<u64>,
    deadline_ms: Option<u64>,
    cycle_ms: Option<u64>,
    cycle_max: Option<u32>,
) -> Result<TimerSpec> {
    match (duration_ms, deadline_ms, cycle_ms) {
        (Some(ms), None, None) => Ok(TimerSpec::Duration { ms }),
        (None, Some(deadline), None) => Ok(TimerSpec::Date {
            deadline_ms: deadline,
        }),
        (None, None, Some(interval)) => Ok(TimerSpec::Cycle {
            interval_ms: interval,
            max_fires: cycle_max.unwrap_or(1),
        }),
        (None, None, None) => Err(anyhow!(
            "Timer must specify duration_ms, deadline_ms, or cycle_ms"
        )),
        _ => Err(anyhow!(
            "Timer must specify exactly one of duration_ms, deadline_ms, or cycle_ms"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Basic linear DTO → IR → verify succeeds
    #[test]
    fn test_linear_dto_to_ir() {
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
        assert_eq!(ir.node_count(), 3);
        assert_eq!(ir.edge_count(), 2);

        // Verify passes
        let errors = bpmn_lite_compiler::verifier::verify(&ir);
        assert!(errors.is_empty(), "Verifier errors: {:?}", errors);
    }

    /// RaceWait returns error in Phase A
    #[test]
    fn test_race_wait_rejected() {
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::RaceWait {
                    id: "race".to_string(),
                    arms: vec![],
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "race".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "race".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        let result = dto_to_ir(&dto);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("RaceWait"));
    }

    /// Error edge synthesizes BoundaryError node
    #[test]
    fn test_error_edge_synthesis() {
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
                NodeDto::ServiceTask {
                    id: "escalation".to_string(),
                    task_type: "escalate".to_string(),
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
                // Error edge: task_a → escalation on BIZ_001
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "escalation".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: Some(ErrorEdge {
                        error_code: "BIZ_001".to_string(),
                        retries: 0,
                    }),
                },
                EdgeDto {
                    from: "escalation".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        let ir = dto_to_ir(&dto).unwrap();
        // Original 4 nodes + 1 synthetic BoundaryError = 5
        assert_eq!(ir.node_count(), 5);

        // Verify the synthetic node exists
        let has_boundary_error = ir.node_indices().any(|idx| {
            matches!(
                &ir[idx],
                IRNode::BoundaryError {
                    id,
                    attached_to,
                    error_code: Some(code),
                } if id == "err_task_a_BIZ_001" && attached_to == "task_a" && code == "BIZ_001"
            )
        });
        assert!(has_boundary_error, "Expected synthetic BoundaryError node");

        // Verifier should pass
        let errors = bpmn_lite_compiler::verifier::verify(&ir);
        assert!(errors.is_empty(), "Verifier errors: {:?}", errors);
    }
}
