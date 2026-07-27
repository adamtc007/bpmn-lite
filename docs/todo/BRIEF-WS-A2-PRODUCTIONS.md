# DISPATCH BRIEF — WS-A.2 slice 4: ReplaceNode + first productions (GRIND)

Executor: Sonnet-tier. Plan: EOP-PLAN-BPMN-DESIGN-003 v0.2 §WS-A.2 (RATIFIED).
Upstream (FROZEN): designer-graph @ `5c54b0e` — all of schema.rs; ops.rs slices 1-3.

## Invariants & Absolute Boundaries (verbatim-binding, as slices 1-3)
I16/I18/F4/fail-closed-naming-BPMN-ids, regions-constructed-closed, exclusions stand (Race/CallSubprocess/RollbackGuard). NEW: **P9 — productions are deterministic COMPOSITIONS of existing `Operation`s**: a production builds a `Vec<Operation>` and applies them in sequence to ONE staged candidate; it never mutates the graph through any other surface and never mints identity (every key/id arrives in its bindings struct).

## Deliverables (ops.rs + new productions.rs, `pub mod productions;` in lib.rs)

1. `Operation::ReplaceNode { target: NodeKey, key: NodeKey, node: IRNode }` (the §12.1 gap): new node takes over ALL of target's incoming and outgoing edges (ids/conditions preserved), then target is removed. Refused if: target unknown; any guard attaches to target via `attached_to_key` (same rule as DeleteNode — replace-under-guard is delete+attach, explicit); duplicate key/BPMN id (bubbles F3). The new node's ID may equal the target's ONLY if you remove target first while preserving edges — implement however is clean, but `replace preserving same BPMN id` must WORK (receipt below) since rename-in-place is the common designer edit.
2. `productions.rs` with `pub struct ProductionBindings`-style per-production binding structs and, per §12.2, these four (the rest come later):
   - `request_and_wait(bindings) -> Vec<Operation>`: ServiceTask (send) then MessageWait (correlated receive), inserted after an anchor — compose from InsertAfter × 2.
   - `parallel_checks_and_join`: CreateParallelRegion with N ServiceTask branches.
   - `for_each_with_ceiling`: CreateMultiInstanceRegion (collection flag name, declared max, task type in bindings).
   - `reminder_then_escalate`: AttachRearmingGuard with a `TimerSpec::Cycle` trigger on an anchor task + an escalation ServiceTask appended AFTER the guard's host continuation via InsertAfter — the guard has NO sequence edges (slice-2 pattern); the escalation node sits on the normal path (forward rework, not a backward edge).
   Each returns `Vec<Operation>`; provide `pub fn apply_production(base: &DesignerDag, ops: Vec<Operation>, provenance: Provenance) -> Result<StagedCandidate>` that folds `apply` over the sequence (each step's candidate feeds the next; first refusal aborts, base untouched).
3. Every production's ops vector must round-trip serde (they are the edit-log entries, Q5) — one test serializes a production's Vec<Operation> to JSON and back and re-applies identically.

## Receipts
1. GREEN `replace_node_takes_over_edges` incl. same-BPMN-id rename-in-place; RED `replace_under_guard_refused` naming both ids.
2. GREEN per production: apply_production → full-chain `admit()` green (4 tests). reminder_then_escalate's guard must be non-interrupting + Cycle (assert projected IR).
3. RED `production_aborts_atomically`: a production whose 2nd op collides on an id refuses; base unchanged.
4. Serde round-trip receipt (point 3 above).
All prior 35 green; `cargo check --workspace` clean. HALT per Rule 7. Do NOT commit — report as before.
