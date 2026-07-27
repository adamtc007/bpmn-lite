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
//! `Start`, `End`, `ServiceTask`, and `GatewayAnd`/`GatewayInclusive`
//! matched diverging/converging pairs (via the exposed [`gateway_pairs`]
//! pairing oracle — never a hand-rolled re-pairing). `DataObject` nodes
//! are structural-only (zero bytecode, per their own IR doc comment) and
//! are simply omitted from the projected plan.
//!
//! Explicitly OUT of scope, refused with a named [`IrPlanError`] rather
//! than guessed at:
//! - `GatewayXor` — has no `direction` field (unlike `GatewayAnd`/
//!   `GatewayInclusive`) and no compiler-exposed join-pairing oracle; its
//!   DSL counterpart's `join` id is an explicit AST annotation
//!   (`linter.rs`'s `NodeAst::Split.join`) with no IR equivalent. Adding
//!   XOR support needs its own traced join-inference design, not a guess.
//! - `BoundaryTimer`, `BoundaryError`, `MessageWait`, `HumanWait`,
//!   `SendTask`, `MultiInstance`, `TimerWait`, `FfiServiceTask` — none has
//!   an `ExecutionNode` representation in `WorkflowExecutionPlan` at all;
//!   confirmed by trace that `lint()` never constructs one for any of
//!   these kinds (they are XML/IR-authoring-only constructs the plan
//!   format has never had to represent).
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

            IRNode::DataObject { .. } => {
                // Structural-only, zero bytecode (ir.rs's own doc comment) —
                // no ExecutionNode representation needed or possible.
            }

            other => {
                return Err(IrPlanError::UnsupportedNode { id, kind: node_kind_name(other) });
            }
        }
    }

    let mut plan = WorkflowExecutionPlan {
        workflow_id,
        nodes,
        start_node: ir[start_idx].id().to_owned(),
        placeholder_schema: PlaceholderSchema::default(),
        closure_manifest: Some(serde_json::json!({ "dependencies": [] })),
        regime_version: std::env::var("BPMN_LITE_REGIME_VERSION").ok(),
        mathematically_proved: true,
        unsafe_breeches: Vec::new(),
        compiled_bytecode: None,
    };
    plan.analyze_safety();

    validate_dag(&plan).map_err(IrPlanError::DagInvalid)?;

    Ok(plan)
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

    /// RED: v2-only node kinds (guards/waits/MI/FFI) have no
    /// WorkflowExecutionPlan representation — must fail closed, never a
    /// lossy shoehorn.
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

        let err = project_ir(&g, "wf".into()).expect_err("BoundaryTimer must be refused");
        assert!(matches!(
            err,
            IrPlanError::UnsupportedNode { kind: "BoundaryTimer", .. }
        ));
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
}
