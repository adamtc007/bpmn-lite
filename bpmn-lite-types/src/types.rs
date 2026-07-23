use crate::session_stack::SessionStackState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

// ─── Scalar aliases ───────────────────────────────────────────

/// Bytecode address (instruction pointer) — a static artifact coordinate.
///
/// Distinct from `crate::concurrency::RecordId` (a runtime handle) by the
/// D1 static-structure/dynamic-activation law (V&S §4): "Artifact addresses
/// never appear in runtime state; runtime handles never appear in
/// artifacts." `Addr` wraps `u32`, `RecordId` wraps `Uuid`; there is no
/// `From`/`Into` between them, so mixing the two is a compile error. See
/// `bpmn_lite_types::concurrency`'s compile-fail doctest for the proof.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Addr(u32);

impl Addr {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The raw instruction offset, for indexing into the compiled program.
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// The raw `u32` value, e.g. for hashing/debug formatting.
    pub const fn get(self) -> u32 {
        self.0
    }

    pub const fn saturating_add(self, rhs: u32) -> Self {
        Self(self.0.saturating_add(rhs))
    }
}

impl From<u32> for Addr {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<Addr> for u32 {
    fn from(addr: Addr) -> Self {
        addr.0
    }
}

impl std::fmt::Display for Addr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Add<u32> for Addr {
    type Output = Addr;
    fn add(self, rhs: u32) -> Addr {
        Addr(self.0 + rhs)
    }
}

impl std::ops::AddAssign<u32> for Addr {
    fn add_assign(&mut self, rhs: u32) {
        self.0 += rhs;
    }
}

/// Join barrier identifier.
pub type JoinId = u32;

/// Wait point identifier.
pub type WaitId = u32;

/// Interned orch_flag name.
pub type FlagKey = u32;

/// Epoch milliseconds (UTC).
pub type Timestamp = i64;

/// Byte-offset span into the original source text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceSpan {
    /// Inclusive start byte offset.
    pub start: u32,
    /// Exclusive end byte offset.
    pub end: u32,
}

impl SourceSpan {
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }
}

// ─── Value ────────────────────────────────────────────────────

/// A compact value on the orch stack or in flags. Never domain payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    I64(i64),
    /// Interned string id.
    Str(u32),
    /// Opaque handle into external stores.
    Ref(u32),
}

/// One deterministic branch in a DSL payload router.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadRouteBranch {
    /// Placeholder key in `ProcessInstance::placeholder_values`.
    pub placeholder: String,
    /// Canonical scalar spelling required by this branch.
    pub expected_value: String,
    pub target: Addr,
}

// ─── Bytecode instructions ────────────────────────────────────

/// The 18-opcode ISA for the BPMN-Lite VM, plus the v2 D2 instruction set
/// (EOP-BPMN-ISA-002 V2.7, `V2`-prefixed variants below the v1 block).
///
/// V2.7 7.2 addressing-wall proof: every v2 `Instr` addressing field is
/// typed `Addr`, never `bpmn_lite_types::concurrency::RecordId` — the same
/// activation-law wall `Addr` itself documents (V1.1). This is a hard
/// compiler error, not a lint, since `Addr`/`RecordId` have no `From`/
/// `Into` between them.
///
/// ```compile_fail
/// use bpmn_lite_types::concurrency::RecordId;
/// use bpmn_lite_types::types::Instr;
///
/// let _bad = Instr::V2Guard {
///     handler: RecordId::new(uuid::Uuid::nil()),
/// };
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Instr {
    // Control flow
    Jump {
        target: Addr,
    },
    BrIf {
        target: Addr,
    },
    BrIfNot {
        target: Addr,
    },

    // Stack ops
    PushBool(bool),
    PushI64(i64),
    Pop,

    // Flags (read/write ProcessInstance.flags)
    LoadFlag {
        key: FlagKey,
    },
    StoreFlag {
        key: FlagKey,
    },

    // Work (activates job for ob-poc worker)
    ExecNative {
        task_type: u32,
        argc: u16,
        retc: u16,
    },

    /// A task produced by the DSL frontend. Static arguments and output
    /// lineage are embedded in the canonical artifact rather than interpreted
    /// by a second runtime.
    ExecDslTask {
        task_type: u32,
        static_args: BTreeMap<String, String>,
        produces_placeholder: Option<String>,
    },

    /// Route on canonical placeholder state. This replaces PlanWalker domain
    /// string special-cases with data carried by the executable artifact.
    RoutePayload {
        branches: Box<[PayloadRouteBranch]>,
        default_target: Option<Addr>,
    },

    // In-process FFI invocation (A2 dispatch model / A5 lowering target)
    /// Invoke a registered FFI execution owner in-process.
    ///
    /// `template_id` is the 32-byte BLAKE3 digest identifying the published
    /// `FfiTemplate` in the catalogue. The `FfiTaskDecl` stored at this
    /// instruction's bytecode address in `CompiledProgram.ffi_task_decls`
    /// carries the compiled input/output bindings.
    ExecFfi {
        template_id: [u8; 32],
        argc: u16,
        retc: u16,
    },

    /// Publish a message into the engine's message buffer (BPMN Send Task).
    ///
    /// Fire-and-continue. The interned message `name` is paired with the value
    /// in register `corr_reg` (the correlation key) and inserted into the same
    /// buffer that `V2WaitMsg`/`signal_inner` read from. No fiber parking, no
    /// reply expected; the next instruction executes on the same tick.
    PublishMessage {
        name: u32,
        corr_reg: u8,
    },
    /// Cancel a specific pending wait (used by engine after race resolution).
    /// Retained as a kernel no-op (see `apply`'s handler) — no v2 word
    /// supersedes this; no D2 scope covers per-wait cancellation, only
    /// scope-level unwind (`V2CancelScope`/guard retirement).
    CancelWait {
        wait_id: WaitId,
    },

    // Bounded loops
    IncCounter {
        counter_id: u32,
    },
    BrCounterLt {
        counter_id: u32,
        limit: u32,
        target: Addr,
    },

    // Lifecycle
    End,
    EndTerminate,
    Fail {
        code: u32,
    },

    // ─── v2 ISA (EOP-BPMN-ISA-002 V2.7) ────────────────────────
    //
    // D2 word inventory, transcribed from `docs/todo/EOP-VS-BPMN-ISA-002.md`
    // §5 ("the stack effects ARE the spec — transcribe them, do not
    // reinterpret"). Every word below carries a `V2`-prefixed identifier by
    // explicit, confirmed disposition (V2.7 entry amendment): v1's
    // `Fork`/`Join`/`WaitFor`/`WaitUntil`/`WaitMsg`/`WaitAny`/`WaitArm`/
    // `ForkInclusive`/`JoinDynamic` variants looked name- or shape-adjacent
    // to several D2 words but were NOT conformant (v1 `Fork`/`Join` used
    // static `JoinId`+arrival-counting, not D2's dynamic-handle/activation-
    // record model; v1 `WaitArm::Timer` encoded interrupting-vs-not as a
    // `bool` flag, the exact anti-pattern D2's `GUARD>`/`GUARD-N>` rejects
    // by using distinct opcodes; v1 `WaitFor`/`WaitUntil`/`WaitMsg` were
    // bare parks, but D2's words are durable-effect-emitting). Reusing a v1
    // identifier for v2 semantics would have been a dual-path-by-stealth —
    // the same variant meaning two things depending on which tranche's code
    // touches it, so every v2 word got its own `V2`-prefixed identifier
    // instead. **V5.3 (landed 2026-07-23) deleted the entire v1 concurrency
    // block as one unit** — `Fork`/`Join`/`WaitFor`/`WaitUntil`/`WaitMsg`/
    // `WaitAny`/`WaitArm`/`ForkInclusive`/`JoinDynamic`/`RaceId`/
    // `RacePlanEntry` are gone; every real workflow now lowers exclusively
    // to the `V2*` words below (plus the permanently-shared primitives —
    // `Jump`/`BrIf`/`BrIfNot`, stack ops, flags, `ExecNative`/`ExecFfi`/
    // `ExecDslTask`, `RoutePayload`, `PublishMessage`, `CancelWait`,
    // `IncCounter`/`BrCounterLt`, `End`/`EndTerminate`/`Fail` — none of
    // which are D2 concurrency-scope constructs, so none were superseded).
    //
    // Addressing (V2.7 7.2): guard handler extents, race arm resume
    // targets, and the FORK/JOIN static pairing annotation are all
    // `Addr`-space, never `RecordId` (`bpmn_lite_types::concurrency::RecordId`)
    // — proof material for the verifier (V-3's arity check), never runtime
    // execution state. Runtime `V2Join` resolution is exclusively via the
    // dynamically-inherited handle minted by `V2Fork`, "never by static
    // identity" (§5). See the compile-fail doctest below.
    /// `GUARD>` — `( -- ) [ -- h ]`. Allocate an interrupting-guard record,
    /// push its handle onto the control stack. `handler` is a verified code
    /// address (never a `RecordId` — see module-level addressing note).
    V2Guard {
        handler: Addr,
    },
    /// `<GUARD` — `( -- ) [ h -- ]`. Pop and retire the guard. Verifier:
    /// must match its `V2Guard` on every path (V-1).
    V2GuardEnd,
    /// `GUARD-N>` — as `V2Guard`, non-interrupting: on trigger, spawn the
    /// handler fibre without unwinding members. Distinct opcode from
    /// `V2Guard`, not a flag — "the distinction must be visible to static
    /// analysis at the opcode level" (§5, review-ratified).
    V2GuardN {
        handler: Addr,
    },
    /// `<GUARD-N` — retire a non-interrupting guard.
    V2GuardNEnd,

    /// `GUARD-R>` — `( -- ) [ -- h ]`. Allocate an interrupting,
    /// rollback-capable guard record and push its handle. A18 (ratified):
    /// `GUARD>`'s control disposition (unwind members, spawn handler) and
    /// a rollback's data disposition (restore the A3 rollback-set
    /// snapshot on cancellation) are orthogonal — bundling both onto one
    /// opcode was the defect A18 corrects. `GUARD-R>` deliberately carries
    /// NO `handler` field (unlike `V2Guard`/`V2GuardN`): its only closes
    /// are the normal `<GUARD-R` pairing and an unwind triggered by
    /// `V2CancelScope` or automatic rollback-on-definitive-failure
    /// (`apply_job_failure`) — never `V2TriggerGuard`, which requires a
    /// handler address to spawn and rejects a handle that has none.
    /// Verifier V-10 (dominance): `GUARD-R>`'s scope must dominate any
    /// `FORK` within its extent — it may contain a complete `FORK`/`JOIN`
    /// region, but must never itself be opened while already nested
    /// inside one (rolling back shared instance-wide state —
    /// `domain_payload`/`flags`/`join_expected`/session stack — while
    /// sibling fork branches keep running concurrently is exactly the
    /// barrier-starvation hazard V-10 forecloses).
    V2GuardR,
    /// `<GUARD-R` — `( -- ) [ h -- ]`. Pop and retire a `GUARD-R>` record
    /// on its normal (non-rollback) path. Verifier: must match its
    /// `V2GuardR` on every path (V-1); per V-10's companion check, a
    /// `GUARD-R>`-opened handle is also the only legal `V2CancelScope`
    /// target among the three guard-kind opcodes.
    V2GuardREnd,

    /// `GUARD-TIMER>` — `( duration -- )`. §18 v0.10 ruling I: guards carry
    /// an arming trigger, timer as the first kind — a boundary timer is a
    /// guard with a deadline, indifferent to whether the guarded work is
    /// synchronous (`ExecNative`/`ExecFfi`), effect-based
    /// (`AWAIT-EFFECT`), or a nested `FORK` region. Opcode-shape decision
    /// (recorded here, not just in the plan doc, since the choice is a
    /// per-word contract): a NEW, distinct instruction — mirroring how
    /// `V2ArmTimer` arms an already-open `RACE{` record via
    /// `fiber.control_stack.last()` — rather than an optional operand
    /// folded onto `V2Guard`/`V2GuardN`/`V2GuardR` themselves. Rejected
    /// alternative: a conditional/sentinel operand on the guard-open words
    /// would make their stack effect depend on whether a trigger is
    /// present, exactly the "flag on a word, not a distinct opcode"
    /// anti-pattern V&S §5 already rejected for interrupting-vs-not
    /// (`GUARD>` vs `GUARD-N>`) and rollback-vs-not (`GUARD-R>`) — the
    /// guard-open words keep their existing zero-operand encoding
    /// unconditionally, so a guard opened without this word is byte-for-
    /// byte what it was before this ruling landed.
    ///
    /// Must immediately follow the guard-open instruction it arms
    /// (verifier-enforced, `v2_verifier::verify_v2_control_stack`) — "at
    /// guard-open time," not at any later point in the guarded body.
    /// Applies uniformly to all three guard kinds (`V2Guard`/`V2GuardN`/
    /// `V2GuardR`) — nothing about V-10's dominance constraint interacts
    /// with arming; see the kernel's `apply_timer` doc comment for how a
    /// timer-armed `V2GuardR` resolves (automatic rollback, since it
    /// carries no `handler` to trigger). No control-stack or concurrency-
    /// record change (as `V2ArmTimer`); pops `duration` from the operand
    /// stack (V2.7 addressing-review BLOCKING #2.4 rider — a runtime-
    /// computed duration must be representable) and emits
    /// `DurableEffect::ScheduleTimer` (`TimerKind::V2GuardTimer`) bound to
    /// the guard's own `RecordId`, using the same T5 deterministic
    /// timer-ID derivation (`EffectId::for_instruction`) `V2ArmTimer`/
    /// `WAIT-FOR` already use.
    V2GuardArmTimer,

    /// `GUARD-TIMER-CYCLE>` — `( -- )`. Post-close remediation, restoring
    /// V&S §13 amendment v0.5 ruling A ("`GUARD-N>` re-arms after trigger
    /// — the record stays `Armed`... This is the default") to
    /// `GUARD-TIMER>`'s own timer-triggered path specifically: v0.5's
    /// ruling was about the general trigger mechanism
    /// (`Command::V2TriggerGuard`), which already re-arms unconditionally
    /// (nothing to fix there); `GUARD-TIMER>` (ruling I, landed after
    /// v0.5) was fire-once by construction and never inherited the same
    /// default — this word is that fix's bounded-cycle rider, not a new
    /// design. `GUARD-TIMER>`'s own timer fire against a `GUARD-N>`-kind
    /// record now re-arms UNCONDITIONALLY by default (matching ruling A's
    /// "nothing... expresses a non-re-arming variant" for GuardN
    /// specifically) — this word narrows that default from unbounded to a
    /// finite count, mirroring BPMN's own `timeCycle` bounded-repeat syntax
    /// (`R<n>/PT<duration>`). Optional and additive: a `GUARD-TIMER>` with
    /// no following `GUARD-TIMER-CYCLE>` is byte-for-byte what ruling I
    /// already built, now simply re-arming forever for `GUARD-N>` (and
    /// still never re-arming for `GUARD>`/`GUARD-R>`, whose triggers
    /// retire the record on fire, per V-10's dominance/rollback treatment
    /// — closing the record makes any repeat count moot for those two,
    /// which is why the verifier restricts this word to a `GUARD-N>`
    /// target only, not "any guard").
    ///
    /// Must immediately follow the `GUARD-TIMER>` instruction it bounds
    /// (verifier-enforced by instruction shape, `v2_verifier::
    /// verify_v2_control_stack`) — never merely "somewhere inside the
    /// guarded body," same adjacency discipline `GUARD-TIMER>` itself
    /// already applies to its own guard-open. `max_fires` is a static
    /// embedded field (not an operand-stack value), same rationale as
    /// `V2RaceOpen`'s `arm_count`: this is a verify-time-checkable shape
    /// property (the verifier needs it to reject `max_fires == 0` as
    /// meaningless — a cycle that never fires once is not a cycle), not a
    /// runtime-computed quantity the way the duration it rides alongside
    /// is.
    ///
    /// No control-stack or concurrency-record change; touches no `Instr`
    /// operand stack at all — it patches the `DurableEffect::ScheduleTimer`
    /// effect `GUARD-TIMER>` just pushed (guaranteed, by the same
    /// adjacency the verifier enforces, to be the last element of this
    /// tick's own `changes.effects`), attaching a `TimerRepeatSpec` whose
    /// `remaining` field narrows from `GUARD-TIMER>`'s own default
    /// (`u32::MAX`, "unbounded") to `max_fires`. `TimerRepeatSpec`/
    /// `TimerMutation::Rearm` are pre-existing generic timer-scheduling
    /// infrastructure (`bpmn-lite-types::transition`, `ClaimedTimer::
    /// repeat_spec`) — this is the first `V2*` word to populate them; no
    /// new type was needed. **Not ratified D2 vocabulary** — same class as
    /// `V2RouteZeroMatch`/`V2MiIndexLive`: a kernel-level completion of an
    /// already-ratified semantics gap (§18 v0.10's own plan-doc "GUARD-N>
    /// cycle" finding), not new design.
    V2GuardTimerCycle {
        max_fires: u32,
    },

    /// `RACE{` — `( n -- ) [ -- h ]`. Open a first-wins race record over
    /// the next `arm_count` arms. `arm_count` is a static embedded field
    /// (not an operand-stack value) because V-5's race-shape theorem needs
    /// to count `V2Arm*` words statically, mirroring v1 `Fork`'s embedded
    /// arity convention.
    V2RaceOpen {
        arm_count: u16,
    },
    /// `ARM-TIMER` — `( duration -- ) [ h ]`. Arm a timer alternative.
    /// `target` is the static resume address if this arm wins (V2.7 7.2
    /// addressing decision) — verifier-checkable, and genuinely static
    /// (V-5 needs it verify-time-known). `duration`, per §5's literal
    /// notation, is popped from the **operand stack** — deliberately NOT
    /// embedded as a static field (V2.7 addressing-review BLOCKING #2.4:
    /// an embedded `u64` cannot carry a duration computed at runtime from
    /// a BPMN data object, which V5's frontends must be able to lower; a
    /// compile-time-constant duration is simply `PushI64(const); ArmTimer`
    /// — the constant case costs nothing, the dynamic case is now
    /// representable). Do not conflate this with `arm_count`/`pairing`,
    /// which stay static because they are genuinely compile-time-known,
    /// unlike a duration. Execution emits `DurableEffect::ScheduleTimer`
    /// with `TimerKind::Race` (§5: "per T5") — an effect-emitting word,
    /// not a bare park; this is the specific behaviour v1's
    /// `WaitArm::Timer` does not provide as a standalone instruction.
    V2ArmTimer {
        target: Addr,
    },
    /// `ARM-MSG` — `( correlation -- ) [ h ]`. Arm a message alternative.
    /// `target` as above; `corr_reg` names the register holding the
    /// correlation key (mirrors v1 `WaitMsg`'s `corr_reg` convention).
    V2ArmMsg {
        target: Addr,
        name: u32,
        corr_reg: u8,
    },
    /// `ARM-EFFECT` — `( effect-desc -- ) [ h ]`. Arm an external-effect
    /// alternative; emits `DurableEffect::Invoke` on arming. Shape mirrors
    /// `ExecFfi`'s existing FFI-invocation convention (`template_id` +
    /// operand-stack `argc`/`retc`).
    V2ArmEffect {
        target: Addr,
        template_id: [u8; 32],
        argc: u16,
        retc: u16,
    },
    /// `}RACE` — `( -- ) [ h -- ]`. Park on the race. A command addressed
    /// to an armed alternative resolves it: winner continuation runs at
    /// its arm's `target`; other arms' pending effects consumed/cancelled;
    /// losing members cancelled in fibre-ID order — one transition (§5).
    V2RaceClose,

    /// `FORK n` — `( addr1..addrn n -- ) [ s ]`. Allocate a fresh barrier
    /// *activation record*; create fibres at `targets`, each inheriting
    /// the parent's control stack **with the barrier handle pushed on
    /// top**. Re-entry (a bounded loop containing this instruction)
    /// allocates a fresh activation per execution. `pairing` is a static
    /// `Addr`-space identity (this instruction's own address) that the
    /// matching `V2Join`(s) reference — proof material for V-3's arity
    /// check only; runtime resolution is exclusively via the dynamically
    /// inherited handle, never by `pairing` (§5: "never by static
    /// identity").
    V2Fork {
        targets: Box<[Addr]>,
        pairing: Addr,
    },
    /// `JOIN` — `( -- ) [ h -- ]`. Pop the inherited barrier handle;
    /// decrement that activation; park unless last arrival; last arrival
    /// continues and retires it. `pairing` matches the allocating
    /// `V2Fork`'s `pairing` field (verifier-only, see above) — resolution
    /// itself is by dynamic handle only.
    V2Join {
        pairing: Addr,
    },

    /// `WAIT-FOR` — `( duration -- )`. Pops `duration` from the operand
    /// stack (V2.7 addressing-review BLOCKING #2.4 — not an embedded
    /// field; see `V2ArmTimer`'s doc comment for the full rationale: a
    /// static field cannot carry a runtime-computed duration, which V5
    /// must be able to lower a dynamic BPMN timer expression to). Parks +
    /// emits `DurableEffect::ScheduleTimer` (`TimerKind::Wait`) bound to
    /// this instruction's `(instance, fibre, pc)` per T5's deterministic
    /// derivation (`EffectId::for_instruction`). Deliberately distinct
    /// from v1 `WaitFor` (see module-level note) — the difference is
    /// behavioural: this word MUST append the effect, v1's `WaitFor` is a
    /// bare park.
    V2WaitFor,
    /// `WAIT-UNTIL` — as `V2WaitFor`, absolute deadline popped from the
    /// operand stack.
    V2WaitUntil,
    /// `WAIT-MSG` — `( correlation -- )`. Park + register message
    /// correlation. No durable effect (message arrival is external, not
    /// kernel-scheduled) — matches v1 `WaitMsg`'s shape but kept as a
    /// distinct identifier per the coexistence rule.
    V2WaitMsg {
        name: u32,
        corr_reg: u8,
    },
    /// `AWAIT-EFFECT` — `( effect-desc -- )`. Emit
    /// `DurableEffect::Invoke` + park on completion; effect-ID derivation
    /// is the word's responsibility (§5, E5). Shape mirrors
    /// `ExecFfi`/`V2ArmEffect`'s `template_id`+`argc`/`retc` convention.
    V2AwaitEffect {
        template_id: [u8; 32],
        argc: u16,
        retc: u16,
    },

    /// `CANCEL-SCOPE` — `( -- ) [ h ]`. Explicit cancellation (BPMN
    /// *terminate* semantics): unwind members innermost-first, run no
    /// handler. Cancel-events are encoded as guards (§5, "see Q4").
    V2CancelScope,

    // ─── V5 dynamic-arity gateway lowering glue (§18 rulings H/J) ──────
    //
    // Neither word is in the V&S §5 D2 table — they are compiler-internal
    // glue discovered while lowering inclusive gateways onto `V2Fork`'s
    // proven skip-to-`V2Join` pattern (ruling H), not a runtime concept
    // BPMN or the ISA needed named. Both are documented here at the same
    // rigor as a ratified D2 word (V2.7 checklist, `docs/todo/
    // EOP-PLAN-BPMN-ISA-002.md`'s V5 post-close writeup) per CLAUDE.md's
    // "no trap doors" discipline — an unratified opcode is still a real
    // opcode, not exempt from the checklist that governs `GUARD-TIMER>`.
    /// `( -- bool )`. Push `instance.placeholder_matches(placeholder,
    /// expected_value)`. The DSL-side analogue of `LoadFlag` — XML's
    /// inclusive-gateway conditions are already flag-based (`LoadFlag`
    /// reused as-is), but the DSL's inclusive split conditions are
    /// `(placeholder, expected_value)` string-equality checks with no
    /// existing operand-stack-producing primitive (`Instr::ForkPayload`'s
    /// kernel handler evaluates them internally, never exposing a
    /// bytecode-level "load and check" step). Needed so a DSL inclusive
    /// gateway's per-branch skip check (`BrIfNot` after this word, mirroring
    /// XML's `LoadFlag`+`BrIfNot`) and the zero-match precheck ahead of
    /// `V2Fork` (see `V2RouteZeroMatch`) can be expressed as straight-line
    /// bytecode instead of a second fat evaluate-and-branch instruction.
    V2LoadPlaceholderMatch {
        placeholder: String,
        expected_value: String,
    },
    /// `( -- )`. Unconditionally raises an `Incident` (`fail_contract`,
    /// `ErrorClass::ContractViolation`) naming the gateway via `debug_map`
    /// — the same message shape and mechanism `RoutePayload`/`ForkPayload`/
    /// `ForkInclusive`'s existing zero-match arms already use (§18 ruling
    /// J). No successors (V-11 terminal, same class as `Fail`/
    /// `V2CancelScope` — "static dead end, by design," not reached on any
    /// path where a branch matched). Reachable only by falling through a
    /// chain of `LoadFlag`/`V2LoadPlaceholderMatch` + `BrIf`-to-`V2Fork`
    /// checks with none taken — the dual-arity `V2Fork` lowering's
    /// synchronous pre-fork zero-match check (see the V5 plan-doc writeup
    /// for why this runs BEFORE `V2Fork` rather than as a `V2Fork`
    /// operand: `V2Fork` has no condition-evaluation or zero-match
    /// capability of its own, by design — it is a dumb "always spawn N
    /// static targets" instruction, and staying that way is what keeps
    /// K-3/V-3 unchanged per ruling H).
    V2RouteZeroMatch,

    // ─── V5 dynamic-arity multi-instance lowering glue (§18 ruling K) ──
    //
    // Neither word is D2 vocabulary — same class as `V2RouteZeroMatch`/
    // `V2LoadPlaceholderMatch` immediately above: compiler-internal glue
    // discovered lowering parallel multi-instance onto `V2Fork`'s proven
    // skip-to-`V2Join` pattern (ruling H), documented at the same rigor as
    // a ratified D2 word per CLAUDE.md's "no trap doors" discipline. A
    // real, reportable finding drove both: this codebase has NO
    // array/collection-valued `Value`/`DataObjectType`/`BindingSource` at
    // any layer (checked, not assumed — `ffi_bindings.rs`'s `PrimitiveType`
    // is `Bool`/`I64`/`F64`/`String` only). Building one is a materially
    // larger, separate feature (a new `Value::Array`, JSON-path array
    // indexing, a new `BindingSource` shape) than MI's own arity/skip
    // scaffolding needs. Scoped down, deliberately and visibly rather than
    // silently: an MI activity's `inputCollection` is represented here by
    // its RUNTIME LENGTH ONLY (an `I64`-valued flag the workflow populates
    // before the fork runs) — enough to prove/check arity and to decide,
    // per fibre, whether index `i` is live. Per-element VALUE access into
    // the inner activity's own task body is NOT built by these two words
    // and remains out of scope (see the V5 plan-doc writeup's fork note).
    /// `( -- bool )`. Push `true` iff the static `index` is less than the
    /// runtime value of `instance.flags[length_flag]` (missing key or a
    /// non-`I64`/negative value defaults to length 0 — the same "legal
    /// empty collection" default ruling K item (c) ratifies, not a special
    /// case). The MI-side analogue of `LoadFlag`/`V2LoadPlaceholderMatch`:
    /// each of a `V2Fork`'s `n` = declared-max synthesized branch headers
    /// carries its own compile-time-constant `index`, checks liveness with
    /// this word, and `BrIfNot`s straight to the shared `V2Join` if not
    /// live — identical mechanism to a gateway's false-condition skip
    /// (ruling H), applied to an index bound instead of a flag condition.
    V2MiIndexLive {
        length_flag: FlagKey,
        index: u32,
    },
    /// `( -- )`. Runs once, straight-line, immediately before `V2Fork` (the
    /// same "before, not inside, not after" placement `V2RouteZeroMatch`
    /// uses and for the identical reason — `V2Fork` commits to spawning
    /// `targets.len()` fibres unconditionally the moment it executes, so
    /// there is no "spawn fewer" outcome to recover from afterward). If the
    /// runtime length of `instance.flags[length_flag]` exceeds the
    /// artifact-declared `max` (ruling K delta (a): a required static
    /// ceiling, since Zeebe has none), returns
    /// `TransitionError::ResourceLimitExceeded` — a hard, non-resumable
    /// reject, the SAME shape `validate_snapshot_limits` already uses for
    /// fiber-count/stack/register overflow, not the `Incident`/`fail_contract`
    /// route `V2RouteZeroMatch` uses. Deliberately NOT unified with
    /// zero-match's Incident mechanism: exceeding a declared capacity is an
    /// artifact/input-data invariant violation (reject outright, per
    /// CLAUDE.md's "fail closed; reject, don't skip"), not a routing gap a
    /// human can resolve by amending payload and calling `ResolveIncident`
    /// — there is no sense in which "wait, then re-decide" applies to
    /// "this collection is bigger than the artifact declared it could be."
    V2MiArityCheck {
        length_flag: FlagKey,
        max: u32,
    },
}

/// One armed alternative of a v2 race, captured at runtime by
/// `V2ArmTimer`/`V2ArmMsg` as they execute (V&S §5 — a v2-bearing artifact
/// carries no static `race_plan` side table, V-9's structural check
/// forbids it; every alternative's resolution data must live in runtime
/// state instead). `target` is each arm's own winning-resume address.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum V2RaceArm {
    Timer { target: Addr },
    Msg { target: Addr, name: u32, corr_reg: u8 },
    /// Captured by `V2ArmEffect` — the effect's own `effect_id` (derived at
    /// arm time from `(instance, fiber, pc)`, same as `V2AwaitEffect`) is
    /// the resolution key; `apply_ffi_completion` matches it against this
    /// arm the same way message delivery matches `Msg`'s `name`/`corr_reg`.
    Effect {
        target: Addr,
        effect_id: crate::EffectId,
        template_id: [u8; 32],
    },
}

// ─── Fiber ────────────────────────────────────────────────────

/// Fiber wait state — what the fiber is blocked on.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum WaitState {
    Running,
    Timer {
        deadline_ms: u64,
    },
    Msg {
        wait_id: WaitId,
        name: u32,
        corr_key: Value,
    },
    /// Parked waiting for ob-poc worker completion (NEW in v0.9).
    Job {
        job_key: String,
    },
    Effect {
        effect_id: crate::EffectId,
    },
    Join {
        join_id: JoinId,
    },
    Incident {
        incident_id: Uuid,
    },
    /// Parked at a v2 `JOIN` (non-last arrival), waiting for the barrier
    /// record to retire (V&S v0.4 §5/§12 ruling B). `record_id` is the
    /// dynamic handle — v2 barriers are `RecordId`-identified, not the v1
    /// static `JoinId`, so this cannot reuse `WaitState::Join` (V2.7's
    /// coexistence rule: distinct identifier, never reuse a v1-shaped
    /// variant for v2 semantics even where it looks close).
    V2Barrier {
        record_id: crate::concurrency::RecordId,
    },
    /// Parked at a v2 `}RACE` — waiting for the first of `arms` to
    /// resolve (a message delivery matching an armed `V2ArmMsg`, or the
    /// armed `V2ArmTimer`'s durable timer firing). `record_id` is the
    /// `RecordKind::Race` handle on the fiber's own control stack (V4.1
    /// design note, Adam-ratified: race-arm data lives here, on the
    /// fiber's `WaitState`, not on the shared `ConcurrencyRecord` — arms
    /// are single-fiber-owned per V-5, so nothing else ever needs to see
    /// them, and this mirrors v1 `WaitState::Race`'s own inline-data
    /// shape exactly rather than forcing a race-only payload onto the
    /// already-frozen `ConcurrencyRecord`).
    V2Race {
        record_id: crate::concurrency::RecordId,
        arms: Vec<V2RaceArm>,
    },
}

/// A fiber is a lightweight execution thread within a process instance.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fiber {
    pub fiber_id: Uuid,
    pub pc: Addr,
    pub stack: Vec<Value>,
    pub regs: [Value; 8],
    pub wait: WaitState,
    /// Monotonic counter incremented by IncCounter. Used in job_key derivation.
    pub loop_epoch: u32,
    /// D1 control stack: ordered handles into the snapshot's concurrency
    /// table for the scopes this fibre is currently inside (V&S §2, §4).
    /// Populated by V4's words; empty for every v2-pre-V4 fibre.
    pub control_stack: Vec<crate::concurrency::Handle>,
}

impl Fiber {
    pub fn new(fiber_id: Uuid, pc: impl Into<Addr>) -> Self {
        Self {
            fiber_id,
            pc: pc.into(),
            stack: Vec::new(),
            regs: std::array::from_fn(|_| Value::Bool(false)),
            wait: WaitState::Running,
            loop_epoch: 0,
            control_stack: Vec::new(),
        }
    }
}

// ─── Process instance ─────────────────────────────────────────

/// Top-level process state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ProcessState {
    Running,
    Completed {
        at: Timestamp,
    },
    Cancelled {
        reason: String,
        at: Timestamp,
    },
    Terminated {
        at: Timestamp,
    },
    /// Genuinely, permanently dead — no `Incident` record exists to resolve
    /// and no command revives this instance. Distinct from `Incidented`
    /// (see below); do not conflate the two — that conflation is exactly
    /// the defect this variant split fixes.
    Failed {
        incident_id: Uuid,
    },
    /// Parked on an open `Incident`, waiting for a human/operator to call
    /// `Command::ResolveIncident`. Not terminal — `ResolveIncident`
    /// demonstrably revives this back to `Running`
    /// (`bpmn-lite-kernel/src/lib.rs`). Also not schedulable — the
    /// scheduler must not tick an instance parked on an incident.
    Incidented {
        incident_id: Uuid,
    },
    WaitingOnSubmission {
        callout_id: uuid::Uuid,
        node_id: String,
    },
    WaitingOnInvocation {
        execution_id: uuid::Uuid,
        node_id: String,
    },
}

impl ProcessState {
    /// Returns true if the process is in a terminal state (no further
    /// progress possible). Written as an explicit exhaustive match (no
    /// wildcard arm, no `matches!` macro — `matches!` desugars to an
    /// implicit `_ => false`, which would silently absorb a future
    /// variant instead of forcing it to be classified here) so that
    /// adding a future `ProcessState` variant is a compile error until
    /// this predicate deliberately answers for it. Companion predicate:
    /// `is_schedulable()`, below — see V&S §14's meta-rule (no variant
    /// reaches either question by falling through a wildcard).
    pub fn is_terminal(&self) -> bool {
        match self {
            ProcessState::Running => false,
            ProcessState::Completed { .. } => true,
            ProcessState::Cancelled { .. } => true,
            ProcessState::Terminated { .. } => true,
            ProcessState::Failed { .. } => true,
            ProcessState::Incidented { .. } => false,
            ProcessState::WaitingOnSubmission { .. } => false,
            ProcessState::WaitingOnInvocation { .. } => false,
        }
    }

    /// Returns true if the scheduler should attempt to tick this instance.
    /// Explicit exhaustive match, no wildcard — same reasoning as
    /// `is_terminal()`. `true` for exactly one variant: `Running`.
    /// `Completed`/`Cancelled`/`Terminated`/`Failed`/`Incidented` are all
    /// `false` because none of them make forward progress via ordinary
    /// scheduler ticking; `WaitingOnSubmission`/`WaitingOnInvocation` are
    /// `false` because they resolve exclusively via the external
    /// `Command::StartChildResult` signal, never via ticking.
    pub fn is_schedulable(&self) -> bool {
        match self {
            ProcessState::Running => true,
            ProcessState::Completed { .. }
            | ProcessState::Cancelled { .. }
            | ProcessState::Terminated { .. }
            | ProcessState::Failed { .. }
            | ProcessState::Incidented { .. }
            | ProcessState::WaitingOnSubmission { .. }
            | ProcessState::WaitingOnInvocation { .. } => false,
        }
    }
}

/// A single process instance — the top-level execution context.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessInstance {
    pub instance_id: Uuid,
    pub tenant_id: String,
    pub process_key: String,
    pub bytecode_version: [u8; 32],
    /// Opaque canonical JSON — never parsed by the VM.
    pub domain_payload: Arc<str>,
    /// BLAKE3 of domain_payload.
    pub domain_payload_hash: [u8; 32],
    /// Canonical session stack copied by value from ob-poc when BPMN starts.
    pub session_stack: SessionStackState,
    /// Orchestration flags — flat primitives for branching.
    pub flags: BTreeMap<FlagKey, Value>,
    /// Bounded loop counters — separate from orchestration flags.
    pub counters: BTreeMap<u32, u32>,
    /// Dynamic join expected counts — written by ForkInclusive, read by JoinDynamic.
    pub join_expected: BTreeMap<JoinId, u16>,
    pub state: ProcessState,
    /// ob-poc runbook_entry_id for correlation.
    pub correlation_id: String,
    /// Originating ob-poc runbook entry executing this BPMN-routed verb.
    pub entry_id: Uuid,
    /// Originating ob-poc runbook containing the parked entry.
    pub runbook_id: Uuid,
    pub created_at: Timestamp,
    /// A19 — BLAKE3 hash of immutable fields set at creation.
    /// None for instances created before A19 migration; those are
    /// treated as "not yet hashed" and skipped at verification until
    /// their next save_instance call populates the field.
    pub integrity_hash: Option<[u8; 32]>,
    /// A19 — Quarantine marker. None for normal instances; set to
    /// 'integrity_violation' when a pickup boundary detects hash mismatch.
    /// Quarantined instances are skipped by the scheduler and all handlers.
    pub quarantine_state: Option<String>,
    /// T3 — plan-based execution: hash of the stored WorkflowExecutionPlan.
    /// None for bytecode-path instances.
    #[serde(default)]
    pub plan_hash: Option<[u8; 32]>,
    /// T3 — current node id in the WorkflowExecutionPlan.
    #[serde(default)]
    pub current_node_id: Option<String>,
    /// T3 — placeholder values produced by executed callout nodes.
    #[serde(default)]
    pub placeholder_values: Option<serde_json::Value>,
}

impl ProcessInstance {
    /// Bind a DSL task output from the canonical domain payload. The accepted
    /// key spellings are explicit frontend aliases; absence or malformed JSON
    /// is an error and never becomes `null` or a default value.
    pub fn bind_placeholder_from_payload(&mut self, placeholder: &str) -> Result<(), &'static str> {
        let payload: serde_json::Value =
            serde_json::from_str(&self.domain_payload).map_err(|_| "domain payload is not JSON")?;
        let object = payload
            .as_object()
            .ok_or("domain payload is not a JSON object")?;
        let plain = placeholder.trim_start_matches('@');
        let snake = plain.replace('-', "_");
        let value = object
            .get(placeholder)
            .or_else(|| object.get(plain))
            .or_else(|| object.get(&snake))
            .cloned()
            .ok_or("declared DSL output is absent from domain payload")?;
        let placeholders = self
            .placeholder_values
            .get_or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        let placeholders = placeholders
            .as_object_mut()
            .ok_or("placeholder state is not a JSON object")?;
        placeholders.insert(placeholder.to_string(), value);
        Ok(())
    }

    pub fn placeholder_matches(&self, placeholder: &str, expected: &str) -> bool {
        self.placeholder_values
            .as_ref()
            .and_then(|values| values.get(placeholder))
            .is_some_and(|value| canonical_json_scalar(value) == Some(expected))
    }
}

fn canonical_json_scalar(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::String(value) => Some(value.as_str()),
        serde_json::Value::Bool(true) => Some("true"),
        serde_json::Value::Bool(false) => Some("false"),
        _ => None,
    }
}

// ─── Job activation/completion (the wire types) ───────────────

/// Delivered to ob-poc worker when EXEC_NATIVE fires.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobActivation {
    pub job_key: String,
    pub tenant_id: String,
    pub process_instance_id: Uuid,
    pub task_type: String,
    pub service_task_id: String,
    pub domain_payload: String,
    pub domain_payload_hash: [u8; 32],
    pub session_stack: SessionStackState,
    pub orch_flags: BTreeMap<String, Value>,
    pub retries_remaining: u32,
    pub entry_id: Uuid,
    pub runbook_id: Uuid,
    pub worker_id: String,
    pub claim_token: String,
    pub claim_expires_at: Option<Timestamp>,
    pub attempt_count: u32,
    pub failure_count: u32,
    pub not_before: Option<Timestamp>,
}

/// Returned by ob-poc worker after verb execution.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JobCompletion {
    pub job_key: String,
    pub domain_payload: String,
    /// Optimistic concurrency guard: hash of the instance payload snapshot the worker read.
    /// This is not the hash of `domain_payload`; the engine recomputes the new canonical hash
    /// from the returned payload before persistence.
    pub expected_instance_payload_hash: [u8; 32],
    pub orch_flags: BTreeMap<String, Value>,
}

/// Returned by ob-poc worker on failure.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobFailure {
    pub job_key: String,
    pub error_class: ErrorClass,
    pub message: String,
    pub retry_hint_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ErrorClass {
    Transient,
    ContractViolation,
    BusinessRejection { rejection_code: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum BufferMessageResult {
    Inserted,
    Duplicate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BufferedMessage {
    pub tenant_id: String,
    pub message_name: String,
    pub correlation_key: String,
    pub msg_id: String,
    pub payload: Vec<u8>,
    pub payload_hash: Option<[u8; 32]>,
    pub process_instance_id: Option<Uuid>,
    pub received_at: Timestamp,
    pub expires_at: Timestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimedBufferedMessage {
    pub message: BufferedMessage,
    pub claim_token: String,
    pub claim_until: Timestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PayloadUpdate {
    pub payload: String,
    pub payload_hash: [u8; 32],
}

// ─── Compiler artifacts ───────────────────────────────────────

/// The output of the compiler pipeline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompiledProgram {
    /// BLAKE3 of the serialized program — version key.
    pub(crate) bytecode_version: [u8; 32],
    pub(crate) program: Vec<Instr>,
    /// Bytecode address → BPMN element id (for diagnostics).
    pub(crate) debug_map: BTreeMap<Addr, String>,
    pub(crate) join_plan: BTreeMap<JoinId, JoinPlanEntry>,
    pub(crate) wait_plan: BTreeMap<WaitId, WaitPlanEntry>,
    pub(crate) message_name_map: BTreeMap<u32, String>,
    /// task_type → set of flags it may write.
    pub(crate) write_set: BTreeMap<String, HashSet<FlagKey>>,
    /// All task_type references in the program.
    pub(crate) task_manifest: Vec<String>,
    /// ExecNative bytecode addr → ordered error routes (specific codes first, catch-all last).
    pub(crate) error_route_map: BTreeMap<Addr, Vec<ErrorRoute>>,
    /// Compile-time flag name → FlagKey mapping. Inverted from the lowering intern table;
    /// preserved so the FFI binding layer can resolve symbolic variable names to storage keys.
    pub(crate) flag_symbol_table: BTreeMap<FlagKey, String>,
    /// Resolved data-object declarations keyed by data-object id attribute.
    /// Populated by the A5 lowering pass; empty for processes with no
    /// `<bpmn:dataObject>` declarations.
    pub(crate) data_objects: BTreeMap<String, crate::ffi_bindings::DataObjectDecl>,
    /// Compiled FFI task declarations indexed by the bytecode address of
    /// the corresponding `Instr::ExecFfi` instruction.
    /// Populated by the A5 lowering pass; empty for processes with no
    /// `<bpmn:taskDefinition implementation="...">` annotations.
    pub(crate) ffi_task_decls: BTreeMap<Addr, crate::ffi_bindings::FfiTaskDecl>,
    /// V4 D2 — compiled FFI task declarations for `V2ArmEffect`/`V2AwaitEffect`,
    /// indexed by the bytecode address of the corresponding v2 instruction.
    /// Kept as a separate table from `ffi_task_decls` because V-9 requires
    /// every `ffi_task_decls` address to be `Instr::ExecFfi` — mixing v1/v2
    /// addresses into one table would violate that pairing check. Not part
    /// of `LegacyProgramParts`/`legacy_program!`: defaulted to empty in
    /// `from_legacy_parts` so none of the existing macro call sites break;
    /// set via `with_v2_ffi_task_decls` for programs that need it.
    pub(crate) v2_ffi_task_decls: BTreeMap<Addr, crate::ffi_bindings::FfiTaskDecl>,
}

/// Field-less compatibility tuple used only while pre-T7 callers migrate.
/// It cannot become executable without admission by the envelope verifier.
#[doc(hidden)]
pub type LegacyProgramParts = (
    [u8; 32],
    Vec<Instr>,
    BTreeMap<Addr, String>,
    BTreeMap<JoinId, JoinPlanEntry>,
    BTreeMap<WaitId, WaitPlanEntry>,
    BTreeMap<u32, String>,
    BTreeMap<String, HashSet<FlagKey>>,
    Vec<String>,
    BTreeMap<Addr, Vec<ErrorRoute>>,
    BTreeMap<FlagKey, String>,
    BTreeMap<String, crate::ffi_bindings::DataObjectDecl>,
    BTreeMap<Addr, crate::ffi_bindings::FfiTaskDecl>,
);

impl CompiledProgram {
    #[doc(hidden)]
    pub fn from_legacy_parts(parts: LegacyProgramParts) -> Self {
        let (
            bytecode_version,
            program,
            debug_map,
            join_plan,
            wait_plan,
            message_name_map,
            write_set,
            task_manifest,
            error_route_map,
            flag_symbol_table,
            data_objects,
            ffi_task_decls,
        ) = parts;
        Self {
            bytecode_version,
            program,
            debug_map,
            join_plan,
            wait_plan,
            message_name_map,
            write_set,
            task_manifest,
            error_route_map,
            flag_symbol_table,
            data_objects,
            ffi_task_decls,
            v2_ffi_task_decls: BTreeMap::new(),
        }
    }

    /// Set the v2 FFI-effect binding table (`V2ArmEffect`/`V2AwaitEffect`).
    /// Not part of `LegacyProgramParts` — see field doc on `v2_ffi_task_decls`.
    #[doc(hidden)]
    pub fn with_v2_ffi_task_decls(
        mut self,
        decls: BTreeMap<Addr, crate::ffi_bindings::FfiTaskDecl>,
    ) -> Self {
        self.v2_ffi_task_decls = decls;
        self
    }

    pub fn v2_ffi_task_decls(&self) -> &BTreeMap<Addr, crate::ffi_bindings::FfiTaskDecl> {
        &self.v2_ffi_task_decls
    }

    pub fn bytecode_version(&self) -> [u8; 32] {
        self.bytecode_version
    }
    pub fn program(&self) -> &Vec<Instr> {
        &self.program
    }
    #[doc(hidden)]
    pub fn program_mut(&mut self) -> &mut Vec<Instr> {
        &mut self.program
    }
    pub fn debug_map(&self) -> &BTreeMap<Addr, String> {
        &self.debug_map
    }
    pub fn join_plan(&self) -> &BTreeMap<JoinId, JoinPlanEntry> {
        &self.join_plan
    }
    pub fn wait_plan(&self) -> &BTreeMap<WaitId, WaitPlanEntry> {
        &self.wait_plan
    }
    pub fn message_name_map(&self) -> &BTreeMap<u32, String> {
        &self.message_name_map
    }
    pub fn write_set(&self) -> &BTreeMap<String, HashSet<FlagKey>> {
        &self.write_set
    }
    pub fn task_manifest(&self) -> &Vec<String> {
        &self.task_manifest
    }
    pub fn error_route_map(&self) -> &BTreeMap<Addr, Vec<ErrorRoute>> {
        &self.error_route_map
    }
    pub fn flag_symbol_table(&self) -> &BTreeMap<FlagKey, String> {
        &self.flag_symbol_table
    }
    pub fn data_objects(&self) -> &BTreeMap<String, crate::ffi_bindings::DataObjectDecl> {
        &self.data_objects
    }
    pub fn ffi_task_decls(&self) -> &BTreeMap<Addr, crate::ffi_bindings::FfiTaskDecl> {
        &self.ffi_task_decls
    }
}

#[macro_export]
macro_rules! legacy_program {
    (
        bytecode_version: $bytecode_version:expr,
        program: $program:expr,
        debug_map: $debug_map:expr,
        join_plan: $join_plan:expr,
        wait_plan: $wait_plan:expr,
        message_name_map: $message_name_map:expr,
        write_set: $write_set:expr,
        task_manifest: $task_manifest:expr,
        error_route_map: $error_route_map:expr,
        flag_symbol_table: $flag_symbol_table:expr,
        data_objects: $data_objects:expr,
        ffi_task_decls: $ffi_task_decls:expr $(,)?
    ) => {
        $crate::CompiledProgram::from_legacy_parts((
            $bytecode_version,
            $program,
            $debug_map,
            $join_plan,
            $wait_plan,
            $message_name_map,
            $write_set,
            $task_manifest,
            $error_route_map,
            $flag_symbol_table,
            $data_objects,
            $ffi_task_decls,
        ))
    };
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinPlanEntry {
    pub expected: u16,
    pub next: Addr,
    pub reg_template: [Value; 8],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum WaitType {
    Timer,
    Msg,
    Human,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WaitPlanEntry {
    pub wait_type: WaitType,
    pub name: Option<u32>,
    pub corr_source: Option<u8>,
}

// ─── Incidents ────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Incident {
    pub incident_id: Uuid,
    pub process_instance_id: Uuid,
    pub fiber_id: Uuid,
    pub service_task_id: String,
    pub bytecode_addr: Addr,
    pub error_class: ErrorClass,
    pub message: String,
    pub retry_count: u32,
    pub created_at: Timestamp,
    pub resolved_at: Option<Timestamp>,
    pub resolution: Option<String>,
}

// ─── Error routing ────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorRoute {
    pub error_code: Option<String>,
    pub resume_at: Addr,
    pub boundary_element_id: String,
}

#[cfg(test)]
mod process_state_predicate_tests {
    use super::*;

    fn representative_instances() -> Vec<(&'static str, ProcessState, bool, bool)> {
        // (label, state, expected is_terminal, expected is_schedulable)
        let uid = Uuid::from_u128(1);
        vec![
            ("Running", ProcessState::Running, false, true),
            (
                "Completed",
                ProcessState::Completed { at: 0 },
                true,
                false,
            ),
            (
                "Cancelled",
                ProcessState::Cancelled {
                    reason: "r".to_string(),
                    at: 0,
                },
                true,
                false,
            ),
            (
                "Terminated",
                ProcessState::Terminated { at: 0 },
                true,
                false,
            ),
            (
                "Failed",
                ProcessState::Failed { incident_id: uid },
                true,
                false,
            ),
            (
                "Incidented",
                ProcessState::Incidented { incident_id: uid },
                false,
                false,
            ),
            (
                "WaitingOnSubmission",
                ProcessState::WaitingOnSubmission {
                    callout_id: uid,
                    node_id: "n".to_string(),
                },
                false,
                false,
            ),
            (
                "WaitingOnInvocation",
                ProcessState::WaitingOnInvocation {
                    execution_id: uid,
                    node_id: "n".to_string(),
                },
                false,
                false,
            ),
        ]
    }

    #[test]
    fn is_terminal_and_is_schedulable_classify_every_variant_as_expected() {
        for (label, state, expected_terminal, expected_schedulable) in representative_instances()
        {
            assert_eq!(
                state.is_terminal(),
                expected_terminal,
                "{label}: is_terminal() mismatch"
            );
            assert_eq!(
                state.is_schedulable(),
                expected_schedulable,
                "{label}: is_schedulable() mismatch"
            );
        }
    }

    #[test]
    fn is_schedulable_is_true_only_for_running() {
        let schedulable: Vec<&str> = representative_instances()
            .into_iter()
            .filter(|(_, _, _, sched)| *sched)
            .map(|(label, ..)| label)
            .collect();
        assert_eq!(schedulable, vec!["Running"]);
    }

    #[test]
    fn incidented_is_neither_terminal_nor_schedulable() {
        let state = ProcessState::Incidented {
            incident_id: Uuid::from_u128(42),
        };
        assert!(!state.is_terminal(), "Incidented must not be terminal — ResolveIncident revives it");
        assert!(!state.is_schedulable(), "Incidented must not be schedulable — it awaits ResolveIncident, not ticking");
    }
}
