use ob_poc_types::session_stack::SessionStackState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

// ─── Scalar aliases ───────────────────────────────────────────

/// Bytecode address (instruction pointer).
pub type Addr = u32;

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

// ─── Bytecode instructions ────────────────────────────────────

/// The 18-opcode ISA for the BPMN-Lite VM.
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
}

impl Fiber {
    pub fn new(fiber_id: Uuid, pc: Addr) -> Self {
        Self {
            fiber_id,
            pc,
            stack: Vec::new(),
            regs: std::array::from_fn(|_| Value::Bool(false)),
            wait: WaitState::Running,
            loop_epoch: 0,
        }
    }
}

// ─── Process instance ─────────────────────────────────────────

/// Top-level process state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ProcessState {
    Running,
    Completed { at: Timestamp },
    Cancelled { reason: String, at: Timestamp },
    Terminated { at: Timestamp },
    Failed { incident_id: Uuid },
    WaitingOnSubmission { callout_id: uuid::Uuid, node_id: String },
    WaitingOnInvocation { execution_id: uuid::Uuid, node_id: String },
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub bytecode_version: [u8; 32],
    pub program: Vec<Instr>,
    /// Bytecode address → BPMN element id (for diagnostics).
    pub debug_map: BTreeMap<Addr, String>,
    pub join_plan: BTreeMap<JoinId, JoinPlanEntry>,
    pub wait_plan: BTreeMap<WaitId, WaitPlanEntry>,
    pub message_name_map: BTreeMap<u32, String>,
    pub race_plan: BTreeMap<RaceId, RacePlanEntry>,
    /// ExecNative bytecode addr → RaceId for tasks with boundary timers.
    pub boundary_map: BTreeMap<Addr, RaceId>,
    /// task_type → set of flags it may write.
    pub write_set: BTreeMap<String, HashSet<FlagKey>>,
    /// All task_type references in the program.
    pub task_manifest: Vec<String>,
    /// ExecNative bytecode addr → ordered error routes (specific codes first, catch-all last).
    pub error_route_map: BTreeMap<Addr, Vec<ErrorRoute>>,
    /// Compile-time flag name → FlagKey mapping. Inverted from the lowering intern table;
    /// preserved so the FFI binding layer can resolve symbolic variable names to storage keys.
    pub flag_symbol_table: BTreeMap<FlagKey, String>,
    /// Resolved data-object declarations keyed by data-object id attribute.
    /// Populated by the A5 lowering pass; empty for processes with no
    /// `<bpmn:dataObject>` declarations.
    pub data_objects: BTreeMap<String, crate::ffi_bindings::DataObjectDecl>,
    /// Compiled FFI task declarations indexed by the bytecode address of
    /// the corresponding `Instr::ExecFfi` instruction.
    /// Populated by the A5 lowering pass; empty for processes with no
    /// `<bpmn:taskDefinition implementation="...">` annotations.
    pub ffi_task_decls: BTreeMap<Addr, crate::ffi_bindings::FfiTaskDecl>,
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
