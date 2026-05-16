//! Integration tests: exercise the full BPMN-Lite lifecycle through the engine facade.
//!
//! These tests verify the complete pipeline that the gRPC handlers delegate to:
//! Compile → StartProcess → ActivateJobs → CompleteJob → Inspect
//!
//! The gRPC handlers are thin wrappers around BpmnLiteEngine, so testing the engine
//! with proto-compatible data formats validates the full stack.

use std::collections::BTreeMap;
use std::sync::Arc;

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::{ErrorClass, ProcessState};
use bpmn_lite_vm::compute_hash;

/// Minimal BPMN with one service task.
const MINIMAL_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="test_proc" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="do_work" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

/// Two-task BPMN: Start → task_a → task_b → End
const TWO_TASK_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="two_task" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task_a" name="step_one" />
    <bpmn:serviceTask id="task_b" name="step_two" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task_a" />
    <bpmn:sequenceFlow id="f2" sourceRef="task_a" targetRef="task_b" />
    <bpmn:sequenceFlow id="f3" sourceRef="task_b" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

fn new_engine() -> Arc<BpmnLiteEngine> {
    let store = Arc::new(MemoryStore::new());
    Arc::new(BpmnLiteEngine::new(store))
}

/// Full lifecycle: Compile → Start → Run → ActivateJobs → CompleteJob → Run → Completed
#[tokio::test]
async fn test_full_lifecycle() {
    let engine = new_engine();

    // 1. Compile
    let compile_result = engine.compile(MINIMAL_BPMN).await.unwrap();
    let bytecode_version = compile_result.bytecode_version;
    assert!(!compile_result.task_types.is_empty());
    assert!(compile_result.task_types.contains(&"do_work".to_string()));

    // Verify bytecode_version is 32 bytes (proto uses bytes field)
    assert_eq!(bytecode_version.len(), 32);

    // 2. Start process
    let payload = r#"{"case":"integration-test"}"#;
    let hash = compute_hash(payload);

    let instance_id = engine
        .start("test_proc", bytecode_version, payload, hash, "corr-1")
        .await
        .unwrap();

    // Verify instance_id is a valid UUID (proto sends as string)
    let instance_str = instance_id.to_string();
    assert!(uuid::Uuid::parse_str(&instance_str).is_ok());

    // 3. Run instance — triggers job creation
    let activations = engine.run_instance(instance_id).await.unwrap();
    let extra = engine
        .activate_jobs(&["do_work".to_string()], 10)
        .await
        .unwrap();
    let all_jobs: Vec<_> = activations.into_iter().chain(extra).collect();
    assert!(!all_jobs.is_empty(), "Should have at least one job");

    let job = &all_jobs[0];
    assert_eq!(job.task_type, "do_work");
    assert_eq!(job.process_instance_id, instance_id);
    assert_eq!(job.domain_payload, payload);
    assert_eq!(job.domain_payload_hash, hash);

    // 4. Inspect — Running with parked fiber
    let inspection = engine.inspect(instance_id).await.unwrap();
    assert!(matches!(inspection.state, ProcessState::Running));
    assert!(!inspection.fibers.is_empty());

    // 5. Complete the job
    // domain_payload_hash must match the INSTANCE's current payload hash
    let result_payload = r#"{"result":"done"}"#;
    engine
        .complete_job(&job.job_key, result_payload, hash, BTreeMap::new())
        .await
        .unwrap();

    // 6. Run to advance past completed job → End
    engine.run_instance(instance_id).await.unwrap();

    // 7. Inspect — Completed
    let final_state = engine.inspect(instance_id).await.unwrap();
    assert!(
        matches!(final_state.state, ProcessState::Completed { .. }),
        "Expected Completed, got {:?}",
        final_state.state
    );

    // 8. Verify event log
    let events = engine.read_events(instance_id, 0).await.unwrap();
    assert!(
        events.len() >= 2,
        "Expected at least InstanceStarted + Completed events, got {}",
        events.len()
    );

    // Verify JSON serialization (proto uses payload_json)
    for (_, event) in &events {
        let json = serde_json::to_string(event).unwrap();
        assert!(!json.is_empty());
    }
}

/// Two-task workflow: complete both jobs sequentially.
#[tokio::test]
async fn test_two_task_sequential() {
    let engine = new_engine();

    let compile_result = engine.compile(TWO_TASK_BPMN).await.unwrap();
    let bv = compile_result.bytecode_version;

    let payload = r#"{"step":"initial"}"#;
    let hash = compute_hash(payload);
    let instance_id = engine
        .start("two_task", bv, payload, hash, "corr-2")
        .await
        .unwrap();

    // Run → first job (step_one)
    let jobs1 = engine.run_instance(instance_id).await.unwrap();
    let extra1 = engine
        .activate_jobs(&["step_one".to_string(), "step_two".to_string()], 10)
        .await
        .unwrap();
    let all1: Vec<_> = jobs1.into_iter().chain(extra1).collect();
    assert!(!all1.is_empty());
    assert_eq!(all1[0].task_type, "step_one");

    // Complete first job with updated payload
    // domain_payload_hash must match the INSTANCE's current payload hash
    let payload2 = r#"{"step":"after_one"}"#;
    engine
        .complete_job(&all1[0].job_key, payload2, hash, BTreeMap::new())
        .await
        .unwrap();

    // Run → second job (step_two)
    let jobs2 = engine.run_instance(instance_id).await.unwrap();
    let extra2 = engine
        .activate_jobs(&["step_one".to_string(), "step_two".to_string()], 10)
        .await
        .unwrap();
    let all2: Vec<_> = jobs2.into_iter().chain(extra2).collect();
    assert!(!all2.is_empty());
    assert_eq!(all2[0].task_type, "step_two");

    // Complete second job
    // domain_payload_hash must match the INSTANCE's current payload hash
    // (which was updated to payload2 by the first completion's apply_completion)
    let payload3 = r#"{"step":"after_two"}"#;
    let hash2 = compute_hash(payload2);
    engine
        .complete_job(&all2[0].job_key, payload3, hash2, BTreeMap::new())
        .await
        .unwrap();

    // Run → End
    engine.run_instance(instance_id).await.unwrap();

    let final_state = engine.inspect(instance_id).await.unwrap();
    assert!(
        matches!(final_state.state, ProcessState::Completed { .. }),
        "Expected Completed after two tasks, got {:?}",
        final_state.state
    );
}

/// Cancel flow: start process, cancel before completing.
#[tokio::test]
async fn test_cancel_flow() {
    let engine = new_engine();

    let compile_result = engine.compile(MINIMAL_BPMN).await.unwrap();
    let payload = r#"{"case":"cancel-test"}"#;
    let hash = compute_hash(payload);

    let instance_id = engine
        .start(
            "test_proc",
            compile_result.bytecode_version,
            payload,
            hash,
            "corr-cancel",
        )
        .await
        .unwrap();

    engine.run_instance(instance_id).await.unwrap();

    // Cancel
    engine
        .cancel(instance_id, "User requested cancellation")
        .await
        .unwrap();

    let inspection = engine.inspect(instance_id).await.unwrap();
    assert!(
        matches!(inspection.state, ProcessState::Cancelled { .. }),
        "Expected Cancelled, got {:?}",
        inspection.state
    );
    assert!(
        inspection.fibers.is_empty(),
        "All fibers should be deleted after cancel"
    );

    // Verify cancel event in log
    let events = engine.read_events(instance_id, 0).await.unwrap();
    let event_debug: Vec<String> = events.iter().map(|(_, e)| format!("{:?}", e)).collect();
    assert!(
        event_debug.iter().any(|e| e.contains("Cancelled")),
        "Missing Cancelled event in {:?}",
        event_debug
    );
}

/// FailJob flow: start, fail the job, verify incident.
#[tokio::test]
async fn test_fail_job_creates_incident() {
    let engine = new_engine();

    let compile_result = engine.compile(MINIMAL_BPMN).await.unwrap();
    let payload = r#"{"case":"fail-test"}"#;
    let hash = compute_hash(payload);

    let instance_id = engine
        .start(
            "test_proc",
            compile_result.bytecode_version,
            payload,
            hash,
            "corr-fail",
        )
        .await
        .unwrap();

    let activations = engine.run_instance(instance_id).await.unwrap();
    let extra = engine
        .activate_jobs(&["do_work".to_string()], 10)
        .await
        .unwrap();
    let all_jobs: Vec<_> = activations.into_iter().chain(extra).collect();
    assert!(!all_jobs.is_empty());

    // Fail the job
    engine
        .fail_job(
            &all_jobs[0].job_key,
            ErrorClass::Transient,
            "Service temporarily unavailable",
        )
        .await
        .unwrap();

    let inspection = engine.inspect(instance_id).await.unwrap();
    assert!(
        !inspection.incidents.is_empty(),
        "Should have an incident after fail_job"
    );
    assert_eq!(
        inspection.incidents[0].message,
        "Service temporarily unavailable"
    );
}

/// Compilation error: invalid BPMN should fail gracefully.
#[tokio::test]
async fn test_compile_invalid_bpmn() {
    let engine = new_engine();

    let result = engine.compile("<invalid>not bpmn</invalid>").await;
    assert!(result.is_err(), "Should fail on invalid BPMN");
}

/// gRPC over-the-wire smoke test against a running server.
///
/// Set BPMN_LITE_URL to run (e.g., `BPMN_LITE_URL=http://127.0.0.1:50051`).
/// Skipped by default (ignored test). Run with:
///   cargo test --test integration test_grpc_smoke -- --ignored
#[tokio::test]
#[ignore]
async fn test_grpc_smoke() {
    use ::bpmn_lite_server::grpc::proto::bpmn_lite_client::BpmnLiteClient;
    use ::bpmn_lite_server::grpc::proto::*;

    let url =
        std::env::var("BPMN_LITE_URL").unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());

    let mut client = BpmnLiteClient::connect(url.clone())
        .await
        .unwrap_or_else(|e| panic!("Cannot connect to {}: {}", url, e));

    // 1. Compile
    let compile_resp = client
        .compile(CompileRequest {
            bpmn_xml: MINIMAL_BPMN.to_string(),
            validate_only: false,
        })
        .await
        .expect("Compile RPC failed")
        .into_inner();

    assert!(
        !compile_resp.bytecode_version.is_empty(),
        "bytecode_version should be non-empty"
    );
    println!(
        "Compile OK: bytecode_version={} bytes, diagnostics={}",
        compile_resp.bytecode_version.len(),
        compile_resp.diagnostics.len()
    );

    // 2. Start process
    let payload = r#"{"case":"grpc-smoke"}"#;
    let hash = compute_hash(payload);

    let start_resp = client
        .start_process(StartRequest {
            process_key: "test_proc".to_string(),
            bytecode_version: compile_resp.bytecode_version,
            domain_payload: payload.to_string(),
            domain_payload_hash: hash.to_vec(),
            session_stack_json: String::new(),
            orch_flags: Default::default(),
            correlation_id: "smoke-corr-1".to_string(),
            entry_id: uuid::Uuid::new_v4().to_string(),
            runbook_id: uuid::Uuid::new_v4().to_string(),
            tenant_id: String::new(),
        })
        .await
        .expect("StartProcess RPC failed")
        .into_inner();

    let instance_id = start_resp.process_instance_id.clone();
    assert!(!instance_id.is_empty());
    println!("StartProcess OK: instance_id={}", instance_id);

    // 3. Inspect — should be Running with a parked fiber (waiting on job)
    let inspect_resp = client
        .inspect(InspectRequest {
            process_instance_id: instance_id.clone(),
            tenant_id: String::new(),
        })
        .await
        .expect("Inspect RPC failed")
        .into_inner();

    assert_eq!(inspect_resp.state, "RUNNING");
    println!(
        "Inspect OK: state={}, fibers={}, waits={}",
        inspect_resp.state,
        inspect_resp.fibers.len(),
        inspect_resp.waits.len()
    );

    // 4. ActivateJobs — get the pending job
    let mut jobs_stream = client
        .activate_jobs(ActivateJobsRequest {
            task_types: vec!["do_work".to_string()],
            max_jobs: 10,
            timeout_ms: 1000,
            worker_id: "smoke-worker".to_string(),
            tenant_id: String::new(),
        })
        .await
        .expect("ActivateJobs RPC failed")
        .into_inner();

    let mut jobs = Vec::new();
    while let Some(job) = jobs_stream
        .message()
        .await
        .expect("Error reading job stream")
    {
        jobs.push(job);
    }
    assert!(!jobs.is_empty(), "Should have at least one job activation");
    let job = &jobs[0];
    assert_eq!(job.task_type, "do_work");
    println!(
        "ActivateJobs OK: {} job(s), first task_type={}",
        jobs.len(),
        job.task_type
    );

    // 5. CompleteJob — complete the job with updated payload
    // domain_payload_hash must match the INSTANCE's current payload hash
    let result_payload = r#"{"result":"smoke-done"}"#;

    client
        .complete_job(CompleteJobRequest {
            job_key: job.job_key.clone(),
            domain_payload: result_payload.to_string(),
            domain_payload_hash: hash.to_vec(),
            orch_flags: Default::default(),
            worker_id: job.worker_id.clone(),
            claim_token: job.claim_token.clone(),
            tenant_id: job.tenant_id.clone(),
        })
        .await
        .expect("CompleteJob RPC failed");
    println!("CompleteJob OK: job_key={}", job.job_key);

    // 6. Inspect again — should be Completed (single-task process)
    let final_inspect = client
        .inspect(InspectRequest {
            process_instance_id: instance_id.clone(),
            tenant_id: String::new(),
        })
        .await
        .expect("Final Inspect failed")
        .into_inner();

    assert_eq!(
        final_inspect.state, "COMPLETED",
        "Expected COMPLETED after job completion, got {}",
        final_inspect.state
    );
    println!("Final Inspect OK: state={}", final_inspect.state);

    // 7. SubscribeEvents — verify event log
    let mut events_stream = client
        .subscribe_events(SubscribeRequest {
            process_instance_id: instance_id,
            tenant_id: String::new(),
        })
        .await
        .expect("SubscribeEvents RPC failed")
        .into_inner();

    let mut events = Vec::new();
    while let Some(event) = events_stream
        .message()
        .await
        .expect("Error reading event stream")
    {
        events.push(event);
    }
    assert!(
        events.len() >= 2,
        "Expected at least 2 events (Started, Completed), got {}",
        events.len()
    );
    println!("SubscribeEvents OK: {} events", events.len());

    println!("\n=== gRPC smoke test PASSED (full lifecycle over the wire) ===");
}

/// Domain payload hash flows through correctly.
#[tokio::test]
async fn test_payload_hash_integrity() {
    let engine = new_engine();

    let compile_result = engine.compile(MINIMAL_BPMN).await.unwrap();
    let payload = r#"{"important":"data","hash":"must-match"}"#;
    let hash = compute_hash(payload);

    let instance_id = engine
        .start(
            "test_proc",
            compile_result.bytecode_version,
            payload,
            hash,
            "corr-hash",
        )
        .await
        .unwrap();

    let activations = engine.run_instance(instance_id).await.unwrap();
    let extra = engine
        .activate_jobs(&["do_work".to_string()], 10)
        .await
        .unwrap();
    let all_jobs: Vec<_> = activations.into_iter().chain(extra).collect();
    assert!(!all_jobs.is_empty());

    // Verify the job carries the original hash
    assert_eq!(all_jobs[0].domain_payload_hash, hash);
    assert_eq!(all_jobs[0].domain_payload, payload);
}
