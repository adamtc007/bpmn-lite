use crate::{ArtifactStoreError, ClaimError, CommitError, CommitOutcome, StoreResult};
use async_trait::async_trait;
use bpmn_lite_types::events::RuntimeEvent;
use bpmn_lite_types::*;
use uuid::Uuid;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TemplateSummary {
    pub name: String,
    pub latest_version: u32,
    pub plan_hash: [u8; 32],
    pub created_at: String,
}

// ── Designer sessions (EOP-SAGE-REPL-BPMN-001 T1) ──────────────────────

/// One append-only event in a design session. Revisions carry the FULL
/// DSL source (undo = re-append an earlier revision's source; nothing is
/// ever deleted), utterances carry the REPL dialogue.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DesignSessionEventKind {
    Revision { dsl_source: String, note: String },
    Utterance { text: String, response: String },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DesignSessionEvent {
    pub seq: u64,
    pub kind: DesignSessionEventKind,
    pub at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DesignSessionStatus {
    Draft,
    Saved,
}

/// Full session aggregate: metadata + the ordered event log. The
/// CURRENT source is the last Revision event's `dsl_source` (derived,
/// never stored separately — one source of truth).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DesignSessionRecord {
    pub id: Uuid,
    pub tenant_id: String,
    pub name: String,
    pub status: DesignSessionStatus,
    /// (template_name, version, plan_hash) once saved-as-template.
    pub template_ref: Option<(String, u32, [u8; 32])>,
    pub events: Vec<DesignSessionEvent>,
    pub created_at: String,
}

impl DesignSessionRecord {
    /// The session's current DSL source: the last Revision event.
    pub fn current_source(&self) -> Option<&str> {
        self.events.iter().rev().find_map(|event| match &event.kind {
            DesignSessionEventKind::Revision { dsl_source, .. } => Some(dsl_source.as_str()),
            DesignSessionEventKind::Utterance { .. } => None,
        })
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DesignSessionSummary {
    pub id: Uuid,
    pub name: String,
    pub status: DesignSessionStatus,
    pub revisions: u32,
    pub updated_at: String,
}

/// Transactional runtime claims, aggregate reads, and fenced transition commits.
#[async_trait]
pub trait RuntimeStore: Send + Sync + JournalReader {
    // ── Instance ──

    async fn load_instance(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
    ) -> StoreResult<Option<ProcessInstance>>;

    // ── Fibers ──

    async fn load_fiber(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        fiber_id: Uuid,
    ) -> StoreResult<Option<Fiber>>;

    async fn load_fibers(&self, tenant_id: &TenantId, instance_id: Uuid)
        -> StoreResult<Vec<Fiber>>;

    // ── Dedupe cache (engine-side) ──

    async fn dedupe_get(
        &self,
        tenant_id: &TenantId,
        key: &str,
    ) -> StoreResult<Option<JobCompletion>>;

    // ── Job queue ──

    async fn dequeue_jobs(
        &self,
        task_types: &[String],
        max: usize,
        tenant_id: &TenantId,
        worker_id: &str,
        lease_ms: u64,
    ) -> StoreResult<Vec<JobActivation>>;

    async fn validate_job_claim(
        &self,
        tenant_id: &TenantId,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> StoreResult<bool>;

    // ── Dead-letter queue ──

    async fn dead_letter_put(
        &self,
        name: u32,
        corr_key: &Value,
        payload: &[u8],
        ttl_ms: u64,
    ) -> StoreResult<()>;

    async fn dead_letter_take(&self, name: u32, corr_key: &Value) -> StoreResult<Option<Vec<u8>>>;

    async fn claim_buffered_message(
        &self,
        tenant_id: &TenantId,
        message_name: &str,
        correlation_key: &str,
        claim_ms: u64,
    ) -> StoreResult<Option<ClaimedBufferedMessage>>;

    async fn reclaim_stale_buffered_message_claims(&self) -> StoreResult<u32>;

    async fn prune_expired_messages(&self) -> StoreResult<u32>;

    // ── Incidents ──

    async fn load_incidents(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Vec<Incident>>;

    // ── Durability maintenance ──

    /// Reclaim jobs stuck in 'claimed' state past timeout. Returns count reclaimed.
    async fn reclaim_stale_jobs(&self, timeout_ms: u64) -> StoreResult<u32>;

    /// Prune dedupe entries older than the given age. Returns count pruned.
    async fn prune_dedupe_cache(&self, older_than_ms: u64) -> StoreResult<u32>;

    /// List all running (non-terminal) instance IDs.
    async fn list_running_instances(&self, tenant_id: &TenantId) -> StoreResult<Vec<Uuid>>;

    /// Claim a bounded batch of running instances for scheduler work.
    async fn claim_running_instances(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<Uuid>>;

    /// Claim the mutation right for one process instance.
    ///
    /// This is a transition guard, not a scheduler batch primitive. Every
    /// mutating engine path should claim this before changing instance state.
    async fn claim_instance_for_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
    ) -> std::result::Result<Option<Claim>, ClaimError>;

    /// Fence an instance and return its complete persisted aggregate plus the
    /// command to apply. Engine execution uses this method instead of issuing
    /// independent instance/fiber/incident loads.
    async fn claim_work_for_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
        command: Command,
    ) -> std::result::Result<Option<ClaimedWork>, ClaimError> {
        let Some(claim) = self
            .claim_instance_for_transition(tenant_id, instance_id, owner, lease_ms)
            .await?
        else {
            return Ok(None);
        };
        let load_result = self.load_snapshot_envelope(tenant_id, instance_id).await;
        let (envelope, claim_error) = match load_result {
            Ok(Some(envelope)) => (Some(envelope), None),
            Ok(None) => (
                None,
                Some(ClaimError::Integrity(
                    "claimed instance has no snapshot".to_string(),
                )),
            ),
            Err(error) => (None, Some(ClaimError::Unavailable(error.to_string()))),
        };
        if let Some(claim_error) = claim_error {
            if let Err(release_error) = self
                .release_instance_transition(tenant_id, instance_id, owner)
                .await
            {
                return Err(ClaimError::Unavailable(format!(
                    "{claim_error}; transition lease release also failed: {release_error}"
                )));
            }
            return Err(claim_error);
        }
        let envelope = envelope.ok_or_else(|| {
            ClaimError::Integrity("claimed instance snapshot load returned no result".to_string())
        })?;
        let snapshot = envelope.state().to_runtime_snapshot();
        Ok(Some(ClaimedWork::new(claim, snapshot, command)))
    }

    /// Commit one complete logical transition under optimistic revision and
    /// monotonic fencing checks.
    async fn commit_transition(
        &self,
        claim: &Claim,
        transition: &Transition,
    ) -> std::result::Result<CommitOutcome, CommitError>;

    async fn lookup_start_instance(
        &self,
        tenant_id: &TenantId,
        idempotency_key: Uuid,
    ) -> StoreResult<Option<Uuid>>;

    /// Claim a bounded set of due timer deliveries without holding locks while
    /// the owning instances are loaded and transitioned.
    async fn claim_due_timers(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedTimer>>;

    /// Release a timer delivery lease after a transient failure before commit.
    async fn release_timer_claim(&self, timer: &ClaimedTimer) -> StoreResult<()>;

    /// Lease committed external effects for dispatch. The lease transaction is
    /// short and is closed before any network or FFI call begins.
    async fn claim_pending_effects(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedEffect>>;

    /// Persist a remote response exactly once and release its dispatch lease.
    async fn record_effect_response(
        &self,
        effect: &ClaimedEffect,
        response: &EffectResponse,
    ) -> StoreResult<bool>;

    /// Load durable responses awaiting normal fenced kernel application.
    async fn load_effect_responses(
        &self,
        tenant_id: &TenantId,
        limit: usize,
    ) -> StoreResult<Vec<PendingEffectResponse>>;

    /// Release a dispatch lease after transport failure without advancing state.
    async fn release_effect_claim(&self, effect: &ClaimedEffect) -> StoreResult<()>;

    /// Atomically release a dispatch lease and persist the deterministic retry
    /// attempt/due time selected by the versioned policy.
    async fn schedule_effect_retry(
        &self,
        effect: &ClaimedEffect,
        decision: RetryDecision,
        error: &str,
    ) -> StoreResult<()>;

    /// Release a previously claimed transition guard if this owner still holds it.
    async fn release_instance_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
    ) -> StoreResult<()>;

    /// Read the current arrive count for a join barrier.
    async fn join_get(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        join_id: JoinId,
    ) -> StoreResult<u16>;
}

/// Immutable executable artifact and legacy migration-plan repository.
#[async_trait]
pub trait ArtifactRepository: Send + Sync {
    // ── Program store (versioned bytecode) ──

    async fn store_program(&self, version: [u8; 32], program: &CompiledProgram) -> StoreResult<()>;

    async fn load_program(&self, version: [u8; 32]) -> StoreResult<Option<CompiledProgram>>;

    async fn store_artifact(
        &self,
        artifact: &ExecutableWorkflow,
    ) -> std::result::Result<(), ArtifactStoreError>;

    async fn load_artifact(
        &self,
        hash: ArtifactHash,
    ) -> std::result::Result<Option<ExecutableWorkflow>, ArtifactStoreError>;

    /// T3 — Store a serialised WorkflowExecutionPlan keyed by BLAKE3 hash.
    /// Idempotent: ON CONFLICT DO NOTHING in Postgres; no-op if already present in MemoryStore.
    async fn store_plan(&self, plan_hash: [u8; 32], plan_json: &str) -> StoreResult<()>;

    /// T3 — Load a WorkflowExecutionPlan by BLAKE3 hash. Returns None if not found.
    async fn load_plan(&self, plan_hash: [u8; 32]) -> StoreResult<Option<String>>;
}

/// Read-only event, snapshot, journal, and payload-history access.
#[async_trait]
pub trait JournalReader: Send + Sync {
    // ── Event log (append-only) ──

    /// Append an event and return its sequence number.
    async fn read_events(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        from_seq: u64,
    ) -> StoreResult<Vec<(u64, RuntimeEvent)>>;

    /// Load the authoritative, versioned aggregate checkpoint.
    async fn load_snapshot_envelope(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Option<SnapshotEnvelope>>;

    /// Read the deterministic journal tail in revision order.
    async fn read_journal(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        after_revision: Option<u64>,
    ) -> StoreResult<Vec<JournalRecord>>;

    // ── Payload history (for PITR) ──

    async fn load_payload_version(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        hash: &[u8; 32],
    ) -> StoreResult<Option<String>>;
}

/// Administrative, tenant-directory, and projection/catalogue access.
#[async_trait]
pub trait AdminProjectionStore: Send + Sync {
    // ── Template catalog ──

    async fn store_template(
        &self,
        name: &str,
        version: u32,
        plan_hash: [u8; 32],
        dsl_body: &str,
    ) -> StoreResult<()>;

    async fn load_template_version(
        &self,
        name: &str,
        version: u32,
    ) -> StoreResult<Option<(String, [u8; 32])>>;

    async fn load_latest_template_version(
        &self,
        name: &str,
    ) -> StoreResult<Option<(u32, String, [u8; 32])>>;

    async fn list_templates(&self) -> StoreResult<Vec<TemplateSummary>>;

    /// Lightweight readiness probe for the backing store.
    async fn health_check(&self) -> StoreResult<()>;

    // ── Tenant directory ──

    /// Register a tenant on first use. Idempotent — safe to call on every
    /// process start. The tenants table has no RLS so the scheduler can
    /// enumerate all tenants without a tenant context set.
    async fn ensure_tenant(&self, tenant_id: &TenantId) -> StoreResult<()>;

    /// List all known tenant IDs. Used by the scheduler to enumerate tenants
    /// for per-tenant tick batches.
    async fn list_tenants(&self) -> StoreResult<Vec<String>>;

    /// List tenant IDs assigned to a specific pool. Returns an empty vec if the
    /// pool doesn't exist or has no tenants. Used by bpmn-controller to query
    /// pool membership.
    async fn list_tenants_in_pool(&self, pool_id: &str) -> StoreResult<Vec<String>>;

    // ── Designer sessions (EOP-SAGE-REPL-BPMN-001 T1) ──
    // Tenant-scoped, append-only event log. NOTE: unlike the template
    // catalog (which predates tenancy on this surface), sessions carry
    // tenant_id from birth; RLS alignment for the postgres tables rides
    // the T5 deployment review.

    /// Create a session with its seed source as Revision event seq 0.
    /// `id` is caller-generated (deterministic-id discipline).
    async fn create_design_session(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
        name: &str,
        dsl_source: &str,
    ) -> StoreResult<()>;

    async fn load_design_session(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
    ) -> StoreResult<Option<DesignSessionRecord>>;

    async fn list_design_sessions(
        &self,
        tenant_id: &TenantId,
    ) -> StoreResult<Vec<DesignSessionSummary>>;

    /// Append one event; returns the assigned sequence number.
    async fn append_design_session_event(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
        kind: &DesignSessionEventKind,
    ) -> StoreResult<u64>;

    /// Mark saved-as-template, recording the template pin on the session.
    async fn mark_design_session_saved(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
        template_name: &str,
        template_version: u32,
        plan_hash: [u8; 32],
    ) -> StoreResult<()>;
}

/// Composite capability object used where an engine needs all four store surfaces.
pub trait WorkflowStore:
    RuntimeStore + ArtifactRepository + JournalReader + AdminProjectionStore
{
}

impl<T> WorkflowStore for T where
    T: RuntimeStore + ArtifactRepository + JournalReader + AdminProjectionStore
{
}

/// Test and migration fixture helper that still exercises the sole writable
/// store boundary. Production start paths should add the root fiber and start
/// event to the same transition.
pub async fn commit_initial_snapshot(
    store: &dyn WorkflowStore,
    instance: ProcessInstance,
) -> std::result::Result<CommitOutcome, CommitError> {
    let tenant_id = TenantId::new(instance.tenant_id.clone())
        .map_err(|error| CommitError::Integrity(error.to_string()))?;
    let claim = Claim::new(tenant_id, instance.instance_id, 0, 0);
    store
        .commit_transition(&claim, &TransitionBuilder::new(instance).build())
        .await
}

/// Commit a complete fixture/admin snapshot through the fenced write path.
pub async fn commit_snapshot(
    store: &dyn WorkflowStore,
    owner: &str,
    instance: ProcessInstance,
) -> std::result::Result<CommitOutcome, CommitError> {
    let tenant_id = TenantId::new(instance.tenant_id.clone())
        .map_err(|error| CommitError::Integrity(error.to_string()))?;
    let instance_id = instance.instance_id;
    let claim = store
        .claim_instance_for_transition(&tenant_id, instance_id, owner, 30_000)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?
        .ok_or(CommitError::Conflict)?;
    let outcome = store
        .commit_transition(&claim, &TransitionBuilder::new(instance).build())
        .await?;
    store
        .release_instance_transition(&tenant_id, instance_id, owner)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?;
    Ok(outcome)
}

#[derive(Debug, Clone)]
pub enum TickOperation {
    SaveFiber {
        fiber: Fiber,
    },
    DeleteFiber {
        fiber_id: Uuid,
    },
    DeleteAllFibers,
    JoinArrive {
        join_id: JoinId,
    },
    JoinReset {
        join_id: JoinId,
    },
    EnqueueJob {
        job: JobActivation,
    },
    AckJob {
        job_key: String,
    },
    CancelJobsForInstance,
    AppendEvent {
        event: RuntimeEvent,
    },
    SaveIncident {
        incident: Incident,
    },
    SaveInstance {
        instance: ProcessInstance,
    },
    UpdateInstanceState {
        state: ProcessState,
    },
    ReleaseBufferedMessageClaim {
        message: ClaimedBufferedMessage,
    },
    ConsumeBufferedMessage {
        message: ClaimedBufferedMessage,
    },
    InsertPendingInvocation {
        pending: crate::pending::PendingInvocation,
    },
    InsertOutbox {
        id: Uuid,
        target_domain: String,
        target_endpoint: String,
        payload: Vec<u8>,
        idempotency_key: Uuid,
        callout_id: Uuid,
    },
    TakePendingInvocation {
        execution_id: Uuid,
    },
    StartChild {
        child: Box<ChildStart>,
    },
    ScheduleTimer {
        effect: DurableEffect,
    },
    TimerMutation {
        mutation: TimerMutation,
    },
}

#[derive(Debug, Clone)]
pub struct TransactionContext {
    pub instance_id: Uuid,
    pub tenant_id: String,
    pub ops: Vec<TickOperation>,
}

impl TransactionContext {
    pub fn new(instance_id: Uuid, tenant_id: String) -> Self {
        Self {
            instance_id,
            tenant_id,
            ops: Vec::new(),
        }
    }

    pub fn add_op(&mut self, op: TickOperation) {
        self.ops.push(op);
    }

    pub fn get_join_count(&self, join_id: JoinId, base_count: u16) -> u16 {
        let mut count = base_count;
        for op in &self.ops {
            match op {
                TickOperation::JoinArrive { join_id: jid } if *jid == join_id => {
                    count += 1;
                }
                TickOperation::JoinReset { join_id: jid } if *jid == join_id => {
                    count = 0;
                }
                _ => {}
            }
        }
        count
    }
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("TakePendingInvocation: already consumed")]
pub struct AlreadyConsumedError;

/// T4 compatibility bridge. T7 deletes `TickOperation` and this conversion
/// after every producer builds `Transition` directly.
#[doc(hidden)]
pub fn transition_from_tick_ops(current: &ProcessInstance, ops: &[TickOperation]) -> Transition {
    let mut snapshot = current.clone();
    for op in ops {
        match op {
            TickOperation::SaveInstance { instance } => snapshot = instance.clone(),
            TickOperation::UpdateInstanceState { state } => snapshot.state = state.clone(),
            _ => {}
        }
    }

    let mut builder = TransitionBuilder::new(snapshot);
    let mut delete_all_fibers = false;
    let delete_all_joins = false;
    let mut cancel_jobs = false;
    for op in ops {
        builder = match op {
            TickOperation::SaveFiber { fiber } => builder.upsert_fiber(fiber.clone()),
            TickOperation::DeleteFiber { fiber_id } => builder.delete_fiber(*fiber_id),
            TickOperation::DeleteAllFibers => {
                delete_all_fibers = true;
                builder
            }
            TickOperation::JoinArrive { join_id } => {
                builder.join_mutation(JoinMutation::Arrive(*join_id))
            }
            TickOperation::JoinReset { join_id } => {
                builder.join_mutation(JoinMutation::Reset(*join_id))
            }
            TickOperation::EnqueueJob { job } => builder.enqueue_job(job.clone()),
            TickOperation::AckJob { job_key } => builder.ack_job(job_key.clone()),
            TickOperation::CancelJobsForInstance => {
                cancel_jobs = true;
                builder
            }
            TickOperation::AppendEvent { event } => builder.event(event.clone()),
            TickOperation::SaveIncident { incident } => builder.incident(incident.clone()),
            TickOperation::ReleaseBufferedMessageClaim { message } => {
                builder.buffered_message(BufferedMessageMutation::Release(message.clone()))
            }
            TickOperation::ConsumeBufferedMessage { message } => {
                builder.buffered_message(BufferedMessageMutation::Consume(message.clone()))
            }
            TickOperation::InsertPendingInvocation { pending } => {
                let identity = PendingInvocationIdentity {
                    callout_id: pending.callout_id,
                    process_instance_id: pending.process_instance_id,
                    node_id: pending.node_id.clone(),
                    target_domain: pending.target_domain.clone(),
                    verb_id: pending.verb_id.clone(),
                    idempotency_key: pending.idempotency_key,
                };
                let timing = PendingInvocationTiming {
                    execution_id: pending.execution_id,
                    submitted_at: pending.submitted_at.timestamp_millis(),
                    ack_received_at: pending
                        .ack_received_at
                        .map(|value| value.timestamp_millis()),
                    timeout_at: pending.timeout_at.map(|value| value.timestamp_millis()),
                };
                builder.pending_invocation(PendingInvocationWrite::new(identity, timing))
            }
            TickOperation::InsertOutbox {
                id,
                target_domain,
                target_endpoint,
                payload,
                idempotency_key,
                callout_id,
            } => builder.outbox(OutboxWrite::new(
                *id,
                target_domain.clone(),
                target_endpoint.clone(),
                payload.clone(),
                *idempotency_key,
                *callout_id,
            )),
            TickOperation::TakePendingInvocation { execution_id } => {
                builder.take_pending(*execution_id)
            }
            TickOperation::StartChild { child } => builder.child_start((**child).clone()),
            TickOperation::ScheduleTimer { effect } => builder.effect(effect.clone()),
            TickOperation::TimerMutation { mutation } => builder.timer_mutation(mutation.clone()),
            TickOperation::SaveInstance { .. } | TickOperation::UpdateInstanceState { .. } => {
                builder
            }
        };
    }
    if delete_all_fibers || delete_all_joins || cancel_jobs {
        builder = builder.terminal_cleanup(TerminalCleanup::new(
            delete_all_fibers,
            delete_all_joins,
            cancel_jobs,
        ));
    }
    builder.build()
}
