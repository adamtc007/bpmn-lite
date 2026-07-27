# DISPATCH BRIEF — WS-A.2 slice 1: operation core + linear operations (GRIND)

Executor: Sonnet-tier. Plan: EOP-PLAN-BPMN-DESIGN-003 v0.2 §WS-A.2.
Upstream interface (FROZEN): `designer-graph/src/schema.rs` + `board_candidate.rs` @ `833af47`.

## Invariants & Absolute Boundaries (reproduce verbatim in code docs; violating any is a rejected review)

1. **I16**: structural derivation (pairing, regions, merge identity) is consumed from `bpmn_lite_compiler::{compute_post_dominators, compute_region_map, gateway_pairs}` — NEVER computed in this crate. This slice needs none of them; do not import them.
2. **I23 + review-F8**: every edge-introducing operation pre-gates forward-only flow with `petgraph::algo::has_path_connecting(&graph, to, from, None)` — if a path exists from `to` back to `from`, adding `from → to` would close a cycle: REFUSE with an error naming both node ids. The compiler's cyclicity gate remains the backstop, never the mechanism.
3. **I18**: operations apply to a CLONE (the staged candidate), never to the authoritative DAG. Ratification is the caller's separate act.
4. **Review-F4**: `NodeKey`s for created nodes are carried IN the operation record (caller/log-supplied), never minted inside `apply`. No `Uuid::new_v4()` anywhere in ops.rs.
5. **Fail closed**: every refusal is an `Err` naming the offending node/edge id. No skips, no defaults, no `#[allow]`.
6. Do NOT alter schema.rs or board_candidate.rs except: schema.rs mutator visibility stays `pub(crate)` (ops.rs is in-crate and may call them).

## Deliverable: `designer-graph/src/ops.rs` (+ `pub mod ops;` in lib.rs)

Implement EXACTLY this surface (fill in bodies; keep signatures):

```rust
use crate::schema::{DesignerDag, DesignerEdge, NodeKey, Provenance};
use anyhow::Result;
use bpmn_lite_compiler::ir::{ConditionExpr, IRNode};
use serde::{Deserialize, Serialize};

/// One deterministic edit. Carries every created identity (F4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    /// Insert `node` (with `key`) after `anchor`: re-points every
    /// anchor→X edge (X = anchor's current successors) to node→X,
    /// then adds anchor→node with `edge_id`.
    InsertAfter { anchor: NodeKey, key: NodeKey, node: IRNode, edge_id: String },
    /// Mirror of InsertAfter on the predecessor side.
    InsertBefore { anchor: NodeKey, key: NodeKey, node: IRNode, edge_id: String },
    /// Append with NO re-pointing: just node + anchor→node edge.
    /// Refused if anchor already has an outgoing edge (that is
    /// InsertAfter's job) — keeps Append unambiguous.
    AppendNode { anchor: NodeKey, key: NodeKey, node: IRNode, edge_id: String },
    /// Connect two existing nodes (forward-only pre-gate, I23).
    Connect { from: NodeKey, to: NodeKey, edge_id: String, condition: Option<ConditionExpr> },
    /// Delete a node and ALL its incident edges. Refused if any
    /// OTHER node's `attached_to_key` references it (a guard would
    /// dangle) — error names both ids.
    DeleteNode { target: NodeKey },
}

/// Applying an operation to a staged candidate.
pub struct StagedCandidate {
    pub candidate: DesignerDag,
    pub applied: Operation,
    pub provenance: Provenance,
}

/// THE entry point: clone `base`, apply `op` deterministically,
/// return the candidate. Does NOT run admission — the caller stages
/// then calls `candidate.admit()` (the full production chain) before
/// ratification. Refusals per the boundaries above.
pub fn apply(base: &DesignerDag, op: Operation, provenance: Provenance) -> Result<StagedCandidate>;
```

Implementation notes:
- You will need `DesignerDag` accessors for successors/predecessors/edge re-pointing. Add MINIMAL `pub(crate)` helpers to schema.rs if required (e.g. `successors(&self, key) -> Vec<NodeKey>`, `remove_edge_between`, `remove_node`) — keep them `pub(crate)`, document each as "ops.rs support, not a public mutation surface".
- `DeleteNode` must also clean `bpmn_ids`/`edge_ids` sets so ids are reusable after deletion.
- InsertAfter/InsertBefore edge re-pointing must preserve the re-pointed edges' ids and conditions.

## Receipts (all mandatory; test module in ops.rs)

1. GREEN `insert_after_repoints_and_admits`: start→t1→end; InsertAfter(anchor=t1, t2) → start→t1→t2→end; `candidate.admit()` green; base DAG UNCHANGED (I18 — assert base.node_count() unchanged).
2. GREEN `insert_before_mirror`: same shape via InsertBefore(anchor=t1).
3. RED `connect_refuses_backward_edge`: start→a→b→end; Connect(b→a... i.e. from=b? no: from=b,to=a) refused; error names both ids; base unchanged.
4. RED `append_refuses_on_occupied_anchor`: anchor with existing outgoing edge → AppendNode refused.
5. RED `delete_refuses_dangling_guard`: task + boundary guard attached via `attached_to_key` → DeleteNode(task) refused naming both ids; GREEN: delete the guard first, then the task deletes clean.
6. GREEN `deleted_ids_are_reusable`: delete t2 then insert a new node with BPMN id "t2" succeeds.
7. RED `duplicate_key_or_id_refused_through_ops`: applying an op whose `key`/`node.id()`/`edge_id` collides is refused (bubbles schema's F3 checks).
8. Determinism: applying the same Operation to clones of the same base twice yields candidates with identical `to_ir()` node/edge id sets.

## HALT conditions (Rule 7 — report, do not adapt)

- If schema.rs lacks a capability you cannot add as a `pub(crate)` helper without changing EXISTING signatures/semantics → HALT and report the exact gap.
- If any receipt cannot go green without weakening a boundary above → HALT.

Run `cargo test -p designer-graph` (all green, including existing 9) and `cargo check --workspace`. Do not commit — report with a diff summary; the orchestrator commits after review.
