use super::*;
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::*;
use bpmn_lite_vm::{compute_hash, TickOutcome, Vm};
use ob_poc_types::session_stack::SessionStackState;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

/// Integration test: compile → start → run → activate jobs → complete → verify completion
#[tokio::test]
async fn test_engine_full_lifecycle() {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // 1. Compile a minimal BPMN
    let bpmn = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="test_proc" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:serviceTask id="task1" name="Do Work">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="do_work" />
              </bpmn:extensionElements>
            </bpmn:serviceTask>
            <bpmn:endEvent id="end" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
            <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
          </bpmn:process>
        </bpmn:definitions>"#;

    let compile_result = engine.compile(bpmn).await.unwrap();
    assert!(!compile_result.task_types.is_empty());

    // 2. Start a process
    let payload = r#"{"case":"test"}"#;
    let hash = compute_hash(payload);
    let instance_id = engine
        .start(
            "test_proc",
            compile_result.bytecode_version,
            payload,
            hash,
            "corr-1",
        )
        .await
        .unwrap();

    // 3. Run the instance — should enqueue a job and park
    let activations = engine.run_instance(instance_id).await.unwrap();

    // 4. Inspect — should be Running with a fiber parked on Job
    let inspection = engine.inspect(instance_id).await.unwrap();
    assert_eq!(inspection.state, ProcessState::Running);

    // 5. Activate jobs — may have been dequeued in run_instance already
    let extra_jobs = engine
        .activate_jobs(&["do_work".to_string()], 10)
        .await
        .unwrap();
    let all_jobs: Vec<_> = activations.into_iter().chain(extra_jobs).collect();
    assert!(
        !all_jobs.is_empty(),
        "Should have at least one job activation"
    );

    let job = &all_jobs[0];
    let job_key = job.job_key.clone();

    // 6. Complete the job
    // domain_payload_hash must match the INSTANCE's current payload hash
    let result_payload = r#"{"result":"done"}"#;
    engine
        .complete_job(&job_key, result_payload, hash, BTreeMap::new())
        .await
        .unwrap();

    // 7. Run instance again to advance past the completed job
    engine.run_instance(instance_id).await.unwrap();

    // 8. Inspect — should be Completed
    let final_inspection = engine.inspect(instance_id).await.unwrap();
    assert!(
        matches!(final_inspection.state, ProcessState::Completed { .. }),
        "Expected Completed, got {:?}",
        final_inspection.state
    );

    // 9. Verify events
    let events = engine.read_events(instance_id, 0).await.unwrap();
    assert!(events.len() >= 2); // At least InstanceStarted + Completed
}

#[tokio::test]
async fn test_start_with_session_stack_copies_value() {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let bpmn = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="copy_proc" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:endEvent id="end" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="end" />
          </bpmn:process>
        </bpmn:definitions>"#;

    let compile_result = engine.compile(bpmn).await.unwrap();
    let payload = r#"{"case":"copy"}"#;
    let hash = compute_hash(payload);
    let original_scope_id = Uuid::new_v4();
    let mutated_scope_id = Uuid::new_v4();

    let mut session_stack = SessionStackState {
        session_id: Uuid::new_v4(),
        scope: Some(ob_poc_types::session_stack::SessionScopeState {
            client_group_id: original_scope_id,
            client_group_name: Some("Original".to_string()),
        }),
        active_workspace: Some(ob_poc_types::session_stack::SessionWorkspaceKind::Kyc),
        workspace_stack: Vec::new(),
        trace_sequence: 5,
    };

    let instance_id = engine
        .start_with_params(StartParams {
            process_key: "copy_proc".to_string(),
            bytecode_version: compile_result.bytecode_version,
            domain_payload: payload.to_string(),
            domain_payload_hash: hash,
            correlation_id: "corr-copy".to_string(),
            session_stack: session_stack.clone(),
            entry_id: Uuid::new_v4(),
            runbook_id: Uuid::new_v4(),
        })
        .await
        .unwrap();

    session_stack.scope = Some(ob_poc_types::session_stack::SessionScopeState {
        client_group_id: mutated_scope_id,
        client_group_name: Some("Mutated".to_string()),
    });
    session_stack.active_workspace = Some(ob_poc_types::session_stack::SessionWorkspaceKind::Deal);
    session_stack.trace_sequence = 77;

    let loaded = store.load_instance(instance_id).await.unwrap().unwrap();
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
        Some(ob_poc_types::session_stack::SessionWorkspaceKind::Kyc)
    );
    assert_eq!(loaded.session_stack.trace_sequence, 5);
}

#[tokio::test]
async fn test_job_activation_preserves_runbook_lineage() {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let bpmn = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
          <bpmn:process id="lineage_proc" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:serviceTask id="work" name="lineage_task" />
            <bpmn:endEvent id="end" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="work" />
            <bpmn:sequenceFlow id="f2" sourceRef="work" targetRef="end" />
          </bpmn:process>
        </bpmn:definitions>"#;

    let compile_result = engine.compile(bpmn).await.unwrap();
    let payload = r#"{"case":"lineage"}"#;
    let hash = compute_hash(payload);
    let entry_id = Uuid::new_v4();
    let runbook_id = Uuid::new_v4();

    let instance_id = engine
        .start_with_params(StartParams {
            process_key: "lineage_proc".to_string(),
            bytecode_version: compile_result.bytecode_version,
            domain_payload: payload.to_string(),
            domain_payload_hash: hash,
            correlation_id: "corr-lineage".to_string(),
            session_stack: SessionStackState::default(),
            entry_id,
            runbook_id,
        })
        .await
        .unwrap();

    engine.tick_instance(instance_id).await.unwrap();
    let jobs = engine
        .activate_jobs(&["lineage_task".to_string()], 1)
        .await
        .unwrap();

    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].entry_id, entry_id);
    assert_eq!(jobs[0].runbook_id, runbook_id);
}

// ── Shared BPMN fixture for T-CANCEL tests ──

const SINGLE_TASK_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                      xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
      <bpmn:process id="cancel_proc" isExecutable="true">
        <bpmn:startEvent id="start" />
        <bpmn:serviceTask id="task1" name="Work">
          <bpmn:extensionElements>
            <zeebe:taskDefinition type="do_work" />
          </bpmn:extensionElements>
        </bpmn:serviceTask>
        <bpmn:endEvent id="end" />
        <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
        <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
      </bpmn:process>
    </bpmn:definitions>"#;

/// Helper: compile + start + run until job is parked, return (engine, store, instance_id, job_key, hash).
async fn setup_parked_job() -> (
    BpmnLiteEngine,
    Arc<dyn ProcessStore>,
    Uuid,
    String,
    [u8; 32],
) {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let cr = engine.compile(SINGLE_TASK_BPMN).await.unwrap();
    let payload = r#"{"case":"cancel-test"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start(
            "cancel_proc",
            cr.bytecode_version,
            payload,
            hash,
            "corr-cancel",
        )
        .await
        .unwrap();

    let activations = engine.run_instance(iid).await.unwrap();
    let extra = engine
        .activate_jobs(&["do_work".to_string()], 10)
        .await
        .unwrap();
    let all: Vec<_> = activations.into_iter().chain(extra).collect();
    assert!(!all.is_empty(), "Expected at least one job activation");
    let job_key = all[0].job_key.clone();

    (engine, store, iid, job_key, hash)
}

// ── T-CANCEL-1: complete_job on cancelled instance → Ok + SignalIgnored ──

#[tokio::test]
async fn t_cancel_complete_after_cancel() {
    let (engine, store, iid, job_key, hash) = setup_parked_job().await;

    // Cancel the instance while job is parked
    engine.cancel(iid, "user-requested").await.unwrap();

    // Attempt complete_job on cancelled instance — should succeed (no error)
    let result = engine
        .complete_job(&job_key, r#"{"late":"true"}"#, hash, BTreeMap::new())
        .await;
    assert!(
        result.is_ok(),
        "complete_job on cancelled instance should not error"
    );

    // Verify SignalIgnored event was emitted
    let events = store.read_events(iid, 0).await.unwrap();
    let has_signal_ignored = events.iter().any(|(_, e)| {
            matches!(e, RuntimeEvent::SignalIgnored { signal_desc } if signal_desc.contains("Cancelled"))
        });
    assert!(
        has_signal_ignored,
        "Expected SignalIgnored event, got: {:?}",
        events.iter().map(|(_, e)| e).collect::<Vec<_>>()
    );

    // Verify instance is still Cancelled (no state corruption)
    let inspection = engine.inspect(iid).await.unwrap();
    assert!(matches!(inspection.state, ProcessState::Cancelled { .. }));
}

// ── T-CANCEL-2: duplicate complete_job → Ok (dedupe, no double mutation) ──

#[tokio::test]
async fn t_cancel_duplicate_complete() {
    let (engine, store, iid, job_key, hash) = setup_parked_job().await;

    // First complete — should succeed normally
    engine
        .complete_job(&job_key, r#"{"r":"first"}"#, hash, BTreeMap::new())
        .await
        .unwrap();

    // Count events after first complete
    let events_after_first = store.read_events(iid, 0).await.unwrap().len();

    // Second complete with same job_key — should be silently accepted (dedupe)
    let result = engine
        .complete_job(&job_key, r#"{"r":"second"}"#, hash, BTreeMap::new())
        .await;
    assert!(result.is_ok(), "Duplicate complete_job should not error");

    // No new events should be emitted (dedupe short-circuits)
    let events_after_second = store.read_events(iid, 0).await.unwrap().len();
    assert_eq!(
        events_after_first, events_after_second,
        "Dedupe should not emit additional events"
    );
}

#[tokio::test]
async fn test_complete_job_recomputes_payload_hash() {
    let (engine, store, iid, job_key, expected_hash) = setup_parked_job().await;
    let new_payload = r#"{"result":"done","version":2}"#;
    let new_hash = compute_hash(new_payload);

    engine
        .complete_job(&job_key, new_payload, expected_hash, BTreeMap::new())
        .await
        .unwrap();

    let persisted = store.load_instance(iid).await.unwrap().unwrap();
    assert_eq!(persisted.domain_payload.as_ref(), new_payload);
    assert_eq!(persisted.domain_payload_hash, new_hash);

    let history_payload = store
        .load_payload_version(iid, &new_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(history_payload, new_payload);

    let events = store.read_events(iid, 0).await.unwrap();
    let completed = events
        .iter()
        .find_map(|(_, event)| match event {
            RuntimeEvent::JobCompleted {
                payload_hash_before,
                payload_hash_after,
                ..
            } => Some((*payload_hash_before, *payload_hash_after)),
            _ => None,
        })
        .expect("missing JobCompleted event");
    assert_eq!(completed.0, expected_hash);
    assert_eq!(completed.1, new_hash);
}

#[tokio::test]
async fn test_complete_job_rejects_stale_expected_hash() {
    let (engine, store, iid, job_key, _expected_hash) = setup_parked_job().await;
    let stale_hash = compute_hash(r#"{"stale":true}"#);

    let result = engine
        .complete_job(
            &job_key,
            r#"{"result":"nope"}"#,
            stale_hash,
            BTreeMap::new(),
        )
        .await;
    assert!(result.is_err(), "stale expected hash must be rejected");

    let persisted = store.load_instance(iid).await.unwrap().unwrap();
    assert_eq!(
        persisted.domain_payload.as_ref(),
        r#"{"case":"cancel-test"}"#
    );
    assert_eq!(
        persisted.domain_payload_hash,
        compute_hash(r#"{"case":"cancel-test"}"#)
    );
}

// ── T-CANCEL-3: cancel purges job queue + emits WaitCancelled ──

#[tokio::test]
async fn t_cancel_purges_jobs() {
    let (engine, store, iid, _job_key, _hash) = setup_parked_job().await;

    // Verify fiber is parked on Job before cancel
    let inspection = engine.inspect(iid).await.unwrap();
    assert_eq!(inspection.fibers.len(), 1);
    assert!(matches!(
        inspection.fibers[0].wait_state,
        WaitState::Job { .. }
    ));

    // Cancel — should purge jobs and emit WaitCancelled
    engine.cancel(iid, "cleanup").await.unwrap();

    // Verify no fibers remain
    let post_cancel = engine.inspect(iid).await.unwrap();
    assert!(
        post_cancel.fibers.is_empty(),
        "All fibers should be deleted"
    );

    // Verify job queue is empty (no orphan jobs)
    let remaining_jobs = engine
        .activate_jobs(&["do_work".to_string()], 10)
        .await
        .unwrap();
    assert!(
        remaining_jobs.is_empty(),
        "Job queue should be purged after cancel"
    );

    // Verify WaitCancelled event was emitted
    let events = store.read_events(iid, 0).await.unwrap();
    let has_wait_cancelled = events.iter().any(
        |(_, e)| matches!(e, RuntimeEvent::WaitCancelled { reason, .. } if reason == "cleanup"),
    );
    assert!(
        has_wait_cancelled,
        "Expected WaitCancelled event, got: {:?}",
        events.iter().map(|(_, e)| e).collect::<Vec<_>>()
    );

    // Verify Cancelled event also emitted
    let has_cancelled = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::Cancelled { reason } if reason == "cleanup"));
    assert!(has_cancelled, "Expected Cancelled event");
}

// ── T-CANCEL-4: signal on completed instance → Ok + SignalIgnored ──

#[tokio::test]
async fn t_cancel_signal_after_complete() {
    let (engine, store, iid, job_key, hash) = setup_parked_job().await;

    // Complete the job and advance to End
    engine
        .complete_job(&job_key, r#"{"done":true}"#, hash, BTreeMap::new())
        .await
        .unwrap();
    engine.run_instance(iid).await.unwrap();

    // Verify instance is Completed
    let inspection = engine.inspect(iid).await.unwrap();
    assert!(
        matches!(inspection.state, ProcessState::Completed { .. }),
        "Expected Completed, got {:?}",
        inspection.state
    );

    // Signal on completed instance — should succeed (no error)
    let result = engine
        .signal(iid, "late_msg", "corr-1", None, None, Some("late-1"))
        .await;
    assert!(
        result.is_ok(),
        "signal on completed instance should not error"
    );

    // Verify SignalIgnored event was emitted
    let events = store.read_events(iid, 0).await.unwrap();
    let has_signal_ignored = events.iter().any(|(_, e)| {
            matches!(e, RuntimeEvent::SignalIgnored { signal_desc } if signal_desc.contains("Completed"))
        });
    assert!(
        has_signal_ignored,
        "Expected SignalIgnored event for completed instance"
    );
}

// ── T-CANCEL-5: signal on running instance with no Msg fiber → Ok + SignalIgnored ──

#[tokio::test]
async fn t_cancel_signal_no_match() {
    let (engine, store, iid, _job_key, _hash) = setup_parked_job().await;

    // Instance is Running with fiber parked on Job (not Msg)
    let inspection = engine.inspect(iid).await.unwrap();
    assert_eq!(inspection.state, ProcessState::Running);
    assert!(matches!(
        inspection.fibers[0].wait_state,
        WaitState::Job { .. }
    ));

    // Signal — no fiber is waiting for a message
    let result = engine
        .signal(iid, "ghost_msg", "corr-ghost", None, None, Some("ghost-1"))
        .await;
    assert!(
        result.is_ok(),
        "signal with no matching fiber should not error"
    );

    // Verify the unmatched running signal was durably buffered.
    let events = store.read_events(iid, 0).await.unwrap();
    let has_signal_ignored = events.iter().any(
        |(_, e)| matches!(e, RuntimeEvent::MessageBuffered { msg_id, .. } if msg_id == "ghost-1"),
    );
    assert!(
        has_signal_ignored,
        "Expected MessageBuffered event for no-match signal, got: {:?}",
        events.iter().map(|(_, e)| e).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_transient_fail_with_claim_retries_then_requeues() {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let cr = engine.compile(SINGLE_TASK_BPMN).await.unwrap();
    let payload = r#"{"case":"retry"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start(
            "cancel_proc",
            cr.bytecode_version,
            payload,
            hash,
            "corr-retry",
        )
        .await
        .unwrap();
    engine.tick_instance(iid).await.unwrap();

    let jobs = engine
        .activate_jobs_for_worker_with_lease(&["do_work".to_string()], 1, "worker-a", 300_000)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);

    engine
        .fail_job_with_claim(
            &jobs[0].job_key,
            ErrorClass::Transient,
            "temporary outage",
            &jobs[0].worker_id,
            &jobs[0].claim_token,
        )
        .await
        .unwrap();

    let inspection = engine.inspect(iid).await.unwrap();
    assert!(matches!(inspection.state, ProcessState::Running));
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;

    let retried = engine
        .activate_jobs_for_worker_with_lease(&["do_work".to_string()], 1, "worker-b", 300_000)
        .await
        .unwrap();
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].job_key, jobs[0].job_key);
    assert_eq!(retried[0].worker_id, "worker-b");
    assert_eq!(retried[0].retries_remaining, 2);
}

#[tokio::test]
async fn test_signal_matches_message_name_and_correlation_key() {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = CompiledProgram {
        bytecode_version: [90u8; 32],
        program: vec![
            Instr::WaitMsg {
                wait_id: 0,
                name: 1,
                corr_reg: 0,
            },
            Instr::End,
        ],
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::from([(1, "case_arrived".to_string())]),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let payload = r#"{"case":"signal"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start(
            "signal_proc",
            program.bytecode_version,
            payload,
            hash,
            "corr",
        )
        .await
        .unwrap();
    engine.tick_instance(iid).await.unwrap();

    let fibers = store.load_fibers(iid).await.unwrap();
    assert!(matches!(fibers[0].wait, WaitState::Msg { .. }));

    engine
        .signal_with_value(
            iid,
            "case_arrived",
            Value::Bool(true),
            None,
            None,
            Some("wrong"),
        )
        .await
        .unwrap();
    let fibers = store.load_fibers(iid).await.unwrap();
    assert!(matches!(fibers[0].wait, WaitState::Msg { .. }));

    engine
        .signal_with_value(
            iid,
            "case_arrived",
            Value::Bool(false),
            None,
            None,
            Some("right"),
        )
        .await
        .unwrap();
    let fibers = store.load_fibers(iid).await.unwrap();
    assert_eq!(fibers[0].wait, WaitState::Running);
    assert_eq!(fibers[0].pc, 1);

    let events_after_first_delivery = store.read_events(iid, 0).await.unwrap().len();
    engine
        .signal_with_value(
            iid,
            "case_arrived",
            Value::Bool(false),
            None,
            None,
            Some("right"),
        )
        .await
        .unwrap();
    let events_after_duplicate = store.read_events(iid, 0).await.unwrap().len();
    assert_eq!(events_after_duplicate, events_after_first_delivery);
}

#[tokio::test]
async fn test_signal_before_wait_msg_is_buffered_and_consumed() {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = CompiledProgram {
        bytecode_version: [91u8; 32],
        program: vec![
            Instr::WaitMsg {
                wait_id: 0,
                name: 1,
                corr_reg: 0,
            },
            Instr::End,
        ],
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let payload = r#"{"case":"early-signal"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start(
            "signal_proc",
            program.bytecode_version,
            payload,
            hash,
            "corr",
        )
        .await
        .unwrap();

    engine
        .signal_with_value(iid, "1", Value::Bool(false), None, None, Some("early"))
        .await
        .unwrap();

    engine.tick_instance(iid).await.unwrap();

    let inspection = engine.inspect(iid).await.unwrap();
    assert!(matches!(inspection.state, ProcessState::Completed { .. }));
    let events = store.read_events(iid, 0).await.unwrap();
    assert!(events
        .iter()
        .any(|(_, event)| matches!(event, RuntimeEvent::MessageBuffered { .. })));
    assert!(events
        .iter()
        .any(|(_, event)| matches!(event, RuntimeEvent::BufferedMessageConsumed { .. })));
}

#[tokio::test]
async fn test_signal_requires_msg_id_for_idempotency() {
    let (engine, _store, iid, _job_key, _hash) = setup_parked_job().await;

    let result = engine
        .signal(iid, "ghost_msg", "corr-ghost", None, None, None)
        .await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("msg_id is required"));
}

#[tokio::test]
async fn test_tenant_scoped_engine_rejects_cross_tenant_instance_access() {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let tenant_a = BpmnLiteEngine::new_with_tenant(store.clone(), "tenant-a");
    let tenant_b = BpmnLiteEngine::new_with_tenant(store.clone(), "tenant-b");

    let compile_result = tenant_a.compile(SINGLE_TASK_BPMN).await.unwrap();
    let payload = r#"{"case":"tenant-a"}"#;
    let hash = compute_hash(payload);
    let iid = tenant_a
        .start(
            "cancel_proc",
            compile_result.bytecode_version,
            payload,
            hash,
            "tenant-corr",
        )
        .await
        .unwrap();
    tenant_a.tick_instance(iid).await.unwrap();

    let inspection = tenant_a.inspect(iid).await.unwrap();
    assert_eq!(inspection.tenant_id, "tenant-a");

    assert!(tenant_b.inspect(iid).await.is_err());
    assert!(tenant_b.read_events(iid, 0).await.is_err());

    let tenant_b_jobs = tenant_b
        .activate_jobs_for_worker(&["do_work".to_string()], 10, "worker-b")
        .await
        .unwrap();
    assert!(tenant_b_jobs.is_empty());

    let tenant_a_jobs = tenant_a
        .activate_jobs_for_worker(&["do_work".to_string()], 10, "worker-a")
        .await
        .unwrap();
    assert_eq!(tenant_a_jobs.len(), 1);
    assert_eq!(tenant_a_jobs[0].tenant_id, "tenant-a");
}

#[tokio::test]
async fn test_recovery_scanner_reports_running_instance_inconsistencies_by_tenant() {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let tenant_a = BpmnLiteEngine::new_with_tenant(store.clone(), "tenant-a");
    let tenant_b = BpmnLiteEngine::new_with_tenant(store.clone(), "tenant-b");

    let instance_id = Uuid::now_v7();
    let payload = "{}";
    let instance = ProcessInstance {
        instance_id,
        tenant_id: "tenant-a".to_string(),
        process_key: "orphaned".to_string(),
        bytecode_version: [17u8; 32],
        domain_payload: payload.into(),
        domain_payload_hash: compute_hash(payload),
        session_stack: SessionStackState::default(),
        flags: BTreeMap::new(),
        counters: BTreeMap::new(),
        join_expected: BTreeMap::new(),
        state: ProcessState::Running,
        correlation_id: "recover-me".to_string(),
        entry_id: Uuid::nil(),
        runbook_id: Uuid::nil(),
        created_at: 1,
    };
    store.save_instance(&instance).await.unwrap();

    let issues = tenant_a.scan_recoverable_inconsistencies().await.unwrap();
    let kinds = issues
        .iter()
        .map(|issue| issue.kind.as_str())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"missing_program"));
    assert!(kinds.contains(&"missing_fibers"));
    assert!(kinds.contains(&"missing_start_event"));

    let tenant_b_issues = tenant_b.scan_recoverable_inconsistencies().await.unwrap();
    assert!(tenant_b_issues.is_empty());
}

// ═══════════════════════════════════════════════════════════
//  Phase 2A: Non-Interrupting Boundary Timer Tests (T-NI)
// ═══════════════════════════════════════════════════════════

/// BPMN with non-interrupting boundary timer (cancelActivity="false").
const NI_BOUNDARY_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                      xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
      <bpmn:process id="ni_proc" isExecutable="true">
        <bpmn:startEvent id="start" />
        <bpmn:serviceTask id="long_task" name="Long Running Task">
          <bpmn:extensionElements>
            <zeebe:taskDefinition type="long_work" />
          </bpmn:extensionElements>
        </bpmn:serviceTask>
        <bpmn:boundaryEvent id="reminder" attachedToRef="long_task" cancelActivity="false">
          <bpmn:timerEventDefinition>
            <bpmn:timeDuration>PT1S</bpmn:timeDuration>
          </bpmn:timerEventDefinition>
        </bpmn:boundaryEvent>
        <bpmn:serviceTask id="send_reminder" name="Send Reminder">
          <bpmn:extensionElements>
            <zeebe:taskDefinition type="send_reminder" />
          </bpmn:extensionElements>
        </bpmn:serviceTask>
        <bpmn:endEvent id="end_normal" />
        <bpmn:endEvent id="end_reminder" />
        <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="long_task" />
        <bpmn:sequenceFlow id="f2" sourceRef="long_task" targetRef="end_normal" />
        <bpmn:sequenceFlow id="f3" sourceRef="reminder" targetRef="send_reminder" />
        <bpmn:sequenceFlow id="f4" sourceRef="send_reminder" targetRef="end_reminder" />
      </bpmn:process>
    </bpmn:definitions>"#;

/// BPMN with non-interrupting cycle timer (R3/PT1S — fires 3 times).
const NI_CYCLE_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                      xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
      <bpmn:process id="ni_cycle_proc" isExecutable="true">
        <bpmn:startEvent id="start" />
        <bpmn:serviceTask id="long_task" name="Long Running Task">
          <bpmn:extensionElements>
            <zeebe:taskDefinition type="long_work" />
          </bpmn:extensionElements>
        </bpmn:serviceTask>
        <bpmn:boundaryEvent id="reminder" attachedToRef="long_task" cancelActivity="false">
          <bpmn:timerEventDefinition>
            <bpmn:timeCycle>R3/PT1S</bpmn:timeCycle>
          </bpmn:timerEventDefinition>
        </bpmn:boundaryEvent>
        <bpmn:serviceTask id="send_reminder" name="Send Reminder">
          <bpmn:extensionElements>
            <zeebe:taskDefinition type="send_reminder" />
          </bpmn:extensionElements>
        </bpmn:serviceTask>
        <bpmn:endEvent id="end_normal" />
        <bpmn:endEvent id="end_reminder" />
        <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="long_task" />
        <bpmn:sequenceFlow id="f2" sourceRef="long_task" targetRef="end_normal" />
        <bpmn:sequenceFlow id="f3" sourceRef="reminder" targetRef="send_reminder" />
        <bpmn:sequenceFlow id="f4" sourceRef="send_reminder" targetRef="end_reminder" />
      </bpmn:process>
    </bpmn:definitions>"#;

/// Helper: compile + start + tick until fiber is promoted to Race, return components.
/// Manipulates timer deadline to be in the past for immediate firing.
async fn setup_ni_race(
    bpmn: &str,
) -> (
    BpmnLiteEngine,
    Arc<dyn ProcessStore>,
    Uuid,
    String,
    [u8; 32],
) {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let cr = engine.compile(bpmn).await.unwrap();
    let payload = r#"{"case":"ni-test"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start("ni_proc", cr.bytecode_version, payload, hash, "corr-ni")
        .await
        .unwrap();

    // Tick to get fiber parked on Job, then promoted to Race
    engine.tick_instance(iid).await.unwrap();

    // Dequeue jobs so we have the job_key
    let jobs = engine
        .activate_jobs(&["long_work".to_string()], 10)
        .await
        .unwrap();
    assert!(!jobs.is_empty(), "Expected job activation");
    let job_key = jobs[0].job_key.clone();

    // Verify fiber is now in Race state
    let fibers = store.load_fibers(iid).await.unwrap();
    assert_eq!(fibers.len(), 1);
    assert!(
        matches!(&fibers[0].wait, WaitState::Race { .. }),
        "Expected Race, got {:?}",
        fibers[0].wait
    );

    // Manipulate deadline to be in the past so next tick fires the timer
    let mut fiber = fibers[0].clone();
    if let WaitState::Race {
        ref mut timer_deadline_ms,
        ..
    } = fiber.wait
    {
        *timer_deadline_ms = Some(0); // epoch = definitely in the past
    }
    store.save_fiber(iid, &fiber).await.unwrap();

    (engine, store, iid, job_key, hash)
}

// ── T-NI-1: Non-interrupting timer fires → spawns child, main stays in Race ──

#[tokio::test]
async fn t_ni_1_non_interrupting_spawns_child() {
    let (engine, store, iid, job_key, _hash) = setup_ni_race(NI_BOUNDARY_BPMN).await;

    // Tick — timer deadline is in the past, should fire non-interrupting
    engine.tick_instance(iid).await.unwrap();

    // Verify: should now have 2 fibers (main in Race/Job, child Running)
    let fibers = store.load_fibers(iid).await.unwrap();
    assert_eq!(
        fibers.len(),
        2,
        "Expected 2 fibers (main + spawned child), got {}",
        fibers.len()
    );

    // Main fiber should still reference the job (either Race or Job state)
    let main_fiber = fibers.iter().find(|f| {
        matches!(&f.wait, WaitState::Race { job_key: Some(jk), .. } if *jk == job_key)
            || matches!(&f.wait, WaitState::Job { job_key: jk } if *jk == job_key)
    });
    assert!(
        main_fiber.is_some(),
        "Main fiber should still have the job_key"
    );

    // Verify BoundaryFired event was emitted
    let events = store.read_events(iid, 0).await.unwrap();
    let has_boundary_fired = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::BoundaryFired { .. }));
    assert!(
        has_boundary_fired,
        "Expected BoundaryFired event, got: {:?}",
        events.iter().map(|(_, e)| e).collect::<Vec<_>>()
    );

    // Instance should still be Running (not Completed)
    let inspection = engine.inspect(iid).await.unwrap();
    assert_eq!(inspection.state, ProcessState::Running);
}

// ── T-NI-2: Cycle R3 fires 3 times, spawns 3 child fibers ──

#[tokio::test]
async fn t_ni_2_cycle_fires_multiple_times() {
    let (engine, store, iid, _job_key, _hash) = setup_ni_race(NI_CYCLE_BPMN).await;

    // Fire 3 iterations by ticking + resetting deadline each time
    for i in 0..3 {
        engine.tick_instance(iid).await.unwrap();

        let fibers = store.load_fibers(iid).await.unwrap();
        // After each fire: 1 main + (i+1) child fibers
        // But child fibers may have run to End and been removed
        // Just check that total is >= 1 (main still exists)
        assert!(
            !fibers.is_empty(),
            "Fibers should not be empty after iteration {}",
            i
        );

        // Reset deadline on the Race fiber for next iteration (if still in Race)
        for f in &fibers {
            if let WaitState::Race { .. } = &f.wait {
                let mut updated = f.clone();
                if let WaitState::Race {
                    ref mut timer_deadline_ms,
                    ..
                } = updated.wait
                {
                    *timer_deadline_ms = Some(0);
                }
                store.save_fiber(iid, &updated).await.unwrap();
            }
        }
    }

    // Verify 3 BoundaryFired events were emitted
    let events = store.read_events(iid, 0).await.unwrap();
    let boundary_fired_count = events
        .iter()
        .filter(|(_, e)| matches!(e, RuntimeEvent::BoundaryFired { .. }))
        .count();
    assert_eq!(
        boundary_fired_count, 3,
        "Expected 3 BoundaryFired events, got {}",
        boundary_fired_count
    );

    // Verify 3 TimerCycleIteration events
    let iteration_count = events
        .iter()
        .filter(|(_, e)| matches!(e, RuntimeEvent::TimerCycleIteration { .. }))
        .count();
    assert_eq!(
        iteration_count, 3,
        "Expected 3 TimerCycleIteration events, got {}",
        iteration_count
    );
}

// ── T-NI-3: Cycle exhausted → fiber reverts to plain Job wait ──

#[tokio::test]
async fn t_ni_3_cycle_exhausted_reverts_to_job() {
    let (engine, store, iid, job_key, _hash) = setup_ni_race(NI_CYCLE_BPMN).await;

    // Fire all 3 iterations
    for _ in 0..3 {
        engine.tick_instance(iid).await.unwrap();

        // Reset deadline for next tick
        let fibers = store.load_fibers(iid).await.unwrap();
        for f in &fibers {
            if let WaitState::Race { .. } = &f.wait {
                let mut updated = f.clone();
                if let WaitState::Race {
                    ref mut timer_deadline_ms,
                    ..
                } = updated.wait
                {
                    *timer_deadline_ms = Some(0);
                }
                store.save_fiber(iid, &updated).await.unwrap();
            }
        }
    }

    // After 3 fires, the main fiber should revert to Job state (cycle exhausted)
    let fibers = store.load_fibers(iid).await.unwrap();
    let main_has_job = fibers
        .iter()
        .any(|f| matches!(&f.wait, WaitState::Job { job_key: jk } if *jk == job_key));
    assert!(
        main_has_job,
        "After cycle exhaustion, main fiber should revert to Job wait. Got: {:?}",
        fibers.iter().map(|f| &f.wait).collect::<Vec<_>>()
    );

    // Verify TimerCycleExhausted event
    let events = store.read_events(iid, 0).await.unwrap();
    let has_exhausted = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::TimerCycleExhausted { total_fired: 3, .. }));
    assert!(
        has_exhausted,
        "Expected TimerCycleExhausted with total_fired=3"
    );
}

// ── T-NI-4: Job completes before non-interrupting timer → normal resolution ──

#[tokio::test]
async fn t_ni_4_job_completes_before_timer() {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let cr = engine.compile(NI_BOUNDARY_BPMN).await.unwrap();
    let payload = r#"{"case":"ni-job-first"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start("ni_proc", cr.bytecode_version, payload, hash, "corr-ni4")
        .await
        .unwrap();

    // Tick to promote fiber to Race
    engine.tick_instance(iid).await.unwrap();

    let jobs = engine
        .activate_jobs(&["long_work".to_string()], 10)
        .await
        .unwrap();
    assert!(!jobs.is_empty());
    let job_key = jobs[0].job_key.clone();

    // Complete the job BEFORE the timer fires
    let result_payload = r#"{"result":"done"}"#;
    engine
        .complete_job(&job_key, result_payload, hash, BTreeMap::new())
        .await
        .unwrap();

    // Tick to advance past the completed job
    engine.tick_instance(iid).await.unwrap();

    // Run the child tasks if any were spawned
    let remaining_jobs = engine
        .activate_jobs(&["long_work".to_string(), "send_reminder".to_string()], 10)
        .await
        .unwrap();
    for job in &remaining_jobs {
        let _ = engine
            .complete_job(
                &job.job_key,
                r#"{"r":"done"}"#,
                compute_hash(
                    &store
                        .load_instance(iid)
                        .await
                        .unwrap()
                        .unwrap()
                        .domain_payload,
                ),
                BTreeMap::new(),
            )
            .await;
    }

    // Keep ticking to reach completion
    for _ in 0..5 {
        engine.tick_instance(iid).await.unwrap();
    }

    // Instance should eventually complete (job resolved the race via Internal arm)
    let inspection = engine.inspect(iid).await.unwrap();
    assert!(
        matches!(inspection.state, ProcessState::Completed { .. }),
        "Expected Completed after job finishes, got {:?}",
        inspection.state
    );

    // No BoundaryFired events (timer never fired)
    let events = store.read_events(iid, 0).await.unwrap();
    let boundary_fired = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::BoundaryFired { .. }));
    assert!(
        !boundary_fired,
        "BoundaryFired should not have been emitted when job completes first"
    );
}

// ── T-NI-5: Verifier rejects cycle + interrupting=true ──

#[tokio::test]
async fn t_ni_5_verifier_rejects_cycle_interrupting() {
    let bpmn = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="bad_proc" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:serviceTask id="task1" name="Work">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="do_work" />
              </bpmn:extensionElements>
            </bpmn:serviceTask>
            <bpmn:boundaryEvent id="bad_timer" attachedToRef="task1" cancelActivity="true">
              <bpmn:timerEventDefinition>
                <bpmn:timeCycle>R3/PT1H</bpmn:timeCycle>
              </bpmn:timerEventDefinition>
            </bpmn:boundaryEvent>
            <bpmn:endEvent id="end" />
            <bpmn:endEvent id="end2" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
            <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
            <bpmn:sequenceFlow id="f3" sourceRef="bad_timer" targetRef="end2" />
          </bpmn:process>
        </bpmn:definitions>"#;

    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store);

    let result = engine.compile(bpmn).await;
    assert!(result.is_err(), "Should reject cycle + interrupting=true");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("cycle timers must be non-interrupting"),
        "Error should mention cycle + non-interrupting, got: {}",
        err_msg
    );
}

// ── Phase 5.1: Terminate End Event tests ────────────────────────

/// T-TERM-1: Single fiber hits EndTerminate → instance Terminated.
#[tokio::test]
async fn t_term_1_single_fiber_terminate() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = CompiledProgram {
        bytecode_version: [40u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::EndTerminate,
        ],
        debug_map: BTreeMap::from([(0, "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-1")
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 1);

    let payload = "{}";
    let hash = compute_hash(payload);
    engine
        .complete_job(&jobs[0].job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    // Assert: Terminated
    let instance = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(
        matches!(instance.state, ProcessState::Terminated { .. }),
        "Expected Terminated, got {:?}",
        instance.state
    );

    // Assert: no fibers remain
    let fibers = store.load_fibers(instance_id).await.unwrap();
    assert!(fibers.is_empty());

    // Assert: Terminated event
    let events = store.read_events(instance_id, 0).await.unwrap();
    let has_term = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::Terminated { .. }));
    assert!(has_term);
}

/// T-TERM-2: Parallel flow — one branch terminates, other branch killed.
/// Order-independent: handles either fiber executing first after Fork.
#[tokio::test]
async fn t_term_2_parallel_terminate_kills_siblings() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Fork → Branch A (EndTerminate), Branch B (ExecNative → End)
    let program = CompiledProgram {
        bytecode_version: [41u8; 32],
        program: vec![
            Instr::Fork {
                targets: Box::new([1, 2]),
            }, // 0: fork
            Instr::EndTerminate, // 1: Branch A terminates
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 2: Branch B task
            Instr::End,          // 3: Branch B end
        ],
        debug_map: BTreeMap::from([(2, "slow_task".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["slow_task".to_string()],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-2")
        .await
        .unwrap();

    // Tick until instance reaches terminal state.
    for _ in 0..5 {
        engine.tick_instance(instance_id).await.unwrap();
        let inst = store.load_instance(instance_id).await.unwrap().unwrap();
        if inst.state.is_terminal() {
            break;
        }
    }

    // Assert: instance is Terminated (not Completed, not Running)
    let instance = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(
        matches!(instance.state, ProcessState::Terminated { .. }),
        "Expected Terminated, got {:?}",
        instance.state
    );

    // Assert: no fibers remain
    let fibers = store.load_fibers(instance_id).await.unwrap();
    assert!(fibers.is_empty(), "All fibers should be deleted");

    // Assert: Terminated event emitted
    let events = store.read_events(instance_id, 0).await.unwrap();
    let has_term = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::Terminated { .. }));
    assert!(has_term, "Should emit Terminated event");

    // Assert: no jobs for this instance remain
    let jobs = store
        .dequeue_jobs(
            &["slow_task".to_string()],
            100,
            "default",
            "test-worker",
            300_000,
        )
        .await
        .unwrap();
    let instance_jobs: Vec<_> = jobs
        .iter()
        .filter(|j| j.process_instance_id == instance_id)
        .collect();
    assert!(
        instance_jobs.is_empty(),
        "No jobs should remain for terminated instance"
    );
}

/// T-TERM-3: complete_job on Terminated instance → safe via is_terminal() guard.
#[tokio::test]
async fn t_term_3_complete_job_after_terminate() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Single fiber: ExecNative → EndTerminate
    let program = CompiledProgram {
        bytecode_version: [42u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::EndTerminate,
        ],
        debug_map: BTreeMap::from([(0, "task_x".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_x".to_string()],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-3")
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    let job_key = jobs[0].job_key.clone();

    // Complete the job → fiber advances to EndTerminate → instance Terminated
    let payload = "{}";
    let hash = compute_hash(payload);
    engine
        .complete_job(&job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    assert!(matches!(
        store
            .load_instance(instance_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        ProcessState::Terminated { .. }
    ));

    // Now try a SECOND complete_job with the same key (ghost signal)
    // Should be safe — is_terminal() guard catches it
    let result = engine
        .complete_job(&job_key, payload, hash, BTreeMap::new())
        .await;
    assert!(
        result.is_ok(),
        "Late complete_job on Terminated instance should not error"
    );

    // State unchanged
    let instance = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(matches!(instance.state, ProcessState::Terminated { .. }));
}

/// T-TERM-4: Parser + lowering: <terminateEventDefinition> → EndTerminate instruction.
/// NOTE: engine.compile() returns CompileResult. Use store.load_program() to inspect bytecode.
#[tokio::test]
async fn t_term_4_parse_terminate_end_event() {
    let bpmn_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:serviceTask id="task_a" name="Task A">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="task_a"/>
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end_term">
      <bpmn:terminateEventDefinition/>
    </bpmn:endEvent>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task_a"/>
    <bpmn:sequenceFlow id="f2" sourceRef="task_a" targetRef="end_term"/>
  </bpmn:process>
</bpmn:definitions>"#;

    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let compile_result = engine.compile(bpmn_xml).await;
    assert!(
        compile_result.is_ok(),
        "Should compile: {:?}",
        compile_result.err()
    );

    let compiled = compile_result.unwrap();

    // Load the actual program from store to inspect instructions
    let program = store
        .load_program(compiled.bytecode_version)
        .await
        .unwrap()
        .expect("Program should be stored after compile");

    let has_end_terminate = program
        .program
        .iter()
        .any(|i| matches!(i, Instr::EndTerminate));
    assert!(
        has_end_terminate,
        "Program should contain EndTerminate instruction"
    );
}

// ═══════════════════════════════════════════════════════════
//  Phase 5.2: Error boundary routing
// ═══════════════════════════════════════════════════════════

/// T-ERR-1: BusinessRejection with matching error route → fiber routes to escalation.
#[tokio::test]
async fn t_err_1_business_error_routes_to_handler() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Bytecode:
    // 0: ExecNative(sanctions_check)  — parks fiber
    // 1: Jump(4)                      — normal continuation
    // 2: ExecNative(enhanced_review)  — error handler path
    // 3: End                          — error handler end
    // 4: End                          — normal end
    let program = CompiledProgram {
        bytecode_version: [50u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 0
            Instr::Jump { target: 4 }, // 1
            Instr::ExecNative {
                task_type: 1,
                argc: 0,
                retc: 0,
            }, // 2: error handler
            Instr::End,                // 3
            Instr::End,                // 4
        ],
        debug_map: BTreeMap::from([
            (0, "sanctions_check".to_string()),
            (2, "enhanced_review".to_string()),
        ]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["sanctions_check".to_string(), "enhanced_review".to_string()],
        error_route_map: BTreeMap::from([(
            0,
            vec![ErrorRoute {
                error_code: Some("SANCTIONS_HIT".to_string()),
                resume_at: 2,
                boundary_element_id: "catch_sanctions".to_string(),
            }],
        )]),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-1")
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 1);
    let job_key = jobs[0].job_key.clone();

    // Fail with matching error code
    engine
        .fail_job(
            &job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "SANCTIONS_HIT".to_string(),
            },
            "Sanctions screening returned a hit",
        )
        .await
        .unwrap();

    // Assert: ErrorRouted event emitted
    let events = store.read_events(instance_id, 0).await.unwrap();
    let has_routed = events.iter().any(|(_, e)| {
            matches!(e, RuntimeEvent::ErrorRouted { error_code, .. } if error_code == "SANCTIONS_HIT")
        });
    assert!(has_routed, "Should emit ErrorRouted event");

    // Assert: NO incident created
    let has_incident = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::IncidentCreated { .. }));
    assert!(
        !has_incident,
        "Should NOT create incident when error route matches"
    );

    // Assert: instance is still Running (not Failed)
    let instance = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(
        matches!(instance.state, ProcessState::Running),
        "Instance should stay Running after error routing, got {:?}",
        instance.state
    );

    // Assert: fiber was routed to error handler (pc=2)
    let fibers = store.load_fibers(instance_id).await.unwrap();
    let routed_fiber = fibers.iter().find(|f| f.wait == WaitState::Running);
    assert!(
        routed_fiber.is_some(),
        "Fiber should be Running at error handler path"
    );

    // Tick to advance the routed fiber
    engine.tick_instance(instance_id).await.unwrap();

    // Should now have a job for enhanced_review
    let new_jobs = store
        .dequeue_jobs(
            &["enhanced_review".to_string()],
            10,
            "default",
            "test-worker",
            300_000,
        )
        .await
        .unwrap();
    assert!(
        !new_jobs.is_empty(),
        "Should activate enhanced_review job after routing"
    );
}

/// T-ERR-2: BusinessRejection with NO matching route → incident (existing behavior).
#[tokio::test]
async fn t_err_2_unmatched_error_creates_incident() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Same program but error_route_map only catches SANCTIONS_HIT
    let program = CompiledProgram {
        bytecode_version: [51u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::End,
        ],
        debug_map: BTreeMap::from([(0, "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        error_route_map: BTreeMap::from([(
            0,
            vec![ErrorRoute {
                error_code: Some("SANCTIONS_HIT".to_string()),
                resume_at: 99, // doesn't matter, won't be used
                boundary_element_id: "catch_sanctions".to_string(),
            }],
        )]),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-2")
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();

    // Fail with NON-matching error code
    engine
        .fail_job(
            &jobs[0].job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "KYC_EXPIRED".to_string(),
            },
            "KYC expired",
        )
        .await
        .unwrap();

    // Assert: incident created
    let events = store.read_events(instance_id, 0).await.unwrap();
    let has_incident = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::IncidentCreated { .. }));
    assert!(has_incident, "Unmatched error should create incident");

    // Assert: NO ErrorRouted
    let has_routed = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::ErrorRouted { .. }));
    assert!(
        !has_routed,
        "Should NOT emit ErrorRouted for unmatched code"
    );

    // Assert: instance Failed
    let instance = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(matches!(instance.state, ProcessState::Failed { .. }));
}

/// T-ERR-3: Catch-all error route (error_code: None) catches any BusinessRejection.
#[tokio::test]
async fn t_err_3_catch_all_routes_any_business_error() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = CompiledProgram {
        bytecode_version: [52u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 0
            Instr::Jump { target: 3 }, // 1
            Instr::End,                // 2: error handler end
            Instr::End,                // 3: normal end
        ],
        debug_map: BTreeMap::from([(0, "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        error_route_map: BTreeMap::from([(
            0,
            vec![ErrorRoute {
                error_code: None, // catch-all
                resume_at: 2,
                boundary_element_id: "catch_all".to_string(),
            }],
        )]),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-3")
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();

    // Fail with ANY business error — catch-all should match
    engine
        .fail_job(
            &jobs[0].job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "ANYTHING_GOES".to_string(),
            },
            "some error",
        )
        .await
        .unwrap();

    // Assert: routed, not incident
    let events = store.read_events(instance_id, 0).await.unwrap();
    let has_routed = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::ErrorRouted { .. }));
    assert!(has_routed, "Catch-all should route any BusinessRejection");

    let instance = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(matches!(instance.state, ProcessState::Running));
}

/// T-ERR-4: Transient error always creates incident, even with error route present.
#[tokio::test]
async fn t_err_4_transient_error_always_incident() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = CompiledProgram {
        bytecode_version: [53u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::End,
            Instr::End, // error handler (won't be used)
        ],
        debug_map: BTreeMap::from([(0, "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        error_route_map: BTreeMap::from([(
            0,
            vec![ErrorRoute {
                error_code: None, // catch-all
                resume_at: 2,
                boundary_element_id: "catch_all".to_string(),
            }],
        )]),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-4")
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();

    // Fail with Transient — error routes should NOT apply
    engine
        .fail_job(&jobs[0].job_key, ErrorClass::Transient, "timeout")
        .await
        .unwrap();

    // Assert: incident, NOT routed
    let events = store.read_events(instance_id, 0).await.unwrap();
    let has_incident = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::IncidentCreated { .. }));
    assert!(has_incident, "Transient errors must always create incident");

    let has_routed = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::ErrorRouted { .. }));
    assert!(
        !has_routed,
        "Transient errors must NOT trigger error routes"
    );
}

/// T-ERR-5: fail_job on terminated instance → safe via is_terminal() guard.
#[tokio::test]
async fn t_err_5_fail_job_on_terminated_instance() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Single fiber: ExecNative → EndTerminate
    let program = CompiledProgram {
        bytecode_version: [54u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::EndTerminate,
        ],
        debug_map: BTreeMap::from([(0, "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-5")
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    let job_key = jobs[0].job_key.clone();

    // Complete job → EndTerminate → Terminated
    let payload = "{}";
    let hash = compute_hash(payload);
    engine
        .complete_job(&job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    assert!(matches!(
        store
            .load_instance(instance_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        ProcessState::Terminated { .. }
    ));

    // Late fail_job — should be safe
    let result = engine
        .fail_job(
            &job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "LATE".to_string(),
            },
            "late failure",
        )
        .await;
    assert!(
        result.is_ok(),
        "fail_job on terminated instance should not error"
    );

    // Assert: SignalIgnored event
    let events = store.read_events(instance_id, 0).await.unwrap();
    let has_ignored = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::SignalIgnored { .. }));
    assert!(has_ignored, "Should emit SignalIgnored for late fail_job");
}

// ═══════════════════════════════════════════════════════════
//  Phase 5.3: Bounded loops
// ═══════════════════════════════════════════════════════════

/// T-LOOP-1: IncCounter + BrCounterLt retry loop executes exactly N times.
#[tokio::test]
async fn t_loop_1_bounded_retry_executes_n_times() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Simulates: task_a fails → error route → IncCounter → BrCounterLt(limit=3) → retry or end
    // Bytecode:
    // 0: ExecNative(task_a)         — parks fiber
    // 1: Jump(5)                    — normal end (skip error handler)
    // 2: IncCounter(0)              — error handler: bump counter
    // 3: BrCounterLt(0, 3, 0)      — if counter<3, retry task_a
    // 4: End                        — counter exhausted, escalation end
    // 5: End                        — normal end
    let program = CompiledProgram {
        bytecode_version: [60u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 0
            Instr::Jump { target: 5 },           // 1
            Instr::IncCounter { counter_id: 0 }, // 2
            Instr::BrCounterLt {
                counter_id: 0,
                limit: 3,
                target: 0,
            }, // 3
            Instr::End,                          // 4
            Instr::End,                          // 5
        ],
        debug_map: BTreeMap::from([(0, "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        error_route_map: BTreeMap::from([(
            0,
            vec![ErrorRoute {
                error_code: Some("RETRY_ME".to_string()),
                resume_at: 2,
                boundary_element_id: "catch_retry".to_string(),
            }],
        )]),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-1")
        .await
        .unwrap();

    // Iteration 1: activate → fail → error route → IncCounter(counter=1) → BrCounterLt(1<3 → retry)
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 1);
    engine
        .fail_job(
            &jobs[0].job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "RETRY_ME".to_string(),
            },
            "attempt 1",
        )
        .await
        .unwrap();
    // Fiber is Running at addr 2 (IncCounter). Tick to advance through IncCounter → BrCounterLt → back to 0
    engine.tick_instance(instance_id).await.unwrap();
    // Now fiber is at addr 0 again (ExecNative), parks on job
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 1, "Iteration 2 should activate task_a");

    // Iteration 2: fail again
    engine
        .fail_job(
            &jobs[0].job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "RETRY_ME".to_string(),
            },
            "attempt 2",
        )
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 1, "Iteration 3 should activate task_a");

    // Iteration 3: fail one more time → counter=3, BrCounterLt(3<3=false) → fall through to End
    engine
        .fail_job(
            &jobs[0].job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "RETRY_ME".to_string(),
            },
            "attempt 3",
        )
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    // Counter exhausted: fiber fell through to addr 4 (End). Tick to complete.
    engine.tick_instance(instance_id).await.unwrap();

    // Assert: instance completed (via End, not stuck in loop)
    let instance = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(
        matches!(instance.state, ProcessState::Completed { .. }),
        "Expected Completed after counter exhaustion, got {:?}",
        instance.state
    );

    // Assert: counter value is 3
    assert_eq!(instance.counters.get(&0), Some(&3));

    // Assert: 3 ErrorRouted events
    let events = store.read_events(instance_id, 0).await.unwrap();
    let routed_count = events
        .iter()
        .filter(|(_, e)| matches!(e, RuntimeEvent::ErrorRouted { .. }))
        .count();
    assert_eq!(routed_count, 3, "Should have exactly 3 error routes");
}

/// T-LOOP-2: Job keys are unique across loop iterations (loop_epoch in key).
#[tokio::test]
async fn t_loop_2_unique_job_keys_per_iteration() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = CompiledProgram {
        bytecode_version: [61u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 0
            Instr::Jump { target: 5 },           // 1
            Instr::IncCounter { counter_id: 0 }, // 2
            Instr::BrCounterLt {
                counter_id: 0,
                limit: 2,
                target: 0,
            }, // 3
            Instr::End,                          // 4
            Instr::End,                          // 5
        ],
        debug_map: BTreeMap::from([(0, "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        error_route_map: BTreeMap::from([(
            0,
            vec![ErrorRoute {
                error_code: None, // catch-all
                resume_at: 2,
                boundary_element_id: "catch_all".to_string(),
            }],
        )]),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-2")
        .await
        .unwrap();

    let mut all_job_keys = Vec::new();

    // Iteration 1
    let jobs = engine.run_instance(instance_id).await.unwrap();
    all_job_keys.push(jobs[0].job_key.clone());
    engine
        .fail_job(
            &jobs[0].job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "ERR".to_string(),
            },
            "err",
        )
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    // Iteration 2
    let jobs = engine.run_instance(instance_id).await.unwrap();
    all_job_keys.push(jobs[0].job_key.clone());

    // Assert: job keys are different despite same PC
    assert_ne!(
        all_job_keys[0], all_job_keys[1],
        "Job keys must differ across iterations: {:?}",
        all_job_keys
    );

    // Both keys should end with different epochs
    assert!(
        all_job_keys[0].ends_with(":0"),
        "First key epoch 0: {}",
        all_job_keys[0]
    );
    assert!(
        all_job_keys[1].ends_with(":1"),
        "Second key epoch 1: {}",
        all_job_keys[1]
    );
}

/// T-LOOP-3: BrCounterLt with counter=0 (never incremented) → always branches if limit>0.
#[tokio::test]
async fn t_loop_3_counter_starts_at_zero() {
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let vm = Vm::new(store.clone());

    let program = CompiledProgram {
        bytecode_version: [62u8; 32],
        program: vec![
            Instr::BrCounterLt {
                counter_id: 5,
                limit: 1,
                target: 2,
            }, // 0: counter=0 < 1 → jump to 2
            Instr::Fail { code: 99 }, // 1: unreachable
            Instr::End,               // 2: landed here
        ],
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let mut instance = ProcessInstance {
        instance_id: Uuid::now_v7(),
        process_key: "test".to_string(),
        bytecode_version: program.bytecode_version,
        tenant_id: "default".to_string(),
        domain_payload: "{}".to_string().into(),
        domain_payload_hash: [0u8; 32],
        session_stack: SessionStackState::default(),
        flags: BTreeMap::new(),
        counters: BTreeMap::new(),
        join_expected: BTreeMap::new(),
        state: ProcessState::Running,
        correlation_id: "corr".to_string(),
        entry_id: Uuid::new_v4(),
        runbook_id: Uuid::new_v4(),
        created_at: 0,
    };
    store.save_instance(&instance).await.unwrap();

    let mut fiber = Fiber::new(Uuid::now_v7(), 0);
    store
        .save_fiber(instance.instance_id, &fiber)
        .await
        .unwrap();

    let outcome = vm
        .run_fiber(&mut fiber, &mut instance, &program, 100)
        .await
        .unwrap();

    // Should have jumped to 2 (End) and ended
    assert!(
        matches!(outcome, TickOutcome::Ended),
        "Counter 5 starts at 0, 0 < 1 should branch to End. Got: {:?}",
        outcome
    );
}

/// T-LOOP-4: Bytecode verifier rejects unguarded backward Jump.
#[tokio::test]
async fn t_loop_4_verifier_rejects_backward_jump() {
    let program = CompiledProgram {
        bytecode_version: [63u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 0
            Instr::Jump { target: 0 }, // 1: backward jump! infinite loop
            Instr::End,                // 2: unreachable
        ],
        debug_map: BTreeMap::from([(0, "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };

    let errors = bpmn_lite_compiler::verifier::verify_bytecode(&program);
    assert!(!errors.is_empty(), "Should reject backward Jump");
    assert!(
        errors[0].message.contains("Backward jump"),
        "Error should mention backward jump: {}",
        errors[0].message
    );
}

/// T-LOOP-5: Bytecode verifier allows BrCounterLt backward jump.
#[tokio::test]
async fn t_loop_5_verifier_allows_br_counter_lt_backward() {
    let program = CompiledProgram {
        bytecode_version: [64u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 0
            Instr::IncCounter { counter_id: 0 }, // 1
            Instr::BrCounterLt {
                counter_id: 0,
                limit: 3,
                target: 0,
            }, // 2: backward, but bounded
            Instr::End,                          // 3
        ],
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };

    let errors = bpmn_lite_compiler::verifier::verify_bytecode(&program);
    assert!(
        errors.is_empty(),
        "BrCounterLt backward should be allowed, got errors: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ═══════════════════════════════════════════════════════════
//  Phase 5A: Inclusive gateway
// ═══════════════════════════════════════════════════════════

/// T-IG-1: All conditions truthy → all branches run → join waits for all → completes.
#[tokio::test]
async fn t_ig_1_all_branches_taken() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = CompiledProgram {
        bytecode_version: [70u8; 32],
        program: vec![
            Instr::ForkInclusive {
                branches: Box::new([
                    InclusiveBranch {
                        condition_flag: None,
                        target: 2,
                    },
                    InclusiveBranch {
                        condition_flag: Some(0),
                        target: 4,
                    },
                    InclusiveBranch {
                        condition_flag: Some(1),
                        target: 6,
                    },
                ]),
                join_id: 0,
                default_target: None,
            },
            Instr::End, // 1: placeholder
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 2: identity_check
            Instr::JoinDynamic { id: 0, next: 8 }, // 3
            Instr::ExecNative {
                task_type: 1,
                argc: 0,
                retc: 0,
            }, // 4: edd_check
            Instr::JoinDynamic { id: 0, next: 8 }, // 5
            Instr::ExecNative {
                task_type: 2,
                argc: 0,
                retc: 0,
            }, // 6: pep_screening
            Instr::JoinDynamic { id: 0, next: 8 }, // 7
            Instr::End, // 8: done
        ],
        debug_map: BTreeMap::from([
            (2, "identity_check".to_string()),
            (4, "edd_check".to_string()),
            (6, "pep_screening".to_string()),
        ]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![
            "identity_check".to_string(),
            "edd_check".to_string(),
            "pep_screening".to_string(),
        ],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    // Start with both flags true → all 3 branches taken
    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-1")
        .await
        .unwrap();

    // Set flags before first tick
    let mut inst = store.load_instance(instance_id).await.unwrap().unwrap();
    inst.flags.insert(0, Value::Bool(true)); // high_risk
    inst.flags.insert(1, Value::Bool(true)); // pep_flagged
    store.save_instance(&inst).await.unwrap();

    // Tick → ForkInclusive evaluates: all 3 taken → 3 fibers spawned
    engine.tick_instance(instance_id).await.unwrap();

    // Assert: InclusiveForkTaken event with expected=3
    let events = store.read_events(instance_id, 0).await.unwrap();
    let fork_event = events
        .iter()
        .find(|(_, e)| matches!(e, RuntimeEvent::InclusiveForkTaken { .. }));
    assert!(fork_event.is_some(), "Should emit InclusiveForkTaken");

    // Assert: join_expected[0] = 3
    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert_eq!(inst.join_expected.get(&0), Some(&3));

    // Run → 3 jobs activated
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 3, "All 3 branches should activate jobs");

    // Complete all 3 jobs
    for job in &jobs {
        let payload = "{}";
        let hash = bpmn_lite_vm::compute_hash(payload);
        engine
            .complete_job(&job.job_key, payload, hash, BTreeMap::new())
            .await
            .unwrap();
    }

    // Tick until complete
    for _ in 0..5 {
        engine.tick_instance(instance_id).await.unwrap();
        let inst = store.load_instance(instance_id).await.unwrap().unwrap();
        if inst.state.is_terminal() {
            break;
        }
    }

    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "Expected Completed, got {:?}",
        inst.state
    );
}

/// T-IG-2: Only 1 of 3 conditions truthy → 1 branch runs → join waits for 1 → immediate release.
#[tokio::test]
async fn t_ig_2_single_branch_taken() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = CompiledProgram {
        bytecode_version: [71u8; 32],
        program: vec![
            Instr::ForkInclusive {
                branches: Box::new([
                    InclusiveBranch {
                        condition_flag: None,
                        target: 2,
                    },
                    InclusiveBranch {
                        condition_flag: Some(0),
                        target: 4,
                    },
                    InclusiveBranch {
                        condition_flag: Some(1),
                        target: 6,
                    },
                ]),
                join_id: 0,
                default_target: None,
            },
            Instr::End,
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::JoinDynamic { id: 0, next: 8 },
            Instr::ExecNative {
                task_type: 1,
                argc: 0,
                retc: 0,
            },
            Instr::JoinDynamic { id: 0, next: 8 },
            Instr::ExecNative {
                task_type: 2,
                argc: 0,
                retc: 0,
            },
            Instr::JoinDynamic { id: 0, next: 8 },
            Instr::End,
        ],
        debug_map: BTreeMap::from([
            (2, "identity_check".to_string()),
            (4, "edd_check".to_string()),
            (6, "pep_screening".to_string()),
        ]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![
            "identity_check".to_string(),
            "edd_check".to_string(),
            "pep_screening".to_string(),
        ],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    // Start with flags FALSE → only unconditional branch (A) taken
    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-2")
        .await
        .unwrap();

    // Tick → ForkInclusive: only branch A taken → 1 fiber
    engine.tick_instance(instance_id).await.unwrap();

    // Assert: join_expected[0] = 1
    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert_eq!(inst.join_expected.get(&0), Some(&1));

    // Run → 1 job
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 1, "Only unconditional branch should spawn");

    // Complete job → JoinDynamic expects 1, 1 arrives → immediate release
    let payload = "{}";
    let hash = bpmn_lite_vm::compute_hash(payload);
    engine
        .complete_job(&jobs[0].job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();

    for _ in 0..5 {
        engine.tick_instance(instance_id).await.unwrap();
        let inst = store.load_instance(instance_id).await.unwrap().unwrap();
        if inst.state.is_terminal() {
            break;
        }
    }

    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(matches!(inst.state, ProcessState::Completed { .. }));
}

/// T-IG-3: Zero conditions match, no default → incident.
#[tokio::test]
async fn t_ig_3_zero_match_no_default_incident() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // ALL branches conditional — no unconditional
    let program = CompiledProgram {
        bytecode_version: [72u8; 32],
        program: vec![
            Instr::ForkInclusive {
                branches: Box::new([
                    InclusiveBranch {
                        condition_flag: Some(0),
                        target: 2,
                    },
                    InclusiveBranch {
                        condition_flag: Some(1),
                        target: 4,
                    },
                ]),
                join_id: 0,
                default_target: None, // no default!
            },
            Instr::End,
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::JoinDynamic { id: 0, next: 6 },
            Instr::ExecNative {
                task_type: 1,
                argc: 0,
                retc: 0,
            },
            Instr::JoinDynamic { id: 0, next: 6 },
            Instr::End,
        ],
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string(), "task_b".to_string()],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-3")
        .await
        .unwrap();
    // No flags set → all conditions false → zero match

    engine.tick_instance(instance_id).await.unwrap();

    // Assert: instance Failed with incident
    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(
        matches!(inst.state, ProcessState::Failed { .. }),
        "Zero match with no default should create incident, got {:?}",
        inst.state
    );

    let events = store.read_events(instance_id, 0).await.unwrap();
    let has_incident = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::IncidentCreated { .. }));
    assert!(has_incident, "Should emit IncidentCreated");
}

/// T-IG-4: Zero conditions match WITH default → default branch runs.
#[tokio::test]
async fn t_ig_4_zero_match_with_default() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = CompiledProgram {
        bytecode_version: [73u8; 32],
        program: vec![
            Instr::ForkInclusive {
                branches: Box::new([InclusiveBranch {
                    condition_flag: Some(0),
                    target: 2,
                }]),
                join_id: 0,
                default_target: Some(4), // default branch
            },
            Instr::End,
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 2: conditional
            Instr::JoinDynamic { id: 0, next: 6 },
            Instr::ExecNative {
                task_type: 1,
                argc: 0,
                retc: 0,
            }, // 4: default
            Instr::JoinDynamic { id: 0, next: 6 },
            Instr::End, // 6: done
        ],
        debug_map: BTreeMap::from([
            (2, "conditional_task".to_string()),
            (4, "default_task".to_string()),
        ]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["conditional_task".to_string(), "default_task".to_string()],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-4")
        .await
        .unwrap();
    // No flags → condition false → default taken

    engine.tick_instance(instance_id).await.unwrap();

    // Assert: join_expected = 1 (default branch only)
    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert_eq!(inst.join_expected.get(&0), Some(&1));

    // Run → should get default_task job
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 1);

    // Complete and finish
    let payload = "{}";
    let hash = bpmn_lite_vm::compute_hash(payload);
    engine
        .complete_job(&jobs[0].job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();
    for _ in 0..5 {
        engine.tick_instance(instance_id).await.unwrap();
        let inst = store.load_instance(instance_id).await.unwrap().unwrap();
        if inst.state.is_terminal() {
            break;
        }
    }

    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(matches!(inst.state, ProcessState::Completed { .. }));
}

/// T-IG-5: JoinDynamic releases only after dynamic expected count arrivals.
#[tokio::test]
async fn t_ig_5_join_waits_for_dynamic_count() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // 2 of 3 branches taken → join waits for exactly 2
    let program = CompiledProgram {
        bytecode_version: [74u8; 32],
        program: vec![
            Instr::ForkInclusive {
                branches: Box::new([
                    InclusiveBranch {
                        condition_flag: None,
                        target: 2,
                    },
                    InclusiveBranch {
                        condition_flag: Some(0),
                        target: 4,
                    },
                    InclusiveBranch {
                        condition_flag: Some(1),
                        target: 6,
                    },
                ]),
                join_id: 0,
                default_target: None,
            },
            Instr::End,
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::JoinDynamic { id: 0, next: 8 },
            Instr::ExecNative {
                task_type: 1,
                argc: 0,
                retc: 0,
            },
            Instr::JoinDynamic { id: 0, next: 8 },
            Instr::ExecNative {
                task_type: 2,
                argc: 0,
                retc: 0,
            },
            Instr::JoinDynamic { id: 0, next: 8 },
            Instr::End,
        ],
        debug_map: BTreeMap::from([
            (2, "task_a".to_string()),
            (4, "task_b".to_string()),
            (6, "task_c".to_string()),
        ]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![
            "task_a".to_string(),
            "task_b".to_string(),
            "task_c".to_string(),
        ],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version, &program)
        .await
        .unwrap();

    let instance_id = engine
        .start("test", program.bytecode_version, "{}", [0u8; 32], "corr-5")
        .await
        .unwrap();

    // Set flag_0=true, flag_1=false → 2 branches taken (unconditional + flag_0)
    let mut inst = store.load_instance(instance_id).await.unwrap().unwrap();
    inst.flags.insert(0, Value::Bool(true));
    // flag 1 not set = false
    store.save_instance(&inst).await.unwrap();

    engine.tick_instance(instance_id).await.unwrap();

    assert_eq!(
        store
            .load_instance(instance_id)
            .await
            .unwrap()
            .unwrap()
            .join_expected
            .get(&0),
        Some(&2)
    );

    // Run → 2 jobs
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 2, "Should have 2 jobs (branches A and B)");

    // Complete first job → join has 1/2, should NOT release yet
    let payload = "{}";
    let hash = bpmn_lite_vm::compute_hash(payload);
    engine
        .complete_job(&jobs[0].job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    // Instance still Running (waiting for 2nd branch)
    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(
        matches!(inst.state, ProcessState::Running),
        "Should still be Running, got {:?}",
        inst.state
    );

    // Complete second job → join has 2/2, releases
    engine
        .complete_job(&jobs[1].job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();
    for _ in 0..5 {
        engine.tick_instance(instance_id).await.unwrap();
        let inst = store.load_instance(instance_id).await.unwrap().unwrap();
        if inst.state.is_terminal() {
            break;
        }
    }

    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(matches!(inst.state, ProcessState::Completed { .. }));
}

/// T-IG-6: Full compiler pipeline — parse inclusiveGateway from BPMN XML.
#[tokio::test]
async fn t_ig_6_parse_inclusive_gateway() {
    let bpmn_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:serviceTask id="prep" name="Prep">
      <bpmn:extensionElements><zeebe:taskDefinition type="prep"/></bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:inclusiveGateway id="ig_fork" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_a" name="Identity Check">
      <bpmn:extensionElements><zeebe:taskDefinition type="identity_check"/></bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task_b" name="EDD Check">
      <bpmn:extensionElements><zeebe:taskDefinition type="edd_check"/></bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:inclusiveGateway id="ig_join" gatewayDirection="Converging"/>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="prep"/>
    <bpmn:sequenceFlow id="f2" sourceRef="prep" targetRef="ig_fork"/>
    <bpmn:sequenceFlow id="f3" sourceRef="ig_fork" targetRef="task_a"/>
    <bpmn:sequenceFlow id="f4" sourceRef="ig_fork" targetRef="task_b">
      <bpmn:conditionExpression>= high_risk == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f5" sourceRef="task_a" targetRef="ig_join"/>
    <bpmn:sequenceFlow id="f6" sourceRef="task_b" targetRef="ig_join"/>
    <bpmn:sequenceFlow id="f7" sourceRef="ig_join" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#;

    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let result = engine.compile(bpmn_xml).await;
    assert!(
        result.is_ok(),
        "Should compile inclusive gateway BPMN: {:?}",
        result.err()
    );

    let compiled = result.unwrap();
    let program = store
        .load_program(compiled.bytecode_version)
        .await
        .unwrap()
        .unwrap();

    // Should contain ForkInclusive and JoinDynamic instructions
    let has_fork_inclusive = program
        .program
        .iter()
        .any(|i| matches!(i, Instr::ForkInclusive { .. }));
    assert!(
        has_fork_inclusive,
        "Should contain ForkInclusive instruction"
    );

    let has_join_dynamic = program
        .program
        .iter()
        .any(|i| matches!(i, Instr::JoinDynamic { .. }));
    assert!(has_join_dynamic, "Should contain JoinDynamic instruction");
}

// ═══════════════════════════════════════════════════════════
//  Authoring Phase A: YAML → DTO → IR → Bytecode → Execute
// ═══════════════════════════════════════════════════════════

/// T-AUTH-1: Basic sequence: start → task_a → task_b → end.
/// YAML compile + execute to Completed.
#[tokio::test]
async fn t_auth_1_basic_sequence_yaml() {
    let yaml = r#"
id: basic-seq
nodes:
  - kind: Start
    id: start
  - kind: ServiceTask
    id: task_a
    task_type: do_a
  - kind: ServiceTask
    id: task_b
    task_type: do_b
  - kind: End
    id: end
edges:
  - from: start
    to: task_a
  - from: task_a
    to: task_b
  - from: task_b
    to: end
"#;
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Compile from YAML
    let program = bpmn_lite_authoring::publish::compile_program_from_yaml(yaml).unwrap();
    let cr = engine.store_compiled_program(program).await.unwrap();
    assert!(cr.task_types.contains(&"do_a".to_string()));
    assert!(cr.task_types.contains(&"do_b".to_string()));

    // Start instance
    let payload = r#"{"test":"auth1"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start(
            "basic-seq",
            cr.bytecode_version,
            payload,
            hash,
            "corr-auth1",
        )
        .await
        .unwrap();

    // Tick → task_a job
    let jobs = engine.run_instance(iid).await.unwrap();
    let extra = engine
        .activate_jobs(&["do_a".to_string()], 10)
        .await
        .unwrap();
    let all_jobs: Vec<_> = jobs.into_iter().chain(extra).collect();
    assert!(!all_jobs.is_empty(), "Should have do_a job");

    // Complete task_a
    engine
        .complete_job(
            &all_jobs[0].job_key,
            r#"{"a":"done"}"#,
            hash,
            BTreeMap::new(),
        )
        .await
        .unwrap();

    // Tick → task_b job
    engine.tick_instance(iid).await.unwrap();
    let jobs_b = engine
        .activate_jobs(&["do_b".to_string()], 10)
        .await
        .unwrap();
    assert!(!jobs_b.is_empty(), "Should have do_b job");

    // Complete task_b — hash must match instance's current domain_payload
    // (updated to task_a's completion payload after first complete_job)
    let hash_b = compute_hash(r#"{"a":"done"}"#);
    engine
        .complete_job(
            &jobs_b[0].job_key,
            r#"{"b":"done"}"#,
            hash_b,
            BTreeMap::new(),
        )
        .await
        .unwrap();

    // Tick → Completed
    engine.tick_instance(iid).await.unwrap();
    let inspection = engine.inspect(iid).await.unwrap();
    assert!(
        matches!(inspection.state, ProcessState::Completed { .. }),
        "Expected Completed, got {:?}",
        inspection.state
    );
}

/// T-AUTH-2: Inclusive gateway round-trip from YAML.
/// Unconditional + 2 conditional branches, set 1 flag true → 2 branches taken.
#[tokio::test]
async fn t_auth_2_inclusive_gateway_yaml() {
    use bpmn_lite_authoring::dto::*;
    use bpmn_lite_compiler::ir::GatewayDirection;

    let dto = WorkflowGraphDto {
        id: "inclusive-test".to_string(),
        meta: None,
        nodes: vec![
            NodeDto::Start {
                id: "start".to_string(),
            },
            NodeDto::InclusiveGateway {
                id: "ig_fork".to_string(),
                direction: GatewayDirection::Diverging,
                join: Some("ig_join".to_string()),
            },
            NodeDto::ServiceTask {
                id: "always".to_string(),
                task_type: "always_task".to_string(),
                bpmn_id: None,
            },
            NodeDto::ServiceTask {
                id: "branch_a".to_string(),
                task_type: "branch_a_task".to_string(),
                bpmn_id: None,
            },
            NodeDto::ServiceTask {
                id: "branch_b".to_string(),
                task_type: "branch_b_task".to_string(),
                bpmn_id: None,
            },
            NodeDto::InclusiveGateway {
                id: "ig_join".to_string(),
                direction: GatewayDirection::Converging,
                join: None,
            },
            NodeDto::End {
                id: "end".to_string(),
                terminate: false,
            },
        ],
        edges: vec![
            EdgeDto {
                from: "start".to_string(),
                to: "ig_fork".to_string(),
                condition: None,
                is_default: false,
                on_error: None,
            },
            // Unconditional branch (always taken)
            EdgeDto {
                from: "ig_fork".to_string(),
                to: "always".to_string(),
                condition: None,
                is_default: false,
                on_error: None,
            },
            // Conditional: flag_a == true
            EdgeDto {
                from: "ig_fork".to_string(),
                to: "branch_a".to_string(),
                condition: Some(FlagCondition {
                    flag: "flag_a".to_string(),
                    op: FlagOp::Eq,
                    value: FlagValue::Bool(true),
                }),
                is_default: false,
                on_error: None,
            },
            // Conditional: flag_b == true
            EdgeDto {
                from: "ig_fork".to_string(),
                to: "branch_b".to_string(),
                condition: Some(FlagCondition {
                    flag: "flag_b".to_string(),
                    op: FlagOp::Eq,
                    value: FlagValue::Bool(true),
                }),
                is_default: false,
                on_error: None,
            },
            EdgeDto {
                from: "always".to_string(),
                to: "ig_join".to_string(),
                condition: None,
                is_default: false,
                on_error: None,
            },
            EdgeDto {
                from: "branch_a".to_string(),
                to: "ig_join".to_string(),
                condition: None,
                is_default: false,
                on_error: None,
            },
            EdgeDto {
                from: "branch_b".to_string(),
                to: "ig_join".to_string(),
                condition: None,
                is_default: false,
                on_error: None,
            },
            EdgeDto {
                from: "ig_join".to_string(),
                to: "end".to_string(),
                condition: None,
                is_default: false,
                on_error: None,
            },
        ],
    };

    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = bpmn_lite_authoring::publish::compile_program_from_dto(&dto).unwrap();
    let cr = engine.store_compiled_program(program).await.unwrap();

    // Start with flag_a=true, flag_b=false
    let payload = r#"{"test":"ig"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start(
            "inclusive-test",
            cr.bytecode_version,
            payload,
            hash,
            "corr-ig",
        )
        .await
        .unwrap();

    // Flag names are interned as sequential u32 keys during lowering.
    // flag_a is first interned → key 0, flag_b → key 1.
    // Set flag key 0 (flag_a) = true before tick
    {
        let mut inst = store.load_instance(iid).await.unwrap().unwrap();
        inst.flags.insert(0, Value::Bool(true));
        // flag_b (key 1) not set = defaults to false
        store.save_instance(&inst).await.unwrap();
    }

    // Tick — ForkInclusive should spawn 2 fibers (unconditional + flag_a)
    engine.tick_instance(iid).await.unwrap();

    let inspection = engine.inspect(iid).await.unwrap();
    assert_eq!(inspection.state, ProcessState::Running);
    // Should have at least 2 fibers for the 2 branches
    assert!(
        inspection.fibers.len() >= 2,
        "Expected >=2 fibers for inclusive fork, got {}",
        inspection.fibers.len()
    );
}

/// T-AUTH-3: RaceWait deferred to Phase B.
#[tokio::test]
#[ignore = "RaceWait not supported in Phase A — deferred to Phase B"]
async fn t_auth_3_race_wait() {
    // Placeholder — RaceWait DTO nodes are rejected by dto_to_ir in Phase A
}

/// T-AUTH-4: Error routing from YAML — ServiceTask with on_error edge.
/// Fail job → routes to escalation.
#[tokio::test]
async fn t_auth_4_error_routing_yaml() {
    let yaml = r#"
id: error-route
nodes:
  - kind: Start
    id: start
  - kind: ServiceTask
    id: risky_task
    task_type: risky_work
  - kind: ServiceTask
    id: escalation
    task_type: handle_escalation
  - kind: End
    id: end_normal
  - kind: End
    id: end_error
edges:
  - from: start
    to: risky_task
  - from: risky_task
    to: end_normal
  - from: risky_task
    to: escalation
    on_error:
      error_code: BIZ_FAIL
      retries: 0
  - from: escalation
    to: end_error
"#;
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = bpmn_lite_authoring::publish::compile_program_from_yaml(yaml).unwrap();
    let cr = engine.store_compiled_program(program).await.unwrap();

    let payload = r#"{"test":"err"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start(
            "error-route",
            cr.bytecode_version,
            payload,
            hash,
            "corr-err",
        )
        .await
        .unwrap();

    // Tick → risky_task job
    let jobs = engine.run_instance(iid).await.unwrap();
    let extra = engine
        .activate_jobs(&["risky_work".to_string()], 10)
        .await
        .unwrap();
    let all_jobs: Vec<_> = jobs.into_iter().chain(extra).collect();
    assert!(!all_jobs.is_empty());

    // Fail with matching error code
    engine
        .fail_job(
            &all_jobs[0].job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "BIZ_FAIL".to_string(),
            },
            "Business failure",
        )
        .await
        .unwrap();

    // Verify error was routed (not incident)
    let events = store.read_events(iid, 0).await.unwrap();
    let has_routed = events.iter().any(|(_, e)| {
            matches!(e, RuntimeEvent::ErrorRouted { error_code, .. } if error_code == "BIZ_FAIL")
        });
    assert!(has_routed, "Should route error to escalation handler");

    // Instance should still be Running (not Failed)
    let inst = store.load_instance(iid).await.unwrap().unwrap();
    assert!(
        matches!(inst.state, ProcessState::Running),
        "Instance should be Running after error routing, got {:?}",
        inst.state
    );

    // Tick to advance to escalation handler
    engine.tick_instance(iid).await.unwrap();
    let esc_jobs = engine
        .activate_jobs(&["handle_escalation".to_string()], 10)
        .await
        .unwrap();
    assert!(
        !esc_jobs.is_empty(),
        "Should have handle_escalation job after error routing"
    );
}

/// T-AUTH-5: XOR with is_default=true edge. Condition false → default path.
#[tokio::test]
async fn t_auth_5_xor_default_yaml() {
    let yaml = r#"
id: xor-default
nodes:
  - kind: Start
    id: start
  - kind: ExclusiveGateway
    id: decision
  - kind: ServiceTask
    id: approved_path
    task_type: do_approved
  - kind: ServiceTask
    id: fallback_path
    task_type: do_fallback
  - kind: End
    id: end
edges:
  - from: start
    to: decision
  - from: decision
    to: approved_path
    condition:
      flag: approved
      op: "=="
      value: true
  - from: decision
    to: fallback_path
    is_default: true
  - from: approved_path
    to: end
  - from: fallback_path
    to: end
"#;
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = bpmn_lite_authoring::publish::compile_program_from_yaml(yaml).unwrap();
    let cr = engine.store_compiled_program(program).await.unwrap();

    let payload = r#"{"test":"xor"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start(
            "xor-default",
            cr.bytecode_version,
            payload,
            hash,
            "corr-xor",
        )
        .await
        .unwrap();

    // Do NOT set "approved" flag → condition is false → default path
    // Tick to advance through XOR
    engine.tick_instance(iid).await.unwrap();

    // Should get do_fallback job (not do_approved)
    let jobs_fallback = engine
        .activate_jobs(&["do_fallback".to_string()], 10)
        .await
        .unwrap();
    let jobs_approved = engine
        .activate_jobs(&["do_approved".to_string()], 10)
        .await
        .unwrap();

    assert!(
        !jobs_fallback.is_empty(),
        "Default path (do_fallback) should be taken"
    );
    assert!(
        jobs_approved.is_empty(),
        "Conditional path (do_approved) should NOT be taken"
    );

    // Complete fallback → end
    engine
        .complete_job(
            &jobs_fallback[0].job_key,
            r#"{"r":"fb"}"#,
            hash,
            BTreeMap::new(),
        )
        .await
        .unwrap();
    engine.tick_instance(iid).await.unwrap();

    let inspection = engine.inspect(iid).await.unwrap();
    assert!(
        matches!(inspection.state, ProcessState::Completed { .. }),
        "Expected Completed, got {:?}",
        inspection.state
    );
}

/// T-AUTH-6: Boundary timer from YAML DTO.
#[tokio::test]
#[ignore = "BoundaryTimer time simulation requires deadline manipulation — covered by BPMN XML tests"]
async fn t_auth_6_boundary_timer_yaml() {
    // BoundaryTimer from DTO compiles correctly (verified in dto_to_ir tests).
    // Runtime behavior (timer firing, race resolution) is covered by
    // the existing BPMN XML boundary timer tests (T-NI-*).
}
