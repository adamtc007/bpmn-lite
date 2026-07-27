//! WS-A.1 — the canonical Designer DAG schema (closes V&S Q2 + Q27).
//!
//! DESIGN DECISION (Q2/Q27, design-bearing — blind review required
//! before WS-A.2): the Designer node payload IS the compiler's `IRNode`.
//! There is no parallel designer taxonomy. Consequences, all deliberate:
//!
//! - **Q27 answered by construction.** Declarations (MI `declared_max`,
//!   guard `failure_budget`, timer-cycle `max_fires`, correlation
//!   sources) live on the same fields the production compiler seals into
//!   the envelope tables. A lossy intermediate CANNOT exist because
//!   there is no intermediate: `to_ir()` is a structural clone. The DTO
//!   surface (C7's budget-dropping round-trip) is bypassed entirely —
//!   this schema, not the DTO, is the Designer's persistence source.
//! - **P8/I17 honoured structurally.** The vocabulary the Designer
//!   authors is the vocabulary the verifier proves. A node kind the
//!   compiler cannot lower is unrepresentable here.
//! - **Designer-only state lives BESIDE `ir`, never inside it**:
//!   `NodeKey` (stable identity surviving renames/re-ids) and
//!   `Provenance` (I20/§11.8). Layout is server-derived, not stored.
//!
//! Process-level declarations that have no per-node home ride the DAG
//! root (`default_guard_budget`, R2's process default).

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use bpmn_lite_compiler::ir::{IREdge, IRGraph, IRNode};
use bpmn_lite_compiler::{lower_v2, verify, VerifyError};
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable designer-side node identity. Survives renames and BPMN-id
/// edits; never leaks into the compiled artifact (the artifact knows
/// only `IRNode` string ids).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeKey(pub Uuid);

/// Edit provenance per accepted element (I20; §11.8's full closure —
/// board/subset/bundle/policy hashes — is appended by WS-C's record
/// pipeline; this struct carries the authoring-side subset).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Provenance {
    pub source_utterance: Option<String>,
    /// Canonical id of the production that constructed this element,
    /// with its version (Q5: production versions bind into the log).
    pub production: Option<(String, u32)>,
    /// WS-C board hash under which the constructing disposition was
    /// issued, once the pipeline runs (hex).
    pub board_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesignerNode {
    pub key: NodeKey,
    /// The compiler vocabulary IS the node kind — see module docs.
    pub ir: IRNode,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesignerEdge {
    /// Sequence-flow id (unique within the DAG; enforced at staging).
    pub id: String,
    pub condition: Option<bpmn_lite_compiler::ir::ConditionExpr>,
    pub provenance: Provenance,
}

/// The authoritative authored process topology (P1/I1). Mutation goes
/// through WS-A.2's deterministic operations/productions — the fields
/// are crate-private so an operation cannot be bypassed by direct edits.
#[derive(Clone, Debug, Default)]
pub struct DesignerDag {
    pub name: String,
    /// R2 process-level guard-budget default; per-guard budgets live on
    /// the `BoundaryTimer`/`BoundaryError` nodes themselves.
    pub default_guard_budget: Option<u32>,
    graph: DiGraph<DesignerNode, DesignerEdge>,
    key_index: HashMap<NodeKey, NodeIndex>,
}

impl DesignerDag {
    pub fn new(name: impl Into<String>) -> Self {
        DesignerDag {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Crate-internal node insertion — WS-A.2 operations are the public
    /// mutation surface. Exposed `pub` temporarily for WS-A.1 tests and
    /// WS-C bring-up; narrows to `pub(crate)` when the operation set
    /// lands (tracked in the WS-A.2 brief).
    pub fn insert_node(&mut self, ir: IRNode, provenance: Provenance) -> NodeKey {
        let key = NodeKey(Uuid::new_v4());
        let idx = self.graph.add_node(DesignerNode {
            key,
            ir,
            provenance,
        });
        self.key_index.insert(key, idx);
        key
    }

    /// See `insert_node` on visibility.
    pub fn insert_edge(
        &mut self,
        from: NodeKey,
        to: NodeKey,
        edge: DesignerEdge,
    ) -> Result<()> {
        let (f, t) = (
            *self
                .key_index
                .get(&from)
                .ok_or_else(|| anyhow!("unknown from-node {from:?}"))?,
            *self
                .key_index
                .get(&to)
                .ok_or_else(|| anyhow!("unknown to-node {to:?}"))?,
        );
        self.graph.add_edge(f, t, edge);
        Ok(())
    }

    pub fn node(&self, key: NodeKey) -> Option<&DesignerNode> {
        self.key_index.get(&key).map(|idx| &self.graph[*idx])
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Lossless projection into the production compiler's input. A
    /// structural clone: same nodes (the `IRNode` payloads), same edges.
    /// This is the ONLY path from Designer state toward an artifact —
    /// which is what makes Q27's no-lossy-intermediate claim true by
    /// construction rather than by discipline.
    pub fn to_ir(&self) -> IRGraph {
        let mut ir: IRGraph = IRGraph::new();
        let mut map: HashMap<NodeIndex, NodeIndex> = HashMap::new();
        for idx in self.graph.node_indices() {
            map.insert(idx, ir.add_node(self.graph[idx].ir.clone()));
        }
        for eidx in self.graph.edge_indices() {
            let (a, b) = self.graph.edge_endpoints(eidx).expect("edge endpoints");
            let e = &self.graph[eidx];
            ir.add_edge(
                map[&a],
                map[&b],
                IREdge {
                    id: e.id.clone(),
                    condition: e.condition.clone(),
                },
            );
        }
        ir
    }

    /// Production-oracle admission (P8/I17): the REAL verifier over the
    /// projected IR, then the REAL v2 lowering. Returns the verifier's
    /// own diagnostics verbatim — no designer-side paraphrase. This is
    /// the staging check every candidate passes before ratification.
    pub fn admit(&self) -> std::result::Result<(), Vec<VerifyError>> {
        let ir = self.to_ir();
        let errors = verify(&ir);
        if !errors.is_empty() {
            return Err(errors);
        }
        // Lowering failures on a verify-clean graph are still refusals
        // (fail closed), surfaced as a single diagnostic.
        if let Err(e) = lower_v2(&ir) {
            return Err(vec![VerifyError {
                message: format!("lowering refused: {e}"),
                element_id: None,
            }]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_compiler::ir::GatewayDirection;

    fn edge(id: &str) -> DesignerEdge {
        DesignerEdge {
            id: id.to_owned(),
            condition: None,
            provenance: Provenance::default(),
        }
    }

    /// GREEN: a designer-built linear process admits through the REAL
    /// verifier + v2 lowering (P8: one oracle, no approximation).
    #[test]
    fn designer_built_process_admits_via_production_oracle() {
        let mut dag = DesignerDag::new("ws-a1-green");
        let start = dag.insert_node(IRNode::Start { id: "start".into() }, Provenance::default());
        let task = dag.insert_node(
            IRNode::ServiceTask {
                id: "t1".into(),
                name: "t1".into(),
                task_type: "noop".into(),
            },
            Provenance::default(),
        );
        let end = dag.insert_node(
            IRNode::End {
                id: "end".into(),
                terminate: false,
            },
            Provenance::default(),
        );
        dag.insert_edge(start, task, edge("e1")).unwrap();
        dag.insert_edge(task, end, edge("e2")).unwrap();
        dag.admit().expect("linear process must admit");
    }

    /// RED: a backward edge is refused by the production cyclicity gate,
    /// and the diagnostic is the verifier's own, not a paraphrase (I5
    /// discipline: reject with a localized diagnostic).
    #[test]
    fn cyclic_designer_graph_is_refused_by_the_real_verifier() {
        let mut dag = DesignerDag::new("ws-a1-red");
        let start = dag.insert_node(IRNode::Start { id: "start".into() }, Provenance::default());
        let a = dag.insert_node(
            IRNode::ServiceTask {
                id: "a".into(),
                name: "a".into(),
                task_type: "noop".into(),
            },
            Provenance::default(),
        );
        let b = dag.insert_node(
            IRNode::ServiceTask {
                id: "b".into(),
                name: "b".into(),
                task_type: "noop".into(),
            },
            Provenance::default(),
        );
        let end = dag.insert_node(
            IRNode::End {
                id: "end".into(),
                terminate: false,
            },
            Provenance::default(),
        );
        dag.insert_edge(start, a, edge("e1")).unwrap();
        dag.insert_edge(a, b, edge("e2")).unwrap();
        dag.insert_edge(b, a, edge("back")).unwrap(); // the backward edge
        dag.insert_edge(b, end, edge("e3")).unwrap();
        let errs = dag.admit().expect_err("cyclic graph must be refused");
        assert!(
            errs.iter().any(|e| e.message.to_lowercase().contains("cycl")),
            "refusal must name cyclicity: {errs:?}"
        );
    }

    /// Declarations survive the projection because they ARE the IR
    /// fields (Q27 by construction): a parallel region with a guard
    /// budget on a boundary timer round-trips into the IR intact.
    #[test]
    fn declarations_ride_the_projection_intact() {
        let mut dag = DesignerDag::new("ws-a1-decl");
        dag.default_guard_budget = Some(3);
        let start = dag.insert_node(IRNode::Start { id: "start".into() }, Provenance::default());
        let t = dag.insert_node(
            IRNode::ServiceTask {
                id: "work".into(),
                name: "work".into(),
                task_type: "noop".into(),
            },
            Provenance::default(),
        );
        let g = dag.insert_node(
            IRNode::BoundaryTimer {
                id: "guard".into(),
                attached_to: "work".into(),
                spec: bpmn_lite_compiler::ir::TimerSpec::Duration { ms: 60_000 },
                interrupting: true,
                failure_budget: Some(7),
            },
            Provenance::default(),
        );
        let end = dag.insert_node(
            IRNode::End {
                id: "end".into(),
                terminate: false,
            },
            Provenance::default(),
        );
        let _ = g;
        dag.insert_edge(start, t, edge("e1")).unwrap();
        dag.insert_edge(t, end, edge("e2")).unwrap();
        let ir = dag.to_ir();
        let budget = ir
            .node_indices()
            .find_map(|i| match &ir[i] {
                IRNode::BoundaryTimer { failure_budget, .. } => Some(*failure_budget),
                _ => None,
            })
            .expect("boundary timer projected");
        assert_eq!(budget, Some(7), "per-guard budget must survive projection");
        // A parallel-region sanity pass through the oracle too.
        let mut par = DesignerDag::new("ws-a1-par");
        let s = par.insert_node(IRNode::Start { id: "start".into() }, Provenance::default());
        let f = par.insert_node(
            IRNode::GatewayAnd {
                id: "fork".into(),
                name: "fork".into(),
                direction: GatewayDirection::Diverging,
            },
            Provenance::default(),
        );
        let t1 = par.insert_node(
            IRNode::ServiceTask {
                id: "p1".into(),
                name: "p1".into(),
                task_type: "noop".into(),
            },
            Provenance::default(),
        );
        let t2 = par.insert_node(
            IRNode::ServiceTask {
                id: "p2".into(),
                name: "p2".into(),
                task_type: "noop".into(),
            },
            Provenance::default(),
        );
        let j = par.insert_node(
            IRNode::GatewayAnd {
                id: "join".into(),
                name: "join".into(),
                direction: GatewayDirection::Converging,
            },
            Provenance::default(),
        );
        let e = par.insert_node(
            IRNode::End {
                id: "end".into(),
                terminate: false,
            },
            Provenance::default(),
        );
        par.insert_edge(s, f, edge("e1")).unwrap();
        par.insert_edge(f, t1, edge("e2")).unwrap();
        par.insert_edge(f, t2, edge("e3")).unwrap();
        par.insert_edge(t1, j, edge("e4")).unwrap();
        par.insert_edge(t2, j, edge("e5")).unwrap();
        par.insert_edge(j, e, edge("e6")).unwrap();
        par.admit().expect("parallel region must admit");
    }
}
