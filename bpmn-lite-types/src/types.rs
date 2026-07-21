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

/// Race group identifier (compile-time constant, same width as WaitId).
pub type RaceId = u32;

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

// ─── Cycle spec (non-interrupting timer repetition) ───────────

/// Describes a repeating timer cycle (ISO 8601 `R<n>/PT<duration>`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CycleSpec {
    /// Interval between fires in milliseconds.
    pub interval_ms: u64,
    /// Maximum number of fires (0 = unlimited, but we cap at a sane default).
    pub max_fires: u32,
}

// ─── Wait arms (race semantics) ───────────────────────────────

/// Compile-time description of one arm in a WaitAny race.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum WaitArm {
    /// Wall-clock timer (duration from now).
    Timer {
        duration_ms: u64,
        resume_at: Addr,
        /// If false, firing does NOT resolve the race (fork-on-fire).
        interrupting: bool,
        /// If Some, timer re-registers after each fire up to max_fires.
        cycle: Option<CycleSpec>,
    },
    /// Wall-clock timer (absolute deadline).
    Deadline { deadline_ms: u64, resume_at: Addr },
    /// External message with correlation.
    Msg {
        name: u32,
        corr_reg: u8,
        resume_at: Addr,
    },
    /// Internal engine signal (e.g., job completion for boundary events — Phase 2).
    Internal {
        kind: u32,
        key_reg: u8,
        resume_at: Addr,
    },
}

impl WaitArm {
    pub fn resume_at(&self) -> Addr {
        match self {
            WaitArm::Timer { resume_at, .. }
            | WaitArm::Deadline { resume_at, .. }
            | WaitArm::Msg { resume_at, .. }
            | WaitArm::Internal { resume_at, .. } => *resume_at,
        }
    }
}

// ─── Inclusive gateway branch descriptor ──────────────────────

/// One branch of an inclusive (OR) gateway fork.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InclusiveBranch {
    /// Flag to evaluate. `None` = unconditional (always taken).
    pub condition_flag: Option<FlagKey>,
    /// Bytecode address to spawn fiber at if condition is truthy.
    pub target: Addr,
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

    /// Inclusive DSL split driven by placeholder state. Every matching branch
    /// becomes a real fiber and the selected cardinality feeds `JoinDynamic`.
    ForkPayload {
        branches: Box<[PayloadRouteBranch]>,
        join_id: JoinId,
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

    // Concurrency
    Fork {
        targets: Box<[Addr]>,
    },
    Join {
        id: JoinId,
        expected: u16,
        next: Addr,
    },

    // Waits
    WaitFor {
        ms: u64,
    },
    WaitUntil {
        deadline_ms: u64,
    },
    WaitMsg {
        wait_id: WaitId,
        name: u32,
        corr_reg: u8,
    },

    /// Publish a message into the engine's message buffer (BPMN Send Task).
    ///
    /// Fire-and-continue. The interned message `name` is paired with the value
    /// in register `corr_reg` (the correlation key) and inserted into the same
    /// buffer that `WaitMsg`/`signal_inner` read from. No fiber parking, no
    /// reply expected; the next instruction executes on the same tick.
    PublishMessage {
        name: u32,
        corr_reg: u8,
    },

    // Race semantics
    /// Race: wait for the first of N arms to resolve.
    WaitAny {
        race_id: RaceId,
        arms: Box<[WaitArm]>,
    },
    /// Cancel a specific pending wait (used by engine after race resolution).
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

    // Inclusive gateway (OR fork/join)
    ForkInclusive {
        branches: Box<[InclusiveBranch]>,
        join_id: JoinId,
        default_target: Option<Addr>,
    },
    JoinDynamic {
        id: JoinId,
        next: Addr,
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
    // explicit, confirmed disposition (V2.7 entry amendment): the v1
    // `Fork`/`Join`/`WaitFor`/`WaitUntil`/`WaitMsg`/`WaitAny`/`WaitArm`
    // variants above are live, currently-executing v1 semantics that look
    // name- or shape-adjacent to several D2 words but are NOT conformant
    // (v1 `Fork`/`Join` use static `JoinId`+arrival-counting, not D2's
    // dynamic-handle/activation-record model; v1 `WaitArm::Timer` encodes
    // interrupting-vs-not as a `bool` flag, the exact anti-pattern D2's
    // `GUARD>`/`GUARD-N>` rejects by using distinct opcodes; v1
    // `WaitFor`/`WaitUntil`/`WaitMsg` are bare parks, but D2's words are
    // durable-effect-emitting). Reusing a v1 identifier for v2 semantics
    // would be a dual-path-by-stealth — the same variant meaning two things
    // depending on which tranche's code touches it. Every v1 variant above
    // stays byte-frozen; V5.3 deletes the entire v1 block as one unit.
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
    /// Parked in a race — waiting for first arm to fire.
    Race {
        race_id: RaceId,
        /// Absolute deadline (epoch ms) for the timer arm, if any.
        timer_deadline_ms: Option<u64>,
        /// Preserved from WaitState::Job during boundary timer promotion.
        job_key: Option<String>,
        /// If false, timer fires fork a new fiber instead of resolving the race.
        interrupting: bool,
        /// Index of the timer arm in the race_plan arms vec (computed, not hardcoded).
        timer_arm_index: Option<usize>,
        /// Remaining cycle fires (decremented each fire). None = no cycle.
        cycle_remaining: Option<u32>,
        /// How many times the timer has fired so far (for event numbering).
        cycle_fired_count: u32,
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
    Failed {
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
    /// Returns true if the process is in a terminal state (no further progress possible).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ProcessState::Completed { .. }
                | ProcessState::Cancelled { .. }
                | ProcessState::Terminated { .. }
                | ProcessState::Failed { .. }
        )
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
    pub(crate) race_plan: BTreeMap<RaceId, RacePlanEntry>,
    /// ExecNative bytecode addr → RaceId for tasks with boundary timers.
    pub(crate) boundary_map: BTreeMap<Addr, RaceId>,
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
    BTreeMap<RaceId, RacePlanEntry>,
    BTreeMap<Addr, RaceId>,
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
            race_plan,
            boundary_map,
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
            race_plan,
            boundary_map,
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
    pub fn race_plan(&self) -> &BTreeMap<RaceId, RacePlanEntry> {
        &self.race_plan
    }
    pub fn boundary_map(&self) -> &BTreeMap<Addr, RaceId> {
        &self.boundary_map
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
        race_plan: $race_plan:expr,
        boundary_map: $boundary_map:expr,
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
            $race_plan,
            $boundary_map,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RacePlanEntry {
    pub arms: Vec<WaitArm>,
    /// BPMN element ID of the boundary event (for audit events).
    /// None for non-boundary races (e.g., WaitAny opcode).
    pub boundary_element_id: Option<String>,
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
