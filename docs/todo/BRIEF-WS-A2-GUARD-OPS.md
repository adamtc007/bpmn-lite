# DISPATCH BRIEF — WS-A.2 slice 2: guard + declaration operations (GRIND)

Executor: Sonnet-tier. Plan: EOP-PLAN-BPMN-DESIGN-003 v0.2 §WS-A.2 (RATIFIED).
Upstream (FROZEN): designer-graph @ `23475eb` — schema.rs, board_candidate.rs, ops.rs slice 1.

## Invariants & Absolute Boundaries (verbatim-binding, same as slice 1)

1. I16: never import/compute structural derivation here.
2. I18: clone-and-stage only; `apply` never touches the base.
3. F4: all created `NodeKey`s arrive in the `Operation` record; no `Uuid::new_v4()` outside test helpers.
4. Fail closed; every refusal names the offending node/guard id — resolve BPMN ids in messages (also FIX slice 1's `Connect` refusal to name BPMN ids alongside the keys; keep the key in the message).
5. **EXCLUDED BY DESIGN — do not implement:** `AttachRollbackGuard` (GUARD-R has no `IRNode` kind; pending substrate trace). If you find yourself wanting it, that is a HALT, not an improvisation.
6. V&S §4.5 pre-gates (verifier stays the backstop): a `TimerSpec::Cycle` trigger is legal ONLY on a non-interrupting guard — refuse cycle-on-interrupting at `apply` naming the guard id.

## Deliverable: extend `Operation` in ops.rs with EXACTLY these variants

```rust
/// Attach an INTERRUPTING boundary guard to `host`.
AttachGuard { host: NodeKey, key: NodeKey, guard_id: String, trigger: GuardTrigger },
/// Attach a NON-INTERRUPTING (re-arming) boundary guard to `host`.
AttachRearmingGuard { host: NodeKey, key: NodeKey, guard_id: String, trigger: GuardTrigger },
/// Replace the arming trigger on an existing boundary guard.
SetGuardTrigger { guard: NodeKey, trigger: GuardTrigger },
/// Set/override a guard's failure budget (None clears to inherit the
/// workflow default).
SetGuardBudget { guard: NodeKey, failure_budget: Option<u32> },
/// Set the correlation-source expression on a waiting node
/// (MessageWait / HumanWait / SendTask — anything else is refused).
SetCorrelationSource { node: NodeKey, corr_key_source: String },
```

with:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum GuardTrigger {
    Timer(bpmn_lite_compiler::ir::TimerSpec),
    Error { error_code: Option<String> },
}
```

Semantics:
- Attach*: insert a `BoundaryTimer` (Timer trigger; `interrupting` = true for AttachGuard, false for AttachRearming) or `BoundaryError` (Error trigger) node with `attached_to_key = Some(host)`, `attached_to` string = host's current BPMN id, `failure_budget: None`, NO sequence edges. Host must exist; guard_id collisions bubble schema F3.
- **BoundaryError is interrupting-only in this substrate** (F2a: non-interrupting error boundaries are parse-rejected): `AttachRearmingGuard` with an `Error` trigger is REFUSED at apply, naming the guard id.
- Cycle triggers: `AttachGuard`/`SetGuardTrigger` must refuse `TimerSpec::Cycle` when the guard is/would be interrupting (boundary rule 6).
- SetGuardTrigger on a `BoundaryError` node may only set another Error trigger; switching Timer↔Error guard KIND is refused (that is delete + attach, an explicit two-op edit).
- SetGuardBudget refused on non-`Boundary*` nodes (I24: budget on a non-guard is unrepresentable — keep it that way), naming the node id.
- SetCorrelationSource mutates the field in place (`MessageWait.corr_key_source` / `HumanWait.corr_key_source` / `SendTask.corr_key_source`); refused elsewhere.

## Receipts (mandatory, ops.rs test module)

1. GREEN attach interrupting timer guard → `admit()` green; guard projected with host's id.
2. GREEN attach rearming (non-interrupting) CYCLE guard → admit green (cycle on non-interrupting is the legal combination).
3. RED cycle-on-interrupting refused at apply (AttachGuard with Cycle), naming guard id — AND a companion note-in-test verifying the verifier backstop also rejects it if constructed directly via schema (bypass parity: build the same shape with schema mutators, assert `admit()` refuses).
4. RED rearming Error guard refused (F2a mirror), naming guard id.
5. GREEN SetGuardBudget(Some(9)) on a guard → projected IR carries 9; envelope `v2_guard_budgets` reflects it after admit (inspect via the admitted workflow if surface exists — else assert on `to_ir()` field and note the envelope check as done by admission itself).
6. RED SetGuardBudget on a ServiceTask refused naming the id.
7. GREEN SetCorrelationSource on MessageWait → field updated; RED on ServiceTask.
8. RED kind-switch refused (SetGuardTrigger Timer→Error on a BoundaryError).
9. Slice-1 polish receipt: Connect refusal message now contains both BPMN ids.
All prior 17 tests stay green; `cargo check --workspace` clean.

## HALT conditions
Substrate lacking (e.g. a needed IR field absent, cycle legality already enforced somewhere that contradicts rule 6) → HALT with the exact gap. Do NOT commit — report files changed, helpers added (if any, `pub(crate)`, justified), verbatim test results, deviations.
