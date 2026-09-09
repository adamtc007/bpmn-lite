# BL-T5 receipt — **kernel**: guard records are host-owned (F6, ruled option b)

**Ruling applied:** handler fibres do not inherit the guard-record handle at spawn.
Host↔handler decoupled; no reference counting, no deferral, no special case in retirement.

## Reproduction (pre-change)

After any non-interrupting guard fire, the host could never complete its guarded task:
the handler fibre inherited the guard's own token (`apply_v2_trigger_guard`'s V-4
POST-push prefix, `pos + 1`), so the host's `V2GuardNEnd` retirement left a stale handle
on the handler's control stack — Ring 3 K-2 rejection with the handler alive (parked) or
already ended. Reproduced with constant `:duration-ms` (pre-existing, unrelated to
`:deadline-from`); both variants were baseline-cemented in BL-T3.

## What changed

- **Kernel** (`bpmn-lite-kernel/src/lib.rs::apply_v2_trigger_guard`): handler stack is
  the PRE-push prefix (`..pos`) for BOTH guard kinds — ancestors above the guard only.
  Membership reconciliation follows automatically (handler never registered on the guard
  record). The guard's identity reaches the handler as **data** — the
  `V2GuardNTriggered`/`V2GuardTriggered` event carries `record_id` — never as a handle.
  (An explicit in-frame value was NOT added: `Fiber` has no aux slot, pushing an operand
  would break handler-entry stack balance, and nothing consumes it today; noted here
  rather than silently built.)
- **`V2GuardNEnd`**: unchanged — retires regardless of handler state, exactly as before;
  the decoupling removes the hazard rather than managing it.
- **Verifier** (`bpmn-lite-types/src/v2_verifier.rs`): `V2GuardN`'s handler edge now
  propagates the PRE-push state (same derivation as `V2Guard`); the original POST-push
  divergence had been flagged "for confirmation, not silently landed as settled" — F6 is
  that confirmation, decided the other way. Fixture flipped to
  `v4_guardn_handler_entry_state_is_pre_push_no_own_token` + new adversarial
  `v4_guardn_handler_popping_own_token_is_rejected`.
- **Both frontends**: the compensating escape-terminal `V2GuardNEnd` injection walks
  (`lowering.rs` `guardn_close_before_end`, `dsl/frontend.rs` `guardn_close_ends`) are
  **removed entirely** (mechanism deleted, not neutered — no empty-set threading).
- **Fixtures updated to the new shape** (each was asserting the old rule, not a
  regression): kernel `v2_guardn_trigger_spawns_handler_without_unwinding_and_rearms`
  (now asserts empty handler stack + zero membership mutations),
  `v2_endterminate_retires_…`, `definitive_job_failure_under_…`, K-invariant property
  topologies 1/2; compiler `rearming_cycle_guard_scaffold_matches_xml_path` (one
  `V2GuardNEnd`, host body only, both paths); engine `setup_ni_cycle` bytecode.
- **ISA doc** (`docs/todo/EOP-VS-BPMN-ISA-002.md` §K-2): amended in place — guard
  records are host-owned; handler fibres never hold them; V-4 PRE-push for both kinds.

## Theorem re-run receipts

- **K-theorems:** kernel suite 49/0, including both K-invariant property suites
  (`k_invariants_hold_across_every_topology_deterministically`,
  `…_after_random_tick_and_trigger_sequences` — random command sequences over 5
  topologies, K-1..K-3 checked after every `apply`). `bpmn-lite-kernel/fuzz` compiles.
- **V-theorems:** bpmn-lite-types suite 120+/0 (full verifier corpus incl. the flipped
  V-4 fixtures and the new adversarial reject).
- **Genuine K-2 rejection preserved:** `ring3_rejects_a_dangling_control_stack_handle`
  and `ring3_rejects_a_handle_whose_record_was_dropped` still pass — foreign-handle use
  still fails closed.
- **Re-arm unaffected:** `t_ni_2_cycle_fires_multiple_times` (three fires through the
  changed spawn path), `t_ni_3_cycle_exhausted_reverts_to_job`, kernel
  `…_without_unwinding_and_rearms` (double trigger) — all green.
- **Flipped baselines → standing tests:**
  `t_bl5_host_completes_after_ni_fire_with_live_handler` and
  `t_bl5_host_completes_after_ni_fire_and_handler_end` — the host completes its guarded
  task after a fire, both handler states, and advances to the next step.
- **BL-T3 full acceptance now green:**
  `t_bl3_named_deadlines_fire_named_effects_in_declared_order` — 3 named timers, 3
  start-supplied deadlines, 3 named effects in declared order, host continuing after
  each fire (handler alive at each continuation), instance runs to `Completed`.
- **Full workspace suite:** green, zero failures.

## Out of scope, logged

`bpmn-lite-engine/fuzz` doesn't compile — pre-existing dev-capture trait drift, verified
by stash (F7 in the follow-ups).
