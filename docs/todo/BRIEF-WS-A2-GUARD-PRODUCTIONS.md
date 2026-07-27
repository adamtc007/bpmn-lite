# DISPATCH BRIEF — WS-A.2 slice 5: interrupting_timeout + non_interrupting_notification (GRIND)

Executor: Sonnet-tier. Upstream (FROZEN): designer-graph @ `fd17bd8`.
Same Invariants & Absolute Boundaries as slices 1-4, verbatim. NEW RULE from slice-4's remediation, binding: **a production ALONE must admit — it owns its complete shape incl. any guard escape flow; receipts must not add completion ops.**

## Deliverables (productions.rs only)

1. `interrupting_timeout(bindings) -> Vec<Operation>` (§12.2 INTERRUPTING_TIMEOUT): AttachGuard (Timer **Duration** trigger — Cycle on interrupting is refused by ops and must not be constructible through these bindings: type the field as `duration_ms: u64`, build the TimerSpec inside) + AppendNode(guard → timeout-continuation node) + AppendNode(continuation → own End). Bindings mirror ReminderThenEscalateBindings' shape.
2. `non_interrupting_notification(bindings)` (§12.2): AttachRearmingGuard (Cycle trigger: `interval_ms: u64, max_fires: u32` fields, TimerSpec built inside) + AppendNode(guard → notification node) + AppendNode(notification → own End).
3. EXCLUDED, do not add: timer_message_race (Race excluded), call_durable_subprocess (excluded), human_review_with_rework (pending an XOR-default-edge trace — CAREFUL, not yours).

## Receipts
1. GREEN each production: apply_production → admit() ALONE; projected IR asserted (interrupting+Duration for timeout; non-interrupting+Cycle for notification).
2. RED: attempt to express a cycle timeout through interrupting_timeout is UNREPRESENTABLE (no test possible = the receipt is the bindings type — state this in a doc comment) — instead add RED `production_ops_reject_via_ops_gates`: hand-build the illegal op vector (AttachGuard with Cycle) and show apply_production refuses via the slice-2 gate.
3. Serde round-trip for one of the new productions.
All prior 45 green; workspace check clean. HALT per Rule 7. No commits — report as before.
