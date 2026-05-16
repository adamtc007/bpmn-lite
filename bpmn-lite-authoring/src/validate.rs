use super::dto::*;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub rule: String,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.rule, self.message)
    }
}

/// Validate a WorkflowGraphDto before IR conversion. Returns all errors found.
pub fn validate_dto(dto: &WorkflowGraphDto) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Build lookup maps
    let mut node_map: HashMap<&str, &NodeDto> = HashMap::new();
    let mut outgoing: HashMap<&str, Vec<&EdgeDto>> = HashMap::new();

    // V1: Node IDs must be unique
    for node in &dto.nodes {
        let id = node.id();
        if node_map.contains_key(id) {
            errors.push(ValidationError {
                rule: "V1".to_string(),
                message: format!("Duplicate node id: {}", id),
            });
        } else {
            node_map.insert(id, node);
        }
    }

    // V2: Node IDs must not contain '.'
    for node in &dto.nodes {
        if node.id().contains('.') {
            errors.push(ValidationError {
                rule: "V2".to_string(),
                message: format!("Node id contains '.': {}", node.id()),
            });
        }
    }

    // Build outgoing edge map
    for edge in &dto.edges {
        outgoing.entry(edge.from.as_str()).or_default().push(edge);
    }

    // V3 + V4: Edge from/to reference valid nodes or valid dot-notation
    for edge in &dto.edges {
        validate_edge_ref(&edge.from, &node_map, &mut errors, "from");
        validate_edge_ref(&edge.to, &node_map, &mut errors, "to");
    }

    // V5: Exactly one Start node
    let start_count = dto
        .nodes
        .iter()
        .filter(|n| matches!(n, NodeDto::Start { .. }))
        .count();
    if start_count != 1 {
        errors.push(ValidationError {
            rule: "V5".to_string(),
            message: format!("Expected exactly one Start node, found {}", start_count),
        });
    }

    // V6: At least one End node
    let end_count = dto
        .nodes
        .iter()
        .filter(|n| matches!(n, NodeDto::End { .. }))
        .count();
    if end_count == 0 {
        errors.push(ValidationError {
            rule: "V6".to_string(),
            message: "No End node found".to_string(),
        });
    }

    // V7: ExclusiveGateway — exactly one default outgoing edge
    for node in &dto.nodes {
        if let NodeDto::ExclusiveGateway { id } = node {
            let out_edges: Vec<&&EdgeDto> = outgoing
                .get(id.as_str())
                .map(|v| v.iter().collect())
                .unwrap_or_default();

            if out_edges.len() > 1 {
                // Count edges that qualify as "default" (is_default or no condition and no on_error)
                let default_count = out_edges
                    .iter()
                    .filter(|e| e.is_default || (e.condition.is_none() && e.on_error.is_none()))
                    .count();

                if default_count != 1 {
                    errors.push(ValidationError {
                        rule: "V7".to_string(),
                        message: format!(
                            "ExclusiveGateway {}: must have exactly one default outgoing edge, found {}",
                            id, default_count
                        ),
                    });
                }
            }
        }
    }

    // V8: Diverging ParallelGateway.join references valid Converging ParallelGateway
    for node in &dto.nodes {
        if let NodeDto::ParallelGateway {
            id,
            direction: bpmn_lite_compiler::ir::GatewayDirection::Diverging,
            join: Some(join_id),
        } = node
        {
            let valid = node_map.get(join_id.as_str()).is_some_and(|n| {
                matches!(
                    n,
                    NodeDto::ParallelGateway {
                        direction: bpmn_lite_compiler::ir::GatewayDirection::Converging,
                        ..
                    }
                )
            });
            if !valid {
                errors.push(ValidationError {
                    rule: "V8".to_string(),
                    message: format!(
                        "ParallelGateway {}: join '{}' not found or not Converging",
                        id, join_id
                    ),
                });
            }
        }
    }

    // V9: Diverging InclusiveGateway.join references valid Converging InclusiveGateway
    for node in &dto.nodes {
        if let NodeDto::InclusiveGateway {
            id,
            direction: bpmn_lite_compiler::ir::GatewayDirection::Diverging,
            join: Some(join_id),
        } = node
        {
            let valid = node_map.get(join_id.as_str()).is_some_and(|n| {
                matches!(
                    n,
                    NodeDto::InclusiveGateway {
                        direction: bpmn_lite_compiler::ir::GatewayDirection::Converging,
                        ..
                    }
                )
            });
            if !valid {
                errors.push(ValidationError {
                    rule: "V9".to_string(),
                    message: format!(
                        "InclusiveGateway {}: join '{}' not found or not Converging",
                        id, join_id
                    ),
                });
            }
        }
    }

    // V10: At most one InclusiveGateway pair
    let inclusive_diverging = dto
        .nodes
        .iter()
        .filter(|n| {
            matches!(
                n,
                NodeDto::InclusiveGateway {
                    direction: bpmn_lite_compiler::ir::GatewayDirection::Diverging,
                    ..
                }
            )
        })
        .count();
    let inclusive_converging = dto
        .nodes
        .iter()
        .filter(|n| {
            matches!(
                n,
                NodeDto::InclusiveGateway {
                    direction: bpmn_lite_compiler::ir::GatewayDirection::Converging,
                    ..
                }
            )
        })
        .count();
    if inclusive_diverging > 1 || inclusive_converging > 1 {
        errors.push(ValidationError {
            rule: "V10".to_string(),
            message: "Multiple inclusive gateway pairs not supported (v1)".to_string(),
        });
    }

    // V11: BoundaryTimer.host references a ServiceTask
    for node in &dto.nodes {
        if let NodeDto::BoundaryTimer { id, host, .. } = node {
            let valid = node_map
                .get(host.as_str())
                .is_some_and(|n| matches!(n, NodeDto::ServiceTask { .. }));
            if !valid {
                errors.push(ValidationError {
                    rule: "V11".to_string(),
                    message: format!("BoundaryTimer {}: host '{}' is not a ServiceTask", id, host),
                });
            }
        }
    }

    // V12: BoundaryTimer has exactly one outgoing edge
    for node in &dto.nodes {
        if let NodeDto::BoundaryTimer { id, .. } = node {
            let out_count = outgoing.get(id.as_str()).map(|v| v.len()).unwrap_or(0);
            if out_count != 1 {
                errors.push(ValidationError {
                    rule: "V12".to_string(),
                    message: format!(
                        "BoundaryTimer {}: must have exactly one outgoing edge, found {}",
                        id, out_count
                    ),
                });
            }
        }
    }

    // V13: No edge has both condition and on_error
    for edge in &dto.edges {
        if edge.condition.is_some() && edge.on_error.is_some() {
            errors.push(ValidationError {
                rule: "V13".to_string(),
                message: format!(
                    "Edge {}→{}: condition and on_error are mutually exclusive",
                    edge.from, edge.to
                ),
            });
        }
    }

    // V14: No edge has both condition and is_default
    for edge in &dto.edges {
        if edge.condition.is_some() && edge.is_default {
            errors.push(ValidationError {
                rule: "V14".to_string(),
                message: format!(
                    "Edge {}→{}: condition and is_default are mutually exclusive",
                    edge.from, edge.to
                ),
            });
        }
    }

    // V15: Error edge from references a ServiceTask
    for edge in &dto.edges {
        if edge.on_error.is_some() {
            // Resolve base node (strip dot-notation)
            let base_id = if edge.from.contains('.') {
                edge.from.split('.').next().unwrap_or(&edge.from)
            } else {
                &edge.from
            };
            let valid = node_map
                .get(base_id)
                .is_some_and(|n| matches!(n, NodeDto::ServiceTask { .. }));
            if !valid {
                errors.push(ValidationError {
                    rule: "V15".to_string(),
                    message: format!(
                        "Error edge from '{}': must reference a ServiceTask",
                        edge.from
                    ),
                });
            }
        }
    }

    errors
}

/// Validate an edge `from` or `to` reference. Handles dot-notation for RaceWait arms.
fn validate_edge_ref(
    reference: &str,
    node_map: &HashMap<&str, &NodeDto>,
    errors: &mut Vec<ValidationError>,
    field_name: &str,
) {
    if reference.contains('.') {
        // Dot-notation: split on first '.'
        let (race_id, arm_id) = reference.split_once('.').unwrap();

        // V3: race_id must reference a node
        if !node_map.contains_key(race_id) {
            errors.push(ValidationError {
                rule: "V3".to_string(),
                message: format!(
                    "Edge references unknown node: {} ({})",
                    reference, field_name
                ),
            });
            return;
        }

        // V4: race_id must be a RaceWait and arm_id must match one of its arms
        match node_map.get(race_id) {
            Some(NodeDto::RaceWait { arms, .. }) => {
                let arm_exists = arms.iter().any(|a| a.arm_id == arm_id);
                if !arm_exists {
                    errors.push(ValidationError {
                        rule: "V4".to_string(),
                        message: format!("Invalid race arm reference: {}", reference),
                    });
                }
            }
            _ => {
                errors.push(ValidationError {
                    rule: "V4".to_string(),
                    message: format!(
                        "Invalid race arm reference: {} (node is not a RaceWait)",
                        reference
                    ),
                });
            }
        }
    } else {
        // Simple node reference
        if !node_map.contains_key(reference) {
            errors.push(ValidationError {
                rule: "V3".to_string(),
                message: format!(
                    "Edge references unknown node: {} ({})",
                    reference, field_name
                ),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_valid_dto() -> WorkflowGraphDto {
        WorkflowGraphDto {
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
        }
    }

    #[test]
    fn test_minimal_valid_passes() {
        let dto = minimal_valid_dto();
        let errors = validate_dto(&dto);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    /// T-AUTH-7: V1 — Duplicate node id
    #[test]
    fn t_auth_7_v1_duplicate_id() {
        let mut dto = minimal_valid_dto();
        dto.nodes.push(NodeDto::ServiceTask {
            id: "task_a".to_string(),
            task_type: "other".to_string(),
            bpmn_id: None,
        });
        let errors = validate_dto(&dto);
        assert!(errors.iter().any(|e| e.rule == "V1"), "Expected V1 error");
    }

    /// T-AUTH-7: V2 — Node id contains '.'
    #[test]
    fn t_auth_7_v2_dot_in_id() {
        let mut dto = minimal_valid_dto();
        dto.nodes[1] = NodeDto::ServiceTask {
            id: "task.a".to_string(),
            task_type: "do_work".to_string(),
            bpmn_id: None,
        };
        // Also update edges to match
        dto.edges[0].to = "task.a".to_string();
        dto.edges[1].from = "task.a".to_string();
        let errors = validate_dto(&dto);
        assert!(errors.iter().any(|e| e.rule == "V2"), "Expected V2 error");
    }

    /// T-AUTH-7: V4 — Invalid dot-notation reference
    #[test]
    fn t_auth_7_v4_invalid_race_ref() {
        let mut dto = minimal_valid_dto();
        // Add an edge with dot-notation where the node is NOT a RaceWait
        dto.edges.push(EdgeDto {
            from: "task_a.arm1".to_string(),
            to: "end".to_string(),
            condition: None,
            is_default: false,
            on_error: None,
        });
        let errors = validate_dto(&dto);
        assert!(errors.iter().any(|e| e.rule == "V4"), "Expected V4 error");
    }

    /// T-AUTH-7: V7 — ExclusiveGateway missing default
    #[test]
    fn t_auth_7_v7_missing_xor_default() {
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ExclusiveGateway {
                    id: "xor".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "a".to_string(),
                    task_type: "do_a".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ServiceTask {
                    id: "b".to_string(),
                    task_type: "do_b".to_string(),
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
                    to: "xor".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                // Both edges have conditions — NO default
                EdgeDto {
                    from: "xor".to_string(),
                    to: "a".to_string(),
                    condition: Some(FlagCondition {
                        flag: "x".to_string(),
                        op: FlagOp::Eq,
                        value: FlagValue::Bool(true),
                    }),
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".to_string(),
                    to: "b".to_string(),
                    condition: Some(FlagCondition {
                        flag: "y".to_string(),
                        op: FlagOp::Eq,
                        value: FlagValue::Bool(true),
                    }),
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "b".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };
        let errors = validate_dto(&dto);
        assert!(errors.iter().any(|e| e.rule == "V7"), "Expected V7 error");
    }

    /// T-AUTH-7: V11 — BoundaryTimer host is not a ServiceTask
    #[test]
    fn t_auth_7_v11_boundary_wrong_host() {
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ExclusiveGateway {
                    id: "gw".to_string(),
                },
                NodeDto::BoundaryTimer {
                    id: "bt".to_string(),
                    host: "gw".to_string(), // gw is NOT a ServiceTask
                    duration_ms: Some(5000),
                    deadline_ms: None,
                    cycle_ms: None,
                    cycle_max: None,
                    interrupting: true,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "gw".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "bt".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };
        let errors = validate_dto(&dto);
        assert!(
            errors.iter().any(|e| e.rule == "V11"),
            "Expected V11 error, got: {:?}",
            errors
        );
    }

    /// T-AUTH-7: V13 — Edge with both condition and on_error
    #[test]
    fn t_auth_7_v13_condition_and_error() {
        let mut dto = minimal_valid_dto();
        dto.edges[1].condition = Some(FlagCondition {
            flag: "x".to_string(),
            op: FlagOp::Eq,
            value: FlagValue::Bool(true),
        });
        dto.edges[1].on_error = Some(ErrorEdge {
            error_code: "BIZ_001".to_string(),
            retries: 0,
        });
        let errors = validate_dto(&dto);
        assert!(errors.iter().any(|e| e.rule == "V13"), "Expected V13 error");
    }
}
