# EOP-EX-BPMN-ISA-002 — Worked Example / Shared Oracle

**Status: LOCKED v1.1 — against `EOP-VS-BPMN-ISA-002.md` v0.4.** V4's golden-transition fixtures (plan item 4.5) are checked against this document byte-for-byte from here. Drafting this oracle surfaced three points v0.3 left underdetermined (cancellation order, `JOIN` survivor semantics, control-stack-delta emission for deleted fibres); Adam ratified all three as V&S v0.4 (see that document's §12 for the amendment note and full reasoning). This document is updated to match v0.4 below — nothing here contradicts the frozen spec it's locked against.

**v1.1 amendment (2026-07-22, V4.6 blind-review remediation):** building `v2_ex_oracle_scenario_1_message_wins_reproduces_last_arrival_survivor_transition` (V4.5) against the real kernel surfaced two places where §2 step 5's bullet list, as drafted, was incomplete relative to its own prose and to v0.4's own rulings — not a semantic disagreement, a drafting omission. Both are corrected in place below (not just noted in a review file), so this document stays the single source of truth a cold reader can trust without cross-checking a review artifact: (1) the mutation list omitted `Retire(G)`, even though the same paragraph's prose already says `F1` "continues... to 14 (`V2GuardEnd`, pops `G`)", which necessarily retires it, and omitted the K-1 ancestor-membership fix-up `Insert` for `F2`'s stale reference to `G` (a consequence of V4.2's `effective_control_stack`, which postdates this oracle's original drafting); (2) `fibers_delete` omitted `F1` itself — real kernel behavior deletes a fibre that reaches `End` in the same transition it runs through `GuardEnd`, so `F1`'s "sole survivor of the barrier" status (ruling B) does not mean it persists as a live fibre afterward, only that no *other* fibre inherits the join-point continuation. Both corrections are proven, not asserted, by the cited test — `cargo test -p bpmn-lite-kernel v2_ex_oracle_scenario_1` is green against the exact values now written below.

**Purpose:** the adversarial worked example named in the V&S doc — "interrupting guard over a parallel subprocess nested inside a race with a message alternative," hand-lowered with full dual-stack traces including the cancellation cascade. This is authored *before* V4's kernel-word implementation exists, precisely so V4 has an independent, hand-derived target to reproduce byte-for-byte rather than a spec V4's own author could unconsciously bend to match whatever the code happens to do.

**Interpretation note (ratified, V&S v0.4 §9.1):** "a parallel subprocess nested inside a race" is realized as a `V2Fork` (the parallel subprocess) with one branch containing the race, not a race with a fork/join as one of its arms — a race's arms are effect/message/timer *registrations* only (V-5), and durable parallel work (`FORK`, which allocates a barrier activation and creates member fibres) is not something a race arm can hold. The fork must be the outer structure; this draft's original reading (b) is the ratified one, not an open choice.

---

## 1. The program

```
0:  V2Guard { handler: 16 }
1:  V2Fork { targets: [2, 6], pairing: 1 }

     branch A — long-running task, modeled as a plain timer wait:
2:  PushI64(60000)
3:  V2WaitFor
4:  V2Join { pairing: 1 }
5:  Jump { target: 14 }

     branch B — race between a timer and an "approval" message:
6:  V2RaceOpen { arm_count: 2 }
7:  PushI64(30000)
8:  V2ArmTimer { target: 11 }
9:  V2ArmMsg { target: 12, name: 100, corr_reg: 0 }   // 100 = "ApprovalReceived"
10: V2RaceClose
11: Jump { target: 13 }                                // timer-win
12: Jump { target: 13 }                                // msg-win
13: V2Join { pairing: 1 }

     convergence:
14: V2GuardEnd
15: End                                                 // normal completion

     interrupting handler (V-4: pre-push entry state — see below):
16: ExecNative { task_type: 0, argc: 0, retc: 0 }        // "NotifyCancelled"
17: End                                                  // handler completion
```

This decodes and passes `verify_program`/`verify_v2_control_stack` under the current V3 implementation (both branches converge on address 14 with identical control-stack content `[Guard(0)]`, matching V2.7/V3's merge-equality requirement) — proven by a real checked-in test, `bpmn-lite-types/src/v2_verifier.rs`'s `ex_oracle_draft_v2_program_from_eop_ex_doc_is_admitted`, not just asserted by this document. `cargo test -p bpmn-lite-types ex_oracle_draft` is green. This test re-runs on every future ISA change, so if a future edit to `successors()`/`verify_v2_control_stack` ever breaks this specific program's admission, it fails loudly here rather than silently invalidating this oracle.

Per V3's V-4 finding, the handler at address 16 is entered with the **pre-push** control-stack state — empty (`[]`), since this `V2Guard` is the outermost scope on the fibre that opened it (F0's control stack was empty when it executed address 0).

---

## 2. Scenario 1 — happy path (message wins the race)

Narrative only, since the cascade in §3 is the scenario that actually exercises the interesting D1 machinery:

1. `F0` (root fibre) executes 0 (`V2Guard`, opens record `G`, `RecordKind::Guard{interrupting:true}`, control stack push) → 1 (`V2Fork`, allocates `Barrier` record `BAR{arity:2}`, spawns `F1` at 2 and `F2` at 6, each inheriting `[G, BAR]`; `F0` is deleted).
2. `F1` runs 2→3, parks on `V2WaitFor` (`WaitState::Timer{deadline: now+60000}`, `DurableEffect::ScheduleTimer{kind: TimerKind::Wait}` emitted).
3. `F2` runs 6→10: opens `Race` record `RACE`, arms a timer alternative (`ScheduleTimer{kind: TimerKind::Race{...}}`, due `now+30000`) and a message alternative (correlation on register 0, message name 100), parks (`WaitState::Race{...}`).
4. An external `ApprovalReceived` message (correlation matching register 0's value) arrives before the 30s timer and before `F1`'s 60s wait — resolves `RACE`: `F2`'s `Race` record retires, the still-armed timer alternative's effect is cancelled, `F2` resumes at address 12 → 13 (`V2Join`), pops `BAR`'s handle, parks (barrier not yet complete — `F1` hasn't arrived).
5. Eventually `F1`'s 60s timer fires; on the next tick `F1` resumes at 4 (`V2Join`), pops `BAR`'s handle — **last arrival** — `BAR` retires. Per V&S v0.4 §5/§12 ruling B, `F1` (the last arrival) is the sole survivor of the barrier and continues straight through 14 (`V2GuardEnd`, pops **and retires** `G`) to 15 (`End`), all in the same transition; reaching `End` deletes `F1` in that same transition — ordinary fibre-completion behavior, not a special case of ruling B/C. "Sole survivor" means no *other* fibre inherits the join-point continuation, not that `F1` persists afterward. `F2` (the non-last arrival, parked at its own `JOIN` since step 4) is deleted at the moment `BAR` retires, per ruling B.

   - `fibers_delete: [F2, F1]` — `F2` first, cancelled as `BAR` retires; `F1` second, deleted on reaching `End`.
   - `fibers_upsert: []` — no new fibre spawned.
   - `concurrency_mutations: [Insert(G'), Retire(BAR), Retire(G)]` — `Insert(G')` is a K-1 ancestor-membership fix-up: `F2` still names `G` via its effective control stack up to this point (V4.2's `effective_control_stack`), so `G`'s `members` set is updated to drop `F2` before `BAR` and then `G` itself retire.
   - `control_stack_deltas: [Pop{fiber_id: F1, handle: BAR}, Pop{fiber_id: F1, handle: G}]` — `F1`'s own pops are emitted as it executes `JOIN` then `V2GuardEnd` in this transition, even though it is itself deleted moments later on reaching `End`. `F2` emits no deltas of its own — per v0.4 §12 ruling C, its entry in `fibers_delete` plus the `concurrency_mutations` above are the complete statement of what happened to its stack.

---

## 3. Scenario 2 — the cancellation cascade (the actual point of this oracle)

**Setup:** same as steps 1–3 above — `F0` forks into `F1` (parked on a plain 60s wait) and `F2` (parked on the armed race, both alternatives still live, neither fired). Both children are fully open members of `BAR`; neither has reached its `JOIN`. This is deliberate: with neither fibre having reached `JOIN` yet, ruling B's survivor question doesn't arise here at all (there is no last arrival — both fibres are cancelled outright), which is exactly why this scenario is the richer one for exercising cancellation specifically, distinct from and complementary to §2's happy-path `JOIN`-survivor case.

**Trigger:** an external signal resolves the interrupting guard `G`'s trigger condition. (**Open item for V4**: the exact `Command` variant that addresses a guard record doesn't exist in the codebase yet — `kernel::apply`'s `Command` enum has no guard-trigger case, since no v2 word has kernel semantics until V4. This oracle constrains the *output* Transition below; V4 defines the triggering `Command` shape and must reproduce this output for it.)

### Expected `Transition`, field by field

**`fibers_delete`:** `[F1, F2]` — both parked children, cancelled.

**`fibers_upsert`:** `[F3]` — the handler fibre, spawned at address 16, control stack `[]` (V-4: pre-push — `G` never existed on the handler's own stack, per the interrupting-guard derivation in `docs/todo/EOP-PLAN-BPMN-ISA-002.md`'s V3 section).

**Cancellation order — record-nesting order, fibre-ID tiebreak within a record (V&S v0.4 §4/§12 ruling A).** The record tree at cancellation: `G` (root) → `BAR` (child, opened by the `V2Fork` executing inside `G`'s guarded region) → `RACE` (child of `BAR`, armed by `F2`, a member fibre of `BAR`). Innermost-first means retiring the deepest *record* first, not the deepest *fibre*: `RACE` retires before `BAR`, `BAR` retires before `G`. `F2`'s cancellation happens as part of retiring `RACE` (the record it directly holds); `F1`'s cancellation happens as part of retiring `BAR` (both `F1` and `F2` are `BAR`'s members — at this point `F2`'s own `RACE` scope has already been retired, so cancelling `F2` from `BAR`'s membership is uncomplicated by the time `BAR` retires). Within `BAR`'s single retirement, its two member fibres (`F1`, `F2`) would tiebreak by fibre-ID if both still needed independent action at that step; here `F2` is already gone from the `RACE` step, so only `F1` remains to cancel at the `BAR` step. This draft's original (superseded) reading used fibre control-stack *depth* (`F2`=3, `F1`=2) as the ordering primitive — for this specific scenario it produced the same `RACE`-then-`BAR`-then-`G` output, which is exactly why the error was easy to miss, but depth is not the correct general primitive (V&S v0.4 §12 explains why: two fibres at equal depth holding differently-nested records are indistinguishable by depth, but not by the record tree).

**`concurrency_mutations`** (in cancellation order):
1. `Retire(RACE)` — `F2`'s race record, retired as part of cancelling `F2`.
2. `Retire(BAR)` — once both members (`F1`, `F2`) are gone, the barrier retires. (Not `Remove`: retained per `RecordState::Retired`'s doc, "retained rather than removed where kind demands history" — this draft applies that uniformly rather than assuming `Barrier` is exempt; V4 may decide otherwise.)
3. `Retire(G)` — the guard itself retires as its trigger consumes it (interrupting fire is terminal for the guard record).

**`control_stack_deltas`:** **empty.** Per V&S v0.4 §4/§12 ruling C, deltas describe surviving fibres only; both `F1` and `F2` are deleted in this transition (§3's setup differs from §2's happy path — here cancellation fires *before* either branch reaches `JOIN`, so there is no survivor at all, unlike §2 where `F1` survives). `fibers_delete: [F1, F2]` plus the `concurrency_mutations` below are the complete statement of what happened to their stacks; this draft's original cut (three `Pop` deltas for the about-to-be-deleted fibres) is superseded.

**Effect cancellation:**
- `F1`'s `ScheduleTimer{kind: Wait}` effect (60s wait) — cancelled/superseded, no longer due.
- `F2`'s `ScheduleTimer{kind: Race{...}}` effect (30s timer arm) — cancelled.
- `F2`'s armed message alternative (name 100, correlation on register 0) — deregistered; no `DurableEffect` was pending for it specifically (message arms are a registration, not an effect per V2.7's `V2ArmMsg` design), so nothing to cancel in `effect_mutations` for it — flagged as a distinct case from the timer arm, worth a reviewer sanity-check.

**`events`:** at minimum, one event marking the guard's trigger/firing and one per cancelled member — exact `RuntimeEvent` variant names are V4's to define (none of the existing v1-era `RuntimeEvent` variants model a v2 guard firing). Not pinned further in this draft.

**Everything else** (`jobs_enqueue`, `outbox`, `incidents`, `dedupe`, etc.) — empty for this scenario; no FFI/job effects are in flight.

---

## 4. Open items for V4

Three of the original five were D1-level semantic gaps and are now closed by V&S v0.4 (§12): which fibre survives a `JOIN`'s last arrival (ruling B), whether cancellation order is fibre-depth or record-nesting (ruling A), and whether `control_stack_deltas` are emitted for deleted fibres (ruling C). Two remain open — both are V4 *implementation* choices, not D1 semantics, so they don't require a V&S amendment, just a decision when V4 is built:

1. Exact `Command` shape for triggering an armed guard.
2. Exact `RuntimeEvent` variants for guard-trigger and member-cancellation events.

Neither blocks locking this oracle — the `Transition` output this document pins doesn't depend on the *name* of the triggering `Command` or the exact `RuntimeEvent` variant tags, only on the `fibers_delete`/`fibers_upsert`/`concurrency_mutations`/`control_stack_deltas`/effect-cancellation content, which v0.4 now fully determines. V4's author resolves these two when defining `Command`/`RuntimeEvent`.

---

## 5. V5 addendum — §18's new theorems are deliberately NOT folded in (2026-07-23)

`EOP-VS-BPMN-ISA-002.md` §18 item 4 flagged V-10 (dominance)/V-11 (terminal reachability)/V-3-as-weakened (dynamic-arity bound) as "hand-assembled fixtures exist ahead of ratification, not yet folded into the locked oracle... if warranted, is separate follow-on work not scoped here." All three now have complete fixture corpora (positive-admitted and negative-rejected cases) in `bpmn-lite-types/src/v2_verifier.rs` — grep `V-10`/`V-11`/the dynamic-arity `V-3` tests there for the actual proofs.

**Deliberate decision: not warranted, and this document says why rather than leaving the question open.** This oracle's own stated **Purpose** (top of this document) is a single adversarial worked example proving *runtime `Transition` shape* for the D1 cancellation-cascade mechanism (guard/fork/race nesting) — a hand-derived trace of what the kernel must produce for one fixed program under one fixed command sequence. V-10/V-11/V-3 are a structurally different kind of claim: static, compile-time verifier *admission* theorems (does this program pass or fail `verify_v2_control_stack`), with no `Transition` to trace at all — there is no cancellation cascade, no fibre delta, no concurrency mutation to hand-derive for "this program is/isn't admitted." Forcing them into this document's format would mean inventing worked examples that don't exercise what the oracle exists to exercise, duplicating coverage `v2_verifier.rs`'s own positive/negative fixture pairs already provide more directly, in the idiom (`assert!`/`expect_err` against the real verifier) that actually tests the claim being made. The right home for a static-admission theorem's proof is a verifier unit test, not a runtime-trace oracle — this document stays locked at v1.1, scoped to what it was built to prove.
