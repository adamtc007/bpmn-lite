//! Multi-crate application vertical: `bpmn_lite_engine::BpmnLiteEngine`
//! driving a real `bpmn-lite-store-postgres` store end-to-end (compile via
//! `bpmn_lite_compiler`, start/tick/complete via the engine, replay via
//! `bpmn_lite_kernel::replay`), plus two artifact-store round-trips that
//! compile through `bpmn_lite_compiler` before hitting Postgres.
//! Moved from `bpmn-lite-store-postgres/src/store_postgres.rs`'s `mod
//! tests` under EOP-PLAN-CRATE-HYGIENE-001 H1 (work item 3): these tests
//! reach beyond the store's own persistence contract into engine/compiler/
//! kernel composition, so they no longer belong in the store crate's unit
//! tests.

mod common;

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_store::{ArtifactRepository, ArtifactStoreError, JournalReader, RuntimeStore};
use bpmn_lite_types::{ProcessState, RuntimeEvent, TenantId};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

/// Minimal single-task BPMN for T-PG-14.
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

/// T-PG-14: Full engine smoke test
#[tokio::test]
async fn test_pg_full_engine_smoke() {
    let (_pool, store, _lock) = common::setup().await;
    let store = Arc::new(store);
    let engine = BpmnLiteEngine::new(store.clone());

    // Compile
    let compiled = engine.compile(SMOKE_BPMN).await.unwrap();
    let version = compiled.bytecode_version;

    // Start process
    let payload = r#"{"case_id":"test-123"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let instance_id = engine
        .start("smoke_proc", version, payload, hash, "test-corr-1")
        .await
        .unwrap();
    let genesis = store
        .load_snapshot_envelope(&TenantId::new("default").unwrap(), instance_id)
        .await
        .unwrap()
        .unwrap();

    // Tick to advance (produces jobs)
    engine.tick_instance(instance_id).await.unwrap();

    // Get task types from compile result
    let task_types = compiled.task_types;
    assert!(
        !task_types.is_empty(),
        "program should have at least one task"
    );

    // Dequeue job
    let jobs = store
        .dequeue_jobs(&task_types, 1, &TenantId::default(), "test-worker", 300_000)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1, "should have 1 job");

    let job = &jobs[0];

    // Complete job
    let completion_payload = r#"{"result":"ok"}"#;
    engine
        .complete_job(
            &job.job_key,
            completion_payload,
            job.domain_payload_hash,
            BTreeMap::new(),
        )
        .await
        .unwrap();

    // Tick again to advance past the completed job
    engine.tick_instance(instance_id).await.unwrap();

    // Check instance state
    let inst = store
        .load_instance(&TenantId::new("default").unwrap(), instance_id)
        .await
        .unwrap()
        .unwrap();

    // Read events — should have at least InstanceStarted
    let events = store
        .read_events(&TenantId::new("default").unwrap(), instance_id, 1)
        .await
        .unwrap();
    assert!(
        events.len() >= 2,
        "should have multiple events, got {}",
        events.len()
    );

    // First event should be InstanceStarted
    match &events[0].1 {
        RuntimeEvent::InstanceStarted { .. } => {}
        other => panic!("expected InstanceStarted, got {:?}", other),
    }

    // Single-task process should be Completed after completing the one job
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );

    let artifact = store
        .load_artifact(bpmn_lite_types::ArtifactHash::from_bytes(version))
        .await
        .unwrap()
        .unwrap();
    let journal_tail = store
        .read_journal(
            &TenantId::new("default").unwrap(),
            instance_id,
            Some(genesis.revision()),
        )
        .await
        .unwrap();
    let replayed = bpmn_lite_kernel::replay(&artifact, &genesis, &journal_tail).unwrap();
    let committed = store
        .load_snapshot_envelope(&TenantId::new("default").unwrap(), instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        replayed.state_hash().unwrap(),
        committed.state_hash().unwrap()
    );
}

#[tokio::test]
async fn test_artifact_insert_verifies_bytes_and_detects_collision() {
    let (pool, store, _lock) = common::setup().await;
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
      <bpmn:process id="artifact" isExecutable="true">
        <bpmn:startEvent id="start" />
        <bpmn:endEvent id="end" />
        <bpmn:sequenceFlow id="flow" sourceRef="start" targetRef="end" />
      </bpmn:process>
    </bpmn:definitions>"#;
    let graph = bpmn_lite_compiler::parse_bpmn(xml).unwrap();
    let artifact = bpmn_lite_compiler::Compiler::lower(&graph).unwrap();

    store.store_artifact(&artifact).await.unwrap();
    store.store_artifact(&artifact).await.unwrap();
    let loaded = store.load_artifact(artifact.hash()).await.unwrap().unwrap();
    assert_eq!(loaded.hash(), artifact.hash());
    assert_eq!(
        loaded.canonical_bytes().unwrap(),
        artifact.canonical_bytes().unwrap()
    );

    sqlx::query("UPDATE compiled_programs SET canonical_bytes = $2 WHERE artifact_hash = $1")
        .bind(&artifact.hash().into_bytes()[..])
        .bind(b"corrupt".as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.store_artifact(&artifact).await,
        Err(ArtifactStoreError::ArtifactCollision { .. })
    ));
}

#[tokio::test]
async fn test_legacy_artifact_load_recanonicalizes_and_records_lineage() {
    let (pool, store, _lock) = common::setup().await;
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
      <bpmn:process id="legacy" isExecutable="true">
        <bpmn:startEvent id="start" />
        <bpmn:endEvent id="end" />
        <bpmn:sequenceFlow id="flow" sourceRef="start" targetRef="end" />
      </bpmn:process>
    </bpmn:definitions>"#;
    let graph = bpmn_lite_compiler::parse_bpmn(xml).unwrap();
    let legacy = bpmn_lite_compiler::lower(&graph).unwrap();
    let old_hash = legacy.bytecode_version();
    store.store_program(old_hash, &legacy).await.unwrap();

    let migrated = store
        .load_artifact(bpmn_lite_types::ArtifactHash::from_bytes(old_hash))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(migrated.hash().into_bytes(), old_hash);

    let recorded: Vec<u8> =
        sqlx::query_scalar("SELECT new_hash FROM artifact_lineage WHERE old_hash = $1")
            .bind(&old_hash[..])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(recorded, migrated.hash().as_bytes());
}

/// F-02 fix (Phase 1, `zed_agent_execution_lease_remediation_plan.md`):
/// the real external worker protocol, `complete_job_with_claim`, must
/// reject a stale worker whose claim was reassigned, and must never
/// touch the current claimant's row or the process revision. Unlike
/// `test_phase0_f02_...` (still in `bpmn-lite-store-postgres`, which
/// exercises the legacy unconditional primitive directly), this drives
/// the actual production call path end-to-end: compile, start, dequeue,
/// force reassignment through the real reclaim path, then complete.
#[tokio::test]
async fn test_phase1_f02_complete_job_with_claim_rejects_stale_worker() {
    let (pool, store, _lock) = common::setup().await;
    let store = Arc::new(store);
    let engine = BpmnLiteEngine::new(store.clone());

    let compiled = engine.compile(SMOKE_BPMN).await.unwrap();
    let version = compiled.bytecode_version;
    let payload = r#"{"case_id":"phase1-f02"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let instance_id = engine
        .start("smoke_proc", version, payload, hash, "phase1-f02-corr")
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    let task_types = compiled.task_types;
    let jobs = store
        .dequeue_jobs(&task_types, 1, &TenantId::default(), "worker-a", 30_000)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1, "should have 1 job");
    let job_a = jobs[0].clone();
    let stale_token = job_a.claim_token.clone();

    // A stalls; force expiry and reassign to worker B through the
    // real reclaim + dequeue paths (F-06: claim_expires_at, not
    // claimed_at, is authority).
    sqlx::query(
        "UPDATE job_queue SET claim_expires_at = now() - interval '1 hour' \
         WHERE job_key = $1",
    )
    .bind(&job_a.job_key)
    .execute(&pool)
    .await
    .unwrap();
    let reclaimed = store.reclaim_stale_jobs().await.unwrap();
    assert!(reclaimed >= 1, "stale reclaim must reassign the job");

    let jobs_b = store
        .dequeue_jobs(&task_types, 1, &TenantId::default(), "worker-b", 30_000)
        .await
        .unwrap();
    assert_eq!(jobs_b.len(), 1, "worker B must claim the reassigned job");
    let job_b = &jobs_b[0];

    // A's belated completion must be rejected outright — no process
    // transition, no deletion of B's row.
    let stale_result = engine
        .complete_job_with_claim(
            &job_a.job_key,
            r#"{"result":"stale"}"#,
            job_a.domain_payload_hash,
            BTreeMap::new(),
            "worker-a",
            &stale_token,
        )
        .await;
    assert!(
        stale_result.is_err(),
        "F-02: stale worker A's completion must be rejected, not silently accepted"
    );

    let b_row: Option<(String, String)> =
        sqlx::query_as("SELECT status, worker_id FROM job_queue WHERE job_key = $1")
            .bind(&job_a.job_key)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(
        b_row,
        Some(("claimed".to_string(), "worker-b".to_string())),
        "F-02: worker B's live claim must survive worker A's rejected stale completion"
    );

    // B's legitimate completion succeeds and the instance progresses.
    engine
        .complete_job_with_claim(
            &job_b.job_key,
            r#"{"result":"ok"}"#,
            job_b.domain_payload_hash,
            BTreeMap::new(),
            "worker-b",
            &job_b.claim_token,
        )
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    let inst = store
        .load_instance(&TenantId::new("default").unwrap(), instance_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed after B's legitimate completion, got {:?}",
        inst.state
    );
}

/// F-03 (active-active recovery): `recover_all_tenants` treats a busy
/// transition claim (`None`, because a live peer already holds an
/// unexpired lease) as fatal via `.ok_or_else(...)?`, aborting the
/// entire recovery scan instead of skipping the one busy instance. A
/// second replica starting while a healthy peer is doing legitimate
/// work must not fail its own startup gate.
///
/// EXPECTED TO FAIL at baseline (`6e4de6d`) — `recover_all_tenants`
/// returns `Err`. Fixed by Phase 4's active-active-tolerant recovery.
#[tokio::test]
async fn test_phase0_f03_active_active_recovery_aborts_on_busy_lease() {
    let (_pool, store, _lock) = common::setup().await;
    let iid = Uuid::now_v7();
    let tenant_id = "default";
    let tenant = TenantId::new(tenant_id).unwrap();

    let mut inst = common::make_instance(iid);
    inst.tenant_id = tenant_id.to_string();
    common::save_instance(&store, "replica-a", &inst).await.unwrap();

    // Replica A holds a legitimate, unexpired transition lease.
    let claim = store
        .claim_instance_for_transition(&tenant, iid, "replica-a", 30_000)
        .await
        .unwrap();
    assert!(claim.is_some(), "replica A must hold a live lease");

    // Replica B starts up and runs recovery while A's lease is live.
    let engine = BpmnLiteEngine::new(Arc::new(store));
    let report = engine.recover_all_tenants("replica-b", 100, 30_000).await;

    let report = report.unwrap_or_else(|error| {
        panic!(
            "F-03: startup recovery must skip an instance busy with a live peer lease, not \
             abort the whole scan; got {error:?}"
        )
    });
    assert_eq!(
        report.instances_busy_skipped, 1,
        "the busy instance must be counted as skipped, not silently dropped"
    );
    assert_eq!(
        report.instances_verified, 0,
        "an instance recovery could not fence must not also count as verified"
    );
}
