# EOP-REVIEW-BPMN-ISA-002 — V2.7 ISA Definition: Addressing-Scheme Review

**Status:** open — awaiting authorship-blind review.
**Reviewer:** must not be the author of the V2.7 diff (CAREFUL-tier protocol, `CLAUDE.md`).
**Scope:** the v2 `Instr` set added in Tranche V2.7 (`docs/todo/EOP-PLAN-BPMN-ISA-002.md`, "Tranche V2.7 — ISA Definition"), specifically the addressing-scheme decisions that propagate into V3 (verifier), V4 (kernel), and V5 (frontend lowering). This is *not* a request to review kernel-word semantics — no kernel words exist yet; that is V4.

Reference material: `docs/todo/EOP-VS-BPMN-ISA-002.md` §5 (D2 word inventory — the frozen source of truth) and §4 (D1 activation law). The reviewer should check the code against §5's table directly, not against this document's paraphrase.

---

## 1. What changed

| File | What |
|---|---|
| `bpmn-lite-types/src/types.rs` | 16 new `Instr` variants, `V2`-prefixed, appended after the v1 `Fail` variant (lines ~370-499). Compile-fail doctest on `Instr`'s enum doc comment (line ~199). |
| `bpmn-lite-types/src/artifact.rs` | `successors()` extended with v2 CFG edges (~line 607-635); new `ControlStackEffect` enum + `v2_control_stack_effect()` (~line 640-683); `#[cfg(test)] mod v2_fixtures` (~line 708 to end) — two hand-assembled programs. |
| `bpmn-lite-kernel/src/lib.rs` | `TransitionError::V2InstructionNotYetInterpretable(&'static str)` variant; one catch-all match arm in `apply_tick` over all 16 `V2*` variants returning it; `instr_v2_word_name()` helper. |
| `docs/todo/EOP-PLAN-BPMN-ISA-002.md` | New "Tranche V2.7" section (between V2.1h and V3), dependency diagram updated. |

No compiler emits v2 words (that's V5). No kernel word has real semantics (that's V4). This tranche is types + addressing + CFG-edge shape + hand-built fixtures, nothing else.

---

## 2. Decisions to review

### 2.1 Coexistence: zero v1 reuse

**Claim:** v1 `Instr` already has name- or shape-adjacent but non-conformant prior art for `Fork`/`Join` (static `JoinId`+arrival-count, not D2's dynamic-handle/activation-record model) and `WaitAny`/`WaitArm` (interrupting-vs-not as a `bool` flag, the exact anti-pattern D2's distinct-opcode requirement rejects). Disposition: every D2 word gets a `V2`-prefixed identifier; zero v1 variant is renamed, reused, or redefined.

**Check:** grep `bpmn-lite-types/src/types.rs` for the v1 `Fork`/`Join`/`WaitFor`/`WaitUntil`/`WaitMsg`/`WaitAny` variants (lines ~262-304) — confirm none were touched (diff should show pure addition, zero modification lines inside the v1 block). Confirm every new variant name is `V2`-prefixed with no exceptions.

**Why it matters:** if this is wrong — if a v1 identifier was silently reused or an existing v1 match arm's behavior was changed — every live v1-executing artifact is corrupted, silently, since nothing else in the codebase would detect a meaning-change on an existing serialized tag. This is the single highest-consequence check in this review.

### 2.2 Addressing: `Addr`-only, never `RecordId`

**Claim (V2.7 7.2):** guard handler extent (`V2Guard`/`V2GuardN`'s `handler`), race arm resume targets (`V2ArmTimer`/`V2ArmMsg`/`V2ArmEffect`'s `target`), and the `V2Fork`/`V2Join` static pairing annotation (`pairing`) are all `Addr`-space — proof material for the verifier's future V-3 arity check, never runtime execution state. Runtime `V2Join` resolution is exclusively via the dynamically-inherited handle minted by `V2Fork` ("never by static identity", §5).

**Check:**
- Every field above is typed `Addr` in `types.rs` — confirm by reading the variant definitions directly (~lines 379-458), not by trusting this summary.
- Compile-fail doctest (`types.rs` ~line 199-213) attempts `Instr::V2Guard { handler: RecordId::new(uuid::Uuid::nil()) }` and must fail to compile. Run: `cargo test -p bpmn-lite-types --doc` — expect `types::Instr (line 199) - compile fail ... ok`.
- Confirm no code path anywhere converts an `Addr` to a `RecordId` or vice versa for these fields (there is no `From`/`Into` between the two types at all — this is inherited from V1.1's existing wall, not new to V2.7, but confirm V2.7 didn't add a new conversion path).

**Open question for the reviewer:** is "the FORK's own address, reused as its `pairing` value, matched verbatim by the JOIN(s)' `pairing` field" an adequate proof-material identity scheme for V-3's future arity check, or does V-3 need something richer (e.g. a generation counter for re-entrant loops, so two different dynamic activations of the same static FORK are distinguishable to the verifier)? V2.7's position is that re-entrancy is a *dynamic* concern the kernel's activation-record allocation handles (a fresh activation per execution, V&S §5) and the verifier's arity check is purely static (same FORK, same `targets.len()`, regardless of how many times it executes) — but this is exactly the kind of judgment call V2.7 flagged as "propagates to V3" and didn't want decided unilaterally. Confirm or challenge this reasoning.

### 2.3 CFG representation: bytecode-stream, not `IRGraph`

**Claim (V2.7 7.3):** the verifier will walk a CFG built directly over the flat v2 `Instr` stream (extending `artifact.rs`'s pre-existing `successors()`), not the compiler's `IRGraph` (`bpmn-lite-compiler`'s `IRNode`/`IREdge`, BPMN-XML-shaped), because V2.7 7.4's hand-assembled fixtures have no compiler-side IR at all — no compiler emits v2 words until V5.

**Check:**
- `artifact.rs`'s `successors()` (~line 529 onward, v2 additions ~607-635) — confirm the new edges are semantically right against §5, not just "compiles and the fixtures pass":
  - `V2Guard`/`V2GuardN`: handler address + normal fallthrough (both are real reachable continuations — the guard doesn't consume control, it registers a handler and proceeds).
  - `V2ArmTimer`/`V2ArmMsg`/`V2ArmEffect`: arm's `target` + fallthrough to the next arm/`}RACE` (arms are declared sequentially; each arm's target is only reached if that arm wins, but statically both are reachable).
  - `V2Fork`: only the `targets`, no fallthrough (mirrors v1 `Fork`'s existing convention — the forking fiber does not itself continue past the fork point).
  - `V2RaceClose`/`V2CancelScope`: **no static successors at all** — `V2RaceClose`'s only continuations are the arm targets already recorded on the `V2Arm*` instructions; `V2CancelScope` unwinds and does not resume the fiber (treated like `End`/`EndTerminate`/`Fail`). **This is the one edge-shape decision most likely to be wrong or incomplete** — confirm against §5's actual semantics for `}RACE` and `CANCEL-SCOPE`, not just against internal consistency with the rest of the diff.
- `ControlStackEffect`/`v2_control_stack_effect()` (~line 640-683): a `Push`/`Pop`/`Peek`/`None` classification per instruction, explicitly scoped as "the dataflow fact V3 builds on, not the interpretation itself." Confirm the classification is defensible per §5's `[ ... ]` column for each word — in particular, `V2Fork`'s `None` classification (the pushed handle lands on the *spawned fibres'* stacks, not the forking fibre's own — a cross-fibre effect this single-instruction classification deliberately doesn't model, documented in a comment at that site) is worth independent scrutiny, since it's the one instruction whose real effect isn't visible in its own classification at all.

**Open question for the reviewer:** is a flat `successors()` extension (matching the existing v1 style exactly) the right long-term home, or should V3 introduce a dedicated CFG type now rather than layering more special cases onto a function that started as a v1 reachability helper? V2.7's position was "extend what already exists rather than build new machinery speculatively" (minimal-diff bias) — confirm this doesn't paint V3 into a corner.

### 2.4 Effect-emission shape on WAIT-*/ARM-* words

**Claim:** unlike v1's bare-park `WaitFor`/`WaitUntil`/`WaitMsg`, v2's `V2WaitFor`/`V2WaitUntil`/`V2AwaitEffect` and `V2ArmTimer`/`V2ArmEffect` are effect-emitting per §5 (`ScheduleTimer`/`Invoke` appended to `Transition.effects`), and their field shapes were chosen to carry enough data for that at V4 (mirroring `ExecFfi`'s existing `template_id`+`argc`/`retc` convention for the effect-invocation words, and embedding `duration_ms`/`deadline_ms` as static fields — a deliberate deviation from §5's literal `( duration -- )` operand-stack notation, on the grounds that v1's `Fork`'s arity and `WaitFor`'s `ms` are already static embedded fields in this codebase's convention, and V-5's race-shape theorem needs `V2RaceOpen`'s `arm_count` to be statically known regardless).

**Check:** is the static-embedded-field choice (versus literal operand-stack popping, which §5's notation shows) going to cause friction in V4 or V5? V5's frontends will need to *compile* a dynamic duration expression (e.g., a BPMN timer duration computed from a data object) down to something — if `duration_ms` is a static `u64` field, a dynamic duration has nowhere to go. **This may be the single most consequential open question in this review**, since it's an ISA design constraint that gets much more expensive to change once V4 and V5 depend on it. V2.7 did not surface this trade-off explicitly before making the call — flagging it now for the reviewer's judgment rather than treating it as settled.

---

## 3. Evidence already gathered (not a substitute for independent verification)

- `cargo test -p bpmn-lite-types --doc` → both compile-fail doctests (`concurrency::RecordId`, `types::Instr`) pass.
- `cargo test -p bpmn-lite-types v2_fixtures` → 2/2 hand-assembled fixtures decode, pass the real `verify_program` structural checks (reachability, stack-height consistency, backward-branch bounding — no special-casing for v2), and round-trip byte-identically through the artifact's canonical form.
- `cargo build --workspace`, `cargo test --workspace` (103/103 binaries), `cargo clippy --workspace --all-targets --all-features -- -D warnings` all clean.
- `scripts/check-layering.sh`, `scripts/check-glossary.sh` (+ `--self-test`) clean.

None of this substitutes for the reviewer independently reading §5 against the code. Green tests prove the code is internally consistent and does what its author intended — they cannot prove the author's intent matches the frozen spec, which is exactly what this review is for.

---

## 4. Reviewer's disposition

- [x] 2.1 Coexistence rule — **verified clean.**
- [x] 2.2 Addressing wall — **verified clean.** Reviewer explicitly rejected adding a generation counter to the `V2Fork`/`V2Join` `pairing` field to disambiguate re-entrant activations: the static-only reasoning in §2.2 is correct as written, and a counter would itself be a §4 violation (runtime state used as artifact proof material).
- [x] 2.3 CFG edges — **CONCERN, since remediated (r.2).** `}RACE` performs `[ h -- ]` but was modeled as a no-successor sink; the arm-target edge needs to hang off `V2RaceClose` (where resolution and the pop both happen per §5), not off the `V2Arm*` word, so the abstract entry state at each arm target reflects the handle already popped — not stranded on a dead path. Required a fixture-level proof, not just a narrative fix (`race_handle_is_popped_in_abstract_entry_state_at_each_arm_target`).
- [x] 2.4 Effect-emission field shape — **BLOCKING, since remediated (r.1).** Static `duration_ms`/`deadline_ms` fields deviate from §5's `( duration -- )` operand-stack notation and give a runtime-computed BPMN timer duration nowhere to go at V5. Reworked to pop from the operand stack; `V2RaceOpen.arm_count` correctly kept static (verify-time-known, unlike duration) — do not conflate the two.
- [x] Handoff note (not a defect): `v2_control_stack_effect()`'s `None` for `V2Fork` must not be read by V3 as "nothing to model" — the pushed handle is a cross-fibre effect on the spawned fibres, requiring its own explicit dataflow fact in V3's abstract interpreter. Recorded in the function's doc comment (r.3).

**Overall: CLOSED.** Both findings remediated with fixture-level proof; full workspace re-verified green. V3 unblocked, pending the short confirmation re-review requested before V3 begins.
