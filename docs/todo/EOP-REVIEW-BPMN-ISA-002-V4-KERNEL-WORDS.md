# EOP-REVIEW-BPMN-ISA-002 — V4 Kernel Words: K-Theorems, Ring 3, Golden Transitions

**Status:** one BLOCKING finding confirmed and fixed with a red→green receipt (below); everything else independently reviewed and closed clean. V4 is ready to close pending Adam's sign-off on the fix and the two documented open items (V4.4 skip, oracle enumeration gaps).
**Reviewer:** independent `general-purpose` agent (not the author of the V4 diff), dispatched fresh — told to read the spec/oracle directly rather than trust doc comments, not to run the build/tests, not to fix anything, and to assume at least one undiscovered bug exists. Its top finding was then independently re-derived by the author from the code and the exact spec text (not taken on faith) before being fixed.
**Scope:** `bpmn-lite-kernel/src/lib.rs` (`apply_tick`'s V2AwaitEffect/V2ArmEffect handling, `apply_ffi_completion`, `check_k_invariants`, `effective_control_stack`, `ring3_shadow_check`/`derive_post_transition_frame`, `apply_job_failure`'s rollback-on-fail branch, `v2_reconcile_ancestor_membership`) and `bpmn-lite-types/src/integrity_rings.rs` (`IntegrityError::Ring3Runtime`). Checklist = V&S §6 (integrity rings), §7 (K-1/K-2/K-3), §13 amendment v0.5 (rulings A/B/C), and the locked oracle `docs/todo/EOP-EX-BPMN-ISA-002.md`.

---

## 1. What changed (V4.1–V4.5)

| Tranche | File(s) | What |
|---|---|---|
| V4.1 | `bpmn-lite-types/src/{types,artifact,canonical}.rs`, `bpmn-lite-kernel/src/lib.rs` | Full D2 word set given real `apply()` semantics: `V2Guard`/`V2GuardN`/`V2GuardEnd`/`V2GuardNEnd`/`V2Fork`/`V2Join`/`V2RaceOpen`/`V2RaceArm`/`V2RaceClose`/`V2CancelScope`/`V2ArmTimer`/`V2WaitFor`/`V2WaitUntil`/`V2AwaitEffect`/`V2ArmEffect`. New `v2_ffi_task_decls` side table (kept separate from v1's `ffi_task_decls` per V-9's exclusive-pairing rule). `V2RaceArm::Effect` canonical variant (tag `0x02`). Ancestor-membership reconciliation (`v2_reconcile_ancestor_membership`) so nested guard/barrier/race records stay consistent across fork/join/trigger/cancel.
| V4.2 | `bpmn-lite-kernel/src/lib.rs`, `Cargo.toml` | `check_k_invariants` (K-1 member liveness, K-2 stack↔membership consistency, K-3 barrier soundness `0 < count <= arity`), `effective_control_stack` (control stack plus any parked wait-state-implied handle). 200-case proptest driving random `Tick`/`V2TriggerGuard` sequences against the 18-instruction oracle program. Found and fixed 3 real kernel bugs (guard-open member registration, ancestor-membership transfer on fork/cancellation) plus 1 checker-side bug.
| V4.3 | `bpmn-lite-types/src/integrity_rings.rs`, `bpmn-lite-kernel/src/lib.rs` | `IntegrityError::Ring3Runtime`, `TransitionError::Integrity`. `ring3_shadow_check` runs unconditionally on every `apply()` result: PC/stack/control-depth bounds, K-1/K-2/K-3, single-owner pending-effect check. Two hand-corruption receipt tests.
| V4.4 | — | **Skipped** — task text contradicts the V2.7 entry amendment on when the v1 block is deleted; surfaced to Adam, who said "skip." Recorded in the plan doc, not silently resolved either way.
| V4.5 | `bpmn-lite-kernel/src/lib.rs` | Golden-transition tests reproducing both scenarios of the locked oracle (`EOP-EX-BPMN-ISA-002.md`) byte-for-byte against real `kernel::apply()` calls. Found and documented two drafting gaps in the oracle's own bullet-list prose (not fixed in the locked file).

No compiler emission of v2 words yet (V5's scope) — V4 is exclusively: does the kernel correctly *execute* v2 words once emitted by hand, and does it fail closed when something is structurally wrong.

---

## 2. Independent review — methodology

The review agent was given the diff, the spec (§6/§7/§13), and the locked oracle, and instructed to:
- Derive expected behaviour from the spec text directly, not from this session's doc comments (which could themselves encode the same misreading as the code).
- Not run `cargo test`/`cargo build` — read the code and reason about it, so its findings are independent of whatever the existing test suite happens to already assert.
- Assume at least one bug exists and actively hunt rather than confirm-check.

Its report distinguished one **concrete, reproducible, spec-contradicting bug** from a set of items it checked and found either correct or an already-documented, deliberate deviation (the V4.4 skip, the oracle gaps). That finding is below, already fixed with a receipt.

---

## 3. Confirmed finding — `apply_job_failure`'s guard selection ignored guard nesting

**Location:** `bpmn-lite-kernel/src/lib.rs`, `apply_job_failure`, the V&S §13 ruling C automatic-rollback-on-definitive-failure branch (~line 2372, pre-fix).

**Spec text (§13 amendment v0.5, ruling C):**
> Automatic rollback on definitive failure, interrupting guards only. [...] `GUARD-N>` scopes are unaffected — non-interrupting guards don't unwind on trigger by design (ruling A above), so "roll back on fail" doesn't fit their model; **today's v1 incident/routing path is unchanged for fibres whose innermost armed guard is non-interrupting**, or who sit inside no guard scope at all.

**Bug:** the pre-fix code was
```rust
if let Some(guard_handle) = fiber.control_stack.iter().rev().find(|id| {
    matches!(
        snapshot.concurrency_table().get(**id),
        Some(record)
            if matches!(record.kind, RecordKind::Guard { interrupting: true })
                && record.state == RecordState::Armed
    )
}) { /* automatic rollback via guard_handle */ }
```
This scans the control stack innermost-to-outermost and returns the **first interrupting guard found anywhere on the stack** — it does not stop at the innermost guard-kind record regardless of that record's own interrupting flag. For a fibre nested `V2Guard(interrupting) > V2GuardN(non-interrupting) > [failing task]`, the innermost armed guard is the non-interrupting `V2GuardN`, so per ruling C the v1 incident path must fire. The old code instead skips past the non-interrupting `V2GuardN` and finds the outer interrupting `V2Guard`, incorrectly triggering automatic rollback (and killing the fibre) for a topology the spec explicitly carves out.

**Blast radius:** narrow — only fibres with a non-interrupting guard nested *inside* an interrupting one, hitting a definitive job failure while parked under the inner scope. No V4.1–V4.5 test exercised nested-guard topology (all prior tests use a single, unnested guard), so this was not caught until this review.

**Fix:** replaced the `.find()` with `.find_map()` that walks innermost-to-outermost, skips non-guard handles (a `V2Barrier`/`V2Race` isn't a guard, so it doesn't count as "the innermost guard"), and **stops at the first guard-kind record it meets** — returning its `interrupting` flag rather than continuing to search for *any* interrupting guard further out. Automatic rollback now fires only when that innermost guard's flag is `true`.

```rust
let innermost_guard = fiber.control_stack.iter().rev().find_map(|id| {
    let record = snapshot.concurrency_table().get(*id)?;
    if record.state != RecordState::Armed {
        return None;
    }
    match record.kind {
        RecordKind::Guard { interrupting } => Some((*id, interrupting)),
        _ => None,
    }
});
if let Some((guard_handle, true)) = innermost_guard { /* automatic rollback, unchanged */ }
```

**Receipt (red→green, both runs on the exact same test):**
- New test `definitive_job_failure_under_non_interrupting_guard_nested_inside_interrupting_guard_still_incidents` (`bpmn-lite-kernel/src/lib.rs`), program: `V2Guard(interrupting) → V2GuardN(non-interrupting) → ExecNative → ...`, definitive `EffectFailed` while parked on the job.
  - **Against the pre-fix code:** `FAILED` — `assertion left == right failed: ... left: 0, right: 1` (`t2.incidents().len()`, expected 1, got 0 — rollback fired instead of the incident).
  - **Against the fixed code:** `ok` — incident fires, fibre is not killed, payload is not rolled back, no concurrency mutation (no scope retired).
- Existing single-guard regression test `definitive_job_failure_inside_interrupting_guard_rolls_back_instead_of_incident` still `ok` on the fixed code — the non-nested case is unchanged.
- `cargo test -p bpmn-lite-kernel` (21 tests) and `-p bpmn-lite-types` (74 + 2 doctests) both green; `cargo build --workspace` and `cargo clippy -p bpmn-lite-kernel --all-targets -- -D warnings` both clean.

---

## 4. Everything else the review checked and closed clean

- **K-1/K-2/K-3 discharge (V4.2):** `check_k_invariants` correctly scopes to `RecordState::Armed` records only (retired records' dangling membership is by-design, not a gap); `effective_control_stack`'s wait-state-implied-handle addition is necessary because `V2Join`(non-last)/`V2RaceClose` park with their handle moved into `WaitState`, not the control stack — confirmed against `v2_cancel_guard_scope`'s pre-existing convention for the same pattern.
- **Ring 3 (V4.3):** `ring3_shadow_check` runs unconditionally on every `apply()` call, not gated to specific commands — matches the spec's "every apply() call is a park or resume, so it always applies" framing. Single-owner pending-effect check correctly walks both `WaitState::Effect` and `WaitState::V2Race` arms into one `BTreeMap<EffectId, Uuid>`, so a duplicate owner on either wait-shape is caught.
- **Golden-transition fixtures (V4.5):** both oracle scenarios reproduce byte-for-byte against real `apply()` calls. The oracle document itself was correctly left untouched (respecting its LOCKED status) despite finding two drafting gaps in its bullet-list prose (a missing `Retire(G)` mention, a missing F1-in-`fibers_delete` mention) — both are omissions in the prose, not semantic disagreements, evidenced by the oracle's own field-by-field expected-value tables already implying the behaviour the test asserts.
- **V4.4 skip:** confirmed still correctly recorded as an open contradiction (not silently resolved either way) between the task text and the V2.7 entry amendment on when the v1 block is deleted — deleting it now would break every currently-compiled v1 program, and the two plan-doc lines disagree on timing. Left exactly as Adam directed ("skip"), documented in `EOP-PLAN-BPMN-ISA-002.md`.

---

## 5. Reviewer's disposition

- [x] V4.1 (D2 word set) — **verified clean**, including the `V2ArmEffect`/`V2AwaitEffect` FFI-completion resolution logic and the `v2_ffi_task_decls` side-table pairing check.
- [x] V4.2 (K-theorems) — **verified clean.**
- [x] V4.3 (Ring 3) — **verified clean.**
- [x] V4.4 — **correctly left open**, contradiction accurately surfaced, no action needed beyond what's already recorded.
- [x] V4.5 (golden transitions) — **verified clean**, oracle gaps correctly documented rather than silently patched around.
- [x] `apply_job_failure` guard selection — **BLOCKING, confirmed and fixed.** Red→green receipt above. No other call site in the kernel does stack-scanning guard selection (grep confirms `apply_job_failure` is the only place that walks `control_stack` looking for a `RecordKind::Guard`), so this fix has no analogous sibling bug to also check.

**Overall: V4 is closed pending Adam's sign-off** on this fix and acknowledgment of the two already-documented, deliberately-left-open items (V4.4's contradiction, the oracle's two prose gaps) — neither of which is new information, both were already surfaced during V4.4/V4.5 and are repeated here only for a reviewer reading this document cold.
