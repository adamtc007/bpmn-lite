//! WS-A.1 — the canonical Designer DAG schema (closes V&S Q2 + Q27).
//! Remediated per the WS-A.1 blind review (disposition in the plan doc §F).
//!
//! DESIGN DECISION (blind-reviewed, survives with riders): the Designer
//! node payload IS the compiler's `IRNode`. There is no parallel designer
//! taxonomy. Consequences, all deliberate:
//!
//! - **Q27 answered by construction FOR PER-NODE declarations** (review
//!   rider 1): MI `declared_max`, guard `failure_budget`, timer-cycle
//!   `max_fires`, correlation sources live on the fields the production
//!   compiler seals; `to_ir()` is a structural clone, so no per-node
//!   declaration can be dropped in an intermediate. Process-LEVEL
//!   declarations have no `IRNode` home and ride the DAG root
//!   (`default_guard_budget`), carried into admission explicitly by
//!   `admit()` — never through `to_ir()` alone.
//! - **P8/I17 honoured structurally.** A node kind the compiler cannot
//!   lower is unrepresentable here.
//! - **This schema is NOT the persistence format** (review rider 2;
//!   V&S §6.2/§12.5): the durable surface is the EDIT LOG with bound
//!   production versions; the DAG is its replay product. Any DAG
//!   snapshot persistence goes through a versioned envelope, never raw
//!   serde of these types.
//! - **Designer-side referential integrity is `NodeKey`-level** (review
//!   rider 3): intra-IR string references (`attached_to`) are
//!   artifact-facing vocabulary; the designer tracks attachment by
//!   `NodeKey` and `to_ir()` projects the CURRENT host id, so renames
//!   cannot dangle or silently re-point a guard.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use bpmn_lite_compiler::ir::{IREdge, IRGraph, IRNode};
use bpmn_lite_compiler::{verify, Compiler, VerifiedWorkflow, VerifyError};
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable designer-side node identity. Survives renames and BPMN-id
/// edits; never leaks into the compiled artifact. Keys are SUPPLIED by
/// the caller (the operation record owns key generation — Q5/§12.5:
/// edit-log replay must reconstruct identical keys, so the mutator must
/// not mint nondeterministic ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeKey(pub Uuid);

/// Edit provenance per accepted element (I20; §11.8's full closure is
/// appended by WS-C's record pipeline).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Provenance {
    pub source_utterance: Option<String>,
    /// Canonical id + version of the constructing production (Q5).
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
    /// Boundary-node attachment by designer identity (review F2): for
    /// `BoundaryTimer`/`BoundaryError` nodes this is the HOST's key;
    /// `to_ir()` projects the host's current BPMN id into `attached_to`,
    /// making rename-desync unconstructible. `None` for non-boundary
    /// nodes.
    pub attached_to_key: Option<NodeKey>,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesignerEdge {
    /// Sequence-flow id — uniqueness enforced at insertion (review F3).
    pub id: String,
    pub condition: Option<bpmn_lite_compiler::ir::ConditionExpr>,
    pub provenance: Provenance,
}

/// The authoritative authored process topology (P1/I1). The mutators are
/// `pub(crate)`: WS-A.2's deterministic operations/productions are the
/// public mutation surface (I18) — external crates cannot bypass them.
#[derive(Clone, Debug, Default)]
pub struct DesignerDag {
    pub name: String,
    /// R2 process-level guard-budget default; per-guard budgets live on
    /// the `Boundary*` nodes themselves. Carried into admission by
    /// `admit()` (review F1 — `to_ir()` alone cannot carry it).
    pub default_guard_budget: Option<u32>,
    graph: DiGraph<DesignerNode, DesignerEdge>,
    key_index: HashMap<NodeKey, NodeIndex>,
    bpmn_ids: HashSet<String>,
    edge_ids: HashSet<String>,
}

impl DesignerDag {
    pub fn new(name: impl Into<String>) -> Self {
        DesignerDag {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Insert a node under a caller-supplied key. Fail-closed on
    /// duplicate `NodeKey` or duplicate BPMN id (review F3: two nodes
    /// named "t1" would make `attached_to`/budget-key/correlation
    /// resolution ambiguous downstream — refused here, and a matching
    /// duplicate-id theorem now also lives in the production verifier).
    pub(crate) fn insert_node(
        &mut self,
        key: NodeKey,
        ir: IRNode,
        attached_to_key: Option<NodeKey>,
        provenance: Provenance,
    ) -> Result<NodeKey> {
        if self.key_index.contains_key(&key) {
            return Err(anyhow!("duplicate NodeKey {key:?}"));
        }
        let id = ir.id().to_owned();
        if !self.bpmn_ids.insert(id.clone()) {
            return Err(anyhow!("duplicate BPMN node id '{id}'"));
        }
        if let Some(host) = attached_to_key {
            if !self.key_index.contains_key(&host) {
                return Err(anyhow!("attached_to_key {host:?} names an unknown node"));
            }
            if !matches!(ir, IRNode::BoundaryTimer { .. } | IRNode::BoundaryError { .. }) {
                return Err(anyhow!(
                    "attached_to_key is only legal on boundary nodes (got '{id}')"
                ));
            }
        }
        let idx = self.graph.add_node(DesignerNode {
            key,
            ir,
            attached_to_key,
            provenance,
        });
        self.key_index.insert(key, idx);
        Ok(key)
    }

    /// Fail-closed on duplicate flow id (review F3).
    pub(crate) fn insert_edge(
        &mut self,
        from: NodeKey,
        to: NodeKey,
        edge: DesignerEdge,
    ) -> Result<()> {
        if !self.edge_ids.insert(edge.id.clone()) {
            return Err(anyhow!("duplicate sequence-flow id '{}'", edge.id));
        }
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

    /// Structural projection into the production compiler's input.
    /// Carries every PER-NODE declaration verbatim; boundary nodes get
    /// `attached_to` rewritten from their host's CURRENT id (review F2).
    /// Process-level declarations do NOT ride this projection — `admit()`
    /// passes them explicitly (review F1).
    pub fn to_ir(&self) -> Result<IRGraph> {
        let mut ir: IRGraph = IRGraph::new();
        let mut map: HashMap<NodeIndex, NodeIndex> = HashMap::new();
        for idx in self.graph.node_indices() {
            let node = &self.graph[idx];
            let mut payload = node.ir.clone();
            if let Some(host_key) = node.attached_to_key {
                let host_idx = self
                    .key_index
                    .get(&host_key)
                    .ok_or_else(|| anyhow!("attached_to_key {host_key:?} dangling"))?;
                let host_id = self.graph[*host_idx].ir.id().to_owned();
                match &mut payload {
                    IRNode::BoundaryTimer { attached_to, .. }
                    | IRNode::BoundaryError { attached_to, .. } => *attached_to = host_id,
                    _ => unreachable!("attached_to_key on non-boundary refused at insert"),
                }
            }
            map.insert(idx, ir.add_node(payload));
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
        Ok(ir)
    }

    /// Production-oracle admission — the FULL direct-compilation chain
    /// (review F5): `verify` (structured diagnostics verbatim) →
    /// `Compiler::lower_with_default` (= lowering + `verify_bytecode` +
    /// envelope construction + the types-crate V-1..V-11 pass via
    /// `from_verified_envelope`), carrying the process-level
    /// `default_guard_budget` (review F1). This is what G1 measures
    /// against direct compilation — same chain, same theorems, same
    /// diagnostics.
    pub fn admit(&self) -> std::result::Result<VerifiedWorkflow, Vec<VerifyError>> {
        let ir = self.to_ir().map_err(|e| {
            vec![VerifyError {
                message: format!("projection refused: {e}"),
                element_id: None,
            }]
        })?;
        let errors = verify(&ir);
        if !errors.is_empty() {
            return Err(errors);
        }
        Compiler::lower_with_default(&ir, self.default_guard_budget).map_err(|e| {
            vec![VerifyError {
                message: format!("{e}"),
                element_id: None,
            }]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_compiler::ir::GatewayDirection;

    fn key() -> NodeKey {
        NodeKey(Uuid::new_v4())
    }

    fn edge(id: &str) -> DesignerEdge {
        DesignerEdge {
            id: id.to_owned(),
            condition: None,
            provenance: Provenance::default(),
        }
    }

    fn task(id: &str) -> IRNode {
        IRNode::ServiceTask {
            id: id.into(),
            name: id.into(),
            task_type: "noop".into(),
        }
    }

    fn end() -> IRNode {
        IRNode::End {
            id: "end".into(),
            terminate: false,
        }
    }

    fn linear(name: &str) -> (DesignerDag, NodeKey, NodeKey, NodeKey) {
        let mut dag = DesignerDag::new(name);
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let t = dag.insert_node(key(), task("t1"), None, Provenance::default()).unwrap();
        let e = dag.insert_node(key(), end(), None, Provenance::default()).unwrap();
        dag.insert_edge(s, t, edge("e1")).unwrap();
        dag.insert_edge(t, e, edge("e2")).unwrap();
        (dag, s, t, e)
    }

    /// GREEN: full-chain admission (verify + lower + bytecode-verify +
    /// envelope V-1..V-11) on a designer-built process.
    #[test]
    fn designer_built_process_admits_via_full_production_chain() {
        let (dag, ..) = linear("ws-a1-green");
        dag.admit().expect("linear process must admit");
    }

    /// F1 red→green: the process-level default_guard_budget reaches the
    /// SEALED ENVELOPE. Red before remediation: admit() lowered with
    /// None and the artifact carried the conservative default while the
    /// designer said 3.
    #[test]
    fn process_default_guard_budget_reaches_the_sealed_envelope() {
        let (mut dag, ..) = linear("ws-a1-f1");
        dag.default_guard_budget = Some(3);
        let wf = dag.admit().expect("must admit");
        assert_eq!(
            wf.envelope().metadata().default_guard_budget().max_failures(),
            3,
            "designer-declared process default must be sealed into the artifact"
        );
        let (dag_none, ..) = linear("ws-a1-f1b");
        let wf_none = dag_none.admit().expect("must admit");
        assert_eq!(
            wf_none.envelope().metadata().default_guard_budget().max_failures(),
            bpmn_lite_types::transition::ScopeFailureBudget::conservative_default().max_failures(),
            "undeclared default must fall back to the compiled-in conservative default"
        );
    }

    /// RED: backward edge refused by the production cyclicity gate with
    /// the verifier's own diagnostic.
    #[test]
    fn cyclic_designer_graph_is_refused_by_the_real_verifier() {
        let mut dag = DesignerDag::new("ws-a1-red");
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let a = dag.insert_node(key(), task("a"), None, Provenance::default()).unwrap();
        let b = dag.insert_node(key(), task("b"), None, Provenance::default()).unwrap();
        let e = dag.insert_node(key(), end(), None, Provenance::default()).unwrap();
        dag.insert_edge(s, a, edge("e1")).unwrap();
        dag.insert_edge(a, b, edge("e2")).unwrap();
        dag.insert_edge(b, a, edge("back")).unwrap();
        dag.insert_edge(b, e, edge("e3")).unwrap();
        let errs = dag.admit().expect_err("cyclic graph must be refused");
        assert!(
            errs.iter().any(|e| e.message.to_lowercase().contains("cycl")),
            "refusal must name cyclicity: {errs:?}"
        );
    }

    /// F3 red: duplicate BPMN node id / duplicate flow id refused at
    /// insertion, naming the id.
    #[test]
    fn duplicate_ids_are_refused_at_insertion() {
        let mut dag = DesignerDag::new("ws-a1-f3");
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let t = dag.insert_node(key(), task("t1"), None, Provenance::default()).unwrap();
        let err = dag
            .insert_node(key(), task("t1"), None, Provenance::default())
            .expect_err("duplicate BPMN id must be refused");
        assert!(err.to_string().contains("t1"));
        dag.insert_edge(s, t, edge("e1")).unwrap();
        let err = dag
            .insert_edge(s, t, edge("e1"))
            .expect_err("duplicate flow id must be refused");
        assert!(err.to_string().contains("e1"));
    }

    /// F2 red→green: attachment is NodeKey-level; to_ir() projects the
    /// host's CURRENT id even when the IR payload's attached_to string
    /// is stale, so a rename can neither dangle nor re-point the guard.
    #[test]
    fn boundary_attachment_projects_the_hosts_current_id() {
        let mut dag = DesignerDag::new("ws-a1-f2");
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let host = dag
            .insert_node(key(), task("renamed_work"), None, Provenance::default())
            .unwrap();
        let e = dag.insert_node(key(), end(), None, Provenance::default()).unwrap();
        dag.insert_node(
            key(),
            IRNode::BoundaryTimer {
                id: "guard".into(),
                attached_to: "stale_old_name".into(), // deliberately stale
                spec: bpmn_lite_compiler::ir::TimerSpec::Duration { ms: 60_000 },
                interrupting: true,
                failure_budget: Some(7),
            },
            Some(host),
            Provenance::default(),
        )
        .unwrap();
        dag.insert_edge(s, host, edge("e1")).unwrap();
        dag.insert_edge(host, e, edge("e2")).unwrap();
        let ir = dag.to_ir().unwrap();
        let attached = ir
            .node_indices()
            .find_map(|i| match &ir[i] {
                IRNode::BoundaryTimer { attached_to, .. } => Some(attached_to.clone()),
                _ => None,
            })
            .expect("boundary projected");
        assert_eq!(
            attached, "renamed_work",
            "projection must use the host's current id, not the stale string"
        );
        // Non-boundary attachment refused at insert.
        let mut dag2 = DesignerDag::new("ws-a1-f2b");
        let h = dag2
            .insert_node(key(), task("h"), None, Provenance::default())
            .unwrap();
        assert!(dag2
            .insert_node(key(), task("not_a_guard"), Some(h), Provenance::default())
            .is_err());
    }

    /// Declarations-survive test, no camouflage (review F1 note): the
    /// per-node budget assertion AND a full parallel-region admission.
    #[test]
    fn per_node_declarations_ride_the_projection_intact() {
        let mut dag = DesignerDag::new("ws-a1-decl");
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let t = dag.insert_node(key(), task("work"), None, Provenance::default()).unwrap();
        let e = dag.insert_node(key(), end(), None, Provenance::default()).unwrap();
        dag.insert_node(
            key(),
            IRNode::BoundaryTimer {
                id: "guard".into(),
                attached_to: "work".into(),
                spec: bpmn_lite_compiler::ir::TimerSpec::Duration { ms: 60_000 },
                interrupting: true,
                failure_budget: Some(7),
            },
            Some(t),
            Provenance::default(),
        )
        .unwrap();
        dag.insert_edge(s, t, edge("e1")).unwrap();
        dag.insert_edge(t, e, edge("e2")).unwrap();
        let ir = dag.to_ir().unwrap();
        let budget = ir
            .node_indices()
            .find_map(|i| match &ir[i] {
                IRNode::BoundaryTimer { failure_budget, .. } => Some(*failure_budget),
                _ => None,
            })
            .expect("boundary timer projected");
        assert_eq!(budget, Some(7));

        let mut par = DesignerDag::new("ws-a1-par");
        let s = par
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let f = par
            .insert_node(
                key(),
                IRNode::GatewayAnd {
                    id: "fork".into(),
                    name: "fork".into(),
                    direction: GatewayDirection::Diverging,
                },
                None,
                Provenance::default(),
            )
            .unwrap();
        let t1 = par.insert_node(key(), task("p1"), None, Provenance::default()).unwrap();
        let t2 = par.insert_node(key(), task("p2"), None, Provenance::default()).unwrap();
        let j = par
            .insert_node(
                key(),
                IRNode::GatewayAnd {
                    id: "join".into(),
                    name: "join".into(),
                    direction: GatewayDirection::Converging,
                },
                None,
                Provenance::default(),
            )
            .unwrap();
        let e = par.insert_node(key(), end(), None, Provenance::default()).unwrap();
        par.insert_edge(s, f, edge("e1")).unwrap();
        par.insert_edge(f, t1, edge("e2")).unwrap();
        par.insert_edge(f, t2, edge("e3")).unwrap();
        par.insert_edge(t1, j, edge("e4")).unwrap();
        par.insert_edge(t2, j, edge("e5")).unwrap();
        par.insert_edge(j, e, edge("e6")).unwrap();
        par.admit().expect("parallel region must admit");
    }
}
