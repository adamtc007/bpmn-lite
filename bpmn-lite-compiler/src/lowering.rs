use crate::ir::*;
use anyhow::{anyhow, Result};
use bpmn_lite_types::*;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::{BTreeMap, HashMap, HashSet};

/// V5.3 (§18, landed 2026-07-23): `lower()` now emits v2 words
/// unconditionally for every construct, including inclusive gateways
/// (`V2Fork`/`V2Join` + the dynamic-arity skip-to-join pattern, ruling H)
/// and boundary timers (`V2Guard`/`V2GuardN` + `GUARD-TIMER>`, ruling I) —
/// the two constructs that, until this landing, stayed on a v1-only path
/// behind a `LoweringTarget` gate because `bpmn-lite-engine/src/tests.rs`'s
/// `T-IG-6`/`t_auth_6_boundary_timer_yaml` (plus `T-AUTH-2`) locked v1's
/// literal output through `lower()`. Those tests are relocked to v2's
/// output as part of this same landing (see their own doc comments) — the
/// `LoweringTarget` enum this module used to carry (`V1`/`V2`, threaded
/// through a `lower_with_target` dispatcher) is retired entirely, not left
/// as a now-single-valued indirection: every v1-only branch this module
/// had (`Instr::ForkInclusive`/`JoinDynamic` emission, the `race_plan`/
/// `boundary_map` boundary-timer side-table construction, the
/// `IRNode::MultiInstance` v1-rejection arm) is deleted outright, per
/// V5.3's "delete v1 instruction emission from both compilers" mandate —
/// not gated on a value nothing ever sets differently any more.
pub fn lower(graph: &IRGraph) -> Result<CompiledProgram> {
    let start_idx = find_start(graph).ok_or_else(|| anyhow!("No Start node in IR graph"))?;

    // Topological traversal to assign bytecode addresses.
    // We do a BFS from start to get a linear ordering.
    let order = topo_order(graph, start_idx);

    // String interning for task_types and flag names
    let mut task_intern: HashMap<String, u32> = HashMap::new();
    let mut flag_intern: HashMap<String, FlagKey> = HashMap::new();
    let mut task_manifest: Vec<String> = Vec::new();

    // First pass: intern strings and assign addresses
    let mut node_addr: HashMap<NodeIndex, Addr> = HashMap::new();
    let mut instructions: Vec<Instr> = Vec::new();
    let mut debug_map: BTreeMap<Addr, String> = BTreeMap::new();
    let join_plan: BTreeMap<JoinId, JoinPlanEntry> = BTreeMap::new();
    let wait_plan: BTreeMap<WaitId, WaitPlanEntry> = BTreeMap::new();
    let mut message_name_map: BTreeMap<u32, String> = BTreeMap::new();
    let mut write_set: BTreeMap<String, HashSet<FlagKey>> = BTreeMap::new();

    // Build boundary timer lookup: host_task_id → (BoundaryTimer node_idx, timer_spec)
    // Phase 2 verifier guarantees max 1 per host. Moved ahead of the
    // address-assignment pass below (was previously built after it,
    // V5-pre-existing ordering) because `LoweringTarget::V2` sizing needs
    // to know, per host task, whether a boundary timer is attached BEFORE
    // computing that task's own instruction count — the v1 path never
    // needed this (boundary timers cost zero bytecode under v1, resolved
    // entirely through the `race_plan`/`boundary_map` side tables instead).
    let mut boundary_lookup: HashMap<String, (NodeIndex, TimerSpec)> = HashMap::new();
    for &node_idx in &order {
        if let IRNode::BoundaryTimer {
            attached_to, spec, ..
        } = &graph[node_idx]
        {
            boundary_lookup.insert(attached_to.clone(), (node_idx, spec.clone()));
        }
    }

    // V5 (§18 ruling I, `LoweringTarget::V2` only): a non-interrupting
    // (`V2GuardN`) boundary timer's handler fibre inherits the guard's
    // token POST-push (V4's ratified `V2GuardN` handler-entry-state
    // derivation, `bpmn-lite-kernel`'s
    // `v2_nested_guard_n_under_guard`-family fixtures, `V2GuardN`'s handler
    // at e.g. address 7 closing with its own `V2GuardNEnd` at address 7
    // before reaching `End`) — unlike an interrupting `V2Guard`'s handler,
    // which runs PRE-push (the scope was already retired at trigger time)
    // and needs no close. V-1 (control-stack balance) therefore requires
    // the escalation flow's own terminal `End`/`EndTerminate` to close the
    // inherited `V2GuardN` scope before it, not just the host task's
    // normal-completion path. Walked here, over the IR graph (not
    // bytecode — addresses aren't assigned yet), following the escalation
    // flow's SEQUENCE OF SINGLE-SUCCESSOR nodes from the boundary event's
    // outgoing flow to its own terminal End. **Deliberately narrow, and
    // recorded as such rather than silently assumed general:** a
    // branching escalation flow (a gateway, or a second boundary timer
    // nested in the handler chain) is NOT resolved by this walk — it
    // simply finds no terminal to mark, and `verify_v2_control_stack`
    // then rejects the resulting program with a precise V-1 diagnostic
    // ("control stack not empty at program end") rather than this
    // function silently mis-lowering it. A linear escalation chain (the
    // common BPMN shape — one or more tasks between the boundary event and
    // its own end) is fully supported; a branching one is a real,
    // out-of-this-step's-scope gap, not a hidden defect.
    let mut guardn_close_before_end: HashSet<NodeIndex> = HashSet::new();
    {
        for (host_idx, spec) in boundary_lookup.values() {
            let interrupting = matches!(
                &graph[*host_idx],
                IRNode::BoundaryTimer {
                    interrupting: true,
                    ..
                }
            );
            let _ = spec;
            if interrupting {
                continue;
            }
            let mut cursor = get_successors(graph, *host_idx).into_iter().next();
            let mut steps = 0usize;
            while let Some(idx) = cursor {
                steps += 1;
                if steps > graph.node_count() {
                    break; // cycle guard — should not occur (V-8 forbids
                           // unbounded backward v2 edges elsewhere), but
                           // this walk must not hang regardless.
                }
                if matches!(&graph[idx], IRNode::End { .. }) {
                    guardn_close_before_end.insert(idx);
                    break;
                }
                let succs = get_successors(graph, idx);
                cursor = if succs.len() == 1 {
                    Some(succs[0])
                } else {
                    None // branching escalation flow — unsupported, see above.
                };
            }
        }
    }

    // V5 (§18 ruling H, `LoweringTarget::V2` only): per diverging
    // `GatewayInclusive` node, classify its outgoing edges into "always
    // live" (at most one — an edge with no condition; XML has no separate
    // default-flow concept beyond this, see the module-level V5 doc note
    // below) and "conditional" (a flag-guarded edge, needs a runtime
    // check). Computed unconditionally (cheap, a handful of nodes) rather
    // than gated on `target`, so `LoweringTarget::V1`'s behaviour is
    // provably untouched by this block's presence.
    let inclusive_branches = collect_inclusive_branches(graph, &order);

    // Reserve addresses in order
    // We do two passes: first to assign addresses, then to emit instructions
    // For simplicity, we emit instructions in a single pass with placeholder fixups

    // Assign base address per node
    let mut addr = Addr::new(0);
    for &node_idx in &order {
        node_addr.insert(node_idx, addr);
        addr += instr_count_for(graph, node_idx, &boundary_lookup, &inclusive_branches, &guardn_close_before_end);
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

    // Pre-scan: pair each converging GatewayAnd (parallel join) with its
    // diverging counterpart's own bytecode `Addr`, for `V2Fork`/`V2Join`'s
    // static `pairing` field (V-3's arity proof; runtime resolution is by
    // dynamic handle only — see the V&S §5 word-table entry for `JOIN`).
    // `order` is BFS from Start, so a Diverging gateway is always visited
    // strictly before any Join reachable only through its forked branches.
    // Correctness depends on well-nested (SESE) topology — CLAUDE.md's
    // settled decision, not a stated V&S theorem — which `verifier::verify`
    // now checks structurally (§4a below) before `lower` is called in the
    // real pipeline, rejecting non-well-nested input rather than letting
    // this stack silently mispair it.
    let mut fork_pairing_stack: Vec<Addr> = Vec::new();
    let mut fork_pairing: HashMap<NodeIndex, Addr> = HashMap::new();
    for &node_idx in &order {
        match &graph[node_idx] {
            IRNode::GatewayAnd {
                direction: GatewayDirection::Diverging,
                ..
            } => {
                fork_pairing_stack.push(node_addr[&node_idx]);
            }
            IRNode::GatewayAnd {
                direction: GatewayDirection::Converging,
                ..
            } => {
                if let Some(pairing) = fork_pairing_stack.pop() {
                    fork_pairing.insert(node_idx, pairing);
                }
            }
            _ => {}
        }
    }

    // V5 (§18 ruling H, `LoweringTarget::V2` only): pair each diverging
    // `GatewayInclusive` with its converging counterpart's own bytecode
    // `Addr`, exactly the `fork_pairing` pattern above, generalized to a
    // second, independent stack so `GatewayAnd` and `GatewayInclusive`
    // pairs never cross-match. `V2Fork`'s `pairing` field is NOT
    // necessarily the diverging node's own base address
    // (`node_addr[&node_idx]`) — a pure-conditional gateway's zero-match
    // precheck (`inclusive_precheck_len`) occupies the node's own leading
    // instructions, pushing `V2Fork` itself to `base + precheck_len`. Both
    // maps below account for that offset, computed once here (not
    // re-derived at emission time) using `inclusive_branches`, the same
    // classification the sizing pass already computed.
    let mut inclusive_pairing_stack: Vec<NodeIndex> = Vec::new();
    // Diverging node → its own paired converging node's base `Addr` (the
    // skip-to-join target every per-branch header needs).
    let mut inclusive_join_addr: HashMap<NodeIndex, Addr> = HashMap::new();
    // Converging node → its paired diverging node's ACTUAL `V2Fork`
    // address (`base + precheck_len`, not the diverging node's own `base`)
    // — `V2Join`'s `pairing` field.
    let mut inclusive_fork_addr: HashMap<NodeIndex, Addr> = HashMap::new();
    for &node_idx in &order {
        match &graph[node_idx] {
            IRNode::GatewayInclusive {
                direction: GatewayDirection::Diverging,
                ..
            } => {
                inclusive_pairing_stack.push(node_idx);
            }
            IRNode::GatewayInclusive {
                direction: GatewayDirection::Converging,
                ..
            } => {
                if let Some(diverging_idx) = inclusive_pairing_stack.pop() {
                    inclusive_join_addr.insert(diverging_idx, node_addr[&node_idx]);
                    let precheck_len = inclusive_branches
                        .get(&diverging_idx)
                        .map(|branches| inclusive_precheck_len(branches))
                        .unwrap_or(0);
                    inclusive_fork_addr
                        .insert(node_idx, node_addr[&diverging_idx] + precheck_len);
                }
            }
            _ => {}
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
        while instructions.len() < base.index() {
            instructions.push(Instr::Jump { target: base });
        }

        // Structural-only IR nodes intentionally emit no instruction. Recording
        // their shared/terminal address would create a dangling debug-map entry.
        if instr_count_for(graph, node_idx, &boundary_lookup, &inclusive_branches, &guardn_close_before_end) > 0 {
            debug_map.insert(base, node.id().to_string());
        }

        match node {
            IRNode::Start { .. } => {
                // Start is a no-op — just a marker. Jump to next.
                let successors = get_successors(graph, node_idx);
                if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(base + 1u32);
                    instructions.push(Instr::Jump { target });
                } else {
                    instructions.push(Instr::End);
                }
            }

            IRNode::End { terminate, .. } => {
                if guardn_close_before_end.contains(&node_idx) {
                    // A non-interrupting boundary timer's escalation
                    // terminal — close the inherited V2GuardN scope before
                    // this End (see the pre-pass doc comment above).
                    instructions.push(Instr::V2GuardNEnd);
                }
                if *terminate {
                    instructions.push(Instr::EndTerminate);
                } else {
                    instructions.push(Instr::End);
                }
            }

            IRNode::ServiceTask { id, task_type, .. } => {
                lower_boundary_guarded_task_v2(
                    graph,
                    node_idx,
                    id,
                    task_type,
                    base,
                    &node_addr,
                    &boundary_lookup,
                    &mut task_intern,
                    &mut task_manifest,
                    &mut instructions,
                )?;
            }

            IRNode::GatewayXor { .. } => {
                let outgoing: Vec<_> = graph
                    .edges_directed(node_idx, petgraph::Direction::Outgoing)
                    .collect();

                // Emit condition checks for edges with conditions
                let mut default_target = None;
                for edge in &outgoing {
                    let target_idx = edge.target();
                    let target_addr = node_addr.get(&target_idx).copied().unwrap_or(Addr::new(0));

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
                        .map(|s| node_addr.get(s).copied().unwrap_or(Addr::new(0)))
                        .collect();
                    instructions.push(Instr::V2Fork {
                        targets,
                        pairing: base,
                    });
                }
                GatewayDirection::Converging => {
                    // No `join_id`/`JoinPlanEntry` side-table entry (V-9
                    // forbids it surviving into a v2 envelope) — resolution
                    // is via the dynamically-inherited handle only.
                    //
                    // No silent fallback here: `verifier::verify` rejects
                    // non-well-nested GatewayAnd topology before `lower` is
                    // ever called in the real pipeline (§4a), but `lower`
                    // is itself a public fn callable directly — a missing
                    // pairing must fail loudly, not default to this node's
                    // own address and mispair silently.
                    let pairing = fork_pairing.get(&node_idx).copied().ok_or_else(|| {
                        anyhow!(
                            "GatewayAnd '{}' (converging) has no matching diverging \
                             GatewayAnd on the fork-pairing stack — non-well-nested \
                             parallel-gateway topology",
                            node.id()
                        )
                    })?;
                    instructions.push(Instr::V2Join { pairing });

                    // `V2Join` carries no `next` field (continuation is
                    // PC+1 on last arrival, per K-3) — an explicit trailing
                    // `Jump` supplies what v1's embedded `next` used to.
                    let successors = get_successors(graph, node_idx);
                    let next = successors
                        .first()
                        .and_then(|s| node_addr.get(s).copied())
                        .unwrap_or(Addr::new(0));
                    instructions.push(Instr::Jump { target: next });
                }
            },

            // V5 (§18 ruling H, `LoweringTarget::V2`): dynamic-arity
            // `V2Fork`/`V2Join` lowering, replacing v1's fat
            // `ForkInclusive`/`JoinDynamic` with the proven skip-to-join
            // pattern — see `lower_inclusive_diverging_v2`'s doc comment
            // for the full shape and the zero-match/default-branch design.
            IRNode::GatewayInclusive {
                direction: GatewayDirection::Diverging,
                ..
            } => {
                let join_addr = inclusive_join_addr.get(&node_idx).copied().ok_or_else(|| {
                    anyhow!(
                        "GatewayInclusive '{}' (diverging) has no matching converging \
                         GatewayInclusive — non-well-nested inclusive-gateway topology",
                        node.id()
                    )
                })?;
                lower_inclusive_diverging_v2(
                    graph,
                    node_idx,
                    base,
                    join_addr,
                    &node_addr,
                    &mut flag_intern,
                    &mut instructions,
                );
            }
            IRNode::GatewayInclusive {
                direction: GatewayDirection::Converging,
                ..
            } => {
                let pairing = inclusive_fork_addr.get(&node_idx).copied().ok_or_else(|| {
                    anyhow!(
                        "GatewayInclusive '{}' (converging) has no matching diverging \
                         GatewayInclusive — non-well-nested inclusive-gateway topology",
                        node.id()
                    )
                })?;
                instructions.push(Instr::V2Join { pairing });
                // `V2Join` carries no `next` field (continuation is PC+1 on
                // last arrival, per K-3) — same trailing-`Jump` pattern as
                // `GatewayAnd`'s already-landed v2 converging arm.
                let successors = get_successors(graph, node_idx);
                let next = successors
                    .first()
                    .and_then(|s| node_addr.get(s).copied())
                    .unwrap_or(Addr::new(0));
                instructions.push(Instr::Jump { target: next });
            }

            IRNode::TimerWait { spec, .. } => {
                // `V2WaitFor`/`V2WaitUntil` pop their duration/deadline from
                // the operand stack (V2.7 addressing-review BLOCKING #2.4)
                // rather than carrying it as a static field, so an explicit
                // `PushI64` precedes them — the established pattern (see
                // `bpmn-lite-kernel`'s own golden fixtures).
                match spec {
                    TimerSpec::Duration { ms } => {
                        instructions.push(Instr::PushI64(*ms as i64));
                        instructions.push(Instr::V2WaitFor);
                    }
                    TimerSpec::Date { deadline_ms } => {
                        instructions.push(Instr::PushI64(*deadline_ms as i64));
                        instructions.push(Instr::V2WaitUntil);
                    }
                    TimerSpec::Cycle { interval_ms, .. } => {
                        // Standalone timer cycle treated as single wait for first interval
                        instructions.push(Instr::PushI64(*interval_ms as i64));
                        instructions.push(Instr::V2WaitFor);
                    }
                }

                // `V2WaitFor`/`V2WaitUntil` carry no `next` field (kernel
                // continuation is PC+1 on resume) — an explicit trailing
                // `Jump` supplies what v1's embedded field used to.
                let successors = get_successors(graph, node_idx);
                if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(Addr::new(0));
                    instructions.push(Instr::Jump { target });
                }
            }

            IRNode::MessageWait {
                name: msg_name,
                corr_key_source,
                ..
            } => {
                // V5.3 (§18, landed 2026-07-23): migrated from v1 `WaitMsg`
                // to `V2WaitMsg` — the last standing-alone v1 word this
                // frontend still emitted (`TimerWait`/`GatewayAnd` already
                // switched over in V5.1/5.2). `V2WaitMsg { name, corr_reg }`
                // carries no `wait_id` (v1's `CancelWait` bookkeeping
                // identifier, inert in resolution — see the kernel's own
                // `V2WaitMsg` handler comment) and needs no `wait_plan`
                // entry (a static side table V-9 forbids surviving into a
                // v2-bearing artifact, same as `V2Fork`/`V2Join`'s already-
                // dropped `join_id`/`JoinPlanEntry`).
                let name_id = intern_flag(&mut flag_intern, msg_name);
                message_name_map.insert(name_id, msg_name.clone());
                let corr_reg = parse_corr_reg(corr_key_source);

                instructions.push(Instr::V2WaitMsg {
                    name: name_id,
                    corr_reg,
                });

                let successors = get_successors(graph, node_idx);
                if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(Addr::new(0));
                    instructions.push(Instr::Jump { target });
                }
            }

            IRNode::SendTask {
                message_name,
                corr_key_source,
                ..
            } => {
                let name_id = intern_flag(&mut flag_intern, message_name);
                message_name_map.insert(name_id, message_name.clone());
                let corr_reg = parse_corr_reg(corr_key_source);

                instructions.push(Instr::PublishMessage {
                    name: name_id,
                    corr_reg,
                });

                let successors = get_successors(graph, node_idx);
                if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(Addr::new(0));
                    instructions.push(Instr::Jump { target });
                }
            }

            IRNode::HumanWait {
                name: msg_name,
                corr_key_source,
                ..
            } => {
                // V5.3: same `V2WaitMsg` migration as `MessageWait` above —
                // v1 distinguished `WaitType::Msg`/`WaitType::Human` only
                // via the now-deleted `wait_plan` side table; `V2WaitMsg`
                // draws no such distinction at the bytecode level (both
                // are "park on a named correlated signal"), matching how
                // v1's own kernel resolution (`apply_message`) never
                // branched on `WaitType` either — it was descriptive
                // metadata, not behavior-affecting.
                let name_id = intern_flag(&mut flag_intern, msg_name);
                message_name_map.insert(name_id, msg_name.clone());
                let corr_reg = parse_corr_reg(corr_key_source);

                instructions.push(Instr::V2WaitMsg {
                    name: name_id,
                    corr_reg,
                });

                let successors = get_successors(graph, node_idx);
                if let Some(next) = successors.first() {
                    let target = node_addr.get(next).copied().unwrap_or(Addr::new(0));
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
                // Compile input/output bindings — identical for both
                // `LoweringTarget`s; only the surrounding instruction
                // sequence (guard-wrapped or not) differs below.
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
                let successors = get_successors(graph, node_idx);

                // `exec_addr` is wherever `ExecFfi` actually lands, NOT
                // necessarily this node's own `base` — under
                // `LoweringTarget::V2` with a boundary timer, guard-open +
                // arm-timer instructions precede it (V5, ruling I).
                let exec_addr;

                if let Some((bt_node_idx, spec)) = boundary_lookup.get(id) {
                    let interrupting = matches!(
                        &graph[*bt_node_idx],
                        IRNode::BoundaryTimer {
                            interrupting: true,
                            ..
                        }
                    );
                    let escalation_addr = get_successors(graph, *bt_node_idx)
                        .first()
                        .and_then(|s| node_addr.get(s).copied())
                        .ok_or_else(|| {
                            anyhow!(
                                "boundary timer on '{id}' has no outgoing flow — its \
                                 handler target cannot be resolved"
                            )
                        })?;
                    // Same adjacency requirement as the `ServiceTask`
                    // arm (`lower_boundary_guarded_task_v2`'s doc
                    // comment) — `duration` pushed BEFORE guard-open.
                    instructions.push(Instr::PushI64(timer_spec_duration_ms(spec) as i64));
                    if interrupting {
                        instructions.push(Instr::V2Guard {
                            handler: escalation_addr,
                        });
                    } else {
                        instructions.push(Instr::V2GuardN {
                            handler: escalation_addr,
                        });
                    }
                    instructions.push(Instr::V2GuardArmTimer);
                    exec_addr = Addr::new(instructions.len() as u32);
                    instructions.push(Instr::ExecFfi {
                        template_id: *template_id,
                        argc,
                        retc,
                    });
                    instructions.push(if interrupting {
                        Instr::V2GuardEnd
                    } else {
                        Instr::V2GuardNEnd
                    });
                    let target_addr = successors
                        .first()
                        .and_then(|s| node_addr.get(s).copied())
                        .unwrap_or(base + 6u32);
                    instructions.push(Instr::Jump { target: target_addr });
                } else {
                    exec_addr = Addr::new(instructions.len() as u32);
                    instructions.push(Instr::ExecFfi {
                        template_id: *template_id,
                        argc,
                        retc,
                    });
                    let target_addr = successors
                        .first()
                        .and_then(|s| node_addr.get(s).copied())
                        .unwrap_or(base + 2u32);
                    instructions.push(Instr::Jump { target: target_addr });
                }

                // Register FfiTaskDecl — same for both targets, keyed by
                // wherever `ExecFfi` actually landed.
                ffi_task_decls.insert(
                    exec_addr,
                    bpmn_lite_types::ffi_bindings::FfiTaskDecl {
                        template_id: *template_id,
                        inputs: compiled_inputs,
                        outputs: compiled_outputs,
                    },
                );
            }

            // §18 ruling K: v2-only. See `lower_multi_instance_v2`'s doc
            // comment for the emitted shape.
            IRNode::MultiInstance {
                id,
                task_type,
                collection_flag_name,
                declared_max,
                ..
            } => {
                lower_multi_instance_v2(
                    graph,
                    node_idx,
                    base,
                    id,
                    task_type,
                    collection_flag_name,
                    *declared_max,
                    &node_addr,
                    &mut task_intern,
                    &mut task_manifest,
                    &mut flag_intern,
                    &mut instructions,
                )?;
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

    Ok(bpmn_lite_types::legacy_program! {
        bytecode_version: bytecode_version,
        program: instructions,
        debug_map: debug_map,
        join_plan: join_plan,
        wait_plan: wait_plan,
        message_name_map: message_name_map,
        write_set: write_set,
        task_manifest: task_manifest,
        error_route_map: error_route_map,
        flag_symbol_table: flag_symbol_table,
        data_objects: data_objects,
        ffi_task_decls: ffi_task_decls,
    })
}

/// V5.3 (§18, landed 2026-07-23): thin alias, retained for the existing
/// call sites that named `lower_v2`/`Compiler::lower_v2` explicitly
/// (`bpmn-lite-engine/src/tests.rs`'s `t_ig_v2_*`/`t_boundary_timer_v2_*`/
/// `t_mi_v2_*` fixtures, this module's own `_v2`-suffixed tests) rather
/// than deleting the name and touching every one of them — `lower()` is
/// no longer a distinct v1 path this aliases *around*, it's the same
/// function. Kept, not removed, per Part A's explicit "your call" —
/// removing it would be a purely mechanical rename across ~18 call sites
/// for zero behavioural gain.
pub fn lower_v2(graph: &IRGraph) -> Result<CompiledProgram> {
    lower(graph)
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

fn estimate_instr_count(graph: &IRGraph, node: NodeIndex) -> u32 {
    match &graph[node] {
        IRNode::Start { .. } => 1,
        IRNode::End { .. } => 1,
        IRNode::ServiceTask { .. } => 2, // ExecNative + Jump
        IRNode::GatewayXor { .. } => {
            let outgoing = graph
                .edges_directed(node, petgraph::Direction::Outgoing)
                .count();
            // Each conditional edge: LoadFlag + BrIf, plus default Jump
            (outgoing as u32 * 2).max(1) + 1
        }
        IRNode::GatewayAnd {
            direction: GatewayDirection::Diverging,
            ..
        } => 1, // V2Fork
        IRNode::GatewayAnd {
            direction: GatewayDirection::Converging,
            ..
        } => 2, // V2Join + Jump (V2Join carries no `next` field)
        IRNode::GatewayInclusive { .. } => 1, // v1-shaped estimator; superseded per-direction by instr_count_for
        IRNode::TimerWait { .. } => 3,         // PushI64 + V2WaitFor/V2WaitUntil + Jump
        IRNode::MessageWait { .. } => 2,      // V2WaitMsg + Jump
        IRNode::HumanWait { .. } => 2,        // V2WaitMsg + Jump
        IRNode::BoundaryTimer { .. } => 0,    // structural only — no bytecode emitted
        IRNode::BoundaryError { .. } => 0,    // structural only — no bytecode emitted
        IRNode::DataObject { .. } => 0,       // structural only — no bytecode emitted
        IRNode::FfiServiceTask { .. } => 2,   // ExecFfi + Jump
        IRNode::SendTask { .. } => 2,         // PublishMessage + Jump
        // `IRNode::MultiInstance` is v2-only (§18 ruling K) — `lower()`
        // (v1) rejects any graph containing one before instruction
        // emission reaches it (see the `IRNode::MultiInstance` v1 catch
        // arm in the emission match below); this arm exists only to keep
        // this function's match exhaustive, per the "no wildcard" V-9/A18
        // discipline this codebase already follows elsewhere. Never
        // actually consulted for address assignment in a program that
        // will successfully compile.
        IRNode::MultiInstance { .. } => 0,
    }
}

// ── V5 (§18 rulings H/I) — `LoweringTarget::V2`-only sizing ──────────────

/// One outgoing edge of a diverging `GatewayInclusive`, classified for v2
/// lowering purposes. `always_live` mirrors the v1 `InclusiveBranch`
/// convention ("no condition = always taken") — XML's `IREdge` has no
/// separate default-flow annotation distinct from "no condition," so this
/// single field is both "unconditional branch" and XML's only expressible
/// notion of a default (see `lower_inclusive_diverging_v2`'s doc comment).
struct InclusiveBranchInfo {
    always_live: bool,
}

/// Precompute every diverging `GatewayInclusive`'s branch classification,
/// keyed by its own `NodeIndex`, ahead of both the sizing pass and the
/// emission pass — computed unconditionally (cheap; only inclusive-gateway
/// nodes produce an entry) so `LoweringTarget::V1`'s codepath is
/// unaffected by this block merely existing.
fn collect_inclusive_branches(
    graph: &IRGraph,
    order: &[NodeIndex],
) -> HashMap<NodeIndex, Vec<InclusiveBranchInfo>> {
    let mut result = HashMap::new();
    for &node_idx in order {
        if let IRNode::GatewayInclusive {
            direction: GatewayDirection::Diverging,
            ..
        } = &graph[node_idx]
        {
            let branches = graph
                .edges_directed(node_idx, petgraph::Direction::Outgoing)
                .map(|edge| InclusiveBranchInfo {
                    always_live: edge.weight().condition.is_none(),
                })
                .collect();
            result.insert(node_idx, branches);
        }
    }
    result
}

/// A diverging `GatewayInclusive`'s v2 instruction count (§18 ruling H):
/// the zero-match precheck (omitted entirely when an always-live branch
/// exists — its presence makes the fork's target set provably non-empty,
/// so there is nothing for the precheck to guard against; see
/// `lower_inclusive_diverging_v2`), the `V2Fork` itself, and one
/// per-branch header. A conditional branch's header costs three
/// instructions (`LoadFlag`/`V2LoadPlaceholderMatch` + `BrIfNot` +
/// `Jump`) since it must re-check its own condition to decide
/// skip-vs-real-work per the dynamic-arity pattern; an always-live
/// branch's header costs one (a bare `Jump`), since it never skips.
fn inclusive_diverging_instr_count(branches: &[InclusiveBranchInfo]) -> u32 {
    let fork = 1;
    let headers: u32 = branches
        .iter()
        .map(|branch| if branch.always_live { 1 } else { 3 })
        .sum();
    inclusive_precheck_len(branches) + fork + headers
}

/// The zero-match precheck's own instruction count (§18 ruling J) — 0 when
/// an always-live branch exists (the precheck can never fire, so it is
/// omitted, not merely skippable), otherwise 2 per conditional branch
/// (`LoadFlag`/`V2LoadPlaceholderMatch` + `BrIf`) plus 1 for the trailing
/// `V2RouteZeroMatch`. Shared by both the sizing pass
/// (`inclusive_diverging_instr_count`) and the pre-pass that resolves
/// `V2Fork`'s own address ahead of emission (`inclusive_fork_addr`,
/// `lower_with_target`) — both need the identical formula, computed once.
fn inclusive_precheck_len(branches: &[InclusiveBranchInfo]) -> u32 {
    let has_always_live = branches.iter().any(|branch| branch.always_live);
    if has_always_live {
        0
    } else {
        let conditional_count = branches.iter().filter(|branch| !branch.always_live).count() as u32;
        conditional_count.saturating_mul(2) + 1
    }
}

/// `LoweringTarget`-aware instruction-count dispatch. Delegates to
/// `estimate_instr_count` (v1, unchanged) for every node kind except the
/// two `LoweringTarget::V2` changes this step makes: a boundary-timer host
/// task (guard-wrapped: `V2Guard`/`V2GuardN` + `PushI64` +
/// `V2GuardArmTimer` + the task's own instructions, unchanged count +
/// `V2GuardEnd`/`V2GuardNEnd` — 4 more than the v1 `ExecNative`/`ExecFfi` +
/// `Jump` pair) and a `GatewayInclusive` (both directions — see
/// `inclusive_diverging_instr_count` and the `V2Join`+`Jump` converging
/// count, matching `GatewayAnd`'s already-landed v2 converging shape).
fn instr_count_for(
    graph: &IRGraph,
    node_idx: NodeIndex,
    boundary_lookup: &HashMap<String, (NodeIndex, TimerSpec)>,
    inclusive_branches: &HashMap<NodeIndex, Vec<InclusiveBranchInfo>>,
    guardn_close_before_end: &HashSet<NodeIndex>,
) -> u32 {
    match &graph[node_idx] {
        IRNode::ServiceTask { id, .. } | IRNode::FfiServiceTask { id, .. } => {
            let base = estimate_instr_count(graph, node_idx); // ExecNative/ExecFfi + Jump
            if boundary_lookup.contains_key(id) {
                base + 4 // guard-open + PushI64 + V2GuardArmTimer + guard-end
            } else {
                base
            }
        }
        IRNode::GatewayInclusive {
            direction: GatewayDirection::Diverging,
            ..
        } => inclusive_diverging_instr_count(
            inclusive_branches
                .get(&node_idx)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        ),
        IRNode::GatewayInclusive {
            direction: GatewayDirection::Converging,
            ..
        } => 2, // V2Join + Jump — V2Join carries no `next` field
        // §18 ruling K: `V2MiArityCheck`(1) + `V2Fork`(1) + `declared_max`
        // synthesized branch headers, each `V2MiIndexLive`+`BrIfNot`+
        // `V2MiLoadElement`+`StoreFlag`+`ExecNative`+`Jump` (6 — grew from
        // 4 in ruling K Part 2, which added the middle two for per-element
        // value delivery) + `V2Join`(1) + trailing `Jump`(1). Fully
        // self-contained within this one node's block — unlike
        // `GatewayInclusive`'s diverging/converging node PAIR (whose
        // branches point at already-existing downstream graph nodes), MI
        // has exactly one incoming and one outgoing sequence flow and
        // synthesizes its own `declared_max` branches internally, so no
        // separate pairing-stack bookkeeping across graph nodes is needed.
        IRNode::MultiInstance { declared_max, .. } => declared_max.saturating_mul(6) + 4,
        IRNode::End { .. } if guardn_close_before_end.contains(&node_idx) => {
            // +1 for the `V2GuardNEnd` a non-interrupting boundary timer's
            // escalation terminal must emit before its own `End`/
            // `EndTerminate` — see the `guardn_close_before_end` pre-pass
            // doc comment above.
            estimate_instr_count(graph, node_idx) + 1
        }
        _ => estimate_instr_count(graph, node_idx),
    }
}

/// A `TimerSpec` to a single relative-duration milliseconds value, for
/// words with only a `WAIT-FOR`-shaped ( duration -- ) operand and no
/// `WAIT-UNTIL`-shaped absolute-deadline counterpart — `GUARD-TIMER>`
/// (ruling I) is exactly this: one opcode, not two, unlike `TimerWait`'s
/// v2 lowering (`V2WaitFor`/`V2WaitUntil`, both present). `Date` converts
/// to an equivalent relative duration at compile time (the same
/// `now()`-relative conversion the v1 `FfiServiceTask` boundary-timer arm
/// already performed for its `WaitArm::Timer` construction, reused
/// verbatim rather than re-derived). `Cycle` is treated as its first
/// interval only — the same simplification `IRNode::TimerWait`'s v2
/// lowering already makes ("standalone timer cycle treated as single wait
/// for first interval") — **a known, recorded gap, not silently
/// papered over**: v1's boundary-timer `Cycle` re-fires up to
/// `max_fires` times via `WaitArm::Timer{cycle}`; `GUARD-N>` (ruling G/I)
/// re-arms unconditionally on every normal completion of its guarded
/// body, which has no notion of a fire-count ceiling at all. A workflow
/// depending on `Cycle`'s bounded re-fire count is not faithfully
/// representable by `GUARD-TIMER>` as ratified — flagged here rather than
/// silently narrowed, since deciding what (if anything) replaces it is a
/// design question this step doesn't own.
fn timer_spec_duration_ms(spec: &TimerSpec) -> u64 {
    match spec {
        TimerSpec::Duration { ms } => *ms,
        TimerSpec::Date { deadline_ms } => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            deadline_ms.saturating_sub(now)
        }
        TimerSpec::Cycle { interval_ms, .. } => *interval_ms,
    }
}

/// V5 (§18 ruling I, `LoweringTarget::V2` only): lower a `ServiceTask` that
/// may host a boundary timer. Without one, identical to the v1 arm minus
/// the (never-populated, for this path) `race_plan`/`boundary_map` side
/// effects: `ExecNative` + `Jump`. With one, the guard-wrapped shape ruling
/// I ratifies: `V2Guard`/`V2GuardN` (interrupting per the BPMN boundary
/// event's own flag) immediately followed by `GUARD-TIMER>`'s arming pair
/// (`PushI64` + `V2GuardArmTimer`, verifier-enforced adjacency), then the
/// task's own work, then the matching guard-close, then the normal-path
/// `Jump`. The guard's `handler` is the boundary event's own outgoing
/// flow's target address — "that outgoing flow IS the handler target"
/// (this step's own brief) — resolved exactly as the v1 arm already
/// resolves `escalation_addr`, reused verbatim.
#[allow(clippy::too_many_arguments)]
fn lower_boundary_guarded_task_v2(
    graph: &IRGraph,
    node_idx: NodeIndex,
    id: &str,
    task_type: &str,
    base: Addr,
    node_addr: &HashMap<NodeIndex, Addr>,
    boundary_lookup: &HashMap<String, (NodeIndex, TimerSpec)>,
    task_intern: &mut HashMap<String, u32>,
    task_manifest: &mut Vec<String>,
    instructions: &mut Vec<Instr>,
) -> Result<()> {
    let task_id = intern_task(task_intern, task_manifest, task_type);
    let successors = get_successors(graph, node_idx);
    let boundary = boundary_lookup.get(id);

    let Some((bt_node_idx, spec)) = boundary else {
        // No boundary timer: identical shape to v1's unguarded case.
        instructions.push(Instr::ExecNative {
            task_type: task_id,
            argc: 0,
            retc: 0,
        });
        let target = successors
            .first()
            .and_then(|s| node_addr.get(s).copied())
            .unwrap_or(base + 2u32);
        instructions.push(Instr::Jump { target });
        return Ok(());
    };

    let interrupting = matches!(
        &graph[*bt_node_idx],
        IRNode::BoundaryTimer {
            interrupting: true,
            ..
        }
    );
    let escalation_addr = get_successors(graph, *bt_node_idx)
        .first()
        .and_then(|s| node_addr.get(s).copied())
        .ok_or_else(|| {
            anyhow!(
                "boundary timer on '{id}' has no outgoing flow — its handler \
                 target cannot be resolved"
            )
        })?;

    // `GUARD-TIMER>` must land at exactly `guard_open_addr + 1`
    // (verifier-enforced adjacency, `v2_verifier::verify_v2_control_stack`)
    // — so its own operand (`duration`) must be pushed BEFORE the guard
    // opens, not between guard-open and arm (`V2Guard`/`V2GuardN` touch
    // neither stack, so the pushed value survives across them untouched;
    // see `bpmn-lite-kernel`'s own `v2_guard_timer_trigger_fires_the_same_
    // cascade_as_manual_v2_trigger_guard` fixture, `PushI64` at index 0,
    // guard-open at index 1, `V2GuardArmTimer` at index 2).
    instructions.push(Instr::PushI64(timer_spec_duration_ms(spec) as i64));
    if interrupting {
        instructions.push(Instr::V2Guard {
            handler: escalation_addr,
        });
    } else {
        instructions.push(Instr::V2GuardN {
            handler: escalation_addr,
        });
    }
    instructions.push(Instr::V2GuardArmTimer);
    instructions.push(Instr::ExecNative {
        task_type: task_id,
        argc: 0,
        retc: 0,
    });
    instructions.push(if interrupting {
        Instr::V2GuardEnd
    } else {
        Instr::V2GuardNEnd
    });
    let target = successors
        .first()
        .and_then(|s| node_addr.get(s).copied())
        .unwrap_or(base + 6u32);
    instructions.push(Instr::Jump { target });
    Ok(())
}

/// V5 (§18 ruling H, `LoweringTarget::V2` only): lower a diverging
/// `GatewayInclusive` to `V2Fork` using the dynamic-arity skip-to-join
/// pattern the kernel already proves sound for a fixed-shape `V2Fork`
/// (`v2fork_mixed_real_work_and_skip_to_join_branches_retires_barrier_via_unmodified_mechanism`,
/// `bpmn-lite-kernel`). `V2Fork` has no condition-evaluation or zero-match
/// capability of its own — it is a dumb "always spawn N static targets"
/// instruction, by design (keeping it that way is what keeps K-3/V-3
/// unchanged, ruling H) — so all of that lives in straight-line code
/// around it instead:
///
/// ```text
/// [precheck, OMITTED if any branch is always-live]
///   LoadFlag(f1); BrIf(fork_addr)      -- per conditional branch, short-
///   ...                                   circuit OR: any true skips
///   LoadFlag(fk); BrIf(fork_addr)         straight to the fork
///   V2RouteZeroMatch                   -- unreached if any branch matched;
///                                          raises an Incident (ruling J)
/// fork_addr:
///   V2Fork { targets: [h1..hn], pairing: fork_addr }
/// h_i (conditional):  LoadFlag(fi); BrIfNot(join_addr); Jump(ti)
/// h_i (always-live):  Jump(ti)
/// ```
///
/// **Zero-match precheck, and why it runs BEFORE `V2Fork` rather than as
/// one of `V2Fork`'s own operands or a post-fork check:** `V2Fork` commits
/// to spawning `targets.len()` fibres unconditionally the moment it
/// executes — there is no "spawn 0" outcome to recover from afterward, so
/// the zero-match decision (raise an Incident, never fork at all) must be
/// made on the single forking fibre BEFORE `V2Fork` runs, in ordinary
/// straight-line code. This is the design decision the brief asked to be
/// resolved and documented, not assumed: the alternative (evaluate inside
/// `V2Fork` itself) would require `V2Fork` to gain condition-evaluation
/// and zero-match capability it deliberately does not have (ruling H's
/// entire point is that `V2Fork` stays a dumb fixed-arity spawn); a
/// post-fork check is incoherent, since by "post-fork" `n` fibres already
/// exist. Two new opcodes make the precheck expressible without touching
/// `V2Fork`: `V2RouteZeroMatch` (the incident-raise itself, reusing
/// `fail_contract` — investigated first, per the brief: `fail_contract` is
/// a private `fn` inside `bpmn-lite-kernel`, reachable only from within an
/// `Instr` match arm in `apply`'s own dispatch loop, so it cannot be
/// "called from straight-line code" without a new opcode wrapping it —
/// there is no existing general-purpose "assert or incident" primitive to
/// reuse) and `V2LoadPlaceholderMatch` (the DSL-side condition-evaluation
/// gap this same design surfaced — see its own doc comment).
///
/// **Precheck omission when an always-live branch exists.** An
/// unconditional edge (XML: no `condition` on the `IREdge` — the same
/// convention v1's `InclusiveBranch::condition_flag: None` already used)
/// or a DSL default flow (`SplitMode::Inclusive`'s `routes()`-derived
/// `default_target`) makes the fork's target set provably non-empty by
/// construction — there is no runtime state under which zero branches are
/// live, so emitting a precheck that can never actually fire would be
/// dead code, not defensive coding. XML has no separate "default flow"
/// annotation distinct from "no condition" (`IREdge` carries the same
/// `Option<ConditionExpr>` an XOR gateway's default edge uses) — so
/// always-live/unconditional and "default" are, for this IR, the same
/// concept and the same code path; this is XML's complete answer to the
/// brief's "default branch handling" requirement, not a partial one.
fn lower_inclusive_diverging_v2(
    graph: &IRGraph,
    node_idx: NodeIndex,
    base: Addr,
    join_addr: Addr,
    node_addr: &HashMap<NodeIndex, Addr>,
    flag_intern: &mut HashMap<String, FlagKey>,
    instructions: &mut Vec<Instr>,
) {
    struct Branch {
        always_live: bool,
        flag_name: Option<String>,
        target_idx: NodeIndex,
    }

    let branches: Vec<Branch> = graph
        .edges_directed(node_idx, petgraph::Direction::Outgoing)
        .map(|edge| Branch {
            always_live: edge.weight().condition.is_none(),
            flag_name: edge.weight().condition.as_ref().map(|c| c.flag_name.clone()),
            target_idx: edge.target(),
        })
        .collect();

    let has_always_live = branches.iter().any(|branch| branch.always_live);
    let conditional_count = branches.iter().filter(|branch| !branch.always_live).count() as u32;

    let fork_addr = if has_always_live {
        base
    } else {
        base + conditional_count.saturating_mul(2) + 1
    };

    if !has_always_live {
        for branch in &branches {
            let flag_name = branch
                .flag_name
                .as_ref()
                .expect("non-always-live branch always carries a condition");
            let key = intern_flag(flag_intern, flag_name);
            instructions.push(Instr::LoadFlag { key });
            instructions.push(Instr::BrIf { target: fork_addr });
        }
        instructions.push(Instr::V2RouteZeroMatch);
    }

    debug_assert_eq!(Addr::new(instructions.len() as u32), fork_addr);

    let mut header_addr = fork_addr + 1u32;
    let mut headers: Vec<Addr> = Vec::with_capacity(branches.len());
    for branch in &branches {
        headers.push(header_addr);
        header_addr += if branch.always_live { 1u32 } else { 3u32 };
    }

    instructions.push(Instr::V2Fork {
        targets: headers.into_boxed_slice(),
        pairing: fork_addr,
    });

    for branch in &branches {
        let real_target = node_addr
            .get(&branch.target_idx)
            .copied()
            .unwrap_or(Addr::new(0));
        if branch.always_live {
            instructions.push(Instr::Jump { target: real_target });
        } else {
            let flag_name = branch
                .flag_name
                .as_ref()
                .expect("non-always-live branch always carries a condition");
            let key = intern_flag(flag_intern, flag_name);
            instructions.push(Instr::LoadFlag { key });
            instructions.push(Instr::BrIfNot { target: join_addr });
            instructions.push(Instr::Jump { target: real_target });
        }
    }
}

/// §18 ruling K: lower a parallel multi-instance activity to `V2Fork`
/// sized to its `declared_max`, using the SAME skip-to-`V2Join` mechanism
/// ruling H's dynamic-arity gateway lowering already proves sound
/// (`lower_inclusive_diverging_v2`'s doc comment) — applied to an index
/// bound instead of a per-branch flag condition.
///
/// **Revised for ruling K Part 2 (per-element value access, landed
/// 2026-07-23).** Each live branch now also delivers its own element's
/// `Value` to the inner activity's job, via `V2MiLoadElement` +
/// `StoreFlag` immediately before `ExecNative`:
///
/// ```text
/// base:
///   V2MiArityCheck { collection_flag, max: declared_max }  -- hard reject
///                                                              if actual > declared_max
///   V2Fork { targets: [h_0..h_{n-1}], pairing: base+1 }
/// h_i:  V2MiIndexLive { collection_flag, index: i }
///       BrIfNot(join_addr)
///       V2MiLoadElement { collection_flag, index: i }  -- ( -- value )
///       StoreFlag { key: element_flag_i }              -- ( value -- )
///       ExecNative { task_type }             -- the inner activity's own work
///       Jump(join_addr)
/// join_addr:
///   V2Join { pairing: base+1 }
///   Jump(next)
/// ```
///
/// **Why `V2MiLoadElement` + the pre-existing `StoreFlag`, not a change to
/// `ExecNative` itself — a real finding, checked before writing this,
/// exactly per the landing brief's own instruction not to assume.**
/// `ExecNative`'s `argc` field (`Instr::ExecNative { task_type, argc,
/// retc }`) looks, from its name and its `stack_effect` table entry
/// (`(argc, retc)`, `artifact.rs`), like the natural place to deliver an
/// operand-stack value into the job. It is not: `argc` is set to `0` at
/// **every** call site in this codebase (grep-verified — service tasks,
/// FFI tasks, MI) and the kernel's `ExecNative` handler
/// (`bpmn-lite-kernel/src/lib.rs`) destructures `{ task_type, retc, .. }`
/// — it never reads `argc` and never pops any operand off `fiber.stack`
/// before dispatching the job. The field is vestigial today; wiring MI
/// element delivery through it would mean either (a) teaching `ExecNative`
/// to actually pop and forward `argc` operands — a change to a mechanism
/// every service task in the codebase shares, well outside this landing's
/// scope, or (b) pushing a value `ExecNative` silently never consumes,
/// leaving it dead on the fibre's own operand stack forever (a real,
/// if harmless-to-K-invariants, leak — and pointless, since nothing
/// downstream could read it either). Neither is the by-value delivery
/// mechanism this landing needs.
///
/// What the external job actually receives is `orch_flags` — a snapshot
/// of `instance.flags` at the moment `ExecNative` runs (`bpmn-lite-kernel`,
/// `orch_flags: instance.flags.iter().map(...).collect()`), the SAME
/// pipeline every existing service task's inputs already flow through.
/// `V2MiLoadElement` (pushes the element `Value` — the existing MI-side
/// analogue of `LoadFlag`) immediately followed by the pre-existing
/// `StoreFlag` (pops it into a synthesized per-branch flag,
/// `<node_id>_mi_element_<i>`) composes two already-proven primitives to
/// route the value through that same existing pipeline, rather than
/// inventing a new one. Per-branch-UNIQUE flag keys (not one shared
/// scratch key reused across branches) were the deliberate choice here:
/// a shared key would still be race-free in practice (one fibre's
/// `StoreFlag`+`ExecNative` pair executes atomically within a single
/// `apply()` transition, and `JobActivation.orch_flags` is a materialized
/// clone at enqueue time, not a live reference — so even a reused key
/// cannot leak one branch's element into another's already-enqueued job),
/// but reasoning about that correctly requires understanding the tick
/// model's atomicity; a unique key per branch needs no such argument at
/// all. The extra `declared_max` interned flag keys are cheap.
///
/// **Unlike `GatewayInclusive`'s diverging/converging node pair**, whose
/// branches are edges to already-existing downstream graph nodes needing a
/// cross-node pairing stack, MI is a SINGLE `IRNode` with exactly one
/// incoming and one outgoing sequence flow: all `declared_max` branches are
/// synthesized bytecode internal to this one node's own instruction block,
/// self-contained, no pairing stack needed.
///
/// **Empty collection falls out of the mechanism with zero special-casing**
/// (ruling K item (c), and the design hypothesis this ruling asked to be
/// verified before building — confirmed): if the runtime length is 0, every
/// `V2MiIndexLive` check is false, every branch `BrIfNot`s straight to
/// `V2Join`, the barrier still retires normally (all `n` "arrived," just
/// instantly), and execution continues past the join — no incident, no
/// zero-match precheck, nothing borrowed from the gateway case. This is
/// exactly why `V2MiArityCheck` is a hard reject rather than an `Incident`-
/// raising precheck: MI's `V2Fork` has no "don't fork at all" precondition
/// the way a gateway's true zero-match does (see ruling K's own text) — it
/// always forks, so there is nothing here that plays the zero-match
/// precheck's role at all.
#[allow(clippy::too_many_arguments)]
fn lower_multi_instance_v2(
    graph: &IRGraph,
    node_idx: NodeIndex,
    base: Addr,
    node_id: &str,
    task_type: &str,
    collection_flag_name: &str,
    declared_max: u32,
    node_addr: &HashMap<NodeIndex, Addr>,
    task_intern: &mut HashMap<String, u32>,
    task_manifest: &mut Vec<String>,
    flag_intern: &mut HashMap<String, FlagKey>,
    instructions: &mut Vec<Instr>,
) -> Result<()> {
    let collection_flag = intern_flag(flag_intern, collection_flag_name);
    let task_id = intern_task(task_intern, task_manifest, task_type);

    let fork_addr = base + 1u32;
    // Each branch is now 6 instructions long: V2MiIndexLive, BrIfNot,
    // V2MiLoadElement, StoreFlag, ExecNative, Jump (was 4 before ruling K
    // Part 2 added the middle two).
    let join_addr = fork_addr + 1u32 + declared_max.saturating_mul(6);

    instructions.push(Instr::V2MiArityCheck {
        collection_flag,
        max: declared_max,
    });
    debug_assert_eq!(Addr::new(instructions.len() as u32), fork_addr);

    let mut header_addr = fork_addr + 1u32;
    let mut headers: Vec<Addr> = Vec::with_capacity(declared_max as usize);
    for _ in 0..declared_max {
        headers.push(header_addr);
        header_addr += 6u32;
    }

    instructions.push(Instr::V2Fork {
        targets: headers.into_boxed_slice(),
        pairing: fork_addr,
    });

    for index in 0..declared_max {
        let element_flag = intern_flag(flag_intern, &format!("{node_id}_mi_element_{index}"));
        instructions.push(Instr::V2MiIndexLive {
            collection_flag,
            index,
        });
        instructions.push(Instr::BrIfNot { target: join_addr });
        instructions.push(Instr::V2MiLoadElement {
            collection_flag,
            index,
        });
        instructions.push(Instr::StoreFlag { key: element_flag });
        instructions.push(Instr::ExecNative {
            task_type: task_id,
            argc: 0,
            retc: 0,
        });
        instructions.push(Instr::Jump { target: join_addr });
    }

    debug_assert_eq!(Addr::new(instructions.len() as u32), join_addr);
    instructions.push(Instr::V2Join { pairing: fork_addr });

    let successors = get_successors(graph, node_idx);
    let next = successors
        .first()
        .and_then(|s| node_addr.get(s).copied())
        .unwrap_or(Addr::new(0));
    instructions.push(Instr::Jump { target: next });

    Ok(())
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
        assert!(program.program().len() >= 3);
        assert_eq!(program.task_manifest(), &["create_case"]);

        // Last instruction should be End
        let last = program.program().last().unwrap();
        assert!(matches!(last, Instr::End));

        // Should have ExecNative somewhere
        assert!(program
            .program()
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
            .program()
            .iter()
            .any(|i| matches!(i, Instr::LoadFlag { .. })));
        assert!(program
            .program()
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

        let fork_pairing = program
            .program()
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

        let join_count = program
            .program()
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

        assert!(
            program
                .program()
                .iter()
                .any(|i| matches!(i, Instr::PushI64(5000))),
            "V2WaitFor's duration is popped from the operand stack, not embedded"
        );
        assert!(program
            .program()
            .iter()
            .any(|i| matches!(i, Instr::V2WaitFor)));
        // MessageWait lowers to V2WaitMsg (V5.3, §18, landed 2026-07-23 —
        // the v1 WaitMsg-kernel-continuation question this comment used to
        // flag was resolved earlier in V5's own post-close work, and this
        // step completed the migration: v1 `WaitMsg` is deleted entirely).
        assert!(program
            .program()
            .iter()
            .any(|i| matches!(i, Instr::V2WaitMsg { .. })));
        assert!(program
            .message_name_map()
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
        assert_ne!(program.bytecode_version(), [0u8; 32]);

        // Debug map should have entries
        assert!(!program.debug_map().is_empty());

        // Task manifest should list task types
        assert!(program.task_manifest().contains(&"create_case".to_string()));
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
            !program.flag_symbol_table().is_empty(),
            "flag_symbol_table should be non-empty when conditions reference named flags"
        );
        assert!(
            program
                .flag_symbol_table()
                .values()
                .any(|n| n == "approved"),
            "flag_symbol_table should contain the condition flag name 'approved'"
        );

        // The FlagKey stored in the table must be used as a LoadFlag operand.
        let table_key = *program
            .flag_symbol_table()
            .iter()
            .find(|(_, n)| *n == "approved")
            .unwrap()
            .0;
        assert!(
            program
                .program()
                .iter()
                .any(|i| matches!(i, Instr::LoadFlag { key } if *key == table_key)),
            "the FlagKey from flag_symbol_table must appear as a LoadFlag operand"
        );

        // Linear graph (no conditions) produces an empty symbol table.
        let linear = lower(&make_linear_graph()).unwrap();
        assert!(
            linear.flag_symbol_table().is_empty(),
            "flag_symbol_table should be empty when no conditions reference named flags"
        );
    }

    // ═══════════════════════════════════════════════════════════
    //  V5 post-close (§18 rulings H/I/J) — v2 boundary-timer and
    //  inclusive-gateway XML lowering (`lower_v2`, `LoweringTarget::V2`).
    // ═══════════════════════════════════════════════════════════

    fn make_boundary_timer_graph(interrupting: bool) -> IRGraph {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let host = graph.add_node(IRNode::ServiceTask {
            id: "host".to_string(),
            name: "Host".to_string(),
            task_type: "long_work".to_string(),
        });
        let normal_end = graph.add_node(IRNode::End {
            id: "normal_end".to_string(),
            terminate: false,
        });
        let boundary = graph.add_node(IRNode::BoundaryTimer {
            id: "timeout".to_string(),
            attached_to: "host".to_string(),
            spec: TimerSpec::Duration { ms: 5000 },
            interrupting,
        });
        let escalate = graph.add_node(IRNode::ServiceTask {
            id: "escalate".to_string(),
            name: "Escalate".to_string(),
            task_type: "escalate_work".to_string(),
        });
        let timeout_end = graph.add_node(IRNode::End {
            id: "timeout_end".to_string(),
            terminate: false,
        });

        graph.add_edge(start, host, IREdge { id: "f1".to_string(), condition: None });
        graph.add_edge(host, normal_end, IREdge { id: "f2".to_string(), condition: None });
        graph.add_edge(boundary, escalate, IREdge { id: "f3".to_string(), condition: None });
        graph.add_edge(escalate, timeout_end, IREdge { id: "f4".to_string(), condition: None });

        graph
    }

    /// V5 boundary-timer: interrupting case lowers to `V2Guard` +
    /// `GUARD-TIMER>` wrapping the host task, closed by `V2GuardEnd`.
    #[test]
    fn test_boundary_timer_lowering_v2_interrupting() {
        let graph = make_boundary_timer_graph(true);
        verifier::verify_or_err(&graph).unwrap();
        let program = lower_v2(&graph).unwrap();

        let instrs = program.program();
        assert!(
            instrs.iter().any(|i| matches!(i, Instr::V2Guard { .. })),
            "interrupting boundary timer must open V2Guard"
        );
        assert!(
            !instrs.iter().any(|i| matches!(i, Instr::V2GuardN { .. })),
            "interrupting boundary timer must not open V2GuardN"
        );
        assert!(
            instrs.iter().any(|i| matches!(i, Instr::V2GuardArmTimer)),
            "GUARD-TIMER> must arm the guard"
        );
        assert!(
            instrs.iter().any(|i| matches!(i, Instr::PushI64(5000))),
            "the timer duration (5000ms) must be pushed before GUARD-TIMER>"
        );
        assert!(
            instrs.iter().any(|i| matches!(i, Instr::V2GuardEnd)),
            "the guard must be closed on the normal-completion path"
        );
        assert!(
            instrs.iter().any(|i| matches!(i, Instr::ExecNative { .. })),
            "the host task's own work must still execute inside the guard"
        );

        // V-1..V-11 all pass — `Compiler::lower_v2` runs the full pipeline
        // (verify → lower → verify-bytecode → envelope), the same shape
        // `Compiler::lower` proves for the v1 path.
        crate::Compiler::lower_v2(&graph).expect("v2 boundary-timer lowering must verify");

        // V5.3 (§18, landed 2026-07-23): v1's `race_plan`/`boundary_map`
        // mechanism this assertion used to check for absence of is deleted
        // entirely — there is no field left on `CompiledProgram` to be
        // empty or populated, which is the stronger, type-level version
        // of the same guarantee this assertion used to prove at runtime.
    }

    /// V5 boundary-timer: non-interrupting case opens `V2GuardN`, closed by
    /// `V2GuardNEnd` — distinct opcodes, not a flag (mirrors `GUARD>`/
    /// `GUARD-N>`'s existing v2 encoding).
    #[test]
    fn test_boundary_timer_lowering_v2_non_interrupting() {
        let graph = make_boundary_timer_graph(false);
        verifier::verify_or_err(&graph).unwrap();
        let program = lower_v2(&graph).unwrap();

        let instrs = program.program();
        assert!(
            instrs.iter().any(|i| matches!(i, Instr::V2GuardN { .. })),
            "non-interrupting boundary timer must open V2GuardN"
        );
        assert!(
            !instrs.iter().any(|i| matches!(i, Instr::V2Guard { .. })),
            "non-interrupting boundary timer must not open V2Guard"
        );
        assert!(
            instrs.iter().any(|i| matches!(i, Instr::V2GuardNEnd)),
            "the guard must be closed on the normal-completion path"
        );
        assert!(instrs.iter().any(|i| matches!(i, Instr::V2GuardArmTimer)));

        crate::Compiler::lower_v2(&graph).expect("v2 boundary-timer lowering must verify");
    }

    /// A `ServiceTask` with no boundary timer lowers identically under
    /// `lower_v2` to plain `ExecNative` + `Jump` — no guard wrapping is
    /// introduced where none was asked for.
    #[test]
    fn test_service_task_without_boundary_timer_unaffected_by_v2() {
        let graph = make_linear_graph();
        let program = lower_v2(&graph).unwrap();
        assert!(!program.program().iter().any(|i| {
            matches!(
                i,
                Instr::V2Guard { .. } | Instr::V2GuardN { .. } | Instr::V2GuardArmTimer
            )
        }));
        assert!(program
            .program()
            .iter()
            .any(|i| matches!(i, Instr::ExecNative { .. })));
    }

    /// Build a diverging/converging `GatewayInclusive` pair with the given
    /// outgoing-edge shape from the diverging gateway: `(flag_name,
    /// task_type)` per conditional branch, plus `unconditional_task_type`
    /// for an optional always-live branch (`None` = pure-conditional
    /// gateway, exercising the zero-match precheck).
    fn make_inclusive_gateway_graph(
        conditional: &[(&str, &str)],
        unconditional_task_type: Option<&str>,
    ) -> IRGraph {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let fork = graph.add_node(IRNode::GatewayInclusive {
            id: "ig_fork".to_string(),
            name: "Fork".to_string(),
            direction: GatewayDirection::Diverging,
        });
        graph.add_edge(start, fork, IREdge { id: "f_start".to_string(), condition: None });

        let join = graph.add_node(IRNode::GatewayInclusive {
            id: "ig_join".to_string(),
            name: "Join".to_string(),
            direction: GatewayDirection::Converging,
        });
        let end = graph.add_node(IRNode::End { id: "end".to_string(), terminate: false });
        graph.add_edge(join, end, IREdge { id: "f_end".to_string(), condition: None });

        let mut flow_id = 0u32;
        let mut next_flow = || {
            flow_id += 1;
            format!("f{flow_id}")
        };

        if let Some(task_type) = unconditional_task_type {
            let task = graph.add_node(IRNode::ServiceTask {
                id: format!("{task_type}_node"),
                name: task_type.to_string(),
                task_type: task_type.to_string(),
            });
            graph.add_edge(fork, task, IREdge { id: next_flow(), condition: None });
            graph.add_edge(task, join, IREdge { id: next_flow(), condition: None });
        }

        for (flag_name, task_type) in conditional {
            let task = graph.add_node(IRNode::ServiceTask {
                id: format!("{task_type}_node"),
                name: task_type.to_string(),
                task_type: task_type.to_string(),
            });
            graph.add_edge(
                fork,
                task,
                IREdge {
                    id: next_flow(),
                    condition: Some(ConditionExpr {
                        flag_name: flag_name.to_string(),
                        op: ConditionOp::Eq,
                        literal: ConditionLiteral::Bool(true),
                    }),
                },
            );
            graph.add_edge(task, join, IREdge { id: next_flow(), condition: None });
        }

        graph
    }

    /// V5 inclusive gateway (a)/(b): pure-conditional gateway (no
    /// always-live branch) lowers to a zero-match precheck ahead of
    /// `V2Fork`, one skip-checking header per branch, and a shared
    /// `V2Join`.
    #[test]
    fn test_inclusive_gateway_lowering_v2_pure_conditional_shape() {
        let graph = make_inclusive_gateway_graph(
            &[("flag_a", "branch_a"), ("flag_b", "branch_b")],
            None,
        );
        verifier::verify_or_err(&graph).unwrap();
        let program = lower_v2(&graph).unwrap();
        let instrs = program.program();

        assert!(
            instrs.iter().any(|i| matches!(i, Instr::V2RouteZeroMatch)),
            "a pure-conditional gateway must emit the zero-match precheck"
        );
        let fork = instrs
            .iter()
            .find_map(|i| match i {
                Instr::V2Fork { targets, pairing } => Some((targets.clone(), *pairing)),
                _ => None,
            })
            .expect("a V2Fork must be emitted");
        assert_eq!(fork.0.len(), 2, "two conditional branches → two V2Fork targets");
        assert_eq!(
            instrs.iter().filter(|i| matches!(i, Instr::V2Join { pairing } if *pairing == fork.1)).count(),
            1,
            "exactly one V2Join referencing the V2Fork's pairing"
        );
        crate::Compiler::lower_v2(&graph).expect("v2 inclusive-gateway lowering must verify");
    }

    /// V5 inclusive gateway, default-branch handling: an always-live
    /// (unconditional) branch makes the fork's target set provably
    /// non-empty, so the zero-match precheck is omitted entirely — dead
    /// code would otherwise never fire.
    #[test]
    fn test_inclusive_gateway_lowering_v2_with_always_live_branch_omits_precheck() {
        let graph = make_inclusive_gateway_graph(&[("flag_a", "branch_a")], Some("always_task"));
        verifier::verify_or_err(&graph).unwrap();
        let program = lower_v2(&graph).unwrap();
        let instrs = program.program();

        assert!(
            !instrs.iter().any(|i| matches!(i, Instr::V2RouteZeroMatch)),
            "an always-live branch must make the zero-match precheck unreachable, \
             so it must not be emitted at all"
        );
        let fork = instrs
            .iter()
            .find_map(|i| match i {
                Instr::V2Fork { targets, .. } => Some(targets.len()),
                _ => None,
            })
            .expect("a V2Fork must be emitted");
        assert_eq!(fork, 2, "one conditional + one always-live branch → two targets");

        crate::Compiler::lower_v2(&graph).expect("v2 inclusive-gateway lowering must verify");
    }

    /// V5.3 (§18, landed 2026-07-23): relocked in place. `lower()`'s
    /// default flip (Part A) means `lower`/`lower_v2` are no longer
    /// independent entry points that could silently diverge — `lower_v2`
    /// is now a thin alias (`lowering::lower_v2`'s own doc comment). This
    /// test used to prove they stayed genuinely independent (locking
    /// `lower()`'s v1 `ForkInclusive`/`JoinDynamic` output against
    /// `lower_v2()`'s v2 output for the identical graph); it now proves
    /// the opposite, and equally load-bearing, property: the alias holds
    /// byte-for-byte, not just "produces an equivalent-looking program."
    #[test]
    fn test_lower_and_lower_v2_are_byte_identical_aliases() {
        let graph = make_inclusive_gateway_graph(
            &[("flag_a", "branch_a"), ("flag_b", "branch_b")],
            None,
        );
        let via_lower = lower(&graph).unwrap();
        let via_lower_v2 = lower_v2(&graph).unwrap();
        assert_eq!(
            via_lower.bytecode_version(),
            via_lower_v2.bytecode_version(),
            "lower() and lower_v2() must be byte-identical for the same graph"
        );
        assert!(via_lower
            .program()
            .iter()
            .any(|i| matches!(i, Instr::V2Fork { .. })));
    }

    // ═══════════════════════════════════════════════════════════
    //  §18 ruling K: multi-instance lowering tests
    // ═══════════════════════════════════════════════════════════

    fn make_multi_instance_graph(declared_max: u32) -> IRGraph {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let mi = graph.add_node(IRNode::MultiInstance {
            id: "verify_docs".to_string(),
            name: "Verify Doc".to_string(),
            task_type: "verify_doc".to_string(),
            collection_flag_name: "doc_count".to_string(),
            declared_max,
        });
        let end = graph.add_node(IRNode::End { id: "end".to_string(), terminate: false });
        graph.add_edge(start, mi, IREdge { id: "f1".to_string(), condition: None });
        graph.add_edge(mi, end, IREdge { id: "f2".to_string(), condition: None });
        graph
    }

    /// An MI activity with declared max `n` lowers to a `V2Fork` with
    /// exactly `n` targets, each guarded by a `V2MiIndexLive` skip check —
    /// the shape §18 ruling K's design hypothesis asked to be verified
    /// before building.
    #[test]
    fn test_multi_instance_lowering_v2_fork_shape() {
        let graph = make_multi_instance_graph(3);
        verifier::verify_or_err(&graph).unwrap();
        let program = lower_v2(&graph).unwrap();
        let instrs = program.program();

        assert!(
            instrs.iter().any(|i| matches!(i, Instr::V2MiArityCheck { max: 3, .. })),
            "must precheck actual length against the declared max BEFORE V2Fork"
        );
        let fork = instrs
            .iter()
            .find_map(|i| match i {
                Instr::V2Fork { targets, .. } => Some(targets),
                _ => None,
            })
            .expect("V2Fork must be present");
        assert_eq!(fork.len(), 3, "V2Fork must have exactly declared_max targets");

        let index_live_count = instrs
            .iter()
            .filter(|i| matches!(i, Instr::V2MiIndexLive { .. }))
            .count();
        assert_eq!(index_live_count, 3, "one V2MiIndexLive skip check per branch");

        let indices: std::collections::BTreeSet<u32> = instrs
            .iter()
            .filter_map(|i| match i {
                Instr::V2MiIndexLive { index, .. } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(indices, std::collections::BTreeSet::from([0, 1, 2]));

        assert!(instrs.iter().any(|i| matches!(i, Instr::V2Join { .. })));

        crate::Compiler::lower_v2(&graph).expect("v2 MI lowering must verify");
    }

    /// §18 ruling K Part 2: each branch also delivers its own element's
    /// `Value` via `V2MiLoadElement` (one per branch, same index set as
    /// `V2MiIndexLive`) immediately followed by a `StoreFlag` into a
    /// per-branch-UNIQUE flag key — proves no two branches share a
    /// scratch flag (the design this landing chose specifically to avoid
    /// needing to reason about tick-atomicity to argue correctness).
    #[test]
    fn test_multi_instance_lowering_delivers_per_branch_element_value() {
        let graph = make_multi_instance_graph(3);
        let program = lower_v2(&graph).unwrap();
        let instrs = program.program();

        let load_element_indices: std::collections::BTreeSet<u32> = instrs
            .iter()
            .filter_map(|i| match i {
                Instr::V2MiLoadElement { index, .. } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(
            load_element_indices,
            std::collections::BTreeSet::from([0, 1, 2]),
            "one V2MiLoadElement per branch, same index set as V2MiIndexLive"
        );

        // Every V2MiLoadElement must be immediately followed by a StoreFlag
        // (pops the loaded value into that branch's own element flag) —
        // proves the composition is wired the way the doc comment claims,
        // not merely that both instructions exist somewhere.
        let mut store_flag_keys = Vec::new();
        for (pc, instr) in instrs.iter().enumerate() {
            if matches!(instr, Instr::V2MiLoadElement { .. }) {
                match instrs.get(pc + 1) {
                    Some(Instr::StoreFlag { key }) => store_flag_keys.push(*key),
                    other => panic!(
                        "V2MiLoadElement at pc {pc} must be immediately followed by \
                         StoreFlag, got {other:?}"
                    ),
                }
            }
        }
        assert_eq!(store_flag_keys.len(), 3);
        let unique_keys: std::collections::BTreeSet<_> = store_flag_keys.iter().collect();
        assert_eq!(
            unique_keys.len(),
            3,
            "each branch must write its element into its OWN flag key, not a shared one"
        );

        // Every StoreFlag must be immediately followed by ExecNative — the
        // element is written before the inner activity's job is
        // dispatched, so the job's orch_flags snapshot includes it.
        for (pc, instr) in instrs.iter().enumerate() {
            if matches!(instr, Instr::StoreFlag { .. })
                && load_element_indices_contains_predecessor(instrs, pc)
            {
                assert!(
                    matches!(instrs.get(pc + 1), Some(Instr::ExecNative { .. })),
                    "StoreFlag at pc {pc} (MI element write) must be immediately \
                     followed by ExecNative"
                );
            }
        }
    }

    /// Helper for the test above: true iff `instrs[pc - 1]` is a
    /// `V2MiLoadElement` — i.e. this `StoreFlag` is the MI-element write,
    /// not some other unrelated `StoreFlag` in the program.
    fn load_element_indices_contains_predecessor(instrs: &[Instr], pc: usize) -> bool {
        pc > 0 && matches!(instrs[pc - 1], Instr::V2MiLoadElement { .. })
    }

    // V5.3 (§18, landed 2026-07-23): relocked in place — `lower()` no
    // longer has a distinct v1 path that rejects `IRNode::MultiInstance`;
    // it now lowers it exactly like `lower_v2()` does (both are the same
    // function, per Part A's flip). The old assertion ("v1 lower()
    // rejects, only lower_v2() succeeds") is not merely outdated, it is
    // now FALSE — `lower()` succeeds. Renamed and rewritten to prove
    // `lower()` produces the v2 MI shape directly, matching
    // `test_multi_instance_lowering_v2_fork_shape`'s own assertions.
    #[test]
    fn test_multi_instance_lowers_via_default_lower() {
        let graph = make_multi_instance_graph(3);
        verifier::verify_or_err(&graph).unwrap();
        let program = lower(&graph).expect("lower() must now lower a MultiInstance node directly");
        assert!(
            program
                .program()
                .iter()
                .any(|i| matches!(i, Instr::V2MiArityCheck { max: 3, .. })),
            "expected a V2MiArityCheck with the declared max"
        );
        assert!(
            program
                .program()
                .iter()
                .any(|i| matches!(i, Instr::V2Fork { targets, .. } if targets.len() == 3)),
            "expected a V2Fork with declared_max targets"
        );
    }
}
