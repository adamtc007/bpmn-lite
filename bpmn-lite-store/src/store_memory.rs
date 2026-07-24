use crate::pending::PendingInvocationStore;
#[cfg(test)]
use crate::store::{transition_from_tick_ops, TickOperation};
use crate::store::{AdminProjectionStore, ArtifactRepository, JournalReader, RuntimeStore};
use crate::{ArtifactStoreError, ClaimError, CommitError, CommitOutcome, StoreError, StoreResult};
#[cfg(test)]
type Result<T> = StoreResult<T>;
use async_trait::async_trait;
use bpmn_lite_types::events::RuntimeEvent;
use bpmn_lite_types::*;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use uuid::Uuid;

struct Inner {
    instances: HashMap<Uuid, ProcessInstance>,
    fibers: HashMap<(Uuid, Uuid), Fiber>,
    join_counters: HashMap<(Uuid, JoinId), u16>,
    dedupe: HashMap<String, (JobCompletion, Instant)>,
    message_buffer: HashMap<(String, String, String, String), BufferedMessage>,
    message_buffer_claims: HashMap<(String, String, String, String), (String, i64)>,
    message_buffer_consumed: HashSet<(String, String, String, String)>,
    job_queue: VecDeque<JobActivation>,
    /// Jobs that have been dequeued but not yet acked.
    inflight_jobs: HashMap<String, (JobActivation, Instant)>,
    programs: HashMap<[u8; 32], CompiledProgram>,
    artifacts: HashMap<ArtifactHash, Vec<u8>>,
    artifact_lineage: HashMap<ArtifactHash, ArtifactHash>,
    plans: HashMap<[u8; 32], String>,
    templates: HashMap<(String, u32), (String, [u8; 32], String)>, // (name, version) -> (dsl, plan_hash, created_at)
    dead_letter: HashMap<(u32, String), (Vec<u8>, u64)>,
    events: HashMap<Uuid, Vec<(u64, RuntimeEvent)>>,
    event_seq: HashMap<Uuid, u64>,
    payload_history: HashMap<(Uuid, [u8; 32]), String>,
    incidents: HashMap<Uuid, Vec<Incident>>,
    concurrency_tables: HashMap<Uuid, ConcurrencyTable>,
    transition_leases: HashMap<Uuid, (String, Instant, u64)>,
    revisions: HashMap<Uuid, u64>,
    durable_effects: HashMap<EffectId, MemoryEffect>,
    timers: HashMap<EffectId, MemoryTimer>,
    outbox: HashMap<Uuid, (String, String, Vec<u8>, Uuid, Uuid)>,
    starts: HashMap<(String, Uuid), Uuid>,
    snapshots: HashMap<Uuid, SnapshotEnvelope>,
    journals: HashMap<Uuid, Vec<JournalRecord>>,
}

#[derive(Clone)]
struct MemoryTimer {
    tenant_id: TenantId,
    instance_id: Uuid,
    fiber_id: Uuid,
    due_at: u64,
    kind: TimerKind,
    repeat_spec: Option<TimerRepeatSpec>,
    state: MemoryTimerState,
    claim: Option<(String, Uuid, u64)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemoryTimerState {
    Armed,
    Consumed,
    Cancelled,
}

#[derive(Clone)]
struct MemoryEffect {
    tenant_id: TenantId,
    instance_id: Uuid,
    effect: DurableEffect,
    state: MemoryEffectState,
    claim: Option<(String, Uuid, u64)>,
    response: Option<EffectResponse>,
    attempt: u32,
    policy_version: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemoryEffectState {
    Pending,
    Dispatching,
    Accepted,
    Terminal,
}

/// In-memory implementation of `WorkflowStore` for POC/testing.
pub struct MemoryStore {
    inner: RwLock<Inner>,
    pub pending_store: crate::pending::MemoryPendingInvocationStore,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                instances: HashMap::new(),
                fibers: HashMap::new(),
                join_counters: HashMap::new(),
                dedupe: HashMap::new(),
                message_buffer: HashMap::new(),
                message_buffer_claims: HashMap::new(),
                message_buffer_consumed: HashSet::new(),
                job_queue: VecDeque::new(),
                inflight_jobs: HashMap::new(),
                programs: HashMap::new(),
                artifacts: HashMap::new(),
                artifact_lineage: HashMap::new(),
                plans: HashMap::new(),
                templates: HashMap::new(),
                dead_letter: HashMap::new(),
                events: HashMap::new(),
                event_seq: HashMap::new(),
                payload_history: HashMap::new(),
                incidents: HashMap::new(),
                concurrency_tables: HashMap::new(),
                transition_leases: HashMap::new(),
                revisions: HashMap::new(),
                durable_effects: HashMap::new(),
                timers: HashMap::new(),
                outbox: HashMap::new(),
                starts: HashMap::new(),
                snapshots: HashMap::new(),
                journals: HashMap::new(),
            }),
            pending_store: crate::pending::MemoryPendingInvocationStore::new(),
        }
    }

    /// Test-only visibility into the in-memory outbox: raw encoded
    /// `(target_domain, target_endpoint, payload)` rows, most-recent-last
    /// insertion order is not guaranteed (`HashMap`-backed) — callers that
    /// care about a specific row should filter/assert rather than index.
    /// Added for G6a (EOP-DESIGN-CONTROLPLANE-G6A-SNAPSHOT-PIN-CARRIER-001
    /// §8) so `plan_walker_tests.rs` can decode a real submitted
    /// `InvocationRequest` and assert its `snapshot_pin` without a live
    /// Postgres-backed `dsl-bus-storage` outbox.
    pub async fn outbox_rows_for_test(&self) -> Vec<(String, String, Vec<u8>)> {
        self.inner
            .read()
            .await
            .outbox
            .values()
            .map(
                |(target_domain, target_endpoint, payload, _idem, _callout)| {
                    (
                        target_domain.clone(),
                        target_endpoint.clone(),
                        payload.clone(),
                    )
                },
            )
            .collect()
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Serialize a `Value` into a deterministic string key for dead-letter lookup.
fn value_key(v: &Value) -> String {
    match v {
        Value::Bool(b) => format!("b:{b}"),
        Value::I64(n) => format!("i:{n}"),
        Value::Str(s) => format!("s:{s}"),
        Value::Ref(r) => format!("r:{r}"),
        // §18 ruling K Part 2: `Value::Array` is new. Same "a:" + hex of
        // canonical bytes convention as `bpmn-lite-kernel`'s own
        // `value_key` — deterministic and unambiguous, distinct from the
        // scalar prefixes above, rather than a panic on an unreachable-
        // until-now match arm.
        Value::Array(_) => format!(
            "a:{}",
            v.to_canonical_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[async_trait]
impl RuntimeStore for MemoryStore {
    // ── Instance ──

    async fn load_instance(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
    ) -> StoreResult<Option<ProcessInstance>> {
        let r = self.inner.read().await;
        Ok(r.instances
            .get(&id)
            .filter(|instance| instance.tenant_id == tenant_id.as_str())
            .cloned())
    }

    // ── Fibers ──

    async fn load_fiber(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        fiber_id: Uuid,
    ) -> StoreResult<Option<Fiber>> {
        let r = self.inner.read().await;
        if r.instances
            .get(&instance_id)
            .is_none_or(|instance| instance.tenant_id != tenant_id.as_str())
        {
            return Ok(None);
        }
        Ok(r.fibers.get(&(instance_id, fiber_id)).cloned())
    }

    async fn load_fibers(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Vec<Fiber>> {
        let r = self.inner.read().await;
        if r.instances
            .get(&instance_id)
            .is_none_or(|instance| instance.tenant_id != tenant_id.as_str())
        {
            return Ok(Vec::new());
        }
        Ok(r.fibers
            .iter()
            .filter(|((iid, _), _)| *iid == instance_id)
            .map(|(_, f)| f.clone())
            .collect())
    }

    // ── Join barriers ──

    // ── Dedupe cache ──

    async fn dedupe_get(
        &self,
        _tenant_id: &TenantId,
        key: &str,
    ) -> StoreResult<Option<JobCompletion>> {
        let r = self.inner.read().await;
        Ok(r.dedupe.get(key).map(|(c, _)| c.clone()))
    }

    // ── Job queue ──

    async fn dequeue_jobs(
        &self,
        task_types: &[String],
        max: usize,
        tenant_id: &TenantId,
        worker_id: &str,
        lease_ms: u64,
    ) -> StoreResult<Vec<JobActivation>> {
        let mut w = self.inner.write().await;
        let mut result = Vec::new();
        let mut remaining = VecDeque::new();
        let now = now_ms();
        let claim_expires_at = now + lease_ms as i64;

        while let Some(mut job) = w.job_queue.pop_front() {
            let job_tenant = w
                .instances
                .get(&job.process_instance_id)
                .map(|instance| instance.tenant_id.clone());
            let same_tenant = job_tenant
                .as_ref()
                .map(|instance_tenant| instance_tenant == tenant_id.as_str())
                .unwrap_or(false);
            let due = job
                .not_before
                .map(|not_before| not_before <= now)
                .unwrap_or(true);
            if result.len() < max && same_tenant && due && task_types.contains(&job.task_type) {
                if let Some(job_tenant) = job_tenant {
                    job.tenant_id = job_tenant;
                }
                job.worker_id = worker_id.to_string();
                job.claim_token = Uuid::now_v7().to_string();
                job.claim_expires_at = Some(claim_expires_at);
                job.attempt_count += 1;
                w.inflight_jobs
                    .insert(job.job_key.clone(), (job.clone(), Instant::now()));
                result.push(job);
            } else {
                remaining.push_back(job);
            }
        }
        w.job_queue = remaining;
        Ok(result)
    }

    async fn validate_job_claim(
        &self,
        _tenant_id: &TenantId,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> StoreResult<bool> {
        let w = self.inner.read().await;
        Ok(w.inflight_jobs
            .get(job_key)
            .map(|(job, _)| {
                job.worker_id == worker_id
                    && job.claim_token == claim_token
                    && job
                        .claim_expires_at
                        .map(|expires_at| expires_at > now_ms())
                        .unwrap_or(false)
            })
            .unwrap_or(false))
    }

    // ── Dead-letter queue ──

    async fn dead_letter_put(
        &self,
        name: u32,
        corr_key: &Value,
        payload: &[u8],
        ttl_ms: u64,
    ) -> StoreResult<()> {
        let mut w = self.inner.write().await;
        let key = (name, value_key(corr_key));
        w.dead_letter.insert(key, (payload.to_vec(), ttl_ms));
        Ok(())
    }

    async fn dead_letter_take(&self, name: u32, corr_key: &Value) -> StoreResult<Option<Vec<u8>>> {
        let mut w = self.inner.write().await;
        let key = (name, value_key(corr_key));
        Ok(w.dead_letter.remove(&key).map(|(data, _)| data))
    }

    async fn claim_buffered_message(
        &self,
        tenant_id: &TenantId,
        message_name: &str,
        correlation_key: &str,
        claim_ms: u64,
    ) -> StoreResult<Option<ClaimedBufferedMessage>> {
        let mut w = self.inner.write().await;
        let now = now_ms();
        let key = w
            .message_buffer
            .iter()
            .filter(|((tenant, name, corr, _), msg)| {
                tenant == tenant_id.as_str()
                    && name == message_name
                    && corr == correlation_key
                    && msg.expires_at > now
            })
            .filter(|(key, _)| {
                w.message_buffer_claims
                    .get(*key)
                    .map(|(_, claim_until)| *claim_until <= now)
                    .unwrap_or(true)
            })
            .min_by_key(|(_, msg)| msg.received_at)
            .map(|(key, msg)| (key.clone(), msg.clone()));

        let Some((key, message)) = key else {
            return Ok(None);
        };
        let claim_token = Uuid::now_v7().to_string();
        let claim_until = now + claim_ms as i64;
        w.message_buffer_claims
            .insert(key, (claim_token.clone(), claim_until));
        Ok(Some(ClaimedBufferedMessage {
            message,
            claim_token,
            claim_until,
        }))
    }

    async fn reclaim_stale_buffered_message_claims(&self) -> StoreResult<u32> {
        let mut w = self.inner.write().await;
        let now = now_ms();
        let before = w.message_buffer_claims.len();
        w.message_buffer_claims
            .retain(|_, (_, claim_until)| *claim_until > now);
        Ok((before - w.message_buffer_claims.len()) as u32)
    }

    async fn prune_expired_messages(&self) -> StoreResult<u32> {
        let mut w = self.inner.write().await;
        let now = now_ms();
        let before = w.message_buffer.len();
        let expired: Vec<_> = w
            .message_buffer
            .iter()
            .filter(|(_, msg)| msg.expires_at <= now)
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            if let Some(msg) = w.message_buffer.remove(&key) {
                if let Some(instance_id) = msg.process_instance_id {
                    let seq = w.event_seq.entry(instance_id).or_insert(0);
                    *seq += 1;
                    let current_seq = *seq;
                    w.events.entry(instance_id).or_default().push((
                        current_seq,
                        RuntimeEvent::BufferedMessageExpired {
                            message_name: msg.message_name,
                            correlation_key: msg.correlation_key,
                            msg_id: msg.msg_id,
                        },
                    ));
                }
            }
            w.message_buffer_claims.remove(&key);
        }
        Ok((before - w.message_buffer.len()) as u32)
    }

    // ── Incidents ──

    async fn load_incidents(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Vec<Incident>> {
        let r = self.inner.read().await;
        if r.instances
            .get(&instance_id)
            .is_none_or(|instance| instance.tenant_id != tenant_id.as_str())
        {
            return Ok(Vec::new());
        }
        Ok(r.incidents.get(&instance_id).cloned().unwrap_or_default())
    }

    // ── Durability maintenance ──

    async fn reclaim_stale_jobs(&self, _timeout_ms: u64) -> StoreResult<u32> {
        let mut w = self.inner.write().await;
        let now = now_ms();
        let stale_keys: Vec<String> = w
            .inflight_jobs
            .iter()
            .filter(|(_, (job, _))| {
                job.claim_expires_at
                    .map(|expires_at| expires_at < now)
                    .unwrap_or(false)
            })
            .map(|(key, _)| key.clone())
            .collect();
        let count = stale_keys.len() as u32;
        for key in stale_keys {
            if let Some((mut job, _)) = w.inflight_jobs.remove(&key) {
                let previous_worker_id = (!job.worker_id.is_empty()).then(|| job.worker_id.clone());
                let process_instance_id = job.process_instance_id;
                if job.retries_remaining > 1 {
                    job.retries_remaining -= 1;
                    job.failure_count += 1;
                    job.worker_id.clear();
                    job.claim_token.clear();
                    job.claim_expires_at = None;
                    w.job_queue.push_back(job);
                }
                let seq = w.event_seq.entry(process_instance_id).or_insert(0);
                *seq += 1;
                let current_seq = *seq;
                w.events.entry(process_instance_id).or_default().push((
                    current_seq,
                    RuntimeEvent::JobReclaimed {
                        job_key: key,
                        previous_worker_id,
                    },
                ));
            }
        }
        Ok(count)
    }

    async fn prune_dedupe_cache(&self, older_than_ms: u64) -> StoreResult<u32> {
        let mut w = self.inner.write().await;
        let threshold = std::time::Duration::from_millis(older_than_ms);
        let now = Instant::now();
        let before = w.dedupe.len();
        w.dedupe
            .retain(|_, (_, created_at)| now.duration_since(*created_at) <= threshold);
        Ok((before - w.dedupe.len()) as u32)
    }

    async fn list_running_instances(&self, tenant_id: &TenantId) -> StoreResult<Vec<Uuid>> {
        let r = self.inner.read().await;
        Ok(r.instances
            .iter()
            .filter(|(_, inst)| inst.state.is_schedulable() && inst.tenant_id == tenant_id.as_str())
            .map(|(id, _)| *id)
            .collect())
    }

    async fn claim_running_instances(
        &self,
        tenant_id: &TenantId,
        _owner: &str,
        limit: usize,
        _lease_ms: u64,
    ) -> StoreResult<Vec<Uuid>> {
        let ids = self.list_running_instances(tenant_id).await?;
        Ok(ids.into_iter().take(limit).collect())
    }

    async fn claim_instance_for_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
    ) -> std::result::Result<Option<Claim>, ClaimError> {
        let mut w = self.inner.write().await;
        let Some(instance) = w.instances.get(&instance_id) else {
            return Ok(None);
        };
        if instance.tenant_id != tenant_id.as_str() {
            return Ok(None);
        }
        let current_revision = *w.revisions.get(&instance_id).unwrap_or(&0);
        let integrity_result = (|| {
            let snapshot = w
                .snapshots
                .get(&instance_id)
                .ok_or_else(|| "missing snapshot envelope".to_string())?;
            let journal = w
                .journals
                .get(&instance_id)
                .and_then(|records| records.last())
                .ok_or_else(|| "missing journal head".to_string())?;
            if snapshot.revision() != current_revision
                || snapshot.state().instance().instance_id != instance_id
                || snapshot.state().instance().tenant_id != tenant_id.as_str()
                || snapshot.state().instance().bytecode_version != instance.bytecode_version
                || journal.new_revision() != current_revision
                || journal.artifact_hash() != instance.bytecode_version
                || journal.state_hash()
                    != snapshot.state_hash().map_err(|error| error.to_string())?
            {
                return Err("snapshot and journal head diverge".to_string());
            }
            Ok::<(), String>(())
        })();
        if let Err(reason) = integrity_result {
            if let Some(instance) = w.instances.get_mut(&instance_id) {
                instance.quarantine_state = Some("replay_integrity_violation".to_string());
            }
            w.transition_leases.remove(&instance_id);
            return Err(ClaimError::Integrity(reason));
        }

        let now = Instant::now();
        let lease_until = now + Duration::from_millis(lease_ms);
        match w.transition_leases.get(&instance_id) {
            Some((current_owner, expires_at, _)) if current_owner != owner && *expires_at > now => {
                Ok(None)
            }
            _ => {
                let is_renewal = matches!(
                    w.transition_leases.get(&instance_id),
                    Some((current_owner, expires_at, _)) if current_owner == owner && *expires_at > now
                );
                let previous_fence = w
                    .transition_leases
                    .get(&instance_id)
                    .map(|(_, _, fence)| *fence)
                    .unwrap_or(0);
                let fence = if is_renewal {
                    previous_fence
                } else {
                    previous_fence.checked_add(1).ok_or_else(|| {
                        ClaimError::Invalid("transition fence overflow".to_string())
                    })?
                };
                w.transition_leases
                    .insert(instance_id, (owner.to_string(), lease_until, fence));
                let tenant_id = tenant_id.clone();
                Ok(Some(Claim::new(
                    tenant_id,
                    instance_id,
                    current_revision,
                    fence,
                )))
            }
        }
    }

    async fn commit_transition(
        &self,
        claim: &Claim,
        transition: &Transition,
    ) -> std::result::Result<CommitOutcome, CommitError> {
        for execution_id in transition.pending_take() {
            let pending = self
                .pending_store
                .lookup_by_execution_id(claim.tenant_id(), *execution_id)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
            if pending.is_none() {
                return Err(CommitError::Conflict);
            }
        }
        for pending in transition.pending_invocations() {
            let submitted_at = chrono::DateTime::from_timestamp_millis(pending.submitted_at())
                .ok_or_else(|| CommitError::Integrity("invalid submitted_at".to_string()))?;
            let ack_received_at = pending
                .ack_received_at()
                .map(|value| {
                    chrono::DateTime::from_timestamp_millis(value).ok_or_else(|| {
                        CommitError::Integrity("invalid ack_received_at".to_string())
                    })
                })
                .transpose()?;
            let timeout_at = pending
                .timeout_at()
                .map(|value| {
                    chrono::DateTime::from_timestamp_millis(value)
                        .ok_or_else(|| CommitError::Integrity("invalid timeout_at".to_string()))
                })
                .transpose()?;
            let record = crate::pending::PendingInvocation {
                tenant_id: claim.tenant_id().clone(),
                callout_id: pending.callout_id(),
                process_instance_id: pending.process_instance_id(),
                node_id: pending.node_id().to_string(),
                target_domain: pending.target_domain().to_string(),
                verb_id: pending.verb_id().to_string(),
                idempotency_key: pending.idempotency_key(),
                execution_id: pending.execution_id(),
                submitted_at,
                ack_received_at,
                timeout_at,
            };
            self.pending_store
                .insert(record)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?;
        }
        for execution_id in transition.pending_take() {
            self.pending_store
                .take_by_execution_id(claim.tenant_id(), *execution_id)
                .await
                .map_err(|error| CommitError::Unavailable(error.to_string()))?
                .ok_or(CommitError::Conflict)?;
        }

        let mut w = self.inner.write().await;
        let instance_id = claim.instance_id();
        let is_start = !w.instances.contains_key(&instance_id)
            && claim.expected_revision() == 0
            && claim.fence() == 0;
        if !is_start {
            let current = w
                .instances
                .get(&instance_id)
                .ok_or_else(|| CommitError::Integrity("instance not found".to_string()))?;
            if current.tenant_id != claim.tenant_id().as_str() {
                return Err(CommitError::Integrity(
                    "claim tenant does not own instance".to_string(),
                ));
            }
            let current_fence = w
                .transition_leases
                .get(&instance_id)
                .map(|(_, _, fence)| *fence)
                .unwrap_or(0);
            if current_fence != claim.fence() {
                return Err(CommitError::StaleFence);
            }
            let current_revision = *w.revisions.get(&instance_id).unwrap_or(&0);
            if current_revision != claim.expected_revision() {
                return Err(CommitError::Conflict);
            }
        }
        if transition.next_snapshot().tenant_id != claim.tenant_id().as_str()
            || transition.next_snapshot().instance_id != instance_id
        {
            return Err(CommitError::Integrity(
                "claim and transition aggregate identity differ".to_string(),
            ));
        }
        if let Some(start) = transition.start_dedupe() {
            let command = start.command();
            if command.tenant_id() != claim.tenant_id()
                || command.instance_id() != instance_id
                || command.artifact_hash() != transition.next_snapshot().bytecode_version
                || command.initial_payload_hash() != transition.next_snapshot().domain_payload_hash
            {
                return Err(CommitError::Integrity(
                    "start command lineage does not match initial snapshot".to_string(),
                ));
            }
            if w.starts.contains_key(&(
                claim.tenant_id().as_str().to_string(),
                command.idempotency_key(),
            )) {
                return Ok(CommitOutcome::IdempotentNoOp);
            }
        }
        for write in transition.dedupe() {
            if w.dedupe.contains_key(write.key()) {
                return Ok(CommitOutcome::IdempotentNoOp);
            }
        }
        for mutation in transition.buffered_messages() {
            if let BufferedMessageMutation::Insert(message)
            | BufferedMessageMutation::Deliver(message) = mutation
            {
                let key = (
                    message.tenant_id.clone(),
                    message.message_name.clone(),
                    message.correlation_key.clone(),
                    message.msg_id.clone(),
                );
                if w.message_buffer.contains_key(&key) || w.message_buffer_consumed.contains(&key) {
                    return Ok(CommitOutcome::IdempotentNoOp);
                }
            } else if let BufferedMessageMutation::Consume(message) = mutation {
                let key = (
                    message.message.tenant_id.clone(),
                    message.message.message_name.clone(),
                    message.message.correlation_key.clone(),
                    message.message.msg_id.clone(),
                );
                match w.message_buffer_claims.get(&key) {
                    Some((token, _)) if token == &message.claim_token => {}
                    _ => return Err(CommitError::Conflict),
                }
            }
        }
        for mutation in transition.timer_mutations() {
            let (timer_id, claim_token) = match mutation {
                TimerMutation::Consume {
                    timer_id,
                    claim_token,
                }
                | TimerMutation::Rearm {
                    timer_id,
                    claim_token,
                    ..
                } => (*timer_id, *claim_token),
                TimerMutation::CancelRace { .. } | TimerMutation::V2CancelRace { .. } => continue,
            };
            let Some(timer) = w.timers.get(&timer_id) else {
                return Ok(CommitOutcome::IdempotentNoOp);
            };
            let claim_matches = matches!(
                timer.claim,
                Some((_, token, _)) if token == claim_token
            );
            if timer.state != MemoryTimerState::Armed
                || &timer.tenant_id != claim.tenant_id()
                || timer.instance_id != instance_id
                || !claim_matches
            {
                return Ok(CommitOutcome::IdempotentNoOp);
            }
        }

        let mut snapshot = transition.next_snapshot().clone();
        if let Some(state) = transition.state_override() {
            snapshot.state = state.clone();
        }
        w.instances.insert(instance_id, snapshot.clone());
        if let Some(start) = transition.start_dedupe() {
            w.starts.insert(
                (
                    claim.tenant_id().as_str().to_string(),
                    start.command().idempotency_key(),
                ),
                instance_id,
            );
        }
        w.payload_history.insert(
            (instance_id, snapshot.domain_payload_hash),
            snapshot.domain_payload.to_string(),
        );
        for fiber in transition.fibers_upsert() {
            w.fibers
                .insert((instance_id, fiber.fiber_id), fiber.clone());
        }
        for fiber_id in transition.fibers_delete() {
            w.fibers.remove(&(instance_id, *fiber_id));
        }
        for mutation in transition.join_mutations() {
            match mutation {
                JoinMutation::Arrive(join_id) => {
                    let count = w.join_counters.entry((instance_id, *join_id)).or_insert(0);
                    *count += 1;
                }
                JoinMutation::Reset(join_id) => {
                    w.join_counters.insert((instance_id, *join_id), 0);
                }
            }
        }
        for job in transition.jobs_enqueue() {
            if !w
                .job_queue
                .iter()
                .any(|existing| existing.job_key == job.job_key)
                && !w.inflight_jobs.contains_key(&job.job_key)
            {
                w.job_queue.push_back(job.clone());
            }
        }
        for job_key in transition.jobs_ack() {
            w.inflight_jobs.remove(job_key);
            w.job_queue.retain(|job| &job.job_key != job_key);
        }
        for mutation in transition.job_mutations() {
            match mutation {
                JobMutation::RetryClaimed {
                    job_key,
                    worker_id,
                    claim_token,
                    error_class,
                    error_message: _,
                    not_before_ms,
                } => {
                    let Some((mut job, _)) = w.inflight_jobs.remove(job_key) else {
                        return Err(CommitError::Conflict);
                    };
                    if job.worker_id != *worker_id || job.claim_token != *claim_token {
                        w.inflight_jobs
                            .insert(job.job_key.clone(), (job, Instant::now()));
                        return Err(CommitError::Conflict);
                    }
                    job.worker_id.clear();
                    job.claim_token.clear();
                    job.claim_expires_at = None;
                    job.not_before = Some(*not_before_ms);
                    job.failure_count = job.failure_count.saturating_add(1);
                    job.retries_remaining = job.retries_remaining.saturating_sub(1);
                    job.orch_flags.insert(
                        "last_error_class".to_string(),
                        Value::Str(error_class.len() as u32),
                    );
                    w.job_queue.push_back(job);
                }
                JobMutation::DeadLetterClaimed {
                    job_key,
                    worker_id,
                    claim_token,
                    ..
                } => {
                    let Some((job, _)) = w.inflight_jobs.remove(job_key) else {
                        return Err(CommitError::Conflict);
                    };
                    if job.worker_id != *worker_id || job.claim_token != *claim_token {
                        w.inflight_jobs
                            .insert(job.job_key.clone(), (job, Instant::now()));
                        return Err(CommitError::Conflict);
                    }
                }
            }
        }
        if transition.terminal_cleanup().delete_all_fibers() {
            w.fibers.retain(|(id, _), _| *id != instance_id);
            for timer in w.timers.values_mut() {
                if timer.instance_id == instance_id && timer.state == MemoryTimerState::Armed {
                    timer.state = MemoryTimerState::Cancelled;
                    timer.claim = None;
                }
            }
        }
        if transition.terminal_cleanup().delete_all_joins() {
            w.join_counters.retain(|(id, _), _| *id != instance_id);
        }
        if transition.terminal_cleanup().cancel_jobs() {
            w.job_queue
                .retain(|job| job.process_instance_id != instance_id);
            w.inflight_jobs
                .retain(|_, (job, _)| job.process_instance_id != instance_id);
        }
        for write in transition.dedupe() {
            w.dedupe.insert(
                write.key().to_string(),
                (write.completion().clone(), Instant::now()),
            );
        }
        for incident in transition.incidents() {
            let incidents = w.incidents.entry(instance_id).or_default();
            if let Some(existing) = incidents
                .iter_mut()
                .find(|existing| existing.incident_id == incident.incident_id)
            {
                *existing = incident.clone();
            } else {
                incidents.push(incident.clone());
            }
        }
        for mutation in transition.buffered_messages() {
            match mutation {
                BufferedMessageMutation::Insert(message) => {
                    let key = (
                        message.tenant_id.clone(),
                        message.message_name.clone(),
                        message.correlation_key.clone(),
                        message.msg_id.clone(),
                    );
                    w.message_buffer.insert(key, message.clone());
                }
                BufferedMessageMutation::Deliver(message) => {
                    let key = (
                        message.tenant_id.clone(),
                        message.message_name.clone(),
                        message.correlation_key.clone(),
                        message.msg_id.clone(),
                    );
                    w.message_buffer_consumed.insert(key);
                }
                BufferedMessageMutation::Release(message) => {
                    let key = (
                        message.message.tenant_id.clone(),
                        message.message.message_name.clone(),
                        message.message.correlation_key.clone(),
                        message.message.msg_id.clone(),
                    );
                    w.message_buffer_claims.remove(&key);
                }
                BufferedMessageMutation::Consume(message) => {
                    let key = (
                        message.message.tenant_id.clone(),
                        message.message.message_name.clone(),
                        message.message.correlation_key.clone(),
                        message.message.msg_id.clone(),
                    );
                    w.message_buffer.remove(&key);
                    w.message_buffer_claims.remove(&key);
                    w.message_buffer_consumed.insert(key);
                }
            }
        }
        for outbox in transition.outbox() {
            w.outbox.entry(outbox.id()).or_insert_with(|| {
                (
                    outbox.target_domain().to_string(),
                    outbox.target_endpoint().to_string(),
                    outbox.payload().to_vec(),
                    outbox.idempotency_key(),
                    outbox.callout_id(),
                )
            });
        }
        for effect in transition.effects() {
            match effect {
                DurableEffect::ScheduleTimer {
                    timer_id,
                    fiber_id,
                    due_at,
                    kind,
                    repeat_spec,
                } => {
                    if let Some(existing) = w.timers.get(timer_id) {
                        if &existing.tenant_id != claim.tenant_id()
                            || existing.instance_id != instance_id
                            || existing.fiber_id != *fiber_id
                            || existing.kind != *kind
                        {
                            return Err(CommitError::Integrity(
                                "deterministic timer identity collision".to_string(),
                            ));
                        }
                    } else {
                        w.timers.insert(
                            *timer_id,
                            MemoryTimer {
                                tenant_id: claim.tenant_id().clone(),
                                instance_id,
                                fiber_id: *fiber_id,
                                due_at: *due_at,
                                kind: kind.clone(),
                                repeat_spec: repeat_spec.clone(),
                                state: MemoryTimerState::Armed,
                                claim: None,
                            },
                        );
                    }
                }
                DurableEffect::Invoke { effect_id, .. } => {
                    if let Some(existing) = w.durable_effects.get(effect_id) {
                        if existing.tenant_id != *claim.tenant_id()
                            || existing.instance_id != instance_id
                            || existing.effect != *effect
                        {
                            return Err(CommitError::Integrity(
                                "deterministic effect identity collision".to_string(),
                            ));
                        }
                    } else {
                        w.durable_effects.insert(
                            *effect_id,
                            MemoryEffect {
                                tenant_id: claim.tenant_id().clone(),
                                instance_id,
                                effect: effect.clone(),
                                state: MemoryEffectState::Pending,
                                claim: None,
                                response: None,
                                attempt: 0,
                                policy_version: 1,
                            },
                        );
                    }
                }
            }
        }
        for mutation in transition.effect_mutations() {
            let Some(effect) = w.durable_effects.get_mut(&mutation.effect_id()) else {
                return Err(CommitError::Conflict);
            };
            if effect.tenant_id != *claim.tenant_id() || effect.instance_id != instance_id {
                return Err(CommitError::Conflict);
            }
            effect.state = MemoryEffectState::Terminal;
            effect.claim = None;
            effect.response = None;
        }
        for mutation in transition.timer_mutations() {
            match mutation {
                TimerMutation::Consume { timer_id, .. } => {
                    if let Some(timer) = w.timers.get_mut(timer_id) {
                        timer.state = MemoryTimerState::Consumed;
                        timer.claim = None;
                    }
                }
                TimerMutation::Rearm {
                    timer_id,
                    due_at,
                    repeat_spec,
                    ..
                } => {
                    if let Some(timer) = w.timers.get_mut(timer_id) {
                        timer.due_at = *due_at;
                        timer.repeat_spec = Some(repeat_spec.clone());
                        timer.state = MemoryTimerState::Armed;
                        timer.claim = None;
                    }
                }
                TimerMutation::CancelRace {
                    fiber_id,
                    race_id,
                    except,
                } => {
                    for (timer_id, timer) in &mut w.timers {
                        let same_race = matches!(
                            &timer.kind,
                            TimerKind::Race { race_id: current, .. } if *current == *race_id
                        );
                        if *timer_id != *except
                            && timer.instance_id == instance_id
                            && timer.fiber_id == *fiber_id
                            && same_race
                            && timer.state == MemoryTimerState::Armed
                        {
                            timer.state = MemoryTimerState::Cancelled;
                            timer.claim = None;
                        }
                    }
                }
                TimerMutation::V2CancelRace {
                    fiber_id,
                    record_id,
                    except,
                } => {
                    for (timer_id, timer) in &mut w.timers {
                        let same_race = matches!(
                            &timer.kind,
                            TimerKind::V2Race { record_id: current, .. } if *current == *record_id
                        );
                        if *timer_id != *except
                            && timer.instance_id == instance_id
                            && timer.fiber_id == *fiber_id
                            && same_race
                            && timer.state == MemoryTimerState::Armed
                        {
                            timer.state = MemoryTimerState::Cancelled;
                            timer.claim = None;
                        }
                    }
                }
            }
        }
        for child in transition.child_starts() {
            let child_id = child.instance().instance_id;
            w.instances.insert(child_id, child.instance().clone());
            w.revisions.entry(child_id).or_insert(0);
            w.fibers.insert(
                (child_id, child.root_fiber().fiber_id),
                child.root_fiber().clone(),
            );
            let seq = w.event_seq.entry(child_id).or_insert(0);
            *seq += 1;
            let child_seq = *seq;
            w.events
                .entry(child_id)
                .or_default()
                .push((child_seq, child.start_event().clone()));
            let child_snapshot = SnapshotEnvelope::new(
                CURRENT_ARTIFACT_ABI,
                child.instance().bytecode_version,
                0,
                PersistedSnapshotState::new(
                    child.instance().clone(),
                    [child.root_fiber().clone()],
                    BTreeMap::new(),
                    [],
                    ConcurrencyTable::new(),
                    [],
                ),
            );
            let child_state_hash = child_snapshot
                .state_hash()
                .map_err(|error| CommitError::Integrity(error.to_string()))?;
            let child_command = CommandEnvelope::new(
                EffectId::for_transition(child_id, 0, u32::MAX).as_uuid(),
                child.instance().created_at,
                JournalCommand::Administrative {
                    kind: "child_start".to_string(),
                },
            );
            w.snapshots.insert(child_id, child_snapshot);
            w.journals
                .entry(child_id)
                .or_default()
                .push(JournalRecord::new(
                    child_command,
                    -1,
                    0,
                    child.instance().bytecode_version,
                    [0u8; 32],
                    child_state_hash,
                    std::slice::from_ref(child.start_event()),
                    &[],
                ));
        }
        for event in transition.events() {
            let seq = w.event_seq.entry(instance_id).or_insert(0);
            *seq += 1;
            let current_seq = *seq;
            w.events
                .entry(instance_id)
                .or_default()
                .push((current_seq, event.clone()));
        }
        let new_revision = if is_start {
            w.revisions.insert(instance_id, 0);
            0
        } else {
            w.revisions
                .insert(instance_id, claim.expected_revision() + 1);
            claim.expected_revision() + 1
        };

        let fibers = w
            .fibers
            .iter()
            .filter(|((id, _), _)| *id == instance_id)
            .map(|(_, fiber)| fiber.clone())
            .collect::<Vec<_>>();
        let join_counts = w
            .join_counters
            .iter()
            .filter(|((id, _), _)| *id == instance_id)
            .map(|((_, join_id), count)| (*join_id, *count))
            .collect::<BTreeMap<_, _>>();
        let incidents = w.incidents.get(&instance_id).cloned().unwrap_or_default();
        let concurrency_table = w.concurrency_tables.entry(instance_id).or_default();
        for mutation in transition.concurrency_mutations() {
            match mutation {
                ConcurrencyMutation::Insert(record) => concurrency_table.insert((**record).clone()),
                ConcurrencyMutation::Retire(id) => {
                    if let Some(record) = concurrency_table.get_mut(*id) {
                        record.state = RecordState::Retired;
                    }
                }
                ConcurrencyMutation::Remove(id) => {
                    concurrency_table.remove(*id);
                }
            }
        }
        let concurrency_table = concurrency_table.clone();
        let snapshot_envelope = SnapshotEnvelope::new(
            CURRENT_ARTIFACT_ABI,
            snapshot.bytecode_version,
            new_revision,
            PersistedSnapshotState::new(
                snapshot.clone(),
                fibers,
                join_counts,
                incidents,
                concurrency_table,
                [],
            ),
        );
        let state_hash = snapshot_envelope
            .state_hash()
            .map_err(|error| CommitError::Integrity(error.to_string()))?;
        let command = transition.command_envelope().cloned().unwrap_or_else(|| {
            transition.start_dedupe().map_or_else(
                || {
                    CommandEnvelope::new(
                        EffectId::for_transition(instance_id, new_revision, u32::MAX).as_uuid(),
                        snapshot.created_at,
                        JournalCommand::Administrative {
                            kind: "fixture_or_admin_commit".to_string(),
                        },
                    )
                },
                |start| {
                    CommandEnvelope::new(
                        start.command().idempotency_key(),
                        start.command().logical_time(),
                        JournalCommand::Start(start.command().clone()),
                    )
                },
            )
        });
        let prior_revision = if is_start {
            -1
        } else {
            i64::try_from(claim.expected_revision()).map_err(|_| {
                CommitError::Integrity("revision exceeds signed journal range".to_string())
            })?
        };
        let prior_state_hash = w
            .snapshots
            .get(&instance_id)
            .and_then(|envelope| envelope.state_hash().ok())
            .unwrap_or([0u8; 32]);
        let journal = JournalRecord::new(
            command,
            prior_revision,
            new_revision,
            snapshot.bytecode_version,
            prior_state_hash,
            state_hash,
            transition.events(),
            transition.effects(),
        );
        w.snapshots.insert(instance_id, snapshot_envelope);
        w.journals.entry(instance_id).or_default().push(journal);

        let has_running_fiber = w
            .fibers
            .iter()
            .any(|((id, _), fiber)| *id == instance_id && fiber.wait == WaitState::Running);
        if !matches!(snapshot.state, ProcessState::Running) || !has_running_fiber {
            w.transition_leases.remove(&instance_id);
        }
        Ok(CommitOutcome::Committed { new_revision })
    }

    async fn lookup_start_instance(
        &self,
        tenant_id: &TenantId,
        idempotency_key: Uuid,
    ) -> StoreResult<Option<Uuid>> {
        Ok(self
            .inner
            .read()
            .await
            .starts
            .get(&(tenant_id.to_string(), idempotency_key))
            .copied())
    }

    async fn claim_due_timers(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedTimer>> {
        let tenant = tenant_id.clone();
        let mut w = self.inner.write().await;
        let mut due: Vec<(EffectId, u64)> = w
            .timers
            .iter()
            .filter(|(_, timer)| {
                timer.tenant_id == tenant
                    && timer.state == MemoryTimerState::Armed
                    && timer.due_at <= now_ms
                    && timer
                        .claim
                        .as_ref()
                        .map(|(_, _, until)| *until <= now_ms)
                        .unwrap_or(true)
            })
            .map(|(timer_id, timer)| (*timer_id, timer.due_at))
            .collect();
        due.sort_by_key(|(timer_id, due_at)| (*due_at, timer_id.as_uuid()));
        due.truncate(limit);

        let claim_until = now_ms.saturating_add(lease_ms);
        let mut claimed = Vec::with_capacity(due.len());
        for (timer_id, _) in due {
            let claim_token = Uuid::now_v7();
            if let Some(timer) = w.timers.get_mut(&timer_id) {
                timer.claim = Some((owner.to_string(), claim_token, claim_until));
                claimed.push(ClaimedTimer::new(
                    bpmn_lite_types::ClaimedTimerIdentity::new(
                        timer.tenant_id.clone(),
                        timer_id,
                        timer.instance_id,
                        timer.fiber_id,
                    ),
                    timer.due_at,
                    timer.kind.clone(),
                    timer.repeat_spec.clone(),
                    claim_token,
                ));
            }
        }
        Ok(claimed)
    }

    async fn release_timer_claim(&self, timer: &ClaimedTimer) -> StoreResult<()> {
        let mut w = self.inner.write().await;
        if let Some(current) = w.timers.get_mut(&timer.timer_id()) {
            if matches!(current.claim, Some((_, token, _)) if token == timer.claim_token()) {
                current.claim = None;
            }
        }
        Ok(())
    }

    async fn claim_pending_effects(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedEffect>> {
        let mut w = self.inner.write().await;
        let mut ids: Vec<EffectId> = w
            .durable_effects
            .iter()
            .filter(|(_, effect)| {
                effect.tenant_id.as_str() == tenant_id.as_str()
                    && effect.state != MemoryEffectState::Terminal
                    && effect.response.is_none()
                    && effect
                        .claim
                        .as_ref()
                        .map(|(_, _, until)| *until <= now_ms)
                        .unwrap_or(true)
            })
            .map(|(id, _)| *id)
            .collect();
        ids.sort_by_key(|id| id.as_uuid());
        ids.truncate(limit);
        let claim_until = now_ms.saturating_add(lease_ms);
        let mut claimed = Vec::with_capacity(ids.len());
        for id in ids {
            let claim_token = Uuid::now_v7();
            if let Some(effect) = w.durable_effects.get_mut(&id) {
                effect.state = MemoryEffectState::Dispatching;
                effect.claim = Some((owner.to_string(), claim_token, claim_until));
                claimed.push(ClaimedEffect::new(
                    effect.tenant_id.clone(),
                    effect.instance_id,
                    effect.effect.clone(),
                    claim_token,
                    effect.attempt,
                    effect.policy_version,
                ));
            }
        }
        Ok(claimed)
    }

    async fn record_effect_response(
        &self,
        effect: &ClaimedEffect,
        response: &EffectResponse,
    ) -> StoreResult<bool> {
        let mut w = self.inner.write().await;
        let Some(current) = w.durable_effects.get_mut(&effect.effect().effect_id()) else {
            return Ok(false);
        };
        if current.response.is_some() {
            return Ok(false);
        }
        if !matches!(current.claim, Some((_, token, _)) if token == effect.claim_token()) {
            return Ok(false);
        }
        current.response = Some(response.clone());
        current.state = MemoryEffectState::Accepted;
        current.claim = None;
        Ok(true)
    }

    async fn load_effect_responses(
        &self,
        tenant_id: &TenantId,
        limit: usize,
    ) -> StoreResult<Vec<PendingEffectResponse>> {
        let w = self.inner.read().await;
        let mut responses: Vec<_> = w
            .durable_effects
            .values()
            .filter_map(|effect| {
                (effect.tenant_id.as_str() == tenant_id.as_str())
                    .then(|| {
                        effect.response.as_ref().map(|response| {
                            PendingEffectResponse::new(
                                effect.tenant_id.clone(),
                                effect.instance_id,
                                effect.effect.clone(),
                                response.clone(),
                            )
                        })
                    })
                    .flatten()
            })
            .collect();
        responses.sort_by_key(|response| response.effect_id().as_uuid());
        responses.truncate(limit);
        Ok(responses)
    }

    async fn release_effect_claim(&self, effect: &ClaimedEffect) -> StoreResult<()> {
        let mut w = self.inner.write().await;
        if let Some(current) = w.durable_effects.get_mut(&effect.effect().effect_id()) {
            if matches!(current.claim, Some((_, token, _)) if token == effect.claim_token()) {
                current.state = MemoryEffectState::Pending;
                current.claim = None;
            }
        }
        Ok(())
    }

    async fn schedule_effect_retry(
        &self,
        effect: &ClaimedEffect,
        decision: RetryDecision,
        _error: &str,
    ) -> StoreResult<()> {
        let mut w = self.inner.write().await;
        let current = w
            .durable_effects
            .get_mut(&effect.effect().effect_id())
            .ok_or_else(|| StoreError::NotFound("effect does not exist".into()))?;
        if !matches!(current.claim, Some((_, token, _)) if token == effect.claim_token()) {
            return Err(StoreError::Integrity(
                "effect dispatch lease is stale".into(),
            ));
        }
        match decision {
            RetryDecision::At { attempt, .. } => {
                current.attempt = attempt;
                current.state = MemoryEffectState::Pending;
            }
            RetryDecision::Exhausted | RetryDecision::Terminal => {
                current.state = MemoryEffectState::Terminal;
            }
        }
        current.claim = None;
        Ok(())
    }

    async fn release_instance_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
    ) -> StoreResult<()> {
        let mut w = self.inner.write().await;
        let same_tenant = w
            .instances
            .get(&instance_id)
            .map(|instance| instance.tenant_id == tenant_id.as_str())
            .unwrap_or(false);
        if same_tenant
            && matches!(
                w.transition_leases.get(&instance_id),
                Some((current_owner, _, _)) if current_owner == owner
            )
        {
            w.transition_leases.remove(&instance_id);
        }
        Ok(())
    }

    async fn join_get(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        join_id: JoinId,
    ) -> StoreResult<u16> {
        let r = self.inner.read().await;
        if r.instances
            .get(&instance_id)
            .is_none_or(|instance| instance.tenant_id != tenant_id.as_str())
        {
            return Ok(0);
        }
        Ok(*r.join_counters.get(&(instance_id, join_id)).unwrap_or(&0))
    }
}

#[async_trait]
impl ArtifactRepository for MemoryStore {
    // ── Program store ──

    async fn store_program(&self, version: [u8; 32], program: &CompiledProgram) -> StoreResult<()> {
        let mut w = self.inner.write().await;
        w.programs.insert(version, program.clone());
        Ok(())
    }

    async fn load_program(&self, version: [u8; 32]) -> StoreResult<Option<CompiledProgram>> {
        let r = self.inner.read().await;
        if let Some(bytes) = r.artifacts.get(&ArtifactHash::from_bytes(version)) {
            return ExecutableWorkflow::verify(bytes)
                .map(|artifact| Some(artifact.to_legacy_program()))
                .map_err(|error| {
                    StoreError::Integrity(format!("stored artifact failed verification: {error}"))
                });
        }
        Ok(r.programs.get(&version).cloned())
    }

    async fn store_artifact(
        &self,
        artifact: &ExecutableWorkflow,
    ) -> std::result::Result<(), ArtifactStoreError> {
        let bytes = artifact
            .canonical_bytes()
            .map_err(|error| ArtifactStoreError::InvalidArtifact(error.to_string()))?;
        let mut inner = self.inner.write().await;
        match inner.artifacts.get(&artifact.hash()) {
            Some(existing) if existing != &bytes => Err(ArtifactStoreError::ArtifactCollision {
                hash: artifact.hash().into_bytes(),
            }),
            Some(_) => Ok(()),
            None => {
                inner.artifacts.insert(artifact.hash(), bytes);
                Ok(())
            }
        }
    }

    async fn load_artifact(
        &self,
        hash: ArtifactHash,
    ) -> std::result::Result<Option<ExecutableWorkflow>, ArtifactStoreError> {
        let (bytes, legacy) = {
            let inner = self.inner.read().await;
            let resolved = inner.artifact_lineage.get(&hash).copied().unwrap_or(hash);
            (
                inner.artifacts.get(&resolved).cloned(),
                inner.programs.get(hash.as_bytes()).cloned(),
            )
        };
        if let Some(bytes) = bytes {
            return ExecutableWorkflow::verify(&bytes)
                .map(Some)
                .map_err(|error| ArtifactStoreError::InvalidArtifact(error.to_string()));
        }
        let Some(legacy) = legacy else {
            return Ok(None);
        };
        let envelope = ArtifactEnvelope::from_legacy_program(legacy, "legacy-adapter")
            .map_err(|error| ArtifactStoreError::InvalidArtifact(error.to_string()))?;
        let artifact = ExecutableWorkflow::from_verified_envelope(envelope)
            .map_err(|error| ArtifactStoreError::InvalidArtifact(error.to_string()))?;
        self.store_artifact(&artifact).await?;
        self.inner
            .write()
            .await
            .artifact_lineage
            .insert(hash, artifact.hash());
        Ok(Some(artifact))
    }

    async fn store_plan(&self, plan_hash: [u8; 32], plan_json: &str) -> StoreResult<()> {
        let mut w = self.inner.write().await;
        w.plans
            .entry(plan_hash)
            .or_insert_with(|| plan_json.to_owned());
        Ok(())
    }

    async fn load_plan(&self, plan_hash: [u8; 32]) -> StoreResult<Option<String>> {
        let r = self.inner.read().await;
        Ok(r.plans.get(&plan_hash).cloned())
    }
}

#[async_trait]
impl JournalReader for MemoryStore {
    // ── Event log ──

    async fn read_events(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        from_seq: u64,
    ) -> StoreResult<Vec<(u64, RuntimeEvent)>> {
        let r = self.inner.read().await;
        if r.instances
            .get(&instance_id)
            .is_none_or(|instance| instance.tenant_id != tenant_id.as_str())
        {
            return Ok(Vec::new());
        }
        Ok(r.events
            .get(&instance_id)
            .map(|evts| {
                evts.iter()
                    .filter(|(seq, _)| *seq >= from_seq)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn load_snapshot_envelope(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Option<SnapshotEnvelope>> {
        let state = self.inner.read().await;
        Ok(state
            .snapshots
            .get(&instance_id)
            .filter(|snapshot| snapshot.state().instance().tenant_id == tenant_id.as_str())
            .cloned())
    }

    async fn read_journal(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        after_revision: Option<u64>,
    ) -> StoreResult<Vec<JournalRecord>> {
        let state = self.inner.read().await;
        if state
            .instances
            .get(&instance_id)
            .is_none_or(|instance| instance.tenant_id != tenant_id.as_str())
        {
            return Ok(Vec::new());
        }
        Ok(state
            .journals
            .get(&instance_id)
            .into_iter()
            .flatten()
            .filter(|record| after_revision.is_none_or(|revision| record.new_revision() > revision))
            .cloned()
            .collect())
    }

    // ── Payload history ──

    async fn load_payload_version(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        hash: &[u8; 32],
    ) -> StoreResult<Option<String>> {
        let r = self.inner.read().await;
        if r.instances
            .get(&instance_id)
            .is_none_or(|instance| instance.tenant_id != tenant_id.as_str())
        {
            return Ok(None);
        }
        Ok(r.payload_history.get(&(instance_id, *hash)).cloned())
    }
}

#[async_trait]
impl AdminProjectionStore for MemoryStore {
    // ── Template catalog ──

    async fn store_template(
        &self,
        name: &str,
        version: u32,
        plan_hash: [u8; 32],
        dsl_body: &str,
    ) -> StoreResult<()> {
        let mut w = self.inner.write().await;
        let now = chrono::Utc::now().to_rfc3339();
        w.templates.insert(
            (name.to_owned(), version),
            (dsl_body.to_owned(), plan_hash, now),
        );
        Ok(())
    }

    async fn load_template_version(
        &self,
        name: &str,
        version: u32,
    ) -> StoreResult<Option<(String, [u8; 32])>> {
        let r = self.inner.read().await;
        if let Some((dsl, hash, _)) = r.templates.get(&(name.to_owned(), version)) {
            Ok(Some((dsl.clone(), *hash)))
        } else {
            Ok(None)
        }
    }

    async fn load_latest_template_version(
        &self,
        name: &str,
    ) -> StoreResult<Option<(u32, String, [u8; 32])>> {
        let r = self.inner.read().await;
        let mut latest: Option<(u32, String, [u8; 32])> = None;
        for ((t_name, version), (dsl, hash, _)) in r.templates.iter() {
            if t_name == name {
                if let Some((best_v, _, _)) = &latest {
                    if *version > *best_v {
                        latest = Some((*version, dsl.clone(), *hash));
                    }
                } else {
                    latest = Some((*version, dsl.clone(), *hash));
                }
            }
        }
        Ok(latest)
    }

    async fn list_templates(&self) -> StoreResult<Vec<crate::store::TemplateSummary>> {
        let r = self.inner.read().await;
        let mut latest_map: HashMap<String, (u32, [u8; 32], String)> = HashMap::new();
        for ((name, version), (_, hash, created_at)) in r.templates.iter() {
            let entry =
                latest_map
                    .entry(name.clone())
                    .or_insert((*version, *hash, created_at.clone()));
            if *version > entry.0 {
                *entry = (*version, *hash, created_at.clone());
            }
        }
        let summaries = latest_map
            .into_iter()
            .map(
                |(name, (version, hash, created_at))| crate::store::TemplateSummary {
                    name,
                    latest_version: version,
                    plan_hash: hash,
                    created_at,
                },
            )
            .collect();
        Ok(summaries)
    }

    async fn health_check(&self) -> StoreResult<()> {
        Ok(())
    }

    async fn ensure_tenant(&self, _tenant_id: &TenantId) -> StoreResult<()> {
        Ok(()) // MemoryStore is single-process; tenant registration is a no-op.
    }

    async fn list_tenants(&self) -> StoreResult<Vec<String>> {
        let guard = self.inner.read().await;
        let mut tenants: Vec<String> = guard
            .instances
            .values()
            .map(|i| i.tenant_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        tenants.sort();
        Ok(tenants)
    }

    async fn list_tenants_in_pool(&self, pool_id: &str) -> StoreResult<Vec<String>> {
        // MemoryStore has no pool concept; 'default' returns all known tenants,
        // other pool IDs return empty (consistent with an empty dedicated pool).
        if pool_id == "default" {
            self.list_tenants().await
        } else {
            Ok(vec![])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    impl MemoryStore {
        async fn save_instance(&self, _owner: &str, instance: &ProcessInstance) -> Result<()> {
            let tenant =
                TenantId::new(instance.tenant_id.clone()).map_err(StoreError::integrity)?;
            let transition = TransitionBuilder::new(instance.clone()).build();
            self.commit_transition(&Claim::new(tenant, instance.instance_id, 0, 0), &transition)
                .await
                .map(|_| ())
                .map_err(StoreError::integrity)
        }

        async fn fixture_transition(
            &self,
            instance_id: Uuid,
            build: impl FnOnce(TransitionBuilder) -> TransitionBuilder,
        ) -> Result<()> {
            let instance = match self
                .load_instance(&TenantId::default(), instance_id)
                .await?
            {
                Some(instance) => instance,
                None => {
                    self.save_instance("fixture", &make_instance(instance_id))
                        .await?;
                    self.load_instance(&TenantId::default(), instance_id)
                        .await?
                        .ok_or_else(|| {
                            StoreError::Integrity(
                                "fixture instance missing after initialization".into(),
                            )
                        })?
                }
            };
            let tenant_id = instance.tenant_id.clone();
            let claim = self
                .claim_instance_for_transition(
                    &TenantId::new(instance.tenant_id.clone()).unwrap(),
                    instance_id,
                    "fixture",
                    30_000,
                )
                .await
                .map_err(StoreError::integrity)?
                .ok_or_else(|| StoreError::Integrity("fixture instance not claimable".into()))?;
            let result = self
                .commit_transition(&claim, &build(TransitionBuilder::new(instance)).build())
                .await
                .map(|_| ())
                .map_err(StoreError::integrity);
            self.release_instance_transition(
                &TenantId::new(tenant_id).unwrap(),
                instance_id,
                "fixture",
            )
            .await?;
            result
        }

        async fn save_fiber(&self, instance_id: Uuid, fiber: &Fiber) -> Result<()> {
            self.fixture_transition(instance_id, |builder| builder.upsert_fiber(fiber.clone()))
                .await
        }

        async fn delete_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<()> {
            self.fixture_transition(instance_id, |builder| builder.delete_fiber(fiber_id))
                .await
        }

        async fn join_arrive(&self, instance_id: Uuid, join_id: JoinId) -> Result<u16> {
            self.fixture_transition(instance_id, |builder| {
                builder.join_mutation(JoinMutation::Arrive(join_id))
            })
            .await?;
            self.join_get(&TenantId::default(), instance_id, join_id)
                .await
        }

        async fn join_reset(&self, instance_id: Uuid, join_id: JoinId) -> Result<()> {
            self.fixture_transition(instance_id, |builder| {
                builder.join_mutation(JoinMutation::Reset(join_id))
            })
            .await
        }

        async fn dedupe_put(
            &self,
            _tenant_id: &str,
            key: &str,
            completion: &JobCompletion,
        ) -> Result<()> {
            self.inner
                .write()
                .await
                .dedupe
                .insert(key.to_string(), (completion.clone(), Instant::now()));
            Ok(())
        }

        async fn enqueue_job(&self, activation: &JobActivation) -> Result<()> {
            self.fixture_transition(activation.process_instance_id, |builder| {
                builder.enqueue_job(activation.clone())
            })
            .await
        }

        async fn ack_job(&self, _tenant_id: &str, job_key: &str) -> Result<()> {
            let mut guard = self.inner.write().await;
            guard.inflight_jobs.remove(job_key);
            guard.job_queue.retain(|job| job.job_key != job_key);
            Ok(())
        }

        async fn buffer_message(
            &self,
            identity: (&str, &str, &str, &str),
            payload: &[u8],
            payload_hash: Option<[u8; 32]>,
            ttl_ms: u64,
            process_instance_id: Option<Uuid>,
        ) -> Result<BufferMessageResult> {
            let (tenant_id, message_name, correlation_key, msg_id) = identity;
            let mut guard = self.inner.write().await;
            let key = (
                tenant_id.to_string(),
                message_name.to_string(),
                correlation_key.to_string(),
                msg_id.to_string(),
            );
            if guard.message_buffer.contains_key(&key)
                || guard.message_buffer_consumed.contains(&key)
            {
                return Ok(BufferMessageResult::Duplicate);
            }
            let received_at = now_ms();
            guard.message_buffer.insert(
                key,
                BufferedMessage {
                    tenant_id: tenant_id.to_string(),
                    message_name: message_name.to_string(),
                    correlation_key: correlation_key.to_string(),
                    msg_id: msg_id.to_string(),
                    payload: payload.to_vec(),
                    payload_hash,
                    process_instance_id,
                    received_at,
                    expires_at: received_at + ttl_ms as i64,
                },
            );
            Ok(BufferMessageResult::Inserted)
        }

        async fn release_buffered_message_claim(
            &self,
            message: &ClaimedBufferedMessage,
        ) -> Result<bool> {
            let key = (
                message.message.tenant_id.clone(),
                message.message.message_name.clone(),
                message.message.correlation_key.clone(),
                message.message.msg_id.clone(),
            );
            let mut guard = self.inner.write().await;
            let Some((claim_token, _)) = guard.message_buffer_claims.get(&key) else {
                return Ok(false);
            };
            if claim_token != &message.claim_token {
                return Ok(false);
            }
            guard.message_buffer_claims.remove(&key);
            Ok(true)
        }

        async fn atomic_consume_buffered_message(
            &self,
            instance: &ProcessInstance,
            fiber: &Fiber,
            message: &ClaimedBufferedMessage,
            payload_update: Option<&PayloadUpdate>,
            events: &[RuntimeEvent],
        ) -> Result<bool> {
            let mut next = instance.clone();
            if let Some(update) = payload_update {
                next.domain_payload = update.payload.clone().into();
                next.domain_payload_hash = update.payload_hash;
            }
            let mut builder = TransitionBuilder::new(next)
                .upsert_fiber(fiber.clone())
                .buffered_message(BufferedMessageMutation::Consume(message.clone()));
            for event in events {
                builder = builder.event(event.clone());
            }
            let claim = self
                .claim_instance_for_transition(
                    &TenantId::new(instance.tenant_id.clone()).unwrap(),
                    instance.instance_id,
                    "fixture",
                    30_000,
                )
                .await
                .map_err(StoreError::integrity)?
                .ok_or_else(|| StoreError::Integrity("fixture instance not claimable".into()))?;
            Ok(self
                .commit_transition(&claim, &builder.build())
                .await
                .is_ok())
        }

        async fn append_event(&self, instance_id: Uuid, event: &RuntimeEvent) -> Result<u64> {
            self.fixture_transition(instance_id, |builder| builder.event(event.clone()))
                .await?;
            Ok(self
                .read_events(&TenantId::default(), instance_id, 0)
                .await?
                .last()
                .map(|(sequence, _)| *sequence)
                .unwrap_or(0))
        }

        async fn save_payload_version(
            &self,
            instance_id: Uuid,
            hash: &[u8; 32],
            payload: &str,
        ) -> Result<()> {
            self.inner
                .write()
                .await
                .payload_history
                .insert((instance_id, *hash), payload.to_string());
            Ok(())
        }
    }

    async fn commit_ops(
        store: &MemoryStore,
        instance_id: Uuid,
        ops: &[TickOperation],
    ) -> Result<()> {
        let claim = store
            .claim_instance_for_transition(&TenantId::default(), instance_id, "test", 30_000)
            .await
            .map_err(StoreError::integrity)?
            .ok_or_else(|| StoreError::Integrity("test instance is not claimable".into()))?;
        let current = store
            .load_instance(&TenantId::default(), instance_id)
            .await?
            .ok_or_else(|| StoreError::NotFound("test instance is missing".into()))?;
        let transition = transition_from_tick_ops(&current, ops);
        store
            .commit_transition(&claim, &transition)
            .await
            .map(|_| ())
            .map_err(StoreError::integrity)
    }

    fn make_instance(id: Uuid) -> ProcessInstance {
        let payload = r#"{"case_id":"abc"}"#;
        let hash = test_hash(payload);
        ProcessInstance {
            instance_id: id,
            process_key: "test-process".to_string(),
            bytecode_version: [0u8; 32],
            tenant_id: "default".to_string(),
            domain_payload: payload.to_string().into(),
            domain_payload_hash: hash,
            session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
            flags: BTreeMap::from([(0, Value::Bool(true)), (1, Value::I64(42))]),
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: "runbook-entry-1".to_string(),
            entry_id: Uuid::new_v4(),
            runbook_id: Uuid::new_v4(),
            created_at: 1000,
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: None,
        }
    }

    fn test_hash(data: &str) -> [u8; 32] {
        blake3::hash(data.as_bytes()).into()
    }

    /// A2.T1: Save/load instance round-trip
    #[tokio::test]
    async fn test_instance_round_trip() {
        let store = MemoryStore::new();
        let id = Uuid::now_v7();
        let inst = make_instance(id);

        store.save_instance("default", &inst).await.unwrap();
        let loaded = store
            .load_instance(&TenantId::default(), id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(loaded.instance_id, id);
        assert_eq!(loaded.domain_payload, inst.domain_payload);
        assert_eq!(loaded.domain_payload_hash, inst.domain_payload_hash);
        assert_eq!(loaded.flags.len(), 2);
        assert_eq!(loaded.flags[&0], Value::Bool(true));
        assert_eq!(loaded.flags[&1], Value::I64(42));
        assert_eq!(loaded.state, ProcessState::Running);
    }

    /// A2.T1b: Saving an instance copies session_stack by value.
    #[tokio::test]
    async fn test_instance_session_stack_is_not_aliased() {
        let store = MemoryStore::new();
        let id = Uuid::now_v7();
        let original_session_id = Uuid::new_v4();
        let original_scope_id = Uuid::new_v4();
        let mutated_session_id = Uuid::new_v4();
        let mutated_scope_id = Uuid::new_v4();

        let mut inst = make_instance(id);
        inst.session_stack = bpmn_lite_types::session_stack::SessionStackState {
            session_id: original_session_id,
            scope: Some(bpmn_lite_types::session_stack::SessionScopeState {
                client_group_id: original_scope_id,
                client_group_name: Some("Original".to_string()),
            }),
            active_workspace: Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Cbu),
            workspace_stack: Vec::new(),
            trace_sequence: 7,
        };

        store.save_instance("default", &inst).await.unwrap();

        inst.session_stack.session_id = mutated_session_id;
        inst.session_stack.scope = Some(bpmn_lite_types::session_stack::SessionScopeState {
            client_group_id: mutated_scope_id,
            client_group_name: Some("Mutated".to_string()),
        });
        inst.session_stack.active_workspace =
            Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Deal);
        inst.session_stack.trace_sequence = 99;

        let loaded = store
            .load_instance(&TenantId::default(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.session_stack.session_id, original_session_id);
        assert_eq!(
            loaded
                .session_stack
                .scope
                .as_ref()
                .map(|scope| scope.client_group_id),
            Some(original_scope_id)
        );
        assert_eq!(
            loaded.session_stack.active_workspace,
            Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Cbu)
        );
        assert_eq!(loaded.session_stack.trace_sequence, 7);
    }

    /// A2.T2: Save/load/delete fiber round-trip (including WaitState::Job)
    #[tokio::test]
    async fn test_fiber_round_trip() {
        let store = MemoryStore::new();
        let iid = Uuid::now_v7();
        let fid = Uuid::now_v7();

        let mut fiber = Fiber::new(fid, 0);
        fiber.wait = WaitState::Job {
            job_key: "job-123".to_string(),
        };
        fiber.stack.push(Value::I64(99));

        store.save_fiber(iid, &fiber).await.unwrap();
        let loaded = store
            .load_fiber(&TenantId::default(), iid, fid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.fiber_id, fid);
        assert_eq!(
            loaded.wait,
            WaitState::Job {
                job_key: "job-123".to_string()
            }
        );
        assert_eq!(loaded.stack, vec![Value::I64(99)]);

        // Delete
        store.delete_fiber(iid, fid).await.unwrap();
        assert!(store
            .load_fiber(&TenantId::default(), iid, fid)
            .await
            .unwrap()
            .is_none());
    }

    /// A2.T3: Join barrier: arrive 3 times, reset
    #[tokio::test]
    async fn test_join_barrier() {
        let store = MemoryStore::new();
        let iid = Uuid::now_v7();
        let join_id: JoinId = 0;

        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 1);
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 2);
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 3);

        store.join_reset(iid, join_id).await.unwrap();
        assert_eq!(store.join_arrive(iid, join_id).await.unwrap(), 1);
    }

    /// A2.T4: Dedupe: put JobCompletion + get returns cached
    #[tokio::test]
    async fn test_dedupe() {
        let store = MemoryStore::new();
        let completion = JobCompletion {
            job_key: "job-abc".to_string(),
            domain_payload: r#"{"done":true}"#.to_string(),
            expected_instance_payload_hash: test_hash(r#"{"case_id":"abc"}"#),
            orch_flags: BTreeMap::new(),
        };

        assert!(store
            .dedupe_get(&TenantId::default(), "job-abc")
            .await
            .unwrap()
            .is_none());
        store
            .dedupe_put("default", "job-abc", &completion)
            .await
            .unwrap();

        let cached = store
            .dedupe_get(&TenantId::default(), "job-abc")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached.job_key, "job-abc");
        assert_eq!(cached.domain_payload, r#"{"done":true}"#);
    }

    /// A2.T5: Job queue: enqueue 3, dequeue 2, ack 1, dequeue 1 remaining
    #[tokio::test]
    async fn test_job_queue() {
        let store = MemoryStore::new();
        let task_type = "create_case".to_string();
        let session_id = Uuid::new_v4();

        for i in 0..3 {
            let instance_id = Uuid::now_v7();
            store
                .save_instance("default", &make_instance(instance_id))
                .await
                .unwrap();
            store
                .enqueue_job(&JobActivation {
                    job_key: format!("job-{i}"),
                    tenant_id: "default".to_string(),
                    process_instance_id: instance_id,
                    task_type: task_type.clone(),
                    service_task_id: format!("task-{i}"),
                    domain_payload: "{}".to_string(),
                    domain_payload_hash: [0u8; 32],
                    session_stack: bpmn_lite_types::session_stack::SessionStackState {
                        session_id,
                        ..Default::default()
                    },
                    orch_flags: BTreeMap::new(),
                    retries_remaining: 3,
                    entry_id: Uuid::new_v4(),
                    runbook_id: Uuid::new_v4(),
                    worker_id: String::new(),
                    claim_token: String::new(),
                    claim_expires_at: None,
                    attempt_count: 0,
                    failure_count: 0,
                    not_before: None,
                })
                .await
                .unwrap();
        }

        // Dequeue 2
        let batch1 = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                2,
                &TenantId::default(),
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(batch1.len(), 2);
        assert_eq!(batch1[0].job_key, "job-0");
        assert_eq!(batch1[1].job_key, "job-1");
        assert_eq!(batch1[0].worker_id, "test-worker");
        assert!(!batch1[0].claim_token.is_empty());
        assert!(store
            .validate_job_claim(
                &TenantId::default(),
                "job-0",
                "test-worker",
                &batch1[0].claim_token
            )
            .await
            .unwrap());
        assert!(!store
            .validate_job_claim(
                &TenantId::default(),
                "job-0",
                "other-worker",
                &batch1[0].claim_token
            )
            .await
            .unwrap());
        assert!(batch1
            .iter()
            .all(|job| job.session_stack.session_id == session_id));

        // Ack one
        store.ack_job("default", "job-0").await.unwrap();

        // Dequeue remaining
        let batch2 = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                10,
                &TenantId::default(),
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(batch2.len(), 1);
        assert_eq!(batch2[0].job_key, "job-2");
        assert_eq!(batch2[0].session_stack.session_id, session_id);
    }

    #[tokio::test]
    async fn test_job_claim_lease_not_before_and_reclaim() {
        let store = MemoryStore::new();
        let task_type = "create_case".to_string();
        let instance_id = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(instance_id))
            .await
            .unwrap();

        store
            .enqueue_job(&JobActivation {
                job_key: "lease-job".to_string(),
                tenant_id: "default".to_string(),
                process_instance_id: instance_id,
                task_type: task_type.clone(),
                service_task_id: "task-lease".to_string(),
                domain_payload: "{}".to_string(),
                domain_payload_hash: [0u8; 32],
                session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
                orch_flags: BTreeMap::new(),
                retries_remaining: 3,
                entry_id: Uuid::new_v4(),
                runbook_id: Uuid::new_v4(),
                worker_id: String::new(),
                claim_token: String::new(),
                claim_expires_at: None,
                attempt_count: 0,
                failure_count: 0,
                not_before: Some(now_ms() + 60_000),
            })
            .await
            .unwrap();

        let not_due = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                1,
                &TenantId::default(),
                "worker-a",
                1,
            )
            .await
            .unwrap();
        assert!(not_due.is_empty());

        let mut queued = store.inner.write().await;
        queued.job_queue[0].not_before = None;
        drop(queued);

        let claimed = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                1,
                &TenantId::default(),
                "worker-a",
                1,
            )
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].attempt_count, 1);
        assert!(claimed[0].claim_expires_at.is_some());
        assert!(store
            .validate_job_claim(
                &TenantId::default(),
                "lease-job",
                "worker-a",
                &claimed[0].claim_token
            )
            .await
            .unwrap());

        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        assert!(!store
            .validate_job_claim(
                &TenantId::default(),
                "lease-job",
                "worker-a",
                &claimed[0].claim_token
            )
            .await
            .unwrap());
        assert_eq!(store.reclaim_stale_jobs(0).await.unwrap(), 1);

        let reclaimed = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                1,
                &TenantId::default(),
                "worker-b",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(reclaimed.len(), 1);
        assert_eq!(reclaimed[0].worker_id, "worker-b");
        assert_eq!(reclaimed[0].attempt_count, 2);
        assert_eq!(reclaimed[0].failure_count, 1);
    }

    #[tokio::test]
    async fn test_message_buffer_idempotent_claim_release_and_prune() {
        let store = MemoryStore::new();
        assert_eq!(
            store
                .buffer_message(
                    ("default", "1", "b:false", "msg-1"),
                    b"{}",
                    None,
                    60_000,
                    None
                )
                .await
                .unwrap(),
            BufferMessageResult::Inserted
        );
        assert_eq!(
            store
                .buffer_message(
                    ("default", "1", "b:false", "msg-1"),
                    b"{}",
                    None,
                    60_000,
                    None
                )
                .await
                .unwrap(),
            BufferMessageResult::Duplicate
        );

        let claimed = store
            .claim_buffered_message(&TenantId::default(), "1", "b:false", 60_000)
            .await
            .unwrap()
            .expect("buffered message");
        assert_eq!(claimed.message.msg_id, "msg-1");
        assert!(store
            .claim_buffered_message(&TenantId::default(), "1", "b:false", 60_000)
            .await
            .unwrap()
            .is_none());
        assert!(store
            .release_buffered_message_claim(&claimed)
            .await
            .unwrap());
        assert!(store
            .claim_buffered_message(&TenantId::default(), "1", "b:false", 60_000)
            .await
            .unwrap()
            .is_some());

        store
            .buffer_message(("default", "1", "b:false", "expired"), b"{}", None, 0, None)
            .await
            .unwrap();
        assert_eq!(store.prune_expired_messages().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_claimed_buffered_message_is_idempotent_until_atomic_consume() {
        let store = MemoryStore::new();
        let instance_id = Uuid::now_v7();
        let mut instance = make_instance(instance_id);
        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        fiber.wait = WaitState::Msg {
            wait_id: 1,
            name: 1,
            corr_key: "k".to_string(),
        };
        store.save_instance("default", &instance).await.unwrap();
        store.save_fiber(instance_id, &fiber).await.unwrap();

        assert_eq!(
            store
                .buffer_message(
                    ("default", "1", "b:false", "msg-atomic"),
                    br#"{"ok":true}"#,
                    Some([7u8; 32]),
                    60_000,
                    Some(instance_id),
                )
                .await
                .unwrap(),
            BufferMessageResult::Inserted
        );
        assert_eq!(
            store
                .buffer_message(
                    ("default", "1", "b:false", "msg-atomic"),
                    br#"{"ok":true}"#,
                    Some([7u8; 32]),
                    60_000,
                    Some(instance_id),
                )
                .await
                .unwrap(),
            BufferMessageResult::Duplicate
        );

        let claimed = store
            .claim_buffered_message(&TenantId::default(), "1", "b:false", 60_000)
            .await
            .unwrap()
            .expect("claimed message");
        assert!(store
            .claim_buffered_message(&TenantId::default(), "1", "b:false", 60_000)
            .await
            .unwrap()
            .is_none());

        fiber.wait = WaitState::Running;
        fiber.pc = Addr::new(1);
        let payload_update = PayloadUpdate {
            payload: r#"{"ok":true}"#.to_string(),
            payload_hash: [7u8; 32],
        };
        let events = vec![RuntimeEvent::BufferedMessageConsumed {
            message_name: "1".to_string(),
            correlation_key: "b:false".to_string(),
            msg_id: "msg-atomic".to_string(),
            fiber_id: fiber.fiber_id,
        }];
        assert!(store
            .atomic_consume_buffered_message(
                &instance,
                &fiber,
                &claimed,
                Some(&payload_update),
                &events,
            )
            .await
            .unwrap());
        instance = store
            .load_instance(&TenantId::default(), instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.domain_payload.as_ref(), r#"{"ok":true}"#);
        assert_eq!(
            store
                .buffer_message(
                    ("default", "1", "b:false", "msg-atomic"),
                    br#"{"ok":true}"#,
                    Some([7u8; 32]),
                    60_000,
                    Some(instance_id),
                )
                .await
                .unwrap(),
            BufferMessageResult::Duplicate
        );
    }

    /// A2.T5b: Enqueueing a job copies session_stack by value.
    #[tokio::test]
    async fn test_job_queue_session_stack_is_not_aliased() {
        let store = MemoryStore::new();
        let task_type = "create_case".to_string();
        let instance_id = Uuid::now_v7();
        let original_session_id = Uuid::new_v4();
        let mutated_session_id = Uuid::new_v4();

        store
            .save_instance("default", &make_instance(instance_id))
            .await
            .unwrap();

        let mut activation = JobActivation {
            job_key: "job-copy-test".to_string(),
            tenant_id: "default".to_string(),
            process_instance_id: instance_id,
            task_type: task_type.clone(),
            service_task_id: "task-copy-test".to_string(),
            domain_payload: "{}".to_string(),
            domain_payload_hash: [0u8; 32],
            session_stack: bpmn_lite_types::session_stack::SessionStackState {
                session_id: original_session_id,
                active_workspace: Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Kyc),
                trace_sequence: 11,
                ..Default::default()
            },
            orch_flags: BTreeMap::new(),
            retries_remaining: 3,
            entry_id: Uuid::new_v4(),
            runbook_id: Uuid::new_v4(),
            worker_id: String::new(),
            claim_token: String::new(),
            claim_expires_at: None,
            attempt_count: 0,
            failure_count: 0,
            not_before: None,
        };

        store.enqueue_job(&activation).await.unwrap();

        activation.session_stack.session_id = mutated_session_id;
        activation.session_stack.active_workspace =
            Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Deal);
        activation.session_stack.trace_sequence = 42;

        let batch = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                1,
                &TenantId::default(),
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].session_stack.session_id, original_session_id);
        assert_eq!(
            batch[0].session_stack.active_workspace,
            Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Kyc)
        );
        assert_eq!(batch[0].session_stack.trace_sequence, 11);
    }

    /// A2.T6: Event log: append 5 events, read from seq 3 returns 3 events
    #[tokio::test]
    async fn test_event_log() {
        let store = MemoryStore::new();
        let iid = Uuid::now_v7();

        for i in 0..5 {
            let event = RuntimeEvent::FlagSet {
                key: i,
                value: Value::I64(i as i64),
            };
            let seq = store.append_event(iid, &event).await.unwrap();
            assert_eq!(seq, (i + 1) as u64);
        }

        let events = store
            .read_events(&TenantId::default(), iid, 3)
            .await
            .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].0, 3);
        assert_eq!(events[1].0, 4);
        assert_eq!(events[2].0, 5);
    }

    /// A2.T7: Payload history: save 2 versions, load by hash
    #[tokio::test]
    async fn test_payload_history() {
        let store = MemoryStore::new();
        let iid = Uuid::now_v7();
        crate::store::commit_initial_snapshot(&store, make_instance(iid))
            .await
            .unwrap();

        let payload_v1 = r#"{"version":1}"#;
        let hash_v1 = test_hash(payload_v1);
        store
            .save_payload_version(iid, &hash_v1, payload_v1)
            .await
            .unwrap();

        let payload_v2 = r#"{"version":2}"#;
        let hash_v2 = test_hash(payload_v2);
        store
            .save_payload_version(iid, &hash_v2, payload_v2)
            .await
            .unwrap();

        let loaded_v1 = store
            .load_payload_version(&TenantId::default(), iid, &hash_v1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_v1, payload_v1);

        let loaded_v2 = store
            .load_payload_version(&TenantId::default(), iid, &hash_v2)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_v2, payload_v2);

        // Non-existent hash returns None
        let bad_hash = [0xFFu8; 32];
        assert!(store
            .load_payload_version(&TenantId::default(), iid, &bad_hash)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_transition_lease_excludes_other_owner_until_release() {
        let store = MemoryStore::new();
        let iid = Uuid::now_v7();
        store
            .save_instance("default", &make_instance(iid))
            .await
            .unwrap();

        assert!(store
            .claim_instance_for_transition(&TenantId::default(), iid, "owner-a", 5_000)
            .await
            .unwrap()
            .is_some());
        assert!(store
            .claim_instance_for_transition(&TenantId::default(), iid, "owner-b", 5_000)
            .await
            .unwrap()
            .is_none());
        assert!(store
            .claim_instance_for_transition(&TenantId::default(), iid, "owner-a", 5_000)
            .await
            .unwrap()
            .is_some());

        store
            .release_instance_transition(&TenantId::default(), iid, "owner-b")
            .await
            .unwrap();
        assert!(store
            .claim_instance_for_transition(&TenantId::default(), iid, "owner-b", 5_000)
            .await
            .unwrap()
            .is_none());

        store
            .release_instance_transition(&TenantId::default(), iid, "owner-a")
            .await
            .unwrap();
        assert!(store
            .claim_instance_for_transition(&TenantId::default(), iid, "owner-b", 5_000)
            .await
            .unwrap()
            .is_some());
    }

    /// T3.1.E1: Split-brain test (RISK-002) - atomic tick rollback
    #[tokio::test]
    async fn test_split_brain_rollback() {
        let store = MemoryStore::new();
        let instance_id = Uuid::now_v7();

        // 1. Setup pre-tick state: parent fiber exists, join barrier exists with 0 arrivals
        let parent_fiber_id = Uuid::now_v7();
        let parent_fiber = Fiber::new(parent_fiber_id, 0);

        let instance = ProcessInstance {
            instance_id,
            process_key: "test-proc".to_string(),
            bytecode_version: [0u8; 32],
            tenant_id: "default".to_string(),
            domain_payload: "{}".to_string().into(),
            domain_payload_hash: [0u8; 32],
            session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
            flags: BTreeMap::new(),
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: "corr".to_string(),
            entry_id: Uuid::new_v4(),
            runbook_id: Uuid::new_v4(),
            created_at: 0,
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: None,
        };

        store.save_instance("default", &instance).await.unwrap();
        store.save_fiber(instance_id, &parent_fiber).await.unwrap();

        // 2. Build tick operations with an injecting failure (wrong token claim)
        let child1_id = Uuid::now_v7();
        let child1 = Fiber::new(child1_id, 1);
        let child2_id = Uuid::now_v7();
        let child2 = Fiber::new(child2_id, 1);

        let msg = ClaimedBufferedMessage {
            message: BufferedMessage {
                tenant_id: "default".to_string(),
                message_name: "test-msg".to_string(),
                correlation_key: "test-key".to_string(),
                msg_id: "msg-123".to_string(),
                payload: vec![],
                payload_hash: None,
                process_instance_id: None,
                received_at: 0,
                expires_at: 300000,
            },
            claim_token: "wrong-token".to_string(),
            claim_until: 9999999999,
        };

        let ops = vec![
            TickOperation::SaveFiber { fiber: child1 },
            TickOperation::SaveFiber { fiber: child2 },
            TickOperation::JoinArrive { join_id: 10 },
            TickOperation::DeleteFiber {
                fiber_id: parent_fiber_id,
            },
            TickOperation::ConsumeBufferedMessage { message: msg }, // Fail!
        ];

        // 3. Commit tick - expect failure
        let res = commit_ops(&store, instance_id, &ops).await;
        assert!(res.is_err());

        // 4. Assert post-rollback state equals pre-tick state
        let loaded_fibers = store
            .load_fibers(&TenantId::default(), instance_id)
            .await
            .unwrap();
        assert_eq!(loaded_fibers.len(), 1);
        assert_eq!(loaded_fibers[0].fiber_id, parent_fiber_id);

        let join_count = store
            .join_get(&TenantId::default(), instance_id, 10)
            .await
            .unwrap();
        assert_eq!(join_count, 0);

        // 5. Re-run tick without the failing operation
        let successful_ops = vec![
            TickOperation::SaveFiber {
                fiber: Fiber::new(child1_id, 1),
            },
            TickOperation::SaveFiber {
                fiber: Fiber::new(child2_id, 1),
            },
            TickOperation::JoinArrive { join_id: 10 },
            TickOperation::DeleteFiber {
                fiber_id: parent_fiber_id,
            },
        ];
        commit_ops(&store, instance_id, &successful_ops)
            .await
            .unwrap();

        // 6. Assert correct completed state
        let loaded_fibers = store
            .load_fibers(&TenantId::default(), instance_id)
            .await
            .unwrap();
        assert_eq!(loaded_fibers.len(), 2);
        assert!(loaded_fibers.iter().any(|f| f.fiber_id == child1_id));
        assert!(loaded_fibers.iter().any(|f| f.fiber_id == child2_id));

        let join_count = store
            .join_get(&TenantId::default(), instance_id, 10)
            .await
            .unwrap();
        assert_eq!(join_count, 1);
    }

    /// T3.1.E2: Atomicity of events+state test
    #[tokio::test]
    async fn test_atomicity_events_and_state() {
        let store = MemoryStore::new();
        let instance_id = Uuid::now_v7();

        let instance = ProcessInstance {
            instance_id,
            process_key: "test-proc".to_string(),
            bytecode_version: [0u8; 32],
            tenant_id: "default".to_string(),
            domain_payload: "{}".to_string().into(),
            domain_payload_hash: [0u8; 32],
            session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
            flags: BTreeMap::new(),
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: "corr".to_string(),
            entry_id: Uuid::new_v4(),
            runbook_id: Uuid::new_v4(),
            created_at: 0,
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: None,
        };
        store.save_instance("default", &instance).await.unwrap();

        // Build a transactional update that fails
        let msg = ClaimedBufferedMessage {
            message: BufferedMessage {
                tenant_id: "default".to_string(),
                message_name: "test-msg".to_string(),
                correlation_key: "test-key".to_string(),
                msg_id: "msg-123".to_string(),
                payload: vec![],
                payload_hash: None,
                process_instance_id: None,
                received_at: 0,
                expires_at: 300000,
            },
            claim_token: "wrong-token".to_string(),
            claim_until: 9999999999,
        };

        let ops = vec![
            TickOperation::UpdateInstanceState {
                state: ProcessState::Completed { at: 12345 },
            },
            TickOperation::AppendEvent {
                event: RuntimeEvent::Completed { at: 12345 },
            },
            TickOperation::ConsumeBufferedMessage { message: msg }, // Fail!
        ];

        let res = commit_ops(&store, instance_id, &ops).await;
        assert!(res.is_err());

        // Assert neither state updated nor event logged
        let loaded = store
            .load_instance(&TenantId::default(), instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, ProcessState::Running);

        let events = store
            .read_events(&TenantId::default(), instance_id, 0)
            .await
            .unwrap();
        assert_eq!(events.len(), 0);

        // Commit successfully
        let successful_ops = vec![
            TickOperation::UpdateInstanceState {
                state: ProcessState::Completed { at: 12345 },
            },
            TickOperation::AppendEvent {
                event: RuntimeEvent::Completed { at: 12345 },
            },
        ];
        commit_ops(&store, instance_id, &successful_ops)
            .await
            .unwrap();

        // Assert both updated
        let loaded = store
            .load_instance(&TenantId::default(), instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, ProcessState::Completed { at: 12345 });

        let events = store
            .read_events(&TenantId::default(), instance_id, 0)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, RuntimeEvent::Completed { at: 12345 }));
    }

    /// `list_running_instances`/`claim_running_instances` filter on
    /// `ProcessState::is_schedulable()`, not `!is_terminal()` — an
    /// `Incidented` instance is not terminal (`ResolveIncident` revives
    /// it) but must still be excluded from scheduler ticking. Regression
    /// check on the common case: a genuinely `Running` instance must still
    /// be picked up.
    #[tokio::test]
    async fn list_and_claim_running_instances_exclude_incidented_but_include_running() {
        let store = MemoryStore::new();
        let running_id = Uuid::now_v7();
        let incidented_id = Uuid::now_v7();

        let mut running_inst = make_instance(running_id);
        running_inst.state = ProcessState::Running;
        store.save_instance("default", &running_inst).await.unwrap();

        let mut incidented_inst = make_instance(incidented_id);
        incidented_inst.state = ProcessState::Incidented {
            incident_id: Uuid::now_v7(),
        };
        store
            .save_instance("default", &incidented_inst)
            .await
            .unwrap();

        let tenant = TenantId::default();
        let listed = store.list_running_instances(&tenant).await.unwrap();
        assert!(
            listed.contains(&running_id),
            "a Running instance must still be scheduled"
        );
        assert!(
            !listed.contains(&incidented_id),
            "an Incidented instance must be excluded — it awaits ResolveIncident, not ticking"
        );

        let claimed = store
            .claim_running_instances(&tenant, "owner-1", 10, 30_000)
            .await
            .unwrap();
        assert!(claimed.contains(&running_id));
        assert!(!claimed.contains(&incidented_id));
    }
}
