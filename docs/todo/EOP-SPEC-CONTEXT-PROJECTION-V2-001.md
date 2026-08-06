# EOP-SPEC-CONTEXT-PROJECTION-V2-001 — the context side of the candidate-conditioned contract

**Status: APPROVED as spec'd (Adam, 2026-08-06) — F1–F5, N=3 (budget-degradable to 1), §5 staging confirmed. Implementation lands inside retrain step 2, never as a silent serving change.**
**Closes:** the left-hand side of the review's target contract `[utterance + state + dialogue] × [candidate slice + transition + neighbourhood]`. The right-hand side (v3 candidate serializer) is implemented and awaiting weights (CO-02 receipt 2026-08-06).

## 1. What v1 carries (and why it starves the model)

`ContextProjection` v1 (`utterance-engine/src/context.rs`, schema_version 1): pack identity string, graph identity string, anchor node + immediate predecessors/successors/attached guards, global node-kind counts. One serializer for train and serve (DIR-002 A1), hash-derived, injective line grammar, control-character rejects. Structurally sound — semantically thin: no region topology, no dialogue, no authority binding, no state beyond the 1-hop neighbourhood.

## 2. v2 additions

Each field keeps the v1 discipline: constructed-sorted, control-char-rejected, single canonical serializer, version line first.

| # | Field | Content | Source of truth | Why the model needs it |
|---|---|---|---|---|
| F1 | `region_context` | The anchor's innermost enclosing SESE region: region kind (`parallel` / `inclusive` / `multi_instance` / `loop_bounded` / `none`), entry/exit node ids, nesting depth | designer-graph RPST | "close the region", "add another branch here" are unresolvable without knowing what region you're inside — the largest observed context miss class (parallel_branch_interior, mi_node) |
| F2 | `open_guards` | Guards in scope at the anchor beyond directly-attached ones (workflow-default budget, enclosing-region guards), sorted | designer graph | Distinguishes `set_guard_trigger`/`set_guard_budget` targets from "attach a new guard" |
| F3 | `recent_accepted` | The last N=3 accepted/ratified actions this session, as `(candidate_id, anchor_id)` pairs, most recent first | session proposal audit | Dialogue continuity — "now do the same after the other branch" — without carrying raw prior utterances (Q9 §2-safe: candidate ids only, no free text) |
| F4 | `staged_pending` | Whether an unratified staged proposal exists (`true`/`false`) | designer state | "actually change that" reads differently mid-proposal |
| F5 | `pack_authority` | The admitted pack's `artifact_sha256` alongside the existing pack identity string | `bpmn-semantic-pack.lock` (same pin as the WS-2.A SemOS declaration) | Binds every context — train and serve — to the exact admitted pack version; a corpus generated under one pack can never silently serve another |

**Deliberately deferred to schema v3 (not reserved, versioned):** subject/entity lifecycle state, taxonomy position, allowed-transition sets — these are the *domain resolver's* context (WS-3/4, fed by the WS-2.D metadata) and enter when that resolver exists. Designer authoring context does not fake them.

## 3. Budget discipline

The pair budget is 256 tokens and the candidate side grows under v3. Before adoption: a measured token-budget receipt over the full board × the longest real utterances, using the existing pair-survival-under-tokenization assertion (R6) as the gate. If v2 + v3 cannot coexist within budget at N=3 `recent_accepted`, N drops to 2 then 1 before any field is cut, and the receipt records the choice.

## 4. Identity consequences

`CONTEXT_PROJECTION_SCHEMA_VERSION` → 2 (version is line 1 of the preimage — v1/v2 can never hash-collide). `context_projection_hash` in every `DecisionRecord` distinguishes the forms by construction (review N3). Old records stay readable; the funnel joins across versions transparently.

## 5. Staged retrain protocol (single-variable, extends the FK-E ruling)

1. **Wording-only retrain** — adjudicated gateway text, committed 178-family split, v2 serving contract *unchanged*. Measures the wording.
2. **Contract retrain** — v3 candidate serializer + ContextProjection v2 together (one contract jump, jointly admitted), same split. Measures the contract.
3. **Re-split** — the new family split as its own scored step.

Each step scored through the funnel (adjudicated turns) + starter-seed-v1; each produces its own bundle card citing corpus release, charter version, projection schema version, and serializer hashes.

## 6. Review asks (Adam)

- Approve/edit the field set F1–F5 (additions? cuts?).
- N for `recent_accepted` (proposed 3, budget-degradable to 1).
- Confirm the §5 staging (wording → contract → re-split) as the execution order.
