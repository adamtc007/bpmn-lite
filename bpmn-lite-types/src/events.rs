use crate::types::*;
use crate::EffectId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

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
    TimerFired {
        timer_id: EffectId,
        fiber_id: Uuid,
        fired_at: u64,
    },
    WaitMsgSubscribed {
        fiber_id: Uuid,
        name: u32,
        corr_key: String,
    },
    MsgReceived {
        name: u32,
        corr_key: String,
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
    IncidentResolved {
        incident_id: Uuid,
        resolution: String,
    },
    ChildStartAccepted {
        idempotency_key: String,
        child_instance_id: Uuid,
    },
    ChildStartRejected {
        idempotency_key: String,
        child_instance_id: Uuid,
        incident_id: Uuid,
    },
    WaitCancelled {
        fiber_id: Uuid,
        wait_desc: String,
        reason: String,
    },
    SignalIgnored {
        signal_desc: String,
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

    /// A19 — Written when a pickup boundary detects an integrity hash mismatch.
    /// The instance is quarantined after this event is appended.
    InstanceQuarantined {
        instance_id: Uuid,
        tenant_id: String,
        /// "scheduler_claim" | "grpc_handler" | "a17_recovery"
        detection_point: String,
        /// "integrity_hash_mismatch"
        failure_reason: String,
        detected_at: Timestamp,
    },

    // ── V4 D2 word audit events ────────────────────────────────────────────
    // V2-prefixed per the V2.7 coexistence rule (module docs on `Instr`):
    // these carry `RecordId`, not v1's static `JoinId`, so they are not
    // reuses of `Forked`/`JoinArrived`/`JoinReleased` even where the shape
    // looks close. Not part of the canonical hash domain (audit log only —
    // see `canonical.rs`, which has no `RuntimeEvent` impl).
    /// `V2Guard` opened an interrupting-guard record.
    V2GuardOpened {
        record_id: crate::concurrency::RecordId,
        fiber_id: Uuid,
        handler: Addr,
    },
    /// `V2GuardEnd`/`V2GuardREnd` retired a guard record on its normal
    /// (non-triggered/non-rollback) path.
    V2GuardRetired {
        record_id: crate::concurrency::RecordId,
        fiber_id: Uuid,
    },
    /// A18: `V2GuardR` opened a rollback-capable interrupting-guard record.
    /// Distinct from `V2GuardOpened` — carries no `handler` (see
    /// `Instr::V2GuardR`'s doc comment: `GUARD-R>` never spawns a handler
    /// fibre, so there is no `Addr` to report here).
    V2GuardROpened {
        record_id: crate::concurrency::RecordId,
        fiber_id: Uuid,
    },
    /// `V2Fork` allocated a barrier record and spawned member fibres.
    V2Forked {
        record_id: crate::concurrency::RecordId,
        fork_fiber_id: Uuid,
        child_fibers: Vec<Uuid>,
        targets: Vec<Addr>,
    },
    /// `V2Join` arrived but was not the last member — parked.
    V2JoinArrived {
        record_id: crate::concurrency::RecordId,
        fiber_id: Uuid,
    },
    /// `V2Join`'s last arrival — barrier retired, `survivor_fiber_id`
    /// continues in place, `cancelled_fibers` were deleted (V&S v0.4 ruling B).
    V2JoinReleased {
        record_id: crate::concurrency::RecordId,
        survivor_fiber_id: Uuid,
        next_pc: Addr,
        cancelled_fibers: Vec<Uuid>,
    },
    /// `V2RaceOpen` allocated a race record.
    V2RaceOpened {
        record_id: crate::concurrency::RecordId,
        fiber_id: Uuid,
        arm_count: u16,
    },
    /// `V2RaceClose` parked the fiber on its armed alternatives.
    V2RaceClosed {
        record_id: crate::concurrency::RecordId,
        fiber_id: Uuid,
        arm_count: u16,
    },
    /// A v2 race resolved — `fiber_id`'s winning arm fired (message
    /// delivery or timer). The race record retires in the same transition.
    V2RaceWon {
        record_id: crate::concurrency::RecordId,
        fiber_id: Uuid,
    },
    /// An interrupting guard fired: every record nested under it (and
    /// their member fibres) is cancelled in the same transition (V&S
    /// v0.4 §4/§12 ruling A order — `cancelled_records` is listed
    /// deepest-first), and the handler fibre spawns.
    V2GuardTriggered {
        record_id: crate::concurrency::RecordId,
        handler_fiber_id: Uuid,
        cancelled_records: Vec<crate::concurrency::RecordId>,
        cancelled_fibers: Vec<Uuid>,
    },
    /// A non-interrupting guard (`GUARD-N>`) fired: the handler spawns,
    /// nothing is cancelled, the record re-arms (V&S §13 amendment v0.5,
    /// ruling A) — it was never retired, so there is nothing to re-`Insert`.
    V2GuardNTriggered {
        record_id: crate::concurrency::RecordId,
        handler_fiber_id: Uuid,
    },
    /// `V2CancelScope` fired: nested records/fibres cancelled (as
    /// `V2GuardTriggered`, no handler), and the instance's
    /// `domain_payload` was restored to the scope's rollback snapshot.
    V2ScopeCancelled {
        record_id: crate::concurrency::RecordId,
        fiber_id: Uuid,
        cancelled_records: Vec<crate::concurrency::RecordId>,
        cancelled_fibers: Vec<Uuid>,
    },
}
