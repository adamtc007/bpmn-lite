//! WS-A.2 slice 1 — operation core + linear operations (EOP-PLAN-BPMN-
//! DESIGN-003 v0.2 §WS-A.2). The deterministic public mutation surface
//! over the WS-A.1 `DesignerDag` schema (`schema.rs`/`board_candidate.rs`
//! are frozen upstream, unmodified except for the minimal `pub(crate)`
//! support helpers documented in `schema.rs`).
//!
//! Invariants & Absolute Boundaries (binding, reproduced verbatim from the
//! dispatch brief — do not weaken any of these to make a receipt pass):
//!
//! - **I16**: structural derivation (pairing, regions, merge identity) is
//!   consumed from `bpmn_lite_compiler::{compute_post_dominators,
//!   compute_region_map, gateway_pairs}` — NEVER computed in this crate.
//!   This slice needs none of them; none are imported here.
//! - **I23 + review-F8**: every edge-introducing operation pre-gates
//!   forward-only flow with
//!   `petgraph::algo::has_path_connecting(&graph, to, from, None)` — if a
//!   path exists from `to` back to `from`, adding `from -> to` would
//!   close a cycle: REFUSE with an error naming both node ids. The
//!   compiler's cyclicity gate remains the backstop, never the mechanism.
//! - **I18**: operations apply to a CLONE (the staged candidate), never
//!   to the authoritative DAG. Ratification is the caller's separate act.
//! - **Review-F4**: `NodeKey`s for created nodes are carried IN the
//!   operation record (caller/log-supplied), never minted inside `apply`.
//!   No `Uuid::new_v4()` anywhere in this file.
//! - **Fail closed**: every refusal is an `Err` naming the offending
//!   node/edge id. No skips, no defaults, no `#[allow]`.

use crate::schema::{DesignerDag, DesignerEdge, NodeKey, Provenance};
use anyhow::{anyhow, Result};
use bpmn_lite_compiler::ir::{ConditionExpr, IRNode, TimerSpec};
use petgraph::algo::has_path_connecting;
use serde::{Deserialize, Serialize};

/// The arming trigger for a boundary guard (§WS-A.2 slice 2 opcode
/// trichotomy: Attach/AttachRearming/SetGuardTrigger all route through
/// this shared shape). `AttachRollbackGuard` is EXCLUDED BY DESIGN — no
/// `IRNode` kind exists for it yet (pending substrate trace); do not add
/// a variant here to accommodate it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum GuardTrigger {
    Timer(TimerSpec),
    Error { error_code: Option<String> },
}

/// One deterministic edit. Carries every created identity (F4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    /// Insert `node` (with `key`) after `anchor`: re-points every
    /// anchor→X edge (X = anchor's current successors) to node→X,
    /// then adds anchor→node with `edge_id`.
    InsertAfter {
        anchor: NodeKey,
        key: NodeKey,
        node: IRNode,
        edge_id: String,
    },
    /// Mirror of InsertAfter on the predecessor side.
    InsertBefore {
        anchor: NodeKey,
        key: NodeKey,
        node: IRNode,
        edge_id: String,
    },
    /// Append with NO re-pointing: just node + anchor→node edge.
    /// Refused if anchor already has an outgoing edge (that is
    /// InsertAfter's job) — keeps Append unambiguous.
    AppendNode {
        anchor: NodeKey,
        key: NodeKey,
        node: IRNode,
        edge_id: String,
    },
    /// Connect two existing nodes (forward-only pre-gate, I23).
    Connect {
        from: NodeKey,
        to: NodeKey,
        edge_id: String,
        condition: Option<ConditionExpr>,
    },
    /// Delete a node and ALL its incident edges. Refused if any
    /// OTHER node's `attached_to_key` references it (a guard would
    /// dangle) — error names both ids.
    DeleteNode { target: NodeKey },
    /// Attach an INTERRUPTING boundary guard to `host`.
    AttachGuard {
        host: NodeKey,
        key: NodeKey,
        guard_id: String,
        trigger: GuardTrigger,
    },
    /// Attach a NON-INTERRUPTING (re-arming) boundary guard to `host`.
    AttachRearmingGuard {
        host: NodeKey,
        key: NodeKey,
        guard_id: String,
        trigger: GuardTrigger,
    },
    /// Replace the arming trigger on an existing boundary guard.
    SetGuardTrigger { guard: NodeKey, trigger: GuardTrigger },
    /// Set/override a guard's failure budget (`None` clears to inherit
    /// the workflow default).
    SetGuardBudget {
        guard: NodeKey,
        failure_budget: Option<u32>,
    },
    /// Set the correlation-source expression on a waiting node
    /// (MessageWait / HumanWait / SendTask — anything else is refused).
    SetCorrelationSource {
        node: NodeKey,
        corr_key_source: String,
    },
}

/// Applying an operation to a staged candidate.
#[derive(Debug)]
pub struct StagedCandidate {
    pub candidate: DesignerDag,
    pub applied: Operation,
    pub provenance: Provenance,
}

/// THE entry point: clone `base`, apply `op` deterministically, return
/// the candidate. Does NOT run admission — the caller stages then calls
/// `candidate.admit()` (the full production chain) before ratification.
/// Refusals per the boundaries above.
pub fn apply(base: &DesignerDag, op: Operation, provenance: Provenance) -> Result<StagedCandidate> {
    let applied = op.clone();
    let mut candidate = base.clone();

    match op {
        Operation::InsertAfter {
            anchor,
            key,
            node,
            edge_id,
        } => {
            // Gather anchor's current successors and detach their edges
            // BEFORE inserting the new node, preserving each edge's id/
            // condition/provenance for re-pointing (F4: no new flow ids
            // minted for re-pointed edges).
            let targets = candidate.successors(anchor);
            let mut reattach = Vec::with_capacity(targets.len());
            for target in targets {
                let edge = candidate.remove_edge_between(anchor, target)?;
                reattach.push((target, edge));
            }
            candidate.insert_node(key, node, None, provenance.clone())?;
            candidate.insert_edge(
                anchor,
                key,
                DesignerEdge {
                    id: edge_id,
                    condition: None,
                    provenance: provenance.clone(),
                },
            )?;
            for (target, edge) in reattach {
                candidate.insert_edge(key, target, edge)?;
            }
        }

        Operation::InsertBefore {
            anchor,
            key,
            node,
            edge_id,
        } => {
            // Mirror of InsertAfter on the predecessor side.
            let sources = candidate.predecessors(anchor);
            let mut reattach = Vec::with_capacity(sources.len());
            for source in sources {
                let edge = candidate.remove_edge_between(source, anchor)?;
                reattach.push((source, edge));
            }
            candidate.insert_node(key, node, None, provenance.clone())?;
            candidate.insert_edge(
                key,
                anchor,
                DesignerEdge {
                    id: edge_id,
                    condition: None,
                    provenance: provenance.clone(),
                },
            )?;
            for (source, edge) in reattach {
                candidate.insert_edge(source, key, edge)?;
            }
        }

        Operation::AppendNode {
            anchor,
            key,
            node,
            edge_id,
        } => {
            if !candidate.successors(anchor).is_empty() {
                return Err(anyhow!(
                    "AppendNode refused: anchor {anchor:?} already has an outgoing edge \
                     (use InsertAfter to re-point it)"
                ));
            }
            candidate.insert_node(key, node, None, provenance.clone())?;
            candidate.insert_edge(
                anchor,
                key,
                DesignerEdge {
                    id: edge_id,
                    condition: None,
                    provenance: provenance.clone(),
                },
            )?;
        }

        Operation::Connect {
            from,
            to,
            edge_id,
            condition,
        } => {
            // I23 + review-F8: forward-only pre-gate. If a path already
            // exists from `to` back to `from`, adding `from -> to` would
            // close a cycle.
            let from_idx = candidate
                .index_of(from)
                .ok_or_else(|| anyhow!("Connect refused: unknown from-node {from:?}"))?;
            let to_idx = candidate
                .index_of(to)
                .ok_or_else(|| anyhow!("Connect refused: unknown to-node {to:?}"))?;
            if has_path_connecting(candidate.graph(), to_idx, from_idx, None) {
                // Slice-1 polish (dispatch brief item 4): name BPMN ids
                // alongside the keys — both nodes are known-good at this
                // point (index_of just resolved them), so the ids are
                // always resolvable here.
                let from_id = candidate
                    .node(from)
                    .map(|n| n.ir.id().to_owned())
                    .unwrap_or_else(|| format!("{from:?}"));
                let to_id = candidate
                    .node(to)
                    .map(|n| n.ir.id().to_owned())
                    .unwrap_or_else(|| format!("{to:?}"));
                return Err(anyhow!(
                    "Connect {from:?} ('{from_id}') -> {to:?} ('{to_id}') refused: a path \
                     already exists {to:?} ('{to_id}') -> {from:?} ('{from_id}'); adding this \
                     edge would close a cycle (I23)"
                ));
            }
            candidate.insert_edge(
                from,
                to,
                DesignerEdge {
                    id: edge_id,
                    condition,
                    provenance: provenance.clone(),
                },
            )?;
        }

        Operation::DeleteNode { target } => {
            let dependents = candidate.nodes_attached_to(target);
            if !dependents.is_empty() {
                let target_label = candidate
                    .node(target)
                    .map(|n| n.ir.id().to_owned())
                    .unwrap_or_else(|| format!("{target:?}"));
                let dependent_labels: Vec<String> = dependents
                    .iter()
                    .map(|k| {
                        candidate
                            .node(*k)
                            .map(|n| n.ir.id().to_owned())
                            .unwrap_or_else(|| format!("{k:?}"))
                    })
                    .collect();
                return Err(anyhow!(
                    "DeleteNode refused: '{target_label}' ({target:?}) still hosts attached \
                     guard(s) {dependent_labels:?}; delete the guard(s) first"
                ));
            }
            candidate.remove_node(target)?;
        }

        Operation::AttachGuard {
            host,
            key,
            guard_id,
            trigger,
        } => {
            let host_id = candidate
                .node(host)
                .map(|n| n.ir.id().to_owned())
                .ok_or_else(|| anyhow!("AttachGuard '{guard_id}' refused: unknown host {host:?}"))?;
            let ir = match trigger {
                GuardTrigger::Timer(spec) => {
                    // Boundary rule 6 (V&S §4.5 pre-gate): cycle triggers
                    // are legal ONLY on a non-interrupting guard; this is
                    // an interrupting guard by construction.
                    if matches!(spec, TimerSpec::Cycle { .. }) {
                        return Err(anyhow!(
                            "AttachGuard '{guard_id}' refused: a Cycle timer trigger is only \
                             legal on a non-interrupting guard (use AttachRearmingGuard)"
                        ));
                    }
                    IRNode::BoundaryTimer {
                        id: guard_id.clone(),
                        attached_to: host_id,
                        spec,
                        interrupting: true,
                        failure_budget: None,
                    }
                }
                GuardTrigger::Error { error_code } => IRNode::BoundaryError {
                    id: guard_id.clone(),
                    attached_to: host_id,
                    error_code,
                    failure_budget: None,
                },
            };
            candidate.insert_node(key, ir, Some(host), provenance.clone())?;
        }

        Operation::AttachRearmingGuard {
            host,
            key,
            guard_id,
            trigger,
        } => {
            let host_id = candidate.node(host).map(|n| n.ir.id().to_owned()).ok_or_else(|| {
                anyhow!("AttachRearmingGuard '{guard_id}' refused: unknown host {host:?}")
            })?;
            let ir = match trigger {
                // Any TimerSpec is legal on a non-interrupting guard —
                // Cycle-on-non-interrupting is precisely the legal
                // combination boundary rule 6 carves out.
                GuardTrigger::Timer(spec) => IRNode::BoundaryTimer {
                    id: guard_id.clone(),
                    attached_to: host_id,
                    spec,
                    interrupting: false,
                    failure_budget: None,
                },
                // F2a: BoundaryError is interrupting-only in this
                // substrate (non-interrupting error boundaries are
                // parse-rejected) — a re-arming Error guard is
                // unrepresentable, refuse.
                GuardTrigger::Error { .. } => {
                    return Err(anyhow!(
                        "AttachRearmingGuard '{guard_id}' refused: BoundaryError is \
                         interrupting-only in this substrate (F2a) — use AttachGuard"
                    ));
                }
            };
            candidate.insert_node(key, ir, Some(host), provenance.clone())?;
        }

        Operation::SetGuardTrigger { guard, trigger } => {
            // Scoped read: resolve label/kind/current-interrupting as OWNED
            // values first, so this immutable borrow of `candidate` ends
            // before the mutable `node_mut` borrow below (no overlap).
            enum GuardKind {
                Timer { interrupting: bool },
                Error,
            }
            let (guard_label, kind) = {
                let node = candidate
                    .node(guard)
                    .ok_or_else(|| anyhow!("SetGuardTrigger refused: unknown guard {guard:?}"))?;
                let label = node.ir.id().to_owned();
                let kind = match &node.ir {
                    IRNode::BoundaryTimer { interrupting, .. } => GuardKind::Timer {
                        interrupting: *interrupting,
                    },
                    IRNode::BoundaryError { .. } => GuardKind::Error,
                    _ => {
                        return Err(anyhow!(
                            "SetGuardTrigger refused: '{label}' ({guard:?}) is not a boundary \
                             guard"
                        ));
                    }
                };
                (label, kind)
            };
            match kind {
                GuardKind::Timer { interrupting } => {
                    let spec = match trigger {
                        GuardTrigger::Timer(spec) => spec,
                        GuardTrigger::Error { .. } => {
                            return Err(anyhow!(
                                "SetGuardTrigger '{guard_label}' refused: cannot switch a \
                                 Timer boundary guard to an Error trigger (kind-switch is an \
                                 explicit delete + attach)"
                            ));
                        }
                    };
                    // Boundary rule 6.
                    if interrupting && matches!(spec, TimerSpec::Cycle { .. }) {
                        return Err(anyhow!(
                            "SetGuardTrigger '{guard_label}' refused: a Cycle timer trigger is \
                             only legal on a non-interrupting guard (boundary rule 6)"
                        ));
                    }
                    let node_mut = candidate
                        .node_mut(guard)
                        .expect("existence just checked above");
                    match &mut node_mut.ir {
                        IRNode::BoundaryTimer { spec: cur_spec, .. } => *cur_spec = spec,
                        _ => unreachable!("kind checked above"),
                    }
                }
                GuardKind::Error => {
                    let new_code = match trigger {
                        GuardTrigger::Error { error_code } => error_code,
                        GuardTrigger::Timer(_) => {
                            return Err(anyhow!(
                                "SetGuardTrigger '{guard_label}' refused: cannot switch an \
                                 Error boundary guard to a Timer trigger (kind-switch is an \
                                 explicit delete + attach)"
                            ));
                        }
                    };
                    let node_mut = candidate
                        .node_mut(guard)
                        .expect("existence just checked above");
                    match &mut node_mut.ir {
                        IRNode::BoundaryError { error_code, .. } => *error_code = new_code,
                        _ => unreachable!("kind checked above"),
                    }
                }
            }
        }

        Operation::SetGuardBudget { guard, failure_budget } => {
            {
                let node = candidate
                    .node(guard)
                    .ok_or_else(|| anyhow!("SetGuardBudget refused: unknown guard {guard:?}"))?;
                if !matches!(node.ir, IRNode::BoundaryTimer { .. } | IRNode::BoundaryError { .. }) {
                    // I24: budget on a non-guard is unrepresentable.
                    let label = node.ir.id().to_owned();
                    return Err(anyhow!(
                        "SetGuardBudget refused: '{label}' ({guard:?}) is not a boundary guard"
                    ));
                }
            }
            let node_mut = candidate.node_mut(guard).expect("existence just checked above");
            match &mut node_mut.ir {
                IRNode::BoundaryTimer { failure_budget: fb, .. }
                | IRNode::BoundaryError { failure_budget: fb, .. } => *fb = failure_budget,
                _ => unreachable!("kind checked above"),
            }
        }

        Operation::SetCorrelationSource {
            node: target,
            corr_key_source: new_source,
        } => {
            {
                let node = candidate.node(target).ok_or_else(|| {
                    anyhow!("SetCorrelationSource refused: unknown node {target:?}")
                })?;
                if !matches!(
                    node.ir,
                    IRNode::MessageWait { .. } | IRNode::HumanWait { .. } | IRNode::SendTask { .. }
                ) {
                    let label = node.ir.id().to_owned();
                    return Err(anyhow!(
                        "SetCorrelationSource refused: '{label}' ({target:?}) is not a \
                         correlation-bearing node (MessageWait/HumanWait/SendTask only)"
                    ));
                }
            }
            let node_mut = candidate
                .node_mut(target)
                .expect("existence just checked above");
            match &mut node_mut.ir {
                IRNode::MessageWait { corr_key_source, .. }
                | IRNode::HumanWait { corr_key_source, .. }
                | IRNode::SendTask { corr_key_source, .. } => *corr_key_source = new_source,
                _ => unreachable!("kind checked above"),
            }
        }
    }

    Ok(StagedCandidate {
        candidate,
        applied,
        provenance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_compiler::ir::TimerSpec;
    use uuid::Uuid;

    fn key() -> NodeKey {
        NodeKey(Uuid::new_v4())
    }

    fn task(id: &str) -> IRNode {
        IRNode::ServiceTask {
            id: id.into(),
            name: id.into(),
            task_type: "noop".into(),
        }
    }

    fn end_node(id: &str) -> IRNode {
        IRNode::End {
            id: id.into(),
            terminate: false,
        }
    }

    /// start -> t1 -> end
    fn linear(name: &str) -> (DesignerDag, NodeKey, NodeKey, NodeKey) {
        let mut dag = DesignerDag::new(name);
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let t = dag.insert_node(key(), task("t1"), None, Provenance::default()).unwrap();
        let e = dag
            .insert_node(key(), end_node("end"), None, Provenance::default())
            .unwrap();
        dag.insert_edge(
            s,
            t,
            DesignerEdge {
                id: "e1".into(),
                condition: None,
                provenance: Provenance::default(),
            },
        )
        .unwrap();
        dag.insert_edge(
            t,
            e,
            DesignerEdge {
                id: "e2".into(),
                condition: None,
                provenance: Provenance::default(),
            },
        )
        .unwrap();
        (dag, s, t, e)
    }

    /// Receipt 1 (GREEN): start->t1->end; InsertAfter(anchor=t1, t2) ->
    /// start->t1->t2->end; candidate.admit() green; base DAG unchanged.
    #[test]
    fn insert_after_repoints_and_admits() {
        let (base, _s, t1, _e) = linear("recv1");
        let base_count_before = base.node_count();
        let t2_key = key();
        let staged = apply(
            &base,
            Operation::InsertAfter {
                anchor: t1,
                key: t2_key,
                node: task("t2"),
                edge_id: "e1b".into(),
            },
            Provenance::default(),
        )
        .expect("insert_after must succeed");

        // Topology: t1's old edge to end is now t2's edge to end, with the
        // SAME id/condition; a fresh anchor->node edge exists too.
        assert_eq!(staged.candidate.successors(t1), vec![t2_key]);
        let end_targets = staged.candidate.successors(t2_key);
        assert_eq!(end_targets.len(), 1);

        staged.candidate.admit().expect("repointed chain must admit");

        // I18: base DAG unchanged.
        assert_eq!(base.node_count(), base_count_before);
        assert_eq!(base.node_count(), 3);
    }

    /// Receipt 2 (GREEN): same shape via InsertBefore(anchor=t1).
    #[test]
    fn insert_before_mirror() {
        let (base, s, t1, _e) = linear("recv2");
        let base_count_before = base.node_count();
        let t0_key = key();
        let staged = apply(
            &base,
            Operation::InsertBefore {
                anchor: t1,
                key: t0_key,
                node: task("t0"),
                edge_id: "e0b".into(),
            },
            Provenance::default(),
        )
        .expect("insert_before must succeed");

        // start's old edge to t1 is now start's edge to t0; a fresh
        // t0->t1 edge exists too.
        assert_eq!(staged.candidate.successors(s), vec![t0_key]);
        assert_eq!(staged.candidate.successors(t0_key), vec![t1]);

        staged.candidate.admit().expect("repointed chain must admit");
        assert_eq!(base.node_count(), base_count_before);
    }

    /// Receipt 3 (RED): start->a->b->end; Connect(from=b, to=a) refused
    /// (a path a->b already exists, so b->a would close a cycle); error
    /// names both ids; base unchanged.
    #[test]
    fn connect_refuses_backward_edge() {
        let mut dag = DesignerDag::new("recv3");
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let a = dag.insert_node(key(), task("a"), None, Provenance::default()).unwrap();
        let b = dag.insert_node(key(), task("b"), None, Provenance::default()).unwrap();
        let e = dag
            .insert_node(key(), end_node("end"), None, Provenance::default())
            .unwrap();
        dag.insert_edge(
            s,
            a,
            DesignerEdge { id: "e1".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            a,
            b,
            DesignerEdge { id: "e2".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            b,
            e,
            DesignerEdge { id: "e3".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        let base_count_before = dag.node_count();

        let err = apply(
            &dag,
            Operation::Connect {
                from: b,
                to: a,
                edge_id: "back".into(),
                condition: None,
            },
            Provenance::default(),
        )
        .expect_err("backward connect must be refused");
        let msg = err.to_string();
        assert!(msg.contains(&format!("{b:?}")), "error must name from-id: {msg}");
        assert!(msg.contains(&format!("{a:?}")), "error must name to-id: {msg}");
        assert_eq!(dag.node_count(), base_count_before);
    }

    /// Receipt 4 (RED): anchor with existing outgoing edge -> AppendNode
    /// refused.
    #[test]
    fn append_refuses_on_occupied_anchor() {
        let (base, _s, t1, _e) = linear("recv4");
        let err = apply(
            &base,
            Operation::AppendNode {
                anchor: t1,
                key: key(),
                node: task("t2"),
                edge_id: "e_new".into(),
            },
            Provenance::default(),
        )
        .expect_err("append on occupied anchor must be refused");
        assert!(err.to_string().contains(&format!("{t1:?}")));
    }

    /// Receipt 5: task + boundary guard attached via attached_to_key ->
    /// DeleteNode(task) refused naming both ids (RED); delete the guard
    /// first, then the task deletes clean (GREEN).
    #[test]
    fn delete_refuses_dangling_guard() {
        let mut dag = DesignerDag::new("recv5");
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let t = dag.insert_node(key(), task("work"), None, Provenance::default()).unwrap();
        let e = dag
            .insert_node(key(), end_node("end"), None, Provenance::default())
            .unwrap();
        let guard = dag
            .insert_node(
                key(),
                IRNode::BoundaryTimer {
                    id: "guard".into(),
                    attached_to: "work".into(),
                    spec: TimerSpec::Duration { ms: 60_000 },
                    interrupting: true,
                    failure_budget: None,
                },
                Some(t),
                Provenance::default(),
            )
            .unwrap();
        dag.insert_edge(
            s,
            t,
            DesignerEdge { id: "e1".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            t,
            e,
            DesignerEdge { id: "e2".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();

        // RED: deleting the task while the guard still references it.
        let err = apply(&dag, Operation::DeleteNode { target: t }, Provenance::default())
            .expect_err("delete of a guarded node must be refused");
        let msg = err.to_string();
        assert!(msg.contains("work"), "error must name the task: {msg}");
        assert!(msg.contains("guard"), "error must name the guard: {msg}");

        // GREEN: delete the guard first...
        let staged_guard_gone = apply(&dag, Operation::DeleteNode { target: guard }, Provenance::default())
            .expect("guard delete must succeed");
        // ...then the task deletes clean.
        apply(
            &staged_guard_gone.candidate,
            Operation::DeleteNode { target: t },
            Provenance::default(),
        )
        .expect("task delete must succeed once unguarded");
    }

    /// Receipt 6 (GREEN): delete t2 then insert a new node with BPMN id
    /// "t2" succeeds — deletion frees the id for reuse.
    #[test]
    fn deleted_ids_are_reusable() {
        let mut dag = DesignerDag::new("recv6");
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let t1 = dag.insert_node(key(), task("t1"), None, Provenance::default()).unwrap();
        let t2 = dag.insert_node(key(), task("t2"), None, Provenance::default()).unwrap();
        let e = dag
            .insert_node(key(), end_node("end"), None, Provenance::default())
            .unwrap();
        dag.insert_edge(
            s,
            t1,
            DesignerEdge { id: "e1".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            t1,
            t2,
            DesignerEdge { id: "e2".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            t2,
            e,
            DesignerEdge { id: "e3".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();

        let staged_deleted = apply(&dag, Operation::DeleteNode { target: t2 }, Provenance::default())
            .expect("delete t2 must succeed");

        apply(
            &staged_deleted.candidate,
            Operation::AppendNode {
                anchor: t1,
                key: key(),
                node: task("t2"),
                edge_id: "e2_reused".into(),
            },
            Provenance::default(),
        )
        .expect("re-inserting a node with the freed bpmn id 't2' must succeed");
    }

    /// Receipt 7 (RED): applying an op whose key/node.id()/edge_id
    /// collides is refused (bubbles schema's F3 checks).
    #[test]
    fn duplicate_key_or_id_refused_through_ops() {
        let (base, _s, _t1, e) = linear("recv7");

        // Duplicate BPMN node id ("start" already exists), appended at
        // the unoccupied `end` anchor.
        let err = apply(
            &base,
            Operation::AppendNode {
                anchor: e,
                key: key(),
                node: IRNode::Start { id: "start".into() },
                edge_id: "e_dup_id".into(),
            },
            Provenance::default(),
        )
        .expect_err("duplicate bpmn id must be refused");
        assert!(err.to_string().contains("start"));

        // Duplicate NodeKey.
        let (base2, s2, _t1_2, e2) = linear("recv7b");
        let err = apply(
            &base2,
            Operation::AppendNode {
                anchor: e2,
                key: s2, // collides with the start node's key
                node: task("dup_key"),
                edge_id: "e_dup_key".into(),
            },
            Provenance::default(),
        )
        .expect_err("duplicate NodeKey must be refused");
        assert!(err.to_string().contains(&format!("{s2:?}")));

        // Duplicate sequence-flow id ("e2" already exists on t1->end).
        let (base3, _s3, t1_3, e3) = linear("recv7c");
        let err = apply(
            &base3,
            Operation::Connect {
                from: t1_3,
                to: e3,
                edge_id: "e2".into(),
                condition: None,
            },
            Provenance::default(),
        )
        .expect_err("duplicate flow id must be refused");
        assert!(err.to_string().contains("e2"));
    }

    /// Receipt 8: applying the same Operation to clones of the same base
    /// twice yields candidates with identical to_ir() node/edge id sets.
    #[test]
    fn applying_same_operation_twice_is_deterministic() {
        let (base, _s, t1, _e) = linear("recv8");
        let new_key = key();
        let op = Operation::InsertAfter {
            anchor: t1,
            key: new_key,
            node: task("t2"),
            edge_id: "e1b".into(),
        };

        let staged_a = apply(&base, op.clone(), Provenance::default()).expect("first apply");
        let staged_b = apply(&base, op, Provenance::default()).expect("second apply");

        let ir_a = staged_a.candidate.to_ir().unwrap();
        let ir_b = staged_b.candidate.to_ir().unwrap();

        let mut node_ids_a: Vec<&str> = ir_a.node_indices().map(|i| ir_a[i].id()).collect();
        let mut node_ids_b: Vec<&str> = ir_b.node_indices().map(|i| ir_b[i].id()).collect();
        node_ids_a.sort();
        node_ids_b.sort();
        assert_eq!(node_ids_a, node_ids_b);

        let mut edge_ids_a: Vec<&str> = ir_a.edge_indices().map(|i| ir_a[i].id.as_str()).collect();
        let mut edge_ids_b: Vec<&str> = ir_b.edge_indices().map(|i| ir_b[i].id.as_str()).collect();
        edge_ids_a.sort();
        edge_ids_b.sort();
        assert_eq!(edge_ids_a, edge_ids_b);
    }

    fn message_wait(id: &str, corr: &str) -> IRNode {
        IRNode::MessageWait {
            id: id.into(),
            name: id.into(),
            corr_key_source: corr.into(),
        }
    }

    fn find_boundary_timer<'a>(
        ir: &'a bpmn_lite_compiler::ir::IRGraph,
        id: &str,
    ) -> Option<(&'a TimerSpec, bool, Option<u32>)> {
        ir.node_indices().find_map(|i| match &ir[i] {
            IRNode::BoundaryTimer {
                id: bid,
                spec,
                interrupting,
                failure_budget,
                ..
            } if bid == id => Some((spec, *interrupting, *failure_budget)),
            _ => None,
        })
    }

    /// Receipt 1 (GREEN): AttachGuard(host=t1, Timer::Duration) -> the
    /// guard is projected, interrupting, hosted on t1's id; wiring an
    /// outgoing flow (verifier 7c needs one) and admitting the full chain
    /// succeeds.
    #[test]
    fn attach_interrupting_timer_guard_admits() {
        let (base, _s, t1, _e) = linear("recv9-1");
        let guard_key = key();
        let staged = apply(
            &base,
            Operation::AttachGuard {
                host: t1,
                key: guard_key,
                guard_id: "guard1".into(),
                trigger: GuardTrigger::Timer(TimerSpec::Duration { ms: 60_000 }),
            },
            Provenance::default(),
        )
        .expect("attach interrupting timer guard must succeed");

        // The guard needs its OWN outgoing escape path (verifier 7c) —
        // mirroring the production boundary-timer graph shape (host and
        // guard each terminate their own branch, they don't merge back
        // into a shared End without a join).
        let staged = apply(
            &staged.candidate,
            Operation::AppendNode {
                anchor: guard_key,
                key: key(),
                node: end_node("guard1_end"),
                edge_id: "g1_out".into(),
            },
            Provenance::default(),
        )
        .expect("guard needs its own outgoing escape flow to satisfy the verifier's 7c gate");

        let ir = staged.candidate.to_ir().unwrap();
        let (_, interrupting, _) = find_boundary_timer(&ir, "guard1").expect("guard projected");
        assert!(interrupting, "AttachGuard must produce an INTERRUPTING guard");
        let attached_to = ir
            .node_indices()
            .find_map(|i| match &ir[i] {
                IRNode::BoundaryTimer { id, attached_to, .. } if id == "guard1" => {
                    Some(attached_to.clone())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(attached_to, "t1", "guard must be projected with host's current id");

        staged.candidate.admit().expect("interrupting duration guard must admit");
    }

    /// Receipt 2 (GREEN): AttachRearmingGuard with a Cycle timer — cycle
    /// on non-interrupting is exactly the legal combination boundary
    /// rule 6 carves out; admits.
    #[test]
    fn attach_rearming_cycle_guard_admits() {
        let (base, _s, t1, _e) = linear("recv9-2");
        let guard_key = key();
        let staged = apply(
            &base,
            Operation::AttachRearmingGuard {
                host: t1,
                key: guard_key,
                guard_id: "guard2".into(),
                trigger: GuardTrigger::Timer(TimerSpec::Cycle {
                    interval_ms: 5_000,
                    max_fires: 3,
                }),
            },
            Provenance::default(),
        )
        .expect("attach rearming cycle guard must succeed");

        let staged = apply(
            &staged.candidate,
            Operation::AppendNode {
                anchor: guard_key,
                key: key(),
                node: end_node("guard2_end"),
                edge_id: "g2_out".into(),
            },
            Provenance::default(),
        )
        .expect("guard needs its own outgoing escape flow");

        let ir = staged.candidate.to_ir().unwrap();
        let (spec, interrupting, _) = find_boundary_timer(&ir, "guard2").expect("guard projected");
        assert!(!interrupting, "AttachRearmingGuard must produce a NON-interrupting guard");
        assert!(matches!(spec, TimerSpec::Cycle { .. }));

        staged.candidate.admit().expect("non-interrupting cycle guard must admit");
    }

    /// Receipt 3 (RED): AttachGuard with a Cycle trigger is refused at
    /// apply, naming the guard id — boundary rule 6 (cycle is legal ONLY
    /// on a non-interrupting guard). Companion bypass-parity check: the
    /// same shape built directly via schema mutators (bypassing ops.rs
    /// entirely) is also refused by the real verifier backstop.
    #[test]
    fn cycle_on_interrupting_refused_at_apply_and_by_verifier_backstop() {
        let (base, _s, t1, _e) = linear("recv9-3");
        let err = apply(
            &base,
            Operation::AttachGuard {
                host: t1,
                key: key(),
                guard_id: "bad_cycle_guard".into(),
                trigger: GuardTrigger::Timer(TimerSpec::Cycle {
                    interval_ms: 1_000,
                    max_fires: 2,
                }),
            },
            Provenance::default(),
        )
        .expect_err("cycle-on-interrupting must be refused at apply");
        assert!(
            err.to_string().contains("bad_cycle_guard"),
            "refusal must name the guard id: {err}"
        );

        // Bypass parity: construct the identical shape directly via the
        // schema mutators (no ops.rs involved) and confirm admit()'s
        // verifier backstop rejects it too (7b).
        let mut dag = DesignerDag::new("recv9-3-bypass");
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let t = dag.insert_node(key(), task("t1"), None, Provenance::default()).unwrap();
        let e = dag
            .insert_node(key(), end_node("end"), None, Provenance::default())
            .unwrap();
        let g = dag
            .insert_node(
                key(),
                IRNode::BoundaryTimer {
                    id: "bad_cycle_guard".into(),
                    attached_to: "t1".into(),
                    spec: TimerSpec::Cycle {
                        interval_ms: 1_000,
                        max_fires: 2,
                    },
                    interrupting: true,
                    failure_budget: None,
                },
                Some(t),
                Provenance::default(),
            )
            .unwrap();
        dag.insert_edge(
            s,
            t,
            DesignerEdge { id: "e1".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            t,
            e,
            DesignerEdge { id: "e2".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            g,
            e,
            DesignerEdge { id: "g_out".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        let errs = dag.admit().expect_err("verifier backstop must also reject cycle-on-interrupting");
        assert!(
            errs.iter().any(|e| e.message.to_lowercase().contains("non-interrupting")),
            "verifier diagnostic must name the rule: {errs:?}"
        );
    }

    /// Receipt 4 (RED): AttachRearmingGuard with an Error trigger is
    /// refused (F2a mirror: BoundaryError is interrupting-only in this
    /// substrate), naming the guard id.
    #[test]
    fn rearming_error_guard_refused() {
        let (base, _s, t1, _e) = linear("recv9-4");
        let err = apply(
            &base,
            Operation::AttachRearmingGuard {
                host: t1,
                key: key(),
                guard_id: "bad_rearm_error".into(),
                trigger: GuardTrigger::Error { error_code: None },
            },
            Provenance::default(),
        )
        .expect_err("rearming error guard must be refused");
        assert!(
            err.to_string().contains("bad_rearm_error"),
            "refusal must name the guard id: {err}"
        );
    }

    /// Receipt 5 (GREEN): SetGuardBudget(Some(9)) on an attached guard ->
    /// the projected IR carries 9. The envelope's `v2_guard_budgets`
    /// table receiving it is proven by `admit()` running the FULL
    /// production chain (lowering + envelope construction) unmodified;
    /// this test asserts the to_ir() field directly per the brief's
    /// documented fallback, with admission itself as the end-to-end
    /// witness that nothing gets dropped between here and the sealed
    /// artifact (see `v8_boundary_failure_budget_lowers_into_v2_guard_budgets`
    /// in bpmn-lite-compiler for the lowering-level proof of that link).
    #[test]
    fn set_guard_budget_reaches_projected_ir() {
        let (base, _s, t1, _e) = linear("recv9-5");
        let guard_key = key();
        let staged = apply(
            &base,
            Operation::AttachGuard {
                host: t1,
                key: guard_key,
                guard_id: "guard5".into(),
                trigger: GuardTrigger::Timer(TimerSpec::Duration { ms: 30_000 }),
            },
            Provenance::default(),
        )
        .expect("attach must succeed");
        let staged = apply(
            &staged.candidate,
            Operation::AppendNode {
                anchor: guard_key,
                key: key(),
                node: end_node("guard5_end"),
                edge_id: "g5_out".into(),
            },
            Provenance::default(),
        )
        .expect("guard needs its own outgoing escape flow");
        let staged = apply(
            &staged.candidate,
            Operation::SetGuardBudget {
                guard: guard_key,
                failure_budget: Some(9),
            },
            Provenance::default(),
        )
        .expect("SetGuardBudget on a guard must succeed");

        let ir = staged.candidate.to_ir().unwrap();
        let (_, _, budget) = find_boundary_timer(&ir, "guard5").expect("guard projected");
        assert_eq!(budget, Some(9), "declared budget must ride the projection");

        staged.candidate.admit().expect("budgeted guard must still admit end to end");
    }

    /// Receipt 6 (RED): SetGuardBudget on a ServiceTask (not a guard) is
    /// refused naming the id (I24).
    #[test]
    fn set_guard_budget_on_non_guard_refused() {
        let (base, _s, t1, _e) = linear("recv9-6");
        let err = apply(
            &base,
            Operation::SetGuardBudget {
                guard: t1,
                failure_budget: Some(3),
            },
            Provenance::default(),
        )
        .expect_err("SetGuardBudget on a ServiceTask must be refused");
        assert!(err.to_string().contains("t1"), "refusal must name the node id: {err}");
    }

    /// Receipt 7: SetCorrelationSource GREEN on a MessageWait (field
    /// updated in the projection), RED on a ServiceTask.
    #[test]
    fn set_correlation_source_green_on_wait_red_on_task() {
        let mut dag = DesignerDag::new("recv9-7");
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let m = dag
            .insert_node(key(), message_wait("mw", "old_source"), None, Provenance::default())
            .unwrap();
        let t1 = dag.insert_node(key(), task("t1"), None, Provenance::default()).unwrap();
        let e = dag
            .insert_node(key(), end_node("end"), None, Provenance::default())
            .unwrap();
        dag.insert_edge(
            s,
            m,
            DesignerEdge { id: "e1".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            m,
            t1,
            DesignerEdge { id: "e2".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            t1,
            e,
            DesignerEdge { id: "e3".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();

        // GREEN: MessageWait.
        let staged = apply(
            &dag,
            Operation::SetCorrelationSource {
                node: m,
                corr_key_source: "new_source".into(),
            },
            Provenance::default(),
        )
        .expect("SetCorrelationSource on MessageWait must succeed");
        let ir = staged.candidate.to_ir().unwrap();
        let corr = ir
            .node_indices()
            .find_map(|i| match &ir[i] {
                IRNode::MessageWait { id, corr_key_source, .. } if id == "mw" => {
                    Some(corr_key_source.clone())
                }
                _ => None,
            })
            .expect("MessageWait projected");
        assert_eq!(corr, "new_source");

        // RED: ServiceTask.
        let err = apply(
            &dag,
            Operation::SetCorrelationSource {
                node: t1,
                corr_key_source: "x".into(),
            },
            Provenance::default(),
        )
        .expect_err("SetCorrelationSource on a ServiceTask must be refused");
        assert!(err.to_string().contains("t1"), "refusal must name the node id: {err}");
    }

    /// Receipt 8 (RED): SetGuardTrigger kind-switch is refused — a
    /// BoundaryError guard cannot be switched to carry a Timer trigger
    /// (kind-switch is delete + attach, an explicit two-op edit).
    #[test]
    fn set_guard_trigger_kind_switch_refused() {
        let (base, _s, t1, _e) = linear("recv9-8");
        let guard_key = key();
        let staged = apply(
            &base,
            Operation::AttachGuard {
                host: t1,
                key: guard_key,
                guard_id: "guard8".into(),
                trigger: GuardTrigger::Error { error_code: Some("boom".into()) },
            },
            Provenance::default(),
        )
        .expect("attach interrupting error guard must succeed");

        let err = apply(
            &staged.candidate,
            Operation::SetGuardTrigger {
                guard: guard_key,
                trigger: GuardTrigger::Timer(TimerSpec::Duration { ms: 1_000 }),
            },
            Provenance::default(),
        )
        .expect_err("switching a BoundaryError guard to a Timer trigger must be refused");
        assert!(
            err.to_string().contains("guard8"),
            "refusal must name the guard id: {err}"
        );
    }

    /// Receipt 9 (slice-1 polish): Connect's backward-edge refusal now
    /// names both BPMN ids alongside the keys.
    #[test]
    fn connect_refusal_names_bpmn_ids_alongside_keys() {
        let mut dag = DesignerDag::new("recv9-9");
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let a = dag.insert_node(key(), task("a"), None, Provenance::default()).unwrap();
        let b = dag.insert_node(key(), task("b"), None, Provenance::default()).unwrap();
        let e = dag
            .insert_node(key(), end_node("end"), None, Provenance::default())
            .unwrap();
        dag.insert_edge(
            s,
            a,
            DesignerEdge { id: "e1".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            a,
            b,
            DesignerEdge { id: "e2".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            b,
            e,
            DesignerEdge { id: "e3".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();

        let err = apply(
            &dag,
            Operation::Connect {
                from: b,
                to: a,
                edge_id: "back".into(),
                condition: None,
            },
            Provenance::default(),
        )
        .expect_err("backward connect must be refused");
        let msg = err.to_string();
        assert!(msg.contains(&format!("{b:?}")), "must still name from-key: {msg}");
        assert!(msg.contains(&format!("{a:?}")), "must still name to-key: {msg}");
        assert!(msg.contains("'a'"), "must name the from-node's BPMN id: {msg}");
        assert!(msg.contains("'b'"), "must name the to-node's BPMN id: {msg}");
    }
}
