# EOP-VS-BPMN-ISA-002 — Structured-Concurrency Vocabulary for the bpmn-lite Fibre Machine (ISA v2)

**Status:** v0.3 — v0.2 revised per adversarial review R1 (verdict: proceed; all points dispositioned in §11); v0.3 adds the static-structure/dynamic-activation law (§4) and converts `JOIN` to handle-based resolution (§5, V-3) after a code/data-separation challenge caught static-id lookup as a taxonomy leak.
**Depends on:** EOP-PLAN-BPMN-KERNEL-001 landed in full (T4–T12); artifact/snapshot versioning (E8), replay harness (T10), differential-execution harness (T9.7) all operational.
**Deployment context:** greenfield — no production instances exist. Backward compatibility is a non-goal; persisted-state versioning is retained solely as a corruption tripwire (see D3).
**Companion documents (to follow):** EOP-EX-BPMN-ISA-002 (worked example / shared oracle), EOP-PLAN-BPMN-ISA-002 (tranche plan V1–V6).
**Review protocol:** authorship-blind; reviewer context assembled by inclusion-list; the worked example is oracle-excluded until D1–D4 ratify.

---

> ## Design principle
> **Side tables are code. Fibre state is data.**
> Instructions manipulate fibre-owned concurrency state. Everything the machine consults at run time to decide concurrency behaviour is data written by words inside `kernel::apply`, sealed under the frame hash, and replayed deterministically. Everything static is dictionary: hashed in the artifact envelope, immutable, never consulted as runtime state. Every decision in this document is judged against this principle.

## 1. Context and problem statement

Post-KERNEL-001, the fibre machine is durably correct: one deterministic `kernel::apply`, one fenced `commit_transition`, fibres as persisted continuations (PC + operand stack + registers + wait state). What remains structurally unsatisfying is that BPMN's *event-driven and structured-concurrency* semantics — join barriers, race arming/resolution, boundary-event scopes, timer arming — are encoded outside the instruction stream: partly in artifact side tables (race plans, join metadata, boundary routes), partly in auxiliary runtime rows, and historically in engine post-tick passes.

KERNEL-001 T7 made these paths *atomic* by folding them into `apply`, but it did not make them *instructional*. The kernel still interprets side-table cross-references at run time to decide concurrency behaviour. Consequences:

1. **Verification is weaker than it could be.** The T6.3 verifier proves operand-stack safety per CFG edge, but concurrency well-formedness (guard balance, race nesting, fork/join arity) is only checkable by cross-referencing side tables against the instruction stream — referential integrity, not dataflow proof.
2. **Executable meaning is split between code and metadata.** Roughly half of BPMN semantics lives in side tables. This forced the T6 envelope to hash everything, which is correct but treats a symptom.
3. **The kernel carries interpretation logic that a richer ISA would delete.** Race resolution and boundary promotion are ~semantic switch statements over metadata rather than word execution.

### Why FORTH is the right analogy

BPMN-lite execution is already a stack machine: a fibre is a persisted continuation whose entire execution state is a program counter, an operand stack, and registers — precisely the FORTH inner-interpreter shape, chosen originally because it is the minimal machine whose complete state can be serialized at any instruction boundary. FORTH is therefore not a stylistic reference but the closest prior art for the specific problem this ISA solves: durable, resumable execution where the machine state *is* the context-switch frame. FORTH's two-stack architecture (parameter stack for data, return stack for control) is the template this proposal completes: v1 built the parameter stack; v2 adds the control stack. One deliberate departure from a classic threaded interpreter: persistence granularity is the **transition**, not the instruction. A classic FORTH thread may stop between any two NEXTs; this machine chooses its stopping points — parks and commits — and everything between two sealed frames is a pure, deterministic, replayable burst. Commit frequency is therefore proportional to *waits*, not to instruction count.

### Structured-concurrency lineage

The proposal imports one older idea into BPMN execution: control structures should be block-structured and statically checkable. Dijkstra's argument against unstructured jumps, Hoare's CSP alternation (the direct ancestor of `RACE{`), and the modern structured-concurrency principle — a concurrent child's lifetime is bounded by a syntactic scope in its parent — are the intellectual landmarks. BPMN's boundary events, event gateways, and parallel joins are structured-concurrency constructs that the v1 encoding flattened into metadata; v2 restores their block structure to the instruction stream, where a verifier can prove it.

## 2. Glossary (binding for all ISA-002 documents and generated identifiers)

| Term | Meaning — and only this meaning |
|---|---|
| **Scope** | A dynamic extent opened by a word (`GUARD>`, `RACE{`) and closed by its pair, within which fibres are members. The general category. |
| **Guard** | A scope of kind *guard*: carries a handler; interrupting (`GUARD>`) unwinds members on trigger, non-interrupting (`GUARD-N>`) spawns the handler without unwinding. |
| **Race** | A scope of kind *race*: first-wins over N armed alternatives; resolution runs the winner and cancels losing members in one transition. |
| **Boundary event** | The *BPMN authoring construct* that frontends lower to a guard. Never used to name runtime state. |
| **Barrier** | Synchronisation state: the arrival counter created by `FORK n` and consumed by `JOIN n id`. Not a scope. |
| **Control stack** | The per-fibre second stack (return-stack analogue): an ordered stack of concurrency-record handles the fibre is inside. Holds scope handles today; compensation and transaction records later. |
| **Concurrency table** | The snapshot-resident table of concurrency records — scopes, barriers, (later) compensation records — keyed by record ID: kind, members, handler, state, counters. |
| **Handle** | A fibre's reference to a concurrency-table record (an ID pushed on its control stack). |
| **Frame** | The complete persisted context-switch unit hashed by D3-L2: snapshot ‖ fibres ‖ concurrency table ‖ pending effects ‖ revision ‖ artifact hash. Frame size is proportional to live tokens and their bounded stacks — never to program size; the program is shared, immutable, and referenced by hash only. |
| **Instruction (cell)** | One element of the compiled, flattened program — the threaded-code "necklace." Static, immutable, addressed by `Addr`. Never called a token. |
| **Token** | The BPMN sense only: a locus of execution moving through the program. Realized as a **fibre** — a cursor (PC) into the instruction list plus its operand stack, control stack, and registers. The AST/DAG is compile-time only; tokens traverse compiled cells, never tree nodes. |

## 3. Non-goals

- No change to the durability substrate: `Claim`, `Transition`, `commit_transition`, fencing, effect identity, journal, and replay are consumed as-is.
- No change to the effect boundary with ob-poc/DSL runtime: verbs remain `DurableEffect::Invoke` over the bus. This V&S is about *process time*, not domain meaning.
- No new BPMN feature coverage beyond what v1 executes today, except where D2 explicitly admits it (non-interrupting guards).
- No dual-ISA kernel and no migration machinery of any kind: greenfield deployment, exactly one instruction set live in `apply`, cutover is wipe-and-recompile (see D3).
- No error correction or self-healing. The integrity model is detect-and-fail-stop only.

## 4. Decision D1 — Concurrency-state ownership across fibres

### The problem

FORTH `CATCH`/`THROW` marks the return stack of one thread of execution. A BPMN guard covers a *subtree of fibres*: an interrupting timer on a subprocess must, on firing, unwind and cancel every token inside that subprocess, including tokens forked after the guard was armed. A race is first-wins over N alternatives where losers' members are cancelled *in the same transition* as the winner proceeds. A per-fibre stack alone cannot express "this scope covers fibres that do not exist yet."

### Option A — Record ownership by arming fibre, membership by inherited lineage path

- Pros: most FORTH-native; mutation fibre-local; records serialize inside the owning fibre.
- Cons: cancellation scans all fibre paths; records outliving their arming fibre (arming fibre parks/ends while children run) require an ownership-transfer rule — the design wart.

### Option B — Concurrency table alone (fibres reference records by ID, no per-fibre stack)

- Pros: cancellation set directly enumerable; record lifetime decoupled from any fibre.
- Cons: the verifier loses its literal stack; per-fibre balance becomes a derived property proven over a synthesized list — strictly weaker dataflow.

### Option C — Control stack of handles + concurrency table (recommended)

Fibres carry a **control stack** of handles (literal push/pop, classic dataflow for the verifier). The **concurrency table** holds the records: kind, membership, handler, state, counters. `GUARD>` allocates a record and pushes its handle; `<GUARD` pops and retires it; `FORK` copies the parent's control stack into children and registers them as members; `JOIN` decrements a barrier record.

**Canonicity (revised per review R1):** membership is *not* derivable from the control stacks — once cancellation and fibre death exist, a dead fibre's stack is gone while the record's membership history may still matter (and will, for compensation). Therefore:

> The control stack is canonical. Concurrency-table membership is canonical. Their mutual consistency is an invariant (K-2, §7) maintained exclusively by word execution inside `apply`, shadow-asserted at every park/resume (D3-L4).

This also sharpens the response to the "A and B welded together" rebuttal: C is not one fact in two representations; it is two canonical facts — *what am I inside?* (control stack) and *who is inside me?* (membership) — bound by a consistency law. The two questions are fundamentally different; no single structure answers both without pathology.

**Determinism rider:** cancellation and loser-unwinding order is fibre-ID order (context-derived IDs, stable under replay), never map iteration order. Cancelling a fibre unwinds its control stack innermost-first before removal.

### Static structure vs. dynamic activation (v0.3 addition)

The compiled artifact is the whole taxonomy — every block that *could* execute; a token is one cursor in it. These never co-mingle:

> **Artifact addresses never appear in runtime state; runtime handles never appear in artifacts.**

The control stack does not hold program structure — it holds *activations* of static blocks: fresh concurrency-table records allocated per execution (a `GUARD>` in a loop arms a fresh record each iteration; each `FORK` execution allocates a fresh barrier activation). The relationship is exactly return stack to colon definition: activation records, not definitions. Static nesting is proven over the artifact (V-1/V-2); dynamic extent is instantiated per token and governed by K-1..K-3. Enforced by construction: `Addr` (artifact) and `RecordId` (runtime handle) are distinct types with no conversion; V-9 keeps runtime-consulted semantics out of the envelope; the K-invariants keep artifact identities out of records. The lexical failure mode this forbids is the historical one — a token that *is* a pointer into the model (PlanWalker's `current_node_id`) or structure consulted as state (v1 side tables).

## 5. Decision D2 — Word inventory and perimeter

Stack-effect notation: `( operand-stack-before -- after )` `[ control-stack-before -- after ]`. All words are kernel-pure; effect-emitting words append to `Transition.effects`.

| Word | Effect | Semantics |
|---|---|---|
| `GUARD>` | `( handler-addr -- ) [ -- h ]` | Allocate interrupting-guard record, push handle. Handler addr is a verified code address. |
| `<GUARD` | `( -- ) [ h -- ]` | Pop and retire the guard. Verifier: must match its `GUARD>` on all paths (V-1). |
| `GUARD-N>` / `<GUARD-N` | as above | Non-interrupting guard: on trigger, spawn handler fibre *without* unwinding members; record stays armed (or re-arms per spec). Distinct opcodes, not a flag — the distinction must be visible to static analysis at the opcode level, exactly as `CALL`/`RET` are distinct instructions (review-ratified). **The one deliberate v1-coverage extension.** |
| `RACE{` | `( n -- ) [ -- h ]` | Open first-wins race record over the next n arms. |
| `ARM-TIMER` | `( duration -- ) [ h ]` | Emit `ScheduleTimer` effect bound to the race; deterministic timer ID per T5. |
| `ARM-MSG` | `( correlation -- ) [ h ]` | Register message alternative. |
| `ARM-EFFECT` | `( effect-desc -- ) [ h ]` | Arm an external-effect alternative. |
| `}RACE` | `( -- ) [ h -- ]` | Park on the race. A command addressed to an armed alternative resolves it: winner continuation runs; other arms' pending effects consumed/cancelled; losing members cancelled in fibre-ID order — one transition. |
| `FORK n` | `( addr1..addrn n -- ) [ s ]` | Allocate a fresh barrier *activation* record; create n fibres at given addresses, each inheriting the parent's control stack **with the barrier handle pushed on top**, and registered as members. Re-entry (a bounded loop containing this FORK) allocates a fresh activation per execution. Fibre IDs context-derived. |
| `JOIN` | `( -- ) [ h -- ]` | Pop the inherited barrier handle; decrement *that activation*; park unless last arrival; last arrival continues and retires it. Resolution is by dynamic handle only — never by static identity. The static FORK/JOIN pairing id exists solely as a verifier annotation for V-3's arity proof. |
| `WAIT-FOR` / `WAIT-UNTIL` | `( duration\|deadline -- )` | Park + emit timer effect (T5 mechanism as a word). |
| `WAIT-MSG` | `( correlation -- )` | Park on message. |
| `AWAIT-EFFECT` | `( effect-desc -- )` | Emit `DurableEffect::Invoke` + park on completion; effect-ID derivation is the word's, per E5. |
| `CANCEL-SCOPE` | `( -- ) [ h ]` | Explicit cancellation (BPMN *terminate* semantics: unwind members innermost-first, run no handler; cancel-*events* are encoded as guards — see Q4). |

### Deferred to v3 (explicit non-coverage)

- **Compensation.** Reverse-order handler execution over completed activities with its own token semantics. Binding admission requirements on v2: concurrency-table records carry a `kind` discriminant with room for `Compensation`; retirement may *archive* rather than delete (compensation needs completed-scope history); the control stack accepts non-scope record kinds. The table and stack were renamed (from "scope table"/"scope stack") precisely because barriers already break the scope category and compensation will further — the structures are named for their role, not their first tenant.
- **Event subprocesses.** Expressible later as instance-root non-interrupting guards.
- **Multi-instance activities.** Sequential MI is a bounded loop (already verifiable). Parallel MI needs dynamic FORK arity, which breaks V-3's static-arity theorem; deferred to a v3 `FORK-DYN` design with runtime-checked limits.

## 6. Decision D3 — Persistence integrity model (corruption detection for the switch/persist/resume cycle)

### Reframing

There are no production instances; migration machinery is deleted from scope — cutover is wipe-and-recompile. Version fields survive with inverted purpose: each versioned surface accepts **exactly one value**; any other value is corruption or foreign data and refuses atomically. Versioning is a tripwire, not a dispatch key.

Objective: **cast-iron detection of corruption** across the context-switch cycle. Every park seals a frame; every resume proves the seal before a single instruction executes. Detection, not correction: fail-stop by design — never repair, never guess.

### Five concentric rings (revised structure per review R1)

Verification proceeds outermost-in on resume; each ring assumes nothing proven by an inner one.

**Ring 1 — Physical integrity.** Canonical bytes are the stored bytes. One deterministic encoding (BTreeMap-only, fixed field order); the frame is canonicalized, hashed, and *those bytes* persisted (BYTEA). On load: verify the hash over raw bytes **before decoding** — a corrupted frame never reaches the deserializer. Version tripwires are checked here, on the envelope, pre-decode. Operational introspection is a rebuildable JSONB projection, never authoritative (M2 doctrine).

**Ring 2 — Frame integrity.** The hash domain is the transition closure: `state_hash = BLAKE3(canonical(snapshot ‖ fibres by fibre-ID ‖ concurrency table ‖ pending-effect set ‖ revision ‖ artifact_hash))` — everything required to execute the next transition, resolving M9 by construction. Artifact binding lives inside the digest, so wrong-artifact execution is unrepresentable. The journal chains frames: records carry `prior_state_hash → new_state_hash`; the snapshot row stores its hash and producing sequence; resume verifies three-way agreement (recomputed == stored == `journal[last_seq].new_state_hash`). E3 makes torn commits impossible by design; the chain makes an E3 *implementation* failure detectable — the defended threat is future bugs, not only storage faults.

**Ring 3 — Runtime (semantic) integrity.** Unconditional structural asserts over the decoded frame at every park/resume: PC within program; operand/control stack heights within `VerifiedLimits`; every handle resolves in the concurrency table; membership↔control-stack consistency (shadow of K-2); every member references a live fibre (shadow of K-1); barrier counts ≤ static arity; every pending effect owned by exactly one waiting fibre. These are the runtime shadows of proven properties (V-1..V-9 static over the artifact; K-1..K-3 inductive over `apply`): a verified program under a proven kernel yielding a structurally invalid frame has one explanation — corruption — so the asserts are unconditional, fail-closed, O(fibres + records).

**Ring 4 — Replay integrity.** With zero version adapters in the system, T10 replay divergence has a single meaning: corruption. Nightly from-checkpoint replay fleet-wide; forensic from-genesis replay on every quarantine.

**Ring 5 — Quarantine.** Any ring fires ⇒ typed `IntegrityError` variant naming the ring, atomic quarantine, readiness reflects it, zero partial reads of the condemned frame.

### Failure-model coverage

| Threat | Ring |
|---|---|
| Torn / partial write | E3 (prevention) + Ring 2 chain (detection of E3 failure) |
| Bit rot, storage fault, truncation | Ring 1 + Ring 2 |
| Kernel bug writing inconsistent state | Ring 3 + Ring 4 |
| Stale concurrent writer | Fence (E2, landed) |
| Wrong-artifact execution | Artifact hash inside Ring 2 |
| Foreign / stale-format / hand-crafted data | Ring 1 tripwires |

### Named risk R1 — canonical-form drift

Every ring above Ring 1 rests on it: nondeterministic serialization (map ordering, float formatting, enum representation, serde attribute drift, dependency upgrades altering encodings) diverges hashes without corruption — or hides a defect inside tolerated nondeterminism. Mitigations: (a) the canonical encoding is a hand-audited, dependency-pinned module with a golden-bytes corpus (fixture frames whose exact bytes are committed and diffed in CI); (b) round-trip fixed-point law `canonicalize(decode(bytes)) == bytes` for every fixture and property-fuzzed frame; (c) sampled runtime round-trip assertion on commit; (d) any encoding change is definitionally a tripwire change and a wipe — there is no "compatible" encoding change.

## 7. Decision D4 — Proof obligations

Two kinds of theorem, deliberately separated (review R1: member-liveness cannot be an artifact theorem — no static analysis of a program proves a property of frames; what is provable is preservation by the kernel).

### Artifact theorems V-1..V-9 — static, per-CFG-edge dataflow, discharged by the T6.3 verifier

- **V-1 (control-stack balance).** On every fibre-reachable path, the abstract control stack at `<GUARD`/`}RACE` matches the corresponding opener; at every fibre `END`, the control stack is empty.
- **V-2 (proper nesting).** Openers/closers well-bracketed on all paths; no path closes a record opened on a different path without a join point at equal control depth.
- **V-3 (arity agreement).** Every `JOIN` pops a handle whose allocating `FORK` has static arity matching the joined fibre count; static pairing annotations are unique per artifact and referenced by exactly one FORK/JOIN pair. The annotation is proof material only — runtime resolution is exclusively via the inherited handle, so re-entrant FORKs (each execution a fresh activation) are sound by construction.
- **V-4 (handler validity).** Every guard handler address is a valid instruction with well-typed entry state (operand + control stacks) given the guard's extent; handler CFGs terminate in `END` or rejoin at legal state.
- **V-5 (race shape).** `RACE{ n` followed by exactly n `ARM-*` words before `}RACE` on all paths; arms are effect/message/timer registrations only — no unguarded state mutation between arm and park.
- **V-6 (operand-stack safety).** Unchanged from T6.3, extended over all v2 words' operand effects.
- **V-7 (limits).** `VerifiedLimits` gains `max_control_depth`, `max_barriers`, `max_records`; computed maxima embedded in the envelope; Ring 3 asserts them.
- **V-8 (bounded flow).** Backward-flow bounds hold across handler and race-resolution edges; guards create no unbounded re-entry.
- **V-9 (dictionary purity).** The v2 envelope structurally contains no execution-time-mutable semantics: symbols, debug, FFI declarations, correlation-key schemas, DSL vocabulary pin only. The race-plan/join/boundary-route tables do not exist in the v2 envelope type.

### Kernel preservation theorems K-1..K-3 — inductive invariants of `apply`, discharged per-word, property-tested, shadow-asserted by Ring 3

For every command and every word, `apply` preserves:

- **K-1 (member liveness).** Every member of every live concurrency record references a live fibre. (Every fibre-terminating word — `END`, cancellation, race-loss — deregisters the fibre from all records; every registering word registers only live fibres.)
- **K-2 (stack↔membership consistency).** A fibre's control stack and the records' membership sets agree: `h` on fibre F's stack ⟺ F ∈ members(h), for live F. (D1's consistency law.)
- **K-3 (barrier soundness).** For every live barrier: `0 ≤ count ≤ arity`, and `count` equals arity minus the number of member fibres that have executed its `JOIN`; retirement occurs exactly at zero.

Discharge protocol: each word's implementation carries its K-preservation argument as a doc-comment proof sketch; property tests generate word sequences and check K-1..K-3 after every `apply`; golden-transition tests pin exact frames. Ring 3 asserts the same facts in production — a proven invariant with a shadow, not a runtime hope.

Property-test obligation: fuzzed programs violating any V-theorem are rejected with typed errors, none panic (extends T6.6); fuzzed command sequences never produce a K-violation (a K-violation in test is a kernel defect, not an input defect).

## 8. ABI and schema impact summary

| Surface | Change | Version mechanism |
|---|---|---|
| Instruction set | +~14 words; race/join/boundary side tables removed from envelope | `ArtifactAbi` single-value tripwire; no translator |
| Fibre snapshot | + control stack | `SnapshotSchema` single-value tripwire; no upgrade fns |
| Snapshot | + concurrency table (records: id, kind, members, handler, state, counters); persisted as canonical BYTEA per Ring 1 | same tripwire |
| Journal | + events: `ScopeArmed`, `ScopeRetired`, `RaceResolved{winner}`, `ScopeCancelled`, `HandlerSpawned`; + `prior_state_hash`/`new_state_hash` chain (Ring 2) | journal schema single-value tripwire |
| Kernel | race-resolution and boundary-promotion interpretation deleted; word execution + K-theorem discharge added | LOC delta is an exit metric (net negative expected) |
| Verifier | dual-stack interpretation, V-1..V-9 | — |
| Effects/timers/commit protocol | **no change** | — |

## 9. Inputs required before ratification

1. **Fixture corpus:** existing join/race/boundary/timer tests + EOP-EX-BPMN-ISA-002's adversarial workflow (interrupting guard over a parallel subprocess nested inside a race with a message alternative), hand-lowered with full dual-stack traces including the cancellation cascade. Drafted only after D1/D2 ratify, then locked as oracle.
2. **Corruption-injection fixture set:** for each D3 ring, at least one deliberately damaged frame (flipped byte under the hash, chain break, dangling handle, membership asymmetry, over-arity barrier, orphaned pending effect, wrong tripwire version) proving the ring fires with the correct typed `IntegrityError` and quarantines atomically. Committed alongside the golden-bytes corpus.
3. **DSL lowering confirmation:** every plan construct the DSL frontend emits post-T9 maps onto the D2 word set with no residue (splits→`FORK`, joins→`JOIN`, callouts→`AWAIT-EFFECT`, timeouts→`RACE{`/`ARM-TIMER`) — any residue is a D2 amendment, found now, not in tranche V4.

## 10. Questions for reviewers

- Q1: Does the K-2 consistency law (both structures canonical, invariant maintained solely by word execution) survive adversarial scrutiny — is there a command sequence inside `apply` that splits control stack from membership without failing a K-2 property test?
- Q2: Distinct opcodes for non-interrupting guards ratified in review R1; remaining sub-question — should `GUARD-N>` re-arm after trigger by default, or is re-arming a per-guard artifact attribute (verified constant)?
- Q3: Ring 1 makes canonical BYTEA authoritative and demotes queryable state to a rebuildable projection. Is the operational cost acceptable, and should the projection ship in v2 or await demonstrated need? Also: single BLAKE3 over the whole frame vs per-fibre row digests — identical detection power, better forensic localization vs a simpler canonical module. Current lean: whole-frame for v2.
- Q4: `CANCEL-SCOPE` picks BPMN *terminate* semantics (no handler); cancel-*events* encode as guards. Confirm or contest.
- Q5: Is deferring parallel multi-instance (dynamic FORK arity) acceptable for the custody-domain workflow inventory, or does any live EOP runbook require it in v2?

## 11. Review R1 disposition

| Reviewer point | Disposition |
|---|---|
| "Derivable membership" is false once cancellation/death exist | **Accepted; strengthened.** Both structures canonical; consistency is invariant K-2 (§4, §7). |
| Distinct opcodes for non-interrupting guards, never a flag | **Already proposed; ratified.** Rationale recorded at the word (§5). |
| Barrier counts are synchronisation, not scope → rename table | **Accepted.** "Concurrency table"; glossary separates scope/guard/race/barrier/boundary (§2). |
| Scope stack is the return-stack analogue → "control stack" | **Accepted.** Renamed throughout; v3 record kinds (compensation, transactions) push onto the same structure by design (§2, §5). |
| Split D3 into physical/semantic; concentric structure | **Accepted.** Five rings, outermost-in verification (§6). |
| Missing theorem: no live scope references a retired fibre | **Accepted with refinement.** Not statically provable over artifacts; recast as kernel preservation theorem K-1, inductively discharged per-word, shadow-asserted by Ring 3 (§7). |
| Explain why FORTH; cite structured-concurrency lineage; box the principle | **Accepted.** §1 additions; boxed principle at head. |
| Terminology drift (scope/guard/boundary/race/barrier) → glossary | **Accepted.** §2, binding on generated identifiers. |

---

*Ratification of D1–D4 unblocks EOP-EX-BPMN-ISA-002 (oracle) and EOP-PLAN-BPMN-ISA-002 (tranches V1–V6). Nothing in this document alters the KERNEL-001 durability substrate.*
