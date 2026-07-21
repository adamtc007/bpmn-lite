# EOP-REVIEW-BPMN-ISA-002 — V3 Verifier: Abstract-Interpretation Core Review

**Status:** open — awaiting authorship-blind review.
**Reviewer:** must not be the author of the V3 diff (CAREFUL-tier protocol, `CLAUDE.md`).
**Scope:** `bpmn-lite-types/src/v2_verifier.rs` (`verify_v2_control_stack`) and its wiring into `bpmn-lite-types/src/artifact.rs`'s `verify_program`. Checklist = the V&S theorem list (`docs/todo/EOP-VS-BPMN-ISA-002.md` §7, V-1..V-9) — the reviewer should check the code against each theorem's definition directly, not against this document's paraphrase.

This is the highest-risk tranche in the plan (Adam's framing, V3 disposition): **a verifier that is wrong-but-green admits malformed programs whose corrupt frames the integrity rings cannot catch, because the bytes are semantically valid.** Green tests are necessary, not sufficient. This review is the actual gate.

---

## 1. What changed

| File | What |
|---|---|
| `bpmn-lite-types/src/v2_verifier.rs` (new) | `verify_v2_control_stack()` — worklist-based dual-stack abstract interpreter. `ScopeKind`/`ScopeToken`/`AbstractStack`. `V2ControlStackLimits`. 22 tests. |
| `bpmn-lite-types/src/artifact.rs` | `verify_program` calls `verify_v2_control_stack` and embeds its output into `VerifiedLimits` (V-7). `successors`/`race_arm_targets`/`stack_effect`/`require_address` changed from `fn` to `pub(crate) fn` so `v2_verifier.rs` can reuse them — no behavior change to any of the four. |
| `bpmn-lite-types/src/lib.rs` | `pub(crate) mod v2_verifier;` |

No kernel word has real execution semantics yet (V4's scope). No compiler emits v2 words yet (V5's scope). This tranche is exclusively: does the verifier correctly admit legal v2 programs and reject illegal ones.

---

## 2. What each theorem's code actually does — check against §7, not this summary

### V-1 (balance — control stack empty at every END)

`Instr::End | Instr::EndTerminate => if !state.is_empty() { reject }`. Straightforward — the abstract stack at that CFG address must be empty. **Check:** is there any CFG path to an End/EndTerminate that this walk doesn't visit (i.e., a soundness gap where an unreachable-to-the-walker path could carry a corrupt frame past the check)? The walk starts at address 0 and follows `successors()`/the custom v2 transitions exhaustively — confirm there's no address reachable at runtime that isn't reachable in this static walk.

### V-2 (nesting/bracketing across CFG joins)

Two mechanisms, not one:
1. Every close instruction (`V2GuardEnd`/`V2GuardNEnd`/`V2RaceClose`/`V2Join`) checks the TOP token's `kind` (and, for `V2Join`, its `opened_at`) before popping — a `V2GuardEnd` popping a `Race`-kind token is rejected even though depth is fine.
2. `propagate()` requires exact stack-content equality at every CFG merge point (not just matching depth) — this is what catches the "two branches open different kinds, converge, then close" case that depth-only tracking would miss entirely.

**Check the specific test `v2_rejects_cross_path_bracket_violation`** (`v2_verifier.rs`) — read the fixture and confirm it's a genuine adversarial case (two branches, `BrIf` choosing between them, one opens `V2Guard` the other opens `V2RaceOpen`, both converge on one `V2GuardEnd`) and not something that would be rejected for a trivial unrelated reason. **This is the single test most worth hand-tracing.**

### V-3 (arity — static pairing only, no runtime resolution)

`V2Join`'s handler: (a) confirms `pairing` addresses an actual `V2Fork` instruction; (b) confirms the top-of-stack token is `Barrier`-kind AND its `opened_at` equals this `V2Join`'s `pairing`. Neither check touches `RecordId`/`Handle` — **check:** grep `v2_verifier.rs` for any of those two types; there should be zero occurrences, since this module only ever constructs/compares `Addr` values. If a future edit accidentally imports `concurrency::RecordId`, that alone should be treated as a red flag independent of what the code does with it.

**Reviewable design note (not a defect, flagged for judgment):** the V2.7 review explicitly rejected adding a generation counter to `pairing` to disambiguate re-entrant activations, on the grounds that re-entrancy is the kernel's dynamic concern, not static proof material. This module's `V-3` check is consistent with that — it never tries to distinguish which *execution* of a re-entered `V2Fork` a `V2Join` belongs to, only that they're statically paired. Confirm this is still the right call now that it's load-bearing in actual verifier code, not just a addressing-scheme abstraction.

### V-4 (handler entry-state validity) — **the one genuine semantic judgment call in this diff**

`V2Guard`/`V2GuardN`'s handler edge propagates the **pre-push** state (the state before this guard's own token would exist); the fallthrough edge propagates the **post-push** state. Rationale in-code: an interrupting guard unwinds its own scope before the handler runs, so the handler never observes its own not-yet-retired token; a non-interrupting guard's handler is a fresh sibling fibre, not a continuation of the arming fibre's stack, so it doesn't inherit that fibre's newly-pushed token either.

**This is not a mechanical transcription of §5 — it's an inference from §5's prose ("interrupting ... unwind members ... run handler"; "non-interrupting ... spawn handler fibre without unwinding members") to a specific abstract-state rule.** The reviewer should independently derive what the handler's entry state *should* be from §5 and V&S §4 (the activation law) and check whether pre-push is actually right, or whether e.g. the non-interrupting case should differ from the interrupting case (the code currently treats them identically). If this is wrong, every downstream V4 kernel-word proof sketch for `GUARD-N>`'s handler-spawn semantics inherits the error.

### V-5 (race shape)

Not new code — reuses `race_arm_targets()` (V2.7), which already rejects an arm-count mismatch or a non-contiguous arm region. `v2_verifier.rs` calls it from the `V2RaceClose` case and has one sanity test (`v5_rejects_arm_count_mismatch`). **Check:** is reuse-without-re-implementation actually sufficient, or does V-5 require something `race_arm_targets` doesn't check (e.g., V&S's "no unguarded state mutation between arm and park" — does the current tolerance for `PushBool`/`PushI64`/`LoadFlag`/`Pop` between arms, added in V2.7 remediation r.2, correctly distinguish "an arm's own operand computation" from "unguarded state mutation"? `StoreFlag` specifically is excluded from the tolerated set — confirm that's deliberate and correct, not an oversight.)

### V-6 (operand effects of all v2 words)

Not implemented in this module at all — already discharged by V2.7's `stack_effect()` extension inside `verify_program`'s pre-existing operand-height walk (unchanged mechanism, just new match arms for `V2WaitFor`/`V2WaitUntil`/`V2ArmTimer`). **Check:** is it actually correct that V-6 needs no NEW mechanism, or does "operand effects of all v2 words" mean something broader than "operand stack height accounting" that this claim is quietly narrowing? Test: `mutation_operand_underflow_arm_timer_without_push_rejected_by_verify_program` proves the existing mechanism does fire for a v2 underflow case.

### V-7 (max_control_depth/max_barriers/max_records)

`verify_v2_control_stack` tracks `max_control_depth` (max stack length seen at any visited entry state) and `max_barriers` (max count of `Barrier`-kind tokens in any single entry state). `max_records` is incremented once per scope-opening instruction VISITED during the walk (`V2Guard`/`V2GuardN`/`V2RaceOpen`/`V2Fork`) — **check this specifically**: is "count of scope-opening instructions visited by the walk" the right definition of "maximum simultaneously-live concurrency-table records," or does a re-entrant loop's second-and-later iterations (which revisit the same instruction, and per `propagate()`'s convergence check, do NOT re-execute the counting logic since the address is already in `entry_states`) undercount records that are live simultaneously across concurrent fibres? This is a plausible source of an unsound (too-low) `max_records`, which V4/V6 would then use as a runtime bound — an undercounted bound is a real safety issue, not a cosmetic one.

### V-8 (bounded flow across handler/race-resolution edges)

Generalizes backward-edge rejection from v1's opcode-specific check to every v2 edge (`check_forward`, called for guard-handler, race-arm-via-`}RACE`, and fork-target edges). **Check:** is "reject ANY backward v2 edge, unconditionally" too strong? Flagged explicitly in the plan doc for this reason — confirm it doesn't foreclose a legitimate v2 authoring pattern V5's frontends will need (e.g., could a legal program ever need a race arm or guard handler that's lexically earlier in the instruction stream but still finitely bounded some other way V2.7/V3 didn't anticipate?).

### V-9 (structural: no v1 side tables for a v2 artifact)

Test-level check only (`v9_v2_only_artifact_carries_no_v1_race_join_boundary_side_tables`) — not a rejection rule, since nothing currently CAN populate those tables for a v2-only hand-assembled program (they're v1-compiler-populated). **Check:** is a passing test sufficient evidence for V-9, or does V-9 require an active rejection (e.g., if a future mixed v1/v2 artifact somehow has both v1 side-table entries AND v2 instructions, should the verifier reject that combination outright)? This wasn't built — flagging the gap rather than silently treating "no test failure" as "requirement met."

---

## 3. Evidence already gathered (not a substitute for independent verification)

- `cargo test -p bpmn-lite-types v2_verifier` → 22/22 green: 13 theorem-shaped unit tests (one legal-admit, several violation-rejects with theorem-tagged error messages), 9 mutation-corpus tests, 1 proptest (256 cases, zero panics).
- `cargo test -p bpmn-lite-types v2_fixtures` → V2.7's original 3 fixtures still pass now that V3's real checks are wired into `verify_program` (previously they only got operand-stack/reachability checks; now V-1..V-5/V-8 actually run on them).
- `cargo build --workspace`, `cargo test --workspace` (103/103 binaries), `cargo clippy --workspace --all-targets --all-features -- -D warnings` all clean.
- `scripts/check-layering.sh`, `scripts/check-glossary.sh`/`check-canonical-invariant.sh` (+ `--self-test`) all clean.

None of this substitutes for independently tracing the theorem definitions against the code, especially V-4 (the one genuine semantic call) and V-7 (the one plausible undercounting bug flagged above).

---

## 4. Reviewer's disposition

- [x] V-1 — **verified clean.**
- [x] V-2 — **verified clean.** Cross-path bracket violation hand-traced and confirmed correct: exact-stack-content equality at CFG merge is exactly the right mechanism for the depth-balanced-but-nesting-violating case — the reviewer's stated highest-risk concern going in.
- [x] V-3 — **verified clean.**
- [x] V-4 — **BLOCKING: justification required, not necessarily rework.** `V2Guard`/`V2GuardN` were propagating identical pre-push handler-entry state through one shared code path; §5 defines their trigger behaviour differently and §5 is underdetermined on the non-interrupting handler's own control stack, so identical treatment was an unjustified inference. Required: derive each case independently from §5/§4, in-code, with a fixture per case.
- [x] V-5 — **verified clean.** Forward note requested: document why `StoreFlag` is excluded from the tolerated inter-arm operand set (the read-vs-mutate line), so a future edit doesn't add it back.
- [x] V-6 — **verified clean.**
- [x] V-7 — **BLOCKING: confirmed a real undercounting bug, highest priority.** `max_records` counted static scope-opening instructions the walk visits, not maximum simultaneously-live concurrency records. A guard inside a `V2Fork`ed body is one static instruction but live once per spawned fibre; the static count undercounts. Since V4/V6 consume `max_records` as a runtime safety bound, an undercounted bound is a fail-closed check that doesn't fire — a genuine soundness defect, not a cosmetic one. Required: account for fork-fibre multiplicity and loop re-entry, not static visits; add a fixture proving a fork spawning N fibres each opening a guard yields `max_records` reflecting N, not 1.
- [x] V-8 — **accepted, keep unconditional, do not narrow.** Forward note requested: record it as a constraint V5's frontends must confirm they can live with.
- [x] V-9 — **CONFIRM: resolved as "needs an active check."** Determine whether a mixed v1/v2 artifact is type-level constructible; if yes (it is — `Instr` is one enum), add an active rejection rather than leaving V-9 checked-off-but-vacuous.

**Overall, first pass: two BLOCKING (V-4, V-7), one CONFIRM (V-9) resolved to "needs an active check," three forward notes (V-1, V-5, V-8) — all closed in the remediation below (`docs/todo/EOP-PLAN-BPMN-ISA-002.md`, "V3 review remediation" r.1–r.4). **r.2 (V-4)'s resolution diverges from this reviewer's own stated lean** ("each handler runs outside its own guard's scope" for both `V2Guard` and `V2GuardN`) — the executor's independent per-case derivation concluded `V2GuardN`'s handler is POST-push (inherits its own still-armed token), not pre-push, by direct analogy to `V2Fork`'s children-inherit-parent-stack pattern. This specific point is NOT yet re-confirmed by this reviewer and is the one open item in an otherwise-closed remediation — see the short confirmation re-review requested in the plan doc.
