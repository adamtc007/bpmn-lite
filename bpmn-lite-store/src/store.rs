use anyhow::Result;
use async_trait::async_trait;
use bpmn_lite_types::events::RuntimeEvent;
use bpmn_lite_types::*;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Persistence trait for all BPMN-Lite state.
///
/// 28 async methods organized by concern. The VM and engine operate exclusively
/// through this trait, enabling pluggable backends (MemoryStore for POC,
/// Postgres for production).
#[async_trait]
pub trait ProcessStore: Send + Sync {
    // ── Instance ──

    async fn save_instance(&self, instance: &ProcessInstance) -> Result<()>;
    async fn load_instance(&self, id: Uuid) -> Result<Option<ProcessInstance>>;
    async fn update_instance_state(&self, id: Uuid, state: ProcessState) -> Result<()>;
    async fn update_instance_flags(&self, id: Uuid, flags: &BTreeMap<FlagKey, Value>)
        -> Result<()>;
    async fn update_instance_payload(&self, id: Uuid, payload: &str, hash: &[u8; 32])
        -> Result<()>;

    // ── Fibers ──

    async fn save_fiber(&self, instance_id: Uuid, fiber: &Fiber) -> Result<()>;
    async fn load_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<Option<Fiber>>;
    async fn load_fibers(&self, instance_id: Uuid) -> Result<Vec<Fiber>>;
    async fn delete_fiber(&self, instance_id: Uuid, fiber_id: Uuid) -> Result<()>;
    async fn delete_all_fibers(&self, instance_id: Uuid) -> Result<()>;

    // ── Join barriers ──

    /// Increment the arrive count for a join barrier. Returns the new count.
    async fn join_arrive(&self, instance_id: Uuid, join_id: JoinId) -> Result<u16>;
    async fn join_reset(&self, instance_id: Uuid, join_id: JoinId) -> Result<()>;
    async fn join_delete_all(&self, instance_id: Uuid) -> Result<()>;

    // ── Dedupe cache (engine-side) ──

    async fn dedupe_get(&self, key: &str) -> Result<Option<JobCompletion>>;
    async fn dedupe_put(&self, key: &str, completion: &JobCompletion) -> Result<()>;
    async fn record_message_delivery(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        msg_id: &str,
    ) -> Result<bool>;

    // ── Job queue ──

    async fn enqueue_job(&self, activation: &JobActivation) -> Result<()>;
    async fn dequeue_jobs(
        &self,
        task_types: &[String],
        max: usize,
        tenant_id: &str,
        worker_id: &str,
        lease_ms: u64,
    ) -> Result<Vec<JobActivation>>;
    async fn ack_job(&self, job_key: &str) -> Result<()>;
    async fn validate_job_claim(
        &self,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> Result<bool>;
    async fn retry_claimed_job(
        &self,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
        error_class: &str,
        error_message: &str,
        not_before_ms: i64,
    ) -> Result<bool>;
    async fn dead_letter_claimed_job(
        &self,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
        error_class: &str,
        error_message: &str,
        incident_id: Uuid,
    ) -> Result<bool>;

    /// Cancel all pending and inflight jobs for an instance.
    /// Returns the list of cancelled job_keys.
    async fn cancel_jobs_for_instance(&self, instance_id: Uuid) -> Result<Vec<String>>;

    // ── Program store (versioned bytecode) ──

    async fn store_program(&self, version: [u8; 32], program: &CompiledProgram) -> Result<()>;
    async fn load_program(&self, version: [u8; 32]) -> Result<Option<CompiledProgram>>;

    // ── Dead-letter queue ──

    async fn dead_letter_put(
        &self,
        name: u32,
        corr_key: &Value,
        payload: &[u8],
        ttl_ms: u64,
    ) -> Result<()>;
    async fn dead_letter_take(&self, name: u32, corr_key: &Value) -> Result<Option<Vec<u8>>>;

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
    ) -> Result<BufferMessageResult>;
    async fn claim_buffered_message(
        &self,
        tenant_id: &str,
        message_name: &str,
        correlation_key: &str,
        claim_ms: u64,
    ) -> Result<Option<ClaimedBufferedMessage>>;
    async fn atomic_consume_buffered_message(
        &self,
        instance: &ProcessInstance,
        fiber: &Fiber,
        message: &ClaimedBufferedMessage,
        payload_update: Option<&PayloadUpdate>,
        events: &[RuntimeEvent],
    ) -> Result<bool>;
    async fn release_buffered_message_claim(
        &self,
        message: &ClaimedBufferedMessage,
    ) -> Result<bool>;
    async fn reclaim_stale_buffered_message_claims(&self) -> Result<u32>;
    async fn prune_expired_messages(&self) -> Result<u32>;

    // ── Event log (append-only) ──

    /// Append an event and return its sequence number.
    async fn append_event(&self, instance_id: Uuid, event: &RuntimeEvent) -> Result<u64>;
    async fn batch_append_events(&self, instance_id: Uuid, events: &[RuntimeEvent]) -> Result<u64> {
        let mut last_seq = 0;
        for event in events {
            last_seq = self.append_event(instance_id, event).await?;
        }
        Ok(last_seq)
    }
    async fn read_events(
        &self,
        instance_id: Uuid,
        from_seq: u64,
    ) -> Result<Vec<(u64, RuntimeEvent)>>;

    // ── Payload history (for PITR) ──

    async fn save_payload_version(
        &self,
        instance_id: Uuid,
        hash: &[u8; 32],
        payload: &str,
    ) -> Result<()>;
    async fn load_payload_version(
        &self,
        instance_id: Uuid,
        hash: &[u8; 32],
    ) -> Result<Option<String>>;

    // ── Incidents ──

    async fn save_incident(&self, incident: &Incident) -> Result<()>;
    async fn load_incidents(&self, instance_id: Uuid) -> Result<Vec<Incident>>;

    // ── Atomic compound operations ──

    /// Atomically start an instance: save instance + root fiber + InstanceStarted event.
    /// Returns the event sequence number.
    async fn atomic_start(
        &self,
        instance: &ProcessInstance,
        root_fiber: &Fiber,
        event: &RuntimeEvent,
    ) -> Result<u64>;

    /// Atomically complete a job: save instance + dedupe + payload version + events + ack job.
    async fn atomic_complete(
        &self,
        instance: &ProcessInstance,
        completion: &JobCompletion,
        events: &[RuntimeEvent],
    ) -> Result<()>;

    // ── Durability maintenance ──

    /// Reclaim jobs stuck in 'claimed' state past timeout. Returns count reclaimed.
    async fn reclaim_stale_jobs(&self, timeout_ms: u64) -> Result<u32>;

    /// Prune dedupe entries older than the given age. Returns count pruned.
    async fn prune_dedupe_cache(&self, older_than_ms: u64) -> Result<u32>;

    /// List all running (non-terminal) instance IDs.
    async fn list_running_instances(&self, tenant_id: &str) -> Result<Vec<Uuid>>;

    /// Claim a bounded batch of running instances for scheduler work.
    async fn claim_running_instances(
        &self,
        tenant_id: &str,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> Result<Vec<Uuid>>;

    /// Claim the mutation right for one process instance.
    ///
    /// This is a transition guard, not a scheduler batch primitive. Every
    /// mutating engine path should claim this before changing instance state.
    async fn claim_instance_for_transition(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
    ) -> Result<bool>;

    /// Release a previously claimed transition guard if this owner still holds it.
    async fn release_instance_transition(
        &self,
        tenant_id: &str,
        instance_id: Uuid,
        owner: &str,
    ) -> Result<()>;

    /// Lightweight readiness probe for the backing store.
    async fn health_check(&self) -> Result<()>;
}
