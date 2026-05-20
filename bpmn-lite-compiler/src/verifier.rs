use crate::ir::*;
use anyhow::{anyhow, Result};
use bpmn_lite_types::ffi_bindings::{
    BindingSource, BindingTarget, DataObjectStorage, DataObjectType, PrimitiveType,
};
use bpmn_lite_types::{Addr, CompiledProgram, FlagKey, Instr};
use ffi_types::{FfiCatalogueSnapshot, SchemaKind};
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

/// Verify bytecode for bounded-loop safety.
///
/// Rejects backward `Jump`/`BrIf`/`BrIfNot` (infinite loop risk).
/// Allows backward `BrCounterLt` (bounded by counter limit).
pub fn verify_bytecode(program: &CompiledProgram) -> Vec<VerifyError> {
    let mut errors = Vec::new();
    let program_len = program.program.len() as Addr;
    for (addr, instr) in program.program.iter().enumerate() {
        let addr = addr as Addr;
        match instr {
            Instr::Jump { target } | Instr::BrIf { target } | Instr::BrIfNot { target } => {
                check_target(&mut errors, program, addr, *target, program_len);
                if *target < addr {
                    errors.push(VerifyError {
                        message: format!(
                            "Backward jump at addr {} to {} — only BrCounterLt may jump backward",
                            addr, target
                        ),
                        element_id: program.debug_map.get(&addr).cloned(),
                    });
                }
            }
            Instr::BrCounterLt { target, .. } => {
                check_target(&mut errors, program, addr, *target, program_len);
                // BrCounterLt is allowed to jump backward (it's bounded by limit)
            }
            Instr::Fork { targets } => {
                for target in targets.iter().copied() {
                    check_target(&mut errors, program, addr, target, program_len);
                }
            }
            Instr::Join { next, .. } | Instr::JoinDynamic { next, .. } => {
                check_target(&mut errors, program, addr, *next, program_len);
            }
            Instr::WaitAny { arms, .. } => {
                for arm in arms.iter() {
                    check_target(&mut errors, program, addr, arm.resume_at(), program_len);
                }
            }
            Instr::ForkInclusive {
                branches,
                default_target,
                ..
            } => {
                for branch in branches.iter() {
                    check_target(&mut errors, program, addr, branch.target, program_len);
                }
                if let Some(target) = default_target {
                    check_target(&mut errors, program, addr, *target, program_len);
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
            element_id: program.debug_map.get(&addr).cloned(),
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
pub fn verify_ffi_schemas(
    program: &CompiledProgram,
    catalogue: &dyn FfiCatalogueSnapshot,
) -> Vec<VerifyError> {
    let mut errors = Vec::new();

    // Build a reverse-lookup: FlagKey → DataObjectType (for BindingSource::FlagRef type checks).
    let flag_type: HashMap<FlagKey, &DataObjectType> = program
        .data_objects
        .values()
        .filter_map(|d| {
            if let DataObjectStorage::Flag(key) = &d.storage {
                Some((*key, &d.type_decl))
            } else {
                None
            }
        })
        .collect();

    for (addr, task_decl) in &program.ffi_task_decls {
        // 1. Template lookup.
        let template =
            match catalogue.lookup(&task_decl.template_id) {
                Some(t) => t,
                None => {
                    errors.push(VerifyError {
                        message: format!(
                        "FFI template not found in catalogue: {:02x}{:02x}...{:02x}{:02x} (pc={})",
                        task_decl.template_id[0], task_decl.template_id[1],
                        task_decl.template_id[30], task_decl.template_id[31],
                        addr
                    ),
                        element_id: None,
                    });
                    continue; // skip further checks — no template to validate against
                }
            };

        // Index template schemas by field name for O(1) lookup.
        let input_by_name: HashMap<&str, &ffi_types::FieldSchema> = template
            .input_schema
            .iter()
            .map(|f| (f.name.as_str(), f))
            .collect();
        let output_by_name: HashMap<&str, &ffi_types::FieldSchema> = template
            .output_schema
            .iter()
            .map(|f| (f.name.as_str(), f))
            .collect();

        // Track which fields were bound (for required-field check).
        let mut bound_inputs: std::collections::HashSet<&str> = std::collections::HashSet::new();

        // 2. Per-input binding checks.
        for binding in &task_decl.inputs {
            let field_name = binding.target_field.as_str();

            let schema_field = match input_by_name.get(field_name) {
                Some(f) => *f,
                None => {
                    errors.push(VerifyError {
                        message: format!(
                            "input binding targets unknown field '{}' (template has: {})",
                            field_name,
                            template
                                .input_schema
                                .iter()
                                .map(|f| f.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        element_id: None,
                    });
                    continue;
                }
            };
            bound_inputs.insert(field_name);

            // Type-compatibility check.
            if let Some(err_msg) = check_input_source_compat(
                &binding.source,
                &schema_field.kind,
                field_name,
                &flag_type,
            ) {
                errors.push(VerifyError {
                    message: err_msg,
                    element_id: None,
                });
            }
        }

        // 3. Required-input coverage check.
        for field in &template.input_schema {
            if field.required && !bound_inputs.contains(field.name.as_str()) {
                errors.push(VerifyError {
                    message: format!(
                        "required input field '{}' is not bound (template_id={})",
                        field.name,
                        hex_short(&task_decl.template_id)
                    ),
                    element_id: None,
                });
            }
        }

        // 4. Per-output binding checks.
        for binding in &task_decl.outputs {
            let field_name = binding.source_field.as_str();

            let schema_field = match output_by_name.get(field_name) {
                Some(f) => *f,
                None => {
                    errors.push(VerifyError {
                        message: format!(
                            "output binding sources unknown field '{}' (template has: {})",
                            field_name,
                            template
                                .output_schema
                                .iter()
                                .map(|f| f.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        element_id: None,
                    });
                    continue;
                }
            };

            // FlagWrite target only valid for Bool/I64.
            if let BindingTarget::FlagWrite(_) = &binding.target {
                if !schema_field.kind.fits_in_flag() {
                    errors.push(VerifyError {
                        message: format!(
                            "output field '{}' has kind {:?} and cannot target a process flag \
                             (only Bool/I64 fit in bpmn_lite_types::Value); \
                             use a DomainPayload data object instead",
                            field_name, schema_field.kind
                        ),
                        element_id: None,
                    });
                }
            }
        }
    }

    errors
}

/// Check whether a `BindingSource` is type-compatible with a `SchemaKind`.
/// Returns `Some(error_message)` on incompatibility, `None` on success.
fn check_input_source_compat(
    source: &BindingSource,
    kind: &SchemaKind,
    field_name: &str,
    flag_type: &HashMap<FlagKey, &DataObjectType>,
) -> Option<String> {
    use bpmn_lite_types::ffi_bindings::Literal;
    match source {
        BindingSource::Literal(lit) => {
            let compat = match (lit, kind) {
                (Literal::Bool(_), SchemaKind::Bool) => true,
                (Literal::I64(_), SchemaKind::I64) => true,
                (Literal::F64(_), SchemaKind::F64) => true,
                // String literal is accepted for String and SemOsDomain
                // (symbol→domain-value matching done at call time by the owner).
                (Literal::String(_), SchemaKind::String | SchemaKind::SemOsDomain { .. }) => true,
                // DomainPayloadRef + Opaque: flow-through.
                (_, SchemaKind::Opaque { .. }) => true,
                _ => false,
            };
            if compat {
                None
            } else {
                Some(format!(
                    "input field '{}': literal type {:?} is incompatible with schema kind {:?}",
                    field_name, lit, kind
                ))
            }
        }
        BindingSource::FlagRef(key) => {
            // Determine the flag's declared type from the data-object declarations.
            let dt = flag_type.get(key)?; // None → flag not from a data object; skip check
            let compat = match (dt, kind) {
                (DataObjectType::Primitive(PrimitiveType::Bool), SchemaKind::Bool) => true,
                (DataObjectType::Primitive(PrimitiveType::I64), SchemaKind::I64) => true,
                // Str(u32) flags (from flag_intern without a data-object decl)
                // are not directly bindable — the compiler should reject them.
                // If we see a String primitive in a FlagRef, it means an F64/String
                // data object was wrongly assigned Flag storage (shouldn't happen).
                (_, SchemaKind::Opaque { .. }) => true,
                _ => false,
            };
            if compat {
                None
            } else {
                Some(format!(
                    "input field '{}': flag-ref type {:?} is incompatible with schema kind {:?}",
                    field_name, dt, kind
                ))
            }
        }
        // DomainPayloadRef is runtime-typed; compatible with any schema kind.
        BindingSource::DomainPayloadRef(_) => None,
    }
}

fn hex_short(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(16);
    for b in &bytes[..8] {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

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

    // ── A6 verify_ffi_schemas tests ───────────────────────────────────────────

    use bpmn_lite_types::ffi_bindings::{
        BindingSource, BindingTarget, CompiledFfiInputBinding, CompiledFfiOutputBinding,
        FfiTaskDecl, Literal,
    };
    use ffi_types::{compute_template_id, FfiTemplate, FieldSchema, Idempotency, SchemaKind};
    use std::collections::BTreeMap;

    struct MockCatalogue(std::collections::HashMap<[u8; 32], FfiTemplate>);

    impl FfiCatalogueSnapshot for MockCatalogue {
        fn lookup(&self, template_id: &[u8; 32]) -> Option<&FfiTemplate> {
            self.0.get(template_id)
        }
    }

    fn make_template(
        owner_type: &str,
        input_schema: Vec<FieldSchema>,
        output_schema: Vec<FieldSchema>,
    ) -> FfiTemplate {
        let mut t = FfiTemplate {
            template_id: [0u8; 32],
            owner_type: owner_type.to_string(),
            owner_metadata: vec![],
            input_schema,
            output_schema,
            idempotency: Idempotency::Idempotent,
            tenant_id: "t".to_string(),
            published_at: 0,
            publisher: "test".to_string(),
        };
        t.template_id = compute_template_id(&t);
        t
    }

    fn field(name: &str, kind: SchemaKind, required: bool) -> FieldSchema {
        FieldSchema {
            name: name.to_string(),
            kind,
            required,
        }
    }

    fn empty_program() -> CompiledProgram {
        CompiledProgram {
            bytecode_version: [0u8; 32],
            program: vec![],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        }
    }

    #[test]
    fn a6_template_not_in_catalogue_emits_error() {
        let cat = MockCatalogue(Default::default());
        let mut prog = empty_program();
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: [42u8; 32],
                inputs: vec![],
                outputs: vec![],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(!errs.is_empty());
        assert!(errs[0].message.contains("not found"));
    }

    #[test]
    fn a6_unknown_input_field_emits_error() {
        let t = make_template("dmn-lite", vec![field("x", SchemaKind::Bool, true)], vec![]);
        let tid = t.template_id;
        let mut cat_map = std::collections::HashMap::new();
        cat_map.insert(tid, t);
        let cat = MockCatalogue(cat_map);

        let mut prog = empty_program();
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: tid,
                inputs: vec![CompiledFfiInputBinding {
                    target_field: "unknown_field".to_string(),
                    source: BindingSource::Literal(Literal::Bool(true)),
                }],
                outputs: vec![],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(errs
            .iter()
            .any(|e| e.message.contains("unknown") && e.message.contains("unknown_field")));
    }

    #[test]
    fn a6_required_input_not_bound_emits_error() {
        let t = make_template(
            "dmn-lite",
            vec![field("required_x", SchemaKind::Bool, true)],
            vec![],
        );
        let tid = t.template_id;
        let mut cat_map = std::collections::HashMap::new();
        cat_map.insert(tid, t);
        let cat = MockCatalogue(cat_map);

        let mut prog = empty_program();
        // No input bindings at all.
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: tid,
                inputs: vec![],
                outputs: vec![],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(errs
            .iter()
            .any(|e| e.message.contains("required") && e.message.contains("required_x")));
    }

    #[test]
    fn a6_optional_input_not_bound_is_ok() {
        let t = make_template(
            "dmn-lite",
            vec![field("optional_x", SchemaKind::Bool, false)],
            vec![],
        );
        let tid = t.template_id;
        let mut cat_map = std::collections::HashMap::new();
        cat_map.insert(tid, t);
        let cat = MockCatalogue(cat_map);

        let mut prog = empty_program();
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: tid,
                inputs: vec![],
                outputs: vec![],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(
            errs.is_empty(),
            "optional unbound field should not error: {:?}",
            errs
        );
    }

    #[test]
    fn a6_type_compatible_bindings_pass() {
        let t = make_template(
            "dmn-lite",
            vec![
                field("b", SchemaKind::Bool, true),
                field("n", SchemaKind::I64, true),
                field("s", SchemaKind::String, false),
            ],
            vec![],
        );
        let tid = t.template_id;
        let mut cat_map = std::collections::HashMap::new();
        cat_map.insert(tid, t);
        let cat = MockCatalogue(cat_map);

        let mut prog = empty_program();
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: tid,
                inputs: vec![
                    CompiledFfiInputBinding {
                        target_field: "b".to_string(),
                        source: BindingSource::Literal(Literal::Bool(true)),
                    },
                    CompiledFfiInputBinding {
                        target_field: "n".to_string(),
                        source: BindingSource::Literal(Literal::I64(42)),
                    },
                ],
                outputs: vec![],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(
            errs.is_empty(),
            "compatible bindings should pass: {:?}",
            errs
        );
    }

    #[test]
    fn a6_type_incompatible_literal_emits_error() {
        let t = make_template("dmn-lite", vec![field("b", SchemaKind::Bool, true)], vec![]);
        let tid = t.template_id;
        let mut cat_map = std::collections::HashMap::new();
        cat_map.insert(tid, t);
        let cat = MockCatalogue(cat_map);

        let mut prog = empty_program();
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: tid,
                inputs: vec![CompiledFfiInputBinding {
                    target_field: "b".to_string(),
                    source: BindingSource::Literal(Literal::I64(1)), // wrong type
                }],
                outputs: vec![],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(errs
            .iter()
            .any(|e| e.message.contains("incompatible") && e.message.contains("'b'")));
    }

    #[test]
    fn a6_domain_payload_ref_is_always_compatible() {
        let t = make_template(
            "dmn-lite",
            vec![field(
                "domain_field",
                SchemaKind::SemOsDomain {
                    domain_id: uuid::Uuid::nil(),
                    version_hash: [0u8; 32],
                },
                true,
            )],
            vec![],
        );
        let tid = t.template_id;
        let mut cat_map = std::collections::HashMap::new();
        cat_map.insert(tid, t);
        let cat = MockCatalogue(cat_map);

        let mut prog = empty_program();
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: tid,
                inputs: vec![CompiledFfiInputBinding {
                    target_field: "domain_field".to_string(),
                    source: BindingSource::DomainPayloadRef(vec![
                        "customer".to_string(),
                        "jurisdiction".to_string(),
                    ]),
                }],
                outputs: vec![],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(
            errs.is_empty(),
            "DomainPayloadRef should be compatible: {:?}",
            errs
        );
    }

    #[test]
    fn a6_unknown_output_field_emits_error() {
        let t = make_template(
            "dmn-lite",
            vec![],
            vec![field("result", SchemaKind::Bool, false)],
        );
        let tid = t.template_id;
        let mut cat_map = std::collections::HashMap::new();
        cat_map.insert(tid, t);
        let cat = MockCatalogue(cat_map);

        let mut prog = empty_program();
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: tid,
                inputs: vec![],
                outputs: vec![CompiledFfiOutputBinding {
                    source_field: "no_such_field".to_string(),
                    target: BindingTarget::FlagWrite(0),
                }],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(errs
            .iter()
            .any(|e| e.message.contains("unknown") && e.message.contains("no_such_field")));
    }

    #[test]
    fn a6_flag_write_for_non_flag_kind_emits_error() {
        let t = make_template(
            "dmn-lite",
            vec![],
            vec![field("decimal_out", SchemaKind::F64, false)],
        );
        let tid = t.template_id;
        let mut cat_map = std::collections::HashMap::new();
        cat_map.insert(tid, t);
        let cat = MockCatalogue(cat_map);

        let mut prog = empty_program();
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: tid,
                inputs: vec![],
                outputs: vec![CompiledFfiOutputBinding {
                    source_field: "decimal_out".to_string(),
                    target: BindingTarget::FlagWrite(0), // F64 can't go into a flag
                }],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(errs
            .iter()
            .any(|e| e.message.contains("cannot target a process flag")));
    }

    #[test]
    fn a6_flag_write_for_bool_kind_passes() {
        let t = make_template(
            "dmn-lite",
            vec![],
            vec![field("eligible", SchemaKind::Bool, false)],
        );
        let tid = t.template_id;
        let mut cat_map = std::collections::HashMap::new();
        cat_map.insert(tid, t);
        let cat = MockCatalogue(cat_map);

        let mut prog = empty_program();
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: tid,
                inputs: vec![],
                outputs: vec![CompiledFfiOutputBinding {
                    source_field: "eligible".to_string(),
                    target: BindingTarget::FlagWrite(0),
                }],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(errs.is_empty(), "Bool FlagWrite should pass: {:?}", errs);
    }

    #[test]
    fn a6_domain_payload_write_always_passes() {
        let t = make_template(
            "dmn-lite",
            vec![],
            vec![
                field("decimal_out", SchemaKind::F64, false),
                field(
                    "domain_out",
                    SchemaKind::SemOsDomain {
                        domain_id: uuid::Uuid::nil(),
                        version_hash: [0u8; 32],
                    },
                    false,
                ),
            ],
        );
        let tid = t.template_id;
        let mut cat_map = std::collections::HashMap::new();
        cat_map.insert(tid, t);
        let cat = MockCatalogue(cat_map);

        let mut prog = empty_program();
        prog.ffi_task_decls.insert(
            0,
            FfiTaskDecl {
                template_id: tid,
                inputs: vec![],
                outputs: vec![
                    CompiledFfiOutputBinding {
                        source_field: "decimal_out".to_string(),
                        target: BindingTarget::DomainPayloadWrite(vec![
                            "result".to_string(),
                            "decimal".to_string(),
                        ]),
                    },
                    CompiledFfiOutputBinding {
                        source_field: "domain_out".to_string(),
                        target: BindingTarget::DomainPayloadWrite(vec![
                            "customer".to_string(),
                            "type".to_string(),
                        ]),
                    },
                ],
            },
        );
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(
            errs.is_empty(),
            "DomainPayloadWrite should always pass: {:?}",
            errs
        );
    }

    #[test]
    fn a6_clean_program_with_no_ffi_tasks_produces_no_errors() {
        let cat = MockCatalogue(Default::default());
        let prog = empty_program();
        let errs = verify_ffi_schemas(&prog, &cat);
        assert!(errs.is_empty());
    }
}
