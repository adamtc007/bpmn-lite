//! `IRGraph` → canonical `bpmn-dsl` source emission.
//!
//! EOP-PLAN-GRAPH-DSL-BRIDGE-001 B1: the third sanctioned projection off
//! `IRGraph`, sibling to [`crate::dsl::ir_plan`] (IR → plan) and aligned
//! with its refusal posture — **deliberately conservative, fails closed
//! beyond the core** (no lossy encoding, no silent skipping, no
//! `ServiceTask`-faking of unrepresented kinds). Supported: `Start`,
//! `End` (terminate via the `"terminated"` status sentinel, the same
//! pair `ir_plan` writes and `frontend` reads), `ServiceTask`,
//! `MessageWait`, matched `GatewayAnd` diverging/converging pairs (via
//! the exposed [`gateway_pairs`] oracle — never hand-rolled re-pairing),
//! `TimerWait` (D2, ordinary sequence node, all three `TimerSpec`
//! shapes), `MultiInstance` (D3, ordinary sequence node; representable
//! only when its per-element `inputs` is empty — the D3.0 freeze's ruled
//! (b) — a non-empty `inputs` refuses via
//! [`DslEmitError::InputsUnrepresentable`] rather than silently dropping
//! the bindings), and — since D1 (EOP-PLAN-DSL-PARITY-001) —
//! `BoundaryTimer`/`BoundaryError` guards on ServiceTask hosts (decoration forms
//! emitted directly after their host; stage 0 runs over the EFFECTIVE
//! graph of flow + escape + implicit host→guard edges, with a hard
//! totality assert — refuse, never truncate). Every other `IRNode` kind
//! refuses with a named
//! [`DslEmitError`]; the match below has NO wildcard arm, so a new
//! `IRNode` variant breaks this compile rather than falling through.
//!
//! Canonical form (frozen at Gate B0; changing any rule is a version bump
//! of the bridge contract, not a patch):
//! - node order: topological from the unique `Start`, ties broken by BPMN
//!   id (lexicographic, byte-wise);
//! - split flows: ordered by outgoing edge id (lexicographic);
//! - BPMN ids/task_types/message fields pass through verbatim as bare
//!   Symbol tokens — a string that does not lex as one refuses
//!   ([`DslEmitError::UnrepresentableToken`]), it is never renamed or
//!   quoted around;
//! - text: [`ToSexpr`], the DSL's only printer.
//!
//! Refusal ordering (frozen at B0, amended after its blind review): two
//! stages, first refusal wins, so the same graph always yields the same
//! refusal. Stage 0 whole-graph pre-checks in fixed order (`MissingStart`
//! → `MultipleStarts` → `DuplicateNodeId` → `CyclicGraph` →
//! `UnreachableNode`), then `ProcessDeclUnrepresentable`; only after
//! these does a canonical order exist for the Stage-1 per-node scan.
//!
//! The equivalence contract (proven at B2, stated here): compiling the
//! emitted source with a registry that declares every
//! [`EmittedDsl::required_symbols`] entry with an **empty** `BindingDecl`
//! yields a `WorkflowExecutionPlan` equal field-by-field (spans excluded
//! — `ir_plan` stamps `None`, the DSL path stamps source positions) to
//! `project_ir` of the same graph. The empty-decl registry is the honest
//! mirror of "no catalogue signal exists for graph-authored tasks" that
//! `ir_plan`'s own `derive_delivery_mode(None, false, false)` call
//! already encodes; compiling against any other registry is outside the
//! contract. Process-level declarations (`default_guard_budget`/
//! `default_retry_policy`) have no DSL syntax (Gate B0 grammar audit) —
//! a graph that sets either refuses, never silently drops.

use std::collections::{BTreeMap, BTreeSet};

use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::Direction;

use crate::gateway_pairs;
use crate::ir::{GatewayDirection, IRGraph, IRNode};

use super::ast::{
    EndAst, MessageWaitAst, MultiInstanceAst, NodeAst, SplitAst, SplitFlowAst, SplitModeAst,
    StartAst, TaskAst, TimerWaitAst,
    WorkflowSource,
};
use super::refactor::ToSexpr;

/// Why an `IRGraph` was refused emission to canonical `bpmn-dsl` source.
/// Frozen at Gate B0 (see the B0 receipt's catalogue table); additions
/// require a receipt amendment, not a silent new variant.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DslEmitError {
    #[error("graph has no Start node")]
    MissingStart,
    #[error("graph has {} Start nodes ({ids:?}), expected exactly one", ids.len())]
    MultipleStarts { ids: Vec<String> },
    #[error("duplicate BPMN node id '{id}' — emission to an id-keyed source form would silently merge them")]
    DuplicateNodeId { id: String },
    #[error("graph is cyclic (witness node '{id}') — canonical topological emission order is undefined")]
    CyclicGraph { id: String },
    #[error("node '{id}' is not reachable from Start — refusing rather than silently omitting it")]
    UnreachableNode { id: String },
    #[error("process-level declaration '{field}' is set, but the DSL grammar has no syntax to carry it — refusing rather than silently dropping it")]
    ProcessDeclUnrepresentable { field: &'static str },
    #[error("node '{id}' ({kind}) has no bpmn-dsl representation yet")]
    UnsupportedNode { id: String, kind: &'static str },
    #[error("node '{node_id}' field '{field}' value {value:?} does not lex as a DSL Symbol token — ids and pass-through strings are emitted verbatim, never renamed")]
    UnrepresentableToken {
        node_id: String,
        field: &'static str,
        value: String,
    },
    #[error("node '{id}' has {count} outgoing edge(s), expected {expected} for its kind")]
    WrongOutDegree {
        id: String,
        count: usize,
        expected: usize,
    },
    #[error("gateway '{id}' has no matching join (only GatewayAnd diverging/converging pairs are emittable)")]
    UnmatchedGateway { id: String },
    #[error("edge '{edge_id}' on parallel gateway '{gateway_id}' carries a condition — the DSL grammar cannot express a condition on an And-split flow")]
    ConditionOnParallelFlow {
        gateway_id: String,
        edge_id: String,
    },
    #[error("an outgoing edge of node '{id}' carries a condition, but its emitted DSL form has no field to represent one")]
    UnrepresentableCondition { id: String },
    #[error("guard '{guard_id}' is attached to '{host}' ({host_kind}) — boundary guards emit only on ServiceTask hosts (mirror of the projection's GuardHostUnprojected restriction)")]
    GuardOnUnsupportedHost {
        guard_id: String,
        host: String,
        host_kind: &'static str,
    },
    #[error("edge '{edge_id}' flows INTO guard '{guard_id}' — a boundary guard is a decoration with no incoming sequence flow; the grammar cannot target one")]
    FlowIntoGuard { guard_id: String, edge_id: String },
    #[error("guard '{guard_id}' carries failure budget 0 — a zero budget no-ops the guard, and the emitted source would refuse at lint (D1 axis R31); refusing at emission keeps emit-green ⇒ recompile-green")]
    GuardBudgetZero { guard_id: String },
    #[error("guard '{guard_id}' is an interrupting cycle timer — contradictory (a cycle timer rearms by definition), and the emitted source would refuse at lint (D1 axis R33)")]
    InterruptingCycleTimer { guard_id: String },
    #[error("node '{id}' carries {count} per-element input binding(s), but the DSL grammar has no syntax to represent them — refusing rather than silently dropping them (D3.0 freeze §2, ruled (b))")]
    InputsUnrepresentable { id: String, count: usize },
}

/// Whether the source `DesignerDag` carries process-level declarations
/// that `to_ir()` does not project. Set-ness only — the values have no
/// DSL representation, so emission needs nothing more than "is it set"
/// to refuse honestly (the concrete types live in `designer-graph`,
/// which this crate sits below).
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessLevelDecls {
    pub default_guard_budget_set: bool,
    pub default_retry_policy_set: bool,
}

/// A successful emission: canonical source, the AST it was printed from,
/// and the distinct task-type symbols the equivalence registry must
/// declare (with empty bindings) for the source to compile in-contract.
#[derive(Debug, Clone)]
pub struct EmittedDsl {
    pub source: String,
    pub ast: WorkflowSource,
    pub required_symbols: Vec<String>,
}

/// Does `s` lex as a single DSL Symbol token? Mirrors
/// `lexer::is_symbol_start`/`is_symbol_continue` exactly (alnum/`_`/`=`/
/// `-` to start; continuation adds `.` and `:`).
fn is_symbol_token(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    let start_ok = first.is_ascii_alphanumeric() || matches!(first, b'_' | b'=' | b'-');
    start_ok
        && bytes[1..]
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'=' | b':'))
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

/// The zero span used for every constructed AST node: emission has no
/// source positions (spans are the ONLY field excluded from the B0
/// plan-equality table, on exactly this ground).
fn no_span() -> bpmn_lite_types::SourceSpan {
    bpmn_lite_types::SourceSpan::new(0, 0)
}

fn is_guard(node: &IRNode) -> bool {
    matches!(
        node,
        IRNode::BoundaryTimer { .. } | IRNode::BoundaryError { .. }
    )
}

/// D1: guards attached per host index, guard indices sorted by guard id,
/// plus the guards whose `attached_to` resolves to no node (dangling —
/// refused as unreachable in stage 0, but the cycle scan must still be
/// total over them).
fn collect_guards(
    ir: &IRGraph,
) -> (BTreeMap<NodeIndex, Vec<NodeIndex>>, Vec<NodeIndex>) {
    let id_to_idx: BTreeMap<&str, NodeIndex> =
        ir.node_indices().map(|i| (ir[i].id(), i)).collect();
    let mut by_host: BTreeMap<NodeIndex, Vec<NodeIndex>> = BTreeMap::new();
    let mut dangling = Vec::new();
    for idx in ir.node_indices() {
        let host = match &ir[idx] {
            IRNode::BoundaryTimer { attached_to, .. }
            | IRNode::BoundaryError { attached_to, .. } => attached_to,
            _ => continue,
        };
        match id_to_idx.get(host.as_str()) {
            Some(&h) => by_host.entry(h).or_default().push(idx),
            None => dangling.push(idx),
        }
    }
    for v in by_host.values_mut() {
        v.sort_by(|a, b| ir[*a].id().cmp(ir[*b].id()));
    }
    (by_host, dangling)
}

/// Canonical scan over the EFFECTIVE graph (D1.0 §3.1b/§3.2): flow +
/// escape edges plus one implicit host→guard edge per attachment.
/// Guard nodes never enter the ready set — a host's guards emit directly
/// after it, ordered by guard id (their escape edges then release
/// successors normally). `dangling_as_roots` is the stage-0
/// cycle-check mode: a guard whose host doesn't resolve is treated as a
/// root there so the totality verdict is about CYCLES, not about the
/// dangling attachment (which the reachability check refuses next, with
/// the right diagnostic).
fn canonical_scan(
    ir: &IRGraph,
    guards_of: &BTreeMap<NodeIndex, Vec<NodeIndex>>,
    dangling: &[NodeIndex],
    dangling_as_roots: bool,
) -> Vec<NodeIndex> {
    let guard_set: BTreeSet<NodeIndex> = ir
        .node_indices()
        .filter(|&i| is_guard(&ir[i]))
        .collect();
    let mut in_degree: BTreeMap<NodeIndex, usize> = ir
        .node_indices()
        .map(|idx| (idx, ir.edges_directed(idx, Direction::Incoming).count()))
        .collect();
    let mut ready: BTreeMap<String, NodeIndex> = in_degree
        .iter()
        .filter(|(idx, d)| **d == 0 && !guard_set.contains(idx))
        .map(|(idx, _)| (ir[*idx].id().to_owned(), *idx))
        .collect();
    if dangling_as_roots {
        for &g in dangling {
            if in_degree[&g] == 0 {
                ready.insert(ir[g].id().to_owned(), g);
            }
        }
    }
    let mut order = Vec::with_capacity(ir.node_count());
    // Emit one node: push it, release its out-edges, then emit its
    // attached guards directly after it (guard-id order), recursively —
    // "directly after the host" is the frozen canonical rule.
    fn emit_node(
        ir: &IRGraph,
        idx: NodeIndex,
        guards_of: &BTreeMap<NodeIndex, Vec<NodeIndex>>,
        guard_set: &BTreeSet<NodeIndex>,
        in_degree: &mut BTreeMap<NodeIndex, usize>,
        ready: &mut BTreeMap<String, NodeIndex>,
        order: &mut Vec<NodeIndex>,
    ) {
        order.push(idx);
        for edge in ir.edges_directed(idx, Direction::Outgoing) {
            let tgt = edge.target();
            let d = in_degree.get_mut(&tgt).expect("target tracked");
            // Each source emits exactly once, so a zero here would mean a
            // double-decrement — keep that failure loud rather than letting
            // saturation mask a wrong-yet-total order past the totality check.
            debug_assert!(*d > 0, "in-degree double-decrement at '{}'", ir[tgt].id());
            *d = d.saturating_sub(1);
            if *d == 0 && !guard_set.contains(&tgt) {
                ready.insert(ir[tgt].id().to_owned(), tgt);
            }
        }
        if let Some(gs) = guards_of.get(&idx) {
            for &g in gs {
                emit_node(ir, g, guards_of, guard_set, in_degree, ready, order);
            }
        }
    }
    while let Some((id, idx)) = ready.iter().next().map(|(k, v)| (k.clone(), *v)) {
        ready.remove(&id);
        emit_node(
            ir,
            idx,
            guards_of,
            &guard_set,
            &mut in_degree,
            &mut ready,
            &mut order,
        );
    }
    order
}

/// Emit canonical `bpmn-dsl` source for an admitted core-5 `IRGraph`.
///
/// See the module doc for the frozen contract. On refusal, nothing is
/// emitted and the graph is untouched (`&IRGraph` — enforced by the
/// signature).
pub fn emit_dsl(
    ir: &IRGraph,
    workflow_id: &str,
    decls: &ProcessLevelDecls,
) -> Result<EmittedDsl, DslEmitError> {
    // ── Stage 0: whole-graph pre-checks, fixed order ─────────────────────
    let start_ids: Vec<String> = ir
        .node_indices()
        .filter(|&idx| matches!(ir[idx], IRNode::Start { .. }))
        .map(|idx| ir[idx].id().to_owned())
        .collect();
    if start_ids.is_empty() {
        return Err(DslEmitError::MissingStart);
    }
    if start_ids.len() > 1 {
        let mut ids = start_ids;
        ids.sort();
        return Err(DslEmitError::MultipleStarts { ids });
    }

    {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut dups: BTreeSet<&str> = BTreeSet::new();
        for idx in ir.node_indices() {
            let id = ir[idx].id();
            if !seen.insert(id) {
                dups.insert(id);
            }
        }
        if let Some(id) = dups.into_iter().next() {
            // BTreeSet iteration → smallest duplicate id, deterministic
            // regardless of petgraph arena order.
            return Err(DslEmitError::DuplicateNodeId { id: id.to_owned() });
        }
    }

    // D1: guard attachments participate in stage 0 — collect first.
    let (guards_of, dangling_guards) = collect_guards(ir);

    // A guard with an incoming sequence-flow edge is unschedulable (it
    // emits after its host, not via flow) and untargetable in the
    // grammar — refuse by name, smallest guard id then smallest edge id
    // (deterministic). D1.0 amendment recorded in the D1 receipt.
    {
        let mut hits: Vec<(String, String)> = ir
            .node_indices()
            .filter(|&i| is_guard(&ir[i]))
            .flat_map(|g| {
                ir.edges_directed(g, Direction::Incoming)
                    .map(move |e| (ir[g].id().to_owned(), e.weight().id.clone()))
            })
            .collect();
        hits.sort();
        if let Some((guard_id, edge_id)) = hits.into_iter().next() {
            return Err(DslEmitError::FlowIntoGuard { guard_id, edge_id });
        }
    }

    // Cycle check over the EFFECTIVE graph (D1.0 §3.1b — flow + escape +
    // implicit host→guard edges, exactly the graph the emission scan
    // walks and validate_dag's adjacency mirrors): the scan itself, in
    // dangling-as-roots mode, IS the check — leftover nodes mean a cycle
    // in the effective graph (an escape edge back into the guard's own
    // host/ancestor deadlocks the schedule even though plain toposort
    // over flow edges alone is acyclic, since attachment is a field,
    // not an edge). Witness: smallest unscheduled BPMN id.
    {
        let probe = canonical_scan(ir, &guards_of, &dangling_guards, true);
        if probe.len() != ir.node_count() {
            let scheduled: BTreeSet<NodeIndex> = probe.into_iter().collect();
            let mut leftover: Vec<&str> = ir
                .node_indices()
                .filter(|i| !scheduled.contains(i))
                .map(|i| ir[i].id())
                .collect();
            leftover.sort();
            return Err(DslEmitError::CyclicGraph {
                id: leftover
                    .first()
                    .map(|s| (*s).to_owned())
                    .unwrap_or_default(),
            });
        }
    }

    let start_idx = ir
        .node_indices()
        .find(|&idx| matches!(ir[idx], IRNode::Start { .. }))
        .expect("start existence checked above");
    {
        // D1.0 §3.1 reachability fixpoint: flow-DFS from Start, plus
        // guards attached to reachable hosts, plus flow-DFS from each
        // such guard's escape edge, to fixpoint. Dangling-host guards
        // are never reachable — refused here with the right diagnostic
        // (not as a phantom cycle).
        let mut reachable: BTreeSet<NodeIndex> = BTreeSet::new();
        let mut stack = vec![start_idx];
        loop {
            while let Some(idx) = stack.pop() {
                if reachable.insert(idx) {
                    stack.extend(ir.neighbors_directed(idx, Direction::Outgoing));
                }
            }
            let mut grew = false;
            for (host, gs) in &guards_of {
                if reachable.contains(host) {
                    for &g in gs {
                        if !reachable.contains(&g) {
                            stack.push(g);
                            grew = true;
                        }
                    }
                }
            }
            if !grew {
                break;
            }
        }
        let mut unreachable: Vec<&str> = ir
            .node_indices()
            .filter(|idx| !reachable.contains(idx))
            .map(|idx| ir[idx].id())
            .collect();
        unreachable.sort();
        if let Some(id) = unreachable.first() {
            return Err(DslEmitError::UnreachableNode {
                id: (*id).to_owned(),
            });
        }
    }

    if decls.default_guard_budget_set {
        return Err(DslEmitError::ProcessDeclUnrepresentable {
            field: "default_guard_budget",
        });
    }
    if decls.default_retry_policy_set {
        return Err(DslEmitError::ProcessDeclUnrepresentable {
            field: "default_retry_policy",
        });
    }

    if !is_symbol_token(workflow_id) {
        return Err(DslEmitError::UnrepresentableToken {
            node_id: "<workflow>".to_owned(),
            field: "workflow_id",
            value: workflow_id.to_owned(),
        });
    }

    // ── Stage 1: per-node scan in canonical order ────────────────────────
    let order = canonical_scan(ir, &guards_of, &dangling_guards, false);
    // Hard totality assert (D1.0 §3.1b): refuse, never truncate. After
    // the effective-graph cycle check and the reachability fixpoint this
    // cannot fire — it is the guard rail the freeze mandates while the
    // scan mechanism carries guards.
    if order.len() != ir.node_count() {
        let scheduled: BTreeSet<NodeIndex> = order.iter().copied().collect();
        let mut leftover: Vec<&str> = ir
            .node_indices()
            .filter(|i| !scheduled.contains(i))
            .map(|i| ir[i].id())
            .collect();
        leftover.sort();
        return Err(DslEmitError::CyclicGraph {
            id: leftover
                .first()
                .map(|s| (*s).to_owned())
                .unwrap_or_default(),
        });
    }
    let id_to_idx: BTreeMap<&str, NodeIndex> =
        ir.node_indices().map(|i| (ir[i].id(), i)).collect();

    let pairs = gateway_pairs(ir);
    // Reverse map, plus the set of SHARED joins: `gateway_pairs` pairs
    // every diverging And with its immediate post-dominator, so a
    // non-SESE shape where several splits share one converging And
    // produces duplicate join keys — last-write-wins over HashMap
    // iteration order made emission NONDETERMINISTIC (three distinct
    // sources for one graph, all recompiling; B1 blind-review finding 3).
    // A join with more than one paired split has no unique `:split` to
    // print — it refuses as UnmatchedGateway when the canonical scan
    // reaches it (keeping the frozen per-node refusal order intact),
    // never picks one.
    let mut join_to_split: BTreeMap<NodeIndex, NodeIndex> = BTreeMap::new();
    let mut shared_joins: BTreeSet<NodeIndex> = BTreeSet::new();
    for (s, j) in pairs.iter() {
        if join_to_split.insert(*j, *s).is_some() {
            shared_joins.insert(*j);
        }
    }

    // Exactly-one-outgoing check for the kinds that require it — degree
    // only; the condition check is a SEPARATE, later step per the frozen
    // per-node order (WrongOutDegree → UnmatchedGateway → conditions).
    let single_out_edge = |idx: NodeIndex| -> Result<petgraph::graph::EdgeIndex, DslEmitError> {
        let id = ir[idx].id();
        let edges: Vec<_> = ir.edges_directed(idx, Direction::Outgoing).collect();
        if edges.len() != 1 {
            return Err(DslEmitError::WrongOutDegree {
                id: id.to_owned(),
                count: edges.len(),
                expected: 1,
            });
        }
        Ok(edges[0].id())
    };
    // Condition refusal + successor id for a checked single edge.
    let uncond_next = |idx: NodeIndex,
                       eidx: petgraph::graph::EdgeIndex|
     -> Result<String, DslEmitError> {
        if ir[eidx].condition.is_some() {
            return Err(DslEmitError::UnrepresentableCondition {
                id: ir[idx].id().to_owned(),
            });
        }
        let (_, tgt) = ir.edge_endpoints(eidx).expect("edge endpoints");
        Ok(ir[tgt].id().to_owned())
    };

    let check_token = |node_id: &str, field: &'static str, value: &str| {
        if is_symbol_token(value) {
            Ok(())
        } else {
            Err(DslEmitError::UnrepresentableToken {
                node_id: node_id.to_owned(),
                field,
                value: value.to_owned(),
            })
        }
    };

    let mut nodes: Vec<NodeAst> = Vec::with_capacity(order.len());
    let mut required_symbols: BTreeSet<String> = BTreeSet::new();

    // Frozen per-node check order (B0, corrected by B1's blind review —
    // the first cut checked the id token before the kind and pairing
    // before out-degree): UnsupportedNode → UnrepresentableToken →
    // WrongOutDegree → UnmatchedGateway → conditions. The out-of-core
    // arm therefore returns FIRST, before any token check; each in-core
    // arm runs its own checks in exactly that order.
    for idx in order {
        let node = &ir[idx];
        let id = node.id().to_owned();
        match node {
            IRNode::Start { .. } => {
                check_token(&id, "id", &id)?;
                let e = single_out_edge(idx)?;
                nodes.push(NodeAst::Start(StartAst {
                    next: uncond_next(idx, e)?,
                    id,
                    span: no_span(),
                }));
            }
            IRNode::End { terminate, .. } => {
                check_token(&id, "id", &id)?;
                let out = ir.edges_directed(idx, Direction::Outgoing).count();
                if out != 0 {
                    return Err(DslEmitError::WrongOutDegree {
                        id,
                        count: out,
                        expected: 0,
                    });
                }
                nodes.push(NodeAst::End(EndAst {
                    // The exact sentinel pair ir_plan writes and
                    // frontend.rs reads back — never any other string.
                    status: if *terminate { "terminated" } else { "completed" }.to_owned(),
                    id,
                    span: no_span(),
                }));
            }
            IRNode::ServiceTask { task_type, .. } => {
                // `name` has no TaskAst field and is plan-invisible on
                // both paths (project_ir drops it too) — documented
                // authoring-metadata loss, per the B0 receipt.
                check_token(&id, "id", &id)?;
                check_token(&id, "task_type", task_type)?;
                let e = single_out_edge(idx)?;
                required_symbols.insert(task_type.clone());
                nodes.push(NodeAst::Task(TaskAst {
                    next: uncond_next(idx, e)?,
                    id,
                    plug: task_type.clone(),
                    args: Vec::new(),
                    delivery_mode: None,
                    span: no_span(),
                    loop_origin: None,
                }));
            }
            IRNode::MessageWait {
                name,
                corr_key_source,
                ..
            } => {
                check_token(&id, "id", &id)?;
                check_token(&id, "name", name)?;
                check_token(&id, "corr_key_source", corr_key_source)?;
                let e = single_out_edge(idx)?;
                nodes.push(NodeAst::MessageWait(MessageWaitAst {
                    next: uncond_next(idx, e)?,
                    id,
                    name: name.clone(),
                    correlation_source: corr_key_source.clone(),
                    span: no_span(),
                }));
            }
            // D2 (EOP-PLAN-DSL-PARITY-001): ordinary sequence node — all
            // three TimerSpec shapes representable (parity: ir_plan
            // projects any spec). No semantic checks beyond topology;
            // `max_fires: 0` passes on BOTH paths (surfaced in the D2.0
            // freeze for a separate symmetric ruling).
            IRNode::TimerWait { spec, .. } => {
                check_token(&id, "id", &id)?;
                let e = single_out_edge(idx)?;
                nodes.push(NodeAst::TimerWait(TimerWaitAst {
                    next: uncond_next(idx, e)?,
                    id,
                    spec: spec.clone(),
                    span: no_span(),
                }));
            }
            // D3 (EOP-PLAN-DSL-PARITY-001): ordinary sequence node (no
            // split/join pairing, mirrors TimerWait). `name` has no
            // MultiInstanceAst field and is plan-invisible on both paths
            // (`MultiInstanceExecNode` carries neither `name` nor
            // `inputs` — G5.4a). `inputs` is checked here per the D3.0
            // freeze's ruled (b): non-empty refuses rather than silently
            // dropping the bindings. `declared_max: 0` passes on both
            // paths (surfaced in the D3.0 freeze for a separate symmetric
            // ruling, same class as D2.0's `max_fires: 0`).
            IRNode::MultiInstance {
                task_type,
                collection_flag_name,
                declared_max,
                inputs,
                ..
            } => {
                check_token(&id, "id", &id)?;
                check_token(&id, "task_type", task_type)?;
                check_token(&id, "collection_flag_name", collection_flag_name)?;
                if !inputs.is_empty() {
                    return Err(DslEmitError::InputsUnrepresentable {
                        id,
                        count: inputs.len(),
                    });
                }
                // No `required_symbols` entry: unlike `ServiceTask.plug`,
                // `task_type` here is never resolved against the
                // placeholder registry by the linter (verified: neither
                // this tranche's `NodeAst::MultiInstance` lowering arm nor
                // `ir_plan`'s G5.4a projection performs such a lookup) —
                // adding one would be a misleading no-op requirement.
                let e = single_out_edge(idx)?;
                nodes.push(NodeAst::MultiInstance(MultiInstanceAst {
                    next: uncond_next(idx, e)?,
                    id,
                    task_type: task_type.clone(),
                    collection_flag_name: collection_flag_name.clone(),
                    declared_max: *declared_max,
                    span: no_span(),
                }));
            }
            IRNode::GatewayAnd { direction, .. } => match direction {
                GatewayDirection::Diverging => {
                    check_token(&id, "id", &id)?;
                    // Out-degree before pairing (frozen order): a
                    // diverging gateway needs ≥1 outgoing flow.
                    let mut edges: Vec<_> =
                        ir.edges_directed(idx, Direction::Outgoing).collect();
                    if edges.is_empty() {
                        return Err(DslEmitError::WrongOutDegree {
                            id,
                            count: 0,
                            expected: 1,
                        });
                    }
                    let Some(&join_idx) = pairs.get(&idx) else {
                        return Err(DslEmitError::UnmatchedGateway { id });
                    };
                    // Flows in canonical edge order: sorted by edge id.
                    edges.sort_by(|a, b| a.weight().id.cmp(&b.weight().id));
                    let mut flows = Vec::with_capacity(edges.len());
                    for edge in edges {
                        if edge.weight().condition.is_some() {
                            return Err(DslEmitError::ConditionOnParallelFlow {
                                gateway_id: id,
                                edge_id: edge.weight().id.clone(),
                            });
                        }
                        flows.push(SplitFlowAst {
                            condition: None,
                            next: ir[edge.target()].id().to_owned(),
                        });
                    }
                    nodes.push(NodeAst::Split(SplitAst {
                        id,
                        mode: SplitModeAst::And,
                        plug: None,
                        flows,
                        join: ir[join_idx].id().to_owned(),
                        span: no_span(),
                    }));
                }
                GatewayDirection::Converging => {
                    check_token(&id, "id", &id)?;
                    // Out-degree before pairing (frozen order).
                    let e = single_out_edge(idx)?;
                    // A join shared by several paired splits has no
                    // unique `:split` — refuse, never pick one (B1
                    // blind-review finding 3: HashMap last-write-wins
                    // made this nondeterministic).
                    if shared_joins.contains(&idx) {
                        return Err(DslEmitError::UnmatchedGateway { id });
                    }
                    let Some(&split_idx) = join_to_split.get(&idx) else {
                        return Err(DslEmitError::UnmatchedGateway { id });
                    };
                    nodes.push(NodeAst::Join(super::ast::JoinAst {
                        next: uncond_next(idx, e)?,
                        id,
                        mode: super::ast::JoinModeAst::And,
                        split: ir[split_idx].id().to_owned(),
                        span: no_span(),
                    }));
                }
            },
            // D1: boundary guards emit as top-level decoration forms.
            // Per-guard checks in the frozen per-node order: kind gate
            // (this arm), token checks, host-kind, out-degree, condition.
            IRNode::BoundaryTimer {
                attached_to,
                spec,
                interrupting,
                failure_budget,
                ..
            } => {
                check_token(&id, "id", &id)?;
                let host_idx = *id_to_idx
                    .get(attached_to.as_str())
                    .expect("dangling attachments refused in stage 0");
                if !matches!(ir[host_idx], IRNode::ServiceTask { .. }) {
                    return Err(DslEmitError::GuardOnUnsupportedHost {
                        guard_id: id,
                        host: attached_to.clone(),
                        host_kind: node_kind_name(&ir[host_idx]),
                    });
                }
                if *failure_budget == Some(0) {
                    return Err(DslEmitError::GuardBudgetZero { guard_id: id });
                }
                if matches!(spec, crate::ir::TimerSpec::Cycle { .. }) && *interrupting {
                    return Err(DslEmitError::InterruptingCycleTimer { guard_id: id });
                }
                let e = single_out_edge(idx)?;
                nodes.push(NodeAst::BoundaryTimer(super::ast::BoundaryTimerAst {
                    next: uncond_next(idx, e)?,
                    id,
                    host: attached_to.clone(),
                    spec: spec.clone(),
                    interrupting: *interrupting,
                    budget: *failure_budget,
                    span: no_span(),
                }));
            }
            IRNode::BoundaryError {
                attached_to,
                error_code,
                failure_budget,
                ..
            } => {
                check_token(&id, "id", &id)?;
                // :error-code prints as a str-lit; the lexer has no
                // escapes, so a quote or control char breaks re-parse —
                // refuse (D1.0 amendment: the freeze's blanket "str-lit
                // exempt" was too wide, recorded in the D1 receipt).
                if let Some(code) = error_code {
                    if code.contains('"') || code.chars().any(|c| c.is_control()) {
                        return Err(DslEmitError::UnrepresentableToken {
                            node_id: id,
                            field: "error_code",
                            value: code.clone(),
                        });
                    }
                }
                let host_idx = *id_to_idx
                    .get(attached_to.as_str())
                    .expect("dangling attachments refused in stage 0");
                if !matches!(ir[host_idx], IRNode::ServiceTask { .. }) {
                    return Err(DslEmitError::GuardOnUnsupportedHost {
                        guard_id: id,
                        host: attached_to.clone(),
                        host_kind: node_kind_name(&ir[host_idx]),
                    });
                }
                if *failure_budget == Some(0) {
                    return Err(DslEmitError::GuardBudgetZero { guard_id: id });
                }
                let e = single_out_edge(idx)?;
                nodes.push(NodeAst::BoundaryError(super::ast::BoundaryErrorAst {
                    next: uncond_next(idx, e)?,
                    id,
                    host: attached_to.clone(),
                    error_code: error_code.clone(),
                    budget: *failure_budget,
                    span: no_span(),
                }));
            }

            // Out-of-core kinds — NO wildcard arm: a new IRNode variant
            // must break this compile, not fall through (B0's structural
            // fail-closed rule). 6 kinds remain out of core after D1
            // moved the two boundary kinds, D2 moved TimerWait, and D3
            // moved MultiInstance above.
            IRNode::GatewayXor { .. }
            | IRNode::GatewayInclusive { .. }
            | IRNode::HumanWait { .. }
            | IRNode::DataObject { .. }
            | IRNode::FfiServiceTask { .. }
            | IRNode::SendTask { .. } => {
                return Err(DslEmitError::UnsupportedNode {
                    id,
                    kind: node_kind_name(node),
                });
            }
        }
    }

    let ast = WorkflowSource {
        name: workflow_id.to_owned(),
        nodes,
    };
    let source = ast.to_sexpr(0);
    Ok(EmittedDsl {
        source,
        ast,
        required_symbols: required_symbols.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ConditionExpr, ConditionLiteral, ConditionOp, IREdge, TimerSpec};

    fn decls() -> ProcessLevelDecls {
        ProcessLevelDecls::default()
    }

    fn edge(id: &str) -> IREdge {
        IREdge {
            id: id.to_owned(),
            condition: None,
        }
    }

    fn cond_edge(id: &str) -> IREdge {
        IREdge {
            id: id.to_owned(),
            condition: Some(ConditionExpr {
                flag_name: "flag".to_owned(),
                op: ConditionOp::Eq,
                literal: ConditionLiteral::Bool(true),
            }),
        }
    }

    fn start(id: &str) -> IRNode {
        IRNode::Start { id: id.to_owned() }
    }

    fn end(id: &str, terminate: bool) -> IRNode {
        IRNode::End {
            id: id.to_owned(),
            terminate,
        }
    }

    fn task(id: &str, task_type: &str) -> IRNode {
        IRNode::ServiceTask {
            id: id.to_owned(),
            name: String::new(),
            task_type: task_type.to_owned(),
        }
    }

    fn msg(id: &str, name: &str, corr: &str) -> IRNode {
        IRNode::MessageWait {
            id: id.to_owned(),
            name: name.to_owned(),
            corr_key_source: corr.to_owned(),
        }
    }

    fn and_gw(id: &str, direction: GatewayDirection) -> IRNode {
        IRNode::GatewayAnd {
            id: id.to_owned(),
            name: String::new(),
            direction,
        }
    }

    /// start → task → end, the minimal green shape.
    fn linear_graph() -> IRGraph {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let t = ir.add_node(task("t1", "cbu.create"));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, t, edge("f1"));
        ir.add_edge(t, e, edge("f2"));
        ir
    }

    fn registry_for(emitted: &EmittedDsl) -> super::super::linter::StubPlaceholderRegistry {
        let mut reg = super::super::linter::StubPlaceholderRegistry::new();
        for sym in &emitted.required_symbols {
            reg.register_verb(sym, super::super::linter::BindingDecl::default());
        }
        reg
    }

    // ── Green fixtures: emit, recompile, idempotence ─────────────────────

    #[test]
    fn green_linear_emits_recompiles_idempotent() {
        let ir = linear_graph();
        let emitted = emit_dsl(&ir, "wf-linear", &decls()).expect("emit");
        assert_eq!(emitted.required_symbols, vec!["cbu.create".to_owned()]);
        super::super::compile(&emitted.source, &registry_for(&emitted))
            .expect("emitted source must recompile");
        let again = emit_dsl(&ir, "wf-linear", &decls()).expect("emit twice");
        assert_eq!(emitted.source, again.source, "emission must be idempotent");
    }

    #[test]
    fn green_message_wait_and_terminate_end() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let t = ir.add_node(task("t1", "cbu.create"));
        let m = ir.add_node(msg("wait-reply", "reply-received", "case-id"));
        let e = ir.add_node(end("end", true));
        ir.add_edge(s, t, edge("f1"));
        ir.add_edge(t, m, edge("f2"));
        ir.add_edge(m, e, edge("f3"));
        let emitted = emit_dsl(&ir, "wf-msg", &decls()).expect("emit");
        assert!(emitted.source.contains(":status \"terminated\""));
        super::super::compile(&emitted.source, &registry_for(&emitted)).expect("recompile");
    }

    #[test]
    fn green_and_block_two_branches() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let sp = ir.add_node(and_gw("split1", GatewayDirection::Diverging));
        let a = ir.add_node(task("branch-a", "cbu.a"));
        let b = ir.add_node(task("branch-b", "cbu.b"));
        let j = ir.add_node(and_gw("join1", GatewayDirection::Converging));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, sp, edge("f1"));
        ir.add_edge(sp, a, edge("f2"));
        ir.add_edge(sp, b, edge("f3"));
        ir.add_edge(a, j, edge("f4"));
        ir.add_edge(b, j, edge("f5"));
        ir.add_edge(j, e, edge("f6"));
        let emitted = emit_dsl(&ir, "wf-and", &decls()).expect("emit");
        assert!(emitted.source.contains("(split-and :id split1 :join join1"));
        super::super::compile(&emitted.source, &registry_for(&emitted)).expect("recompile");
    }

    fn timer(id: &str, spec: TimerSpec) -> IRNode {
        IRNode::TimerWait {
            id: id.to_owned(),
            spec,
        }
    }

    /// GREEN (D2): all three TimerSpec shapes emit the exact frozen
    /// forms, recompile through the derived registry, and are idempotent.
    #[test]
    fn green_timer_wait_all_three_shapes() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let d = ir.add_node(timer("w-dur", TimerSpec::Duration { ms: 1000 }));
        let a = ir.add_node(timer("w-date", TimerSpec::Date { deadline_ms: 999 }));
        let c = ir.add_node(timer(
            "w-cyc",
            TimerSpec::Cycle {
                interval_ms: 60_000,
                max_fires: 3,
            },
        ));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, d, edge("f1"));
        ir.add_edge(d, a, edge("f2"));
        ir.add_edge(a, c, edge("f3"));
        ir.add_edge(c, e, edge("f4"));
        let emitted = emit_dsl(&ir, "wf-timer", &decls()).expect("emit");
        assert!(emitted
            .source
            .contains("(timer-wait :id w-dur :duration-ms 1000 :next w-date)"));
        assert!(emitted
            .source
            .contains("(timer-wait :id w-date :deadline-ms 999 :next w-cyc)"));
        assert!(emitted
            .source
            .contains("(timer-wait :id w-cyc :cycle-ms 60000 :max-fires 3 :next end)"));
        super::super::compile(&emitted.source, &registry_for(&emitted)).expect("recompile");
        let again = emit_dsl(&ir, "wf-timer", &decls()).expect("emit twice");
        assert_eq!(emitted.source, again.source, "emission must be idempotent");
    }

    /// RED (D2 R-D2.5): out-degree 0 and 2 both refuse WrongOutDegree.
    #[test]
    fn red_timer_wait_out_degree_zero_and_two() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let w = ir.add_node(timer("w1", TimerSpec::Duration { ms: 5 }));
        ir.add_edge(s, w, edge("f1"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::WrongOutDegree { id, count: 0, .. }) => assert_eq!(id, "w1"),
            other => panic!("expected WrongOutDegree(0), got {other:?}"),
        }
        let e1 = ir.add_node(end("end-a", false));
        let e2 = ir.add_node(end("end-b", false));
        ir.add_edge(w, e1, edge("f2"));
        ir.add_edge(w, e2, edge("f3"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::WrongOutDegree { id, count: 2, .. }) => assert_eq!(id, "w1"),
            other => panic!("expected WrongOutDegree(2), got {other:?}"),
        }
    }

    /// RED (D2 R-D2.6): a conditioned outgoing edge refuses.
    #[test]
    fn red_timer_wait_conditioned_edge() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let w = ir.add_node(timer("w1", TimerSpec::Duration { ms: 5 }));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, w, edge("f1"));
        ir.add_edge(w, e, cond_edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnrepresentableCondition { id }) => assert_eq!(id, "w1"),
            other => panic!("expected UnrepresentableCondition, got {other:?}"),
        }
    }

    /// RED (D2 R-D2.7): a non-token id refuses UnrepresentableToken.
    #[test]
    fn red_timer_wait_bad_id_token() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let w = ir.add_node(timer("w 1", TimerSpec::Duration { ms: 5 }));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, w, edge("f1"));
        ir.add_edge(w, e, edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnrepresentableToken { node_id, field, .. }) => {
                assert_eq!(node_id, "w 1");
                assert_eq!(field, "id");
            }
            other => panic!("expected UnrepresentableToken, got {other:?}"),
        }
    }

    fn mi(id: &str, task_type: &str, collection: &str, declared_max: u32) -> IRNode {
        mi_with_inputs(id, task_type, collection, declared_max, Vec::new())
    }

    fn mi_with_inputs(
        id: &str,
        task_type: &str,
        collection: &str,
        declared_max: u32,
        inputs: Vec<crate::ir::FfiInputBinding>,
    ) -> IRNode {
        IRNode::MultiInstance {
            id: id.to_owned(),
            name: String::new(),
            task_type: task_type.to_owned(),
            collection_flag_name: collection.to_owned(),
            declared_max,
            inputs,
        }
    }

    /// GREEN (D3): emits the frozen form, recompiles through the derived
    /// registry (no `required_symbols` entry — verified neither lowering
    /// path resolves `task_type` against the placeholder registry), and
    /// is idempotent.
    #[test]
    fn green_multi_instance_declared_max_round_trip() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let m = ir.add_node(mi("review-all", "review-doc", "docs", 50));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, m, edge("f1"));
        ir.add_edge(m, e, edge("f2"));
        let emitted = emit_dsl(&ir, "wf-mi", &decls()).expect("emit");
        assert!(emitted.source.contains(
            "(multi-instance :id review-all :task-type review-doc :collection docs :max 50 :next end)"
        ));
        assert!(
            emitted.required_symbols.is_empty(),
            "task_type is not registry-resolved on either path"
        );
        super::super::compile(&emitted.source, &registry_for(&emitted)).expect("recompile");
        let again = emit_dsl(&ir, "wf-mi", &decls()).expect("emit twice");
        assert_eq!(emitted.source, again.source, "emission must be idempotent");
    }

    /// RED (D3 R-D3.5): out-degree 0 and 2 both refuse WrongOutDegree.
    #[test]
    fn red_multi_instance_out_degree_zero_and_two() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let m = ir.add_node(mi("m1", "review-doc", "docs", 10));
        ir.add_edge(s, m, edge("f1"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::WrongOutDegree { id, count: 0, .. }) => assert_eq!(id, "m1"),
            other => panic!("expected WrongOutDegree(0), got {other:?}"),
        }
        let e1 = ir.add_node(end("end-a", false));
        let e2 = ir.add_node(end("end-b", false));
        ir.add_edge(m, e1, edge("f2"));
        ir.add_edge(m, e2, edge("f3"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::WrongOutDegree { id, count: 2, .. }) => assert_eq!(id, "m1"),
            other => panic!("expected WrongOutDegree(2), got {other:?}"),
        }
    }

    /// RED (D3 R-D3.6): a conditioned outgoing edge refuses.
    #[test]
    fn red_multi_instance_conditioned_edge() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let m = ir.add_node(mi("m1", "review-doc", "docs", 10));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, m, edge("f1"));
        ir.add_edge(m, e, cond_edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnrepresentableCondition { id }) => assert_eq!(id, "m1"),
            other => panic!("expected UnrepresentableCondition, got {other:?}"),
        }
    }

    /// RED (D3 R-D3.7): a non-token id refuses UnrepresentableToken.
    #[test]
    fn red_multi_instance_bad_id_token() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let m = ir.add_node(mi("m 1", "review-doc", "docs", 10));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, m, edge("f1"));
        ir.add_edge(m, e, edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnrepresentableToken { node_id, field, .. }) => {
                assert_eq!(node_id, "m 1");
                assert_eq!(field, "id");
            }
            other => panic!("expected UnrepresentableToken, got {other:?}"),
        }
    }

    /// RED (D3 R-D3.9, D3.0 freeze §2 ruled (b)): non-empty `inputs`
    /// refuses rather than silently dropping the bindings.
    #[test]
    fn red_multi_instance_non_empty_inputs_unrepresentable() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let m = ir.add_node(mi_with_inputs(
            "m1",
            "review-doc",
            "docs",
            10,
            vec![crate::ir::FfiInputBinding {
                target_field: "priority".into(),
                expression: crate::ir::Expression::Literal(crate::ir::IrLiteral::I64(3)),
            }],
        ));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, m, edge("f1"));
        ir.add_edge(m, e, edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::InputsUnrepresentable { id, count: 1 }) => assert_eq!(id, "m1"),
            other => panic!("expected InputsUnrepresentable, got {other:?}"),
        }
    }

    // ── Red fixtures: exact-variant refusals (never bare is_err) ─────────

    #[test]
    fn red_missing_start() {
        let mut ir = IRGraph::new();
        ir.add_node(end("end", false));
        assert!(matches!(
            emit_dsl(&ir, "wf", &decls()),
            Err(DslEmitError::MissingStart)
        ));
    }

    #[test]
    fn red_multiple_starts_sorted_ids() {
        let mut ir = IRGraph::new();
        let s2 = ir.add_node(start("s2"));
        let s1 = ir.add_node(start("s1"));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s1, e, edge("f1"));
        ir.add_edge(s2, e, edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::MultipleStarts { ids }) => {
                assert_eq!(ids, vec!["s1".to_owned(), "s2".to_owned()])
            }
            other => panic!("expected MultipleStarts, got {other:?}"),
        }
    }

    #[test]
    fn red_duplicate_node_id() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let t1 = ir.add_node(task("t1", "cbu.a"));
        let t2 = ir.add_node(task("t1", "cbu.b"));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, t1, edge("f1"));
        ir.add_edge(t1, t2, edge("f2"));
        ir.add_edge(t2, e, edge("f3"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::DuplicateNodeId { id }) => assert_eq!(id, "t1"),
            other => panic!("expected DuplicateNodeId, got {other:?}"),
        }
    }

    #[test]
    fn red_cyclic_graph() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let t1 = ir.add_node(task("t1", "cbu.a"));
        let t2 = ir.add_node(task("t2", "cbu.b"));
        ir.add_edge(s, t1, edge("f1"));
        ir.add_edge(t1, t2, edge("f2"));
        ir.add_edge(t2, t1, edge("f3"));
        assert!(matches!(
            emit_dsl(&ir, "wf", &decls()),
            Err(DslEmitError::CyclicGraph { .. })
        ));
    }

    #[test]
    fn red_unreachable_node() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let e = ir.add_node(end("end", false));
        let orphan = ir.add_node(task("orphan", "cbu.a"));
        let e2 = ir.add_node(end("end2", false));
        ir.add_edge(s, e, edge("f1"));
        ir.add_edge(orphan, e2, edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnreachableNode { id }) => {
                // smallest unreachable id, deterministic
                assert_eq!(id, "end2");
            }
            other => panic!("expected UnreachableNode, got {other:?}"),
        }
    }

    #[test]
    fn red_process_decls_refuse_in_fixed_order() {
        let ir = linear_graph();
        let both = ProcessLevelDecls {
            default_guard_budget_set: true,
            default_retry_policy_set: true,
        };
        match emit_dsl(&ir, "wf", &both) {
            Err(DslEmitError::ProcessDeclUnrepresentable { field }) => {
                assert_eq!(field, "default_guard_budget", "fixed check order");
            }
            other => panic!("expected ProcessDeclUnrepresentable, got {other:?}"),
        }
        let retry_only = ProcessLevelDecls {
            default_guard_budget_set: false,
            default_retry_policy_set: true,
        };
        match emit_dsl(&ir, "wf", &retry_only) {
            Err(DslEmitError::ProcessDeclUnrepresentable { field }) => {
                assert_eq!(field, "default_retry_policy");
            }
            other => panic!("expected ProcessDeclUnrepresentable, got {other:?}"),
        }
    }

    /// Every out-of-core kind refuses as UnsupportedNode with its own
    /// kind name. D1 cement update (named, not silent): BoundaryTimer/
    /// BoundaryError left this list when guards joined the core — 8
    /// kinds remain.
    #[test]
    fn red_unsupported_node_all_remaining_kinds() {
        let unsupported: Vec<(IRNode, &str)> = vec![
            (
                IRNode::GatewayXor {
                    id: "x".into(),
                    name: String::new(),
                },
                "GatewayXor",
            ),
            (
                IRNode::GatewayInclusive {
                    id: "x".into(),
                    name: String::new(),
                    direction: GatewayDirection::Diverging,
                },
                "GatewayInclusive",
            ),
            // TimerWait left this list at D2 (named cement update — it
            // joined the emission core; see the D2 green/red tests).
            (
                IRNode::HumanWait {
                    id: "x".into(),
                    name: String::new(),
                    task_kind: String::new(),
                    corr_key_source: String::new(),
                },
                "HumanWait",
            ),
            (
                IRNode::DataObject {
                    id: "x".into(),
                    name: String::new(),
                    type_decl: bpmn_lite_types::DataObjectType::Primitive(
                        bpmn_lite_types::PrimitiveType::Bool,
                    ),
                    role: bpmn_lite_types::DataObjectRole::Internal,
                },
                "DataObject",
            ),
            (
                IRNode::FfiServiceTask {
                    id: "x".into(),
                    name: String::new(),
                    template_id: [0u8; 32],
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                },
                "FfiServiceTask",
            ),
            (
                IRNode::SendTask {
                    id: "x".into(),
                    name: String::new(),
                    message_name: String::new(),
                    corr_key_source: String::new(),
                },
                "SendTask",
            ),
            // MultiInstance left this list at D3 (named cement update — it
            // joined the emission core; see the D3 green/red tests).
        ];
        for (node, kind) in unsupported {
            let mut ir = IRGraph::new();
            let s = ir.add_node(start("start"));
            let t = ir.add_node(task("t1", "cbu.a"));
            let n = ir.add_node(node);
            let e = ir.add_node(end("end", false));
            ir.add_edge(s, t, edge("f1"));
            ir.add_edge(t, n, edge("f2"));
            ir.add_edge(n, e, edge("f3"));
            match emit_dsl(&ir, "wf", &decls()) {
                Err(DslEmitError::UnsupportedNode { id, kind: k }) => {
                    assert_eq!(id, "x");
                    assert_eq!(k, kind);
                }
                other => panic!("expected UnsupportedNode({kind}), got {other:?}"),
            }
        }
    }

    #[test]
    fn red_unrepresentable_token_id_with_space() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let t = ir.add_node(task("bad id", "cbu.a"));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, t, edge("f1"));
        ir.add_edge(t, e, edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnrepresentableToken { field, value, .. }) => {
                assert_eq!(field, "id");
                assert_eq!(value, "bad id");
            }
            other => panic!("expected UnrepresentableToken, got {other:?}"),
        }
    }

    #[test]
    fn red_unrepresentable_token_corr_source_with_at() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let m = ir.add_node(msg("wait", "reply", "@case-id"));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, m, edge("f1"));
        ir.add_edge(m, e, edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnrepresentableToken {
                node_id,
                field,
                value,
            }) => {
                assert_eq!(node_id, "wait");
                assert_eq!(field, "corr_key_source");
                assert_eq!(value, "@case-id");
            }
            other => panic!("expected UnrepresentableToken, got {other:?}"),
        }
    }

    #[test]
    fn red_unrepresentable_workflow_id() {
        let ir = linear_graph();
        assert!(matches!(
            emit_dsl(&ir, "bad workflow id", &decls()),
            Err(DslEmitError::UnrepresentableToken { field: "workflow_id", .. })
        ));
    }

    #[test]
    fn red_wrong_out_degree_task_with_two() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let t = ir.add_node(task("t1", "cbu.a"));
        let e1 = ir.add_node(end("end1", false));
        let e2 = ir.add_node(end("end2", false));
        ir.add_edge(s, t, edge("f1"));
        ir.add_edge(t, e1, edge("f2"));
        ir.add_edge(t, e2, edge("f3"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::WrongOutDegree { id, count, expected }) => {
                assert_eq!((id.as_str(), count, expected), ("t1", 2, 1));
            }
            other => panic!("expected WrongOutDegree, got {other:?}"),
        }
    }

    /// R24 — converging gateway with 2 outgoing (blind-review finding).
    #[test]
    fn red_wrong_out_degree_converging_gateway() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let sp = ir.add_node(and_gw("split1", GatewayDirection::Diverging));
        let a = ir.add_node(task("branch-a", "cbu.a"));
        let b = ir.add_node(task("branch-b", "cbu.b"));
        let j = ir.add_node(and_gw("join1", GatewayDirection::Converging));
        let e1 = ir.add_node(end("end1", false));
        let e2 = ir.add_node(end("end2", false));
        ir.add_edge(s, sp, edge("f1"));
        ir.add_edge(sp, a, edge("f2"));
        ir.add_edge(sp, b, edge("f3"));
        ir.add_edge(a, j, edge("f4"));
        ir.add_edge(b, j, edge("f5"));
        ir.add_edge(j, e1, edge("f6"));
        ir.add_edge(j, e2, edge("f7"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::WrongOutDegree { id, count, expected }) => {
                assert_eq!((id.as_str(), count, expected), ("join1", 2, 1));
            }
            other => panic!("expected WrongOutDegree on join, got {other:?}"),
        }
    }

    #[test]
    fn red_unmatched_gateway() {
        // Diverging And whose branches never converge — no pairs entry.
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let sp = ir.add_node(and_gw("split1", GatewayDirection::Diverging));
        let a = ir.add_node(task("branch-a", "cbu.a"));
        let b = ir.add_node(task("branch-b", "cbu.b"));
        let e1 = ir.add_node(end("end1", false));
        let e2 = ir.add_node(end("end2", false));
        ir.add_edge(s, sp, edge("f1"));
        ir.add_edge(sp, a, edge("f2"));
        ir.add_edge(sp, b, edge("f3"));
        ir.add_edge(a, e1, edge("f4"));
        ir.add_edge(b, e2, edge("f5"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnmatchedGateway { id }) => assert_eq!(id, "split1"),
            other => panic!("expected UnmatchedGateway, got {other:?}"),
        }
    }

    #[test]
    fn red_condition_on_parallel_flow() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let sp = ir.add_node(and_gw("split1", GatewayDirection::Diverging));
        let a = ir.add_node(task("branch-a", "cbu.a"));
        let b = ir.add_node(task("branch-b", "cbu.b"));
        let j = ir.add_node(and_gw("join1", GatewayDirection::Converging));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, sp, edge("f1"));
        ir.add_edge(sp, a, cond_edge("f2"));
        ir.add_edge(sp, b, edge("f3"));
        ir.add_edge(a, j, edge("f4"));
        ir.add_edge(b, j, edge("f5"));
        ir.add_edge(j, e, edge("f6"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::ConditionOnParallelFlow {
                gateway_id,
                edge_id,
            }) => {
                assert_eq!((gateway_id.as_str(), edge_id.as_str()), ("split1", "f2"));
            }
            other => panic!("expected ConditionOnParallelFlow, got {other:?}"),
        }
    }

    /// B1 blind-review finding 3 (cement): several diverging Ands whose
    /// post-dominator is ONE shared converging And used to emit
    /// nondeterministically (HashMap last-write-wins picked one split as
    /// the join's `:split` — three distinct sources observed for one
    /// graph). Must refuse deterministically at the shared join instead.
    #[test]
    fn red_shared_join_refuses_deterministically() {
        let build = || {
            let mut ir = IRGraph::new();
            let s = ir.add_node(start("start"));
            let sp0 = ir.add_node(and_gw("sp0", GatewayDirection::Diverging));
            let sp1 = ir.add_node(and_gw("sp1", GatewayDirection::Diverging));
            let sp2 = ir.add_node(and_gw("sp2", GatewayDirection::Diverging));
            let ta = ir.add_node(task("ta", "cbu.a"));
            let tb = ir.add_node(task("tb", "cbu.b"));
            let tc = ir.add_node(task("tc", "cbu.c"));
            let td = ir.add_node(task("td", "cbu.d"));
            let j = ir.add_node(and_gw("j1", GatewayDirection::Converging));
            let e = ir.add_node(end("end", false));
            ir.add_edge(s, sp0, edge("f1"));
            ir.add_edge(sp0, sp1, edge("f2"));
            ir.add_edge(sp0, sp2, edge("f3"));
            ir.add_edge(sp1, ta, edge("f4"));
            ir.add_edge(sp1, tb, edge("f5"));
            ir.add_edge(sp2, tc, edge("f6"));
            ir.add_edge(sp2, td, edge("f7"));
            ir.add_edge(ta, j, edge("f8"));
            ir.add_edge(tb, j, edge("f9"));
            ir.add_edge(tc, j, edge("f10"));
            ir.add_edge(td, j, edge("f11"));
            ir.add_edge(j, e, edge("f12"));
            ir
        };
        for _ in 0..20 {
            let ir = build();
            match emit_dsl(&ir, "wf", &decls()) {
                Err(DslEmitError::UnmatchedGateway { id }) => assert_eq!(id, "j1"),
                other => panic!("expected UnmatchedGateway(j1) every run, got {other:?}"),
            }
        }
    }

    /// B1 blind-review findings 1-2 (cement): frozen Stage-1 per-node
    /// order is UnsupportedNode → UnrepresentableToken → WrongOutDegree
    /// → UnmatchedGateway. An out-of-core node with an unrepresentable id
    /// must refuse as UnsupportedNode; an unmatched converging gateway
    /// with wrong out-degree must refuse as WrongOutDegree.
    #[test]
    fn stage1_order_kind_before_token_and_degree_before_pairing() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let x = ir.add_node(IRNode::GatewayXor {
            id: "bad id".into(),
            name: String::new(),
        });
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, x, edge("f1"));
        ir.add_edge(x, e, edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnsupportedNode { kind, .. }) => assert_eq!(kind, "GatewayXor"),
            other => panic!("kind gate must precede token check, got {other:?}"),
        }

        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let j = ir.add_node(and_gw("join1", GatewayDirection::Converging));
        let e1 = ir.add_node(end("end1", false));
        let e2 = ir.add_node(end("end2", false));
        ir.add_edge(s, j, edge("f1"));
        ir.add_edge(j, e1, edge("f2"));
        ir.add_edge(j, e2, edge("f3"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::WrongOutDegree { id, count, expected }) => {
                assert_eq!((id.as_str(), count, expected), ("join1", 2, 1));
            }
            other => panic!("degree check must precede pairing, got {other:?}"),
        }
    }

    // ── D1 guard reds (R25-R28, R36 + emission-side R31/R33 mirrors) ──

    fn guard_on(host_kind: IRNode) -> IRGraph {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let h = ir.add_node(host_kind);
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, h, edge("f1"));
        ir.add_edge(h, e, edge("f2"));
        let g = ir.add_node(IRNode::BoundaryTimer {
            id: "g1".into(),
            attached_to: ir[h].id().to_owned(),
            spec: TimerSpec::Duration { ms: 1000 },
            interrupting: true,
            failure_budget: None,
        });
        let ee = ir.add_node(end("escape-end", false));
        ir.add_edge(g, ee, edge("f-esc"));
        ir
    }

    #[test]
    fn red_guard_on_message_wait_host() {
        let ir = guard_on(msg("mw", "reply", "case-id"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::GuardOnUnsupportedHost {
                guard_id,
                host,
                host_kind,
            }) => {
                assert_eq!(
                    (guard_id.as_str(), host.as_str(), host_kind),
                    ("g1", "mw", "MessageWait")
                );
            }
            other => panic!("expected GuardOnUnsupportedHost, got {other:?}"),
        }
    }

    /// D2 blind-review cement: TimerWait joined the emission core, so a
    /// designer can now realistically attach a guard to one — the
    /// guards-on-ServiceTask-only rule (D1 frozen) must refuse it by
    /// name. (The D2.0 freeze claimed this red existed; it did not —
    /// recorded as a freeze correction in the D2 receipt.)
    #[test]
    fn red_guard_on_timer_wait_host() {
        let ir = guard_on(timer("tw", TimerSpec::Duration { ms: 1000 }));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::GuardOnUnsupportedHost {
                guard_id,
                host,
                host_kind,
            }) => {
                assert_eq!(
                    (guard_id.as_str(), host.as_str(), host_kind),
                    ("g1", "tw", "TimerWait")
                );
            }
            other => panic!("expected GuardOnUnsupportedHost, got {other:?}"),
        }
    }

    /// D3 R-D3.8: written up front (D2's TimerWait-host red was missed at
    /// freeze time and only added as a review correction — this tranche
    /// does not repeat that gap). The `!matches!(ServiceTask)` host check
    /// is generic over IRNode kind, so no new emit code was needed — only
    /// this fixture, proving MultiInstance is covered like every other
    /// non-ServiceTask host.
    #[test]
    fn red_guard_on_multi_instance_host() {
        let ir = guard_on(mi("m1", "review-doc", "docs", 10));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::GuardOnUnsupportedHost {
                guard_id,
                host,
                host_kind,
            }) => {
                assert_eq!(
                    (guard_id.as_str(), host.as_str(), host_kind),
                    ("g1", "m1", "MultiInstance")
                );
            }
            other => panic!("expected GuardOnUnsupportedHost, got {other:?}"),
        }
    }

    #[test]
    fn red_guard_escape_out_degree_zero_and_two() {
        // 0 escape edges
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let t = ir.add_node(task("t1", "cbu.a"));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, t, edge("f1"));
        ir.add_edge(t, e, edge("f2"));
        ir.add_node(IRNode::BoundaryTimer {
            id: "g1".into(),
            attached_to: "t1".into(),
            spec: TimerSpec::Duration { ms: 1000 },
            interrupting: true,
            failure_budget: None,
        });
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::WrongOutDegree { id, count, expected }) => {
                assert_eq!((id.as_str(), count, expected), ("g1", 0, 1));
            }
            other => panic!("expected WrongOutDegree(0), got {other:?}"),
        }
        // 2 escape edges
        let mut ir = guard_on(task("t1", "cbu.a"));
        let g = ir
            .node_indices()
            .find(|&i| ir[i].id() == "g1")
            .unwrap();
        let ee2 = ir.add_node(end("escape-end-2", false));
        ir.add_edge(g, ee2, edge("f-esc-2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::WrongOutDegree { id, count, expected }) => {
                assert_eq!((id.as_str(), count, expected), ("g1", 2, 1));
            }
            other => panic!("expected WrongOutDegree(2), got {other:?}"),
        }
    }

    #[test]
    fn red_guard_conditioned_escape_edge() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let t = ir.add_node(task("t1", "cbu.a"));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, t, edge("f1"));
        ir.add_edge(t, e, edge("f2"));
        let g = ir.add_node(IRNode::BoundaryTimer {
            id: "g1".into(),
            attached_to: "t1".into(),
            spec: TimerSpec::Duration { ms: 1000 },
            interrupting: true,
            failure_budget: None,
        });
        let ee = ir.add_node(end("escape-end", false));
        ir.add_edge(g, ee, cond_edge("f-esc"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnrepresentableCondition { id }) => assert_eq!(id, "g1"),
            other => panic!("expected UnrepresentableCondition, got {other:?}"),
        }
    }

    /// R36 — escape edge back into the guard's own host: acyclic to
    /// plain flow-edge toposort (attachment is a field), but a deadlock
    /// in the effective graph — must refuse CyclicGraph, never truncate
    /// (the D1.0 blind-review fail-open, closed).
    #[test]
    fn red_guard_escape_into_own_host_refuses_cyclic() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let t = ir.add_node(task("t1", "cbu.a"));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, t, edge("f1"));
        ir.add_edge(t, e, edge("f2"));
        let g = ir.add_node(IRNode::BoundaryTimer {
            id: "g1".into(),
            attached_to: "t1".into(),
            spec: TimerSpec::Duration { ms: 1000 },
            interrupting: true,
            failure_budget: None,
        });
        ir.add_edge(g, t, edge("f-esc"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::CyclicGraph { .. }) => {}
            other => panic!("expected CyclicGraph, got {other:?}"),
        }
    }

    #[test]
    fn red_flow_into_guard() {
        let mut ir = guard_on(task("t1", "cbu.a"));
        let g = ir.node_indices().find(|&i| ir[i].id() == "g1").unwrap();
        let s = ir.node_indices().find(|&i| ir[i].id() == "start").unwrap();
        ir.add_edge(s, g, edge("f-bad"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::FlowIntoGuard { guard_id, edge_id }) => {
                assert_eq!((guard_id.as_str(), edge_id.as_str()), ("g1", "f-bad"));
            }
            other => panic!("expected FlowIntoGuard, got {other:?}"),
        }
    }

    #[test]
    fn red_guard_budget_zero_and_interrupting_cycle() {
        let mut ir = guard_on(task("t1", "cbu.a"));
        let g = ir.node_indices().find(|&i| ir[i].id() == "g1").unwrap();
        if let IRNode::BoundaryTimer { failure_budget, .. } = &mut ir[g] {
            *failure_budget = Some(0);
        }
        assert!(matches!(
            emit_dsl(&ir, "wf", &decls()),
            Err(DslEmitError::GuardBudgetZero { .. })
        ));

        let mut ir = guard_on(task("t1", "cbu.a"));
        let g = ir.node_indices().find(|&i| ir[i].id() == "g1").unwrap();
        if let IRNode::BoundaryTimer { spec, .. } = &mut ir[g] {
            *spec = TimerSpec::Cycle {
                interval_ms: 1000,
                max_fires: 3,
            };
        }
        assert!(matches!(
            emit_dsl(&ir, "wf", &decls()),
            Err(DslEmitError::InterruptingCycleTimer { .. })
        ));
    }

    #[test]
    fn red_unrepresentable_condition_on_task_edge() {
        let mut ir = IRGraph::new();
        let s = ir.add_node(start("start"));
        let t = ir.add_node(task("t1", "cbu.a"));
        let e = ir.add_node(end("end", false));
        ir.add_edge(s, t, edge("f1"));
        ir.add_edge(t, e, cond_edge("f2"));
        match emit_dsl(&ir, "wf", &decls()) {
            Err(DslEmitError::UnrepresentableCondition { id }) => assert_eq!(id, "t1"),
            other => panic!("expected UnrepresentableCondition, got {other:?}"),
        }
    }
}
