use crate::persistence::CommandEnvelope;
use crate::session_stack::SessionStackState;
use crate::{
    ClaimedBufferedMessage, ErrorClass, Fiber, Incident, JobActivation, JobCompletion, JoinId,
    ProcessInstance, ProcessState, RuntimeEvent, Timestamp,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Tenant identifier carried explicitly at every durable transition boundary.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantId(String);

impl TenantId {
    pub fn new(value: impl Into<String>) -> Result<Self, TenantIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(TenantIdError::Empty);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TenantId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl AsRef<str> for TenantId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self("default".to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TenantIdError {
    #[error("tenant id must not be empty")]
    Empty,
}

/// The exclusive right to commit one revision of an instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    tenant_id: TenantId,
    instance_id: Uuid,
    expected_revision: u64,
    fence: u64,
}

/// One fenced unit of locally executable work. The aggregate snapshot and
/// command travel with the claim so the engine never reconstructs state with
/// independent instance/fiber/incident reads.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimedWork {
    claim: Claim,
    snapshot: Snapshot,
    command: Command,
}

impl ClaimedWork {
    pub fn new(claim: Claim, snapshot: Snapshot, command: Command) -> Self {
        Self {
            claim,
            snapshot,
            command,
        }
    }

    pub fn claim(&self) -> &Claim {
        &self.claim
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn command(&self) -> &Command {
        &self.command
    }

    pub fn into_parts(self) -> (Claim, Snapshot, Command) {
        (self.claim, self.snapshot, self.command)
    }
}

pub const START_COMMAND_SCHEMA_VERSION: u16 = 1;

/// Complete, versioned lineage for an idempotent workflow start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartCommand {
    schema_version: u16,
    tenant_id: TenantId,
    instance_id: Uuid,
    artifact_hash: [u8; 32],
    process_key: String,
    entry_id: Uuid,
    runbook_id: Uuid,
    correlation_id: String,
    idempotency_key: Uuid,
    initial_payload: String,
    initial_payload_hash: [u8; 32],
    session_stack: SessionStackState,
    logical_time: Timestamp,
}

impl StartCommand {
    pub fn builder(
        tenant_id: TenantId,
        instance_id: Uuid,
        artifact_hash: [u8; 32],
    ) -> StartCommandBuilder {
        StartCommandBuilder {
            tenant_id,
            instance_id,
            artifact_hash,
            process_key: None,
            entry_id: None,
            runbook_id: None,
            correlation_id: None,
            idempotency_key: None,
            initial_payload: None,
            session_stack: None,
            logical_time: None,
        }
    }

    fn from_builder(builder: StartCommandBuilder) -> Result<Self, StartCommandBuildError> {
        let initial_payload = builder
            .initial_payload
            .ok_or(StartCommandBuildError::Missing("initial_payload"))?;
        let initial_payload_hash = EffectId::content_hash(initial_payload.as_bytes());
        Ok(Self {
            schema_version: START_COMMAND_SCHEMA_VERSION,
            tenant_id: builder.tenant_id,
            instance_id: builder.instance_id,
            artifact_hash: builder.artifact_hash,
            process_key: builder
                .process_key
                .ok_or(StartCommandBuildError::Missing("process_key"))?,
            entry_id: builder
                .entry_id
                .ok_or(StartCommandBuildError::Missing("entry_id"))?,
            runbook_id: builder
                .runbook_id
                .ok_or(StartCommandBuildError::Missing("runbook_id"))?,
            correlation_id: builder
                .correlation_id
                .ok_or(StartCommandBuildError::Missing("correlation_id"))?,
            idempotency_key: builder
                .idempotency_key
                .ok_or(StartCommandBuildError::Missing("idempotency_key"))?,
            initial_payload,
            initial_payload_hash,
            session_stack: builder
                .session_stack
                .ok_or(StartCommandBuildError::Missing("session_stack"))?,
            logical_time: builder
                .logical_time
                .ok_or(StartCommandBuildError::Missing("logical_time"))?,
        })
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }
    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }
    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }
    pub fn artifact_hash(&self) -> [u8; 32] {
        self.artifact_hash
    }
    pub fn process_key(&self) -> &str {
        &self.process_key
    }
    pub fn entry_id(&self) -> Uuid {
        self.entry_id
    }
    pub fn runbook_id(&self) -> Uuid {
        self.runbook_id
    }
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }
    pub fn idempotency_key(&self) -> Uuid {
        self.idempotency_key
    }
    pub fn initial_payload(&self) -> &str {
        &self.initial_payload
    }
    pub fn initial_payload_hash(&self) -> [u8; 32] {
        self.initial_payload_hash
    }
    pub fn session_stack(&self) -> &SessionStackState {
        &self.session_stack
    }
    pub fn logical_time(&self) -> Timestamp {
        self.logical_time
    }
}

/// Validating constructor for the complete lineage required by a canonical start.
#[derive(Clone, Debug)]
pub struct StartCommandBuilder {
    tenant_id: TenantId,
    instance_id: Uuid,
    artifact_hash: [u8; 32],
    process_key: Option<String>,
    entry_id: Option<Uuid>,
    runbook_id: Option<Uuid>,
    correlation_id: Option<String>,
    idempotency_key: Option<Uuid>,
    initial_payload: Option<String>,
    session_stack: Option<SessionStackState>,
    logical_time: Option<Timestamp>,
}

impl StartCommandBuilder {
    pub fn process_key(mut self, value: impl Into<String>) -> Self {
        self.process_key = Some(value.into());
        self
    }

    pub fn lineage(mut self, entry_id: Uuid, runbook_id: Uuid) -> Self {
        self.entry_id = Some(entry_id);
        self.runbook_id = Some(runbook_id);
        self
    }

    pub fn correlation_id(mut self, value: impl Into<String>) -> Self {
        self.correlation_id = Some(value.into());
        self
    }

    pub fn idempotency_key(mut self, value: Uuid) -> Self {
        self.idempotency_key = Some(value);
        self
    }

    pub fn initial_payload(mut self, value: impl Into<String>) -> Self {
        self.initial_payload = Some(value.into());
        self
    }

    pub fn session_stack(mut self, value: SessionStackState) -> Self {
        self.session_stack = Some(value);
        self
    }

    pub fn logical_time(mut self, value: Timestamp) -> Self {
        self.logical_time = Some(value);
        self
    }

    pub fn build(self) -> Result<StartCommand, StartCommandBuildError> {
        StartCommand::from_builder(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StartCommandBuildError {
    #[error("start command is missing required field {0}")]
    Missing(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartDedupe {
    command: StartCommand,
}

impl StartDedupe {
    pub fn new(command: StartCommand) -> Self {
        Self { command }
    }
    pub fn command(&self) -> &StartCommand {
        &self.command
    }
}

impl Claim {
    pub fn new(tenant_id: TenantId, instance_id: Uuid, expected_revision: u64, fence: u64) -> Self {
        Self {
            tenant_id,
            instance_id,
            expected_revision,
            fence,
        }
    }

    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    pub fn expected_revision(&self) -> u64 {
        self.expected_revision
    }

    pub fn fence(&self) -> u64 {
        self.fence
    }
}

/// Stable identity for a durable side effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EffectId(Uuid);

impl EffectId {
    pub fn for_instruction(instance_id: Uuid, fiber_id: Uuid, pc: u32) -> Self {
        Self::for_instruction_ordinal(instance_id, fiber_id, pc, 0)
    }

    pub fn for_instruction_ordinal(
        instance_id: Uuid,
        fiber_id: Uuid,
        pc: u32,
        ordinal: u32,
    ) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"bpmn-lite/effect/instruction/v1");
        hasher.update(instance_id.as_bytes());
        hasher.update(fiber_id.as_bytes());
        hasher.update(&pc.to_be_bytes());
        hasher.update(&ordinal.to_be_bytes());
        Self::from_hash(hasher.finalize())
    }

    pub fn for_transition(instance_id: Uuid, revision: u64, ordinal: u32) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"bpmn-lite/effect/transition/v1");
        hasher.update(instance_id.as_bytes());
        hasher.update(&revision.to_be_bytes());
        hasher.update(&ordinal.to_be_bytes());
        Self::from_hash(hasher.finalize())
    }

    pub fn for_timer_fire(timer_id: EffectId, fire_count: u32) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"bpmn-lite/effect/timer-fire/v1");
        hasher.update(timer_id.as_uuid().as_bytes());
        hasher.update(&fire_count.to_be_bytes());
        Self::from_hash(hasher.finalize())
    }

    pub fn for_command(command_id: Uuid, revision: u64, ordinal: u32) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"bpmn-lite/kernel/id/v1");
        hasher.update(command_id.as_bytes());
        hasher.update(&revision.to_be_bytes());
        hasher.update(&ordinal.to_be_bytes());
        Self::from_hash(hasher.finalize())
    }

    pub fn content_hash(bytes: &[u8]) -> [u8; 32] {
        blake3::hash(bytes).into()
    }

    fn from_hash(hash: blake3::Hash) -> Self {
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash.as_bytes()[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x80;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(Uuid::from_bytes(bytes))
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }

    pub fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimerKind {
    Wait,
    Race {
        race_id: u32,
        arm_index: usize,
        resume_at: u32,
        interrupting: bool,
        job_key: Option<String>,
        boundary_element_id: Option<String>,
        arm_count: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerRepeatSpec {
    interval_ms: u64,
    remaining: u32,
    fired_count: u32,
}

impl TimerRepeatSpec {
    pub fn new(interval_ms: u64, remaining: u32, fired_count: u32) -> Self {
        Self {
            interval_ms,
            remaining,
            fired_count,
        }
    }

    pub fn interval_ms(&self) -> u64 {
        self.interval_ms
    }
    pub fn remaining(&self) -> u32 {
        self.remaining
    }
    pub fn fired_count(&self) -> u32 {
        self.fired_count
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DurableEffect {
    ScheduleTimer {
        timer_id: EffectId,
        fiber_id: Uuid,
        due_at: u64,
        kind: TimerKind,
        repeat_spec: Option<TimerRepeatSpec>,
    },
    Invoke {
        effect_id: EffectId,
        fiber_id: Uuid,
        pc: u32,
        operation: String,
        template_id: [u8; 32],
        input: Vec<u8>,
        idempotency_key: String,
    },
}

pub const EFFECT_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EffectResponse {
    FfiSuccess {
        output_payload: Vec<u8>,
        new_domain_payload: Option<String>,
    },
    FfiNoMatch,
    Failed {
        error_class: ErrorClass,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaimedEffect {
    tenant_id: TenantId,
    instance_id: Uuid,
    effect: DurableEffect,
    claim_token: Uuid,
    attempt: u32,
    policy_version: u32,
}

impl ClaimedEffect {
    pub fn new(
        tenant_id: TenantId,
        instance_id: Uuid,
        effect: DurableEffect,
        claim_token: Uuid,
        attempt: u32,
        policy_version: u32,
    ) -> Self {
        Self {
            tenant_id,
            instance_id,
            effect,
            claim_token,
            attempt,
            policy_version,
        }
    }

    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }
    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }
    pub fn effect(&self) -> &DurableEffect {
        &self.effect
    }
    pub fn claim_token(&self) -> Uuid {
        self.claim_token
    }
    pub fn attempt(&self) -> u32 {
        self.attempt
    }
    pub fn policy_version(&self) -> u32 {
        self.policy_version
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    version: u32,
    max_attempts: u32,
    base_delay_ms: u64,
    max_delay_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetryDecision {
    At { attempt: u32, due_at: u64 },
    Exhausted,
    Terminal,
}

impl RetryPolicy {
    pub fn new(
        version: u32,
        max_attempts: u32,
        base_delay_ms: u64,
        max_delay_ms: u64,
    ) -> Result<Self, &'static str> {
        if version == 0 || max_attempts == 0 || base_delay_ms == 0 || max_delay_ms < base_delay_ms {
            return Err("invalid retry policy bounds");
        }
        Ok(Self {
            version,
            max_attempts,
            base_delay_ms,
            max_delay_ms,
        })
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn decision(
        &self,
        attempt: u32,
        retry_hint: Option<u64>,
        error_class: &ErrorClass,
        logical_time: u64,
        command_id: Uuid,
    ) -> RetryDecision {
        if !matches!(error_class, ErrorClass::Transient) {
            return RetryDecision::Terminal;
        }
        let next_attempt = attempt.saturating_add(1);
        if next_attempt > self.max_attempts {
            return RetryDecision::Exhausted;
        }
        let shift = attempt.min(62);
        let delay = self
            .base_delay_ms
            .saturating_mul(1u64 << shift)
            .min(self.max_delay_ms);
        let hash = EffectId::content_hash(command_id.as_bytes());
        let jitter_seed = u64::from_le_bytes(hash[..8].try_into().unwrap_or([0; 8]));
        let jitter_bound = delay / 4;
        let jitter = if jitter_bound == 0 {
            0
        } else {
            jitter_seed % jitter_bound
        };
        let computed_due = logical_time.saturating_add(delay).saturating_add(jitter);
        RetryDecision::At {
            attempt: next_attempt,
            due_at: retry_hint.map_or(computed_due, |hint| hint.max(computed_due)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingEffectResponse {
    tenant_id: TenantId,
    instance_id: Uuid,
    effect: DurableEffect,
    response: EffectResponse,
}

impl PendingEffectResponse {
    pub fn new(
        tenant_id: TenantId,
        instance_id: Uuid,
        effect: DurableEffect,
        response: EffectResponse,
    ) -> Self {
        Self {
            tenant_id,
            instance_id,
            effect,
            response,
        }
    }

    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }
    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }
    pub fn effect_id(&self) -> EffectId {
        self.effect.effect_id()
    }
    pub fn effect(&self) -> &DurableEffect {
        &self.effect
    }
    pub fn response(&self) -> &EffectResponse {
        &self.response
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectTerminalState {
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectMutation {
    effect_id: EffectId,
    terminal_state: EffectTerminalState,
}

impl EffectMutation {
    pub fn terminal(effect_id: EffectId, terminal_state: EffectTerminalState) -> Self {
        Self {
            effect_id,
            terminal_state,
        }
    }

    pub fn effect_id(&self) -> EffectId {
        self.effect_id
    }
    pub fn terminal_state(&self) -> EffectTerminalState {
        self.terminal_state
    }
}

impl DurableEffect {
    pub fn schedule_timer(
        timer_id: EffectId,
        fiber_id: Uuid,
        due_at: u64,
        kind: TimerKind,
        repeat_spec: Option<TimerRepeatSpec>,
    ) -> Self {
        Self::ScheduleTimer {
            timer_id,
            fiber_id,
            due_at,
            kind,
            repeat_spec,
        }
    }

    pub fn effect_id(&self) -> EffectId {
        match self {
            Self::ScheduleTimer { timer_id, .. } => *timer_id,
            Self::Invoke { effect_id, .. } => *effect_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedTimer {
    tenant_id: TenantId,
    timer_id: EffectId,
    instance_id: Uuid,
    fiber_id: Uuid,
    due_at: u64,
    kind: TimerKind,
    repeat_spec: Option<TimerRepeatSpec>,
    claim_token: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedTimerIdentity {
    tenant_id: TenantId,
    timer_id: EffectId,
    instance_id: Uuid,
    fiber_id: Uuid,
}

impl ClaimedTimerIdentity {
    pub fn new(tenant_id: TenantId, timer_id: EffectId, instance_id: Uuid, fiber_id: Uuid) -> Self {
        Self {
            tenant_id,
            timer_id,
            instance_id,
            fiber_id,
        }
    }
}

impl ClaimedTimer {
    pub fn new(
        identity: ClaimedTimerIdentity,
        due_at: u64,
        kind: TimerKind,
        repeat_spec: Option<TimerRepeatSpec>,
        claim_token: Uuid,
    ) -> Self {
        Self {
            tenant_id: identity.tenant_id,
            timer_id: identity.timer_id,
            instance_id: identity.instance_id,
            fiber_id: identity.fiber_id,
            due_at,
            kind,
            repeat_spec,
            claim_token,
        }
    }

    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }
    pub fn timer_id(&self) -> EffectId {
        self.timer_id
    }
    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }
    pub fn fiber_id(&self) -> Uuid {
        self.fiber_id
    }
    pub fn due_at(&self) -> u64 {
        self.due_at
    }
    pub fn kind(&self) -> &TimerKind {
        &self.kind
    }
    pub fn repeat_spec(&self) -> Option<&TimerRepeatSpec> {
        self.repeat_spec.as_ref()
    }
    pub fn claim_token(&self) -> Uuid {
        self.claim_token
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimerMutation {
    Consume {
        timer_id: EffectId,
        claim_token: Uuid,
    },
    Rearm {
        timer_id: EffectId,
        claim_token: Uuid,
        due_at: u64,
        repeat_spec: TimerRepeatSpec,
    },
    CancelRace {
        fiber_id: Uuid,
        race_id: u32,
        except: EffectId,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EffectOutput {
    Job(JobCompletion),
    Ffi {
        fiber_id: Uuid,
        pc: u32,
        output_payload: Vec<u8>,
        new_domain_payload: Option<String>,
        no_match: bool,
    },
    Json(serde_json::Value),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Tick {
        fiber_id: Option<Uuid>,
    },
    EffectCompleted {
        effect_id: EffectId,
        output: EffectOutput,
    },
    EffectFailed {
        effect_id: EffectId,
        job_key: String,
        error_class: crate::ErrorClass,
        message: String,
        retry: Option<JobRetry>,
    },
    TimerFired {
        timer: ClaimedTimer,
        fired_at: u64,
    },
    MessageDelivered {
        message_id: String,
        name: String,
        correlation_key: String,
        payload: Vec<u8>,
        payload_hash: Option<[u8; 32]>,
        expires_at: Timestamp,
    },
    Cancel {
        reason: String,
    },
    Terminate,
    ResolveIncident {
        incident_id: Uuid,
        resolution: String,
    },
    StartChildResult {
        idempotency_key: String,
        child_instance_id: Uuid,
        accepted: bool,
    },
    JobClaimed {
        job_key: String,
        worker_id: String,
        claim_expires_at: Timestamp,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRetry {
    worker_id: String,
    claim_token: String,
    not_before_ms: i64,
}

impl JobRetry {
    pub fn new(
        worker_id: impl Into<String>,
        claim_token: impl Into<String>,
        not_before_ms: i64,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            claim_token: claim_token.into(),
            not_before_ms,
        }
    }
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }
    pub fn claim_token(&self) -> &str {
        &self.claim_token
    }
    pub fn not_before_ms(&self) -> i64 {
        self.not_before_ms
    }
}

/// Complete in-memory input to one pure kernel transition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    instance: ProcessInstance,
    fibers: BTreeMap<Uuid, Fiber>,
    join_counts: BTreeMap<JoinId, u16>,
    dedupe: BTreeMap<String, JobCompletion>,
    buffered_messages: Vec<ClaimedBufferedMessage>,
    incidents: BTreeMap<Uuid, Incident>,
    /// D1 concurrency table: scopes, barriers, (later) compensation
    /// records, keyed by `RecordId` (V&S §2, §4). Empty for every
    /// v2-pre-V4 snapshot — V4's words are the sole producers.
    concurrency_table: crate::concurrency::ConcurrencyTable,
    /// D3 Ring 2 hash domain: the identity set of effects this snapshot's
    /// fibres are currently waiting on. Empty until the store layer wires
    /// population from the effects/inbox tables.
    pending_effects: std::collections::BTreeSet<EffectId>,
}

impl Snapshot {
    pub fn new(instance: ProcessInstance, fibers: impl IntoIterator<Item = Fiber>) -> Self {
        Self {
            instance,
            fibers: fibers
                .into_iter()
                .map(|fiber| (fiber.fiber_id, fiber))
                .collect(),
            join_counts: BTreeMap::new(),
            dedupe: BTreeMap::new(),
            buffered_messages: Vec::new(),
            incidents: BTreeMap::new(),
            concurrency_table: crate::concurrency::ConcurrencyTable::new(),
            pending_effects: std::collections::BTreeSet::new(),
        }
    }

    pub fn with_concurrency_table(
        mut self,
        concurrency_table: crate::concurrency::ConcurrencyTable,
    ) -> Self {
        self.concurrency_table = concurrency_table;
        self
    }

    pub fn concurrency_table(&self) -> &crate::concurrency::ConcurrencyTable {
        &self.concurrency_table
    }

    pub fn with_pending_effects(
        mut self,
        pending_effects: impl IntoIterator<Item = EffectId>,
    ) -> Self {
        self.pending_effects = pending_effects.into_iter().collect();
        self
    }

    pub fn pending_effects(&self) -> &std::collections::BTreeSet<EffectId> {
        &self.pending_effects
    }

    pub fn with_join_counts(mut self, join_counts: BTreeMap<JoinId, u16>) -> Self {
        self.join_counts = join_counts;
        self
    }

    pub fn with_dedupe(mut self, dedupe: BTreeMap<String, JobCompletion>) -> Self {
        self.dedupe = dedupe;
        self
    }

    pub fn with_buffered_messages(
        mut self,
        buffered_messages: Vec<ClaimedBufferedMessage>,
    ) -> Self {
        self.buffered_messages = buffered_messages;
        self
    }

    pub fn with_incidents(mut self, incidents: impl IntoIterator<Item = Incident>) -> Self {
        self.incidents = incidents
            .into_iter()
            .map(|incident| (incident.incident_id, incident))
            .collect();
        self
    }

    pub fn instance(&self) -> &ProcessInstance {
        &self.instance
    }
    pub fn fibers(&self) -> &BTreeMap<Uuid, Fiber> {
        &self.fibers
    }
    pub fn fiber(&self, fiber_id: Uuid) -> Option<&Fiber> {
        self.fibers.get(&fiber_id)
    }
    pub fn join_count(&self, join_id: JoinId) -> u16 {
        self.join_counts.get(&join_id).copied().unwrap_or(0)
    }
    pub fn dedupe_completion(&self, key: &str) -> Option<&JobCompletion> {
        self.dedupe.get(key)
    }
    pub fn buffered_messages(&self) -> &[ClaimedBufferedMessage] {
        &self.buffered_messages
    }
    pub fn incident(&self, incident_id: Uuid) -> Option<&Incident> {
        self.incidents.get(&incident_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerFireOutcome {
    Applied,
    AlreadyConsumed,
    Stale,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DedupeWrite {
    key: String,
    completion: JobCompletion,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JoinMutation {
    Arrive(JoinId),
    Reset(JoinId),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BufferedMessageMutation {
    Insert(crate::BufferedMessage),
    Deliver(crate::BufferedMessage),
    Release(ClaimedBufferedMessage),
    Consume(ClaimedBufferedMessage),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JobMutation {
    RetryClaimed {
        job_key: String,
        worker_id: String,
        claim_token: String,
        error_class: String,
        error_message: String,
        not_before_ms: i64,
    },
    DeadLetterClaimed {
        job_key: String,
        worker_id: String,
        claim_token: String,
        error_class: String,
        error_message: String,
        incident_id: Uuid,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingInvocationWrite {
    callout_id: Uuid,
    process_instance_id: Uuid,
    node_id: String,
    target_domain: String,
    verb_id: String,
    idempotency_key: Uuid,
    execution_id: Option<Uuid>,
    submitted_at: Timestamp,
    ack_received_at: Option<Timestamp>,
    timeout_at: Option<Timestamp>,
}

#[derive(Clone, Debug)]
pub struct PendingInvocationIdentity {
    pub callout_id: Uuid,
    pub process_instance_id: Uuid,
    pub node_id: String,
    pub target_domain: String,
    pub verb_id: String,
    pub idempotency_key: Uuid,
}

#[derive(Clone, Debug)]
pub struct PendingInvocationTiming {
    pub execution_id: Option<Uuid>,
    pub submitted_at: Timestamp,
    pub ack_received_at: Option<Timestamp>,
    pub timeout_at: Option<Timestamp>,
}

impl PendingInvocationWrite {
    pub fn new(identity: PendingInvocationIdentity, timing: PendingInvocationTiming) -> Self {
        Self {
            callout_id: identity.callout_id,
            process_instance_id: identity.process_instance_id,
            node_id: identity.node_id,
            target_domain: identity.target_domain,
            verb_id: identity.verb_id,
            idempotency_key: identity.idempotency_key,
            execution_id: timing.execution_id,
            submitted_at: timing.submitted_at,
            ack_received_at: timing.ack_received_at,
            timeout_at: timing.timeout_at,
        }
    }

    pub fn callout_id(&self) -> Uuid {
        self.callout_id
    }
    pub fn process_instance_id(&self) -> Uuid {
        self.process_instance_id
    }
    pub fn node_id(&self) -> &str {
        &self.node_id
    }
    pub fn target_domain(&self) -> &str {
        &self.target_domain
    }
    pub fn verb_id(&self) -> &str {
        &self.verb_id
    }
    pub fn idempotency_key(&self) -> Uuid {
        self.idempotency_key
    }
    pub fn execution_id(&self) -> Option<Uuid> {
        self.execution_id
    }
    pub fn submitted_at(&self) -> Timestamp {
        self.submitted_at
    }
    pub fn ack_received_at(&self) -> Option<Timestamp> {
        self.ack_received_at
    }
    pub fn timeout_at(&self) -> Option<Timestamp> {
        self.timeout_at
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxWrite {
    id: Uuid,
    target_domain: String,
    target_endpoint: String,
    payload: Vec<u8>,
    idempotency_key: Uuid,
    callout_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusSubmissionAckMutation {
    outbox_id: Uuid,
    callout_id: Uuid,
    execution_id: Uuid,
    dispatch_claim_token: Uuid,
}

impl BusSubmissionAckMutation {
    pub fn new(
        outbox_id: Uuid,
        callout_id: Uuid,
        execution_id: Uuid,
        dispatch_claim_token: Uuid,
    ) -> Self {
        Self {
            outbox_id,
            callout_id,
            execution_id,
            dispatch_claim_token,
        }
    }

    pub fn outbox_id(&self) -> Uuid {
        self.outbox_id
    }
    pub fn callout_id(&self) -> Uuid {
        self.callout_id
    }
    pub fn execution_id(&self) -> Uuid {
        self.execution_id
    }
    pub fn dispatch_claim_token(&self) -> Uuid {
        self.dispatch_claim_token
    }
}

impl OutboxWrite {
    pub fn new(
        id: Uuid,
        target_domain: String,
        target_endpoint: String,
        payload: Vec<u8>,
        idempotency_key: Uuid,
        callout_id: Uuid,
    ) -> Self {
        Self {
            id,
            target_domain,
            target_endpoint,
            payload,
            idempotency_key,
            callout_id,
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn target_domain(&self) -> &str {
        &self.target_domain
    }
    pub fn target_endpoint(&self) -> &str {
        &self.target_endpoint
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    pub fn idempotency_key(&self) -> Uuid {
        self.idempotency_key
    }
    pub fn callout_id(&self) -> Uuid {
        self.callout_id
    }
}

impl DedupeWrite {
    pub fn new(key: impl Into<String>, completion: JobCompletion) -> Self {
        Self {
            key: key.into(),
            completion,
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn completion(&self) -> &JobCompletion {
        &self.completion
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChildStart {
    instance: ProcessInstance,
    root_fiber: Fiber,
    start_event: RuntimeEvent,
    idempotency_key: String,
}

impl ChildStart {
    pub fn new(
        instance: ProcessInstance,
        root_fiber: Fiber,
        start_event: RuntimeEvent,
        idempotency_key: impl Into<String>,
    ) -> Self {
        Self {
            instance,
            root_fiber,
            start_event,
            idempotency_key: idempotency_key.into(),
        }
    }

    pub fn instance(&self) -> &ProcessInstance {
        &self.instance
    }

    pub fn root_fiber(&self) -> &Fiber {
        &self.root_fiber
    }

    pub fn start_event(&self) -> &RuntimeEvent {
        &self.start_event
    }

    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TerminalCleanup {
    delete_all_fibers: bool,
    delete_all_joins: bool,
    cancel_jobs: bool,
}

impl TerminalCleanup {
    pub fn new(delete_all_fibers: bool, delete_all_joins: bool, cancel_jobs: bool) -> Self {
        Self {
            delete_all_fibers,
            delete_all_joins,
            cancel_jobs,
        }
    }

    pub fn delete_all_fibers(&self) -> bool {
        self.delete_all_fibers
    }

    pub fn delete_all_joins(&self) -> bool {
        self.delete_all_joins
    }

    pub fn cancel_jobs(&self) -> bool {
        self.cancel_jobs
    }
}

/// Complete mutation set for one logical workflow transition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transition {
    next_snapshot: ProcessInstance,
    fibers_upsert: Vec<Fiber>,
    fibers_delete: Vec<Uuid>,
    events: Vec<RuntimeEvent>,
    effects: Vec<DurableEffect>,
    effect_mutations: Vec<EffectMutation>,
    timer_mutations: Vec<TimerMutation>,
    jobs_enqueue: Vec<JobActivation>,
    jobs_ack: Vec<String>,
    job_mutations: Vec<JobMutation>,
    dedupe: Vec<DedupeWrite>,
    incidents: Vec<Incident>,
    join_mutations: Vec<JoinMutation>,
    buffered_messages: Vec<BufferedMessageMutation>,
    pending_invocations: Vec<PendingInvocationWrite>,
    pending_take: Vec<Uuid>,
    outbox: Vec<OutboxWrite>,
    bus_submission_acks: Vec<BusSubmissionAckMutation>,
    state_override: Option<ProcessState>,
    child_starts: Vec<ChildStart>,
    terminal_cleanup: TerminalCleanup,
    start_dedupe: Option<StartDedupe>,
    command_envelope: Option<CommandEnvelope>,
    /// D1 deltas (V1 declares the surface; V4's words are the sole
    /// producers — see `crate::concurrency`).
    concurrency_mutations: Vec<crate::concurrency::ConcurrencyMutation>,
    control_stack_deltas: Vec<crate::concurrency::ControlStackDelta>,
}

impl Transition {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
    pub fn next_snapshot(&self) -> &ProcessInstance {
        &self.next_snapshot
    }
    pub fn fibers_upsert(&self) -> &[Fiber] {
        &self.fibers_upsert
    }
    pub fn fibers_delete(&self) -> &[Uuid] {
        &self.fibers_delete
    }
    pub fn events(&self) -> &[RuntimeEvent] {
        &self.events
    }
    pub fn effects(&self) -> &[DurableEffect] {
        &self.effects
    }
    pub fn effect_mutations(&self) -> &[EffectMutation] {
        &self.effect_mutations
    }
    pub fn timer_mutations(&self) -> &[TimerMutation] {
        &self.timer_mutations
    }
    pub fn jobs_enqueue(&self) -> &[JobActivation] {
        &self.jobs_enqueue
    }
    pub fn jobs_ack(&self) -> &[String] {
        &self.jobs_ack
    }
    pub fn job_mutations(&self) -> &[JobMutation] {
        &self.job_mutations
    }
    pub fn dedupe(&self) -> &[DedupeWrite] {
        &self.dedupe
    }
    pub fn incidents(&self) -> &[Incident] {
        &self.incidents
    }
    pub fn join_mutations(&self) -> &[JoinMutation] {
        &self.join_mutations
    }
    pub fn buffered_messages(&self) -> &[BufferedMessageMutation] {
        &self.buffered_messages
    }
    pub fn pending_invocations(&self) -> &[PendingInvocationWrite] {
        &self.pending_invocations
    }
    pub fn pending_take(&self) -> &[Uuid] {
        &self.pending_take
    }
    pub fn outbox(&self) -> &[OutboxWrite] {
        &self.outbox
    }
    pub fn bus_submission_acks(&self) -> &[BusSubmissionAckMutation] {
        &self.bus_submission_acks
    }
    pub fn state_override(&self) -> Option<&ProcessState> {
        self.state_override.as_ref()
    }
    pub fn child_starts(&self) -> &[ChildStart] {
        &self.child_starts
    }
    pub fn terminal_cleanup(&self) -> &TerminalCleanup {
        &self.terminal_cleanup
    }
    pub fn start_dedupe(&self) -> Option<&StartDedupe> {
        self.start_dedupe.as_ref()
    }
    pub fn command_envelope(&self) -> Option<&CommandEnvelope> {
        self.command_envelope.as_ref()
    }
    pub fn concurrency_mutations(&self) -> &[crate::concurrency::ConcurrencyMutation] {
        &self.concurrency_mutations
    }
    pub fn control_stack_deltas(&self) -> &[crate::concurrency::ControlStackDelta] {
        &self.control_stack_deltas
    }

    pub fn with_command_envelope(mut self, command: CommandEnvelope) -> Self {
        self.command_envelope = Some(command);
        self
    }
}

pub struct TransitionBuilder {
    transition: Transition,
}

impl TransitionBuilder {
    pub fn new(next_snapshot: ProcessInstance) -> Self {
        Self {
            transition: Transition {
                next_snapshot,
                fibers_upsert: Vec::new(),
                fibers_delete: Vec::new(),
                events: Vec::new(),
                effects: Vec::new(),
                effect_mutations: Vec::new(),
                timer_mutations: Vec::new(),
                jobs_enqueue: Vec::new(),
                jobs_ack: Vec::new(),
                job_mutations: Vec::new(),
                dedupe: Vec::new(),
                incidents: Vec::new(),
                join_mutations: Vec::new(),
                buffered_messages: Vec::new(),
                pending_invocations: Vec::new(),
                pending_take: Vec::new(),
                outbox: Vec::new(),
                bus_submission_acks: Vec::new(),
                state_override: None,
                child_starts: Vec::new(),
                terminal_cleanup: TerminalCleanup::default(),
                start_dedupe: None,
                command_envelope: None,
                concurrency_mutations: Vec::new(),
                control_stack_deltas: Vec::new(),
            },
        }
    }

    pub fn upsert_fiber(mut self, fiber: Fiber) -> Self {
        self.transition.fibers_upsert.push(fiber);
        self
    }
    pub fn delete_fiber(mut self, fiber_id: Uuid) -> Self {
        self.transition.fibers_delete.push(fiber_id);
        self
    }
    pub fn event(mut self, event: RuntimeEvent) -> Self {
        self.transition.events.push(event);
        self
    }
    pub fn effect(mut self, effect: DurableEffect) -> Self {
        self.transition.effects.push(effect);
        self
    }
    pub fn effect_mutation(mut self, mutation: EffectMutation) -> Self {
        self.transition.effect_mutations.push(mutation);
        self
    }
    pub fn timer_mutation(mut self, mutation: TimerMutation) -> Self {
        self.transition.timer_mutations.push(mutation);
        self
    }
    pub fn enqueue_job(mut self, job: JobActivation) -> Self {
        self.transition.jobs_enqueue.push(job);
        self
    }
    pub fn ack_job(mut self, job_key: impl Into<String>) -> Self {
        self.transition.jobs_ack.push(job_key.into());
        self
    }
    pub fn job_mutation(mut self, mutation: JobMutation) -> Self {
        self.transition.job_mutations.push(mutation);
        self
    }
    pub fn dedupe(mut self, write: DedupeWrite) -> Self {
        self.transition.dedupe.push(write);
        self
    }
    pub fn incident(mut self, incident: Incident) -> Self {
        self.transition.incidents.push(incident);
        self
    }
    pub fn join_mutation(mut self, mutation: JoinMutation) -> Self {
        self.transition.join_mutations.push(mutation);
        self
    }
    pub fn buffered_message(mut self, mutation: BufferedMessageMutation) -> Self {
        self.transition.buffered_messages.push(mutation);
        self
    }
    pub fn pending_invocation(mut self, pending: PendingInvocationWrite) -> Self {
        self.transition.pending_invocations.push(pending);
        self
    }
    pub fn take_pending(mut self, execution_id: Uuid) -> Self {
        self.transition.pending_take.push(execution_id);
        self
    }
    pub fn outbox(mut self, outbox: OutboxWrite) -> Self {
        self.transition.outbox.push(outbox);
        self
    }
    pub fn bus_submission_ack(mut self, ack: BusSubmissionAckMutation) -> Self {
        self.transition.bus_submission_acks.push(ack);
        self
    }
    pub fn state(mut self, state: ProcessState) -> Self {
        self.transition.state_override = Some(state);
        self
    }
    pub fn child_start(mut self, child: ChildStart) -> Self {
        self.transition.child_starts.push(child);
        self
    }
    pub fn terminal_cleanup(mut self, cleanup: TerminalCleanup) -> Self {
        self.transition.terminal_cleanup = cleanup;
        self
    }
    pub fn start_dedupe(mut self, start: StartDedupe) -> Self {
        self.transition.start_dedupe = Some(start);
        self
    }
    pub fn command_envelope(mut self, command: CommandEnvelope) -> Self {
        self.transition.command_envelope = Some(command);
        self
    }
    pub fn concurrency_mutation(
        mut self,
        mutation: crate::concurrency::ConcurrencyMutation,
    ) -> Self {
        self.transition.concurrency_mutations.push(mutation);
        self
    }
    pub fn control_stack_delta(mut self, delta: crate::concurrency::ControlStackDelta) -> Self {
        self.transition.control_stack_deltas.push(delta);
        self
    }
    pub fn build(self) -> Transition {
        self.transition
    }
}

#[cfg(test)]
mod retry_policy_tests {
    use super::*;

    #[test]
    fn retry_decision_is_deterministic_and_bounded() {
        let policy = RetryPolicy::new(3, 4, 1_000, 8_000).unwrap();
        let command_id = Uuid::from_u128(42);
        let first = policy.decision(1, None, &ErrorClass::Transient, 10_000, command_id);
        let replay = policy.decision(1, None, &ErrorClass::Transient, 10_000, command_id);
        assert_eq!(first, replay);
        let RetryDecision::At { attempt, due_at } = first else {
            panic!("transient attempt must be retried");
        };
        assert_eq!(attempt, 2);
        assert!((12_000..12_500).contains(&due_at));
    }

    #[test]
    fn retry_decision_never_retries_terminal_or_exhausted_errors() {
        let policy = RetryPolicy::new(1, 2, 100, 1_000).unwrap();
        assert_eq!(
            policy.decision(0, None, &ErrorClass::ContractViolation, 0, Uuid::nil()),
            RetryDecision::Terminal
        );
        assert_eq!(
            policy.decision(2, None, &ErrorClass::Transient, 0, Uuid::nil()),
            RetryDecision::Exhausted
        );
    }
}
