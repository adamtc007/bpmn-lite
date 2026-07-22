# EOP-VS-BPMN-ISA-002 — Structured-Concurrency Vocabulary for the bpmn-lite Fibre Machine (ISA v2)

**Status:** v0.9 — consolidating amendment (§17), **fully stable, zero open reviewer questions in §10:** Q1 discharged by the V4.6 fuzz corpus + Ring 3 evidence (not a ratified dialogue, recorded as a different discharge mechanism); Q3 settled by what Ring 1 actually shipped (whole-frame BLAKE3), its projection sub-question left open; Q5 merged with the inclusive-gateway/parallel-MI Gap 1 into one question after a mid-session correction — a ruling that inclusive gateways had "zero live usage" was itself wrong (the search never checked `bpmn-lite-engine/src/tests.rs`'s six-test `T-IG` suite), caught by a full-workspace test run and reverted before landing, corrected to a shared v3 `FORK-DYN` deferral rather than rejection — then that merged question was itself put to Adam directly and ratified closed same-day: dynamic-arity concurrency is **not required in this V&S**, both defer to v3 as a firm (not conditional) deferral; §8's word count corrected from an approximate "+~14" to the exact "+16"; §9's DSL-lowering input marked SATISFIED; `opened_at`'s status (v0.7/v0.8) confirmed unaffected by §15's instance-granular-refusal ruling. See §17 for the full accounting; the inclusive-gateway episode's process detail lives in the plan doc and V0 report, not repeated here. v0.8 revised v0.7 per §16: landing ruling F required `ConcurrencyRecord::opened_at: Option<Addr>`, a frame-resident artifact address, contradicting §15's own claim that F needed no §4 interaction. Checking that surfaced §4's text was already inaccurate — `handler: Option<Addr>` had carried an artifact address since V1.2 without ever violating a K-invariant. §4 is revised to state the real rule: three categories of `Addr` presence in runtime state (code reference/jump target — legitimate; runtime execution identity — forbidden, FORK/JOIN static pairing is the named exemplar; static-site identity for cross-activation accounting — legitimate, added by this amendment). `handler` is category 1, `opened_at` is category 3; no carve-out was needed once §4 said what it meant. §15's §4-interaction paragraph is corrected in place to match. A second, related correction lands with it: §15's "never in the hash domain" principle is clarified — it governs the counter's ticking, not the terminal value recorded once as a field on a genuine execution object (`Incident::retry_count`), which was already part of Ring 2's canonical encoding before ruling E and is not a contradiction. See §16, which also records this as the third instance of the same pattern (a correct practice, an inaccurate slogan) and the authoring habit adopted to catch it earlier next time. v0.7 revised v0.6 per Adam's ratified rulings E/F (§15): the two implementation gaps §14 deliberately left open are closed. E — the attempt-history channel into `Incident` is `Command::EffectFailed`/`EffectResponse::Failed` gaining an `attempt: u32` field, populated by `bpmn-lite-engine`'s `schedule_transient_effect` (the sole site that already calls `RetryPolicy::decision()`), not by individual worker call sites. F — the repeated-failure escalation counter for `ContractViolation` lives store-side, keyed `(tenant_id, instance_id, guard_addr)`, checked and enforced at claim time (T10.3's existing "should this instance run" gate), not in the kernel frame and not as a merely-advisory event consumer. Both settle a unifying principle: execution state lives in the hashed frame; operational accounting about execution lives beside it, claim-gated and journaled, never in the hash domain. See §15 for the full amendment, including the rejected/superseded options and the §4 interaction analysis. v0.6 revised v0.5 per Adam's ratified ruling D (§14): §13 ruling C's "no distinction between error classes" clause is superseded — automatic rollback is now scoped to `ContractViolation` only, with `BusinessRejection`(unmatched) and exhausted-`Transient` always surfacing as an `Incident` instead. Found during V4.6's blind review of ruling C's actual consequences, then corrected directly by Adam. v0.5 revised v0.4 per Adam's ratified rulings, worked out through direct dialogue while building V4.1's kernel words, on `GUARD-N>` re-arm (Q2), `CANCEL-SCOPE`'s compensation-op reuse (Q4), and automatic rollback on definitive failure inside an interrupting guard (a new question §10 hadn't anticipated). See §13 for that amendment note. v0.4 revised v0.3 per Adam's ratified rulings on three points drafting `EOP-EX-BPMN-ISA-002` (the oracle) surfaced as underdetermined: cancellation order (§4, §12), `JOIN` survivor semantics (§5, §12), and control-stack-delta emission for deleted fibres (§4, §12). See §12 for that amendment note. v0.3 revised v0.2 per adversarial review R1 (verdict: proceed; all points dispositioned in §11); v0.3 adds the static-structure/dynamic-activation law (§4) and converts `JOIN` to handle-based resolution (§5, V-3) after a code/data-separation challenge caught static-id lookup as a taxonomy leak.
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

**Control-stack-delta emission rule (v0.4 addition — see §12):** a `Transition`'s control-stack deltas (pushes/pops) describe **surviving fibres only.** A fibre deleted in the same transition emits no pop deltas for its own stack — `fibers_delete` plus the relevant concurrency-table `Retire`/`Remove` mutations are the complete statement of what happened to it; a stack-pop delta for a fibre that no longer exists is redundant with its deletion and creates a K-2 verification oddity (checking stack↔membership consistency for a fibre that is, by the same transition, no longer live). K-2 (§7) is already scoped to live fibres ("for live F"); this rule makes explicit that delta *emission*, not just invariant-checking, respects that scope.

**Determinism rider (v0.4 revision — see §12 for the amendment history):** cancellation order is **record-nesting order, not fibre-depth.** The innermost concurrency-table *record* retires first (a child record — e.g. a race armed inside a forked branch — before its enclosing parent record, walking the record tree, not any fibre's control-stack depth); within one record's retirement, its member fibres are cancelled in fibre-ID order (context-derived IDs, stable under replay), never map iteration order. This is the total, replay-stable order: the record tree is a strict partial order over concurrency-table records, and the fibre-ID tiebreak totalizes it within a record — fibre-depth is not a safe substitute, because two fibres in structurally unrelated branches can sit at equal control-stack depth while holding differently-nested records, which depth-ordering cannot distinguish and record-tree-order can. Cancelling a fibre unwinds its own control stack innermost-first (same record-nesting order, applied to that one fibre's stack) before removal.

### Static structure vs. dynamic activation (v0.3 addition)

The compiled artifact is the whole taxonomy — every block that *could* execute; a token is one cursor in it. These never co-mingle:

> **Artifact addresses never appear in runtime state; runtime handles never appear in artifacts.**

The control stack does not hold program structure — it holds *activations* of static blocks: fresh concurrency-table records allocated per execution (a `GUARD>` in a loop arms a fresh record each iteration; each `FORK` execution allocates a fresh barrier activation). The relationship is exactly return stack to colon definition: activation records, not definitions. Static nesting is proven over the artifact (V-1/V-2); dynamic extent is instantiated per token and governed by K-1..K-3. Enforced by construction: `Addr` (artifact) and `RecordId` (runtime handle) are distinct types with no conversion; V-9 keeps runtime-consulted semantics out of the envelope; the K-invariants keep category (ii) below — runtime execution identity — out of records.

**Three categories of `Addr` presence in runtime state (v0.8 addition — see §16):** the rule above governs *use*, not mere presence. `ConcurrencyRecord::handler: Option<Addr>` has carried an artifact address since V1.2 (§5, `GUARD>`'s only sound encoding of its handler target) without ever having violated K-1..K-3 — the prohibition was never "no `Addr` in a record," even though earlier drafts of this section read that way. Three categories, only the second forbidden:

(i) **Code reference / jump target** (`handler`). Read only to resolve *where* the handler fibre spawns, never *which activation* it is. No alternative encoding exists — the target is inherently static, and resolving it is execution-affecting by design.

(ii) **Runtime execution identity — FORBIDDEN.** This is what the rule actually guards against. The exemplar is the historical failure this document exists to rule out: resolving a `FORK`/`JOIN` pairing by its static site instead of the inherited `RecordId` handle would make two concurrent activations of one static site indistinguishable, breaking re-entrancy (see V-3, §7 — static pairing annotations are proof material only; runtime resolution is exclusively via the handle for exactly this reason).

(iii) **Static-site identity for accounting that must span activations** (`opened_at: Option<Addr>`, §15 v0.7 ruling F) — a category the original text never contemplated. The property that disqualifies `Addr` for (ii) — a static site does not distinguish activations — is exactly the property that qualifies it here: a repeated-failure budget is deliberately *about* a static guard site's history across re-runs, not about any one activation. `opened_at` is read only to key an out-of-frame store aggregate (`guard_failure_budget`); `apply` never consults it to resolve control flow.

`handler` is category (i); `opened_at` is category (iii); nothing in this document has ever populated category (ii) — the K-invariants (K-1..K-3, §7) are what continue to enforce that, unchanged by this clarification. The lexical failure mode category (ii) forbids is the historical one — a token that *is* a pointer into the model (PlanWalker's `current_node_id`) or structure consulted as state (v1 side tables).

## 5. Decision D2 — Word inventory and perimeter

Stack-effect notation: `( operand-stack-before -- after )` `[ control-stack-before -- after ]`. All words are kernel-pure; effect-emitting words append to `Transition.effects`.

| Word | Effect | Semantics |
|---|---|---|
| `GUARD>` | `( handler-addr -- ) [ -- h ]` | Allocate interrupting-guard record, push handle. Handler addr is a verified code address. |
| `<GUARD` | `( -- ) [ h -- ]` | Pop and retire the guard. Verifier: must match its `GUARD>` on all paths (V-1). |
| `GUARD-N>` / `<GUARD-N` | as above | Non-interrupting guard: on trigger, spawn handler fibre *without* unwinding members; record **re-arms** after trigger (Q2 ratified, §13 amendment v0.5). Distinct opcodes, not a flag — the distinction must be visible to static analysis at the opcode level, exactly as `CALL`/`RET` are distinct instructions (review-ratified). **The one deliberate v1-coverage extension.** |
| `RACE{` | `( n -- ) [ -- h ]` | Open first-wins race record over the next n arms. |
| `ARM-TIMER` | `( duration -- ) [ h ]` | Emit `ScheduleTimer` effect bound to the race; deterministic timer ID per T5. |
| `ARM-MSG` | `( correlation -- ) [ h ]` | Register message alternative. |
| `ARM-EFFECT` | `( effect-desc -- ) [ h ]` | Arm an external-effect alternative. |
| `}RACE` | `( -- ) [ h -- ]` | Park on the race. A command addressed to an armed alternative resolves it: winner continuation runs; other arms' pending effects consumed/cancelled; losing members cancelled in fibre-ID order — one transition. |
| `FORK n` | `( addr1..addrn n -- ) [ s ]` | Allocate a fresh barrier *activation* record; create n fibres at given addresses, each inheriting the parent's control stack **with the barrier handle pushed on top**, and registered as members. Re-entry (a bounded loop containing this FORK) allocates a fresh activation per execution. Fibre IDs context-derived. |
| `JOIN` | `( -- ) [ h -- ]` | Pop the inherited barrier handle; decrement *that activation*; park unless last arrival; last arrival continues and retires it. Resolution is by dynamic handle only — never by static identity. The static FORK/JOIN pairing id exists solely as a verifier annotation for V-3's arity proof. **Survivor semantics (v0.4 addition, see §12):** of the N fibres a `FORK n` created, the fibre whose `JOIN` is the *last arrival* is the sole survivor — it continues execution past the join point with the single post-join continuation. The N−1 non-last arrivals are **deleted at the moment the barrier retires** (not before — each parks at its own `JOIN` until then). This resolves what the barrier's fate alone (§4) left open: which physical fibre ID the join point's continuation belongs to. |
| `WAIT-FOR` / `WAIT-UNTIL` | `( duration\|deadline -- )` | Park + emit timer effect (T5 mechanism as a word). |
| `WAIT-MSG` | `( correlation -- )` | Park on message. |
| `AWAIT-EFFECT` | `( effect-desc -- )` | Emit `DurableEffect::Invoke` + park on completion; effect-ID derivation is the word's, per E5. |
| `CANCEL-SCOPE` | `( -- ) [ h ]` | Explicit cancellation reusing the compensation op (Q4 ratified, §13 amendment v0.5): pop the calling fibre's own handle, restore the scope's rollback snapshot (captured at `GUARD>`/`GUARD-N>` open as standard lifecycle behaviour), unwind nested members innermost-first (record-nesting order per ruling A), run no handler — the cancelling fibre continues in place. Not the earlier BPMN-terminate-only reading (no rollback); cancel-*events* still encode as guards. |

### Deferred to v3 (explicit non-coverage)

- **Compensation.** Reverse-order handler execution over completed activities with its own token semantics. Binding admission requirements on v2: concurrency-table records carry a `kind` discriminant with room for `Compensation`; retirement may *archive* rather than delete (compensation needs completed-scope history); the control stack accepts non-scope record kinds. The table and stack were renamed (from "scope table"/"scope stack") precisely because barriers already break the scope category and compensation will further — the structures are named for their role, not their first tenant.
- **Event subprocesses.** Expressible later as instance-root non-interrupting guards.
- **Multi-instance activities and inclusive (OR) gateways — one deferral, not two (v0.9 addition, see §17).** Parallel MI needs dynamic FORK arity, which breaks V-3's static-arity theorem. Inclusive gateways are the same problem in different notation: a combination-enumeration lowering (each of the compile-time-enumerable legal output combinations as its own static `V2Fork`/`V2Join` pair) would preserve V-3, but the join side still needs dynamic arity in the general case (waiting for however many of the enumerated combination's branches actually complete), which is exactly what parallel MI's deferral already covers. Both defer together to a single v3 `FORK-DYN` design with runtime-checked limits — not two separate work items. `bpmn-lite-engine/src/tests.rs`'s `T-IG-1` through `T-IG-6` (all-branches-taken, single-branch-taken, zero-match with/without default, dynamic-count join, full XML-parse-through-compile) plus `T-AUTH-2` are a real, deliberately-built v1 specification of inclusive-gateway semantics — preserved as-is, untouched by V5 (v1 execution keeps working; V5 only changes what the *v2* lowering path covers, and it never covered inclusive gateways), and adopted as the fixture corpus `FORK-DYN` design work starts from rather than rebuilding from scratch.

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
| Instruction set | +16 words (exact count, v0.9 — §5's table, `GUARD-N>`/`<GUARD-N` and `WAIT-FOR`/`WAIT-UNTIL` each counted as two distinct opcodes, matching `bpmn-lite-types::Instr`'s 16 `V2*` variants); race/join/boundary side tables removed from envelope | `ArtifactAbi` single-value tripwire; no translator |
| Fibre snapshot | + control stack | `SnapshotSchema` single-value tripwire; no upgrade fns |
| Snapshot | + concurrency table (records: id, kind, members, handler, state, counters); persisted as canonical BYTEA per Ring 1 | same tripwire |
| Journal | + events: `ScopeArmed`, `ScopeRetired`, `RaceResolved{winner}`, `ScopeCancelled`, `HandlerSpawned`; + `prior_state_hash`/`new_state_hash` chain (Ring 2) | journal schema single-value tripwire |
| Kernel | race-resolution and boundary-promotion interpretation deleted; word execution + K-theorem discharge added | LOC delta is an exit metric (net negative expected) |
| Verifier | dual-stack interpretation, V-1..V-9 | — |
| Effects/timers/commit protocol | **no change** | — |

## 9. Inputs required before ratification

1. **Fixture corpus:** existing join/race/boundary/timer tests + EOP-EX-BPMN-ISA-002's adversarial workflow (interrupting guard over a parallel subprocess nested inside a race with a message alternative), hand-lowered with full dual-stack traces including the cancellation cascade. Drafted only after D1/D2 ratify, then locked as oracle. **Interpretation note (v0.4 addition, §12):** "a parallel subprocess nested inside a race" is realized as a `V2Fork` (the parallel subprocess) with one branch containing the race, not a race with a fork/join as one of its arms — a race's arms are effect/message/timer *registrations* (V-5: "no unguarded state mutation between arm and park"), and durable parallel work (a `FORK`, which allocates a barrier activation and creates member fibres) is not a registration a race arm can hold; the fork must be the outer structure. `EOP-EX-BPMN-ISA-002` is locked under this reading.
2. **Corruption-injection fixture set:** for each D3 ring, at least one deliberately damaged frame (flipped byte under the hash, chain break, dangling handle, membership asymmetry, over-arity barrier, orphaned pending effect, wrong tripwire version) proving the ring fires with the correct typed `IntegrityError` and quarantines atomically. Committed alongside the golden-bytes corpus.
3. **DSL lowering confirmation — SATISFIED (v0.9, §17, confirmed 2026-07-22).** Every DSL construct that emits an `Instr` today maps onto a D2 word with no residue. The word set in fact has *spare* capacity (`AWAIT-EFFECT`, `RACE{`/`ARM-TIMER` have no DSL emitter yet, since the DSL has no timeout/callout-await AST nodes) — spare capacity is not residue; this input specifically asks whether DSL output lacks a word, not the reverse. Gated V5's entry.

## 10. Questions for reviewers

- ~~Q1~~: **Discharged by evidence, not proof (v0.9, §17).** Closed via property-fuzz + shadow-assert coverage rather than an adversarial-review dialogue like Q2/Q4 — a different discharge mechanism, recorded as such, not silently treated as equivalent.
- ~~Q2~~: **Ratified (§13, amendment v0.5).** `GUARD-N>` re-arms after trigger by default.
- ~~Q3~~: **Settled by what was actually built (v0.9, §17).** Whole-frame BLAKE3 (not per-fibre digests) is what `bpmn-lite-store-postgres` implements and has shipped since V2.2 — the "current lean" is now simply the fact. The projection question (ship in v2 or await demonstrated need) remains genuinely deferred, not ratified either way — no v2 projection has been built or requested.
- ~~Q4~~: **Ratified (§13, amendment v0.5).** `CANCEL-SCOPE` reuses the compensation op: rollback-snapshot restore + no-handler cancellation, not the plain terminate reading. Cancel-*events* still encode as guards.
- ~~Q5~~: **Ratified closed (v0.9, §17, 2026-07-22).** Merged with Gap 1 into one question — do any live EOP runbooks require dynamic-arity concurrency (parallel multi-instance or inclusive gateways) in v2? Adam's ruling: **no, not required in this V&S.** Both defer to v3 `FORK-DYN` as a firm deferral, not a conditional one pending unknown runbook need. `bpmn-lite-engine`'s `T-IG-1..6`/`T-AUTH-2` remain the fixture corpus that work starts from when v3 opens.

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

## 12. Amendment v0.4 (2026-07-21)

**Why this amendment exists:** D1–D4 ratified at v0.3 and unblocked drafting of `EOP-EX-BPMN-ISA-002` (the worked-example oracle) per §11's closing note. Drafting that oracle — specifically, hand-tracing the cancellation cascade for an interrupting guard over a forked region containing a nested armed race — surfaced three points v0.3 left underdetermined. Rather than let the oracle (or, worse, V4's kernel implementation) silently fill these gaps with an un-reviewed convention, they are ratified here as a conscious, versioned amendment. This is the freeze discipline working as intended: frozen means changes are deliberate and reviewed, not that the document is inviolable. Six months from now, "why does cancellation use record nesting?" has an answer that points here.

**A — Cancellation order is record-nesting order, not fibre-depth (§4).** The v0.3 determinism rider said cancellation/loser-unwinding is "fibre-ID order... innermost-first" without specifying what "innermost" ranges over. The oracle's first draft read it as fibre control-stack *depth* (deeper fibre cancels first) — this happened to give the correct answer for that specific scenario, but the primitive is wrong in general: two fibres in structurally unrelated branches can sit at equal control-stack depth while holding differently-nested concurrency-table records, and depth-ordering cannot distinguish them. The corrected primitive is the **record tree**: concurrency-table records form a strict partial order (a record opened inside another record's dynamic extent is its child); cancellation retires the innermost record first, walking that tree, and within one record's retirement its member fibres cancel in fibre-ID order. This is total and replay-stable — the record tree gives a strict partial order, fibre-ID totalizes ties within a record — which fibre-depth is not guaranteed to be. Determinism-critical: this governs the exact byte sequence of every cancellation-cascade transition, which is exactly what gets hashed and replayed.

**B — `JOIN` survivor semantics (§5).** v0.3's `JOIN` row pinned the barrier record's fate ("last arrival continues and retires it") but not which of the `FORK`'s N spawned fibres physically survives. Ratified: the last-arriving fibre is the sole survivor and continues past the join point; the N−1 non-last arrivals are deleted at the moment the barrier retires (each having parked at its own `JOIN` until then, not deleted earlier). This closes a real gap — without it, "last arrival continues" doesn't say which fibre ID owns the post-join continuation, information V4's `apply` needs to construct a well-formed `Transition`.

**C — No control-stack deltas for deleted fibres (§4).** `Transition.control_stack_deltas` (V1's declared surface, V4's sole producer) describes surviving fibres' stack changes only. A fibre deleted in the same transition (per B, or by cancellation per A) is fully described by `fibers_delete` plus the relevant concurrency-table `Retire`/`Remove` mutations; emitting a pop-delta for a fibre that no longer exists is redundant and creates a K-2 verification oddity, since K-2 is already scoped to live fibres. This is an emission-discipline clarification, not a new invariant.

**Scope of this amendment:** all three rulings are D1-level (concurrency-table/control-stack semantics), not new decisions (D5, D6, ...) — they close gaps within D1's existing shape, so they're recorded as amendments to §4/§5 rather than new top-level sections. Nothing here alters D2's word inventory, D3's persistence model, or D4's theorem list beyond what A/B/C's text says. `EOP-EX-BPMN-ISA-002` is updated to match this amendment and locked against v0.4, not v0.3.

---

## 13. Amendment v0.5 (2026-07-21)

**Why this amendment exists:** building V4.1's kernel words surfaced Q2 and Q4 (§10) as live blockers — `GUARD-N>`'s re-arm behaviour and `CANCEL-SCOPE`'s actual semantics had no ratified answer, and `CANCEL-SCOPE`'s ratified answer in turn exposed a third, larger question with no prior §5/§10 entry at all: what happens when an activity *inside* an interrupting guard fails outright. All three are recorded here together since B and C below are direct mechanical consequences of A.

**A — `GUARD-N>` re-arms after trigger (Q2).** On trigger, `GUARD-N>` spawns its handler fibre without unwinding members (as already specified) and the guard record stays `Armed` — it is not retired, so it can trigger again. This is the default; nothing in v2 currently expresses a non-re-arming variant, and none is added here.

**B — `CANCEL-SCOPE` reuses the compensation op, not plain BPMN-terminate (Q4).** The v0.3 working assumption (unwind members, no handler — pure terminate semantics) is superseded. Ratified: every `Guard`-kind scope (`GUARD>`/`GUARD-N>` alike) captures a **rollback snapshot** of the process instance's `domain_payload` at the moment it opens — "a standard lifecycle snapshot," not opt-in, not a separate word. `CANCEL-SCOPE` pops the calling fibre's own handle, restores that snapshot, unwinds nested members innermost-first (ruling A's record-nesting walk, §12), and runs no handler — the cancelling fibre continues in place past the instruction. This is deliberately narrower than `RecordKind::Compensation`'s full "reverse-order handler execution" (§5) — that record kind stays uninhabited by v2; this rollback is a property of `Guard`-kind records specifically, implemented as two new optional fields on `ConcurrencyRecord` (`rollback_domain_payload`, `rollback_domain_payload_hash`), not by instantiating `Compensation`. Cancel-*events* still encode as guards, unchanged from the v0.3 reading.

**C — Automatic rollback on definitive failure, interrupting guards only.** The most consequential piece: inside an **interrupting** `GUARD>` scope (not `GUARD-N>`), a *definitive* job/effect failure — one that has exhausted retry and matches no `error_route_map` entry, i.e. exactly the point v1's `apply_job_failure` would otherwise create an `Incident` — instead performs `CANCEL-SCOPE`'s exact restore-and-unwind operation automatically, with no `CANCEL-SCOPE` instruction required in the program. This replaces v1's error-class taxonomy and incident/routing machinery *for that one boundary, inside an interrupting guard scope only*: no distinction between `ContractViolation`/`BusinessRejection`/exhausted-`Transient`, no error routing, no incident. Explicitly out of scope for this rule, each independently reasoned:
  - **A retriable `Transient` failure is not "a fail" yet** — it can still succeed, so it is retried exactly as today (`JobRetryScheduled`); the rule only fires once a failure is definitive.
  - **A timer/wait resolving is not a failure at all** — it's ordinary forward progress (a race losing an arm, a wait elapsing), handled entirely outside `apply_job_failure`, and this rule has no reach there.
  - **`GUARD-N>` scopes are unaffected** — non-interrupting guards don't unwind on trigger by design (ruling A above), so "roll back on fail" doesn't fit their model; today's v1 incident/routing path is unchanged for fibres whose innermost armed guard is non-interrupting, or who sit inside no guard scope at all.
  - **The triggering fibre is killed, not continued or auto-respawned.** Unlike an in-line `CANCEL-SCOPE` (the executing fibre survives and continues past it), an externally-surfaced job failure has no "next instruction" to fall through to. The instance is left exactly as it was at scope-open — "so it can simply be re-run" — re-running is an external decision against the now-clean state, not something the kernel initiates.

**Scope of this amendment:** A and B are D2 word-semantics clarifications (closing Q2/Q4). C is new — no prior D1–D4 section anticipated an automatic (non-instruction-triggered) concurrency-table mutation, so it is recorded as its own ruling rather than folded into §12's D1 amendments. `ConcurrencyRecord`'s shape gained two new optional fields (B) — this is compatible with V1's frozen record shape in spirit (additive, canonically encoded, `None` for every non-`Guard` kind) but is itself a deliberate, reviewed change to a type V1 called frozen, recorded here for that reason.

---

## 14. Amendment v0.6 (2026-07-22)

**Why this amendment exists:** §13 ruling C, as ratified, explicitly says the automatic-rollback rule applies with "no distinction between `ContractViolation`/`BusinessRejection`/exhausted-`Transient`" — any definitive failure inside an interrupting guard rolls back, full stop. Reviewing that rule's actual consequences (V4.6's blind review, then Adam directly) found this is wrong for two of the three classes, not merely under-implemented: an unmatched `BusinessRejection` is a gap in the *workflow's own route map*, not a machine fault, and rolling back destroys the evidence that a stated business outcome occurred; an exhausted-retry `Transient` is the retry budget's own terminal state and belongs in quarantine with its attempt history intact, not silently erased by a rollback. This supersedes ruling C's "no distinction" clause with **ruling D** below — the underlying mechanics of C (which records retire, how the payload is restored, that the triggering fibre is killed not continued) are unchanged; only which failure classes are eligible for the mechanism changes.

**Ruling D — the failure taxonomy is enumerated, not fall-through.** Of the three classes reaching this boundary (retriable `Transient` and routed `BusinessRejection` are handled earlier and unaffected), exactly one is eligible for §13's automatic-rollback mechanism:

1. **`ContractViolation` — technical fault / corruption.** The only class eligible for automatic rollback, and only when the fibre's innermost armed guard is interrupting (§13's original carve-out for `GUARD-N>` and unguarded fibres is otherwise unchanged). Repeated identical failures of the same scope should escalate rather than loop forever if re-run externally — attempt-counted outside the reverted state, budget reused from `RetryPolicy` (T8.5) rather than a second backoff mechanism. **Not designed here** — see the open items below.
2. **`BusinessRejection` — unmatched route.** Never rolls back, regardless of guard nesting. Always surfaces as an `Incident` — the workflow's route-map gap is information for whoever fixes the workflow, not something the kernel erases.
3. **`Transient`, exhausted (reached this boundary with `retry: None`).** Never rolls back — `RetryDecision::Exhausted` is a terminal state of the existing retry budget, not a fresh failure for the guard-scope machinery to reinterpret. Always surfaces as an `Incident`, with the attempt history that led to exhaustion preserved on it. **Attempt history is not designed here** — see the open items below.

**Meta-rule (E6, "no success-by-default," mirrored onto the failure taxonomy): no failure class reaches rollback by falling through.** The kernel's rollback-eligibility check is an exhaustive match over `ErrorClass` with no wildcard arm — a future fourth `ErrorClass` variant is a compile error at that match site until deliberately classified into one of the three rows above (or a new row), not a silent inheritor of whatever a wildcard would have done. Enforced by the type system, not by convention or code review alone.

**Open, not resolved here — two implementation gaps ruling D exposes but does not close, both requiring Adam's disposition before they're built:**
- **Attempt history has no channel into the kernel today.** `Command::EffectFailed` carries `retry: Option<JobRetry>` (worker id / claim token / backoff hint only, no attempt count), and `Incident::retry_count` has always been hardcoded to `0` regardless of actual attempts — a pre-existing gap, not introduced by this amendment, but one class 3's "attempt history preserved" depends on closing. Candidates: extend `Command::EffectFailed`'s shape with an attempt count (an ABI-relevant change touching every construction site), or look it up store-side from whatever already runs `RetryPolicy::decision()`. Not decided here.
- **No mechanism exists for class 1's repeated-failure escalation.** Nothing survives a guard-scope rollback outside the reverted `domain_payload` — the guard record itself retires as part of the cascade, so even a counter stored on the `ConcurrencyRecord` is lost on the next re-open. An external actor blindly re-running an identically-failing scope has no kernel-provided signal to stop and escalate to quarantine instead. Candidates: a new `ProcessInstance` field outside the rollback snapshot's scope, an external consumer of `V2ScopeCancelled` events keyed by the guard's static `Addr` (survives across re-opens, unlike `RecordId`), or a store-side counter. Not decided here.

**Scope of this amendment:** ruling D revises §13 ruling C's stated scope (the "no distinction" clause), not its mechanics. The two open items are new architectural questions this revision surfaces, deliberately left open rather than guessed at.

---

## 15. Amendment v0.7 (2026-07-22) — RATIFIED

**Why this amendment exists:** §14 closed the failure-taxonomy fall-through but deliberately left two implementation gaps open rather than guess at them: where an attempt count reaches `Incident`, and where a repeated-failure budget for `ContractViolation` lives outside the reverted rollback state. `EOP-PROP-BPMN-ISA-002-A15` worked both into ratifiable decisions with options, a recommendation, and anticipated rebuttals — the same discipline as D1–D4 and the oracle rulings (§12). Ratified as written, with one point (E) resolved by a concrete probe rather than left conditional.

**Unifying principle.** *Execution state lives in the frame — hashed, replayed, integrity-covered. Operational accounting lives beside it — claim-gated and journaled, never in the hash domain.* Attempt counts and repeated-failure budgets both describe a scope's history and health; neither is consulted by `apply` to decide what the machine does next. Putting either in the frame would make every failed attempt mutate hash-covered state, invent a "fields exempt from rollback" concept with no natural boundary, and — for anything keyed by code position — drag artifact addresses into runtime state, in violation of §4.

**Ruling E — attempt-history channel: `EffectResponse::Failed`/`Command::EffectFailed` gain an `attempt: u32` field, populated by the engine adapter, not by individual workers.**

Resolved by probe, not left conditional: `bpmn-lite-types`'s hash-covered frame (`PersistedSnapshotState`, `Transition`) represents pending effects as a bare `pending_effects: BTreeSet<EffectId>` — no per-effect metadata, no `attempt` field anywhere in the Ring-2 hash domain. The only `attempt` column lives store-side, in Postgres's `workflow_effects` table (`bpmn-lite-store-postgres/migrations/050_workflow_effects.sql`), read back into `ClaimedEffect::attempt()`. Since E4 kernel purity already forecloses a store-side lookup from inside `apply`, and the count is not in the frame `apply` receives either, **E-3 (read it from the `Snapshot`) is unavailable.** This resolves the recommendation's stated condition ("verify first, then E-3 if available, else E-2") to **E-2**.

E-2's populating site is concrete, not hypothetical: `bpmn-lite-engine/src/engine.rs`'s `schedule_transient_effect` is the sole call site invoking `RetryPolicy::decision()`; `effect.attempt()` is already in hand there and is currently discarded on `RetryDecision::Exhausted` rather than threaded into `EffectResponse::Failed`, which flows unchanged into `Command::EffectFailed`. One authoritative source, not N worker call sites each capable of drifting or — worse — a crashed-and-redispatched worker reporting attempt 1 on the fiftieth attempt (E-1, rejected on this basis alone). Wiring `Incident::retry_count` from this field also closes the independent, pre-existing defect of it being hardcoded to `0` regardless of actual attempts.

**On "never in the hash domain" and `Incident::retry_count` (v0.8 clarification — see §16):** `Incident` was already part of the canonical encoding before this ruling, so `retry_count`'s populated value does enter Ring 2 — which reads, out of context, as a direct contradiction of the unifying principle above. It is not: the principle governs the *counter*, not any value ever derived from it. The counter ticks store-side, once per attempt (`workflow_effects.attempt`), mutating nothing hash-covered. Its terminal value enters the frame exactly once, as a field on an `Incident` — which is genuine execution state, not accounting: an incident is a durable workflow object that changes what happens next (surfaced to operators, drives routing, is itself subject to resolution). The principle is "accounting doesn't *tick* in the frame," not "no accounting-derived value may ever appear in the frame" — a terminal summary recorded once, as part of a legitimate execution object, is not the thing the principle forbids.

**Ruling F — repeated-failure budget: a store-side counter keyed `(tenant_id, instance_id, guard_addr)`, checked and enforced at claim time.**

F-3, as recommended. `RecordId` does not survive a guard's re-open (the record retires with the rollback cascade); only the guard's static `Addr` does, so any durable key is `(instance, guard Addr)` or coarser. F-1 (a `ProcessInstance` field) is rejected: it places an artifact address inside the frame, which §4 forbids as written, and would require a ratified carve-out for "diagnostic metadata exempt from rollback" — a category that, once it exists, accretes. F-2 (an external `V2ScopeCancelled`-event consumer) is rejected as advisory-only: it can observe a scope's fiftieth failure but cannot refuse the fifty-first, which is the weak form of the guarantee for a fail-stop system. F-3 is chosen because it is enforcing, not observing, and because it extends T10.3's existing claim-time "should this instance run" gate rather than inventing a second gate that can come to disagree with the first — the same category as the lease and the fence, neither of which is in the hash domain either.

**F's scope of refusal, settled explicitly (raised as a wrinkle, not left implicit):** the budget is scoped per-guard (keyed by `Addr`), but T10.3's claim decision is instance-granular. Ratified: **exhausting one guard's budget refuses claims for that instance** (instance-granular refusal, not scope-granular partial admission) — a partial-admission model (ticking fibres outside the exhausted scope while refusing only that scope's re-entry) would require the claim path to reason about which fibres a claim would touch before granting it, which T10.3 does not do today and which this amendment does not propose adding. An instance with one permanently-exhausted guard is, correctly, an instance that needs operator intervention, not partial operation.

**F's sub-questions, ratified as proposed:**
- **Budget shape:** reuses `RetryPolicy`'s *shape* (bounded, deterministic, terminating in escalation) at a distinct granularity and clock (per-scope-re-run, not per-effect-with-backoff) — not the literal `RetryPolicy` type. A second, explicitly distinct policy type, so "attempt 3" is never ambiguous between the two callers.
- **Reset semantics:** the counter resets when the scope closes successfully (`<GUARD`/`<GUARD-N` executes normally). Ratified explicitly per the proposal's own warning: a non-resetting counter is a slow-acting denial of service against long-lived workflows that accumulate unrelated historical failures.
- **Terminal action:** quarantine-and-surface (an `Incident`), consistent with every other exhausted budget in the system — not silent refusal.

**§4 interaction:** E is a `Command`-shape change, not frame state — no interaction. F's counter is store-side, in the same category as the lease/fence/journal — records *about* execution, never consulted *during* execution. **Superseded in part by §16:** landing F surfaced that its key, `(tenant_id, instance_id, guard_addr)`, needs a `guard_addr` that outlives one guard activation, and `RecordId` does not survive a guard's re-open — so `ConcurrencyRecord` needed a new frame-resident field, `opened_at: Option<Addr>`, to carry it. This is frame state, contrary to this paragraph's claim that F needed none. §16 resolves it: not a carve-out (a ratified exception to a rule that otherwise stands), but a correction to §4's own text, which had always permitted exactly this category of `Addr` presence (`handler` predates this ruling) without saying so precisely. See §16 for the full reasoning; `opened_at` is that section's category (iii).

**Scope of this amendment:** closes the two items §14 left open. Unparks task #78. `apply_job_failure` may now be extended to read/write the new channels — not yet done as of this ratification, tracked as follow-on implementation work spanning `bpmn-lite-types` (new `Command`/`EffectResponse` fields), `bpmn-lite-kernel` (`Incident::retry_count` wiring), `bpmn-lite-engine` (populate `attempt` at the one call site), and `bpmn-lite-store-postgres` (new migration for the repeated-failure-budget table, claim-path enforcement). Neither ruling alters §13's rollback mechanics, §14's class eligibility, the D3 integrity rings, or the D4 theorem lists — both concern accounting beside execution, not execution itself.

---

## 16. Amendment v0.8 (2026-07-22)

**Why this amendment exists:** landing ruling F required `ConcurrencyRecord::opened_at: Option<Addr>` — a frame-resident, Ring-2-hashed field carrying an artifact address — which §15's own "§4 interaction" paragraph had said F would not need ("§4 stands unamended"). That was checked, not assumed correct on the strength of the ratification alone: it surfaced that §4's text, "the K-invariants keep artifact identities out of records," was already false the moment it was written — `ConcurrencyRecord::handler: Option<Addr>` has carried an artifact address since V1.2 (§5, `GUARD>`'s handler target) and no K-invariant has ever forbidden it. The document had prohibited something it always did; the actual prohibition, examined honestly, was never about presence.

**What changed:** §4 (Static structure vs. dynamic activation) is revised in place to state the real rule — three categories of `Addr` presence in runtime state, only the second forbidden:

1. **Code reference / jump target** (`handler`) — legitimate, execution-affecting, no alternative encoding exists.
2. **Runtime execution identity — FORBIDDEN.** The worked example is named explicitly for the first time: resolving `FORK`/`JOIN` pairing by static site instead of the inherited `RecordId` handle, which would make concurrent activations of one site indistinguishable and break re-entrancy (V-3, §7). This is what the K-invariants actually guard against.
3. **Static-site identity for accounting that must span activations** (`opened_at`) — a category §4's original text never contemplated, added by this amendment. The same property that disqualifies `Addr` for category 2 — a static site cannot distinguish activations — is what qualifies it for category 3: a repeated-failure budget is deliberately about a site's history *across* re-runs, not about any one activation.

`handler` is retroactively classified as category 1 (no behavior change — it was always sound, only undocumented as such). `opened_at` (§15 v0.7 ruling F) is classified as category 3. §15's "§4 interaction" paragraph is corrected in place to say what actually landed, rather than left standing as a now-false claim next to code that contradicts it.

**A second, related correction lands with this amendment:** §15's unifying principle ("operational accounting… never in the hash domain") reads as contradicted by `Incident::retry_count` — already part of Ring 2's canonical encoding pre-dating this ruling — receiving ruling E's `attempt` value. It is not a contradiction; §15 is amended with one clarifying paragraph (inline, after ruling E's populating-site description) stating the actual boundary: the principle governs the counter's *ticking* (store-side, once per attempt, hash-uncovered), not every value ever derived from it — a terminal summary recorded once as a field on a genuine execution object (`Incident` — durable, routes behavior, is itself resolved) is not what the principle forbids.

**A note on why this is the third such correction, worth recording as practice rather than dropping once fixed:** §4's "artifact addresses never appear in runtime state" was already false via `handler` when first written; "innermost-first" cancellation order was ambiguous between fibre-depth and record-nesting until the oracle forced disambiguation (§3); now §15's "never in the hash domain" reads as contradicted by a field that predates the ruling. Each time the *practice* was right and the *slogan* needed correcting — the principle is stated as a teaching device (memorable, one-line, load-bearing in argument) and then leaned on as a specification, which it is under-specified for. Cheap habit going forward, cited here so it isn't rediscovered at the next principle's expense: when a principle is stated, name one thing it permits that looks like a violation, and one thing it forbids that looks permitted, in the same breath. §4 as revised above now does this (`handler` permitted, FORK/JOIN static-pairing forbidden); §15's addition above does the same for the hash-domain principle.

**Scope of this amendment:** clarifies §4's text, corrects §15's §4-interaction claim to match what ruling F actually required, and adds §15's hash-domain boundary clarification. Does not change any K-invariant's enforcement, any ratified ruling's mechanics, or any code — `opened_at` and `Incident::retry_count` were already landed and tested against the *correct* reading of §4/§15 (both were always sound; only the document's text was the defect) before this amendment made those readings explicit. No carve-out was granted in either case because none was needed once the principles said what they meant.

---

## 17. Amendment v0.9 (2026-07-22) — consolidating; the document goes stable after this

**Why this amendment exists:** a single V5-scoping session surfaced enough small, genuine loose ends — two reviewer questions dischargeable by evidence now on record, one dead lean now a built fact, one closed ruling that had to be reopened and corrected mid-session, exact figures that were previously approximate — that batching them into one consolidating amendment is cheaper than five separate ones, and matches how this document has always preferred to land related findings together (§13, §15). After this, the document is stable: Q5/Gap 1, the one question no repository evidence alone could settle, was put to Adam directly and ratified closed the same day — **§10 now carries zero open reviewer questions.**

**Q1 discharged, by evidence rather than a ratified dialogue.** Unlike Q2/Q4 (settled by Adam ruling on a presented question) or Q3 below (settled by what shipped), Q1 asked whether K-2 survives adversarial scrutiny — answered not by argument but by the 5-topology K-invariant property-fuzz corpus (V4.6 remediation) and Ring 3's unconditional shadow-assert on every `apply` call finding zero violations across the full V4 surface. A different discharge mechanism than Q2/Q4, recorded as such rather than folded into the same "ratified" language, since evidence-of-absence-of-counterexample and a reviewed ruling are not the same epistemic act.

**Q3 settled by what was actually built, not decided freshly.** Whole-frame BLAKE3 (not per-fibre row digests) has been `bpmn-lite-store-postgres`'s Ring 1 mechanism since V2.2 — the "current lean" recorded in §10 was already the shipped fact by the time anyone would go looking for it. The projection sub-question (ship a queryable projection in v2, or await demonstrated need) is left genuinely open — nothing forced an answer, no projection has been built or requested.

**§4/§15's inclusive-gateway and dynamic-arity handling — see the plan doc, not restated here.** The full account of the reject-then-corrected-to-defer episode (`bpmn-lite-engine/src/tests.rs`'s `T-IG-1..6`/`T-AUTH-2` found only via full-workspace test run, the false "zero usage" claim, the revert) lives in `EOP-PLAN-BPMN-ISA-002.md` item 5.2a and `EOP-V0-RECONCILIATION-REPORT.md`'s process-lesson note — this document only carries the ratified outcome, already landed in §5's revised deferred-to-v3 bullet: inclusive gateways and parallel multi-instance are one `FORK-DYN` deferral, not two.

**Q5 and Gap 1, merged and closed (same day, ratified in §10 above).** Both asked whether v2 needs dynamic-arity concurrency now — the one question this amendment couldn't answer from repository evidence alone. Adam's ruling: not required in this V&S. Parallel multi-instance and inclusive gateways both defer to v3 `FORK-DYN` as a firm deferral. **This closes every open reviewer question in §10 — the document has zero open items as of this ruling.**

**§15's hash-domain clarification and §4's three-category framework are v0.8 (§16), not v0.9** — already landed in the prior amendment, referenced here only to confirm nothing further changed them this pass.

**`opened_at`'s status, for the record:** fully landed (§15 v0.7 ruling F, code committed `50e7e7d`), fully tested (three `bpmn-lite-store-postgres` tests: exhaustion→quarantine, explicit-cancel exclusion, reset-on-close), and classified under §16's category (iii). Unaffected by §15's instance-granular-refusal ruling ("exhausting one guard's budget refuses claims for that instance, not just that guard") — that ruling governs the *consequence* of exhaustion, not the *key*; `guard_failure_budget` still counts per `(tenant_id, instance_id, guard_addr)`, so `opened_at` is still exactly what keys it. Nothing about it is open.

**§8 corrected to an exact count.** "+~14 words" is now "+16 words" — `bpmn-lite-types::Instr`'s 16 `V2*` variants, counting `GUARD-N>`/`<GUARD-N` and `WAIT-FOR`/`WAIT-UNTIL` as the distinct opcodes they are (§5's table already listed them this way; only §8's summary was stale).

**§9 input #3 (DSL lowering confirmation) marked SATISFIED,** confirmed during V5 entry-gate scoping: every DSL-emitted construct maps onto a D2 word; the reverse (two words with no DSL emitter yet) is spare capacity, not residue.

**Scope of this amendment:** six small corrections and closures, batched, same day. No K-invariant, ratified ruling, D3 integrity ring, or D4 theorem list changes. No new open question is introduced; Q5/Gap 1 was narrowed from two overlapping questions to one and then ratified closed (not required in this V&S) — **§10 carries zero open reviewer questions as of this amendment.**

---

*Ratification of D1–D4 unblocks EOP-EX-BPMN-ISA-002 (oracle) and EOP-PLAN-BPMN-ISA-002 (tranches V1–V6). Nothing in this document alters the KERNEL-001 durability substrate.*
