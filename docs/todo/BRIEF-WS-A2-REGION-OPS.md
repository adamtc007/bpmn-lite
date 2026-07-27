# DISPATCH BRIEF — WS-A.2 slice 3: region operations (GRIND)

Executor: Sonnet-tier. Plan: EOP-PLAN-BPMN-DESIGN-003 v0.2 §WS-A.2 (RATIFIED).
Upstream (FROZEN): designer-graph @ `d4e2406` — schema.rs, ops.rs slices 1-2, board_candidate.rs.

## Invariants & Absolute Boundaries (verbatim-binding, as slices 1-2)

1. I16 (no structural derivation here); I18 (clone-and-stage); F4 (ALL created NodeKeys/ids arrive in the Operation record; no minting in apply); fail-closed refusals naming BPMN ids.
2. **Regions are created CLOSED (SESE by construction, P1/I23):** one operation inserts the complete fork…join block. There is NO open-region state and NO separate CloseParallelRegion operation — `CloseParallelRegion` from §12.1 is subsumed (record this in the module docs verbatim: "regions are constructed closed; the §12.1 Close operation is an artifact of editors with open-region states, which this schema makes unrepresentable").
3. **EXCLUDED BY DESIGN — HALT if you find yourself wanting them:** `CreateRace` (no race/event-gateway IRNode exists — substrate trace pending) and `CallSubprocess` (no call-activity IRNode — trace pending). Do not fake either with ServiceTask.
4. **Topology rule from slice 2's finding:** never wire a region's internal nodes directly into a shared downstream End alongside other paths — merges go through the join. Your green fixtures must respect this (the verifier enforces it: "inconsistent stack height at CFG merge").

## Deliverable: extend `Operation` in ops.rs with EXACTLY these variants

```rust
/// One branch of a to-be-inserted region: the interior node (with its
/// caller-supplied key) and the two flow ids wiring it fork→node→join.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegionBranch {
    pub key: NodeKey,
    pub node: IRNode,
    pub in_edge_id: String,
    pub out_edge_id: String,
    /// Condition on the fork→node edge (inclusive regions; None for
    /// parallel branches — a Some here on a parallel region is REFUSED).
    pub condition: Option<ConditionExpr>,
}

/// Insert a complete parallel fork…join region after `anchor`
/// (InsertAfter semantics: anchor's former outgoing edges re-point,
/// ids preserved, to leave from `join`).
CreateParallelRegion {
    anchor: NodeKey,
    fork_key: NodeKey, fork_node_id: String,
    join_key: NodeKey, join_node_id: String,
    entry_edge_id: String,           // anchor -> fork
    branches: Vec<RegionBranch>,     // >= 2 refused otherwise
},
/// Same shape, GatewayInclusive pair; every branch MUST carry a
/// condition (None on any branch is REFUSED — inclusive without a
/// condition is a parallel branch pretending).
CreateInclusiveRegion { /* same fields */ },
/// Insert a MultiInstance activity after `anchor` (single-node
/// region: the MI node IS the region, ruling K). declared_max is in
/// the IRNode (u32, mandatory by construction — I24).
CreateMultiInstanceRegion { anchor: NodeKey, key: NodeKey, node: IRNode /* must be IRNode::MultiInstance, else REFUSED */, edge_id: String },
/// Add an outgoing conditional branch to an existing XOR gateway:
/// gateway must be IRNode::GatewayXor (else refused); target must
/// exist; forward-only pre-gate applies (slice-1 Connect rule).
CreateBranch { gateway: NodeKey, target: NodeKey, edge_id: String, condition: Option<ConditionExpr> },
```

Construction semantics for the two region ops (shared helper encouraged):
1. Validate branch count ≥ 2; validate condition presence per kind; validate all ids/keys fresh (bubbles F3).
2. Detach anchor's outgoing edges (preserve ids/conditions — slice-1 helper).
3. Insert fork (Diverging), each branch node, join (Converging); wire anchor→fork (entry_edge_id), fork→branch (in_edge_id, condition per kind), branch→join (out_edge_id).
4. Re-attach the preserved edges FROM join.
5. Everything or nothing: any refusal leaves the candidate unusable, base untouched (I18 gives this for free — do not partially mutate the shared base, which is impossible anyway; just return Err).

## Receipts (mandatory)

1. GREEN `parallel_region_inserts_closed_and_admits`: start→t1→end; CreateParallelRegion after t1 with 2 ServiceTask branches → full-chain `admit()` green; base unchanged; the re-pointed t1→end edge id survives on join→end.
2. GREEN `inclusive_region_with_conditions_admits`: 2 conditioned branches → admit green.
3. RED `inclusive_branch_without_condition_refused` + RED `parallel_branch_with_condition_refused` (both naming the branch node id).
4. RED `region_with_one_branch_refused`.
5. GREEN `multi_instance_region_admits` (declared_max in the node; admit green) + RED non-MultiInstance node to CreateMultiInstanceRegion refused.
6. GREEN `xor_branch_with_condition_admits`: build XOR split via ops (InsertAfter a GatewayXor, CreateBranch twice to two new targets... construct however is cleanest through EXISTING ops + CreateBranch; assert admit) — note XOR zero-match is legal (incident edge is the compiler's job, not yours).
7. RED `create_branch_backward_refused` (forward-only pre-gate, names both BPMN ids).
8. Determinism: same region op on clones ⇒ identical `to_ir()` node/edge id sets.
All prior 26 designer-graph tests stay green; `cargo check --workspace` clean.

## HALT conditions
As slices 1-2 (Rule 7). If the verifier refuses a shape this brief calls GREEN, do not adapt the op semantics — HALT with the verifier diagnostic verbatim. Do NOT commit — report files, helpers (pub(crate), justified), verbatim test results, deviations.
