use crate::ir::*;
use anyhow::{anyhow, Result};
use bpmn_lite_types::{Addr, CompiledProgram, Instr};
use petgraph::graph::NodeIndex;
use petgraph::visit::Dfs;
use std::collections::HashMap;

/// Verification errors.
#[derive(Debug, Clone)]
pub struct VerifyError {
    pub message: String,
    pub element_id: Option<String>,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(id) = &self.element_id {
            write!(f, "[{}] {}", id, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

/// Verify structural invariants of the IR graph.
///
/// Returns a list of errors. Empty list means the graph is valid.
pub fn verify(graph: &IRGraph) -> Vec<VerifyError> {
    let mut errors = Vec::new();

    // 1. Exactly one StartEvent
    let starts: Vec<_> = graph
        .node_indices()
        .filter(|&idx| matches!(&graph[idx], IRNode::Start { .. }))
        .collect();

    if starts.is_empty() {
        errors.push(VerifyError {
            message: "No StartEvent found".to_string(),
            element_id: None,
        });
    } else if starts.len() > 1 {
        errors.push(VerifyError {
            message: format!("Multiple StartEvents found ({})", starts.len()),
            element_id: None,
        });
    }

    // 2. At least one EndEvent
    let ends: Vec<_> = graph
        .node_indices()
        .filter(|&idx| matches!(&graph[idx], IRNode::End { .. }))
        .collect();

    if ends.is_empty() {
        errors.push(VerifyError {
            message: "No EndEvent found".to_string(),
            element_id: None,
        });
    }

    // 3. All nodes reachable from Start (or from BoundaryTimer nodes,
    //    which are alternative entry points for escalation paths)
    if let Some(start_idx) = starts.first() {
        let mut reachable = std::collections::HashSet::new();

        // DFS from Start
        let mut dfs = Dfs::new(graph, *start_idx);
        while let Some(nx) = dfs.next(graph) {
            reachable.insert(nx);
        }

        // Also DFS from each BoundaryTimer/BoundaryError node (escalation/error paths)
        for idx in graph.node_indices() {
            let is_boundary = matches!(
                &graph[idx],
                IRNode::BoundaryTimer { .. } | IRNode::BoundaryError { .. }
            );
            if is_boundary && !reachable.contains(&idx) {
                reachable.insert(idx);
                let mut bdfs = Dfs::new(graph, idx);
                while let Some(nx) = bdfs.next(graph) {
                    reachable.insert(nx);
                }
            }
        }

        for idx in graph.node_indices() {
            // DataObject nodes are structural declarations with no sequence-flow
            // edges; they are intentionally unconnected and must not be flagged.
            if matches!(&graph[idx], IRNode::DataObject { .. }) {
                continue;
            }
            if !reachable.contains(&idx) {
                errors.push(VerifyError {
                    message: format!("Unreachable node: {}", graph[idx].id()),
                    element_id: Some(graph[idx].id().to_string()),
                });
            }
        }
    }

    // 4. Parallel gateways: check fork/join pairs
    let forks: Vec<_> = graph
        .node_indices()
        .filter(|&idx| {
            matches!(
                &graph[idx],
                IRNode::GatewayAnd {
                    direction: GatewayDirection::Diverging,
                    ..
                }
            )
        })
        .collect();

    let joins: Vec<_> = graph
        .node_indices()
        .filter(|&idx| {
            matches!(
                &graph[idx],
                IRNode::GatewayAnd {
                    direction: GatewayDirection::Converging,
                    ..
                }
            )
        })
        .collect();

    if forks.len() != joins.len() {
        errors.push(VerifyError {
            message: format!(
                "Mismatched parallel gateways: {} forks, {} joins",
                forks.len(),
                joins.len()
            ),
            element_id: None,
        });
    }

    // 4a. Parallel gateways: structural well-nestedness (SESE), not just
    // matching counts. Adam, 2026-07-22: V5's XML->V2Fork/V2Join lowering
    // pre-pass pairs each Converging GatewayAnd with a Diverging one via a
    // stack, correct only for well-nested topology — a property this
    // codebase has always called "SESE-only" (CLAUDE.md's settled
    // decision, never itself promoted into a V&S theorem) but which
    // nothing in this pipeline actually checked: `dto_to_ir` -> `verify`
    // -> `lower` never ran the DSL's `rpst::verify_sese_nesting` (that
    // lives on a separate importer path, into `WorkflowExecutionPlan`, not
    // `IRGraph`). Mirrors the DSL check's DFS-second-visit algorithm as
    // closely as `IRNode::GatewayAnd`'s schema allows: `GatewayAnd` carries
    // no explicit fork/join name reference (unlike the DSL's
    // `JoinExecNode.split`), so this catches unmatched forks/joins and
    // gross stack-order crossing structurally, but cannot independently
    // cross-check a join against its *intended* fork by name the way the
    // DSL's check does — a strictly weaker, not equivalent, guarantee.
    if let Some(start_idx) = find_start(graph) {
        let mut visited = std::collections::HashSet::new();
        let mut fork_stack: Vec<String> = Vec::new();
        check_gateway_and_nesting(graph, start_idx, &mut visited, &mut fork_stack, &mut errors);
        if !fork_stack.is_empty() {
            errors.push(VerifyError {
                message: format!("Unclosed diverging GatewayAnd node(s): [{}]", fork_stack.join(", ")),
                element_id: None,
            });
        }
    }

    // 5. All task_type references are non-empty (ServiceTask)
    for idx in graph.node_indices() {
        if let IRNode::ServiceTask { id, task_type, .. } = &graph[idx] {
            if task_type.is_empty() {
                errors.push(VerifyError {
                    message: "ServiceTask has empty task_type".to_string(),
                    element_id: Some(id.clone()),
                });
            }
        }
    }

    // 6. XOR diverging gateways should have at least one outgoing edge with a condition
    //    and exactly one default (no condition)
    for idx in graph.node_indices() {
        if matches!(&graph[idx], IRNode::GatewayXor { .. }) {
            let outgoing: Vec<_> = graph
                .edges_directed(idx, petgraph::Direction::Outgoing)
                .collect();

            if outgoing.len() > 1 {
                let with_condition = outgoing
                    .iter()
                    .filter(|e| e.weight().condition.is_some())
                    .count();
                let without_condition = outgoing.len() - with_condition;

                if without_condition != 1 {
                    errors.push(VerifyError {
                        message: format!(
                            "XOR gateway should have exactly 1 default edge, found {}",
                            without_condition
                        ),
                        element_id: Some(graph[idx].id().to_string()),
                    });
                }
            }
        }
    }

    // 7. Boundary event validation
    {
        let mut host_boundary_count: HashMap<String, Vec<String>> = HashMap::new();

        for idx in graph.node_indices() {
            if let IRNode::BoundaryTimer {
                id,
                attached_to,
                interrupting,
                spec,
            } = &graph[idx]
            {
                // 7a. attached_to must reference an existing ServiceTask or HumanWait
                let host_exists = graph.node_indices().any(|other| {
                    matches!(&graph[other],
                        IRNode::ServiceTask { id: host_id, .. } | IRNode::HumanWait { id: host_id, .. }
                        if host_id == attached_to
                    )
                });
                if !host_exists {
                    errors.push(VerifyError {
                        message: format!(
                            "BoundaryTimer '{}' attachedToRef '{}' does not reference a task",
                            id, attached_to
                        ),
                        element_id: Some(id.clone()),
                    });
                }

                // 7b. Cycle timers MUST be non-interrupting (cycle + interrupting is invalid)
                if let TimerSpec::Cycle { .. } = &spec {
                    if *interrupting {
                        errors.push(VerifyError {
                            message: format!(
                                "BoundaryTimer '{}': cycle timers must be non-interrupting (cancelActivity=\"false\")",
                                id
                            ),
                            element_id: Some(id.clone()),
                        });
                    }
                }

                // 7c. Must have at least one outgoing edge
                let outgoing = graph
                    .edges_directed(idx, petgraph::Direction::Outgoing)
                    .count();
                if outgoing == 0 {
                    errors.push(VerifyError {
                        message: format!("BoundaryTimer '{}' has no outgoing sequence flow", id),
                        element_id: Some(id.clone()),
                    });
                }

                host_boundary_count
                    .entry(attached_to.clone())
                    .or_default()
                    .push(id.clone());
            }
        }

        // 7d. Phase 2: max 1 boundary timer per host task
        for (host_id, boundary_ids) in &host_boundary_count {
            if boundary_ids.len() > 1 {
                errors.push(VerifyError {
                    message: format!(
                        "Task '{}' has {} boundary timers (max 1 supported in this version): [{}]",
                        host_id,
                        boundary_ids.len(),
                        boundary_ids.join(", ")
                    ),
                    element_id: Some(host_id.clone()),
                });
            }
        }
    }

    // 8. Boundary error event validation
    {
        // Track catch-all count per host task
        let mut host_catch_all_count: HashMap<String, Vec<String>> = HashMap::new();

        for idx in graph.node_indices() {
            if let IRNode::BoundaryError {
                id,
                attached_to,
                error_code,
            } = &graph[idx]
            {
                // 8a. attached_to must reference an existing ServiceTask
                let host_exists = graph.node_indices().any(|other| {
                    matches!(&graph[other],
                        IRNode::ServiceTask { id: host_id, .. }
                        if host_id == attached_to
                    )
                });
                if !host_exists {
                    errors.push(VerifyError {
                        message: format!(
                            "BoundaryError '{}' attachedToRef '{}' does not reference a ServiceTask",
                            id, attached_to
                        ),
                        element_id: Some(id.clone()),
                    });
                }

                // 8b. Must have exactly 1 outgoing edge
                let outgoing = graph
                    .edges_directed(idx, petgraph::Direction::Outgoing)
                    .count();
                if outgoing != 1 {
                    errors.push(VerifyError {
                        message: format!(
                            "BoundaryError '{}' must have exactly 1 outgoing edge, found {}",
                            id, outgoing
                        ),
                        element_id: Some(id.clone()),
                    });
                }

                // 8c. Track catch-all (error_code: None) per host
                if error_code.is_none() {
                    host_catch_all_count
                        .entry(attached_to.clone())
                        .or_default()
                        .push(id.clone());
                }
            }
        }

        // 8d. At most 1 catch-all BoundaryError per host task
        for (host_id, catch_all_ids) in &host_catch_all_count {
            if catch_all_ids.len() > 1 {
                errors.push(VerifyError {
                    message: format!(
                        "Task '{}' has {} catch-all error boundaries (max 1): [{}]",
                        host_id,
                        catch_all_ids.len(),
                        catch_all_ids.join(", ")
                    ),
                    element_id: Some(host_id.clone()),
                });
            }
        }
    }

    // 9. Inclusive gateway validation
    {
        let mut diverging_count = 0u32;
        let mut converging_count = 0u32;

        for idx in graph.node_indices() {
            match &graph[idx] {
                IRNode::GatewayInclusive {
                    id,
                    direction: GatewayDirection::Diverging,
                    ..
                } => {
                    diverging_count += 1;
                    let outgoing = graph
                        .edges_directed(idx, petgraph::Direction::Outgoing)
                        .count();
                    if outgoing < 2 {
                        errors.push(VerifyError {
                            message: format!(
                                "Inclusive gateway (diverging) must have ≥2 outgoing edges, found {}",
                                outgoing
                            ),
                            element_id: Some(id.clone()),
                        });
                    }
                }
                IRNode::GatewayInclusive {
                    id,
                    direction: GatewayDirection::Converging,
                    ..
                } => {
                    converging_count += 1;
                    let incoming = graph
                        .edges_directed(idx, petgraph::Direction::Incoming)
                        .count();
                    if incoming < 2 {
                        errors.push(VerifyError {
                            message: format!(
                                "Inclusive gateway (converging) must have ≥2 incoming edges, found {}",
                                incoming
                            ),
                            element_id: Some(id.clone()),
                        });
                    }
                    let outgoing = graph
                        .edges_directed(idx, petgraph::Direction::Outgoing)
                        .count();
                    if outgoing != 1 {
                        errors.push(VerifyError {
                            message: format!(
                                "Inclusive gateway (converging) must have exactly 1 outgoing edge, found {}",
                                outgoing
                            ),
                            element_id: Some(id.clone()),
                        });
                    }
                }
                _ => {}
            }
        }

        // v1 constraint: single inclusive pair per process
        if diverging_count > 1 {
            errors.push(VerifyError {
                message: format!(
                    "Multiple diverging inclusive gateways ({}) not supported in v1",
                    diverging_count
                ),
                element_id: None,
            });
        }
        if converging_count > 1 {
            errors.push(VerifyError {
                message: format!(
                    "Multiple converging inclusive gateways ({}) not supported in v1",
                    converging_count
                ),
                element_id: None,
            });
        }
    }

    errors
}

/// Structural well-nestedness (SESE) check for `GatewayAnd` fork/join
/// pairs, mirroring `dsl::rpst::verify_sese_nesting`'s DFS-second-visit
/// algorithm: entering a Diverging gateway pushes its element id; the
/// *second* time DFS reaches a node (i.e. `visited.insert` returns
/// `false`, meaning another path already explored it) is when a
/// Converging gateway's pairing is checked and popped. Unlike the DSL's
/// check, `IRNode::GatewayAnd` has no explicit fork/join name reference
/// to cross-validate against, so this can only detect stack-order
/// crossing and unmatched joins structurally — not verify a join closes
/// the *specific* fork a BPMN author intended.
fn check_gateway_and_nesting(
    graph: &IRGraph,
    curr: NodeIndex,
    visited: &mut std::collections::HashSet<NodeIndex>,
    fork_stack: &mut Vec<String>,
    errors: &mut Vec<VerifyError>,
) {
    if !visited.insert(curr) {
        if let IRNode::GatewayAnd {
            direction: GatewayDirection::Converging,
            ..
        } = &graph[curr]
        {
            if fork_stack.pop().is_none() {
                errors.push(VerifyError {
                    message: "Unmatched GatewayAnd (converging): no open diverging \
                        GatewayAnd found — non-well-nested parallel-gateway topology"
                        .to_string(),
                    element_id: Some(graph[curr].id().to_string()),
                });
            }
        }
        return;
    }

    if let IRNode::GatewayAnd {
        direction: GatewayDirection::Diverging,
        ..
    } = &graph[curr]
    {
        fork_stack.push(graph[curr].id().to_string());
    }

    for neighbor in graph.neighbors(curr) {
        check_gateway_and_nesting(graph, neighbor, visited, fork_stack, errors);
    }
}

/// Verify bytecode for bounded-loop safety.
///
/// Rejects backward `Jump`/`BrIf`/`BrIfNot` (infinite loop risk).
/// Allows backward `BrCounterLt` (bounded by counter limit).
pub fn verify_bytecode(program: &CompiledProgram) -> Vec<VerifyError> {
    let mut errors = Vec::new();
    let program_len = Addr::new(program.program().len() as u32);
    for (addr, instr) in program.program().iter().enumerate() {
        let addr = Addr::new(addr as u32);
        match instr {
            Instr::Jump { target } | Instr::BrIf { target } | Instr::BrIfNot { target } => {
                check_target(&mut errors, program, addr, *target, program_len);
                if *target < addr {
                    errors.push(VerifyError {
                        message: format!(
                            "Backward jump at addr {} to {} — only BrCounterLt may jump backward",
                            addr, target
                        ),
                        element_id: program.debug_map().get(&addr).cloned(),
                    });
                }
            }
            Instr::BrCounterLt { target, .. } => {
                check_target(&mut errors, program, addr, *target, program_len);
                // BrCounterLt is allowed to jump backward (it's bounded by limit)
            }
            Instr::V2Fork { targets, .. } => {
                for target in targets.iter().copied() {
                    check_target(&mut errors, program, addr, target, program_len);
                }
            }
            _ => {}
        }
    }
    errors
}

fn check_target(
    errors: &mut Vec<VerifyError>,
    program: &CompiledProgram,
    addr: Addr,
    target: Addr,
    program_len: Addr,
) {
    if target >= program_len {
        errors.push(VerifyError {
            message: format!(
                "Bytecode target out of bounds at addr {}: target {} >= program len {}",
                addr, target, program_len
            ),
            element_id: program.debug_map().get(&addr).cloned(),
        });
    }
}

/// Verify and return Result — convenience wrapper.
pub fn verify_or_err(graph: &IRGraph) -> Result<()> {
    let errors = verify(graph);
    if errors.is_empty() {
        Ok(())
    } else {
        let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        Err(anyhow!("Verification failed:\n{}", msgs.join("\n")))
    }
}

/// Verify data-object declarations and variable-reference resolution in the IR.
///
/// Per A2 §11 / A5 scope. Checks:
///
/// 1. No duplicate data-object ids.
/// 2. Every `Expression::VarRef` in `FfiServiceTask` input bindings resolves
///    to a declared data-object id.
/// 3. Every `FfiOutputBinding.target_variable` resolves to a declared
///    data-object id.
///
/// The verifier for FFI schema compatibility against the FFI catalogue is
/// `verify_ffi_schemas` (A6 — not yet implemented; requires catalogue access).
pub fn verify_data_objects(graph: &IRGraph) -> Vec<VerifyError> {
    let mut errors = Vec::new();

    // Collect declared data objects (id → node).
    let mut declared: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for idx in graph.node_indices() {
        if let IRNode::DataObject { id, .. } = &graph[idx] {
            if declared.insert(id.clone(), id.clone()).is_some() {
                errors.push(VerifyError {
                    message: format!("duplicate data-object id: '{}'", id),
                    element_id: Some(id.clone()),
                });
            }
        }
    }

    // Check all FfiServiceTask bindings resolve.
    for idx in graph.node_indices() {
        if let IRNode::FfiServiceTask {
            id,
            inputs,
            outputs,
            ..
        } = &graph[idx]
        {
            for binding in inputs {
                if let crate::ir::Expression::VarRef(path) = &binding.expression {
                    let first = path.first().map(|s| s.as_str()).unwrap_or("");
                    if !declared.contains_key(first) {
                        errors.push(VerifyError {
                            message: format!(
                                "unresolved input var ref '{}' in task '{}': \
                                 no data object with id '{}'",
                                path.join("."),
                                id,
                                first
                            ),
                            element_id: Some(id.clone()),
                        });
                    }
                }
            }
            for binding in outputs {
                if !declared.contains_key(&binding.target_variable) {
                    errors.push(VerifyError {
                        message: format!(
                            "unresolved output target '{}' in task '{}': \
                             no data object with id '{}'",
                            binding.target_variable, id, binding.target_variable
                        ),
                        element_id: Some(id.clone()),
                    });
                }
            }
        }
    }

    errors
}

/// Verify FFI task schema bindings against the FFI catalogue.
///
/// Per A2 §11. Called by the compiler after `verify_bytecode` succeeds, when
/// a catalogue snapshot is available. Can also be called independently in
/// tooling contexts (LSP, CI lint).
///
/// Produces structured `VerifyError` items for:
/// - Unknown template id
/// - Unknown input/output field names
/// - Type-incompatible input bindings
/// - Required inputs that are not bound
/// - Output bindings that target a `FlagWrite` with a kind that doesn't fit
///   in `bpmn_lite_types::Value` (non-Bool, non-I64)
#[cfg(test)]
mod tests {
    use super::*;

    /// A4.T5: Verifier rejects graph with no StartEvent
    #[test]
    fn test_no_start_event() {
        let mut graph = IRGraph::new();
        graph.add_node(IRNode::End {
            id: "end1".to_string(),
            terminate: false,
        });

        let errors = verify(&graph);
        assert!(errors.iter().any(|e| e.message.contains("No StartEvent")));
    }

    /// A4.T6: Verifier rejects unstructured parallel gateway
    #[test]
    fn test_unmatched_parallel_gateways() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let fork = graph.add_node(IRNode::GatewayAnd {
            id: "fork1".to_string(),
            name: "Fork".to_string(),
            direction: GatewayDirection::Diverging,
        });
        let end = graph.add_node(IRNode::End {
            id: "end1".to_string(),
            terminate: false,
        });

        graph.add_edge(
            start,
            fork,
            IREdge {
                id: "f1".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            fork,
            end,
            IREdge {
                id: "f2".to_string(),
                condition: None,
            },
        );

        let errors = verify(&graph);
        assert!(errors
            .iter()
            .any(|e| e.message.contains("Mismatched parallel gateways")));
    }

    /// V5.1 correction (2026-07-22): equal fork/join counts do not imply
    /// well-nested topology — the count check alone would admit this.
    /// A Converging `GatewayAnd` appears *before* its Diverging counterpart
    /// in the graph (1 fork, 1 join, but out of nesting order), which the
    /// XML->V2Fork/V2Join lowering pre-pass depends on never happening.
    #[test]
    fn test_out_of_order_gateway_and_rejected_despite_matching_counts() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let join_first = graph.add_node(IRNode::GatewayAnd {
            id: "join_first".to_string(),
            name: "Join".to_string(),
            direction: GatewayDirection::Converging,
        });
        let fork_later = graph.add_node(IRNode::GatewayAnd {
            id: "fork_later".to_string(),
            name: "Fork".to_string(),
            direction: GatewayDirection::Diverging,
        });
        let end1 = graph.add_node(IRNode::End {
            id: "end1".to_string(),
            terminate: false,
        });
        let end2 = graph.add_node(IRNode::End {
            id: "end2".to_string(),
            terminate: false,
        });

        graph.add_edge(
            start,
            join_first,
            IREdge {
                id: "f1".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            join_first,
            fork_later,
            IREdge {
                id: "f2".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            fork_later,
            end1,
            IREdge {
                id: "f3".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            fork_later,
            end2,
            IREdge {
                id: "f4".to_string(),
                condition: None,
            },
        );

        let errors = verify(&graph);
        // The old count-only check (§4) would find 1 fork, 1 join — equal,
        // no error. The structural check (§4a) must still reject it.
        assert!(
            !errors.iter().any(|e| e.message.contains("Mismatched parallel gateways")),
            "counts are equal — the count check alone must not fire here"
        );
        assert!(
            errors.iter().any(|e| e.message.contains("Unclosed diverging GatewayAnd")),
            "the structural nesting check must catch the out-of-order pair"
        );
    }
}
