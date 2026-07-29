//! F8.2/F8.3 — store fault injection + engine crash-recovery (EOP-FUZZ
//! §10). The system under test is the engine's failure discipline, not
//! the store: every store call can fail BEFORE the operation (never
//! reached the store) or AFTER it (operation durable, response lost —
//! the classic at-least-once hazard), and the engine may be dropped and
//! rebuilt over the same store at any point (recovery is snapshot-based
//! and per-call; nothing lives only in engine memory).
//!
//! Oracles:
//!   R-O1 no-panic       — injected Unavailable at any call site is an
//!                         error path, never a panic.
//!   R-O2 conservation   — G-T holds ACROSS faults and restarts: job
//!                         keys are `{instance}:{task_id}:{pc}:{loop_epoch}`,
//!                         so an after-commit fault's redelivery reuses
//!                         the key; a SECOND distinct key at a bound-1
//!                         task after recovery is a duplicated token.
//!   R-O3 recoverability — once faults stop, the instance is finishable
//!                         or cancellable by SOME engine we constructed
//!                         (a leaked transition lease belongs to one of
//!                         our owners; trying all of them is exhaustive).
//!                         An instance no engine can cancel is a stuck
//!                         instance — the finding.
//!
//! Recorded limits: MemoryStore transition leases expire on process-local
//! `Instant` (5s), not the FuzzClock — the driver never waits out a
//! lease; it exercises the renewal path (same owner) and the
//! foreign-owner rejection path instead. True cross-process durability
//! (kill -9, Postgres) is out of scope for in-process fuzzing and
//! remains the Postgres test suite's territory.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_store::store::{
    AdminProjectionStore, ArtifactRepository, JournalReader, RuntimeStore, TemplateSummary,
};
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_store::{
    ArtifactStoreError, ClaimError, CommitError, CommitOutcome, StoreError, StoreResult,
    WorkflowStore,
};
use bpmn_lite_types::events::RuntimeEvent;
use bpmn_lite_types::*;
use uuid::Uuid;

use crate::{
    emit_process, gen_shape, ConservationTracker, FuzzClock, Shape, Tape,
};

// ─── Fault plan ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    /// Fail before the store sees the call.
    Before,
    /// Perform the operation, then report failure — durable effect,
    /// lost response.
    After,
}

/// Deterministic tape-seeded fault schedule, shared by every FaultStore
/// wrapping the same run. `rate` in [0,16]: injection probability
/// rate/16 per store call; 0 disables injection (the recovery-phase
/// oracles require a quiet store).
struct FaultPlan {
    state: AtomicU64,
    rate: AtomicU64,
}

impl FaultPlan {
    pub fn new(seed: u64) -> Self {
        Self {
            state: AtomicU64::new(seed | 1),
            rate: AtomicU64::new(0),
        }
    }

    fn set_rate(&self, rate: u64) {
        self.rate.store(rate.min(16), Ordering::SeqCst);
    }

    fn decide(&self) -> Fault {
        let rate = self.rate.load(Ordering::SeqCst);
        if rate == 0 {
            return Fault::None;
        }
        let draw = self
            .state
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |s| {
                Some(
                    s.wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407),
                )
            })
            .expect("fetch_update with Some never fails")
            >> 33;
        if draw % 16 < rate {
            if draw & (1 << 20) != 0 {
                Fault::After
            } else {
                Fault::Before
            }
        } else {
            Fault::None
        }
    }
}

// ─── FaultStore ──────────────────────────────────────────────────────

/// WorkflowStore wrapper injecting Unavailable faults around a real
/// MemoryStore per the shared FaultPlan.
pub(crate) struct FaultStore {
    inner: Arc<MemoryStore>,
    plan: Arc<FaultPlan>,
}

impl FaultStore {
    pub fn new(inner: Arc<MemoryStore>, plan: Arc<FaultPlan>) -> Self {
        Self { inner, plan }
    }
}

/// Wrap one delegated async call in the fault decision. `$err` builds
/// the method's Unavailable error.
macro_rules! faulty {
    ($self:ident, $err:expr, $fut:expr) => {
        match $self.plan.decide() {
            Fault::Before => Err($err),
            Fault::After => {
                let _ = $fut.await;
                Err($err)
            }
            Fault::None => $fut.await,
        }
    };
}

fn unavailable() -> StoreError {
    StoreError::Unavailable("injected fault".to_string())
}

#[async_trait]
impl RuntimeStore for FaultStore {
    async fn load_instance(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
    ) -> StoreResult<Option<ProcessInstance>> {
        faulty!(self, unavailable(), self.inner.load_instance(tenant_id, id))
    }

    async fn load_fiber(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        fiber_id: Uuid,
    ) -> StoreResult<Option<Fiber>> {
        faulty!(self, unavailable(), self.inner.load_fiber(tenant_id, instance_id, fiber_id))
    }

    async fn load_fibers(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Vec<Fiber>> {
        faulty!(self, unavailable(), self.inner.load_fibers(tenant_id, instance_id))
    }

    async fn dedupe_get(
        &self,
        tenant_id: &TenantId,
        key: &str,
    ) -> StoreResult<Option<JobCompletion>> {
        faulty!(self, unavailable(), self.inner.dedupe_get(tenant_id, key))
    }

    async fn dequeue_jobs(
        &self,
        task_types: &[String],
        max: usize,
        tenant_id: &TenantId,
        worker_id: &str,
        lease_ms: u64,
    ) -> StoreResult<Vec<JobActivation>> {
        faulty!(
            self,
            unavailable(),
            self.inner.dequeue_jobs(task_types, max, tenant_id, worker_id, lease_ms)
        )
    }

    async fn validate_job_claim(
        &self,
        tenant_id: &TenantId,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> StoreResult<bool> {
        faulty!(
            self,
            unavailable(),
            self.inner.validate_job_claim(tenant_id, job_key, worker_id, claim_token)
        )
    }

    async fn dead_letter_put(
        &self,
        name: u32,
        corr_key: &Value,
        payload: &[u8],
        ttl_ms: u64,
    ) -> StoreResult<()> {
        faulty!(
            self,
            unavailable(),
            self.inner.dead_letter_put(name, corr_key, payload, ttl_ms)
        )
    }

    async fn dead_letter_take(&self, name: u32, corr_key: &Value) -> StoreResult<Option<Vec<u8>>> {
        faulty!(self, unavailable(), self.inner.dead_letter_take(name, corr_key))
    }

    async fn claim_buffered_message(
        &self,
        tenant_id: &TenantId,
        message_name: &str,
        correlation_key: &str,
        claim_ms: u64,
    ) -> StoreResult<Option<ClaimedBufferedMessage>> {
        faulty!(
            self,
            unavailable(),
            self.inner
                .claim_buffered_message(tenant_id, message_name, correlation_key, claim_ms)
        )
    }

    async fn reclaim_stale_buffered_message_claims(&self) -> StoreResult<u32> {
        faulty!(self, unavailable(), self.inner.reclaim_stale_buffered_message_claims())
    }

    async fn prune_expired_messages(&self) -> StoreResult<u32> {
        faulty!(self, unavailable(), self.inner.prune_expired_messages())
    }

    async fn load_incidents(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Vec<Incident>> {
        faulty!(self, unavailable(), self.inner.load_incidents(tenant_id, instance_id))
    }

    async fn reclaim_stale_jobs(&self, timeout_ms: u64) -> StoreResult<u32> {
        faulty!(self, unavailable(), self.inner.reclaim_stale_jobs(timeout_ms))
    }

    async fn prune_dedupe_cache(&self, older_than_ms: u64) -> StoreResult<u32> {
        faulty!(self, unavailable(), self.inner.prune_dedupe_cache(older_than_ms))
    }

    async fn list_running_instances(&self, tenant_id: &TenantId) -> StoreResult<Vec<Uuid>> {
        faulty!(self, unavailable(), self.inner.list_running_instances(tenant_id))
    }

    async fn claim_running_instances(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<Uuid>> {
        faulty!(
            self,
            unavailable(),
            self.inner.claim_running_instances(tenant_id, owner, limit, lease_ms)
        )
    }

    async fn claim_instance_for_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
    ) -> std::result::Result<Option<Claim>, ClaimError> {
        faulty!(
            self,
            ClaimError::Unavailable("injected fault".to_string()),
            self.inner
                .claim_instance_for_transition(tenant_id, instance_id, owner, lease_ms)
        )
    }

    async fn commit_transition(
        &self,
        claim: &Claim,
        transition: &Transition,
    ) -> std::result::Result<CommitOutcome, CommitError> {
        faulty!(
            self,
            CommitError::Unavailable("injected fault".to_string()),
            self.inner.commit_transition(claim, transition)
        )
    }

    async fn lookup_start_instance(
        &self,
        tenant_id: &TenantId,
        idempotency_key: Uuid,
    ) -> StoreResult<Option<Uuid>> {
        faulty!(
            self,
            unavailable(),
            self.inner.lookup_start_instance(tenant_id, idempotency_key)
        )
    }

    async fn claim_due_timers(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedTimer>> {
        faulty!(
            self,
            unavailable(),
            self.inner.claim_due_timers(tenant_id, owner, now_ms, limit, lease_ms)
        )
    }

    async fn release_timer_claim(&self, timer: &ClaimedTimer) -> StoreResult<()> {
        faulty!(self, unavailable(), self.inner.release_timer_claim(timer))
    }

    async fn claim_pending_effects(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedEffect>> {
        faulty!(
            self,
            unavailable(),
            self.inner
                .claim_pending_effects(tenant_id, owner, now_ms, limit, lease_ms)
        )
    }

    async fn record_effect_response(
        &self,
        effect: &ClaimedEffect,
        response: &EffectResponse,
    ) -> StoreResult<bool> {
        faulty!(self, unavailable(), self.inner.record_effect_response(effect, response))
    }

    async fn load_effect_responses(
        &self,
        tenant_id: &TenantId,
        limit: usize,
    ) -> StoreResult<Vec<PendingEffectResponse>> {
        faulty!(self, unavailable(), self.inner.load_effect_responses(tenant_id, limit))
    }

    async fn release_effect_claim(&self, effect: &ClaimedEffect) -> StoreResult<()> {
        faulty!(self, unavailable(), self.inner.release_effect_claim(effect))
    }

    async fn schedule_effect_retry(
        &self,
        effect: &ClaimedEffect,
        decision: RetryDecision,
        error: &str,
    ) -> StoreResult<()> {
        faulty!(
            self,
            unavailable(),
            self.inner.schedule_effect_retry(effect, decision, error)
        )
    }

    async fn release_instance_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
    ) -> StoreResult<()> {
        faulty!(
            self,
            unavailable(),
            self.inner
                .release_instance_transition(tenant_id, instance_id, owner)
        )
    }

    async fn join_get(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        join_id: JoinId,
    ) -> StoreResult<u16> {
        faulty!(self, unavailable(), self.inner.join_get(tenant_id, instance_id, join_id))
    }
}

#[async_trait]
impl ArtifactRepository for FaultStore {
    async fn store_program(&self, version: [u8; 32], program: &CompiledProgram) -> StoreResult<()> {
        faulty!(self, unavailable(), self.inner.store_program(version, program))
    }

    async fn load_program(&self, version: [u8; 32]) -> StoreResult<Option<CompiledProgram>> {
        faulty!(self, unavailable(), self.inner.load_program(version))
    }

    async fn store_artifact(
        &self,
        artifact: &ExecutableWorkflow,
    ) -> std::result::Result<(), ArtifactStoreError> {
        faulty!(
            self,
            ArtifactStoreError::Unavailable("injected fault".to_string()),
            self.inner.store_artifact(artifact)
        )
    }

    async fn load_artifact(
        &self,
        hash: ArtifactHash,
    ) -> std::result::Result<Option<ExecutableWorkflow>, ArtifactStoreError> {
        faulty!(
            self,
            ArtifactStoreError::Unavailable("injected fault".to_string()),
            self.inner.load_artifact(hash)
        )
    }

    async fn store_plan(&self, plan_hash: [u8; 32], plan_json: &str) -> StoreResult<()> {
        faulty!(self, unavailable(), self.inner.store_plan(plan_hash, plan_json))
    }

    async fn load_plan(&self, plan_hash: [u8; 32]) -> StoreResult<Option<String>> {
        faulty!(self, unavailable(), self.inner.load_plan(plan_hash))
    }
}

#[async_trait]
impl JournalReader for FaultStore {
    async fn read_events(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        from_seq: u64,
    ) -> StoreResult<Vec<(u64, RuntimeEvent)>> {
        faulty!(
            self,
            unavailable(),
            self.inner.read_events(tenant_id, instance_id, from_seq)
        )
    }

    async fn load_snapshot_envelope(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Option<SnapshotEnvelope>> {
        faulty!(
            self,
            unavailable(),
            self.inner.load_snapshot_envelope(tenant_id, instance_id)
        )
    }

    async fn read_journal(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        after_revision: Option<u64>,
    ) -> StoreResult<Vec<JournalRecord>> {
        faulty!(
            self,
            unavailable(),
            self.inner.read_journal(tenant_id, instance_id, after_revision)
        )
    }

    async fn load_payload_version(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        hash: &[u8; 32],
    ) -> StoreResult<Option<String>> {
        faulty!(
            self,
            unavailable(),
            self.inner.load_payload_version(tenant_id, instance_id, hash)
        )
    }
}

#[async_trait]
impl AdminProjectionStore for FaultStore {
    async fn store_template(
        &self,
        name: &str,
        version: u32,
        plan_hash: [u8; 32],
        dsl_body: &str,
    ) -> StoreResult<()> {
        faulty!(
            self,
            unavailable(),
            self.inner.store_template(name, version, plan_hash, dsl_body)
        )
    }

    async fn load_template_version(
        &self,
        name: &str,
        version: u32,
    ) -> StoreResult<Option<(String, [u8; 32])>> {
        faulty!(self, unavailable(), self.inner.load_template_version(name, version))
    }

    async fn load_latest_template_version(
        &self,
        name: &str,
    ) -> StoreResult<Option<(u32, String, [u8; 32])>> {
        faulty!(self, unavailable(), self.inner.load_latest_template_version(name))
    }

    async fn list_templates(&self) -> StoreResult<Vec<TemplateSummary>> {
        faulty!(self, unavailable(), self.inner.list_templates())
    }

    async fn health_check(&self) -> StoreResult<()> {
        faulty!(self, unavailable(), self.inner.health_check())
    }

    async fn ensure_tenant(&self, tenant_id: &TenantId) -> StoreResult<()> {
        faulty!(self, unavailable(), self.inner.ensure_tenant(tenant_id))
    }

    async fn list_tenants(&self) -> StoreResult<Vec<String>> {
        faulty!(self, unavailable(), self.inner.list_tenants())
    }

    async fn list_tenants_in_pool(&self, pool_id: &str) -> StoreResult<Vec<String>> {
        faulty!(self, unavailable(), self.inner.list_tenants_in_pool(pool_id))
    }

    async fn create_design_session(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
        name: &str,
        dsl_source: &str,
    ) -> StoreResult<()> {
        faulty!(
            self,
            unavailable(),
            self.inner.create_design_session(tenant_id, id, name, dsl_source)
        )
    }

    async fn load_design_session(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
    ) -> StoreResult<Option<bpmn_lite_store::store::DesignSessionRecord>> {
        faulty!(self, unavailable(), self.inner.load_design_session(tenant_id, id))
    }

    async fn list_design_sessions(
        &self,
        tenant_id: &TenantId,
    ) -> StoreResult<Vec<bpmn_lite_store::store::DesignSessionSummary>> {
        faulty!(self, unavailable(), self.inner.list_design_sessions(tenant_id))
    }

    async fn append_design_session_event(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
        kind: &bpmn_lite_store::store::DesignSessionEventKind,
    ) -> StoreResult<u64> {
        faulty!(
            self,
            unavailable(),
            self.inner.append_design_session_event(tenant_id, id, kind)
        )
    }

    async fn mark_design_session_saved(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
        template_name: &str,
        template_version: u32,
        plan_hash: [u8; 32],
    ) -> StoreResult<()> {
        faulty!(
            self,
            unavailable(),
            self.inner
                .mark_design_session_saved(tenant_id, id, template_name, template_version, plan_hash)
        )
    }
}

// ─── Recovery drive loop ─────────────────────────────────────────────

/// F8.2/F8.3 driver: generated graph over a fault-injected shared store,
/// with tape-chosen engine restarts. Faults are OFF for compile/start
/// (G-A stays a compiler oracle, not a weather report), tape-set for the
/// storm phase, and OFF again for the final recoverability assertions.
pub async fn drive_recovery(data: &[u8]) {
    let mut tape = Tape::new(data);
    let shape = gen_shape(&mut tape);
    let generated = emit_process(&shape);

    let inner = Arc::new(MemoryStore::new());
    let plan = Arc::new(FaultPlan::new(
        u64::from_le_bytes([
            tape.u8(), tape.u8(), tape.u8(), tape.u8(),
            tape.u8(), tape.u8(), tape.u8(), tape.u8(),
        ]),
    ));
    let store: Arc<dyn WorkflowStore> =
        Arc::new(FaultStore::new(inner.clone(), plan.clone()));
    let clock = Arc::new(FuzzClock::new());
    let mut engines = vec![BpmnLiteEngine::new_with_runtime_context(
        store.clone(),
        TenantId::default(),
        clock.clone(),
    )];

    // Quiet store for compile + start: a rejection here would be a real
    // compiler/engine finding, not fault noise.
    let compiled = engines[0]
        .compile(&generated.xml)
        .await
        .unwrap_or_else(|error| {
            panic!("G-A red (recovery tier): {error}\nshape: {shape:?}")
        });
    let orch_flags: BTreeMap<String, Value> = compiled
        .flag_symbol_table
        .iter()
        .filter_map(|(key, name)| {
            generated
                .flag_intents
                .get(name)
                .map(|intent| (format!("flag_{key}"), Value::Bool(*intent)))
        })
        .collect();
    let mut current_hash = EffectId::content_hash(generated.payload.as_bytes());
    let instance_id = engines[0]
        .start(
            "fuzz_graph",
            compiled.bytecode_version,
            &generated.payload,
            current_hash,
            "corr-recovery",
        )
        .await
        .expect("start on a quiet store must succeed");

    // Storm phase: tape-set fault rate, restarts interleaved with drive.
    plan.set_rate(u64::from(tape.u8() % 9)); // 0..=8 of 16
    let mut tracker = ConservationTracker::default();
    let mut job_keys: Vec<String> = Vec::new();
    let steps = 12 + usize::from(tape.u8() % 21);
    for _ in 0..steps {
        let engine = engines.last().expect("at least one engine");
        match tape.u8() % 12 {
            0..=4 => {
                if let Ok(activations) = engine.run_instance(instance_id).await {
                    for job in &activations {
                        if let Err(violation) =
                            tracker.record(&job.task_type, &job.job_key, &generated.bounds)
                        {
                            panic!(
                                "R-O2: conservation violated under faults/restarts: \
                                 {violation}\nshape: {shape:?}"
                            );
                        }
                    }
                    job_keys.extend(activations.into_iter().map(|job| job.job_key));
                }
            }
            5 | 6 => {
                if job_keys.is_empty() {
                    continue;
                }
                let key = job_keys[usize::from(tape.u8()) % job_keys.len()].clone();
                let result_payload = format!(r#"{{"result":{}}}"#, tape.u8());
                if engine
                    .complete_job(&key, &result_payload, current_hash, orch_flags.clone())
                    .await
                    .is_ok()
                {
                    current_hash = EffectId::content_hash(result_payload.as_bytes());
                }
            }
            7 => {
                if job_keys.is_empty() {
                    continue;
                }
                let key = job_keys[usize::from(tape.u8()) % job_keys.len()].clone();
                let error_class = match tape.u8() % 3 {
                    0 => ErrorClass::Transient,
                    1 => ErrorClass::ContractViolation,
                    _ => ErrorClass::BusinessRejection {
                        rejection_code: format!("R{}", tape.u8()),
                    },
                };
                let _ = engine.fail_job(&key, error_class, "fuzz failure").await;
            }
            8 => {
                clock.advance(i64::from(tape.u8()) * 100);
                let _ = engine.tick_instance(instance_id).await;
            }
            9 => {
                let _ = engine.inspect(instance_id).await;
            }
            10 => {
                // F8.3 restart: a NEW engine (fresh transition owner) over
                // the SAME store. Prior engines stay constructed so the
                // final oracle can renew any lease they still hold.
                let next = engines
                    .last()
                    .expect("at least one engine")
                    .for_tenant(TenantId::default());
                engines.push(next);
            }
            _ => {
                plan.set_rate(u64::from(tape.u8() % 9)); // re-weather mid-run
            }
        }
    }

    // Recovery phase: quiet store; the instance must be drivable to a
    // conclusion by the CURRENT engine (drain), and if still non-terminal,
    // cancellable by SOME engine we own (R-O3).
    plan.set_rate(0);
    let engine = engines.last().expect("at least one engine").for_tenant(TenantId::default());
    for _ in 0..24 {
        let Ok(activations) = engine.run_instance(instance_id).await else {
            break;
        };
        if activations.is_empty() {
            break;
        }
        for job in &activations {
            if let Err(violation) =
                tracker.record(&job.task_type, &job.job_key, &generated.bounds)
            {
                panic!("R-O2: conservation violated post-recovery: {violation}\nshape: {shape:?}");
            }
        }
        for job in activations {
            let result_payload = r#"{"recovered":true}"#;
            if engine
                .complete_job(&job.job_key, result_payload, current_hash, orch_flags.clone())
                .await
                .is_ok()
            {
                current_hash = EffectId::content_hash(result_payload.as_bytes());
            }
        }
    }
    engines.push(engine);

    if let Ok(inspection) = engines.last().expect("engine").inspect(instance_id).await {
        if !inspection.state.is_terminal() {
            let mut cancelled = false;
            for engine in &engines {
                if engine
                    .cancel(instance_id, "recovery final cancel")
                    .await
                    .is_ok()
                {
                    cancelled = true;
                    break;
                }
            }
            assert!(
                cancelled,
                "R-O3: instance stuck — non-terminal and no engine (any lease owner) \
                 can cancel it\nshape: {shape:?}"
            );
        }
    }
}

// ─── Cement tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Block;
    use bpmn_lite_store::WorkflowStore;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build test runtime")
    }

    fn lcg_bytes(seed: &mut u64, len: usize) -> Vec<u8> {
        (0..len)
            .map(|_| {
                *seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (*seed >> 33) as u8
            })
            .collect()
    }

    /// F8.3 green receipt: an engine restart mid-run loses nothing — a
    /// NEW engine over the same store finishes the instance, and
    /// conservation holds across the seam.
    #[test]
    fn restart_mid_run_resumes_and_conserves() {
        let shape = Shape {
            blocks: vec![
                Block::Task,
                Block::And {
                    branches: vec![vec![Block::Task], vec![Block::Task]],
                },
                Block::Task,
            ],
        };
        runtime().block_on(async {
            let generated = emit_process(&shape);
            let inner = Arc::new(MemoryStore::new());
            let plan = Arc::new(FaultPlan::new(7));
            let store: Arc<dyn WorkflowStore> =
                Arc::new(FaultStore::new(inner, plan.clone()));
            let clock = Arc::new(FuzzClock::new());
            let engine_a = BpmnLiteEngine::new_with_runtime_context(
                store.clone(),
                TenantId::default(),
                clock.clone(),
            );
            let compiled = engine_a.compile(&generated.xml).await.expect("compile");
            let mut hash = EffectId::content_hash(generated.payload.as_bytes());
            let instance_id = engine_a
                .start("fuzz_graph", compiled.bytecode_version, &generated.payload, hash, "c")
                .await
                .expect("start");

            let mut tracker = ConservationTracker::default();
            // Drive exactly one activation round on engine A, then drop it.
            let jobs = engine_a.run_instance(instance_id).await.expect("run A");
            assert!(!jobs.is_empty(), "first round must activate init");
            for job in &jobs {
                tracker
                    .record(&job.task_type, &job.job_key, &generated.bounds)
                    .expect("R-O2 on engine A");
                engine_a
                    .complete_job(&job.job_key, r#"{"a":1}"#, hash, BTreeMap::new())
                    .await
                    .expect("complete on A");
                hash = EffectId::content_hash(br#"{"a":1}"#);
            }
            let engine_b = engine_a.for_tenant(TenantId::default());
            drop(engine_a);

            // Engine B (fresh transition owner) must finish it.
            for _ in 0..24 {
                let jobs = engine_b.run_instance(instance_id).await.expect("run B");
                if jobs.is_empty() {
                    break;
                }
                for job in jobs {
                    tracker
                        .record(&job.task_type, &job.job_key, &generated.bounds)
                        .expect("R-O2 across restart");
                    engine_b
                        .complete_job(&job.job_key, r#"{"b":1}"#, hash, BTreeMap::new())
                        .await
                        .expect("complete on B");
                    hash = EffectId::content_hash(br#"{"b":1}"#);
                }
            }
            let inspection = engine_b.inspect(instance_id).await.expect("inspect");
            assert!(
                inspection.state.is_terminal(),
                "restarted engine must drive the instance to completion, got {:?}",
                inspection.state
            );
        });
    }

    /// F8.2 red receipt: injection actually fires — at rate 16 every
    /// engine call over the store errors (never panics), and after the
    /// storm the SAME instance recovers to completion on a quiet store.
    #[test]
    fn full_fault_storm_errors_cleanly_then_recovers() {
        let shape = Shape {
            blocks: vec![Block::Task],
        };
        runtime().block_on(async {
            let generated = emit_process(&shape);
            let inner = Arc::new(MemoryStore::new());
            let plan = Arc::new(FaultPlan::new(99));
            let store: Arc<dyn WorkflowStore> =
                Arc::new(FaultStore::new(inner, plan.clone()));
            let engine = BpmnLiteEngine::new_with_runtime_context(
                store,
                TenantId::default(),
                Arc::new(FuzzClock::new()),
            );
            let compiled = engine.compile(&generated.xml).await.expect("compile quiet");
            let hash = EffectId::content_hash(generated.payload.as_bytes());
            let instance_id = engine
                .start("fuzz_graph", compiled.bytecode_version, &generated.payload, hash, "c")
                .await
                .expect("start quiet");

            plan.set_rate(16);
            assert!(
                engine.run_instance(instance_id).await.is_err(),
                "total fault storm must surface as an error, not silence"
            );
            let _ = engine.complete_job("bogus", "{}", hash, BTreeMap::new()).await;
            let _ = engine.inspect(instance_id).await;
            let _ = engine.cancel(instance_id, "storm cancel").await;

            plan.set_rate(0);
            let mut hash = hash;
            for _ in 0..12 {
                let jobs = engine.run_instance(instance_id).await.expect("run quiet");
                if jobs.is_empty() {
                    break;
                }
                for job in jobs {
                    engine
                        .complete_job(&job.job_key, r#"{"r":1}"#, hash, BTreeMap::new())
                        .await
                        .expect("complete quiet");
                    hash = EffectId::content_hash(br#"{"r":1}"#);
                }
            }
            let inspection = engine.inspect(instance_id).await.expect("inspect");
            assert!(
                inspection.state.is_terminal(),
                "instance must recover to completion after the storm, got {:?}",
                inspection.state
            );
        });
    }

    /// Deterministic population through the full recovery driver: mixed
    /// faults, restarts, after-commit losses — all oracles quiet.
    #[test]
    fn recovery_driver_steps_clean_over_tape_population() {
        let mut seed: u64 = 0xD1B5_4A32_D192_ED03;
        runtime().block_on(async {
            for _ in 0..40 {
                let bytes = lcg_bytes(&mut seed, 128);
                drive_recovery(&bytes).await;
            }
        });
    }
}
