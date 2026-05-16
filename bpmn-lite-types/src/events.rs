use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Serializable description of a wait arm for the event log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WaitArmDesc {
    Timer { duration_ms: u64 },
    Deadline { deadline_ms: u64 },
    Msg { name: u32 },
    Internal { kind: u32 },
}

impl From<&WaitArm> for WaitArmDesc {
    fn from(arm: &WaitArm) -> Self {
        match arm {
            WaitArm::Timer {
                duration_ms,
                interrupting: _,
                cycle: _,
                ..
            } => WaitArmDesc::Timer {
                duration_ms: *duration_ms,
            },
            WaitArm::Deadline { deadline_ms, .. } => WaitArmDesc::Deadline {
                deadline_ms: *deadline_ms,
            },
            WaitArm::Msg { name, .. } => WaitArmDesc::Msg { name: *name },
            WaitArm::Internal { kind, .. } => WaitArmDesc::Internal { kind: *kind },
        }
    }
}

/// Runtime events — the durable audit trail for every process instance.
/// 24 variants covering the full lifecycle.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RuntimeEvent {
    InstanceStarted {
        instance_id: Uuid,
        bytecode_version: [u8; 32],
    },
    FiberSpawned {
        fiber_id: Uuid,
        pc: Addr,
        parent: Option<Uuid>,
    },
    JobActivated {
        job_key: String,
        task_type: String,
        service_task_id: String,
        pc: Addr,
    },
    JobClaimed {
        job_key: String,
        worker_id: String,
        claim_expires_at: i64,
    },
    JobCompleted {
        job_key: String,
        payload_hash_before: [u8; 32],
        payload_hash_after: [u8; 32],
        orch_flags_out: BTreeMap<String, Value>,
        pc_next: Addr,
    },
    JobReclaimed {
        job_key: String,
        previous_worker_id: Option<String>,
    },
    JobRetryScheduled {
        job_key: String,
        retry_at: i64,
        retries_remaining: u32,
    },
    JobDeadLettered {
        job_key: String,
        incident_id: Uuid,
    },
    GatewayTaken {
        gateway_id: String,
        branch_taken: Addr,
        condition_value: Value,
    },
    FlagSet {
        key: FlagKey,
        value: Value,
    },
    Forked {
        fork_id: String,
        child_fibers: Vec<Uuid>,
        targets: Vec<Addr>,
    },
    JoinArrived {
        join_id: JoinId,
        fiber_id: Uuid,
    },
    JoinReleased {
        join_id: JoinId,
        next_pc: Addr,
        released_fiber_id: Uuid,
    },
    WaitTimerSet {
        fiber_id: Uuid,
        deadline_ms: u64,
    },
    WaitMsgSubscribed {
        fiber_id: Uuid,
        name: u32,
        corr_key: Value,
    },
    MsgReceived {
        name: u32,
        corr_key: Value,
        msg_ref: Option<Uuid>,
    },
    MessageBuffered {
        message_name: String,
        correlation_key: String,
        msg_id: String,
        expires_at: i64,
    },
    BufferedMessageConsumed {
        message_name: String,
        correlation_key: String,
        msg_id: String,
        fiber_id: Uuid,
    },
    BufferedMessageExpired {
        message_name: String,
        correlation_key: String,
        msg_id: String,
    },
    IncidentCreated {
        incident_id: Uuid,
        service_task_id: String,
        job_key: Option<String>,
    },
    RaceRegistered {
        race_id: RaceId,
        fiber_id: Uuid,
        arms: Vec<WaitArmDesc>,
    },
    RaceWon {
        race_id: RaceId,
        fiber_id: Uuid,
        winner_index: usize,
        resume_at: Addr,
    },
    RaceCancelled {
        race_id: RaceId,
        cancelled_indices: Vec<usize>,
    },
    LateSignalIgnored {
        race_id: RaceId,
        arm_index: usize,
    },
    WaitCancelled {
        fiber_id: Uuid,
        wait_desc: String,
        reason: String,
    },
    SignalIgnored {
        signal_desc: String,
    },
    /// Non-interrupting boundary timer fired — spawned child fiber.
    BoundaryFired {
        race_id: RaceId,
        fiber_id: Uuid,
        spawned_fiber_id: Uuid,
        boundary_element_id: String,
        resume_at: Addr,
    },
    /// One iteration of a timer cycle completed.
    TimerCycleIteration {
        race_id: RaceId,
        fiber_id: Uuid,
        iteration: u32,
        remaining: u32,
    },
    /// All iterations of a timer cycle have been consumed.
    TimerCycleExhausted {
        race_id: RaceId,
        fiber_id: Uuid,
        total_fired: u32,
    },
    Cancelled {
        reason: String,
    },
    Completed {
        at: Timestamp,
    },
    Terminated {
        at: Timestamp,
        fiber_id: Uuid,
    },
    ErrorRouted {
        job_key: String,
        error_code: String,
        boundary_id: String,
        resume_at: Addr,
    },
    /// Bounded loop counter incremented by IncCounter opcode.
    CounterIncremented {
        counter_id: u32,
        new_value: u32,
        loop_epoch: u32,
    },
    /// Inclusive (OR) gateway fork — records which branches were taken and dynamic join count.
    InclusiveForkTaken {
        gateway_id: String,
        branches_taken: Vec<Addr>,
        join_id: JoinId,
        expected: u16,
    },

    // ── FFI audit events (A8) ─────────────────────────────────────────────────

    /// Written BEFORE dispatching an in-process FFI call.
    /// Paired with FfiInvocationCompleted; together they form an
    /// audit record matching `ffi_invocation_record` table rows.
    FfiInvocationPending {
        invocation_id: Uuid,
        /// 32-byte BLAKE3 digest stored as hex string for readability.
        template_id_hex: String,
        caller_task_id: String,
        caller_pc: Addr,
        owner_type: String,
    },

    /// Written AFTER an in-process FFI call returns.
    FfiInvocationCompleted {
        invocation_id: Uuid,
        /// "success" | "no_match" | "incident"
        outcome_kind: String,
        /// For incidents: structured error description.
        error_message: Option<String>,
    },
}
