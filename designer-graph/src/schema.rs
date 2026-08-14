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
use bpmn_lite_compiler::{verify, Compiler, VerifiedWorkflow, VerifyError};
use bpmn_lite_compiler::{IREdge, IRGraph, IRNode};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
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
pub(crate) struct DesignerNode {
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
pub(crate) struct DesignerEdge {
    /// Sequence-flow id — uniqueness enforced at insertion (review F3).
    pub id: String,
    pub condition: Option<bpmn_lite_compiler::ConditionExpr>,
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
    ///
    /// G5.1: tightened from `pub` to `pub(crate)` — until now this field
    /// had no `Operation` and was the one documented exception to "the
    /// mutators are pub(crate)" above (direct field assignment, test-only).
    /// `Operation::SetDefaultGuardBudget` is now the real mutation surface;
    /// cross-crate reads go through the `default_guard_budget()` accessor.
    pub(crate) default_guard_budget: Option<u32>,
    /// G5.2 — workflow-level default retry policy, same carve-out and same
    /// "no `IRNode` home, rides the DAG root" reasoning as
    /// `default_guard_budget` above. Raw/unvalidated (see
    /// `RetryPolicyDecl`'s doc comment); validated by `Compiler::lower_with_default`
    /// at admission time.
    pub(crate) default_retry_policy: Option<bpmn_lite_compiler::RetryPolicyDecl>,
    graph: DiGraph<DesignerNode, DesignerEdge>,
    key_index: HashMap<NodeKey, NodeIndex>,
    bpmn_ids: HashSet<String>,
    edge_ids: HashSet<String>,
}

/// A successful canonical-DSL emission for a [`DesignerDag`]: the
/// compiler's emitted source/AST/required-symbols plus this crate's
/// content-derived graph identity witness (`graph_state_hash` — NOT the
/// server's route-derived hashes; see [`DesignerDag::graph_state_hash`]'s
/// naming-trap warning). EOP-PLAN-GRAPH-DSL-BRIDGE-001 B1.
#[derive(Debug, Clone)]
pub struct DslReceipt {
    pub emitted: bpmn_lite_compiler::dsl::EmittedDsl,
    pub graph_state_hash: String,
}

impl DesignerDag {
    /// Cross-crate read access to the process-level guard-budget default
    /// (the field itself is `pub(crate)` — see its doc comment).
    pub fn default_guard_budget(&self) -> Option<u32> {
        self.default_guard_budget
    }

    /// Cross-crate read access to the process-level default retry policy
    /// (the field itself is `pub(crate)` — see its doc comment).
    pub fn default_retry_policy(&self) -> Option<bpmn_lite_compiler::RetryPolicyDecl> {
        self.default_retry_policy
    }
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
            if !matches!(
                ir,
                IRNode::BoundaryTimer { .. } | IRNode::BoundaryError { .. }
            ) {
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

    /// WS-B.4: resolve a caller-facing BPMN id (what a REST client or
    /// utterance body names as an anchor) to the internal `NodeKey` an
    /// `Operation`/`LegalityOracle` call needs. `None` for an unknown id
    /// — callers turn that into a fail-closed rejection, never a
    /// silent whole-graph fallback.
    pub fn key_for_bpmn_id(&self, id: &str) -> Option<NodeKey> {
        self.graph
            .node_weights()
            .find(|n| n.ir.id() == id)
            .map(|n| n.key)
    }

    /// Public base-graph seeding (DIR-002 Phase B: the corpus generator
    /// lives outside this crate and must construct base graphs). ONLY
    /// `Start` and `DataObject` nodes may be seeded — every flow node,
    /// edge, and guard arrives through the staged operation surface
    /// (`ops::apply`), which is where the refusal discipline lives.
    /// Seeding anything else is a typed reject, not a convenience.
    pub fn seed(&mut self, key: NodeKey, node: IRNode, provenance: Provenance) -> Result<NodeKey> {
        match &node {
            IRNode::Start { .. } | IRNode::DataObject { .. } => {
                self.insert_node(key, node, None, provenance)
            }
            other => Err(anyhow!(
                "seed refused: only Start/DataObject may be seeded (got '{}'); \
                 flow construction goes through the operation surface",
                other.id()
            )),
        }
    }

    pub(crate) fn node(&self, key: NodeKey) -> Option<&DesignerNode> {
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

    /// EOP-PLAN-GRAPH-DSL-BRIDGE-001 B1 — canonical `bpmn-dsl` source
    /// receipt for this DAG: the `to_ir()` projection, plus the
    /// process-level declarations `to_ir()` cannot carry (review F1),
    /// plus the content-derived identity witness. All emission logic
    /// (refusal catalogue, canonical ordering, printing) lives in
    /// [`bpmn_lite_compiler::dsl::emit_dsl`] — this wrapper is field
    /// plumbing only: the compiler sits below this crate and can know
    /// neither these two DAG-root fields nor this crate's
    /// [`Self::graph_state_hash`] identity. A DAG that sets either
    /// process-level declaration refuses
    /// (`DslEmitError::ProcessDeclUnrepresentable`) — the DSL grammar has
    /// no syntax for them; dropping them silently would be a trap door.
    pub fn emit_dsl(&self, workflow_id: &str) -> Result<DslReceipt> {
        let ir = self.to_ir()?;
        let decls = bpmn_lite_compiler::dsl::ProcessLevelDecls {
            default_guard_budget_set: self.default_guard_budget.is_some(),
            default_retry_policy_set: self.default_retry_policy.is_some(),
        };
        let emitted = bpmn_lite_compiler::dsl::emit_dsl(&ir, workflow_id, &decls)?;
        Ok(DslReceipt {
            graph_state_hash: Self::graph_state_hash(&ir),
            emitted,
        })
    }

    /// Structural equivalence of two reconstructed graphs by BPMN element
    /// identity — same node set, same per-node declared content, same edges
    /// by endpoint identity and condition. This compares resulting *state*,
    /// not the edit representation that produced it: internal `NodeKey`
    /// handles never reach `IRGraph` at all, and `IREdge::id` (the
    /// sequence-flow id string) is deliberately excluded from the edge
    /// comparison below — it is exactly the kind of workbook/edit-local
    /// synthesized identifier (like `edge_id`, `guard_id`, `fork_key`,
    /// `join_key`, `entry_edge_id`) that two independently authored but
    /// equivalent edits are never expected to agree on (v0.8 amendment,
    /// `EOP-PLAN-BPMN-GAMEBOARD-001.md` Phase 2 item 9).
    pub fn ir_graphs_equivalent(a: &IRGraph, b: &IRGraph) -> bool {
        if a.node_count() != b.node_count() {
            return false;
        }
        let nodes_a: HashMap<&str, &IRNode> = a.node_weights().map(|n| (n.id(), n)).collect();
        let nodes_b: HashMap<&str, &IRNode> = b.node_weights().map(|n| (n.id(), n)).collect();
        if nodes_a.len() != a.node_count() || nodes_b.len() != b.node_count() {
            // A duplicate bpmn id on one side collided in the map above;
            // never treat that as accidental agreement.
            return false;
        }
        if nodes_a.len() != nodes_b.len() {
            return false;
        }
        for (id, node_a) in &nodes_a {
            match nodes_b.get(id) {
                Some(node_b) if node_b == node_a => {}
                _ => return false,
            }
        }

        let edge_key = |graph: &IRGraph,
                         from: NodeIndex,
                         to: NodeIndex,
                         edge: &IREdge|
         -> (String, String, String) {
            (
                graph[from].id().to_owned(),
                graph[to].id().to_owned(),
                // `ConditionExpr` has no `Ord`; its `Debug` form is a stable
                // enough tie-breaker for sorting two same-endpoint edges.
                format!("{:?}", edge.condition),
            )
        };
        let mut edges_a: Vec<_> = a
            .edge_indices()
            .map(|idx| {
                let (from, to) = a.edge_endpoints(idx).expect("edge endpoints");
                edge_key(a, from, to, &a[idx])
            })
            .collect();
        let mut edges_b: Vec<_> = b
            .edge_indices()
            .map(|idx| {
                let (from, to) = b.edge_endpoints(idx).expect("edge endpoints");
                edge_key(b, from, to, &b[idx])
            })
            .collect();
        edges_a.sort();
        edges_b.sort();
        edges_a == edges_b
    }

    /// Content-derived graph identity (D23/I34, RESEARCH-002 §S2). A digest
    /// over the SAME canonicalisation `ir_graphs_equivalent` already proves
    /// correct — nodes sorted by BPMN id, edges sorted by
    /// `(from_id, to_id, condition_debug)`, `NodeKey`/edge-id synthesized
    /// identity excluded — collapsed into a hash instead of a comparator.
    ///
    /// This is a function of the graph *reached*, never the edit route
    /// taken to reach it: two structurally-identical-but-differently-edited
    /// graphs (`ir_graphs_equivalent(a, b) == true`) always produce the
    /// SAME `graph_state_hash`.
    ///
    /// **Naming trap, stated so it is not rediscovered the hard way:**
    /// `bpmn-lite-server-designer`'s `graph_identity_hash`/`graph_content_hash`
    /// (which become `GraphRevision`/`GraphContentHash`) are BOTH
    /// route-derived — they hash the edit-log payload strings in
    /// storage/event order, never `to_ir()` output, despite
    /// `graph_content_hash`'s name suggesting otherwise (confirmed
    /// empirically: two edit orders reaching an `ir_graphs_equivalent`
    /// graph produced different values for both). This function is the
    /// first genuinely content-derived identity in the stack; callers must
    /// name which identity they need (I34) rather than assume the
    /// existing "content" name already meant this.
    pub fn graph_state_hash(ir: &IRGraph) -> String {
        let mut nodes: Vec<(&str, &IRNode)> = ir.node_weights().map(|n| (n.id(), n)).collect();
        nodes.sort_by_key(|(id, _)| *id);

        let edge_key = |graph: &IRGraph,
                         from: NodeIndex,
                         to: NodeIndex,
                         edge: &IREdge|
         -> (String, String, String) {
            (
                graph[from].id().to_owned(),
                graph[to].id().to_owned(),
                format!("{:?}", edge.condition),
            )
        };
        let mut edges: Vec<(String, String, String)> = ir
            .edge_indices()
            .map(|idx| {
                let (from, to) = ir.edge_endpoints(idx).expect("edge endpoints");
                edge_key(ir, from, to, &ir[idx])
            })
            .collect();
        edges.sort();

        let mut hasher = blake3::Hasher::new();
        hasher.update(b"bpmn-lite-designer-graph-state-v1\0");
        hasher.update(b"nodes\0");
        hasher.update(&(nodes.len() as u64).to_be_bytes());
        for (id, node) in &nodes {
            let node_json =
                serde_json::to_vec(node).expect("IRNode serializes (Serialize, no skip fields)");
            hasher.update(&(id.len() as u64).to_be_bytes());
            hasher.update(id.as_bytes());
            hasher.update(&(node_json.len() as u64).to_be_bytes());
            hasher.update(&node_json);
        }
        hasher.update(b"edges\0");
        hasher.update(&(edges.len() as u64).to_be_bytes());
        for (from, to, cond) in &edges {
            hasher.update(&(from.len() as u64).to_be_bytes());
            hasher.update(from.as_bytes());
            hasher.update(&(to.len() as u64).to_be_bytes());
            hasher.update(to.as_bytes());
            hasher.update(&(cond.len() as u64).to_be_bytes());
            hasher.update(cond.as_bytes());
        }
        hasher.finalize().to_hex().to_string()
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
        Compiler::lower_with_default(&ir, self.default_guard_budget, self.default_retry_policy)
            .map_err(|e| {
                vec![VerifyError {
                    message: format!("{e}"),
                    element_id: None,
                }]
            })
    }

    // ── ops.rs support (WS-A.2) — NOT a public mutation surface ──────────
    // The public mutation surface is `designer_graph::ops::apply` (I18).
    // These are read/mutate primitives the deterministic operations need;
    // none of them decide policy, mint identity, or run admission.

    /// ops.rs support: petgraph index for a designer key, so I23's
    /// pre-gate (`petgraph::algo::has_path_connecting`) can run against
    /// the real graph. `None` if `key` is unknown.
    pub(crate) fn index_of(&self, key: NodeKey) -> Option<NodeIndex> {
        self.key_index.get(&key).copied()
    }

    /// ops.rs support: read-only access to the underlying graph for I23's
    /// forward-only pre-gate. Not a public mutation surface.
    pub(crate) fn graph(&self) -> &DiGraph<DesignerNode, DesignerEdge> {
        &self.graph
    }

    /// ops.rs support: target keys of `key`'s outgoing edges, for
    /// InsertAfter's re-pointing. Empty if `key` is unknown.
    pub(crate) fn successors(&self, key: NodeKey) -> Vec<NodeKey> {
        let idx = match self.key_index.get(&key) {
            Some(&idx) => idx,
            None => return Vec::new(),
        };
        self.graph
            .edges_directed(idx, Direction::Outgoing)
            .map(|e| self.graph[e.target()].key)
            .collect()
    }

    /// ops.rs support: source keys of `key`'s incoming edges, for
    /// InsertBefore's re-pointing. Empty if `key` is unknown.
    pub(crate) fn predecessors(&self, key: NodeKey) -> Vec<NodeKey> {
        let idx = match self.key_index.get(&key) {
            Some(&idx) => idx,
            None => return Vec::new(),
        };
        self.graph
            .edges_directed(idx, Direction::Incoming)
            .map(|e| self.graph[e.source()].key)
            .collect()
    }

    /// ops.rs support: remove the edge `from -> to` and return it (id,
    /// condition, provenance intact) so a caller can re-insert it under
    /// new endpoints without losing identity (I23/F4 — re-pointing must
    /// not mint a new flow id). Frees the edge id for reuse. Errs if
    /// either node or the edge itself is unknown.
    pub(crate) fn remove_edge_between(
        &mut self,
        from: NodeKey,
        to: NodeKey,
    ) -> Result<DesignerEdge> {
        let f = *self
            .key_index
            .get(&from)
            .ok_or_else(|| anyhow!("unknown from-node {from:?}"))?;
        let t = *self
            .key_index
            .get(&to)
            .ok_or_else(|| anyhow!("unknown to-node {to:?}"))?;
        let eidx = self
            .graph
            .find_edge(f, t)
            .ok_or_else(|| anyhow!("no edge {from:?} -> {to:?}"))?;
        let edge = self
            .graph
            .remove_edge(eidx)
            .expect("edge just located by find_edge");
        self.edge_ids.remove(&edge.id);
        Ok(edge)
    }

    /// ops.rs support (WS-A.2 slice 2): mutable access to a node's payload
    /// by key, for the guard/declaration Set* operations
    /// (`SetGuardTrigger`/`SetGuardBudget`/`SetCorrelationSource`), which
    /// mutate a single field IN PLACE — same key, same BPMN id, same
    /// topology, so none of `insert_node`'s identity/uniqueness checks
    /// apply. Not a public mutation surface; `None` if `key` is unknown.
    pub(crate) fn node_mut(&mut self, key: NodeKey) -> Option<&mut DesignerNode> {
        self.key_index
            .get(&key)
            .copied()
            .map(move |idx| &mut self.graph[idx])
    }

    /// ops.rs support: keys of nodes attached to `host` via
    /// `attached_to_key` (i.e. boundary guards hosted on `host`). Used by
    /// DeleteNode to refuse dangling attachments (review-F2's invariant:
    /// attachment is NodeKey-level, so a delete must not leave one
    /// pointing at nothing).
    pub(crate) fn nodes_attached_to(&self, host: NodeKey) -> Vec<NodeKey> {
        self.graph
            .node_weights()
            .filter(|n| n.attached_to_key == Some(host))
            .map(|n| n.key)
            .collect()
    }

    /// ops.rs support: remove `key` and all incident edges. Frees the
    /// node's BPMN id and any incident edge ids for reuse (receipt 6).
    /// Fixes up `key_index` for petgraph's swap-remove semantics (the
    /// node previously at the last `NodeIndex` moves into the removed
    /// slot). Errs if `key` is unknown. Caller (ops.rs) is responsible
    /// for the dangling-attachment refusal BEFORE calling this — this
    /// helper performs no such check itself.
    pub(crate) fn remove_node(&mut self, key: NodeKey) -> Result<()> {
        let idx = *self
            .key_index
            .get(&key)
            .ok_or_else(|| anyhow!("unknown node {key:?}"))?;
        let freed_edge_ids: Vec<String> = self
            .graph
            .edges_directed(idx, Direction::Outgoing)
            .map(|e| e.weight().id.clone())
            .chain(
                self.graph
                    .edges_directed(idx, Direction::Incoming)
                    .map(|e| e.weight().id.clone()),
            )
            .collect();
        let bpmn_id = self.graph[idx].ir.id().to_owned();
        let last = NodeIndex::new(self.graph.node_count() - 1);
        self.graph.remove_node(idx);
        self.key_index.remove(&key);
        self.bpmn_ids.remove(&bpmn_id);
        if last != idx {
            if let Some((&moved_key, _)) = self.key_index.iter().find(|(_, &v)| v == last) {
                self.key_index.insert(moved_key, idx);
            }
        }
        for id in freed_edge_ids {
            self.edge_ids.remove(&id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_compiler::GatewayDirection;

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
            loop_origin: None,
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
            .insert_node(
                key(),
                IRNode::Start { id: "start".into() },
                None,
                Provenance::default(),
            )
            .unwrap();
        let t = dag
            .insert_node(key(), task("t1"), None, Provenance::default())
            .unwrap();
        let e = dag
            .insert_node(key(), end(), None, Provenance::default())
            .unwrap();
        dag.insert_edge(s, t, edge("e1")).unwrap();
        dag.insert_edge(t, e, edge("e2")).unwrap();
        (dag, s, t, e)
    }

    /// v0.8: two independently authored DAGs with different `NodeKey`s and
    /// different edge ids but identical BPMN-visible content/topology are
    /// equivalent — the comparator proves resulting *state*, not the edit
    /// representation (synthesized keys/edge ids never enter it).
    #[test]
    fn ir_graphs_equivalent_ignores_synthesized_key_and_edge_identity() {
        let (dag_a, ..) = linear("ir-eq-a");
        let mut dag_b = DesignerDag::new("ir-eq-b");
        let s = dag_b
            .insert_node(
                key(),
                IRNode::Start { id: "start".into() },
                None,
                Provenance::default(),
            )
            .unwrap();
        let t = dag_b
            .insert_node(key(), task("t1"), None, Provenance::default())
            .unwrap();
        let e = dag_b
            .insert_node(key(), end(), None, Provenance::default())
            .unwrap();
        dag_b.insert_edge(s, t, edge("totally-different-edge-id-1")).unwrap();
        dag_b.insert_edge(t, e, edge("totally-different-edge-id-2")).unwrap();

        assert!(DesignerDag::ir_graphs_equivalent(
            &dag_a.to_ir().unwrap(),
            &dag_b.to_ir().unwrap()
        ));
    }

    /// v0.8 RED: divergent task content (different declared name) is caught
    /// even though topology and ids otherwise line up.
    #[test]
    fn ir_graphs_equivalent_catches_node_content_divergence() {
        let (dag_a, ..) = linear("ir-eq-content-a");
        let mut dag_b = DesignerDag::new("ir-eq-content-b");
        let s = dag_b
            .insert_node(
                key(),
                IRNode::Start { id: "start".into() },
                None,
                Provenance::default(),
            )
            .unwrap();
        let t = dag_b
            .insert_node(
                key(),
                IRNode::ServiceTask {
                    id: "t1".into(),
                    name: "a different declared name".into(),
                    task_type: "noop".into(),
                    loop_origin: None,
                },
                None,
                Provenance::default(),
            )
            .unwrap();
        let e = dag_b
            .insert_node(key(), end(), None, Provenance::default())
            .unwrap();
        dag_b.insert_edge(s, t, edge("e1")).unwrap();
        dag_b.insert_edge(t, e, edge("e2")).unwrap();

        assert!(!DesignerDag::ir_graphs_equivalent(
            &dag_a.to_ir().unwrap(),
            &dag_b.to_ir().unwrap()
        ));
    }

    /// G1.1/D23: `graph_state_hash` is content-derived — two independently
    /// authored DAGs with different `NodeKey`s and edge ids but identical
    /// BPMN-visible content/topology hash IDENTICAL, mirroring the
    /// equivalence comparator's own claim above but for the digest form.
    #[test]
    fn graph_state_hash_ignores_synthesized_key_and_edge_identity() {
        let (dag_a, ..) = linear("state-hash-eq-a");
        let mut dag_b = DesignerDag::new("state-hash-eq-b");
        let s = dag_b
            .insert_node(
                key(),
                IRNode::Start { id: "start".into() },
                None,
                Provenance::default(),
            )
            .unwrap();
        let t = dag_b
            .insert_node(key(), task("t1"), None, Provenance::default())
            .unwrap();
        let e = dag_b
            .insert_node(key(), end(), None, Provenance::default())
            .unwrap();
        dag_b.insert_edge(s, t, edge("totally-different-edge-id-1")).unwrap();
        dag_b.insert_edge(t, e, edge("totally-different-edge-id-2")).unwrap();

        assert_eq!(
            DesignerDag::graph_state_hash(&dag_a.to_ir().unwrap()),
            DesignerDag::graph_state_hash(&dag_b.to_ir().unwrap())
        );
    }

    /// G1.1: two edit ROUTES that reach the same structural graph produce
    /// the same content hash — the empirical claim RESEARCH-002/S2.2 proved
    /// false for the route-derived server-side hashes. Here, insertion
    /// order of the two branch edges differs between `dag_a`/`dag_b`; the
    /// resulting `IRGraph` is the same either way.
    #[test]
    fn graph_state_hash_is_route_independent() {
        let mut dag_a = DesignerDag::new("state-hash-route-a");
        let gw_a = dag_a
            .insert_node(
                key(),
                IRNode::GatewayXor {
                    id: "gw".into(),
                    name: "gw".into(),
                    direction: GatewayDirection::Diverging,
                },
                None,
                Provenance::default(),
            )
            .unwrap();
        let x_a = dag_a
            .insert_node(key(), task("x"), None, Provenance::default())
            .unwrap();
        let y_a = dag_a
            .insert_node(key(), task("y"), None, Provenance::default())
            .unwrap();
        // route A: edge to x inserted before edge to y
        dag_a.insert_edge(gw_a, x_a, edge("e-x")).unwrap();
        dag_a.insert_edge(gw_a, y_a, edge("e-y")).unwrap();

        let mut dag_b = DesignerDag::new("state-hash-route-b");
        let gw_b = dag_b
            .insert_node(
                key(),
                IRNode::GatewayXor {
                    id: "gw".into(),
                    name: "gw".into(),
                    direction: GatewayDirection::Diverging,
                },
                None,
                Provenance::default(),
            )
            .unwrap();
        let y_b = dag_b
            .insert_node(key(), task("y"), None, Provenance::default())
            .unwrap();
        let x_b = dag_b
            .insert_node(key(), task("x"), None, Provenance::default())
            .unwrap();
        // route B: y and x nodes AND edges inserted in the opposite order
        dag_b.insert_edge(gw_b, y_b, edge("e-y")).unwrap();
        dag_b.insert_edge(gw_b, x_b, edge("e-x")).unwrap();

        assert_eq!(
            DesignerDag::graph_state_hash(&dag_a.to_ir().unwrap()),
            DesignerDag::graph_state_hash(&dag_b.to_ir().unwrap())
        );
    }

    /// G1.1 RED: divergent task content (different declared name) changes
    /// the hash, mirroring the equivalence comparator's content-divergence
    /// claim.
    #[test]
    fn graph_state_hash_catches_node_content_divergence() {
        let (dag_a, ..) = linear("state-hash-content-a");
        let mut dag_b = DesignerDag::new("state-hash-content-b");
        let s = dag_b
            .insert_node(
                key(),
                IRNode::Start { id: "start".into() },
                None,
                Provenance::default(),
            )
            .unwrap();
        let t = dag_b
            .insert_node(
                key(),
                IRNode::ServiceTask {
                    id: "t1".into(),
                    name: "a different declared name".into(),
                    task_type: "noop".into(),
                    loop_origin: None,
                },
                None,
                Provenance::default(),
            )
            .unwrap();
        let e = dag_b
            .insert_node(key(), end(), None, Provenance::default())
            .unwrap();
        dag_b.insert_edge(s, t, edge("e1")).unwrap();
        dag_b.insert_edge(t, e, edge("e2")).unwrap();

        assert_ne!(
            DesignerDag::graph_state_hash(&dag_a.to_ir().unwrap()),
            DesignerDag::graph_state_hash(&dag_b.to_ir().unwrap())
        );
    }

    /// v0.8 RED: same node set, but an edge condition diverges — caught by
    /// the edge comparison, not just the node-content comparison.
    #[test]
    fn ir_graphs_equivalent_catches_edge_condition_divergence() {
        let mut dag_a = DesignerDag::new("ir-eq-cond-a");
        let gw_a = dag_a
            .insert_node(
                key(),
                IRNode::GatewayXor {
                    id: "gw".into(),
                    name: "gw".into(),
                    direction: GatewayDirection::Diverging,
                },
                None,
                Provenance::default(),
            )
            .unwrap();
        let t_a = dag_a
            .insert_node(key(), task("t1"), None, Provenance::default())
            .unwrap();
        dag_a
            .insert_edge(
                gw_a,
                t_a,
                DesignerEdge {
                    id: "e1".into(),
                    condition: Some(bpmn_lite_compiler::ConditionExpr {
                        flag_name: "approved".into(),
                        op: bpmn_lite_compiler::ConditionOp::Eq,
                        literal: bpmn_lite_compiler::ConditionLiteral::Bool(true),
                    }),
                    provenance: Provenance::default(),
                },
            )
            .unwrap();

        let mut dag_b = DesignerDag::new("ir-eq-cond-b");
        let gw_b = dag_b
            .insert_node(
                key(),
                IRNode::GatewayXor {
                    id: "gw".into(),
                    name: "gw".into(),
                    direction: GatewayDirection::Diverging,
                },
                None,
                Provenance::default(),
            )
            .unwrap();
        let t_b = dag_b
            .insert_node(key(), task("t1"), None, Provenance::default())
            .unwrap();
        dag_b
            .insert_edge(
                gw_b,
                t_b,
                DesignerEdge {
                    id: "different-edge-id".into(),
                    condition: Some(bpmn_lite_compiler::ConditionExpr {
                        flag_name: "approved".into(),
                        op: bpmn_lite_compiler::ConditionOp::Eq,
                        literal: bpmn_lite_compiler::ConditionLiteral::Bool(false),
                    }),
                    provenance: Provenance::default(),
                },
            )
            .unwrap();

        assert!(!DesignerDag::ir_graphs_equivalent(
            &dag_a.to_ir().unwrap(),
            &dag_b.to_ir().unwrap()
        ));
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
            wf.envelope()
                .metadata()
                .default_guard_budget()
                .max_failures(),
            3,
            "designer-declared process default must be sealed into the artifact"
        );
        let (dag_none, ..) = linear("ws-a1-f1b");
        let wf_none = dag_none.admit().expect("must admit");
        assert_eq!(
            wf_none
                .envelope()
                .metadata()
                .default_guard_budget()
                .max_failures(),
            bpmn_lite_types::ScopeFailureBudget::conservative_default().max_failures(),
            "undeclared default must fall back to the compiled-in conservative default"
        );
    }

    /// RED: backward edge refused by the production cyclicity gate with
    /// the verifier's own diagnostic.
    #[test]
    fn cyclic_designer_graph_is_refused_by_the_real_verifier() {
        let mut dag = DesignerDag::new("ws-a1-red");
        let s = dag
            .insert_node(
                key(),
                IRNode::Start { id: "start".into() },
                None,
                Provenance::default(),
            )
            .unwrap();
        let a = dag
            .insert_node(key(), task("a"), None, Provenance::default())
            .unwrap();
        let b = dag
            .insert_node(key(), task("b"), None, Provenance::default())
            .unwrap();
        let e = dag
            .insert_node(key(), end(), None, Provenance::default())
            .unwrap();
        dag.insert_edge(s, a, edge("e1")).unwrap();
        dag.insert_edge(a, b, edge("e2")).unwrap();
        dag.insert_edge(b, a, edge("back")).unwrap();
        dag.insert_edge(b, e, edge("e3")).unwrap();
        let errs = dag.admit().expect_err("cyclic graph must be refused");
        assert!(
            errs.iter()
                .any(|e| e.message.to_lowercase().contains("cycl")),
            "refusal must name cyclicity: {errs:?}"
        );
    }

    /// F3 red: duplicate BPMN node id / duplicate flow id refused at
    /// insertion, naming the id.
    #[test]
    fn duplicate_ids_are_refused_at_insertion() {
        let mut dag = DesignerDag::new("ws-a1-f3");
        let s = dag
            .insert_node(
                key(),
                IRNode::Start { id: "start".into() },
                None,
                Provenance::default(),
            )
            .unwrap();
        let t = dag
            .insert_node(key(), task("t1"), None, Provenance::default())
            .unwrap();
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
            .insert_node(
                key(),
                IRNode::Start { id: "start".into() },
                None,
                Provenance::default(),
            )
            .unwrap();
        let host = dag
            .insert_node(key(), task("renamed_work"), None, Provenance::default())
            .unwrap();
        let e = dag
            .insert_node(key(), end(), None, Provenance::default())
            .unwrap();
        dag.insert_node(
            key(),
            IRNode::BoundaryTimer {
                id: "guard".into(),
                attached_to: "stale_old_name".into(), // deliberately stale
                spec: bpmn_lite_compiler::TimerSpec::Duration { ms: 60_000 },
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
            .insert_node(
                key(),
                IRNode::Start { id: "start".into() },
                None,
                Provenance::default(),
            )
            .unwrap();
        let t = dag
            .insert_node(key(), task("work"), None, Provenance::default())
            .unwrap();
        let e = dag
            .insert_node(key(), end(), None, Provenance::default())
            .unwrap();
        dag.insert_node(
            key(),
            IRNode::BoundaryTimer {
                id: "guard".into(),
                attached_to: "work".into(),
                spec: bpmn_lite_compiler::TimerSpec::Duration { ms: 60_000 },
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
            .insert_node(
                key(),
                IRNode::Start { id: "start".into() },
                None,
                Provenance::default(),
            )
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
        let t1 = par
            .insert_node(key(), task("p1"), None, Provenance::default())
            .unwrap();
        let t2 = par
            .insert_node(key(), task("p2"), None, Provenance::default())
            .unwrap();
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
        let e = par
            .insert_node(key(), end(), None, Provenance::default())
            .unwrap();
        par.insert_edge(s, f, edge("e1")).unwrap();
        par.insert_edge(f, t1, edge("e2")).unwrap();
        par.insert_edge(f, t2, edge("e3")).unwrap();
        par.insert_edge(t1, j, edge("e4")).unwrap();
        par.insert_edge(t2, j, edge("e5")).unwrap();
        par.insert_edge(j, e, edge("e6")).unwrap();
        par.admit().expect("parallel region must admit");
    }

    /// B1 wrapper: emission succeeds for a plain linear DAG, the witness
    /// is the content-derived hash of the same projection, and setting a
    /// process-level declaration refuses (never silently drops — the
    /// declaration has no DSL syntax and `to_ir()` doesn't carry it, so
    /// silent emission would be the exact trap door the plan forbids).
    #[test]
    fn emit_dsl_wrapper_plumbs_decls_and_witness() {
        let (dag, ..) = linear("emit-wrap");
        let receipt = dag.emit_dsl("wf-emit").expect("linear DAG must emit");
        assert!(receipt.emitted.source.contains("(workflow wf-emit"));
        assert_eq!(
            receipt.emitted.required_symbols,
            vec!["noop".to_owned()],
            "distinct task_types of the DAG"
        );
        let ir = dag.to_ir().unwrap();
        assert_eq!(receipt.graph_state_hash, DesignerDag::graph_state_hash(&ir));

        let (mut budgeted, ..) = linear("emit-wrap-budget");
        budgeted.default_guard_budget = Some(3);
        let err = budgeted.emit_dsl("wf-emit").unwrap_err();
        // Exact-variant assertion through the anyhow boundary (downcast,
        // not a substring check — B1 blind-review finding 6).
        match err.downcast_ref::<bpmn_lite_compiler::dsl::DslEmitError>() {
            Some(bpmn_lite_compiler::dsl::DslEmitError::ProcessDeclUnrepresentable {
                field,
            }) => assert_eq!(*field, "default_guard_budget"),
            other => panic!("expected ProcessDeclUnrepresentable, got {other:?} ({err})"),
        }
    }
}
