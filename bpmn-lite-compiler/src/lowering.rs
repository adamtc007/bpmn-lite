use crate::ir::*;
use anyhow::{anyhow, Result};
use bpmn_lite_types::*;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Lower a verified IR graph to bytecode.
pub fn lower(graph: &IRGraph) -> Result<CompiledProgram> {
    let start_idx = find_start(graph).ok_or_else(|| anyhow!("No Start node in IR graph"))?;

    // Topological traversal to assign bytecode addresses.
    // We do a BFS from start to get a linear ordering.
    let order = topo_order(graph, start_idx);

    // String interning for task_types and flag names
    let mut task_intern: HashMap<String, u32> = HashMap::new();
    let mut flag_intern: HashMap<String, FlagKey> = HashMap::new();
    let mut task_manifest: Vec<String> = Vec::new();
    let mut wait_id_counter: WaitId = 0;
    let mut join_id_counter: JoinId = 0;

    // First pass: intern strings and assign addresses
    let mut node_addr: HashMap<NodeIndex, Addr> = HashMap::new();
    let mut instructions: Vec<Instr> = Vec::new();
    let mut debug_map: BTreeMap<Addr, String> = BTreeMap::new();
    let mut join_plan: BTreeMap<JoinId, JoinPlanEntry> = BTreeMap::new();
    let mut wait_plan: BTreeMap<WaitId, WaitPlanEntry> = BTreeMap::new();
    let mut message_name_map: BTreeMap<u32, String> = BTreeMap::new();
    let mut race_plan: BTreeMap<RaceId, RacePlanEntry> = BTreeMap::new();
    let mut write_set: BTreeMap<String, HashSet<FlagKey>> = BTreeMap::new();

    // Reserve addresses in order
    // We do two passes: first to assign addresses, then to emit instructions
    // For simplicity, we emit instructions in a single pass with placeholder fixups

    // Assign base address per node
    let mut addr: Addr = 0;
    for &node_idx in &order {
        node_addr.insert(node_idx, addr);
        addr += estimate_instr_count(graph, node_idx);
    }

    // Build boundary timer lookup: host_task_id → (BoundaryTimer node_idx, timer_spec)
    // Phase 2 verifier guarantees max 1 per host.
    let mut boundary_lookup: HashMap<String, (NodeIndex, TimerSpec)> = HashMap::new();
    for &node_idx in &order {
        if let IRNode::BoundaryTimer {
            attached_to, spec, ..
        } = &graph[node_idx]
        {
            boundary_lookup.insert(attached_to.clone(), (node_idx, spec.clone()));
        }
    }

    // Build boundary error lookup: host_task_id → Vec<(node_idx, error_code)>
    let mut boundary_error_lookup: HashMap<String, Vec<(NodeIndex, Option<String>)>> =
        HashMap::new();
    for &node_idx in &order {
        if let IRNode::BoundaryError {
            attached_to,
            error_code,
            ..
        } = &graph[node_idx]
        {
            boundary_error_lookup
                .entry(attached_to.clone())
                .or_default()
                .push((node_idx, error_code.clone()));
        }
    }

    let mut race_id_counter: RaceId = 0;
    let mut boundary_map: BTreeMap<Addr, RaceId> = BTreeMap::new();

    // Pre-scan: allocate join_ids for converging inclusive gateways
    let mut inclusive_join_ids: HashMap<String, JoinId> = HashMap::new();
    for &node_idx in &order {
        if matches!(
            &graph[node_idx],
            IRNode::GatewayInclusive {
                direction: GatewayDirection::Converging,
                ..
            }
        ) {
            let id = graph[node_idx].id().to_string();
            let join_id = join_id_counter;
            join_id_counter += 1;
            inclusive_join_ids.insert(id, join_id);
        }
    }

    // ── Data-object pre-pass ──────────────────────────────────────────────────
    // Run before the instruction emission loop so that FfiServiceTask lowering
    // can resolve `Expression::VarRef` to compiled `BindingSource` values.
    // Bool/I64 data objects get FlagKey assignments from flag_intern (same map
    // used by XOR gateway conditions — shared keys, consistent runtime).
    let mut data_objects: BTreeMap<String, bpmn_lite_types::ffi_bindings::DataObjectDecl> =
        BTreeMap::new();
    let mut ffi_task_decls: BTreeMap<Addr, bpmn_lite_types::ffi_bindings::FfiTaskDecl> =
        BTreeMap::new();
    for &node_idx in &order {
        if let IRNode::DataObject {
            id,
            name: _,
            type_decl,
            role,
        } = &graph[node_idx]
        {
            let storage = assign_storage(type_decl, id, &mut flag_intern);
            data_objects.insert(
                id.clone(),
                bpmn_lite_types::ffi_bindings::DataObjectDecl {
                    id: id.clone(),
                    type_decl: type_decl.clone(),
                    storage,
                    role: role.clone(),
                },
            );
        }
    }

    // Second pass: emit instructions
    for &node_idx in &order {
        let base = node_addr[&node_idx];
        let node = &graph[node_idx];

        // Pad instructions array to reach base address
        while instructions.len() < base as usize {
            instructions.push(Instr::Jump { target: base });
        }

        debug_map.insert(base, node.id().to_string());

        match node {
            IRNode::Start { .. } => {
                // Start is a no-op — just a marker. Jump to next.
                let successors = get_successors(graph, node_idx);
                if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(base + 1);
                    instructions.push(Instr::Jump { target });
                } else {
                    instructions.push(Instr::End);
                }
            }

            IRNode::End { terminate, .. } => {
                if *terminate {
                    instructions.push(Instr::EndTerminate);
                } else {
                    instructions.push(Instr::End);
                }
            }

            IRNode::ServiceTask { id, task_type, .. } => {
                let task_id = intern_task(&mut task_intern, &mut task_manifest, task_type);
                let exec_addr = instructions.len() as Addr;
                instructions.push(Instr::ExecNative {
                    task_type: task_id,
                    argc: 0,
                    retc: 0,
                });

                // Normal path: jump to successor
                let successors = get_successors(graph, node_idx);
                let normal_resume = if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(base + 2);
                    instructions.push(Instr::Jump { target });
                    target
                } else {
                    let next_addr = instructions.len() as Addr;
                    instructions.push(Instr::End);
                    next_addr
                };

                // Check for boundary timer on this task
                if let Some((bt_node_idx, spec)) = boundary_lookup.get(id) {
                    let race_id = race_id_counter;
                    race_id_counter += 1;

                    // Resolve escalation successor DIRECTLY (skip boundary node)
                    let escalation_successors = get_successors(graph, *bt_node_idx);
                    let escalation_addr = escalation_successors
                        .first()
                        .and_then(|s| node_addr.get(s).copied())
                        .unwrap_or(0);

                    let bt_interrupting =
                        if let IRNode::BoundaryTimer { interrupting, .. } = &graph[*bt_node_idx] {
                            *interrupting
                        } else {
                            true
                        };

                    let bt_element_id = graph[*bt_node_idx].id().to_string();

                    let timer_arm = match spec {
                        TimerSpec::Duration { ms } => WaitArm::Timer {
                            duration_ms: *ms,
                            resume_at: escalation_addr,
                            interrupting: bt_interrupting,
                            cycle: None,
                        },
                        TimerSpec::Date { deadline_ms } => WaitArm::Deadline {
                            deadline_ms: *deadline_ms,
                            resume_at: escalation_addr,
                        },
                        TimerSpec::Cycle {
                            interval_ms,
                            max_fires,
                        } => WaitArm::Timer {
                            duration_ms: *interval_ms,
                            resume_at: escalation_addr,
                            interrupting: bt_interrupting,
                            cycle: Some(CycleSpec {
                                interval_ms: *interval_ms,
                                max_fires: *max_fires,
                            }),
                        },
                    };

                    let arms = vec![
                        WaitArm::Internal {
                            kind: 0, // JOB_COMPLETE
                            key_reg: 0,
                            resume_at: normal_resume,
                        },
                        timer_arm,
                    ];

                    race_plan.insert(
                        race_id,
                        RacePlanEntry {
                            arms,
                            boundary_element_id: Some(bt_element_id),
                        },
                    );
                    boundary_map.insert(exec_addr, race_id);
                }
            }

            IRNode::GatewayXor { .. } => {
                let outgoing: Vec<_> = graph
                    .edges_directed(node_idx, petgraph::Direction::Outgoing)
                    .collect();

                // Emit condition checks for edges with conditions
                let mut default_target = None;
                for edge in &outgoing {
                    let target_idx = edge.target();
                    let target_addr = node_addr.get(&target_idx).copied().unwrap_or(0);

                    if let Some(cond) = &edge.weight().condition {
                        let flag_key = intern_flag(&mut flag_intern, &cond.flag_name);
                        instructions.push(Instr::LoadFlag { key: flag_key });

                        match (&cond.op, &cond.literal) {
                            (ConditionOp::Eq, ConditionLiteral::Bool(expected)) => {
                                if *expected {
                                    instructions.push(Instr::BrIf {
                                        target: target_addr,
                                    });
                                } else {
                                    instructions.push(Instr::BrIfNot {
                                        target: target_addr,
                                    });
                                }
                            }
                            (ConditionOp::Neq, ConditionLiteral::Bool(expected)) => {
                                if *expected {
                                    instructions.push(Instr::BrIfNot {
                                        target: target_addr,
                                    });
                                } else {
                                    instructions.push(Instr::BrIf {
                                        target: target_addr,
                                    });
                                }
                            }
                            _ => {
                                // For non-bool conditions, push comparison value and branch
                                // Simplified: treat as bool truthiness check
                                instructions.push(Instr::BrIf {
                                    target: target_addr,
                                });
                            }
                        }
                    } else {
                        default_target = Some(target_addr);
                    }
                }

                // Default edge (jump)
                if let Some(target) = default_target {
                    instructions.push(Instr::Jump { target });
                }
            }

            IRNode::GatewayAnd { direction, .. } => match direction {
                GatewayDirection::Diverging => {
                    let successors = get_successors(graph, node_idx);
                    let targets: Box<[Addr]> = successors
                        .iter()
                        .map(|s| node_addr.get(s).copied().unwrap_or(0))
                        .collect();
                    instructions.push(Instr::Fork { targets });
                }
                GatewayDirection::Converging => {
                    let join_id = join_id_counter;
                    join_id_counter += 1;

                    // Count incoming edges as expected arrivals
                    let incoming = graph
                        .edges_directed(node_idx, petgraph::Direction::Incoming)
                        .count() as u16;

                    let successors = get_successors(graph, node_idx);
                    let next = successors
                        .first()
                        .and_then(|s| node_addr.get(s).copied())
                        .unwrap_or(0);

                    join_plan.insert(
                        join_id,
                        JoinPlanEntry {
                            expected: incoming,
                            next,
                            reg_template: std::array::from_fn(|_| Value::Bool(false)),
                        },
                    );

                    instructions.push(Instr::Join {
                        id: join_id,
                        expected: incoming,
                        next,
                    });
                }
            },

            IRNode::GatewayInclusive { direction, .. } => match direction {
                GatewayDirection::Diverging => {
                    // Build InclusiveBranch per outgoing edge
                    let outgoing: Vec<_> = graph
                        .edges_directed(node_idx, petgraph::Direction::Outgoing)
                        .collect();

                    let branches: Vec<InclusiveBranch> = outgoing
                        .iter()
                        .map(|edge| {
                            let target_idx = edge.target();
                            let target_addr = node_addr.get(&target_idx).copied().unwrap_or(0);
                            let condition_flag = edge
                                .weight()
                                .condition
                                .as_ref()
                                .map(|c| intern_flag(&mut flag_intern, &c.flag_name));
                            InclusiveBranch {
                                condition_flag,
                                target: target_addr,
                            }
                        })
                        .collect();

                    // Find paired converging inclusive gateway's join_id
                    // v1 constraint: single pair per process, so there's exactly one
                    let join_id = inclusive_join_ids.values().next().copied().unwrap_or(0);

                    instructions.push(Instr::ForkInclusive {
                        branches: branches.into_boxed_slice(),
                        join_id,
                        default_target: None,
                    });
                }
                GatewayDirection::Converging => {
                    let gw_id = node.id().to_string();
                    let join_id = inclusive_join_ids.get(&gw_id).copied().unwrap_or(0);

                    let successors = get_successors(graph, node_idx);
                    let next = successors
                        .first()
                        .and_then(|s| node_addr.get(s).copied())
                        .unwrap_or(0);

                    instructions.push(Instr::JoinDynamic { id: join_id, next });
                }
            },

            IRNode::TimerWait { spec, .. } => {
                match spec {
                    TimerSpec::Duration { ms } => {
                        instructions.push(Instr::WaitFor { ms: *ms });
                    }
                    TimerSpec::Date { deadline_ms } => {
                        instructions.push(Instr::WaitUntil {
                            deadline_ms: *deadline_ms,
                        });
                    }
                    TimerSpec::Cycle { interval_ms, .. } => {
                        // Standalone timer cycle treated as single wait for first interval
                        instructions.push(Instr::WaitFor { ms: *interval_ms });
                    }
                }

                let successors = get_successors(graph, node_idx);
                if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(0);
                    instructions.push(Instr::Jump { target });
                }
            }

            IRNode::MessageWait {
                name: msg_name,
                corr_key_source,
                ..
            } => {
                let wait_id = wait_id_counter;
                wait_id_counter += 1;
                let name_id = intern_flag(&mut flag_intern, msg_name);
                message_name_map.insert(name_id, msg_name.clone());
                let corr_reg = parse_corr_reg(corr_key_source);

                wait_plan.insert(
                    wait_id,
                    WaitPlanEntry {
                        wait_type: WaitType::Msg,
                        name: Some(name_id),
                        corr_source: Some(corr_reg),
                    },
                );

                instructions.push(Instr::WaitMsg {
                    wait_id,
                    name: name_id,
                    corr_reg,
                });

                let successors = get_successors(graph, node_idx);
                if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(0);
                    instructions.push(Instr::Jump { target });
                }
            }

            IRNode::HumanWait {
                name: msg_name,
                corr_key_source,
                ..
            } => {
                let wait_id = wait_id_counter;
                wait_id_counter += 1;
                let name_id = intern_flag(&mut flag_intern, msg_name);
                message_name_map.insert(name_id, msg_name.clone());
                let corr_reg = parse_corr_reg(corr_key_source);

                wait_plan.insert(
                    wait_id,
                    WaitPlanEntry {
                        wait_type: WaitType::Human,
                        name: Some(name_id),
                        corr_source: Some(corr_reg),
                    },
                );

                instructions.push(Instr::WaitMsg {
                    wait_id,
                    name: name_id,
                    corr_reg,
                });

                let successors = get_successors(graph, node_idx);
                if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(0);
                    instructions.push(Instr::Jump { target });
                }
            }

            IRNode::BoundaryTimer { .. } => {
                // Structural metadata only — no instruction emitted.
                // Lowering resolves boundary successor directly in the ServiceTask arm.
            }

            IRNode::BoundaryError { .. } => {
                // Structural metadata only — no instruction emitted.
                // Lowering resolves boundary error successor to build error_route_map.
            }

            IRNode::DataObject { .. } => {
                // Structural declaration — no bytecode emitted.
                // Resolved during the data-object pre-pass above.
            }

            IRNode::FfiServiceTask {
                id,
                template_id,
                inputs,
                outputs,
                ..
            } => {
                let exec_addr = base;

                // Compile input bindings.
                let compiled_inputs: Vec<bpmn_lite_types::ffi_bindings::CompiledFfiInputBinding> =
                    inputs
                        .iter()
                        .map(|b| {
                            Ok(bpmn_lite_types::ffi_bindings::CompiledFfiInputBinding {
                                target_field: b.target_field.clone(),
                                source: resolve_expression(&b.expression, &data_objects)?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;

                // Compile output bindings.
                let compiled_outputs: Vec<bpmn_lite_types::ffi_bindings::CompiledFfiOutputBinding> =
                    outputs
                        .iter()
                        .map(|b| {
                            Ok(bpmn_lite_types::ffi_bindings::CompiledFfiOutputBinding {
                                source_field: b.source_field.clone(),
                                target: resolve_output_target(&b.target_variable, &data_objects)?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;

                let argc = compiled_inputs.len() as u16;
                let retc = compiled_outputs.len() as u16;

                // Emit ExecFfi instruction.
                instructions.push(Instr::ExecFfi {
                    template_id: *template_id,
                    argc,
                    retc,
                });

                // Normal-path Jump to first successor.
                let successors = get_successors(graph, node_idx);
                let normal_resume = if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(base + 2);
                    instructions.push(Instr::Jump { target });
                    target
                } else {
                    let next_addr = instructions.len() as Addr;
                    instructions.push(Instr::End);
                    next_addr
                };

                // Boundary timer wiring (same as ServiceTask).
                if let Some((bt_node_idx, spec)) = boundary_lookup.get(id) {
                    let race_id = race_id_counter;
                    race_id_counter += 1;
                    let timer_resume = node_addr.get(bt_node_idx).copied().unwrap_or(exec_addr + 2);
                    let duration_ms = match spec {
                        TimerSpec::Duration { ms } => *ms,
                        TimerSpec::Date { deadline_ms } => {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64;
                            deadline_ms.saturating_sub(now)
                        }
                        TimerSpec::Cycle { interval_ms, .. } => *interval_ms,
                    };
                    let interrupting = matches!(
                        graph[*bt_node_idx],
                        IRNode::BoundaryTimer {
                            interrupting: true,
                            ..
                        }
                    );
                    let cycle = if let TimerSpec::Cycle {
                        interval_ms,
                        max_fires,
                    } = spec
                    {
                        Some(bpmn_lite_types::CycleSpec {
                            interval_ms: *interval_ms,
                            max_fires: *max_fires,
                        })
                    } else {
                        None
                    };
                    race_plan.insert(
                        race_id,
                        RacePlanEntry {
                            arms: vec![
                                WaitArm::Internal {
                                    kind: 0,
                                    key_reg: 0,
                                    resume_at: normal_resume,
                                },
                                WaitArm::Timer {
                                    duration_ms,
                                    resume_at: timer_resume,
                                    interrupting,
                                    cycle,
                                },
                            ],
                            boundary_element_id: Some(graph[*bt_node_idx].id().to_string()),
                        },
                    );
                    boundary_map.insert(exec_addr, race_id);
                }

                // Register FfiTaskDecl.
                ffi_task_decls.insert(
                    exec_addr,
                    bpmn_lite_types::ffi_bindings::FfiTaskDecl {
                        template_id: *template_id,
                        inputs: compiled_inputs,
                        outputs: compiled_outputs,
                    },
                );
            }
        }
    }

    // Build error_route_map from BoundaryError nodes
    let mut error_route_map: BTreeMap<Addr, Vec<ErrorRoute>> = BTreeMap::new();
    for (host_task_id, error_boundaries) in &boundary_error_lookup {
        // Find the host task's bytecode address
        let host_node_idx = graph.node_indices().find(|&idx| {
            matches!(&graph[idx], IRNode::ServiceTask { id, .. } | IRNode::FfiServiceTask { id, .. } if id == host_task_id)
        });
        let Some(host_idx) = host_node_idx else {
            continue; // verifier should have caught this
        };
        let host_addr = node_addr[&host_idx];

        let mut routes = Vec::new();
        for (boundary_node_idx, error_code) in error_boundaries {
            // Find the boundary error node's sole outgoing successor
            let successors: Vec<_> = graph.neighbors(*boundary_node_idx).collect();
            let Some(&successor_idx) = successors.first() else {
                continue; // verifier should have caught this
            };
            let resume_at = node_addr[&successor_idx];
            let boundary_element_id = graph[*boundary_node_idx].id().to_string();

            routes.push(ErrorRoute {
                error_code: error_code.clone(),
                resume_at,
                boundary_element_id,
            });
        }

        // Sort: specific error codes first, catch-all (None) last
        routes.sort_by_key(|r| r.error_code.is_none());
        error_route_map.insert(host_addr, routes);
    }

    // Compute bytecode_version as BLAKE3 of serialized program (B14: unified with FFI template IDs)
    let serialized = serde_json::to_string(&instructions)?;
    let bytecode_version: [u8; 32] = blake3::hash(serialized.as_bytes()).into();

    // Build write_set from flag_intern
    for (name, &key) in &flag_intern {
        write_set.entry(name.clone()).or_default().insert(key);
    }

    // Preserve the intern map so the FFI binding layer can resolve names to keys.
    let flag_symbol_table: BTreeMap<FlagKey, String> = flag_intern
        .into_iter()
        .map(|(name, key)| (key, name))
        .collect();

    Ok(CompiledProgram {
        bytecode_version,
        program: instructions,
        debug_map,
        join_plan,
        wait_plan,
        message_name_map,
        race_plan,
        boundary_map,
        write_set,
        task_manifest,
        error_route_map,
        flag_symbol_table,
        data_objects,
        ffi_task_decls,
    })
}

fn topo_order(graph: &IRGraph, start: NodeIndex) -> Vec<NodeIndex> {
    let mut visited = HashSet::new();
    let mut order = Vec::new();
    let mut queue = std::collections::VecDeque::new();

    // Pass 1: BFS from start (normal flow)
    queue.push_back(start);
    visited.insert(start);
    while let Some(node) = queue.pop_front() {
        order.push(node);
        for neighbor in graph.neighbors(node) {
            if visited.insert(neighbor) {
                queue.push_back(neighbor);
            }
        }
    }

    // Pass 2: sweep ALL unvisited nodes (escalation paths, future constructs)
    for idx in graph.node_indices() {
        if visited.insert(idx) {
            queue.push_back(idx);
            while let Some(node) = queue.pop_front() {
                order.push(node);
                for neighbor in graph.neighbors(node) {
                    if visited.insert(neighbor) {
                        queue.push_back(neighbor);
                    }
                }
            }
        }
    }

    order
}

fn get_successors(graph: &IRGraph, node: NodeIndex) -> Vec<NodeIndex> {
    graph.neighbors(node).collect()
}

fn estimate_instr_count(graph: &IRGraph, node: NodeIndex) -> Addr {
    match &graph[node] {
        IRNode::Start { .. } => 1,
        IRNode::End { .. } => 1,
        IRNode::ServiceTask { .. } => 2, // ExecNative + Jump
        IRNode::GatewayXor { .. } => {
            let outgoing = graph
                .edges_directed(node, petgraph::Direction::Outgoing)
                .count();
            // Each conditional edge: LoadFlag + BrIf, plus default Jump
            (outgoing as Addr * 2).max(1) + 1
        }
        IRNode::GatewayAnd { .. } => 1,       // Fork or Join
        IRNode::GatewayInclusive { .. } => 1, // ForkInclusive or JoinDynamic
        IRNode::TimerWait { .. } => 2,        // WaitFor/WaitUntil + Jump
        IRNode::MessageWait { .. } => 2,      // WaitMsg + Jump
        IRNode::HumanWait { .. } => 2,        // WaitMsg + Jump
        IRNode::BoundaryTimer { .. } => 0,    // structural only — no bytecode emitted
        IRNode::BoundaryError { .. } => 0,    // structural only — no bytecode emitted
        IRNode::DataObject { .. } => 0,       // structural only — no bytecode emitted
        IRNode::FfiServiceTask { .. } => 2,   // ExecFfi + Jump
    }
}

// ── FFI / data-object lowering helpers ────────────────────────────────────────

/// Intern a flag name into the flag_intern map. Returns the assigned FlagKey.
/// If the name is already interned, returns the existing key.
pub(crate) fn intern_flag(map: &mut HashMap<String, FlagKey>, name: &str) -> FlagKey {
    if let Some(&key) = map.get(name) {
        return key;
    }
    let key = map.len() as FlagKey;
    map.insert(name.to_string(), key);
    key
}

/// Assign a storage location to a data object based on its type declaration.
/// For Bool and I64 primitives: intern a FlagKey using the data object id.
/// For everything else: top-level DomainPayload path equal to the data object id.
pub(crate) fn assign_storage(
    type_decl: &bpmn_lite_types::ffi_bindings::DataObjectType,
    id: &str,
    flag_intern: &mut HashMap<String, FlagKey>,
) -> bpmn_lite_types::ffi_bindings::DataObjectStorage {
    use bpmn_lite_types::ffi_bindings::{DataObjectStorage, DataObjectType, PrimitiveType};
    match type_decl {
        DataObjectType::Primitive(PrimitiveType::Bool)
        | DataObjectType::Primitive(PrimitiveType::I64) => {
            DataObjectStorage::Flag(intern_flag(flag_intern, id))
        }
        DataObjectType::Primitive(PrimitiveType::F64)
        | DataObjectType::Primitive(PrimitiveType::String)
        | DataObjectType::SemOsDomain { .. } => {
            DataObjectStorage::DomainPayload(vec![id.to_string()])
        }
    }
}

/// Convert an IR-level literal to a compiled-artifact literal.
pub(crate) fn lower_literal(lit: &crate::ir::IrLiteral) -> bpmn_lite_types::ffi_bindings::Literal {
    use crate::ir::IrLiteral;
    use bpmn_lite_types::ffi_bindings::Literal;
    match lit {
        IrLiteral::Bool(b) => Literal::Bool(*b),
        IrLiteral::I64(n) => Literal::I64(*n),
        IrLiteral::F64(f) => Literal::F64(*f),
        IrLiteral::String(s) => Literal::String(s.clone()),
    }
}

/// Resolve an IR-level expression to a `BindingSource`, using the resolved
/// data-object map to translate variable references to storage locations.
pub(crate) fn resolve_expression(
    expr: &crate::ir::Expression,
    data_objects: &BTreeMap<String, bpmn_lite_types::ffi_bindings::DataObjectDecl>,
) -> Result<bpmn_lite_types::ffi_bindings::BindingSource> {
    use crate::ir::Expression;
    use bpmn_lite_types::ffi_bindings::{BindingSource, DataObjectStorage};
    match expr {
        Expression::Literal(lit) => Ok(BindingSource::Literal(lower_literal(lit))),
        Expression::VarRef(path) => {
            let first = path.first().ok_or_else(|| anyhow!("empty var ref path"))?;
            let decl = data_objects.get(first.as_str()).ok_or_else(|| {
                anyhow!(
                    "unresolved variable reference '{}': no data object with id '{}'",
                    path.join("."),
                    first
                )
            })?;
            match &decl.storage {
                DataObjectStorage::Flag(key) => {
                    if path.len() > 1 {
                        anyhow::bail!(
                            "flag-typed data object '{}' cannot have sub-path segments (got '{}')",
                            first,
                            path[1..].join(".")
                        );
                    }
                    Ok(BindingSource::FlagRef(*key))
                }
                DataObjectStorage::DomainPayload(base_path) => {
                    let mut full_path = base_path.clone();
                    full_path.extend(path[1..].iter().cloned());
                    Ok(BindingSource::DomainPayloadRef(full_path))
                }
            }
        }
    }
}

/// Resolve an output target variable name to a `BindingTarget`.
pub(crate) fn resolve_output_target(
    target_variable: &str,
    data_objects: &BTreeMap<String, bpmn_lite_types::ffi_bindings::DataObjectDecl>,
) -> Result<bpmn_lite_types::ffi_bindings::BindingTarget> {
    use bpmn_lite_types::ffi_bindings::{BindingTarget, DataObjectStorage};
    let decl = data_objects.get(target_variable).ok_or_else(|| {
        anyhow!(
            "unresolved output target '{}': no data object with this id",
            target_variable
        )
    })?;
    match &decl.storage {
        DataObjectStorage::Flag(key) => Ok(BindingTarget::FlagWrite(*key)),
        DataObjectStorage::DomainPayload(path) => {
            Ok(BindingTarget::DomainPayloadWrite(path.clone()))
        }
    }
}

fn intern_task(map: &mut HashMap<String, u32>, manifest: &mut Vec<String>, name: &str) -> u32 {
    if let Some(&id) = map.get(name) {
        return id;
    }
    let id = manifest.len() as u32;
    manifest.push(name.to_string());
    map.insert(name.to_string(), id);
    id
}

fn parse_corr_reg(source: &str) -> u8 {
    source.parse::<u8>().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verifier;

    fn make_linear_graph() -> IRGraph {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let task = graph.add_node(IRNode::ServiceTask {
            id: "task1".to_string(),
            name: "Create Case".to_string(),
            task_type: "create_case".to_string(),
        });
        let end = graph.add_node(IRNode::End {
            id: "end".to_string(),
            terminate: false,
        });

        graph.add_edge(
            start,
            task,
            IREdge {
                id: "f1".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            task,
            end,
            IREdge {
                id: "f2".to_string(),
                condition: None,
            },
        );

        graph
    }

    /// A4.T1: Linear IR → correct bytecode
    #[test]
    fn test_linear_lowering() {
        let graph = make_linear_graph();
        verifier::verify_or_err(&graph).unwrap();

        let program = lower(&graph).unwrap();

        // Should contain at least: Jump (start→task), ExecNative, Jump (task→end), End
        assert!(program.program.len() >= 3);
        assert_eq!(program.task_manifest, vec!["create_case"]);

        // Last instruction should be End
        let last = program.program.last().unwrap();
        assert!(matches!(last, Instr::End));

        // Should have ExecNative somewhere
        assert!(program
            .program
            .iter()
            .any(|i| matches!(i, Instr::ExecNative { .. })));
    }

    /// A4.T2: XOR gateway lowers to BrIf/BrIfNot
    #[test]
    fn test_xor_gateway_lowering() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let gw = graph.add_node(IRNode::GatewayXor {
            id: "gw1".to_string(),
            name: "Decision".to_string(),
        });
        let task_a = graph.add_node(IRNode::ServiceTask {
            id: "task_a".to_string(),
            name: "Task A".to_string(),
            task_type: "do_a".to_string(),
        });
        let task_b = graph.add_node(IRNode::ServiceTask {
            id: "task_b".to_string(),
            name: "Task B".to_string(),
            task_type: "do_b".to_string(),
        });
        let end = graph.add_node(IRNode::End {
            id: "end".to_string(),
            terminate: false,
        });

        graph.add_edge(
            start,
            gw,
            IREdge {
                id: "f1".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            gw,
            task_a,
            IREdge {
                id: "f2".to_string(),
                condition: Some(ConditionExpr {
                    flag_name: "approved".to_string(),
                    op: ConditionOp::Eq,
                    literal: ConditionLiteral::Bool(true),
                }),
            },
        );
        graph.add_edge(
            gw,
            task_b,
            IREdge {
                id: "f3".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            task_a,
            end,
            IREdge {
                id: "f4".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            task_b,
            end,
            IREdge {
                id: "f5".to_string(),
                condition: None,
            },
        );

        let program = lower(&graph).unwrap();

        // Should contain LoadFlag + BrIf for the conditional edge
        assert!(program
            .program
            .iter()
            .any(|i| matches!(i, Instr::LoadFlag { .. })));
        assert!(program
            .program
            .iter()
            .any(|i| matches!(i, Instr::BrIf { .. })));
    }

    /// A4.T3: Parallel fork/join lowers to Fork + Join
    #[test]
    fn test_parallel_fork_join_lowering() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let fork = graph.add_node(IRNode::GatewayAnd {
            id: "fork1".to_string(),
            name: "Fork".to_string(),
            direction: GatewayDirection::Diverging,
        });
        let task_a = graph.add_node(IRNode::ServiceTask {
            id: "task_a".to_string(),
            name: "Task A".to_string(),
            task_type: "do_a".to_string(),
        });
        let task_b = graph.add_node(IRNode::ServiceTask {
            id: "task_b".to_string(),
            name: "Task B".to_string(),
            task_type: "do_b".to_string(),
        });
        let join = graph.add_node(IRNode::GatewayAnd {
            id: "join1".to_string(),
            name: "Join".to_string(),
            direction: GatewayDirection::Converging,
        });
        let end = graph.add_node(IRNode::End {
            id: "end".to_string(),
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
            task_a,
            IREdge {
                id: "f2".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            fork,
            task_b,
            IREdge {
                id: "f3".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            task_a,
            join,
            IREdge {
                id: "f4".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            task_b,
            join,
            IREdge {
                id: "f5".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            join,
            end,
            IREdge {
                id: "f6".to_string(),
                condition: None,
            },
        );

        let program = lower(&graph).unwrap();

        assert!(program
            .program
            .iter()
            .any(|i| matches!(i, Instr::Fork { .. })));
        assert!(program
            .program
            .iter()
            .any(|i| matches!(i, Instr::Join { .. })));
    }

    /// A4.T4: Timer/Message waits lower correctly
    #[test]
    fn test_wait_lowering() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let timer = graph.add_node(IRNode::TimerWait {
            id: "timer1".to_string(),
            spec: TimerSpec::Duration { ms: 5000 },
        });
        let msg = graph.add_node(IRNode::MessageWait {
            id: "msg1".to_string(),
            name: "docs_received".to_string(),
            corr_key_source: "0".to_string(),
        });
        let end = graph.add_node(IRNode::End {
            id: "end".to_string(),
            terminate: false,
        });

        graph.add_edge(
            start,
            timer,
            IREdge {
                id: "f1".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            timer,
            msg,
            IREdge {
                id: "f2".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            msg,
            end,
            IREdge {
                id: "f3".to_string(),
                condition: None,
            },
        );

        let program = lower(&graph).unwrap();

        assert!(program
            .program
            .iter()
            .any(|i| matches!(i, Instr::WaitFor { ms: 5000 })));
        assert!(program
            .program
            .iter()
            .any(|i| matches!(i, Instr::WaitMsg { .. })));
        assert!(program
            .message_name_map
            .values()
            .any(|name| name == "docs_received"));
    }

    /// A4.T7: End-to-end IR → verify → lower → bytecode valid
    #[test]
    fn test_end_to_end_ir_to_bytecode() {
        let graph = make_linear_graph();

        // Verify
        verifier::verify_or_err(&graph).unwrap();

        // Lower
        let program = lower(&graph).unwrap();

        // Bytecode version should be non-zero
        assert_ne!(program.bytecode_version, [0u8; 32]);

        // Debug map should have entries
        assert!(!program.debug_map.is_empty());

        // Task manifest should list task types
        assert!(program.task_manifest.contains(&"create_case".to_string()));
    }

    /// Δ8 — flag_symbol_table is preserved from the lowering intern map.
    ///
    /// A process with a named XOR gateway condition produces a non-empty
    /// symbol table whose entry maps the interned FlagKey back to the
    /// source flag_name string.
    #[test]
    fn test_flag_symbol_table_preserved() {
        // Build: Start → XOR (condition on "approved") → task_a / task_b → End
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let gw = graph.add_node(IRNode::GatewayXor {
            id: "gw1".to_string(),
            name: "Decision".to_string(),
        });
        let task_a = graph.add_node(IRNode::ServiceTask {
            id: "task_a".to_string(),
            name: "Task A".to_string(),
            task_type: "do_a".to_string(),
        });
        let task_b = graph.add_node(IRNode::ServiceTask {
            id: "task_b".to_string(),
            name: "Task B".to_string(),
            task_type: "do_b".to_string(),
        });
        let end = graph.add_node(IRNode::End {
            id: "end".to_string(),
            terminate: false,
        });

        graph.add_edge(
            start,
            gw,
            IREdge {
                id: "f1".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            gw,
            task_a,
            IREdge {
                id: "f2".to_string(),
                condition: Some(ConditionExpr {
                    flag_name: "approved".to_string(),
                    op: ConditionOp::Eq,
                    literal: ConditionLiteral::Bool(true),
                }),
            },
        );
        graph.add_edge(
            gw,
            task_b,
            IREdge {
                id: "f3".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            task_a,
            end,
            IREdge {
                id: "f4".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            task_b,
            end,
            IREdge {
                id: "f5".to_string(),
                condition: None,
            },
        );

        let program = lower(&graph).unwrap();

        // The symbol table must be non-empty and contain "approved".
        assert!(
            !program.flag_symbol_table.is_empty(),
            "flag_symbol_table should be non-empty when conditions reference named flags"
        );
        assert!(
            program.flag_symbol_table.values().any(|n| n == "approved"),
            "flag_symbol_table should contain the condition flag name 'approved'"
        );

        // The FlagKey stored in the table must be used as a LoadFlag operand.
        let table_key = *program
            .flag_symbol_table
            .iter()
            .find(|(_, n)| *n == "approved")
            .unwrap()
            .0;
        assert!(
            program
                .program
                .iter()
                .any(|i| matches!(i, Instr::LoadFlag { key } if *key == table_key)),
            "the FlagKey from flag_symbol_table must appear as a LoadFlag operand"
        );

        // Linear graph (no conditions) produces an empty symbol table.
        let linear = lower(&make_linear_graph()).unwrap();
        assert!(
            linear.flag_symbol_table.is_empty(),
            "flag_symbol_table should be empty when no conditions reference named flags"
        );
    }
}
