//! Multi-crate application vertical: `bpmn_lite_engine::BpmnLiteEngine`
//! driving a real `bpmn-lite-store-postgres` store wrapped in a
//! fault-injecting `RuntimeStore`/`ArtifactRepository`/`JournalReader`/
//! `AdminProjectionStore` test double (`ViolatingTestStore`), proving T7/
//! T10's integrity-violation-propagates-and-quarantines contract end to
//! end — including the `ffi_catalogue`/`ffi_dispatcher` startup-recovery
//! path. Moved from `bpmn-lite-store-postgres/src/store_postgres.rs`'s
//! `mod tests` under EOP-PLAN-CRATE-HYGIENE-001 H1 (work item 3): these
//! tests construct `BpmnLiteEngine` (and, for the startup-recovery case,
//! `FfiCatalogue`/`FfiDispatcher`), reaching beyond the store's own
//! persistence contract, so they no longer belong in the store crate's
//! unit tests. `ViolatingTestStore` is shared by all three tests here
//! (it is not needed by any test that stayed behind), so it is not
//! duplicated per-test.

mod common;

use async_trait::async_trait;
use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_store::store::{
    AdminProjectionStore, ArtifactRepository, DesignSessionEventKind, DesignSessionRecord,
    DesignSessionSummary, DevCaptureSessionRecord, JournalReader, RuntimeStore,
};
use bpmn_lite_store::{ArtifactStoreError, ClaimError, CommitError, CommitOutcome, StoreError, StoreResult, TemplateSummary};
use bpmn_lite_store_postgres::PostgresWorkflowStore;
use bpmn_lite_types::*;
use std::sync::Arc;
use uuid::Uuid;

/// Minimal single-task BPMN shared with the engine/kernel vertical file's
/// `SMOKE_BPMN` — duplicated rather than shared across `tests/*.rs` binary
/// crates, which cannot import each other's non-`common` items.
const SMOKE_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="smoke_proc" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="do_work" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

struct ViolatingTestStore {
    inner: Arc<PostgresWorkflowStore>,
    violate_instance_id: Uuid,
    should_fail_load_integrity: std::sync::atomic::AtomicBool,
    should_fail_commit_integrity: std::sync::atomic::AtomicBool,
    should_fail_generic: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl RuntimeStore for ViolatingTestStore {
    async fn load_instance(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
    ) -> StoreResult<Option<ProcessInstance>> {
        if id == self.violate_instance_id {
            if self
                .should_fail_load_integrity
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                return Err(StoreError::Integrity(
                    "injected instance integrity violation".into(),
                ));
            }
            if self
                .should_fail_generic
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                return Err(StoreError::Unavailable(
                    "injected generic load failure".into(),
                ));
            }
        }
        <PostgresWorkflowStore as RuntimeStore>::load_instance(&*self.inner, tenant_id, id).await
    }

    async fn load_fiber(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        fiber_id: Uuid,
    ) -> StoreResult<Option<Fiber>> {
        <PostgresWorkflowStore as RuntimeStore>::load_fiber(
            &*self.inner,
            tenant_id,
            instance_id,
            fiber_id,
        )
        .await
    }

    async fn load_fibers(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Vec<Fiber>> {
        <PostgresWorkflowStore as RuntimeStore>::load_fibers(&*self.inner, tenant_id, instance_id)
            .await
    }

    async fn dedupe_get(
        &self,
        tenant_id: &TenantId,
        key: &str,
    ) -> StoreResult<Option<JobCompletion>> {
        <PostgresWorkflowStore as RuntimeStore>::dedupe_get(&*self.inner, tenant_id, key).await
    }

    async fn dequeue_jobs(
        &self,
        task_types: &[String],
        max: usize,
        tenant_id: &TenantId,
        worker_id: &str,
        lease_ms: u64,
    ) -> StoreResult<Vec<JobActivation>> {
        <PostgresWorkflowStore as RuntimeStore>::dequeue_jobs(
            &*self.inner,
            task_types,
            max,
            tenant_id,
            worker_id,
            lease_ms,
        )
        .await
    }

    async fn validate_job_claim(
        &self,
        tenant_id: &TenantId,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> StoreResult<bool> {
        <PostgresWorkflowStore as RuntimeStore>::validate_job_claim(
            &*self.inner,
            tenant_id,
            job_key,
            worker_id,
            claim_token,
        )
        .await
    }

    async fn dead_letter_put(
        &self,
        name: u32,
        corr_key: &Value,
        payload: &[u8],
        ttl_ms: u64,
    ) -> StoreResult<()> {
        <PostgresWorkflowStore as RuntimeStore>::dead_letter_put(
            &*self.inner,
            name,
            corr_key,
            payload,
            ttl_ms,
        )
        .await
    }

    async fn dead_letter_take(
        &self,
        name: u32,
        corr_key: &Value,
    ) -> StoreResult<Option<Vec<u8>>> {
        <PostgresWorkflowStore as RuntimeStore>::dead_letter_take(&*self.inner, name, corr_key)
            .await
    }

    async fn claim_buffered_message(
        &self,
        tenant_id: &TenantId,
        message_name: &str,
        correlation_key: &str,
        claim_ms: u64,
    ) -> StoreResult<Option<ClaimedBufferedMessage>> {
        <PostgresWorkflowStore as RuntimeStore>::claim_buffered_message(
            &*self.inner,
            tenant_id,
            message_name,
            correlation_key,
            claim_ms,
        )
        .await
    }

    async fn reclaim_stale_buffered_message_claims(&self) -> StoreResult<u32> {
        <PostgresWorkflowStore as RuntimeStore>::reclaim_stale_buffered_message_claims(
            &*self.inner,
        )
        .await
    }

    async fn prune_expired_messages(&self) -> StoreResult<u32> {
        <PostgresWorkflowStore as RuntimeStore>::prune_expired_messages(&*self.inner).await
    }

    async fn load_incidents(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Vec<Incident>> {
        <PostgresWorkflowStore as RuntimeStore>::load_incidents(
            &*self.inner,
            tenant_id,
            instance_id,
        )
        .await
    }

    async fn reclaim_stale_jobs(&self) -> StoreResult<u32> {
        <PostgresWorkflowStore as RuntimeStore>::reclaim_stale_jobs(&*self.inner).await
    }

    async fn prune_dedupe_cache(&self, older_than_ms: u64) -> StoreResult<u32> {
        <PostgresWorkflowStore as RuntimeStore>::prune_dedupe_cache(&*self.inner, older_than_ms)
            .await
    }

    async fn list_running_instances(&self, tenant_id: &TenantId) -> StoreResult<Vec<Uuid>> {
        <PostgresWorkflowStore as RuntimeStore>::list_running_instances(&*self.inner, tenant_id)
            .await
    }

    async fn claim_running_instances(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<Uuid>> {
        <PostgresWorkflowStore as RuntimeStore>::claim_running_instances(
            &*self.inner,
            tenant_id,
            owner,
            limit,
            lease_ms,
        )
        .await
    }

    async fn claim_instance_for_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
    ) -> std::result::Result<Option<Claim>, ClaimError> {
        <PostgresWorkflowStore as RuntimeStore>::claim_instance_for_transition(
            &*self.inner,
            tenant_id,
            instance_id,
            owner,
            lease_ms,
        )
        .await
    }

    async fn commit_transition(
        &self,
        claim: &Claim,
        transition: &Transition,
    ) -> std::result::Result<CommitOutcome, CommitError> {
        if claim.instance_id() == self.violate_instance_id
            && self
                .should_fail_commit_integrity
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(CommitError::Integrity(
                "injected transition integrity violation".into(),
            ));
        }
        <PostgresWorkflowStore as RuntimeStore>::commit_transition(&*self.inner, claim, transition)
            .await
    }

    async fn lookup_start_instance(
        &self,
        tenant_id: &TenantId,
        idempotency_key: Uuid,
    ) -> StoreResult<Option<Uuid>> {
        <PostgresWorkflowStore as RuntimeStore>::lookup_start_instance(
            &*self.inner,
            tenant_id,
            idempotency_key,
        )
        .await
    }

    async fn claim_due_timers(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedTimer>> {
        <PostgresWorkflowStore as RuntimeStore>::claim_due_timers(
            &*self.inner,
            tenant_id,
            owner,
            now_ms,
            limit,
            lease_ms,
        )
        .await
    }

    async fn release_timer_claim(&self, timer: &ClaimedTimer) -> StoreResult<()> {
        <PostgresWorkflowStore as RuntimeStore>::release_timer_claim(&*self.inner, timer).await
    }

    async fn claim_pending_effects(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedEffect>> {
        <PostgresWorkflowStore as RuntimeStore>::claim_pending_effects(
            &*self.inner,
            tenant_id,
            owner,
            now_ms,
            limit,
            lease_ms,
        )
        .await
    }

    async fn record_effect_response(
        &self,
        effect: &ClaimedEffect,
        response: &EffectResponse,
    ) -> StoreResult<bool> {
        <PostgresWorkflowStore as RuntimeStore>::record_effect_response(
            &*self.inner,
            effect,
            response,
        )
        .await
    }

    async fn load_effect_responses(
        &self,
        tenant_id: &TenantId,
        limit: usize,
    ) -> StoreResult<Vec<PendingEffectResponse>> {
        <PostgresWorkflowStore as RuntimeStore>::load_effect_responses(
            &*self.inner,
            tenant_id,
            limit,
        )
        .await
    }

    async fn release_effect_claim(&self, effect: &ClaimedEffect) -> StoreResult<()> {
        <PostgresWorkflowStore as RuntimeStore>::release_effect_claim(&*self.inner, effect).await
    }

    async fn schedule_effect_retry(
        &self,
        effect: &ClaimedEffect,
        decision: RetryDecision,
        error: &str,
    ) -> StoreResult<()> {
        <PostgresWorkflowStore as RuntimeStore>::schedule_effect_retry(
            &*self.inner,
            effect,
            decision,
            error,
        )
        .await
    }

    async fn release_instance_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        lease_token: &str,
    ) -> StoreResult<()> {
        <PostgresWorkflowStore as RuntimeStore>::release_instance_transition(
            &*self.inner,
            tenant_id,
            instance_id,
            lease_token,
        )
        .await
    }

    async fn join_get(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        join_id: JoinId,
    ) -> StoreResult<u16> {
        <PostgresWorkflowStore as RuntimeStore>::join_get(
            &*self.inner,
            tenant_id,
            instance_id,
            join_id,
        )
        .await
    }

    async fn enqueue_activation(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        command_id: Uuid,
        command_kind: &str,
        command: &Command,
        available_at_ms: Option<u64>,
    ) -> StoreResult<bool> {
        <PostgresWorkflowStore as RuntimeStore>::enqueue_activation(
            &*self.inner,
            tenant_id,
            instance_id,
            command_id,
            command_kind,
            command,
            available_at_ms,
        )
        .await
    }

    async fn claim_ready_activations(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedActivation>> {
        <PostgresWorkflowStore as RuntimeStore>::claim_ready_activations(
            &*self.inner,
            tenant_id,
            owner,
            limit,
            lease_ms,
        )
        .await
    }

    async fn renew_activation_claim(
        &self,
        activation: &ClaimedActivation,
        lease_ms: u64,
    ) -> StoreResult<Option<ClaimedActivation>> {
        <PostgresWorkflowStore as RuntimeStore>::renew_activation_claim(
            &*self.inner,
            activation,
            lease_ms,
        )
        .await
    }

    async fn release_activation_to_ready(
        &self,
        activation: &ClaimedActivation,
        not_before_ms: Option<u64>,
    ) -> StoreResult<()> {
        <PostgresWorkflowStore as RuntimeStore>::release_activation_to_ready(
            &*self.inner,
            activation,
            not_before_ms,
        )
        .await
    }

    async fn consume_activation(&self, activation: &ClaimedActivation) -> StoreResult<bool> {
        <PostgresWorkflowStore as RuntimeStore>::consume_activation(&*self.inner, activation).await
    }

    async fn dead_letter_activation(
        &self,
        activation: &ClaimedActivation,
        reason: &str,
    ) -> StoreResult<()> {
        <PostgresWorkflowStore as RuntimeStore>::dead_letter_activation(
            &*self.inner,
            activation,
            reason,
        )
        .await
    }

    async fn reclaim_expired_activations(&self) -> StoreResult<u32> {
        <PostgresWorkflowStore as RuntimeStore>::reclaim_expired_activations(&*self.inner).await
    }
}

#[async_trait]
impl ArtifactRepository for ViolatingTestStore {
    async fn store_program(&self, version: [u8; 32], program: &CompiledProgram) -> StoreResult<()> {
        <PostgresWorkflowStore as ArtifactRepository>::store_program(&*self.inner, version, program)
            .await
    }

    async fn load_program(&self, version: [u8; 32]) -> StoreResult<Option<CompiledProgram>> {
        <PostgresWorkflowStore as ArtifactRepository>::load_program(&*self.inner, version).await
    }

    async fn store_artifact(
        &self,
        artifact: &ExecutableWorkflow,
    ) -> std::result::Result<(), ArtifactStoreError> {
        <PostgresWorkflowStore as ArtifactRepository>::store_artifact(&*self.inner, artifact).await
    }

    async fn load_artifact(
        &self,
        hash: ArtifactHash,
    ) -> std::result::Result<Option<ExecutableWorkflow>, ArtifactStoreError> {
        <PostgresWorkflowStore as ArtifactRepository>::load_artifact(&*self.inner, hash).await
    }

    async fn store_plan(&self, plan_hash: [u8; 32], plan_json: &str) -> StoreResult<()> {
        <PostgresWorkflowStore as ArtifactRepository>::store_plan(&*self.inner, plan_hash, plan_json)
            .await
    }

    async fn load_plan(&self, plan_hash: [u8; 32]) -> StoreResult<Option<String>> {
        <PostgresWorkflowStore as ArtifactRepository>::load_plan(&*self.inner, plan_hash).await
    }
}

#[async_trait]
impl JournalReader for ViolatingTestStore {
    async fn read_events(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        from_seq: u64,
    ) -> StoreResult<Vec<(u64, RuntimeEvent)>> {
        <PostgresWorkflowStore as JournalReader>::read_events(
            &*self.inner,
            tenant_id,
            instance_id,
            from_seq,
        )
        .await
    }

    async fn load_snapshot_envelope(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Option<SnapshotEnvelope>> {
        <PostgresWorkflowStore as JournalReader>::load_snapshot_envelope(
            &*self.inner,
            tenant_id,
            instance_id,
        )
        .await
    }

    async fn read_journal(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        after_revision: Option<u64>,
    ) -> StoreResult<Vec<JournalRecord>> {
        <PostgresWorkflowStore as JournalReader>::read_journal(
            &*self.inner,
            tenant_id,
            instance_id,
            after_revision,
        )
        .await
    }

    async fn load_payload_version(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        hash: &[u8; 32],
    ) -> StoreResult<Option<String>> {
        <PostgresWorkflowStore as JournalReader>::load_payload_version(
            &*self.inner,
            tenant_id,
            instance_id,
            hash,
        )
        .await
    }
}

#[async_trait]
impl AdminProjectionStore for ViolatingTestStore {
    async fn store_template(
        &self,
        name: &str,
        version: u32,
        plan_hash: [u8; 32],
        dsl_body: &str,
    ) -> StoreResult<()> {
        <PostgresWorkflowStore as AdminProjectionStore>::store_template(
            &*self.inner,
            name,
            version,
            plan_hash,
            dsl_body,
        )
        .await
    }

    async fn load_template_version(
        &self,
        name: &str,
        version: u32,
    ) -> StoreResult<Option<(String, [u8; 32])>> {
        <PostgresWorkflowStore as AdminProjectionStore>::load_template_version(
            &*self.inner,
            name,
            version,
        )
        .await
    }

    async fn load_latest_template_version(
        &self,
        name: &str,
    ) -> StoreResult<Option<(u32, String, [u8; 32])>> {
        <PostgresWorkflowStore as AdminProjectionStore>::load_latest_template_version(
            &*self.inner,
            name,
        )
        .await
    }

    async fn list_templates(&self) -> StoreResult<Vec<TemplateSummary>> {
        <PostgresWorkflowStore as AdminProjectionStore>::list_templates(&*self.inner).await
    }

    async fn health_check(&self) -> StoreResult<()> {
        <PostgresWorkflowStore as AdminProjectionStore>::health_check(&*self.inner).await
    }

    async fn ensure_tenant(&self, tenant_id: &TenantId) -> StoreResult<()> {
        <PostgresWorkflowStore as AdminProjectionStore>::ensure_tenant(&*self.inner, tenant_id)
            .await
    }

    async fn list_tenants(&self) -> StoreResult<Vec<String>> {
        <PostgresWorkflowStore as AdminProjectionStore>::list_tenants(&*self.inner).await
    }

    async fn list_tenants_in_pool(&self, pool_id: &str) -> StoreResult<Vec<String>> {
        <PostgresWorkflowStore as AdminProjectionStore>::list_tenants_in_pool(&*self.inner, pool_id)
            .await
    }

    async fn create_design_session(
        &self,
        _tenant_id: &TenantId,
        _id: Uuid,
        _name: &str,
        _dsl_source: &str,
    ) -> StoreResult<()> {
        Err(StoreError::Unavailable("test double".into()))
    }
    async fn load_design_session(
        &self,
        _tenant_id: &TenantId,
        _id: Uuid,
    ) -> StoreResult<Option<DesignSessionRecord>> {
        Err(StoreError::Unavailable("test double".into()))
    }
    async fn list_design_sessions(
        &self,
        _tenant_id: &TenantId,
    ) -> StoreResult<Vec<DesignSessionSummary>> {
        Err(StoreError::Unavailable("test double".into()))
    }
    async fn append_design_session_event(
        &self,
        _tenant_id: &TenantId,
        _id: Uuid,
        _kind: &DesignSessionEventKind,
    ) -> StoreResult<u64> {
        Err(StoreError::Unavailable("test double".into()))
    }
    async fn mark_design_session_saved(
        &self,
        _tenant_id: &TenantId,
        _id: Uuid,
        _template_name: &str,
        _template_version: u32,
        _plan_hash: [u8; 32],
    ) -> StoreResult<()> {
        Err(StoreError::Unavailable("test double".into()))
    }
    async fn open_dev_capture_session(
        &self,
        _session_id: &str,
        _consent_statement_timestamp: &str,
    ) -> StoreResult<()> {
        Err(StoreError::Unavailable("test double".into()))
    }
    async fn append_dev_capture_record(
        &self,
        _session_id: &str,
        _record_json: String,
    ) -> StoreResult<u64> {
        Err(StoreError::Unavailable("test double".into()))
    }
    async fn load_dev_capture_session(
        &self,
        _session_id: &str,
    ) -> StoreResult<Option<DevCaptureSessionRecord>> {
        Err(StoreError::Unavailable("test double".into()))
    }
}

/// E-invariant #1 & #2: Violation -> quarantine, not crash, not churn. Quarantine survives rollback.
/// Drives a tick through the production path, rolls it back, and verifies state changes do not persist while quarantine does.
#[tokio::test]
async fn test_pg_integrity_violation_propagates_and_rolls_back() {
    let (_pool, store, _lock) = common::setup().await;
    let store = Arc::new(store);
    let engine = BpmnLiteEngine::new(store.clone());

    // 1. Compile and start a process instance.
    let compiled = engine.compile(SMOKE_BPMN).await.unwrap();
    let version = compiled.bytecode_version;

    let payload = r#"{"case_id":"test-123"}"#;
    let hash = bpmn_lite_vm::compute_hash(payload);
    let iid = engine
        .start("smoke_proc", version, payload, hash, "test-corr-1")
        .await
        .unwrap();

    // Check pre-tick state: quarantine_state is None, fiber pc is 0.
    let loaded_before = store
        .load_instance(&TenantId::new("default").unwrap(), iid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded_before.quarantine_state, None);
    let fibers_before = store
        .load_fibers(&TenantId::new("default").unwrap(), iid)
        .await
        .unwrap();
    assert_eq!(fibers_before[0].pc, 0.into());

    // 2. Wrap the store in ViolatingTestStore, violating on commit_tick.
    let violating_store = Arc::new(ViolatingTestStore {
        inner: store.clone(),
        violate_instance_id: iid,
        should_fail_load_integrity: std::sync::atomic::AtomicBool::new(false),
        should_fail_commit_integrity: std::sync::atomic::AtomicBool::new(true),
        should_fail_generic: std::sync::atomic::AtomicBool::new(false),
    });
    let engine_with_violating_store = BpmnLiteEngine::new(violating_store.clone());

    // 3. T7 propagates integrity failures. T10 owns atomic quarantine during
    // mandatory integrity verification on load.
    let tick_res = engine_with_violating_store.tick_instance(iid).await;
    assert!(
        tick_res.is_err(),
        "Tick must propagate IntegrityViolation, got {:?}",
        tick_res
    );

    // 4. Assert both halves post-rollback:
    // A. The state change (instance's current_node_id and job queue enqueueing) did NOT persist (rolled back).
    let loaded_after = store
        .load_instance(&TenantId::new("default").unwrap(), iid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded_after.current_node_id.as_deref(),
        None,
        "Instance current_node_id must be rolled back (still None)"
    );

    // Job queue must not contain any jobs for our instance
    let jobs = store
        .dequeue_jobs(
            &["do_work".to_string()],
            100,
            &TenantId::default(),
            "test-worker",
            5000,
        )
        .await
        .unwrap();
    let has_job_for_our_instance = jobs.iter().any(|j| j.process_instance_id == iid);
    assert!(
        !has_job_for_our_instance,
        "Job for our instance must not have been enqueued (rolled back)"
    );

    // B. T7 does not perform an unfenced, separate quarantine write.
    assert_eq!(
        loaded_after.quarantine_state.as_deref(),
        None,
        "quarantine is deferred to the T10 mandatory load gate"
    );
}

/// E-invariant #3: Discrimination / no over-quarantine.
#[tokio::test]
async fn test_pg_non_integrity_failure_does_not_quarantine() {
    let (_pool, store, _lock) = common::setup().await;
    let store = Arc::new(store);
    let iid = Uuid::now_v7();

    store
        .ensure_tenant(&TenantId::new("default").unwrap())
        .await
        .unwrap();
    let inst = common::make_instance(iid);
    common::save_instance(&store, "test-owner", &inst).await.unwrap();

    let violating_store = Arc::new(ViolatingTestStore {
        inner: store.clone(),
        violate_instance_id: iid,
        should_fail_load_integrity: std::sync::atomic::AtomicBool::new(false),
        should_fail_commit_integrity: std::sync::atomic::AtomicBool::new(false),
        should_fail_generic: std::sync::atomic::AtomicBool::new(true),
    });

    let engine = BpmnLiteEngine::new(violating_store.clone());

    let tick_res = engine.tick_instance(iid).await;
    assert!(
        tick_res.is_err(),
        "Tick must propagate non-integrity errors, got {:?}",
        tick_res
    );

    let loaded = store
        .load_instance(&TenantId::new("default").unwrap(), iid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.quarantine_state, None,
        "Ordinary failure must not quarantine instance"
    );

    let claimed = store
        .claim_running_instances(
            &TenantId::new("default").unwrap(),
            "test-scheduler",
            10,
            5000,
        )
        .await
        .unwrap();
    assert!(
        claimed.contains(&iid),
        "Ordinary failed instance must remain claimable/retryable"
    );
}

/// The in-crate `PostgresWorkflowStore` test-only inherent methods
/// `save_fiber`/`append_event` reach into private fields (`self.pool`,
/// `set_tenant_context`'s transaction) that are not exposed cross-crate.
/// This reimplements their exact SQL against the store's already-public
/// `pool()` accessor and `PostgresWorkflowStore::set_tenant_context`
/// associated function (both already `pub`, not newly exposed), omitting
/// only the `pg_notify` LISTEN/NOTIFY side-effect — irrelevant to this
/// test's assertions, which never observe the notification channel.
async fn save_fiber(pool: &sqlx::PgPool, instance_id: Uuid, fiber: &Fiber) {
    let tenant_id = "default".to_string();
    let mut tx = pool.begin().await.unwrap();
    PostgresWorkflowStore::set_tenant_context(&mut tx, &tenant_id)
        .await
        .unwrap();

    let stack = serde_json::to_value(&fiber.stack).unwrap();
    let wait_state = serde_json::to_value(&fiber.wait).unwrap();

    sqlx::query(
        r#"
    INSERT INTO fibers (instance_id, fiber_id, pc, stack, wait_state, loop_epoch, tenant_id)
    VALUES ($1, $2, $3, $4, $5, $6, $7)
    ON CONFLICT (instance_id, fiber_id) DO UPDATE SET
        pc = EXCLUDED.pc,
        stack = EXCLUDED.stack,
        wait_state = EXCLUDED.wait_state,
        loop_epoch = EXCLUDED.loop_epoch
    "#,
    )
    .bind(instance_id)
    .bind(fiber.fiber_id)
    .bind(fiber.pc.get() as i32)
    .bind(&stack)
    .bind(&wait_state)
    .bind(fiber.loop_epoch as i32)
    .bind(&tenant_id)
    .execute(&mut *tx)
    .await
    .unwrap();

    tx.commit().await.unwrap();
}

async fn append_event(pool: &sqlx::PgPool, instance_id: Uuid, event: &RuntimeEvent) {
    let tenant_id = "default".to_string();
    let mut tx = pool.begin().await.unwrap();
    PostgresWorkflowStore::set_tenant_context(&mut tx, &tenant_id)
        .await
        .unwrap();
    let event_json = serde_json::to_value(event).unwrap();

    sqlx::query(
        r#"
    WITH seq AS (
        INSERT INTO event_sequences (instance_id, next_seq, tenant_id)
        VALUES ($1, 1, $3)
        ON CONFLICT (instance_id) DO UPDATE
            SET next_seq = event_sequences.next_seq + 1
        RETURNING next_seq
    )
    INSERT INTO event_log (instance_id, seq, event, tenant_id)
    SELECT $1, seq.next_seq, $2, $3
    FROM seq
    RETURNING seq
    "#,
    )
    .bind(instance_id)
    .bind(&event_json)
    .bind(&tenant_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();

    tx.commit().await.unwrap();
}

/// T7 propagates integrity failures from startup recovery. T10 owns the mandatory
/// load-integrity gate, atomic quarantine, and readiness behavior.
#[tokio::test]
async fn test_pg_integrity_violation_in_startup_recovery() {
    let (pool, store, _lock) = common::setup().await;
    let store = Arc::new(store);

    let iid_corrupt = Uuid::now_v7();
    let iid_healthy = Uuid::now_v7();

    store
        .ensure_tenant(&TenantId::new("default").unwrap())
        .await
        .unwrap();

    // 1. Publish FFI template as NonIdempotent
    use ffi_catalogue::FfiTemplateStore;
    let ffi_store = Arc::new(bpmn_lite_store_postgres::PostgresFfiTemplateStore::new(
        pool.clone(),
    ));
    let mut template = ffi_types::FfiTemplate {
        template_id: [0u8; 32],
        owner_type: "dmn-decision".to_string(),
        owner_metadata: "CheckEligibility".as_bytes().to_vec(),
        input_schema: vec![],
        output_schema: vec![],
        idempotency: ffi_types::Idempotency::NonIdempotent,
        tenant_id: "default".to_string(),
        published_at: 1700000000000,
        publisher: "test".to_string(),
    };
    // publish() now verifies template_id against the content hash
    // (content-addressing guard) — this fixture used to hand-pick an
    // arbitrary placeholder id, which is exactly the kind of mismatch
    // that check exists to catch.
    template.template_id = ffi_types::compute_template_id(&template);
    let template_id = template.template_id;
    let template_id_hex = hex(&template_id);
    ffi_store.publish(&template).await.unwrap();

    // Build FfiDispatcher and cache
    let catalogue = Arc::new(ffi_catalogue::FfiCatalogue::new(ffi_store));
    catalogue
        .load_into_cache(&TenantId::new("default").unwrap())
        .await
        .unwrap();
    let dispatcher = Arc::new(ffi_dispatcher::FfiDispatcher::new(catalogue));

    // 2. Save both instances
    let inst_corrupt = common::make_instance(iid_corrupt);
    let inst_healthy = common::make_instance(iid_healthy);
    common::save_instance(&store, "test-owner", &inst_corrupt).await.unwrap();
    common::save_instance(&store, "test-owner", &inst_healthy).await.unwrap();

    // Save a fiber for both, so the incident creator can locate the fiber at pc = 0
    let fiber_corrupt = Fiber::new(Uuid::now_v7(), 0);
    let fiber_healthy = Fiber::new(Uuid::now_v7(), 0);
    save_fiber(&pool, iid_corrupt, &fiber_corrupt).await;
    save_fiber(&pool, iid_healthy, &fiber_healthy).await;

    // Append pending Ffi invocation event to both
    let ev_corrupt = RuntimeEvent::FfiInvocationPending {
        invocation_id: Uuid::now_v7(),
        template_id_hex: template_id_hex.clone(),
        caller_task_id: "task1".to_string(),
        caller_pc: 0.into(),
        owner_type: "engine".to_string(),
    };
    let ev_healthy = RuntimeEvent::FfiInvocationPending {
        invocation_id: Uuid::now_v7(),
        template_id_hex: template_id_hex.clone(),
        caller_task_id: "task1".to_string(),
        caller_pc: 0.into(),
        owner_type: "engine".to_string(),
    };
    append_event(&pool, iid_corrupt, &ev_corrupt).await;
    append_event(&pool, iid_healthy, &ev_healthy).await;

    // 3. Wrap in ViolatingTestStore. We return IntegrityViolation on load_instance for iid_corrupt.
    let violating_store = Arc::new(ViolatingTestStore {
        inner: store.clone(),
        violate_instance_id: iid_corrupt,
        should_fail_load_integrity: std::sync::atomic::AtomicBool::new(true),
        should_fail_commit_integrity: std::sync::atomic::AtomicBool::new(false),
        should_fail_generic: std::sync::atomic::AtomicBool::new(false),
    });

    let engine = BpmnLiteEngine::new(violating_store.clone()).with_ffi_dispatcher(dispatcher);

    // 4. Recovery must fail closed rather than silently skipping corruption.
    let recovery_result = engine
        .detect_interrupted_ffi_calls(&TenantId::default())
        .await;
    assert!(
        recovery_result.is_err(),
        "startup recovery must propagate integrity failure"
    );

    // 5. Assert:
    // A. T7 does not perform the unfenced quarantine write that T10 replaces.
    let corrupt_loaded = store
        .load_instance(&TenantId::new("default").unwrap(), iid_corrupt)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(corrupt_loaded.quarantine_state, None);

    // B. Recovery stops at the integrity error; it does not partially advance another
    // instance after a failed readiness scan.
    let healthy_loaded = store
        .load_instance(&TenantId::new("default").unwrap(), iid_healthy)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(healthy_loaded.state, ProcessState::Running),
        "healthy instance must remain Running, got {:?}",
        healthy_loaded.state
    );
    assert_eq!(
        healthy_loaded.quarantine_state, None,
        "Healthy instance must not be quarantined"
    );
}
