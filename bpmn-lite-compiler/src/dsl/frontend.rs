use super::plan::{
    ExecutionNode, GuardExecSpec, GuardTriggerExec, JoinMode, SplitMode, TaskExecNode,
    WorkflowExecutionPlan,
};
use crate::ir::TimerSpec;
use crate::lowering::timer_spec_duration_ms;
use crate::VerifiedWorkflow;
use bpmn_lite_types::{
    legacy_program, Addr, ArtifactEnvelope, BindingSource, ExecutableWorkflow, Instr,
    PayloadRouteBranch, ScopeFailureBudget,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendError {
    MissingNode(String),
    InvalidRoute(String),
    CyclicOrUnreachable(Vec<String>),
    UnsupportedLoop(String),
    /// WS-D: a plan construct the frontend cannot faithfully lower —
    /// contradictory guard shapes (two timers on one host, an
    /// interrupting cycle timer) refuse by name, never lowered with the
    /// contradiction silently resolved.
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
    // WS-D D2: validate every guard set before sizing runs, so a
    // contradictory shape refuses by name instead of mis-sizing.
    for node in plan.nodes.values() {
        if let ExecutionNode::Task(task) = node {
            validate_guards(task)?;
        }
    }

    // WS-D D2, mirror of `lowering.rs`'s `guardn_close_before_end` walk:
    // a NON-interrupting guard's handler fibre inherits the guard's
    // scope token, so the escape flow's own terminal End must emit
    // `V2GuardNEnd` before `End` to keep V-1 (control-stack balance).
    // Same recorded narrowness as the XML side: only a linear escape
    // chain is resolved; a branching escape finds no terminal to mark
    // and `verify_v2_control_stack` then rejects the program with a
    // precise V-1 diagnostic rather than this walk mis-lowering it.
    let mut guardn_close_ends: HashSet<String> = HashSet::new();
    for node in plan.nodes.values() {
        let ExecutionNode::Task(task) = node else {
            continue;
        };
        if task.guards.is_empty() || effective_interrupting(task) {
            continue;
        }
        for guard in &task.guards {
            let mut cursor = Some(guard.escape_entry.as_str());
            let mut steps = 0usize;
            while let Some(id) = cursor {
                steps += 1;
                if steps > plan.nodes.len() {
                    break; // cycle guard — must not hang regardless.
                }
                match plan.nodes.get(id) {
                    Some(ExecutionNode::End(e)) => {
                        guardn_close_ends.insert(e.id.clone());
                        break;
                    }
                    Some(next_node) => {
                        let succs = next_node.flow_successors();
                        cursor = if succs.len() == 1 { Some(succs[0]) } else { None };
                    }
                    None => break,
                }
            }
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
        address = address.saturating_add(instruction_count(node, &guardn_close_ends)?);
    }

    let mut instructions = Vec::new();
    let mut debug_map = BTreeMap::new();
    let mut task_manifest = Vec::new();
    let mut task_ids = BTreeMap::new();
    let mut message_ids = BTreeMap::new();
    let mut message_name_map = BTreeMap::new();
    let mut v2_corr_sources = BTreeMap::new();
    let join_plan = BTreeMap::new();
    let mut fork_pairing: BTreeMap<String, Addr> = BTreeMap::new();
    let mut v2_guard_budgets: BTreeMap<Addr, ScopeFailureBudget> = BTreeMap::new();

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
                // WS-D D2: guarded emission mirrors `lowering.rs`'s
                // `lower_boundary_guarded_task_v2` shape exactly —
                // [PushI64 duration] (BEFORE guard-open, so the
                // verifier-enforced `V2GuardArmTimer`-at-open+1 adjacency
                // holds), guard-open, arming words, body, guard-close,
                // Jump. The body is `ExecDslTask` instead of the XML
                // path's `ExecNative` — both park on `WaitState::Job`, and
                // the kernel's guard firing + error routing are
                // fibre/record-based, body-agnostic (traced 2026-08-03).
                let timer = timer_guard(node);
                let errors = error_guards(node);
                let interrupting = effective_interrupting(node);

                if let Some(guard) = timer {
                    let GuardTriggerExec::Timer { spec, .. } = &guard.trigger else {
                        unreachable!("timer_guard returns Timer triggers only");
                    };
                    instructions.push(Instr::PushI64(timer_spec_duration_ms(spec) as i64));
                }
                if !node.guards.is_empty() {
                    let guard_open_addr = Addr::new(instructions.len() as u32);
                    // Budget precedence mirrors the XML path: the timer
                    // guard's own budget, else the first error guard's.
                    let budget_max = timer
                        .and_then(|g| g.failure_budget)
                        .or_else(|| errors.first().and_then(|g| g.failure_budget));
                    if let Some(max_failures) = budget_max {
                        let budget = ScopeFailureBudget::new(1, max_failures).map_err(|error| {
                            FrontendError::Artifact(format!(
                                "guard on '{}' has invalid failure budget {max_failures}: {error}",
                                node.id
                            ))
                        })?;
                        v2_guard_budgets.insert(guard_open_addr, budget);
                    }
                    // Guard-open handler: the timer's escape entry when a
                    // timer is attached (the only case anything fires
                    // through `record.handler`); otherwise the first
                    // error route's own handler as an inert placeholder —
                    // same rule, same rationale as the XML lowering.
                    let handler_entry = timer
                        .map(|g| g.escape_entry.as_str())
                        .or_else(|| errors.first().map(|g| g.escape_entry.as_str()))
                        .expect("guards non-empty");
                    let handler = target(&addresses, handler_entry)?;
                    instructions.push(if interrupting {
                        Instr::V2Guard { handler }
                    } else {
                        Instr::V2GuardN { handler }
                    });
                    if let Some(guard) = timer {
                        instructions.push(Instr::V2GuardArmTimer);
                        if let GuardTriggerExec::Timer {
                            spec: TimerSpec::Cycle { max_fires, .. },
                            ..
                        } = &guard.trigger
                        {
                            debug_assert!(!interrupting, "validate_guards refused this");
                            instructions.push(Instr::V2GuardTimerCycle {
                                max_fires: *max_fires,
                            });
                        }
                    }
                    for guard in &errors {
                        let GuardTriggerExec::Error { error_code } = &guard.trigger else {
                            unreachable!("error_guards returns Error triggers only");
                        };
                        instructions.push(Instr::V2GuardArmError {
                            error_code: error_code.clone().map(String::into_boxed_str),
                            handler: target(&addresses, &guard.escape_entry)?,
                        });
                    }
                }

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
                if !node.guards.is_empty() {
                    instructions.push(if interrupting {
                        Instr::V2GuardEnd
                    } else {
                        Instr::V2GuardNEnd
                    });
                }
                instructions.push(Instr::Jump {
                    target: target(&addresses, &node.next)?,
                });
            }
            ExecutionNode::Wait(node) => {
                // WS-D D2: same shape as the XML path's `TimerWait` arm —
                // operand push + wait word + trailing Jump (the wait words
                // carry no `next`; continuation is PC+1 on resume). Cycle
                // degrades to a single first-interval wait, the same
                // recorded simplification the XML lowering makes.
                match &node.spec {
                    TimerSpec::Duration { ms } => {
                        instructions.push(Instr::PushI64(*ms as i64));
                        instructions.push(Instr::V2WaitFor);
                    }
                    TimerSpec::Date { deadline_ms } => {
                        instructions.push(Instr::PushI64(*deadline_ms as i64));
                        instructions.push(Instr::V2WaitUntil);
                    }
                    TimerSpec::Cycle { interval_ms, .. } => {
                        instructions.push(Instr::PushI64(*interval_ms as i64));
                        instructions.push(Instr::V2WaitFor);
                    }
                }
                instructions.push(Instr::Jump {
                    target: target(&addresses, &node.next)?,
                });
            }
            ExecutionNode::MessageWait(node) => {
                let name = intern_message_name(
                    &node.name,
                    &mut message_ids,
                    &mut message_name_map,
                )?;
                let wait_addr = Addr::new(instructions.len() as u32);
                instructions.push(Instr::V2WaitMsg { name });
                let path = node
                    .correlation_key_source
                    .split('.')
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if path.is_empty() || path.iter().any(String::is_empty) {
                    return Err(FrontendError::UnsupportedPlanConstruct(format!(
                        "message wait '{}' has an empty or malformed correlation source",
                        node.id
                    )));
                }
                v2_corr_sources.insert(wait_addr, BindingSource::DomainPayloadRef(path));
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
            ExecutionNode::End(node) => {
                // WS-D D2: an End reached by a non-interrupting guard's
                // linear escape chain closes the inherited scope first —
                // see the `guardn_close_ends` pre-pass above.
                if guardn_close_ends.contains(&node.id) {
                    instructions.push(Instr::V2GuardNEnd);
                }
                instructions.push(Instr::End);
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
        message_name_map: message_name_map,
        write_set: BTreeMap::new(),
        task_manifest: task_manifest,
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    }
    // WS-D D2: same budget side-table attachment as the XML path; the
    // plan format has no workflow-level default-budget annotation, so the
    // conservative default applies to guards without an explicit budget.
    .with_v2_guard_budgets(v2_guard_budgets, ScopeFailureBudget::conservative_default())
    .with_v2_corr_sources(v2_corr_sources);
    let envelope = ArtifactEnvelope::from_legacy_program(legacy, env!("CARGO_PKG_VERSION"))
        .map_err(|error| FrontendError::Artifact(error.to_string()))?;
    ExecutableWorkflow::from_verified_envelope(envelope)
        .map_err(|error| FrontendError::Artifact(error.to_string()))
}

/// The single timer guard on a task, if any (`validate_guards` refuses >1).
fn timer_guard(task: &TaskExecNode) -> Option<&GuardExecSpec> {
    task.guards
        .iter()
        .find(|g| matches!(g.trigger, GuardTriggerExec::Timer { .. }))
}

/// Error guards in arming order: specific codes first, catch-all last —
/// the kernel's `error_routes` `.find()` precedence, same sort the XML
/// path's `push_error_guard_arms` applies.
fn error_guards(task: &TaskExecNode) -> Vec<&GuardExecSpec> {
    let mut errors: Vec<&GuardExecSpec> = task
        .guards
        .iter()
        .filter(|g| matches!(g.trigger, GuardTriggerExec::Error { .. }))
        .collect();
    errors.sort_by_key(|g| matches!(&g.trigger, GuardTriggerExec::Error { error_code: None }));
    errors
}

/// Combined guard interrupting-ness: any error guard forces interrupting
/// (BPMN has no non-interrupting error boundary), else the timer's own
/// flag — identical to the XML lowering's rule.
fn effective_interrupting(task: &TaskExecNode) -> bool {
    let has_errors = task
        .guards
        .iter()
        .any(|g| matches!(g.trigger, GuardTriggerExec::Error { .. }));
    let timer_interrupting = timer_guard(task).map(|g| {
        matches!(g.trigger, GuardTriggerExec::Timer { interrupting: true, .. })
    });
    has_errors || timer_interrupting.unwrap_or(true)
}

/// WS-D D2: refuse contradictory guard sets by name. Projected plans
/// can't produce these (the verifier admits max one timer per host, §7d,
/// and rejects interrupting cycles, §7b) — but the frontend also lowers
/// caller-submitted plans, which get no such construction guarantee.
fn validate_guards(task: &TaskExecNode) -> Result<(), FrontendError> {
    let timer_count = task
        .guards
        .iter()
        .filter(|g| matches!(g.trigger, GuardTriggerExec::Timer { .. }))
        .count();
    if timer_count > 1 {
        return Err(FrontendError::UnsupportedPlanConstruct(format!(
            "task '{}' carries {timer_count} timer guards — one V2Guard scope arms one timer (verifier §7d allows max one per host)",
            task.id
        )));
    }
    let is_cycle = matches!(
        timer_guard(task).map(|g| &g.trigger),
        Some(GuardTriggerExec::Timer { spec: TimerSpec::Cycle { .. }, .. })
    );
    if is_cycle && effective_interrupting(task) {
        return Err(FrontendError::UnsupportedPlanConstruct(format!(
            "task '{}' has a Cycle timer guard on an interrupting scope (own flag or a co-attached error guard) — cycle timers must be non-interrupting (verifier §7b); refused rather than silently narrowed to fire-once",
            task.id
        )));
    }
    Ok(())
}

fn instruction_count(
    node: &ExecutionNode,
    guardn_close_ends: &HashSet<String>,
) -> Result<u32, FrontendError> {
    match node {
        // WS-D D2 guarded-task block: base ExecDslTask+Jump (2), plus
        // guard open+close (2), plus PushI64+V2GuardArmTimer (2) when a
        // timer is armed, plus V2GuardTimerCycle (1) for a cycle, plus
        // one V2GuardArmError per error route.
        ExecutionNode::Task(node) if !node.guards.is_empty() => {
            let timer = timer_guard(node);
            let cycle: u32 = matches!(
                timer.map(|g| &g.trigger),
                Some(GuardTriggerExec::Timer { spec: TimerSpec::Cycle { .. }, .. })
            ) as u32;
            let timer_len: u32 = if timer.is_some() { 2 } else { 0 };
            let error_len = error_guards(node).len() as u32;
            Ok(4 + timer_len + cycle + error_len)
        }
        ExecutionNode::Task(_) => Ok(2),
        // WS-D D2: PushI64 + V2WaitFor/V2WaitUntil + Jump.
        ExecutionNode::Wait(_) => Ok(3),
        ExecutionNode::MessageWait(_) => Ok(2),
        // WS-D D2: +1 for the V2GuardNEnd a non-interrupting guard's
        // escape terminal emits before its own End.
        ExecutionNode::End(node) if guardn_close_ends.contains(&node.id) => Ok(2),
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
        // WS-D D2: guard escape entries are ordering-successors of their
        // host — V-8 requires guard handler edges to be FORWARD, so the
        // escape subgraph must be addressed after the guarded task, the
        // same order the XML lowering produces.
        ExecutionNode::Task(node) => std::iter::once(node.next.as_str())
            .chain(node.guards.iter().map(|g| g.escape_entry.as_str()))
            .collect(),
        ExecutionNode::Split(node) => node.flows.iter().map(|flow| flow.next.as_str()).collect(),
        ExecutionNode::Join(node) => vec![&node.next],
        ExecutionNode::Wait(node) => vec![&node.next],
        ExecutionNode::MessageWait(node) => vec![&node.next],
        ExecutionNode::End(_) => Vec::new(),
    }
}

fn intern_message_name(
    name: &str,
    ids: &mut BTreeMap<String, u32>,
    names: &mut BTreeMap<u32, String>,
) -> Result<u32, FrontendError> {
    if let Some(id) = ids.get(name) {
        return Ok(*id);
    }
    let mut id = u32::from_le_bytes(blake3::hash(name.as_bytes()).as_bytes()[..4].try_into().map_err(
        |_| FrontendError::Artifact("message-name hash could not be narrowed".to_string()),
    )?);
    while let Some(existing) = names.get(&id) {
        if existing == name {
            ids.insert(name.to_string(), id);
            return Ok(id);
        }
        id = id.wrapping_add(1);
    }
    ids.insert(name.to_string(), id);
    names.insert(id, name.to_string());
    Ok(id)
}

/// G3.3: with `ExecutionNode::Loop` retired (every loop unrolls to forward
/// copies before the linter ever runs — see `unroll::unroll_loops`), there
/// is no cyclic/loop-body special case left to reconcile: a plain
/// Kahn's-algorithm topological sort over `outgoing()` succeeds on any
/// admissible plan, and fails with `CyclicOrUnreachable` on anything that
/// isn't (a real cycle, or a node genuinely unreachable from start).
fn topological_order(plan: &WorkflowExecutionPlan) -> Result<Vec<String>, FrontendError> {
    if !plan.nodes.contains_key(&plan.start_node) {
        return Err(FrontendError::MissingNode(plan.start_node.clone()));
    }
    let mut incoming: BTreeMap<String, usize> =
        plan.nodes.keys().map(|node| (node.clone(), 0)).collect();
    for node in plan.nodes.values() {
        for successor in outgoing(node) {
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
        for successor in outgoing(&plan.nodes[&node_id]) {
            let count = incoming
                .get_mut(successor)
                .ok_or_else(|| FrontendError::MissingNode(successor.to_string()))?;
            *count = count.saturating_sub(1);
            if *count == 0 {
                ready.insert(successor.to_string());
            }
        }
    }
    if order.len() != plan.nodes.len() {
        let seen: HashSet<_> = order.iter().cloned().collect();
        let missing = plan
            .nodes
            .keys()
            .filter(|node| !seen.contains(*node))
            .cloned()
            .collect();
        return Err(FrontendError::CyclicOrUnreachable(missing));
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::plan::{
        EndExecNode, JoinExecNode, PlaceholderSchema, SplitExecFlow, SplitExecNode,
        StartExecNode, TaskExecNode,
    };
    use crate::dsl::{DeliveryMode, StubPlaceholderRegistry};
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
                        loop_origin: None,
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
                        loop_origin: None,
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
                        loop_origin: None,
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

    // G3.1/G3.3 (2026-08-11): `bounded_loop_lowers_to_verifier_admitted_
    // counter_control_flow` retired. It hand-built a plan directly
    // containing `ExecutionNode::Loop` to exercise this file's old
    // `IncCounter`/`BrCounterLt` loop-lowering arm — both the type and the
    // arm are deleted (`unroll::unroll_loops` now expands every loop to
    // forward copies before the linter ever runs, so `ExecutionNode::Loop`
    // can no longer be constructed by any real pipeline). Replaced below
    // with the actual current guarantee: a `(loop ...)` DSL source lowers
    // to N forward-chained task instructions and never emits
    // `IncCounter`/`BrCounterLt` at all.
    #[test]
    fn bounded_loop_lowers_to_n_forward_tasks_with_no_counter_instructions() {
        let src = r#"(workflow test-loop
          (start-event :id start :next retry)
          (loop :id retry :ceiling 3 :body (
             (service-task :id body :verb cbu.create :next retry)
          ) :next end)
          (end-event :id end :status "done"))"#;
        let registry = StubPlaceholderRegistry::new().with_demo_bindings();
        let plan = crate::dsl::compile(src, &registry).expect("compile");
        let workflow = DslFrontend::lower(&plan).expect("lower");
        let instructions = workflow.envelope().instructions();

        assert!(
            !instructions
                .iter()
                .any(|i| matches!(i, Instr::IncCounter { .. } | Instr::BrCounterLt { .. })),
            "no counter-based back-edge instruction should ever be emitted post-G3"
        );
        let exec_dsl_task_count = instructions
            .iter()
            .filter(|i| matches!(i, Instr::ExecDslTask { .. }))
            .count();
        assert_eq!(
            exec_dsl_task_count, 3,
            "ceiling 3 -> 3 distinct forward task instructions (G3.4 audit position: N unrolled \
             copies, not one counter-guarded repeat)"
        );
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
                    loop_origin: None,
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
                    loop_origin: None,
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
                    loop_origin: None,
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

    // ═══════════════════════════════════════════════════════════
    //  WS-D D2 — guard/wait opcode emission. (Replaces the D1 gate test
    //  `guard_bearing_plan_is_refused_until_d2_not_silently_unguarded` —
    //  that test cemented a temporary scaffold explicitly labelled
    //  "until D2"; these receipts are what it was holding the door for.)
    // ═══════════════════════════════════════════════════════════

    use crate::ir::{IREdge, IRGraph, IRNode};

    fn ir_task(id: &str) -> IRNode {
        IRNode::ServiceTask { id: id.into(), name: id.into(), task_type: "noop".into() }
    }

    /// start → t1 → end with a boundary timer guard on t1 whose escape
    /// flow is escalate → end_esc (the productions.rs shape).
    fn guarded_ir(interrupting: bool, spec: crate::ir::TimerSpec, budget: Option<u32>) -> IRGraph {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let t = g.add_node(ir_task("t1"));
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        let bt = g.add_node(IRNode::BoundaryTimer {
            id: "bt".into(),
            attached_to: "t1".into(),
            spec,
            interrupting,
            failure_budget: budget,
        });
        let esc = g.add_node(ir_task("escalate"));
        let esc_end = g.add_node(IRNode::End { id: "end_esc".into(), terminate: false });
        g.add_edge(s, t, IREdge { id: "e1".into(), condition: None });
        g.add_edge(t, end, IREdge { id: "e2".into(), condition: None });
        g.add_edge(bt, esc, IREdge { id: "g1".into(), condition: None });
        g.add_edge(esc, esc_end, IREdge { id: "g2".into(), condition: None });
        g
    }

    /// Structural tag for scaffold comparison: the guard scaffold must be
    /// identical between the XML path and the plan path; only the body
    /// word differs by design (`ExecNative` vs `ExecDslTask` — both park
    /// on `WaitState::Job`, and guard firing is fibre/record-based).
    fn scaffold_tag(instr: &Instr) -> &'static str {
        match instr {
            Instr::PushI64(_) => "PushI64",
            Instr::V2Guard { .. } => "V2Guard",
            Instr::V2GuardN { .. } => "V2GuardN",
            Instr::V2GuardArmTimer => "V2GuardArmTimer",
            Instr::V2GuardTimerCycle { .. } => "V2GuardTimerCycle",
            Instr::V2GuardArmError { .. } => "V2GuardArmError",
            Instr::V2GuardEnd => "V2GuardEnd",
            Instr::V2GuardNEnd => "V2GuardNEnd",
            Instr::ExecNative { .. } | Instr::ExecDslTask { .. } => "BODY",
            Instr::Jump { .. } => "Jump",
            Instr::End => "End",
            _ => "other",
        }
    }

    /// Extract the guarded block: from the PushI64 preceding guard-open
    /// through the trailing Jump after guard-close.
    fn guard_block(instrs: &[Instr]) -> Vec<&'static str> {
        let open = instrs
            .iter()
            .position(|i| matches!(i, Instr::V2Guard { .. } | Instr::V2GuardN { .. }))
            .expect("a guard must open");
        let close = instrs
            .iter()
            .position(|i| matches!(i, Instr::V2GuardEnd | Instr::V2GuardNEnd))
            .expect("a guard must close");
        let start = open.saturating_sub(1);
        instrs[start..=close + 1].iter().map(scaffold_tag).collect()
    }

    /// GREEN (WS-D D2, the phase's core receipt): the SAME guarded IR
    /// graph lowered via the XML path (`Compiler::lower_v2`) and via the
    /// plan path (`project_ir` → `DslFrontend::lower`) produces the
    /// IDENTICAL guard scaffold — same opcodes in the same order, same
    /// budget at the guard-open address, same verifier-enforced
    /// `V2GuardArmTimer`-at-open+1 adjacency — with only the body word
    /// differing. Both artifacts pass the real V-verifier
    /// (`from_verified_envelope`), not a mock.
    #[test]
    fn plan_path_guard_scaffold_matches_xml_path() {
        let g = guarded_ir(true, crate::ir::TimerSpec::Duration { ms: 60_000 }, Some(3));

        let xml_program = crate::lowering::lower_v2(&g).expect("XML path must lower");
        let plan = crate::dsl::ir_plan::project_ir(&g, "wf".into()).expect("must project");
        let workflow = DslFrontend::lower(&plan).expect("plan path must lower and verify");
        let plan_instrs = workflow.envelope().instructions();

        assert_eq!(
            guard_block(xml_program.program()),
            guard_block(plan_instrs),
            "guard scaffold must be identical across the two lowering front-ends"
        );
        // Expected literal shape, cemented:
        assert_eq!(
            guard_block(plan_instrs),
            vec!["PushI64", "V2Guard", "V2GuardArmTimer", "BODY", "V2GuardEnd", "Jump"]
        );

        // Budget lands at the guard-open address on BOTH paths.
        for (label, instrs, budgets) in [
            (
                "xml",
                xml_program.program().as_slice(),
                xml_program.v2_guard_budgets(),
            ),
            (
                "plan",
                plan_instrs,
                workflow.envelope().metadata().v2_guard_budgets(),
            ),
        ] {
            let open = instrs
                .iter()
                .position(|i| matches!(i, Instr::V2Guard { .. }))
                .unwrap();
            let budget = budgets
                .get(&Addr::new(open as u32))
                .unwrap_or_else(|| panic!("{label}: budget must sit at guard-open"));
            assert_eq!(budget.max_failures(), 3, "{label}: declared budget lowered");
            // Verifier-enforced adjacency: arm at open+1.
            assert!(
                matches!(instrs[open + 1], Instr::V2GuardArmTimer),
                "{label}: V2GuardArmTimer must sit at guard-open + 1"
            );
        }
    }

    /// GREEN (WS-D D2): a rearming (non-interrupting) Cycle guard lowers
    /// to V2GuardN + V2GuardArmTimer + V2GuardTimerCycle, and the escape
    /// flow's own terminal End closes the inherited scope with
    /// V2GuardNEnd — identical scaffold to the XML path for the same IR.
    #[test]
    fn rearming_cycle_guard_scaffold_matches_xml_path() {
        let g = guarded_ir(
            false,
            crate::ir::TimerSpec::Cycle { interval_ms: 86_400_000, max_fires: 3 },
            None,
        );

        let xml_program = crate::lowering::lower_v2(&g).expect("XML path must lower");
        let plan = crate::dsl::ir_plan::project_ir(&g, "wf".into()).expect("must project");
        let workflow = DslFrontend::lower(&plan).expect("plan path must lower and verify");
        let plan_instrs = workflow.envelope().instructions();

        assert_eq!(guard_block(xml_program.program()), guard_block(plan_instrs));
        assert_eq!(
            guard_block(plan_instrs),
            vec![
                "PushI64",
                "V2GuardN",
                "V2GuardArmTimer",
                "V2GuardTimerCycle",
                "BODY",
                "V2GuardNEnd",
                "Jump"
            ]
        );
        assert!(
            plan_instrs.iter().any(|i| matches!(i, Instr::V2GuardTimerCycle { max_fires: 3 })),
            "max_fires must survive"
        );
        // The escape terminal emits V2GuardNEnd before its End on both paths.
        let count_guardn_ends = |instrs: &[Instr]| {
            instrs.iter().filter(|i| matches!(i, Instr::V2GuardNEnd)).count()
        };
        assert_eq!(count_guardn_ends(plan_instrs), 2, "body close + escape-terminal close");
        assert_eq!(count_guardn_ends(xml_program.program()), 2);
    }

    /// GREEN (WS-D D2): error guards arm specific-code-first, catch-all
    /// last (kernel `error_routes` precedence) regardless of the order
    /// they appear on the plan node, and any error guard forces the
    /// combined scope interrupting.
    #[test]
    fn error_guards_arm_specific_first_catch_all_last() {
        let mut plan = routing_plan();
        if let Some(ExecutionNode::Task(t)) = plan.nodes.get_mut("decide") {
            // Deliberately catch-all FIRST on the node — emission must sort.
            t.guards.push(crate::dsl::plan::GuardExecSpec {
                guard_id: "be_all".into(),
                trigger: crate::dsl::plan::GuardTriggerExec::Error { error_code: None },
                failure_budget: None,
                escape_entry: "end".into(),
            });
            t.guards.push(crate::dsl::plan::GuardExecSpec {
                guard_id: "be_rej".into(),
                trigger: crate::dsl::plan::GuardTriggerExec::Error {
                    error_code: Some("FILING_REJECTED".into()),
                },
                failure_budget: None,
                escape_entry: "end".into(),
            });
        }
        let workflow = DslFrontend::lower(&plan).expect("error-guarded plan must lower");
        let instrs = workflow.envelope().instructions();
        let arms: Vec<Option<String>> = instrs
            .iter()
            .filter_map(|i| match i {
                Instr::V2GuardArmError { error_code, .. } => {
                    Some(error_code.as_ref().map(|c| c.to_string()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            arms,
            vec![Some("FILING_REJECTED".to_string()), None],
            "specific code first, catch-all last"
        );
        assert!(
            instrs.iter().any(|i| matches!(i, Instr::V2Guard { .. })),
            "error guards force an interrupting V2Guard scope"
        );
    }

    /// GREEN (WS-D D2): Wait nodes lower to operand push + wait word +
    /// Jump — real durable timers, not the dead `bpmn:timer-wait` plug.
    #[test]
    fn wait_nodes_lower_to_v2_wait_words() {
        for (spec, expect_until) in [
            (crate::ir::TimerSpec::Duration { ms: 5000 }, false),
            (crate::ir::TimerSpec::Date { deadline_ms: 1_900_000_000_000 }, true),
        ] {
            let mut plan = routing_plan();
            if let Some(ExecutionNode::Task(t)) = plan.nodes.get_mut("decide") {
                t.next = "w".into();
            }
            plan.nodes.insert(
                "w".into(),
                ExecutionNode::Wait(crate::dsl::plan::WaitExecNode {
                    id: "w".into(),
                    spec: spec.clone(),
                    next: "route".into(),
                    span: None,
                }),
            );
            let workflow = DslFrontend::lower(&plan).expect("wait-bearing plan must lower");
            let instrs = workflow.envelope().instructions();
            let wait_pos = instrs
                .iter()
                .position(|i| matches!(i, Instr::V2WaitFor | Instr::V2WaitUntil))
                .expect("a wait word must be emitted");
            assert!(matches!(instrs[wait_pos - 1], Instr::PushI64(_)));
            assert_eq!(
                matches!(instrs[wait_pos], Instr::V2WaitUntil),
                expect_until,
                "Duration→V2WaitFor, Date→V2WaitUntil"
            );
            assert!(matches!(instrs[wait_pos + 1], Instr::Jump { .. }));
        }
    }

    /// RED (WS-D D2): contradictory guard sets refuse by name — two
    /// timers on one host, and an interrupting Cycle (own flag or forced
    /// by a co-attached error guard).
    #[test]
    fn contradictory_guard_sets_are_refused() {
        let timer = |id: &str, spec: crate::ir::TimerSpec, interrupting: bool| {
            crate::dsl::plan::GuardExecSpec {
                guard_id: id.into(),
                trigger: crate::dsl::plan::GuardTriggerExec::Timer { spec, interrupting },
                failure_budget: None,
                escape_entry: "end".into(),
            }
        };

        let mut plan = routing_plan();
        if let Some(ExecutionNode::Task(t)) = plan.nodes.get_mut("decide") {
            t.guards.push(timer("bt1", crate::ir::TimerSpec::Duration { ms: 1 }, true));
            t.guards.push(timer("bt2", crate::ir::TimerSpec::Duration { ms: 2 }, true));
        }
        assert!(matches!(
            DslFrontend::lower(&plan),
            Err(FrontendError::UnsupportedPlanConstruct(ref m)) if m.contains("timer guards")
        ));

        let mut plan = routing_plan();
        if let Some(ExecutionNode::Task(t)) = plan.nodes.get_mut("decide") {
            t.guards.push(timer(
                "bt",
                crate::ir::TimerSpec::Cycle { interval_ms: 1000, max_fires: 2 },
                true,
            ));
        }
        assert!(matches!(
            DslFrontend::lower(&plan),
            Err(FrontendError::UnsupportedPlanConstruct(ref m)) if m.contains("Cycle")
        ));
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
