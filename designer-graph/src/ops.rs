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
use bpmn_lite_compiler::ir::{ConditionExpr, IRNode};
use petgraph::algo::has_path_connecting;
use serde::{Deserialize, Serialize};

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
                return Err(anyhow!(
                    "Connect {from:?} -> {to:?} refused: a path already exists \
                     {to:?} -> {from:?}; adding this edge would close a cycle (I23)"
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
}
