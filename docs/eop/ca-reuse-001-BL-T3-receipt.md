# BL-T3 receipt — named, data-supplied deadlines (EOP-PLAN-CA-REUSE-001)

**Delivered:** the frontend and start-seeding halves, complete and green, kernel untouched.
**Blocked (surfaced, not worked around):** the full "N named timers fire N named effects in
declared order" acceptance — by a **pre-existing kernel defect** (follow-up **F6**), evidenced
below and cemented in two baseline tests.

## What changed (kernel diff: EMPTY — `git diff bpmn-lite-kernel/` is empty)

- **IR:** `TimerSpec::FromStartValue { name }` (`bpmn-lite-compiler/src/ir.rs`).
- **DSL grammar:** timer shape `:deadline-from <name>` on `boundary-timer` and `timer-wait`
  (`dsl/parser.rs::parse_timer_shape`), exclusivity-checked against the constant shapes;
  printer round-trips it (`dsl/refactor.rs`, both `ToSexpr` impls). The timer's **name** is
  its `:id`; the **named effect** is the authored handler task's verb
  (`clock.<timer-name>-passed` by convention in the tests).
- **Lowering (both frontends, same shape):** where constant specs emit `PushI64(const)`,
  `FromStartValue` emits `LoadFlag { key }` via the existing flag interner — the existing
  dynamic-timer ISA form; no new opcode, no verifier change (`LoadFlag` push-effect equals
  `PushI64`, and the verifier already admits it as an arm-operand producer). Sites:
  `lowering.rs` XML wait arm + both guard arms (`lower_boundary_guarded_task_v2` gained a
  `flag_intern` param), `dsl/frontend.rs` guard + wait arms.
  `timer_spec_duration_ms` carries a messaged-unreachable arm (house pattern) — every
  emission site pre-matches `FromStartValue`.
- **Start path:** `StartCommand`/builder gained optional `deadlines: BTreeMap<String, i64>`
  (absolute epoch-ms; serde `default`, additive). `start_command` seeding: each name must
  resolve in the artifact's own `flag_symbol_table` — unknown name is a **typed start-time
  rejection naming the flag** — and the value is converted to remaining-duration ms as of
  the command's logical time (clamped ≥ 0: a passed deadline fires immediately). F-DSGN-1
  is otherwise intact: this is the one sanctioned, artifact-validated start-flag channel;
  the gRPC `orch_flags` rejection is untouched.
- **Payload rule documented at the type** (`StartCommand.deadlines` field doc):
  identifiers and timestamps only — never amounts. `decimal`→`f64` aliasing NOT touched
  (already logged as F3, `compiler/src/parser.rs:1296`).
- Downstream totality: authoring DTO **fails closed** on the new shape (typed error, no
  silent all-None timer — `bpmn-lite-authoring/src/ir_to_dto.rs`); runner/designer label
  fns gained a label arm.

## Green receipts

- `dsl::refactor::tests::deadline_from_shape_roundtrips_and_compiles` — print↔reparse
  fixpoint + compile, both guard and wait shapes.
- `t_bl3_named_deadline_fires_named_effect_host_unaffected` — named timer armed from the
  start map; a sweep past *all* declared deadlines fires **exactly one** named effect (the
  first declared; later timers not yet armed), carrying the instance id; the
  non-interrupting host stays parked on its own job.
- `t_bl3_interrupting_named_deadline_cancels_host_scope` — interrupting named deadline
  fires the handler's named effect and cancels the host scope cleanly.
- `t_bl3_unknown_deadline_name_is_rejected_at_start` — typed rejection.
- Full workspace suite green (see commit message receipt line).

## The blocker (F6) — why N-in-declared-order cannot pass yet

Firing timer N+1 requires its host to start, which requires host N to complete **after**
its own timer fired. That sequence is impossible today:

1. **Live-handler variant:** host completes while the fired handler is parked → the host's
   `V2GuardNEnd` retires the guard record while the handler's inherited control-stack
   handle still references it → Ring 3 K-2 rejection.
   Cemented: `t_bl3_baseline_k2_host_completion_after_ni_fire_with_live_handler`.
2. **Handler-ended variant:** same K-2, reproduced with **constant** `:duration-ms` — so
   the defect pre-exists BL-T3 entirely.
   Cemented: `t_bl3_baseline_k2_host_completion_after_ni_fire_and_handler_end`.
3. Topology escapes are closed: parallel hosts need handlers that escape the barrier scope
   (verifier V-1 reject — correctly); terminate-ends kill the whole instance; R32 forbids
   N timers on one host.

Existing NI tests never complete a host after a fire, which is why this was undiscovered.
The rejection is fail-closed (integrity, not corruption). Fix is a kernel design fork —
options (a)/(b) in the F6 entry — needing a ruling; both baseline tests flip on the fix,
and the declared-order acceptance test should be added then.
