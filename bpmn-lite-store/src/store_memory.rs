use crate::store::ProcessStore;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use bpmn_lite_types::events::RuntimeEvent;
use bpmn_lite_types::*;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use uuid::Uuid;

struct Inner {
    instances: HashMap<Uuid, ProcessInstance>,
    fibers: HashMap<(Uuid, Uuid), Fiber>,
    join_counters: HashMap<(Uuid, JoinId), u16>,
    dedupe: HashMap<String, (JobCompletion, Instant)>,
    message_dedupe: HashSet<(String, Uuid, String)>,
    message_buffer: HashMap<(String, String, String, String), BufferedMessage>,
    message_buffer_claims: HashMap<(String, String, String, String), (String, i64)>,
    message_buffer_consumed: HashSet<(String, String, String, String)>,
    job_queue: VecDeque<JobActivation>,
    /// Jobs that have been dequeued but not yet acked.
    inflight_jobs: HashMap<String, (JobActivation, Instant)>,
    programs: HashMap<[u8; 32], CompiledProgram>,
    plans: HashMap<[u8; 32], String>,
    dead_letter: HashMap<(u32, String), (Vec<u8>, u64)>,
    events: HashMap<Uuid, Vec<(u64, RuntimeEvent)>>,
    event_seq: HashMap<Uuid, u64>,
    payload_history: HashMap<(Uuid, [u8; 32]), String>,
    incidents: HashMap<Uuid, Vec<Incident>>,
    transition_leases: HashMap<Uuid, (String, Instant)>,
}

/// In-memory implementation of `ProcessStore` for POC/testing.
pub struct MemoryStore {
    inner: RwLock<Inner>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                instances: HashMap::new(),
                fibers: HashMap::new(),
                join_counters: HashMap::new(),
                dedupe: HashMap::new(),
                message_dedupe: HashSet::new(),
                message_buffer: HashMap::new(),
                message_buffer_claims: HashMap::new(),
                message_buffer_consumed: HashSet::new(),
                job_queue: VecDeque::new(),
                inflight_jobs: HashMap::new(),
                programs: HashMap::new(),
                plans: HashMap::new(),
                dead_letter: HashMap::new(),
                events: HashMap::new(),
                event_seq: HashMap::new(),
                payload_history: HashMap::new(),
                incidents: HashMap::new(),
                transition_leases: HashMap::new(),
            }),
        }
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
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[async_trait]
impl ProcessStore for MemoryStore {
    // ── Instance ──

    async fn save_instance(&self, instance: &ProcessInstance) -> Result<()> {
        let mut w = self.inner.write().await;
        w.instances.insert(instance.instance_id, instance.clone());
        Ok(())
    }

    async fn load_instance(&self, id: Uuid) -> Result<Option<ProcessInstance>> {
        let r = self.inner.read().await;
        Ok(r.instances.get(&id).cloned())
    }

    async fn update_instance_state(&self, id: Uuid, state: ProcessState) -> Result<()> {
        let mut w = self.inner.write().await;
        let inst = w
            .instances
            .get_mut(&id)
            .ok_or_else(|| anyhow!("instance not found: {id}"))?;
        inst.state = state;
        Ok(())
    }

    async fn update_instance_flags(
        &self,
        id: Uuid,
        flags: &BTreeMap<FlagKey, Value>,
    ) -> Result<()> {
        let mut w = self.inner.write().await;
        let inst = w
            .instances
            .get_mut(&id)
            .ok_or_else(|| anyhow!("instance not found: {id}"))?;
        inst.flags = flags.clone();
        Ok(())
    }

    async fn update_instance_payload(
        &self,
        id: Uuid,
        payload: &str,
        hash: &[u8; 32],
    ) -> Result<()> {
        let mut w = self.inner.write().await;
        let inst = w
            .instances
            .get_mut(&id)
            .ok_or_else(|| anyhow!("instance not found: {id}"))?;
        inst.domain_payload = Arc::<str>::from(payload);
        inst.domain_payload_hash = *hash;
        Ok(())
    }

    // ── Fibers ──

    async fn save_fiber(&self, instance_id: Uuid, fiber: &Fiber) -> Result<()> {
        let mut w = self.inner.write().await;
        w.fibers
            .insert((instance_id, fiber.fiber_id), fiber.clone());
        Ok(())
    }

    async fn load_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<Option<Fiber>> {
        let r = self.inner.read().await;
        Ok(r.fibers.get(&(instance_id, fiber_id)).cloned())
    }

    async fn load_fibers(&self, instance_id: Uuid) -> Result<Vec<Fiber>> {
        let r = self.inner.read().await;
        Ok(r.fibers
            .iter()
            .filter(|((iid, _), _)| *iid == instance_id)
            .map(|(_, f)| f.clone())
            .collect())
    }

    async fn delete_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<()> {
        let mut w = self.inner.write().await;
        w.fibers.remove(&(instance_id, fiber_id));
        Ok(())
    }

    async fn delete_all_fibers(&self, instance_id: Uuid) -> Result<()> {
        let mut w = self.inner.write().await;
        w.fibers.retain(|(iid, _), _| *iid != instance_id);
        Ok(())
    }

    // ── Join barriers ──

    async fn join_arrive(&self, instance_id: Uuid, join_id: JoinId) -> Result<u16> {
        let mut w = self.inner.write().await;
        let count = w.join_counters.entry((instance_id, join_id)).or_insert(0);
        *count += 1;
        Ok(*count)
    }

    async fn join_reset(&self, instance_id: Uuid, join_id: JoinId) -> Result<()> {
        let mut w = self.inner.write().await;
        w.join_counters.insert((instance_id, join_id), 0);
        Ok(())
    }

    async fn join_delete_all(&self, instance_id: Uuid) -> Result<()> {
        let mut w = self.inner.write().await;
        w.join_counters.retain(|(iid, _), _| *iid != instance_id);
        Ok(())
    }

    // ── Dedupe cache ──

    async fn dedupe_get(&self, key: &str) -> Result<Option<JobCompletion>> {
        let r = self.inner.read().await;
        Ok(r.dedupe.get(key).map(|(c, _)| c.clone()))
    }

    async fn dedupe_put(&self, key: &str, completion: &JobCompletion) -> Result<()> {
        let mut w = self.inner.write().await;
        w.dedupe
            .insert(key.to_string(), (completion.clone(), Instant::now()));
        Ok(())
    }

    async fn record_message_delivery(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        msg_id: &str,
    ) -> Result<bool> {
        let mut w = self.inner.write().await;
        Ok(w.message_dedupe
            .insert((tenant_id.to_string(), instance_id, msg_id.to_string())))
    }

    // ── Job queue ──

    async fn enqueue_job(&self, activation: &JobActivation) -> Result<()> {
        let mut w = self.inner.write().await;
        if w.job_queue
            .iter()
            .any(|job| job.job_key == activation.job_key)
            || w.inflight_jobs.contains_key(&activation.job_key)
        {
            return Ok(());
        }
        w.job_queue.push_back(activation.clone());
        Ok(())
    }

    async fn dequeue_jobs(
        &self,
        task_types: &[String],
        max: usize,
        tenant_id: &str,
        worker_id: &str,
        lease_ms: u64,
    ) -> Result<Vec<JobActivation>> {
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
                .map(|instance_tenant| instance_tenant == tenant_id)
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

    async fn ack_job(&self, job_key: &str) -> Result<()> {
        let mut w = self.inner.write().await;
        w.inflight_jobs.remove(job_key);
        Ok(())
    }

    async fn validate_job_claim(
        &self,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> Result<bool> {
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

    async fn retry_claimed_job(
        &self,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
        error_class: &str,
        error_message: &str,
        not_before_ms: i64,
    ) -> Result<bool> {
        let mut w = self.inner.write().await;
        let Some((mut job, _)) = w.inflight_jobs.remove(job_key) else {
            return Ok(false);
        };
        if job.worker_id != worker_id || job.claim_token != claim_token {
            w.inflight_jobs
                .insert(job.job_key.clone(), (job, Instant::now()));
            return Ok(false);
        }
        if job.retries_remaining <= 1 {
            w.inflight_jobs
                .insert(job.job_key.clone(), (job, Instant::now()));
            return Ok(false);
        }
        job.worker_id.clear();
        job.claim_token.clear();
        job.claim_expires_at = None;
        job.not_before = Some(not_before_ms);
        job.failure_count += 1;
        job.retries_remaining = job.retries_remaining.saturating_sub(1);
        job.orch_flags.insert(
            "last_error_class".to_string(),
            Value::Str(error_class.len() as u32),
        );
        let _ = error_message;
        w.job_queue.push_back(job);
        Ok(true)
    }

    async fn dead_letter_claimed_job(
        &self,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
        _error_class: &str,
        _error_message: &str,
        _incident_id: Uuid,
    ) -> Result<bool> {
        let mut w = self.inner.write().await;
        let Some((job, _)) = w.inflight_jobs.remove(job_key) else {
            return Ok(false);
        };
        if job.worker_id != worker_id || job.claim_token != claim_token {
            w.inflight_jobs
                .insert(job.job_key.clone(), (job, Instant::now()));
            return Ok(false);
        }
        Ok(true)
    }

    async fn cancel_jobs_for_instance(&self, instance_id: Uuid) -> Result<Vec<String>> {
        let mut w = self.inner.write().await;
        let mut cancelled = Vec::new();

        // Remove from job_queue
        let mut remaining = VecDeque::new();
        while let Some(job) = w.job_queue.pop_front() {
            if job.process_instance_id == instance_id {
                cancelled.push(job.job_key.clone());
            } else {
                remaining.push_back(job);
            }
        }
        w.job_queue = remaining;

        // Remove from inflight_jobs
        let inflight_keys: Vec<String> = w
            .inflight_jobs
            .iter()
            .filter(|(_, (job, _))| job.process_instance_id == instance_id)
            .map(|(key, _)| key.clone())
            .collect();
        for key in inflight_keys {
            w.inflight_jobs.remove(&key);
            cancelled.push(key);
        }

        Ok(cancelled)
    }

    // ── Program store ──

    async fn store_program(&self, version: [u8; 32], program: &CompiledProgram) -> Result<()> {
        let mut w = self.inner.write().await;
        w.programs.insert(version, program.clone());
        Ok(())
    }

    async fn load_program(&self, version: [u8; 32]) -> Result<Option<CompiledProgram>> {
        let r = self.inner.read().await;
        Ok(r.programs.get(&version).cloned())
    }

    async fn store_plan(&self, plan_hash: [u8; 32], plan_json: &str) -> Result<()> {
        let mut w = self.inner.write().await;
        w.plans.entry(plan_hash).or_insert_with(|| plan_json.to_owned());
        Ok(())
    }

    async fn load_plan(&self, plan_hash: [u8; 32]) -> Result<Option<String>> {
        let r = self.inner.read().await;
        Ok(r.plans.get(&plan_hash).cloned())
    }

    // ── Dead-letter queue ──

    async fn dead_letter_put(
        &self,
        name: u32,
        corr_key: &Value,
        payload: &[u8],
        ttl_ms: u64,
    ) -> Result<()> {
        let mut w = self.inner.write().await;
        let key = (name, value_key(corr_key));
        w.dead_letter.insert(key, (payload.to_vec(), ttl_ms));
        Ok(())
    }

    async fn dead_letter_take(&self, name: u32, corr_key: &Value) -> Result<Option<Vec<u8>>> {
        let mut w = self.inner.write().await;
        let key = (name, value_key(corr_key));
        Ok(w.dead_letter.remove(&key).map(|(data, _)| data))
    }

    #[allow(clippy::too_many_arguments)]
    async fn buffer_message(
        &self,
        tenant_id: &str,
        message_name: &str,
        correlation_key: &str,
        msg_id: &str,
        payload: &[u8],
        payload_hash: Option<[u8; 32]>,
        ttl_ms: u64,
        process_instance_id: Option<Uuid>,
    ) -> Result<BufferMessageResult> {
        let mut w = self.inner.write().await;
        let key = (
            tenant_id.to_string(),
            message_name.to_string(),
            correlation_key.to_string(),
            msg_id.to_string(),
        );
        if w.message_buffer.contains_key(&key) || w.message_buffer_consumed.contains(&key) {
            return Ok(BufferMessageResult::Duplicate);
        }
        let received_at = now_ms();
        w.message_buffer.insert(
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

    async fn claim_buffered_message(
        &self,
        tenant_id: &str,
        message_name: &str,
        correlation_key: &str,
        claim_ms: u64,
    ) -> Result<Option<ClaimedBufferedMessage>> {
        let mut w = self.inner.write().await;
        let now = now_ms();
        let key = w
            .message_buffer
            .iter()
            .filter(|((tenant, name, corr, _), msg)| {
                tenant == tenant_id
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

    async fn atomic_consume_buffered_message(
        &self,
        instance: &ProcessInstance,
        fiber: &Fiber,
        message: &ClaimedBufferedMessage,
        payload_update: Option<&PayloadUpdate>,
        events: &[RuntimeEvent],
    ) -> Result<bool> {
        let mut w = self.inner.write().await;
        let key = (
            message.message.tenant_id.clone(),
            message.message.message_name.clone(),
            message.message.correlation_key.clone(),
            message.message.msg_id.clone(),
        );
        let Some((claim_token, claim_until)) = w.message_buffer_claims.get(&key) else {
            return Ok(false);
        };
        if claim_token != &message.claim_token || *claim_until != message.claim_until {
            return Ok(false);
        }
        if *claim_until <= now_ms() {
            w.message_buffer_claims.remove(&key);
            return Ok(false);
        }

        let mut instance = instance.clone();
        if let Some(payload_update) = payload_update {
            instance.domain_payload = Arc::from(payload_update.payload.as_str());
            instance.domain_payload_hash = payload_update.payload_hash;
            w.payload_history.insert(
                (instance.instance_id, payload_update.payload_hash),
                payload_update.payload.clone(),
            );
        }
        w.instances.insert(instance.instance_id, instance.clone());
        w.fibers
            .insert((instance.instance_id, fiber.fiber_id), fiber.clone());
        for event in events {
            let seq = w.event_seq.entry(instance.instance_id).or_insert(0);
            *seq += 1;
            let current_seq = *seq;
            w.events
                .entry(instance.instance_id)
                .or_default()
                .push((current_seq, event.clone()));
        }
        w.message_buffer_claims.remove(&key);
        w.message_buffer.remove(&key);
        w.message_buffer_consumed.insert(key);
        Ok(true)
    }

    async fn release_buffered_message_claim(
        &self,
        message: &ClaimedBufferedMessage,
    ) -> Result<bool> {
        let mut w = self.inner.write().await;
        let key = (
            message.message.tenant_id.clone(),
            message.message.message_name.clone(),
            message.message.correlation_key.clone(),
            message.message.msg_id.clone(),
        );
        let Some((claim_token, _)) = w.message_buffer_claims.get(&key) else {
            return Ok(false);
        };
        if claim_token != &message.claim_token {
            return Ok(false);
        }
        w.message_buffer_claims.remove(&key);
        Ok(true)
    }

    async fn reclaim_stale_buffered_message_claims(&self) -> Result<u32> {
        let mut w = self.inner.write().await;
        let now = now_ms();
        let before = w.message_buffer_claims.len();
        w.message_buffer_claims
            .retain(|_, (_, claim_until)| *claim_until > now);
        Ok((before - w.message_buffer_claims.len()) as u32)
    }

    async fn prune_expired_messages(&self) -> Result<u32> {
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

    // ── Event log ──

    async fn append_event(&self, instance_id: Uuid, event: &RuntimeEvent) -> Result<u64> {
        let mut w = self.inner.write().await;
        let seq = w.event_seq.entry(instance_id).or_insert(0);
        *seq += 1;
        let current_seq = *seq;
        w.events
            .entry(instance_id)
            .or_default()
            .push((current_seq, event.clone()));
        Ok(current_seq)
    }

    async fn batch_append_events(&self, instance_id: Uuid, events: &[RuntimeEvent]) -> Result<u64> {
        let mut w = self.inner.write().await;
        let mut last_seq = 0;
        for event in events {
            let seq = w.event_seq.entry(instance_id).or_insert(0);
            *seq += 1;
            let current_seq = *seq;
            w.events
                .entry(instance_id)
                .or_default()
                .push((current_seq, event.clone()));
            last_seq = current_seq;
        }
        Ok(last_seq)
    }

    async fn read_events(
        &self,
        instance_id: Uuid,
        from_seq: u64,
    ) -> Result<Vec<(u64, RuntimeEvent)>> {
        let r = self.inner.read().await;
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

    // ── Payload history ──

    async fn save_payload_version(
        &self,
        instance_id: Uuid,
        hash: &[u8; 32],
        payload: &str,
    ) -> Result<()> {
        let mut w = self.inner.write().await;
        w.payload_history
            .insert((instance_id, *hash), payload.to_string());
        Ok(())
    }

    async fn load_payload_version(
        &self,
        instance_id: Uuid,
        hash: &[u8; 32],
    ) -> Result<Option<String>> {
        let r = self.inner.read().await;
        Ok(r.payload_history.get(&(instance_id, *hash)).cloned())
    }

    // ── Incidents ──

    async fn save_incident(&self, incident: &Incident) -> Result<()> {
        let mut w = self.inner.write().await;
        w.incidents
            .entry(incident.process_instance_id)
            .or_default()
            .push(incident.clone());
        Ok(())
    }

    async fn load_incidents(&self, instance_id: Uuid) -> Result<Vec<Incident>> {
        let r = self.inner.read().await;
        Ok(r.incidents.get(&instance_id).cloned().unwrap_or_default())
    }

    // ── Atomic compound operations ──

    async fn atomic_start(
        &self,
        instance: &ProcessInstance,
        root_fiber: &Fiber,
        event: &RuntimeEvent,
    ) -> Result<u64> {
        let mut w = self.inner.write().await;
        // save instance
        w.instances.insert(instance.instance_id, instance.clone());
        // save fiber
        w.fibers.insert(
            (instance.instance_id, root_fiber.fiber_id),
            root_fiber.clone(),
        );
        // append event
        let seq = w.event_seq.entry(instance.instance_id).or_insert(0);
        *seq += 1;
        let current_seq = *seq;
        w.events
            .entry(instance.instance_id)
            .or_default()
            .push((current_seq, event.clone()));
        Ok(current_seq)
    }

    async fn atomic_complete(
        &self,
        instance: &ProcessInstance,
        completion: &JobCompletion,
        events: &[RuntimeEvent],
    ) -> Result<()> {
        let mut w = self.inner.write().await;
        // save instance (upsert)
        w.instances.insert(instance.instance_id, instance.clone());
        // dedupe put
        w.dedupe.insert(
            completion.job_key.clone(),
            (completion.clone(), Instant::now()),
        );
        // save payload version
        w.payload_history.insert(
            (instance.instance_id, instance.domain_payload_hash),
            instance.domain_payload.to_string(),
        );
        for event in events {
            let seq = w.event_seq.entry(instance.instance_id).or_insert(0);
            *seq += 1;
            let current_seq = *seq;
            w.events
                .entry(instance.instance_id)
                .or_default()
                .push((current_seq, event.clone()));
        }
        w.inflight_jobs.remove(&completion.job_key);
        w.job_queue.retain(|job| job.job_key != completion.job_key);
        Ok(())
    }

    // ── Durability maintenance ──

    async fn reclaim_stale_jobs(&self, _timeout_ms: u64) -> Result<u32> {
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

    async fn prune_dedupe_cache(&self, older_than_ms: u64) -> Result<u32> {
        let mut w = self.inner.write().await;
        let threshold = std::time::Duration::from_millis(older_than_ms);
        let now = Instant::now();
        let before = w.dedupe.len();
        w.dedupe
            .retain(|_, (_, created_at)| now.duration_since(*created_at) <= threshold);
        Ok((before - w.dedupe.len()) as u32)
    }

    async fn list_running_instances(&self, tenant_id: &str) -> Result<Vec<Uuid>> {
        let r = self.inner.read().await;
        Ok(r.instances
            .iter()
            .filter(|(_, inst)| !inst.state.is_terminal() && inst.tenant_id == tenant_id)
            .map(|(id, _)| *id)
            .collect())
    }

    async fn claim_running_instances(
        &self,
        tenant_id: &str,
        _owner: &str,
        limit: usize,
        _lease_ms: u64,
    ) -> Result<Vec<Uuid>> {
        let ids = self.list_running_instances(tenant_id).await?;
        Ok(ids.into_iter().take(limit).collect())
    }

    async fn claim_instance_for_transition(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
    ) -> Result<bool> {
        let mut w = self.inner.write().await;
        let Some(instance) = w.instances.get(&instance_id) else {
            return Ok(false);
        };
        if instance.tenant_id != tenant_id {
            return Ok(false);
        }

        let now = Instant::now();
        let lease_until = now + Duration::from_millis(lease_ms);
        match w.transition_leases.get(&instance_id) {
            Some((current_owner, expires_at)) if current_owner != owner && *expires_at > now => {
                Ok(false)
            }
            _ => {
                w.transition_leases
                    .insert(instance_id, (owner.to_string(), lease_until));
                Ok(true)
            }
        }
    }

    async fn release_instance_transition(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        owner: &str,
    ) -> Result<()> {
        let mut w = self.inner.write().await;
        let same_tenant = w
            .instances
            .get(&instance_id)
            .map(|instance| instance.tenant_id == tenant_id)
            .unwrap_or(false);
        if same_tenant
            && matches!(
                w.transition_leases.get(&instance_id),
                Some((current_owner, _)) if current_owner == owner
            )
        {
            w.transition_leases.remove(&instance_id);
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        Ok(())
    }

    async fn ensure_tenant(&self, _tenant_id: &str) -> Result<()> {
        Ok(()) // MemoryStore is single-process; tenant registration is a no-op.
    }

    async fn list_tenants(&self) -> Result<Vec<String>> {
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

    async fn list_tenants_in_pool(&self, pool_id: &str) -> Result<Vec<String>> {
        // MemoryStore has no pool concept; 'default' returns all known tenants,
        // other pool IDs return empty (consistent with an empty dedicated pool).
        if pool_id == "default" {
            self.list_tenants().await
        } else {
            Ok(vec![])
        }
    }

    async fn quarantine_instance(
        &self,
        instance_id: Uuid,
        _tenant_id: &str,
        _detection_point: &str,
    ) -> Result<()> {
        let mut guard = self.inner.write().await;
        if let Some(inst) = guard.instances.get_mut(&instance_id) {
            inst.quarantine_state = Some("integrity_violation".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

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

        store.save_instance(&inst).await.unwrap();
        let loaded = store.load_instance(id).await.unwrap().unwrap();

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

        store.save_instance(&inst).await.unwrap();

        inst.session_stack.session_id = mutated_session_id;
        inst.session_stack.scope = Some(bpmn_lite_types::session_stack::SessionScopeState {
            client_group_id: mutated_scope_id,
            client_group_name: Some("Mutated".to_string()),
        });
        inst.session_stack.active_workspace =
            Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Deal);
        inst.session_stack.trace_sequence = 99;

        let loaded = store.load_instance(id).await.unwrap().unwrap();
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
        let loaded = store.load_fiber(iid, fid).await.unwrap().unwrap();
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
        assert!(store.load_fiber(iid, fid).await.unwrap().is_none());
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

        assert!(store.dedupe_get("job-abc").await.unwrap().is_none());
        store.dedupe_put("job-abc", &completion).await.unwrap();

        let cached = store.dedupe_get("job-abc").await.unwrap().unwrap();
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
                .save_instance(&make_instance(instance_id))
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
                "default",
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
            .validate_job_claim("job-0", "test-worker", &batch1[0].claim_token)
            .await
            .unwrap());
        assert!(!store
            .validate_job_claim("job-0", "other-worker", &batch1[0].claim_token)
            .await
            .unwrap());
        assert!(batch1
            .iter()
            .all(|job| job.session_stack.session_id == session_id));

        // Ack one
        store.ack_job("job-0").await.unwrap();

        // Dequeue remaining
        let batch2 = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                10,
                "default",
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
            .save_instance(&make_instance(instance_id))
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
                "default",
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
                "default",
                "worker-a",
                1,
            )
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].attempt_count, 1);
        assert!(claimed[0].claim_expires_at.is_some());
        assert!(store
            .validate_job_claim("lease-job", "worker-a", &claimed[0].claim_token)
            .await
            .unwrap());

        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        assert!(!store
            .validate_job_claim("lease-job", "worker-a", &claimed[0].claim_token)
            .await
            .unwrap());
        assert_eq!(store.reclaim_stale_jobs(0).await.unwrap(), 1);

        let reclaimed = store
            .dequeue_jobs(
                std::slice::from_ref(&task_type),
                1,
                "default",
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
                .buffer_message("default", "1", "b:false", "msg-1", b"{}", None, 60_000, None)
                .await
                .unwrap(),
            BufferMessageResult::Inserted
        );
        assert_eq!(
            store
                .buffer_message("default", "1", "b:false", "msg-1", b"{}", None, 60_000, None)
                .await
                .unwrap(),
            BufferMessageResult::Duplicate
        );

        let claimed = store
            .claim_buffered_message("default", "1", "b:false", 60_000)
            .await
            .unwrap()
            .expect("buffered message");
        assert_eq!(claimed.message.msg_id, "msg-1");
        assert!(store
            .claim_buffered_message("default", "1", "b:false", 60_000)
            .await
            .unwrap()
            .is_none());
        assert!(store
            .release_buffered_message_claim(&claimed)
            .await
            .unwrap());
        assert!(store
            .claim_buffered_message("default", "1", "b:false", 60_000)
            .await
            .unwrap()
            .is_some());

        store
            .buffer_message("default", "1", "b:false", "expired", b"{}", None, 0, None)
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
            corr_key: Value::Bool(false),
        };
        store.save_instance(&instance).await.unwrap();
        store.save_fiber(instance_id, &fiber).await.unwrap();

        assert_eq!(
            store
                .buffer_message(
                    "default",
                    "1",
                    "b:false",
                    "msg-atomic",
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
                    "default",
                    "1",
                    "b:false",
                    "msg-atomic",
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
            .claim_buffered_message("default", "1", "b:false", 60_000)
            .await
            .unwrap()
            .expect("claimed message");
        assert!(store
            .claim_buffered_message("default", "1", "b:false", 60_000)
            .await
            .unwrap()
            .is_none());

        fiber.wait = WaitState::Running;
        fiber.pc = 1;
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
        instance = store.load_instance(instance_id).await.unwrap().unwrap();
        assert_eq!(instance.domain_payload.as_ref(), r#"{"ok":true}"#);
        assert_eq!(
            store
                .buffer_message(
                    "default",
                    "1",
                    "b:false",
                    "msg-atomic",
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
            .save_instance(&make_instance(instance_id))
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
                "default",
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

        let events = store.read_events(iid, 3).await.unwrap();
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
            .load_payload_version(iid, &hash_v1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_v1, payload_v1);

        let loaded_v2 = store
            .load_payload_version(iid, &hash_v2)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_v2, payload_v2);

        // Non-existent hash returns None
        let bad_hash = [0xFFu8; 32];
        assert!(store
            .load_payload_version(iid, &bad_hash)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_transition_lease_excludes_other_owner_until_release() {
        let store = MemoryStore::new();
        let iid = Uuid::now_v7();
        store.save_instance(&make_instance(iid)).await.unwrap();

        assert!(store
            .claim_instance_for_transition("default", iid, "owner-a", 5_000)
            .await
            .unwrap());
        assert!(!store
            .claim_instance_for_transition("default", iid, "owner-b", 5_000)
            .await
            .unwrap());
        assert!(store
            .claim_instance_for_transition("default", iid, "owner-a", 5_000)
            .await
            .unwrap());

        store
            .release_instance_transition("default", iid, "owner-b")
            .await
            .unwrap();
        assert!(!store
            .claim_instance_for_transition("default", iid, "owner-b", 5_000)
            .await
            .unwrap());

        store
            .release_instance_transition("default", iid, "owner-a")
            .await
            .unwrap();
        assert!(store
            .claim_instance_for_transition("default", iid, "owner-b", 5_000)
            .await
            .unwrap());
    }
}
