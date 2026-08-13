//! `IRGraph` → `WorkflowExecutionPlan` projection.
//!
//! G2 BLOCKER-2 remediation (Adam's ruling, 2026-07-27): the ONLY
//! sanctioned path from an authored `DesignerDag`/`IRGraph` to the
//! plan-store's stored artifact. Rider 1 requires this be the production
//! compiler's own validate-and-lower path, never a bespoke mapping
//! maintained outside `bpmn-lite-compiler` — this module reuses the SAME
//! [`derive_delivery_mode`] formula and [`validate_dag`] check `lint()`
//! uses for the DSL path, rather than reimplementing either.
//!
//! **Scope — deliberately conservative, fails closed beyond it (CAREFUL
//! tier: no unproven lossy shoehorning under time pressure).** Supported:
//! `Start`, `End`, `ServiceTask`, `MessageWait`, `MultiInstance` (G5.4a),
//! and `GatewayAnd`/`GatewayInclusive` matched diverging/converging pairs
//! (via the exposed [`gateway_pairs`] pairing oracle — never a hand-rolled
//! re-pairing). `DataObject` nodes are structural-only (zero bytecode, per
//! their own IR doc comment) and are simply omitted from the projected
//! plan.
//!
//! **WS-D D1 (Adam's "own phase" ruling, 2026-08-03)** widened the scope
//! with timer semantics: `BoundaryTimer`/`BoundaryError` project onto
//! their HOST task as [`GuardExecSpec`] decorations (mirroring the
//! kernel's model — guard arming is inline in the host's lowering, the
//! escape flow is ordinary downstream nodes) and `TimerWait` projects to
//! [`ExecutionNode::Wait`]. A guard's `escape_entry` is its node's single
//! outgoing successor, exactly how `lowering.rs` resolves the guard
//! `handler` address; a guard with zero or multiple outgoing edges is
//! refused by name ([`IrPlanError::GuardEscapeUnresolved`]) — verifier
//! §7c admits ≥1 edge on a timer guard, but the lowering only ever
//! resolves ONE handler, so projecting >1 would first-edge-guess.
//!
//! Explicitly OUT of scope, refused with a named [`IrPlanError`] rather
//! than guessed at:
//! - `GatewayXor` — has no `direction` field (unlike `GatewayAnd`/
//!   `GatewayInclusive`) and no compiler-exposed join-pairing oracle; its
//!   DSL counterpart's `join` id is an explicit AST annotation
//!   (`linter.rs`'s `NodeAst::Split.join`) with no IR equivalent. Adding
//!   XOR support needs its own traced join-inference design, not a guess.
//! - `HumanWait`, `SendTask`, `FfiServiceTask` — still no `ExecutionNode`
//!   representation in `WorkflowExecutionPlan` (XML/IR-authoring-only
//!   constructs). A guard ATTACHED to such a host therefore also cannot
//!   project — the host itself is refused first. (`MessageWait` and
//!   `MultiInstance`, formerly on this list, project since the
//!   MessageWait exec-node landing and G5.4a respectively — this header
//!   lagged the code and was corrected under EOP-PLAN-GRAPH-DSL-BRIDGE-001
//!   B0.)
//!
//! Placeholder inference is also out of scope for graph-authored
//! `ServiceTask` nodes: DSL's `Task.plug` is a catalogue-registered
//! `domain:verb` string resolved against a `PlaceholderRegistry`; IR's
//! `ServiceTask.task_type` is documented as an external-job dispatch
//! identity (Zeebe `taskDefinition type=` convention, see
//! `IRNode::FfiServiceTask`'s own doc contrasting the two) — a different
//! namespace, not a catalogue symbol. Attempting registry resolution on it
//! would be a category error, not merely strict; projected tasks carry no
//! `produces_placeholder`/`consumes_placeholders` and — through the shared
//! `derive_delivery_mode` formula, fed honestly with `output_consumed:
//! false, is_must_complete: false` (no catalogue signal exists) — default
//! to `DeliveryMode::BestEffort`, the same fallback the DSL path's formula
//! would reach for a task without a registered effect class.

use std::collections::BTreeMap;

use petgraph::visit::EdgeRef;
use petgraph::Direction;

use crate::gateway_pairs;
use crate::ir::{find_start, ConditionLiteral, ConditionOp, GatewayDirection, IRGraph, IRNode};

use super::dag::{validate_dag, DagError};
use super::plan::*;

/// Why an `IRGraph` was refused projection to a `WorkflowExecutionPlan`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum IrPlanError {
    #[error("graph has no Start node")]
    MissingStart,
    #[error("node '{id}' ({kind}) has no WorkflowExecutionPlan representation yet")]
    UnsupportedNode { id: String, kind: &'static str },
    #[error(
        "node '{id}' has {count} outgoing edge(s), expected exactly 1 for a non-gateway node"
    )]
    WrongOutDegree { id: String, count: usize },
    #[error("gateway '{id}' has no matching join (only GatewayAnd/GatewayInclusive diverging/converging pairs are supported)")]
    UnmatchedGateway { id: String },
    #[error("projected plan failed its own DAG validation: {0:?}")]
    DagInvalid(Vec<DagError>),
    #[error("edge on gateway '{gateway_id}' uses condition operator {op:?}, but SplitExecFlow only carries an equality comparison (DSL's own ConditionAst is Eq-only) — no lossy encoding attempted")]
    UnsupportedConditionOperator { gateway_id: String, op: ConditionOp },
    #[error("edge from '{id}' carries a condition, but {kind} has no field to represent one — only diverging GatewayAnd/GatewayInclusive edges can be conditioned")]
    UnrepresentableCondition { id: String, kind: &'static str },
    #[error("boundary guard '{guard_id}' on host '{host}' has {count} outgoing escape edge(s) — GuardExecSpec carries exactly one escape_entry (mirroring the lowering's single-successor handler resolution); 0 means the escape flow is missing, >1 would be first-edge-guessed rather than represented")]
    GuardEscapeUnresolved {
        guard_id: String,
        host: String,
        count: usize,
    },
    #[error("boundary guard '{guard_id}' is attached to '{host}', which did not project to a plan Task — a guard on an unprojected host would be silently dropped, which is the exact fail-open this projection refuses")]
    GuardHostUnprojected { guard_id: String, host: String },
    #[error("boundary guard '{guard_id}' escape flow enters '{escape_entry}', which has no plan representation (structural-only or unprojected node) — the guard's handler would dangle")]
    GuardEscapeDangling {
        guard_id: String,
        escape_entry: String,
    },
}

fn node_kind_name(node: &IRNode) -> &'static str {
    match node {
        IRNode::Start { .. } => "Start",
        IRNode::End { .. } => "End",
        IRNode::ServiceTask { .. } => "ServiceTask",
        IRNode::GatewayXor { .. } => "GatewayXor",
        IRNode::GatewayAnd { .. } => "GatewayAnd",
        IRNode::TimerWait { .. } => "TimerWait",
        IRNode::MessageWait { .. } => "MessageWait",
        IRNode::HumanWait { .. } => "HumanWait",
        IRNode::BoundaryTimer { .. } => "BoundaryTimer",
        IRNode::BoundaryError { .. } => "BoundaryError",
        IRNode::GatewayInclusive { .. } => "GatewayInclusive",
        IRNode::DataObject { .. } => "DataObject",
        IRNode::FfiServiceTask { .. } => "FfiServiceTask",
        IRNode::SendTask { .. } => "SendTask",
        IRNode::MultiInstance { .. } => "MultiInstance",
    }
}

/// One successor's edge id + node id, for nodes required to have exactly
/// one outgoing edge (everything except diverging gateways). `IREdge` is a
/// generic field on EVERY edge, not just diverging-gateway edges — e.g.
/// `Operation::Connect` can attach a condition between any two existing
/// nodes — so this refuses (rather than silently drops) a condition found
/// on an edge whose target `ExecutionNode` kind has no field to carry one.
fn single_successor(
    graph: &IRGraph,
    idx: petgraph::graph::NodeIndex,
) -> Result<String, IrPlanError> {
    let mut out = graph.edges_directed(idx, Direction::Outgoing);
    let first = out.next();
    let extra = out.next();
    match (first, extra) {
        (Some(edge), None) => {
            if edge.weight().condition.is_some() {
                return Err(IrPlanError::UnrepresentableCondition {
                    id: graph[idx].id().to_owned(),
                    kind: node_kind_name(&graph[idx]),
                });
            }
            Ok(graph[edge.target()].id().to_owned())
        }
        (first, extra) => Err(IrPlanError::WrongOutDegree {
            id: graph[idx].id().to_owned(),
            count: first.is_some() as usize + extra.is_some() as usize + out.count(),
        }),
    }
}

/// Project an admitted `IRGraph` into a `WorkflowExecutionPlan`.
///
/// Precondition (same as [`gateway_pairs`]/[`crate::compute_post_dominators`]):
/// `ir` must be acyclic and should already have passed [`crate::verify`] —
/// callers reconstruct via `DesignerDag::to_ir()` after `DesignerDag::admit()`
/// has succeeded, never on an unverified graph.
pub fn project_ir(ir: &IRGraph, workflow_id: String) -> Result<WorkflowExecutionPlan, IrPlanError> {
    let start_idx = find_start(ir).ok_or(IrPlanError::MissingStart)?;
    let pairs = gateway_pairs(ir);
    // Reverse map: converging join index -> its diverging split's id, so a
    // Converging node visited on its own iteration knows which Split it closes.
    let join_to_split: BTreeMap<petgraph::graph::NodeIndex, petgraph::graph::NodeIndex> =
        pairs.iter().map(|(&div, &join)| (join, div)).collect();

    // WS-D D1 pre-pass: collect boundary guards keyed by host id. Guard
    // nodes are decorations, not plan nodes — the node loop below skips
    // them, the host's ServiceTask arm consumes this map, and anything
    // left unconsumed afterwards is a hard error (never a silent drop).
    let mut guards_by_host: BTreeMap<String, Vec<GuardExecSpec>> = BTreeMap::new();
    for idx in ir.node_indices() {
        let (guard_id, host, trigger, failure_budget) = match &ir[idx] {
            IRNode::BoundaryTimer {
                id,
                attached_to,
                spec,
                interrupting,
                failure_budget,
            } => (
                id,
                attached_to,
                GuardTriggerExec::Timer {
                    spec: spec.clone(),
                    interrupting: *interrupting,
                },
                *failure_budget,
            ),
            IRNode::BoundaryError {
                id,
                attached_to,
                error_code,
                failure_budget,
            } => (
                id,
                attached_to,
                GuardTriggerExec::Error {
                    error_code: error_code.clone(),
                },
                *failure_budget,
            ),
            _ => continue,
        };
        let escape_entry = guard_escape_entry(ir, idx, guard_id, host)?;
        guards_by_host
            .entry(host.clone())
            .or_default()
            .push(GuardExecSpec {
                guard_id: guard_id.clone(),
                trigger,
                failure_budget,
                escape_entry,
            });
    }

    let mut nodes: BTreeMap<String, ExecutionNode> = BTreeMap::new();

    for idx in ir.node_indices() {
        let node = &ir[idx];
        let id = node.id().to_owned();

        match node {
            IRNode::Start { id } => {
                let next = single_successor(ir, idx)?;
                nodes.insert(
                    id.clone(),
                    ExecutionNode::Start(StartExecNode { id: id.clone(), next, span: None }),
                );
            }

            IRNode::End { id, terminate } => {
                let status = if *terminate { "terminated" } else { "completed" }.to_owned();
                nodes.insert(
                    id.clone(),
                    ExecutionNode::End(EndExecNode { id: id.clone(), status, span: None }),
                );
            }

            IRNode::ServiceTask { id, task_type, .. } => {
                let next = single_successor(ir, idx)?;
                let delivery_mode = derive_delivery_mode(None, false, false);
                nodes.insert(
                    id.clone(),
                    ExecutionNode::Task(TaskExecNode {
                        id: id.clone(),
                        plug: task_type.clone(),
                        delivery_mode,
                        static_args: Default::default(),
                        next,
                        produces_placeholder: None,
                        consumes_placeholders: Vec::new(),
                        guards: guards_by_host.remove(id).unwrap_or_default(),
                        span: None,
                        loop_origin: None,
                    }),
                );
            }

            // WS-D D1: guard nodes were consumed by the pre-pass above —
            // they decorate their host, they are not plan nodes. The
            // escape SUBGRAPH's own nodes still project normally on their
            // own loop iterations.
            IRNode::BoundaryTimer { .. } | IRNode::BoundaryError { .. } => {}

            IRNode::TimerWait { id, spec } => {
                let next = single_successor(ir, idx)?;
                nodes.insert(
                    id.clone(),
                    ExecutionNode::Wait(WaitExecNode {
                        id: id.clone(),
                        spec: spec.clone(),
                        next,
                        span: None,
                    }),
                );
            }

            IRNode::MessageWait {
                id,
                name,
                corr_key_source,
            } => {
                let next = single_successor(ir, idx)?;
                nodes.insert(
                    id.clone(),
                    ExecutionNode::MessageWait(crate::dsl::plan::MessageWaitExecNode {
                        id: id.clone(),
                        name: name.clone(),
                        correlation_key_source: corr_key_source.clone(),
                        next,
                        span: None,
                    }),
                );
            }

            IRNode::GatewayAnd { id, direction, .. }
            | IRNode::GatewayInclusive { id, direction, .. } => {
                let mode = match node {
                    IRNode::GatewayAnd { .. } => SplitMode::Parallel,
                    IRNode::GatewayInclusive { .. } => SplitMode::Inclusive,
                    _ => unreachable!(),
                };
                match direction {
                    GatewayDirection::Diverging => {
                        let join_idx = pairs
                            .get(&idx)
                            .copied()
                            .ok_or_else(|| IrPlanError::UnmatchedGateway { id: id.clone() })?;
                        // edges_directed (not neighbors_directed + find_edge)
                        // deliberately: neighbors_directed yields a target
                        // once per edge but find_edge always resolves to
                        // the FIRST edge between a pair, silently
                        // misattributing a second edge's condition to a
                        // parallel-edge fork/successor pair. edges_directed
                        // pairs each edge with its own target directly —
                        // the same pattern verifier.rs already uses.
                        let flows: Vec<SplitExecFlow> = ir
                            .edges_directed(idx, Direction::Outgoing)
                            .map(|edge| {
                                let condition = &edge.weight().condition;
                                let (placeholder, expected_value) = match condition {
                                    Some(c) => {
                                        if c.op != ConditionOp::Eq {
                                            return Err(IrPlanError::UnsupportedConditionOperator {
                                                gateway_id: id.clone(),
                                                op: c.op.clone(),
                                            });
                                        }
                                        (
                                            Some(c.flag_name.clone()),
                                            Some(condition_literal_to_expected(&c.literal)),
                                        )
                                    }
                                    None => (None, None),
                                };
                                Ok(SplitExecFlow {
                                    placeholder,
                                    expected_value,
                                    next: ir[edge.target()].id().to_owned(),
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        nodes.insert(
                            id.clone(),
                            ExecutionNode::Split(SplitExecNode {
                                id: id.clone(),
                                mode,
                                routing_socket: None,
                                flows,
                                join: ir[join_idx].id().to_owned(),
                                produces_placeholder: None,
                                span: None,
                            }),
                        );
                    }
                    GatewayDirection::Converging => {
                        let split_idx = join_to_split
                            .get(&idx)
                            .copied()
                            .ok_or_else(|| IrPlanError::UnmatchedGateway { id: id.clone() })?;
                        let join_mode = match mode {
                            SplitMode::Parallel => JoinMode::Parallel,
                            SplitMode::Inclusive => JoinMode::Inclusive,
                            SplitMode::Exclusive => JoinMode::Exclusive,
                        };
                        let next = single_successor(ir, idx)?;
                        nodes.insert(
                            id.clone(),
                            ExecutionNode::Join(JoinExecNode {
                                id: id.clone(),
                                mode: join_mode,
                                split: ir[split_idx].id().to_owned(),
                                next,
                                span: None,
                            }),
                        );
                    }
                }
            }

            IRNode::MultiInstance {
                id,
                task_type,
                collection_flag_name,
                declared_max,
                ..
            } => {
                let next = single_successor(ir, idx)?;
                nodes.insert(
                    id.clone(),
                    ExecutionNode::MultiInstance(crate::dsl::plan::MultiInstanceExecNode {
                        id: id.clone(),
                        task_type: task_type.clone(),
                        collection_flag_name: collection_flag_name.clone(),
                        declared_max: *declared_max,
                        next,
                        span: None,
                    }),
                );
            }

            IRNode::DataObject { .. } => {
                // Structural-only, zero bytecode (ir.rs's own doc comment) —
                // no ExecutionNode representation needed or possible.
            }

            other => {
                return Err(IrPlanError::UnsupportedNode { id, kind: node_kind_name(other) });
            }
        }
    }

    // WS-D D1 fail-closed sweep: a guard whose host never became a plan
    // Task (unverified graph, dangling attached_to) must refuse, not
    // evaporate.
    if let Some((host, specs)) = guards_by_host.iter().next() {
        return Err(IrPlanError::GuardHostUnprojected {
            guard_id: specs[0].guard_id.clone(),
            host: host.clone(),
        });
    }

    // And every escape_entry must reference a node that actually
    // projected — an escape flow entering e.g. a DataObject (omitted from
    // the plan) would leave the guard's handler dangling. Escape CLOSURE
    // (reaching an End) needs no separate check here: every projected
    // non-End node has exactly one successor by construction
    // (`single_successor`) and `validate_dag` below rejects cycles, so a
    // projected escape path always terminates in an End. Hand-built plans
    // don't get that construction guarantee, which is why
    // `analyze_safety` re-proves closure as a breach check.
    for node in nodes.values() {
        if let ExecutionNode::Task(task) = node {
            for guard in &task.guards {
                if !nodes.contains_key(&guard.escape_entry) {
                    return Err(IrPlanError::GuardEscapeDangling {
                        guard_id: guard.guard_id.clone(),
                        escape_entry: guard.escape_entry.clone(),
                    });
                }
            }
        }
    }

    let plan = WorkflowExecutionPlan::new(
        workflow_id,
        nodes,
        ir[start_idx].id().to_owned(),
        PlaceholderSchema::default(),
        Some(serde_json::json!({ "dependencies": [] })),
        std::env::var("BPMN_LITE_REGIME_VERSION").ok(),
    );

    validate_dag(&plan).map_err(IrPlanError::DagInvalid)?;

    Ok(plan)
}

/// Resolve a boundary guard's escape-flow entry: the guard node's single
/// outgoing successor, exactly how `lowering.rs` resolves the guard's
/// `handler` address (`push_error_guard_arms` and the timer arm both take
/// the sole successor). Zero edges = missing escape flow; multiple edges
/// = refused rather than first-edge-guessed (verifier §7c admits ≥1 on a
/// timer guard, but one `handler` address can't represent two routes). A
/// condition on the escape edge has no `GuardExecSpec` field to carry it.
fn guard_escape_entry(
    ir: &IRGraph,
    idx: petgraph::graph::NodeIndex,
    guard_id: &str,
    host: &str,
) -> Result<String, IrPlanError> {
    let edges: Vec<_> = ir.edges_directed(idx, Direction::Outgoing).collect();
    match edges.as_slice() {
        [edge] => {
            if edge.weight().condition.is_some() {
                return Err(IrPlanError::UnrepresentableCondition {
                    id: guard_id.to_owned(),
                    kind: node_kind_name(&ir[idx]),
                });
            }
            Ok(ir[edge.target()].id().to_owned())
        }
        _ => Err(IrPlanError::GuardEscapeUnresolved {
            guard_id: guard_id.to_owned(),
            host: host.to_owned(),
            count: edges.len(),
        }),
    }
}

fn condition_literal_to_expected(literal: &ConditionLiteral) -> String {
    match literal {
        ConditionLiteral::Bool(b) => b.to_string(),
        ConditionLiteral::I64(n) => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ConditionExpr, IREdge, IRGraph};

    fn task(id: &str) -> IRNode {
        IRNode::ServiceTask { id: id.into(), name: id.into(), task_type: "noop".into() }
    }

    /// GREEN: a plain linear Start -> Task -> End chain projects cleanly
    /// and passes its own validate_dag check.
    #[test]
    fn linear_chain_projects() {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let t = g.add_node(task("t1"));
        let e = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        g.add_edge(s, t, IREdge { id: "e1".into(), condition: None });
        g.add_edge(t, e, IREdge { id: "e2".into(), condition: None });

        let plan = project_ir(&g, "wf".into()).expect("linear chain must project");
        assert_eq!(plan.start_node, "start");
        assert_eq!(plan.nodes.len(), 3);
        match plan.nodes.get("t1").unwrap() {
            ExecutionNode::Task(t) => {
                assert_eq!(t.plug, "noop");
                assert_eq!(t.delivery_mode, DeliveryMode::BestEffort);
                assert_eq!(t.next, "end");
            }
            other => panic!("expected Task, got {other:?}"),
        }
    }

    /// GREEN: a matched GatewayAnd diverging/converging pair (via the
    /// gateway_pairs oracle, not hand-rolled pairing) projects to a real
    /// Split/Join pair.
    #[test]
    fn matched_and_gateway_pair_projects_to_split_join() {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let fork = g.add_node(IRNode::GatewayAnd {
            id: "fork".into(),
            name: "fork".into(),
            direction: GatewayDirection::Diverging,
        });
        let a = g.add_node(task("a"));
        let b = g.add_node(task("b"));
        let join = g.add_node(IRNode::GatewayAnd {
            id: "join".into(),
            name: "join".into(),
            direction: GatewayDirection::Converging,
        });
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        g.add_edge(s, fork, IREdge { id: "e0".into(), condition: None });
        g.add_edge(fork, a, IREdge { id: "e1".into(), condition: None });
        g.add_edge(fork, b, IREdge { id: "e2".into(), condition: None });
        g.add_edge(a, join, IREdge { id: "e3".into(), condition: None });
        g.add_edge(b, join, IREdge { id: "e4".into(), condition: None });
        g.add_edge(join, end, IREdge { id: "e5".into(), condition: None });

        let plan = project_ir(&g, "wf".into()).expect("matched AND pair must project");
        match plan.nodes.get("fork").unwrap() {
            ExecutionNode::Split(s) => {
                assert_eq!(s.mode, SplitMode::Parallel);
                assert_eq!(s.join, "join");
                assert_eq!(s.flows.len(), 2);
            }
            other => panic!("expected Split, got {other:?}"),
        }
        match plan.nodes.get("join").unwrap() {
            ExecutionNode::Join(j) => {
                assert_eq!(j.mode, JoinMode::Parallel);
                assert_eq!(j.split, "fork");
                assert_eq!(j.next, "end");
            }
            other => panic!("expected Join, got {other:?}"),
        }
    }

    /// RED: GatewayXor has no compiler-exposed join-pairing oracle — must
    /// fail closed, never guess a join.
    #[test]
    fn xor_gateway_is_refused_not_guessed() {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let x = g.add_node(IRNode::GatewayXor { id: "xor".into(), name: "xor".into() });
        let a = g.add_node(task("a"));
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        g.add_edge(s, x, IREdge { id: "e0".into(), condition: None });
        g.add_edge(x, a, IREdge { id: "e1".into(), condition: None });
        g.add_edge(a, end, IREdge { id: "e2".into(), condition: None });

        let err = project_ir(&g, "wf".into()).expect_err("GatewayXor must be refused");
        assert!(matches!(
            err,
            IrPlanError::UnsupportedNode { kind: "GatewayXor", .. }
        ));
    }

    /// RED (amended for WS-D D1, Adam's "own phase" ruling 2026-08-03):
    /// this cement test originally asserted that ANY BoundaryTimer was
    /// refused (`UnsupportedNode`) — that behaviour was deliberately
    /// replaced by guard projection. What remains cemented is the
    /// fail-closed core the original test actually exercised: this
    /// graph's guard has NO escape flow, and an escape-less guard still
    /// refuses — now by its precise name — rather than projecting a
    /// guard whose handler leads nowhere.
    #[test]
    fn boundary_timer_is_refused_not_shoehorned() {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let t = g.add_node(task("t1"));
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        g.add_node(IRNode::BoundaryTimer {
            id: "bt".into(),
            attached_to: "t1".into(),
            spec: crate::ir::TimerSpec::Duration { ms: 1000 },
            interrupting: true,
            failure_budget: None,
        });
        g.add_edge(s, t, IREdge { id: "e1".into(), condition: None });
        g.add_edge(t, end, IREdge { id: "e2".into(), condition: None });

        let err = project_ir(&g, "wf".into()).expect_err("an escape-less guard must be refused");
        assert!(matches!(
            err,
            IrPlanError::GuardEscapeUnresolved { count: 0, .. }
        ));
    }

    /// Build the canonical guarded shape (`productions.rs`'s
    /// interrupting_timeout): start → t1 → end, guard on t1 whose escape
    /// flow is its own task → own End.
    fn guarded_graph(interrupting: bool, budget: Option<u32>) -> IRGraph {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let t = g.add_node(task("t1"));
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        let bt = g.add_node(IRNode::BoundaryTimer {
            id: "bt".into(),
            attached_to: "t1".into(),
            spec: crate::ir::TimerSpec::Duration { ms: 60_000 },
            interrupting,
            failure_budget: budget,
        });
        let esc = g.add_node(task("escalate"));
        let esc_end = g.add_node(IRNode::End { id: "end_esc".into(), terminate: false });
        g.add_edge(s, t, IREdge { id: "e1".into(), condition: None });
        g.add_edge(t, end, IREdge { id: "e2".into(), condition: None });
        g.add_edge(bt, esc, IREdge { id: "g1".into(), condition: None });
        g.add_edge(esc, esc_end, IREdge { id: "g2".into(), condition: None });
        g
    }

    /// GREEN (WS-D D1): a well-formed interrupting boundary timer
    /// projects as a guard DECORATION on its host — trigger, budget, and
    /// escape entry intact, escape subgraph present as ordinary plan
    /// nodes, and the plan's own proof holds (no breaches).
    #[test]
    fn guarded_task_projects_guard_decoration_with_escape_subgraph() {
        let g = guarded_graph(true, Some(3));
        let plan = project_ir(&g, "wf".into()).expect("guarded task must project");

        let ExecutionNode::Task(t1) = plan.nodes.get("t1").unwrap() else {
            panic!("t1 must be a Task");
        };
        assert_eq!(t1.guards.len(), 1);
        let guard = &t1.guards[0];
        assert_eq!(guard.guard_id, "bt");
        assert_eq!(guard.failure_budget, Some(3));
        assert_eq!(guard.escape_entry, "escalate");
        assert!(matches!(
            guard.trigger,
            GuardTriggerExec::Timer { interrupting: true, spec: crate::ir::TimerSpec::Duration { ms: 60_000 } }
        ));
        // The guard node itself is NOT a plan node; its escape subgraph is.
        assert!(!plan.nodes.contains_key("bt"));
        assert!(plan.nodes.contains_key("escalate"));
        assert!(plan.nodes.contains_key("end_esc"));
        // The proof holds: escape subgraph reachable + closed, SESE intact.
        assert!(
            plan.mathematically_proved(),
            "breaches: {:?}",
            plan.unsafe_breeches()
        );
    }

    /// GREEN (WS-D D1): non-interrupting (rearming) cycle guard projects
    /// with its trigger shape intact.
    #[test]
    fn rearming_cycle_guard_projects() {
        let mut g = guarded_graph(false, None);
        for idx in g.node_indices() {
            if let IRNode::BoundaryTimer { spec, .. } = &mut g[idx] {
                *spec = crate::ir::TimerSpec::Cycle { interval_ms: 86_400_000, max_fires: 3 };
            }
        }
        let plan = project_ir(&g, "wf".into()).expect("rearming guard must project");
        let ExecutionNode::Task(t1) = plan.nodes.get("t1").unwrap() else {
            panic!("t1 must be a Task");
        };
        assert!(matches!(
            t1.guards[0].trigger,
            GuardTriggerExec::Timer {
                interrupting: false,
                spec: crate::ir::TimerSpec::Cycle { interval_ms: 86_400_000, max_fires: 3 }
            }
        ));
    }

    /// GREEN (WS-D D1): a boundary error projects as an Error-triggered
    /// guard (no `interrupting` flag to carry — the type makes
    /// non-interrupting error unrepresentable).
    #[test]
    fn boundary_error_projects_as_error_guard() {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let t = g.add_node(task("t1"));
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        let be = g.add_node(IRNode::BoundaryError {
            id: "be".into(),
            attached_to: "t1".into(),
            error_code: Some("FILING_REJECTED".into()),
            failure_budget: None,
        });
        let esc_end = g.add_node(IRNode::End { id: "end_err".into(), terminate: false });
        g.add_edge(s, t, IREdge { id: "e1".into(), condition: None });
        g.add_edge(t, end, IREdge { id: "e2".into(), condition: None });
        g.add_edge(be, esc_end, IREdge { id: "g1".into(), condition: None });

        let plan = project_ir(&g, "wf".into()).expect("boundary error must project");
        let ExecutionNode::Task(t1) = plan.nodes.get("t1").unwrap() else {
            panic!("t1 must be a Task");
        };
        assert_eq!(t1.guards.len(), 1);
        assert!(matches!(
            &t1.guards[0].trigger,
            GuardTriggerExec::Error { error_code: Some(code) } if code == "FILING_REJECTED"
        ));
        assert_eq!(t1.guards[0].escape_entry, "end_err");
        assert!(plan.mathematically_proved());
    }

    /// GREEN (WS-D D1): standalone TimerWait projects to a first-class
    /// Wait node, not a Task-plug shoehorn.
    #[test]
    fn timer_wait_projects_to_wait_node() {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let w = g.add_node(IRNode::TimerWait {
            id: "cooling_off".into(),
            spec: crate::ir::TimerSpec::Duration { ms: 259_200_000 },
        });
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        g.add_edge(s, w, IREdge { id: "e1".into(), condition: None });
        g.add_edge(w, end, IREdge { id: "e2".into(), condition: None });

        let plan = project_ir(&g, "wf".into()).expect("TimerWait must project");
        let ExecutionNode::Wait(wait) = plan.nodes.get("cooling_off").unwrap() else {
            panic!("cooling_off must be a Wait node");
        };
        assert!(matches!(wait.spec, crate::ir::TimerSpec::Duration { ms: 259_200_000 }));
        assert_eq!(wait.next, "end");
        assert!(plan.mathematically_proved());
    }

    /// RED (WS-D D1): a guard attached to a host that never projected
    /// must refuse — never evaporate.
    #[test]
    fn guard_on_unprojected_host_is_refused_not_dropped() {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let t = g.add_node(task("t1"));
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        let bt = g.add_node(IRNode::BoundaryTimer {
            id: "bt".into(),
            attached_to: "ghost".into(),
            spec: crate::ir::TimerSpec::Duration { ms: 1000 },
            interrupting: true,
            failure_budget: None,
        });
        let esc_end = g.add_node(IRNode::End { id: "end_esc".into(), terminate: false });
        g.add_edge(s, t, IREdge { id: "e1".into(), condition: None });
        g.add_edge(t, end, IREdge { id: "e2".into(), condition: None });
        g.add_edge(bt, esc_end, IREdge { id: "g1".into(), condition: None });

        let err = project_ir(&g, "wf".into())
            .expect_err("a guard on a nonexistent host must be refused");
        assert!(matches!(
            err,
            IrPlanError::GuardHostUnprojected { ref host, .. } if host == "ghost"
        ));
    }

    /// RED (WS-D D1): a condition on the escape edge has no field in
    /// GuardExecSpec — refused, never dropped.
    #[test]
    fn condition_on_escape_edge_is_refused() {
        let mut g = guarded_graph(true, None);
        // Rebuild the escape edge with a condition.
        let bt = g
            .node_indices()
            .find(|&i| matches!(&g[i], IRNode::BoundaryTimer { .. }))
            .unwrap();
        let esc_edge = g.edges_directed(bt, Direction::Outgoing).next().unwrap().id();
        g.remove_edge(esc_edge);
        let esc = g.node_indices().find(|&i| g[i].id() == "escalate").unwrap();
        g.add_edge(
            bt,
            esc,
            IREdge {
                id: "g1".into(),
                condition: Some(ConditionExpr {
                    flag_name: "@flag".into(),
                    op: ConditionOp::Eq,
                    literal: ConditionLiteral::Bool(true),
                }),
            },
        );
        let err = project_ir(&g, "wf".into())
            .expect_err("a conditioned escape edge must be refused");
        assert!(matches!(
            err,
            IrPlanError::UnrepresentableCondition { kind: "BoundaryTimer", .. }
        ));
    }

    /// Message waits are structural plan nodes, never fake service tasks.
    #[test]
    fn still_unsupported_kinds_remain_refused() {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let m = g.add_node(IRNode::MessageWait {
            id: "mw".into(),
            name: "mw".into(),
            corr_key_source: "docs_received".into(),
        });
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        g.add_edge(s, m, IREdge { id: "e1".into(), condition: None });
        g.add_edge(m, end, IREdge { id: "e2".into(), condition: None });

        let plan = project_ir(&g, "wf".into()).expect("MessageWait must project");
        let ExecutionNode::MessageWait(wait) = plan.nodes.get("mw").unwrap() else {
            panic!("mw must be a MessageWait node");
        };
        assert_eq!(wait.name, "mw");
        assert_eq!(wait.correlation_key_source, "docs_received");
        assert_eq!(wait.next, "end");
    }

    /// RED: no Start node at all.
    #[test]
    fn missing_start_is_refused() {
        let mut g: IRGraph = IRGraph::new();
        g.add_node(IRNode::End { id: "end".into(), terminate: false });
        assert!(matches!(project_ir(&g, "wf".into()), Err(IrPlanError::MissingStart)));
    }

    /// RED (blind-review finding, BLOCKER): `IREdge.condition` is a generic
    /// field on every edge, not just diverging-gateway edges —
    /// `Operation::Connect` can attach one between any two existing nodes.
    /// A condition on a plain Task's outgoing edge has no field in
    /// `TaskExecNode` to represent it — must be refused, never silently
    /// dropped (the previous behavior: `single_successor` returned only
    /// the successor id and never inspected the edge's condition).
    #[test]
    fn condition_on_non_gateway_edge_is_refused_not_dropped() {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let t = g.add_node(task("t1"));
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        g.add_edge(s, t, IREdge { id: "e1".into(), condition: None });
        g.add_edge(
            t,
            end,
            IREdge {
                id: "e2".into(),
                condition: Some(ConditionExpr {
                    flag_name: "@flag".into(),
                    op: ConditionOp::Eq,
                    literal: ConditionLiteral::Bool(true),
                }),
            },
        );

        let err = project_ir(&g, "wf".into())
            .expect_err("a condition on a Task's outgoing edge must be refused");
        assert!(matches!(
            err,
            IrPlanError::UnrepresentableCondition { kind: "ServiceTask", .. }
        ));
    }

    /// RED (blind-review finding, CONCERN): two parallel edges from the
    /// same fork to the same successor must never let the second edge's
    /// condition be silently replaced by the first edge's (the
    /// `neighbors_directed` + `find_edge` bug this test would have caught:
    /// `find_edge` always resolves to the FIRST edge between a pair, so a
    /// differently-conditioned second edge to the same target was
    /// misattributed rather than represented or refused).
    #[test]
    fn parallel_edges_to_same_successor_do_not_misattribute_conditions() {
        let mut g: IRGraph = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let fork = g.add_node(IRNode::GatewayInclusive {
            id: "fork".into(),
            name: "fork".into(),
            direction: GatewayDirection::Diverging,
        });
        let a = g.add_node(task("a"));
        // A genuine second branch is required — a fork with only ONE
        // distinct successor (even via two parallel edges) immediately
        // post-dominates at that successor, not at any Converging gateway,
        // so gateway_pairs correctly refuses it as unmatched before this
        // test's flow-building code ever runs. b keeps the fork a real
        // multi-branch split so pairing resolves to `join` as intended.
        let b = g.add_node(task("b"));
        let join = g.add_node(IRNode::GatewayInclusive {
            id: "join".into(),
            name: "join".into(),
            direction: GatewayDirection::Converging,
        });
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        g.add_edge(s, fork, IREdge { id: "e0".into(), condition: None });
        // Two parallel edges fork -> a: first unconditioned, second with a
        // non-Eq operator. If the second edge's condition were silently
        // shadowed by the first (unconditioned) edge, this would wrongly
        // ADMIT instead of refusing — the bug would manifest as a false
        // green, which is why this test asserts the refusal specifically.
        g.add_edge(fork, a, IREdge { id: "e1a".into(), condition: None });
        g.add_edge(
            fork,
            a,
            IREdge {
                id: "e1b".into(),
                condition: Some(ConditionExpr {
                    flag_name: "@flag".into(),
                    op: ConditionOp::Neq,
                    literal: ConditionLiteral::Bool(true),
                }),
            },
        );
        g.add_edge(fork, b, IREdge { id: "e1c".into(), condition: None });
        g.add_edge(a, join, IREdge { id: "e2".into(), condition: None });
        g.add_edge(b, join, IREdge { id: "e2b".into(), condition: None });
        g.add_edge(join, end, IREdge { id: "e3".into(), condition: None });

        let err = project_ir(&g, "wf".into())
            .expect_err("the second parallel edge's Neq condition must not be silently dropped");
        assert!(matches!(err, IrPlanError::UnsupportedConditionOperator { .. }));
    }

    /// G5.4a (GREEN, regression): `IRNode::MultiInstance` used to fall into
    /// the `UnsupportedNode` catch-all — a graph-backed session containing
    /// one could never reach `WorkflowExecutionPlan`, and therefore never
    /// the save-as-template REST endpoint's success path. Confirms the
    /// projected `ExecutionNode::MultiInstance` carries every field
    /// verbatim.
    #[test]
    fn multi_instance_projects_instead_of_refusing() {
        let mut g = IRGraph::new();
        let s = g.add_node(IRNode::Start { id: "start".into() });
        let mi = g.add_node(IRNode::MultiInstance {
            id: "verify_each".into(),
            name: "verify_each".into(),
            task_type: "verify_director".into(),
            collection_flag_name: "directors".into(),
            declared_max: 5,
            inputs: Vec::new(),
        });
        let end = g.add_node(IRNode::End { id: "end".into(), terminate: false });
        g.add_edge(s, mi, IREdge { id: "e1".into(), condition: None });
        g.add_edge(mi, end, IREdge { id: "e2".into(), condition: None });

        let plan = project_ir(&g, "wf".into()).expect("MultiInstance must project, not refuse");
        let ExecutionNode::MultiInstance(node) = &plan.nodes()["verify_each"] else {
            panic!("expected ExecutionNode::MultiInstance, got {:?}", plan.nodes()["verify_each"]);
        };
        assert_eq!(node.task_type, "verify_director");
        assert_eq!(node.collection_flag_name, "directors");
        assert_eq!(node.declared_max, 5);
        assert_eq!(node.next, "end");
    }
}
