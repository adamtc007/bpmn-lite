use super::*;
use bpmn_lite_store::store::WorkflowStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_store::{ArtifactRepository as _, JournalReader as _, RuntimeStore as _};
use bpmn_lite_types::session_stack::SessionStackState;
use bpmn_lite_types::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const FAR_FUTURE_TIMER_MS: u64 = 4_070_908_800_000;

const ORDINARY_TIMER_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="ordinary_timer" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:intermediateCatchEvent id="wait_two_seconds">
      <bpmn:timerEventDefinition>
        <bpmn:timeDuration>PT2S</bpmn:timeDuration>
      </bpmn:timerEventDefinition>
    </bpmn:intermediateCatchEvent>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="wait_two_seconds" />
    <bpmn:sequenceFlow id="f2" sourceRef="wait_two_seconds" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

const ABSOLUTE_TIMER_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="absolute_timer" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:intermediateCatchEvent id="wait_until">
      <bpmn:timerEventDefinition>
        <bpmn:timeDate>4070908800000</bpmn:timeDate>
      </bpmn:timerEventDefinition>
    </bpmn:intermediateCatchEvent>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="wait_until" />
    <bpmn:sequenceFlow id="f2" sourceRef="wait_until" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

#[tokio::test]
async fn durable_wait_for_two_seconds_resumes_via_scheduler() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());
    let compiled = engine.compile(ORDINARY_TIMER_BPMN).await.unwrap();
    let payload = "{}";
    let instance_id = engine
        .start(
            "ordinary_timer",
            compiled.bytecode_version,
            payload,
            bpmn_lite_types::EffectId::content_hash((payload).as_bytes()),
            "timer-wait-for",
        )
        .await
        .unwrap();

    engine.tick_instance(instance_id).await.unwrap();
    assert!(matches!(
        store
            .load_fibers(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id
            )
            .await
            .unwrap()[0]
            .wait,
        WaitState::Timer { .. }
    ));

    tokio::time::sleep(std::time::Duration::from_millis(2_050)).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert_eq!(
        engine
            .tick_due_timers("timer-test", now, 10, 30_000)
            .await
            .unwrap(),
        1
    );
    engine.tick_instance(instance_id).await.unwrap();
    assert!(matches!(
        engine.inspect(instance_id).await.unwrap().state,
        ProcessState::Completed { .. }
    ));
}

#[tokio::test]
async fn durable_wait_until_duplicate_delivery_is_typed_noop() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());
    let compiled = engine.compile(ABSOLUTE_TIMER_BPMN).await.unwrap();
    let instance_id = engine
        .start(
            "absolute_timer",
            compiled.bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "timer-wait-until",
        )
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    let timer = store
        .claim_due_timers(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            "timer-test",
            FAR_FUTURE_TIMER_MS,
            1,
            30_000,
        )
        .await
        .unwrap()
        .pop()
        .unwrap();
    let command = Command::TimerFired {
        timer,
        fired_at: FAR_FUTURE_TIMER_MS,
    };
    assert_eq!(
        engine
            .apply_timer_command(command.clone(), "timer-test")
            .await
            .unwrap(),
        TimerFireOutcome::Applied
    );
    assert_eq!(
        engine
            .apply_timer_command(command, "timer-test")
            .await
            .unwrap(),
        TimerFireOutcome::AlreadyConsumed
    );
}

/// Integration test: compile → start → run → activate jobs → complete → verify completion
#[tokio::test]
async fn test_engine_full_lifecycle() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
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
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
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

/// R4 (baseline review F3): a start command whose `initial_payload` is not
/// valid JSON must be REJECTED at admission — previously it was silently
/// accepted (`.ok().filter(is_object)`) and only failed later at Ring 2
/// frame-hash time, an integrity error deferred from the one place it could
/// have been a clean typed rejection.
#[tokio::test]
async fn start_with_malformed_payload_fails_closed_at_admission() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());
    let bpmn = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="reject_proc" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:endEvent id="end" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="end" />
          </bpmn:process>
        </bpmn:definitions>"#;
    let compile_result = engine.compile(bpmn).await.unwrap();

    let payload = "not json at all {";
    let result = engine
        .start_with_params(StartParams {
            process_key: "reject_proc".to_string(),
            bytecode_version: compile_result.bytecode_version,
            domain_payload: payload.to_string(),
            domain_payload_hash: bpmn_lite_types::EffectId::content_hash((payload).as_bytes()),
            correlation_id: "corr-reject".to_string(),
            session_stack: SessionStackState::default(),
            entry_id: Uuid::new_v4(),
            runbook_id: Uuid::new_v4(),
            expected_preconditions: None,
        })
        .await;
    let error = result.expect_err("malformed initial_payload must be rejected at admission");
    assert!(
        error.to_string().contains("not valid JSON"),
        "rejection must name the defect, got: {error}"
    );

    // Green counterpart: VALID but non-object JSON is legal — object-ness
    // gates only placeholder extraction, never admission.
    let payload = r#"[1, 2, 3]"#;
    engine
        .start_with_params(StartParams {
            process_key: "reject_proc".to_string(),
            bytecode_version: compile_result.bytecode_version,
            domain_payload: payload.to_string(),
            domain_payload_hash: bpmn_lite_types::EffectId::content_hash((payload).as_bytes()),
            correlation_id: "corr-array".to_string(),
            session_stack: SessionStackState::default(),
            entry_id: Uuid::new_v4(),
            runbook_id: Uuid::new_v4(),
            expected_preconditions: None,
        })
        .await
        .expect("valid non-object JSON payload must be admitted");
}

#[tokio::test]
async fn test_start_with_session_stack_copies_value() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
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
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let original_scope_id = Uuid::new_v4();
    let mutated_scope_id = Uuid::new_v4();

    let mut session_stack = SessionStackState {
        session_id: Uuid::new_v4(),
        scope: Some(bpmn_lite_types::session_stack::SessionScopeState {
            client_group_id: original_scope_id,
            client_group_name: Some("Original".to_string()),
        }),
        active_workspace: Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Kyc),
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
            expected_preconditions: None,
        })
        .await
        .unwrap();

    session_stack.scope = Some(bpmn_lite_types::session_stack::SessionScopeState {
        client_group_id: mutated_scope_id,
        client_group_name: Some("Mutated".to_string()),
    });
    session_stack.active_workspace =
        Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Deal);
    session_stack.trace_sequence = 77;

    let loaded = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
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
        Some(bpmn_lite_types::session_stack::SessionWorkspaceKind::Kyc)
    );
    assert_eq!(loaded.session_stack.trace_sequence, 5);
}

#[tokio::test]
async fn test_job_activation_preserves_runbook_lineage() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
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
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
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
            expected_preconditions: None,
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
    Arc<dyn WorkflowStore>,
    Uuid,
    String,
    [u8; 32],
) {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let cr = engine.compile(SINGLE_TASK_BPMN).await.unwrap();
    let payload = r#"{"case":"cancel-test"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
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
    let events = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap();
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
    let events_after_first = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap()
        .len();

    // Second complete with same job_key — should be silently accepted (dedupe)
    let result = engine
        .complete_job(&job_key, r#"{"r":"second"}"#, hash, BTreeMap::new())
        .await;
    assert!(result.is_ok(), "Duplicate complete_job should not error");

    // No new events should be emitted (dedupe short-circuits)
    let events_after_second = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap()
        .len();
    assert_eq!(
        events_after_first, events_after_second,
        "Dedupe should not emit additional events"
    );
}

#[tokio::test]
async fn test_complete_job_recomputes_payload_hash() {
    let (engine, store, iid, job_key, expected_hash) = setup_parked_job().await;
    let new_payload = r#"{"result":"done","version":2}"#;
    let new_hash = bpmn_lite_types::EffectId::content_hash((new_payload).as_bytes());

    engine
        .complete_job(&job_key, new_payload, expected_hash, BTreeMap::new())
        .await
        .unwrap();

    let persisted = store
        .load_instance(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.domain_payload.as_ref(), new_payload);
    assert_eq!(persisted.domain_payload_hash, new_hash);

    let history_payload = store
        .load_payload_version(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            iid,
            &new_hash,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(history_payload, new_payload);

    let events = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap();
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
    let stale_hash = bpmn_lite_types::EffectId::content_hash((r#"{"stale":true}"#).as_bytes());

    let result = engine
        .complete_job(
            &job_key,
            r#"{"result":"nope"}"#,
            stale_hash,
            BTreeMap::new(),
        )
        .await;
    assert!(result.is_err(), "stale expected hash must be rejected");

    let persisted = store
        .load_instance(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        persisted.domain_payload.as_ref(),
        r#"{"case":"cancel-test"}"#
    );
    assert_eq!(
        persisted.domain_payload_hash,
        bpmn_lite_types::EffectId::content_hash((r#"{"case":"cancel-test"}"#).as_bytes())
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
    let events = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap();
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
    let events = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap();
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
    let events = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap();
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
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let cr = engine.compile(SINGLE_TASK_BPMN).await.unwrap();
    let payload = r#"{"case":"retry"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
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
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // V5.3 (§18, landed 2026-07-23): migrated from v1 `Instr::WaitMsg` to
    // `Instr::V2WaitMsg` — v1 `WaitMsg` and its `wait_plan` side-table
    // entry are deleted entirely; `V2WaitMsg` needs neither `wait_id` nor
    // a `wait_plan` row (V-9 forbids a static side table surviving into
    // a v2-bearing artifact).
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [90u8; 32],
        program: vec![
            Instr::V2WaitMsg { name: 1 },
            Instr::End,
        ],
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::from([(1, "case_arrived".to_string())]),
        write_set: BTreeMap::new(),
        task_manifest: vec![],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    }
    .with_v2_corr_sources(BTreeMap::from([(
        bpmn_lite_types::Addr::new(0),
        bpmn_lite_types::BindingSource::Literal(bpmn_lite_types::Literal::Bool(false)),
    )]));
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let payload = r#"{"case":"signal"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let iid = engine
        .start(
            "signal_proc",
            program.bytecode_version(),
            payload,
            hash,
            "corr",
        )
        .await
        .unwrap();
    engine.tick_instance(iid).await.unwrap();

    let fibers = store
        .load_fibers(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap();
    assert!(matches!(fibers[0].wait, WaitState::Msg { .. }));

    engine
        .signal_with_value(
            iid,
            "case_arrived",
            "true".to_string(),
            None,
            None,
            Some("wrong"),
        )
        .await
        .unwrap();
    let fibers = store
        .load_fibers(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap();
    assert!(matches!(fibers[0].wait, WaitState::Msg { .. }));

    engine
        .signal_with_value(
            iid,
            "case_arrived",
            "false".to_string(),
            None,
            None,
            Some("right"),
        )
        .await
        .unwrap();
    let fibers = store
        .load_fibers(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap();
    assert_eq!(fibers[0].wait, WaitState::Running);
    assert_eq!(fibers[0].pc, 1.into());

    let events_after_first_delivery = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap()
        .len();
    engine
        .signal_with_value(
            iid,
            "case_arrived",
            "false".to_string(),
            None,
            None,
            Some("right"),
        )
        .await
        .unwrap();
    let events_after_duplicate = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap()
        .len();
    assert_eq!(events_after_duplicate, events_after_first_delivery);
}

#[tokio::test]
async fn test_signal_before_wait_msg_is_buffered_and_consumed() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // V5.3 (§18, landed 2026-07-23): migrated from v1 `Instr::WaitMsg` to
    // `Instr::V2WaitMsg`. This test in particular is why `V2WaitMsg`'s
    // kernel handler needed the signal-before-wait pre-check ported over
    // as part of this migration (see the handler's own doc comment) —
    // without it, this exact test would have failed.
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [91u8; 32],
        program: vec![
            Instr::V2WaitMsg { name: 1 },
            Instr::End,
        ],
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::from([(1, "1".to_string())]),
        write_set: BTreeMap::new(),
        task_manifest: vec![],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    }
    .with_v2_corr_sources(BTreeMap::from([(
        bpmn_lite_types::Addr::new(0),
        bpmn_lite_types::BindingSource::Literal(bpmn_lite_types::Literal::Bool(false)),
    )]));
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let payload = r#"{"case":"early-signal"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let iid = engine
        .start(
            "signal_proc",
            program.bytecode_version(),
            payload,
            hash,
            "corr",
        )
        .await
        .unwrap();

    engine
        .signal_with_value(iid, "1", "false".to_string(), None, None, Some("early"))
        .await
        .unwrap();

    engine.tick_instance(iid).await.unwrap();

    let inspection = engine.inspect(iid).await.unwrap();
    assert!(matches!(inspection.state, ProcessState::Completed { .. }));
    let events = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap();
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
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let tenant_a = BpmnLiteEngine::new_with_tenant(
        store.clone(),
        bpmn_lite_types::TenantId::new("tenant-a").unwrap(),
    );
    let tenant_b = BpmnLiteEngine::new_with_tenant(
        store.clone(),
        bpmn_lite_types::TenantId::new("tenant-b").unwrap(),
    );

    let compile_result = tenant_a.compile(SINGLE_TASK_BPMN).await.unwrap();
    let payload = r#"{"case":"tenant-a"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
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
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let tenant_a = BpmnLiteEngine::new_with_tenant(
        store.clone(),
        bpmn_lite_types::TenantId::new("tenant-a").unwrap(),
    );
    let tenant_b = BpmnLiteEngine::new_with_tenant(
        store.clone(),
        bpmn_lite_types::TenantId::new("tenant-b").unwrap(),
    );

    let instance_id = Uuid::now_v7();
    let payload = "{}";
    let instance = ProcessInstance {
        instance_id,
        tenant_id: "tenant-a".to_string(),
        process_key: "orphaned".to_string(),
        bytecode_version: [17u8; 32],
        domain_payload: payload.into(),
        domain_payload_hash: bpmn_lite_types::EffectId::content_hash((payload).as_bytes()),
        session_stack: SessionStackState::default(),
        flags: BTreeMap::new(),
        counters: BTreeMap::new(),
        join_expected: BTreeMap::new(),
        state: ProcessState::Running,
        correlation_id: "recover-me".to_string(),
        entry_id: Uuid::nil(),
        runbook_id: Uuid::nil(),
        created_at: 1,
        integrity_hash: None,
        quarantine_state: None,
        plan_hash: None,
        current_node_id: None,
        placeholder_values: None,
    };
    bpmn_lite_store::store::commit_initial_snapshot(store.as_ref(), instance)
        .await
        .unwrap();

    let issues = tenant_a
        .scan_recoverable_inconsistencies(&std::collections::HashSet::new())
        .await
        .unwrap();
    let kinds = issues
        .iter()
        .map(|issue| issue.kind.as_str())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"missing_artifact"));
    assert!(kinds.contains(&"missing_fibers"));
    assert!(kinds.contains(&"missing_start_event"));

    let tenant_b_issues = tenant_b
        .scan_recoverable_inconsistencies(&std::collections::HashSet::new())
        .await
        .unwrap();
    assert!(tenant_b_issues.is_empty());
}

#[tokio::test]
async fn startup_recovery_scans_every_tenant_before_readiness() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let runtime = Arc::new(crate::DeterministicRuntimeContext::new(
        1_800_000_000_000,
        Uuid::from_u128(900),
    ));
    let engine = BpmnLiteEngine::new_with_runtime_context(
        store,
        bpmn_lite_types::TenantId::default(),
        runtime,
    );
    for tenant_id in ["tenant-a", "tenant-b"] {
        let tenant = engine.for_tenant(bpmn_lite_types::TenantId::new(tenant_id).unwrap());
        let compiled = tenant.compile(ORDINARY_TIMER_BPMN).await.unwrap();
        tenant
            .start(
                "recovery-all-tenants",
                compiled.bytecode_version,
                "{}",
                bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
                tenant_id,
            )
            .await
            .unwrap();
    }

    let report = engine
        .recover_all_tenants("recovery", 32, 30_000)
        .await
        .unwrap();
    assert_eq!(report.tenants_scanned, 2);
    assert_eq!(report.instances_verified, 2);
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

// V5.3 (§18, landed 2026-07-23) originally found non-interrupting
// boundary-timer CYCLE (repeat) semantics — v1's `R<n>/PT<duration>`
// `timeCycle` mechanism — to be a real, unresolved design gap and
// deleted `NI_CYCLE_BPMN`/`t_ni_2_cycle_fires_multiple_times`/
// `t_ni_3_cycle_exhausted_reverts_to_job` with the finding recorded in
// place, rather than force a fix through: `Instr::V2GuardArmTimer`
// (`GUARD-TIMER>`, ruling I) was fire-once by construction, and nothing
// in the v2 word set re-armed a guard timer after it fired.
//
// **Restored (post-close remediation, landed 2026-07-23, same day):**
// V&S §13 amendment v0.5 ruling A ("`GUARD-N>` re-arms after trigger...
// the record stays `Armed`... this is the default") already ratified the
// general re-arm behaviour for `GUARD-N>`'s manual trigger
// (`Command::V2TriggerGuard`) — `GUARD-TIMER>`'s own timer-fire path
// simply hadn't been wired to the same default. Fixed at the kernel
// level (`bpmn-lite-kernel/src/lib.rs`'s `TimerKind::V2GuardTimer` arm
// of `apply_timer`): a `GUARD-N>`-kind record's timer now reschedules
// itself in the same transition it fires in
// (`TimerMutation::Rearm`, pre-existing generic timer-schedule
// infrastructure — `bpmn-lite-types::transition::TimerRepeatSpec`/
// `ClaimedTimer::repeat_spec`, this is the first `V2*` word to populate
// them), bounded by a new, optional, additive word,
// `Instr::V2GuardTimerCycle { max_fires }` (word `GUARD-TIMER-CYCLE>`),
// which must immediately follow `GUARD-TIMER>` and only ever bounds a
// `GUARD-N>` target (verifier-enforced, `v2_verifier.rs`). `GUARD>`/
// `GUARD-R>` (interrupting) are unaffected — their record retires on
// trigger, same as before this fix. No frontend lowering was built
// (out of scope, matching how `GUARD-TIMER>` itself landed —
// `lowering.rs`'s `timer_spec_duration_ms` still silently drops
// `TimerSpec::Cycle`'s `max_fires` to a single relative duration; BPMN
// XML `timeCycle` still does not reach `GUARD-TIMER-CYCLE>`), so both
// restored tests below hand-assemble their own `V2GuardN`/
// `GUARD-TIMER>`/`GUARD-TIMER-CYCLE>` bytecode (mirroring the REAL
// lowered shape `lower_boundary_guarded_task_v2` already produces for
// the single-fire case, plus the new cycle word) rather than compiling
// `NI_CYCLE_BPMN` through the XML frontend — proven via the real engine
// (`store_program` + `engine.start`/`tick_instance`/`tick_due_timers`),
// not `bpmn-lite-kernel::apply()` directly, per the brief's own
// instruction to land this restoration in `bpmn-lite-engine/src/tests.rs`.
//
// Single-fire non-interrupting boundary timers (`NI_BOUNDARY_BPMN`,
// `cancelActivity="false"` with a plain `timeDuration`, no cycle) were
// never part of this gap — `V2GuardN` + `GUARD-TIMER>` fully supports
// them, proven below by `t_ni_1_non_interrupting_spawns_child` and
// `t_ni_4_job_completes_before_timer`.

/// Helper: compile (v2 default) + start + tick until the host fiber is
/// parked on its own job, with a `GUARD-TIMER>`-armed non-interrupting
/// guard wrapping it. Unlike the deleted v1 `setup_ni_race`, the host
/// fiber's `WaitState` never changes to a race-shaped variant — arming a
/// guard timer does not park the fiber differently (`V2GuardN`'s own
/// doc comment: "the guarded body keeps executing normally after
/// arming"); it stays plain `WaitState::Job` for as long as the job is
/// outstanding, timer armed or not.
async fn setup_ni_guard(
    bpmn: &str,
) -> (
    BpmnLiteEngine,
    Arc<dyn WorkflowStore>,
    Uuid,
    String,
    [u8; 32],
) {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let cr = engine.compile(bpmn).await.unwrap();
    let payload = r#"{"case":"ni-test"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let iid = engine
        .start("ni_proc", cr.bytecode_version, payload, hash, "corr-ni")
        .await
        .unwrap();

    engine.tick_instance(iid).await.unwrap();

    let jobs = engine
        .activate_jobs(&["long_work".to_string()], 10)
        .await
        .unwrap();
    assert!(!jobs.is_empty(), "Expected job activation");
    let job_key = jobs[0].job_key.clone();

    let fibers = store
        .load_fibers(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap();
    assert_eq!(fibers.len(), 1);
    assert!(
        matches!(&fibers[0].wait, WaitState::Job { job_key: jk } if *jk == job_key),
        "Expected the host fiber parked on its own job, got {:?}",
        fibers[0].wait
    );

    (engine, store, iid, job_key, hash)
}

// ── T-NI-1b: a plain (non-cycle) boundary timer fires exactly once,
// not indefinitely ──

#[tokio::test]
async fn t_ni_1b_non_cycle_timer_does_not_rearm_after_first_fire() {
    let (engine, _store, iid, _job_key, _hash) = setup_ni_guard(NI_BOUNDARY_BPMN).await;

    engine
        .tick_due_timers("timer-test", FAR_FUTURE_TIMER_MS, 10, 30_000)
        .await
        .unwrap();
    engine.tick_instance(iid).await.unwrap();

    // A rearmed timer's new due_at is fired_at + interval_ms, so a second
    // sweep must use a later "as of" timestamp to reach it.
    engine
        .tick_due_timers("timer-test", FAR_FUTURE_TIMER_MS + 60_000, 10, 30_000)
        .await
        .unwrap();
    engine.tick_instance(iid).await.unwrap();

    let escalation_jobs = engine
        .activate_jobs(&["send_reminder".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(
        escalation_jobs.len(),
        1,
        "a non-cycle boundary timer must fire exactly once, got {} escalation jobs",
        escalation_jobs.len()
    );
}

// ── T-NI-1: Non-interrupting GUARD-TIMER> fires → spawns escalation job,
// host fiber's own job wait is unaffected ──

#[tokio::test]
async fn t_ni_1_non_interrupting_spawns_child() {
    let (engine, store, iid, job_key, _hash) = setup_ni_guard(NI_BOUNDARY_BPMN).await;

    engine
        .tick_due_timers("timer-test", FAR_FUTURE_TIMER_MS, 10, 30_000)
        .await
        .unwrap();
    // The fired timer spawns the escalation handler fibre parked; a
    // further tick advances it through its own ExecNative to actually
    // enqueue the job (mirrors t_boundary_timer_v2_guard_timer_fires_
    // and_activates_escalation_job's identical second tick).
    engine.tick_instance(iid).await.unwrap();

    // Host fiber is unaffected by the timer firing — still parked on the
    // same job (non-interrupting: the guard scope re-arms/stays open,
    // it does not touch the host at all).
    let fibers = store
        .load_fibers(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap();
    assert!(
        fibers
            .iter()
            .any(|f| matches!(&f.wait, WaitState::Job { job_key: jk } if *jk == job_key)),
        "Host fiber should still be parked on its own job, got {:?}",
        fibers.iter().map(|f| &f.wait).collect::<Vec<_>>()
    );

    // The escalation branch's own job ("send_reminder") must now be
    // activatable — this is the v2-shaped replacement for the deleted
    // `RuntimeEvent::BoundaryFired` assertion: instead of reading an
    // event, prove the escalation handler fibre actually ran its own
    // `ExecNative` and enqueued real work.
    let escalation_jobs = engine
        .activate_jobs(&["send_reminder".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(
        escalation_jobs.len(),
        1,
        "Expected the escalation handler's own job to be activated"
    );

    // Instance should still be Running (not Completed) — the host branch
    // never resolved.
    let inspection = engine.inspect(iid).await.unwrap();
    assert_eq!(inspection.state, ProcessState::Running);
}

/// Hand-assembled `V2GuardN` + `GUARD-TIMER>` + `GUARD-TIMER-CYCLE>`
/// program, mirroring `lower_boundary_guarded_task_v2`'s real lowered
/// shape (`bpmn-lite-compiler/src/lowering.rs`) for a non-interrupting
/// boundary timer on a service task — `PushI64(duration); V2GuardN;
/// V2GuardArmTimer; <host task>; V2GuardNEnd; End`, with the escalation
/// handler itself closing via `ExecNative; V2GuardNEnd; End` (the exact
/// `guardn_close_before_end` pre-pass shape) — plus this restoration's
/// own new word, `V2GuardTimerCycle { max_fires: 3 }`, bounding the
/// timer to 3 fires (BPMN `R3/PT<duration>`). No `engine.compile(xml)`
/// step: XML `timeCycle` lowering to `GUARD-TIMER-CYCLE>` remains out of
/// scope (see the comment block above), so the program is stored
/// directly via `store_program`, same as `t_term_2_parallel_terminate_
/// kills_siblings`'s own hand-assembled `V2Fork` restoration.
async fn setup_ni_cycle_guard() -> (
    BpmnLiteEngine,
    Arc<dyn WorkflowStore>,
    Uuid,
    String,
    [u8; 32],
) {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [42u8; 32],
        program: vec![
            Instr::PushI64(1_000),                              // 0: duration
            Instr::V2GuardN { handler: 7.into() },               // 1
            Instr::V2GuardArmTimer,                              // 2
            Instr::V2GuardTimerCycle { max_fires: 3 },           // 3: R3
            Instr::ExecNative { task_type: 0, argc: 0, retc: 0 }, // 4: long_work
            Instr::V2GuardNEnd,                                  // 5
            Instr::End,                                          // 6
            Instr::ExecNative { task_type: 1, argc: 0, retc: 0 }, // 7: send_reminder (handler)
            Instr::V2GuardNEnd,                                  // 8
            Instr::End,                                          // 9
        ],
        debug_map: BTreeMap::from([
            (4.into(), "long_work".to_string()),
            (7.into(), "send_reminder".to_string()),
        ]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["long_work".to_string(), "send_reminder".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let payload = r#"{"case":"ni-cycle-test"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let iid = engine
        .start(
            "test",
            program.bytecode_version(),
            payload,
            hash,
            "corr-ni-cycle",
        )
        .await
        .unwrap();

    engine.tick_instance(iid).await.unwrap();

    let jobs = engine
        .activate_jobs(&["long_work".to_string()], 10)
        .await
        .unwrap();
    assert!(!jobs.is_empty(), "Expected job activation");
    let job_key = jobs[0].job_key.clone();

    let fibers = store
        .load_fibers(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap();
    assert_eq!(fibers.len(), 1);
    assert!(
        matches!(&fibers[0].wait, WaitState::Job { job_key: jk } if *jk == job_key),
        "Expected the host fiber parked on its own job, got {:?}",
        fibers[0].wait
    );

    (engine, store, iid, job_key, hash)
}

// ── T-NI-2: Cycle R3 fires 3 times, spawning 3 escalation handler fibres,
// via the SAME durable timer row re-arming each time (`TimerMutation::
// Rearm`), not 3 independently-scheduled timers ──

#[tokio::test]
async fn t_ni_2_cycle_fires_multiple_times() {
    let (engine, store, iid, job_key, _hash) = setup_ni_cycle_guard().await;

    // Fire 3 iterations using a representable deterministic test clock —
    // each `tick_due_timers` call claims the SAME re-armed timer row
    // (`TimerMutation::Rearm` updates its `due_at` in place rather than
    // scheduling a new one), so a growing `fired_at` per iteration is
    // required for the row to become claimable again, exactly as the
    // deleted v1 test's own clock-advancing loop did.
    for i in 0..3u64 {
        let fired_at = FAR_FUTURE_TIMER_MS + i * 100_000;
        let applied = engine
            .tick_due_timers("timer-test", fired_at, 10, 30_000)
            .await
            .unwrap();
        assert_eq!(applied, 1, "iteration {i}: exactly one timer must fire");

        // Host fiber is never touched by any of the 3 fires — it stays
        // parked on its own job throughout, non-interrupting per ruling A.
        let fibers = store
            .load_fibers(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
            .await
            .unwrap();
        assert!(
            fibers
                .iter()
                .any(|f| matches!(&f.wait, WaitState::Job { job_key: jk } if *jk == job_key)),
            "iteration {i}: host fiber should still be parked on its own job"
        );
    }

    // Each of the 3 fires spawned its own, distinct escalation handler
    // fibre — proven via `RuntimeEvent::V2GuardNTriggered` (one per fire,
    // each naming a different `handler_fiber_id`) and
    // `RuntimeEvent::TimerFired` (one per fire, same guard's timer),
    // rather than via job-queue activation counts: the escalation
    // handler's own `ExecNative` (`send_reminder`) computes its job_key
    // from `(instance_id, service_task_id, pc, loop_epoch)` only — no
    // fiber_id component — so 3 concurrently-live handler fibres sitting
    // at the SAME static pc collide onto the SAME job_key. That is a
    // pre-existing gap in job_key derivation (already latent for repeated
    // MANUAL `Command::V2TriggerGuard` re-triggering of the same handler
    // address, unaffected by and out of scope for this restoration — see
    // the comment block above this test), not something this fix
    // introduces or is responsible for resolving; events, not the job
    // queue, are the right place to observe "3 separate spawns" here.
    let tenant = bpmn_lite_types::TenantId::new("default").unwrap();
    let events = store.read_events(&tenant, iid, 0).await.unwrap();
    let timer_fired_count = events
        .iter()
        .filter(|(_, e)| matches!(e, RuntimeEvent::TimerFired { .. }))
        .count();
    assert_eq!(
        timer_fired_count, 3,
        "Expected 3 TimerFired events, one per cycle iteration"
    );
    let spawned_handlers: std::collections::BTreeSet<Uuid> = events
        .iter()
        .filter_map(|(_, e)| match e {
            RuntimeEvent::V2GuardNTriggered {
                handler_fiber_id, ..
            } => Some(*handler_fiber_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        spawned_handlers.len(),
        3,
        "Expected 3 distinct escalation handler fibres, one per fire, got: {events:?}"
    );

    // A further tick_instance runs each spawned handler fibre to its own
    // park point (its own ExecNative dispatch) — the fibre count alone
    // (not job identity) confirms all 3 are real, live, concurrently-held
    // fibres, not the same one reused.
    engine.tick_instance(iid).await.unwrap();
    let fibers = store.load_fibers(&tenant, iid).await.unwrap();
    assert_eq!(
        fibers.len(),
        4,
        "host fibre + 3 escalation handler fibres, all still live"
    );

    let inspection = engine.inspect(iid).await.unwrap();
    assert_eq!(inspection.state, ProcessState::Running);
}

// ── T-NI-3: Cycle exhausted after 3 fires → no 4th rearm; host fiber was
// never diverted out of its own job wait in the first place ──

#[tokio::test]
async fn t_ni_3_cycle_exhausted_reverts_to_job() {
    let (engine, store, iid, job_key, _hash) = setup_ni_cycle_guard().await;

    // Fire all 3 permitted iterations.
    for i in 0..3u64 {
        let fired_at = FAR_FUTURE_TIMER_MS + i * 100_000;
        let applied = engine
            .tick_due_timers("timer-test", fired_at, 10, 30_000)
            .await
            .unwrap();
        assert_eq!(applied, 1, "iteration {i}: exactly one timer must fire");
    }

    // A 4th attempt, well past the 3rd fire's own interval, must find
    // nothing due — `remaining` reached its last permitted fire on the
    // 3rd `TimerFired`, so that transition pushed `TimerMutation::Consume`
    // instead of `Rearm` (see `Instr::V2GuardTimerCycle`'s doc comment):
    // exhaustion means "no further rearm," not a fourth no-op fire.
    let fourth_fired_at = FAR_FUTURE_TIMER_MS + 3 * 100_000;
    let applied = engine
        .tick_due_timers("timer-test", fourth_fired_at, 10, 30_000)
        .await
        .unwrap();
    assert_eq!(
        applied, 0,
        "the cycle is exhausted — no 4th timer should be due"
    );

    // "Reverts to job," precisely for what the v2 mechanism actually
    // does: unlike v1's `WaitState::Race` (which diverted the host fiber
    // out of plain `Job` wait for the cycle's duration and had to revert
    // it back on exhaustion), `GUARD-N>`'s host fiber was NEVER diverted
    // out of `WaitState::Job` by any of the 3 fires — arming/firing a
    // guard timer does not touch the guarded body's own fiber at all.
    // So the host fiber is found in exactly the same `Job` wait,
    // continuously, before, during, and after cycle exhaustion — the v2
    // shape of "reverts to job" is "was never not there."
    let fibers = store
        .load_fibers(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap();
    assert!(
        fibers
            .iter()
            .any(|f| matches!(&f.wait, WaitState::Job { job_key: jk } if *jk == job_key)),
        "After cycle exhaustion, the host fiber must still be in its own (uninterrupted) \
         Job wait. Got: {:?}",
        fibers.iter().map(|f| &f.wait).collect::<Vec<_>>()
    );

    // All 3 fires still happened — exhaustion only suppresses the 4th.
    // Proven via events, not job-queue activation counts — see
    // `t_ni_2_cycle_fires_multiple_times`'s own comment for why (a
    // pre-existing, out-of-scope job_key-collision gap for concurrently-
    // live fibres sharing one static pc).
    let tenant = bpmn_lite_types::TenantId::new("default").unwrap();
    let events = store.read_events(&tenant, iid, 0).await.unwrap();
    let timer_fired_count = events
        .iter()
        .filter(|(_, e)| matches!(e, RuntimeEvent::TimerFired { .. }))
        .count();
    assert_eq!(
        timer_fired_count, 3,
        "All 3 permitted fires must still have happened — exhaustion only suppresses the 4th"
    );
    let spawned_handlers: std::collections::BTreeSet<Uuid> = events
        .iter()
        .filter_map(|(_, e)| match e {
            RuntimeEvent::V2GuardNTriggered {
                handler_fiber_id, ..
            } => Some(*handler_fiber_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        spawned_handlers.len(),
        3,
        "3 distinct escalation handler fibres, got: {events:?}"
    );
}

// ── T-NI-4: Job completes before non-interrupting timer → normal resolution ──

#[tokio::test]
async fn t_ni_4_job_completes_before_timer() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let cr = engine.compile(NI_BOUNDARY_BPMN).await.unwrap();
    let payload = r#"{"case":"ni-job-first"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let iid = engine
        .start("ni_proc", cr.bytecode_version, payload, hash, "corr-ni4")
        .await
        .unwrap();

    // Tick to arm the GUARD-TIMER> and park the host fiber on its job
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
                bpmn_lite_types::EffectId::content_hash(
                    store
                        .load_instance(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
                        .await
                        .unwrap()
                        .unwrap()
                        .domain_payload
                        .as_bytes(),
                ),
                BTreeMap::new(),
            )
            .await;
    }

    // Keep ticking to reach completion
    for _ in 0..5 {
        engine.tick_instance(iid).await.unwrap();
    }

    // Instance should eventually complete via the host's own normal path
    // (V2GuardNEnd closes the still-armed guard scope on the way out).
    let inspection = engine.inspect(iid).await.unwrap();
    assert!(
        matches!(inspection.state, ProcessState::Completed { .. }),
        "Expected Completed after job finishes, got {:?}",
        inspection.state
    );

    // The escalation branch's own job ("send_reminder") must never have
    // been activated — the v2-shaped replacement for the deleted
    // `RuntimeEvent::BoundaryFired` assertion: since the host job
    // completed before the guard timer ever fired, the escalation
    // handler fibre never ran, so its `ExecNative` never enqueued work.
    let escalation_jobs = engine
        .activate_jobs(&["send_reminder".to_string()], 10)
        .await
        .unwrap();
    assert!(
        escalation_jobs.is_empty(),
        "Escalation job should not have been activated when the host job completes first, got {:?}",
        escalation_jobs
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

    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
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

// ── T-NI-6: `timeCycle` XML → `GUARD-TIMER-CYCLE>` frontend wiring
// (this landing's own gap-closer) — full `engine.compile(xml)` pipeline,
// not hand-assembled bytecode like `setup_ni_cycle_guard` above. Proves
// the frontend actually reaches the kernel mechanism `t_ni_2`/`t_ni_3`
// already proved sound; does not re-prove the mechanism itself. ──

const NI_CYCLE_XML_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                      xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
      <bpmn:process id="ni_cycle_xml_proc" isExecutable="true">
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

#[tokio::test]
async fn t_ni_6_xml_cycle_fires_three_times_then_exhausts() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Real XML frontend, not hand-assembled bytecode — this is the
    // thing this landing actually fixes: `timeCycle` reaching
    // `Instr::V2GuardTimerCycle` via `parser::parse_bpmn` +
    // `lowering::lower`/`Compiler::lower`, not just the kernel's own
    // already-proven handling of the word once emitted.
    let cr = engine.compile(NI_CYCLE_XML_BPMN).await.unwrap();
    let payload = r#"{"case":"ni-xml-cycle-test"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let iid = engine
        .start(
            "ni_cycle_xml_proc",
            cr.bytecode_version,
            payload,
            hash,
            "corr-ni6",
        )
        .await
        .unwrap();

    engine.tick_instance(iid).await.unwrap();

    let jobs = engine
        .activate_jobs(&["long_work".to_string()], 10)
        .await
        .unwrap();
    assert!(!jobs.is_empty(), "Expected job activation");
    let job_key = jobs[0].job_key.clone();

    let tenant = bpmn_lite_types::TenantId::new("default").unwrap();
    let fibers = store.load_fibers(&tenant, iid).await.unwrap();
    assert_eq!(fibers.len(), 1);
    assert!(
        matches!(&fibers[0].wait, WaitState::Job { job_key: jk } if *jk == job_key),
        "Expected the host fiber parked on its own job, got {:?}",
        fibers[0].wait
    );

    // Fire all 3 permitted iterations — same re-armed-timer-row pattern
    // as `t_ni_2_cycle_fires_multiple_times`.
    for i in 0..3u64 {
        let fired_at = FAR_FUTURE_TIMER_MS + i * 100_000;
        let applied = engine
            .tick_due_timers("timer-test", fired_at, 10, 30_000)
            .await
            .unwrap();
        assert_eq!(applied, 1, "iteration {i}: exactly one timer must fire");

        // Host fiber is never touched by any of the 3 fires.
        let fibers = store.load_fibers(&tenant, iid).await.unwrap();
        assert!(
            fibers
                .iter()
                .any(|f| matches!(&f.wait, WaitState::Job { job_key: jk } if *jk == job_key)),
            "iteration {i}: host fiber should still be parked on its own job"
        );
    }

    // A 4th attempt must find nothing due — the cycle is exhausted after
    // 3 fires (R3), proving `max_fires` actually reached the kernel via
    // the XML frontend rather than being silently dropped to unbounded
    // or to a single fire.
    let fourth_fired_at = FAR_FUTURE_TIMER_MS + 3 * 100_000;
    let applied = engine
        .tick_due_timers("timer-test", fourth_fired_at, 10, 30_000)
        .await
        .unwrap();
    assert_eq!(
        applied, 0,
        "the cycle is exhausted — no 4th timer should be due"
    );

    // 3 distinct escalation handler fibres were spawned, one per fire —
    // same event-based proof `t_ni_2` uses (job_key collision on the
    // shared static pc makes job-queue activation counts unreliable
    // here, see that test's own comment).
    let events = store.read_events(&tenant, iid, 0).await.unwrap();
    let timer_fired_count = events
        .iter()
        .filter(|(_, e)| matches!(e, RuntimeEvent::TimerFired { .. }))
        .count();
    assert_eq!(
        timer_fired_count, 3,
        "Expected 3 TimerFired events, one per cycle iteration"
    );
    let spawned_handlers: std::collections::BTreeSet<Uuid> = events
        .iter()
        .filter_map(|(_, e)| match e {
            RuntimeEvent::V2GuardNTriggered {
                handler_fiber_id, ..
            } => Some(*handler_fiber_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        spawned_handlers.len(),
        3,
        "Expected 3 distinct escalation handler fibres, one per fire, got: {events:?}"
    );

    let inspection = engine.inspect(iid).await.unwrap();
    assert_eq!(inspection.state, ProcessState::Running);
}

// ── Phase 5.1: Terminate End Event tests ────────────────────────

/// T-TERM-1: Single fiber hits EndTerminate → instance Terminated.
#[tokio::test]
async fn t_term_1_single_fiber_terminate() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [40u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::EndTerminate,
        ],
        debug_map: BTreeMap::from([(0.into(), "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-1",
        )
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 1);

    let payload = "{}";
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    engine
        .complete_job(&jobs[0].job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    // Assert: Terminated
    let instance = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(instance.state, ProcessState::Terminated { .. }),
        "Expected Terminated, got {:?}",
        instance.state
    );

    // Assert: no fibers remain
    let fibers = store
        .load_fibers(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap();
    assert!(fibers.is_empty());

    // Assert: Terminated event
    let events = store
        .read_events(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
            0,
        )
        .await
        .unwrap();
    let has_term = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::Terminated { .. }));
    assert!(has_term);
}

/// T-TERM-2: Parallel flow — one branch terminates, other branch killed.
/// Restored (post-close remediation, landed after the V-1 `EndTerminate`
/// exemption): V5.3 originally deleted this test and flagged the gap it
/// found as a genuine open verifier-soundness question, not a small
/// migration — `V2Fork{targets:[1,2]}` with branch A going straight to
/// `EndTerminate` (never reaching a `V2Join`) was rejected by V-1's
/// control-stack balance check, which (unlike `Fail`) gave `EndTerminate`
/// no exemption from requiring an empty stack. Adam's ruling: `EndTerminate`
/// kills the WHOLE instance, so no fibre's own open scope can be orphaned
/// (every fibre dies with it) — same reasoning as `Fail`'s pre-existing
/// exemption. `v2_verifier.rs`'s V-1 walk now leaves `Instr::EndTerminate`
/// unmatched (falls through to the no-op catch-all, same mechanism `Fail`
/// already used), so this topology verifies again. Order-independent, as
/// the original test's own framing had it: handles either child fibre
/// executing first after the fork — whichever one is NOT branch A either
/// never gets ticked at all (branch A wins the race for the first internal
/// `Tick`) or parks mid-flight on its own dispatched job (branch B wins
/// the race, dispatches `slow_task`, parks, then branch A's `EndTerminate`
/// fires on the next `Tick`) — both are exercised across real test runs
/// since fibre-processing order depends on derived UUID ordering, not
/// fixed here.
#[tokio::test]
async fn t_term_2_parallel_terminate_kills_siblings() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // V2Fork → Branch A (EndTerminate directly, no V2Join), Branch B
    // (ExecNative → EndTerminate, also no V2Join — branch B's own
    // reachable terminal is never actually hit in this test; branch A
    // always wins, either before branch B ever runs or while branch B
    // sits parked on its own dispatched job).
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [41u8; 32],
        program: vec![
            Instr::V2Fork {
                targets: Box::new([1.into(), 2.into()]),
                pairing: 0.into(),
            }, // 0: fork
            Instr::EndTerminate, // 1: Branch A terminates
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 2: Branch B task
            Instr::EndTerminate, // 3: Branch B's own terminal (unreached here)
        ],
        debug_map: BTreeMap::from([(2.into(), "slow_task".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["slow_task".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-2",
        )
        .await
        .unwrap();

    // Tick until instance reaches terminal state.
    for _ in 0..5 {
        engine.tick_instance(instance_id).await.unwrap();
        let inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
        if inst.state.is_terminal() {
            break;
        }
    }

    // Assert: instance is Terminated (not Completed, not Running)
    let instance = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(instance.state, ProcessState::Terminated { .. }),
        "Expected Terminated, got {:?}",
        instance.state
    );

    // Assert: no fibers remain
    let fibers = store
        .load_fibers(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap();
    assert!(fibers.is_empty(), "All fibers should be deleted");

    // Assert: Terminated event emitted
    let events = store
        .read_events(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
            0,
        )
        .await
        .unwrap();
    let has_term = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::Terminated { .. }));
    assert!(has_term, "Should emit Terminated event");

    // Assert: no jobs for this instance remain
    let jobs = store
        .dequeue_jobs(
            &["slow_task".to_string()],
            100,
            &bpmn_lite_types::TenantId::default(),
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
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [42u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::EndTerminate,
        ],
        debug_map: BTreeMap::from([(0.into(), "task_x".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_x".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-3",
        )
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    let job_key = jobs[0].job_key.clone();

    // Complete the job → fiber advances to EndTerminate → instance Terminated
    let payload = "{}";
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    engine
        .complete_job(&job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    assert!(matches!(
        store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id
            )
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
    let instance = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
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
        .program()
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

    // BoundaryError v2 migration: the host task is now wrapped in a
    // `V2Guard`/`V2GuardArmError` scope (§18 v0.10 ruling I's second
    // arming-trigger kind) instead of relying on the deleted
    // `error_route_map` side table. A match now fires via the same
    // spawn-a-new-handler-fiber mechanism `V2TriggerGuard`/timer-fire use
    // (`v2_trigger_guard_changes_with_target`) — the ORIGINAL failing fibre
    // is retired, not resumed in place, so assertions below check for A
    // Running fibre at the handler pc, not the SAME fibre.
    //
    // Bytecode:
    // 0: V2Guard { handler: 5 }                      — opens the guard
    // 1: V2GuardArmError(SANCTIONS_HIT, handler: 5)   — arms the route
    // 2: ExecNative(sanctions_check)                  — parks fiber
    // 3: V2GuardEnd
    // 4: Jump(6)                                       — normal continuation
    // 5: ExecNative(enhanced_review)                   — error handler path
    // 6: End                                           — error handler end
    // 7: End                                           — normal end
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [50u8; 32],
        program: vec![
            Instr::V2Guard { handler: 5.into() }, // 0
            Instr::V2GuardArmError {
                error_code: Some("SANCTIONS_HIT".to_string().into_boxed_str()),
                handler: 5.into(),
            }, // 1
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 2
            Instr::V2GuardEnd,                // 3
            Instr::Jump { target: 7.into() }, // 4
            Instr::ExecNative {
                task_type: 1,
                argc: 0,
                retc: 0,
            }, // 5: error handler
            Instr::End,                // 6
            Instr::End,                // 7
        ],
        debug_map: BTreeMap::from([
            (2.into(), "sanctions_check".to_string()),
            (5.into(), "enhanced_review".to_string()),
        ]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["sanctions_check".to_string(), "enhanced_review".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-1",
        )
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
    let events = store
        .read_events(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
            0,
        )
        .await
        .unwrap();
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
    let instance = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(instance.state, ProcessState::Running),
        "Instance should stay Running after error routing, got {:?}",
        instance.state
    );

    // Assert: fiber was routed to error handler (pc=2)
    let fibers = store
        .load_fibers(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap();
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
            &bpmn_lite_types::TenantId::default(),
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

    // Same guard shape as T-ERR-1 but only arms SANCTIONS_HIT — the fail
    // below uses a non-matching code, so `V2GuardArmError`'s route never
    // fires and this falls through to the ordinary incident path unchanged.
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [51u8; 32],
        program: vec![
            Instr::V2Guard { handler: 4.into() }, // 0 — never fired; must still be in-bounds (artifact verifier checks statically)
            Instr::V2GuardArmError {
                error_code: Some("SANCTIONS_HIT".to_string().into_boxed_str()),
                handler: 4.into(), // never fired — the fail below uses a non-matching code
            }, // 1
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 2
            Instr::V2GuardEnd, // 3
            Instr::End,        // 4
        ],
        debug_map: BTreeMap::from([(2.into(), "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-2",
        )
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
    let events = store
        .read_events(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
            0,
        )
        .await
        .unwrap();
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

    // Assert: instance Incidented (parked on an open Incident, resumable
    // via Command::ResolveIncident — not Failed, which is genuinely dead
    // forever).
    let instance = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(instance.state, ProcessState::Incidented { .. }));
}

/// T-ERR-3: Catch-all error route (error_code: None) catches any BusinessRejection.
#[tokio::test]
async fn t_err_3_catch_all_routes_any_business_error() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [52u8; 32],
        program: vec![
            Instr::V2Guard { handler: 5.into() }, // 0
            Instr::V2GuardArmError {
                error_code: None, // catch-all
                handler: 5.into(),
            }, // 1
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 2
            Instr::V2GuardEnd,                // 3
            Instr::Jump { target: 6.into() }, // 4: normal path
            Instr::End,                // 5: error handler end
            Instr::End,                // 6: normal end
        ],
        debug_map: BTreeMap::from([(2.into(), "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-3",
        )
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
    let events = store
        .read_events(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
            0,
        )
        .await
        .unwrap();
    let has_routed = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::ErrorRouted { .. }));
    assert!(has_routed, "Catch-all should route any BusinessRejection");

    let instance = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(instance.state, ProcessState::Running));
}

/// T-ERR-4: Transient error always creates incident, even with error route present.
#[tokio::test]
async fn t_err_4_transient_error_always_incident() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [53u8; 32],
        program: vec![
            Instr::V2Guard { handler: 4.into() }, // 0
            Instr::V2GuardArmError {
                error_code: None, // catch-all — must NOT apply to Transient
                handler: 4.into(),
            }, // 1
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 2
            Instr::V2GuardEnd, // 3
            Instr::End,        // 4: error handler (won't be used)
            Instr::End,        // 5: normal end
        ],
        debug_map: BTreeMap::from([(2.into(), "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-4",
        )
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();

    // Fail with Transient — error routes should NOT apply
    engine
        .fail_job(&jobs[0].job_key, ErrorClass::Transient, "timeout")
        .await
        .unwrap();

    // Assert: incident, NOT routed
    let events = store
        .read_events(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
            0,
        )
        .await
        .unwrap();
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
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [54u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::EndTerminate,
        ],
        debug_map: BTreeMap::from([(0.into(), "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-5",
        )
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    let job_key = jobs[0].job_key.clone();

    // Complete job → EndTerminate → Terminated
    let payload = "{}";
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    engine
        .complete_job(&job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    assert!(matches!(
        store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id
            )
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
    let events = store
        .read_events(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
            0,
        )
        .await
        .unwrap();
    let has_ignored = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::SignalIgnored { .. }));
    assert!(has_ignored, "Should emit SignalIgnored for late fail_job");
}

/// T-ERR-6: multiple specific-code `V2GuardArmError` routes on ONE guard
/// each route independently — proves the multi-arm mechanism (N armed
/// routes on a single guard record, `ConcurrencyRecord.error_routes`), not
/// just the single-arm case T-ERR-1 already covers. Two independent
/// instances of the SAME program are each failed with a DIFFERENT specific
/// code; each must resolve to its OWN handler, not the other's.
#[tokio::test]
async fn t_err_6_multiple_specific_routes_fire_independently() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Bytecode:
    // 0: V2Guard { handler: 6 }
    // 1: V2GuardArmError(CODE_A, handler: 6)
    // 2: V2GuardArmError(CODE_B, handler: 8)
    // 3: ExecNative(risky_task)
    // 4: V2GuardEnd
    // 5: Jump(10)                — normal path
    // 6: ExecNative(handler_a)   — CODE_A's own handler
    // 7: Jump(9)
    // 8: ExecNative(handler_b)   — CODE_B's own handler
    // 9: End
    // 10: End
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [55u8; 32],
        program: vec![
            Instr::V2Guard { handler: 6.into() }, // 0
            Instr::V2GuardArmError {
                error_code: Some("CODE_A".to_string().into_boxed_str()),
                handler: 6.into(),
            }, // 1
            Instr::V2GuardArmError {
                error_code: Some("CODE_B".to_string().into_boxed_str()),
                handler: 8.into(),
            }, // 2
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 3
            Instr::V2GuardEnd,                 // 4
            Instr::Jump { target: 10.into() }, // 5
            Instr::ExecNative {
                task_type: 1,
                argc: 0,
                retc: 0,
            }, // 6: handler for CODE_A
            Instr::Jump { target: 9.into() }, // 7
            Instr::ExecNative {
                task_type: 2,
                argc: 0,
                retc: 0,
            }, // 8: handler for CODE_B
            Instr::End,  // 9
            Instr::End,  // 10
        ],
        debug_map: BTreeMap::from([
            (3.into(), "risky_task".to_string()),
            (6.into(), "handler_a".to_string()),
            (8.into(), "handler_b".to_string()),
        ]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![
            "risky_task".to_string(),
            "handler_a".to_string(),
            "handler_b".to_string(),
        ],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    // Instance 1: fail with CODE_A — must activate handler_a, never handler_b.
    let instance_a = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-a",
        )
        .await
        .unwrap();
    let jobs_a = engine.run_instance(instance_a).await.unwrap();
    assert_eq!(jobs_a.len(), 1);
    engine
        .fail_job(
            &jobs_a[0].job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "CODE_A".to_string(),
            },
            "attempt A",
        )
        .await
        .unwrap();
    engine.tick_instance(instance_a).await.unwrap();
    let handler_a_jobs = store
        .dequeue_jobs(
            &["handler_a".to_string()],
            10,
            &bpmn_lite_types::TenantId::default(),
            "w",
            300_000,
        )
        .await
        .unwrap();
    assert!(
        !handler_a_jobs.is_empty(),
        "CODE_A must activate handler_a's job"
    );
    let handler_b_jobs_after_a = store
        .dequeue_jobs(
            &["handler_b".to_string()],
            10,
            &bpmn_lite_types::TenantId::default(),
            "w",
            300_000,
        )
        .await
        .unwrap();
    assert!(
        handler_b_jobs_after_a.is_empty(),
        "CODE_A must NOT activate handler_b's job"
    );

    // Instance 2: fail with CODE_B — must activate handler_b, never handler_a.
    let instance_b = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-b",
        )
        .await
        .unwrap();
    let jobs_b = engine.run_instance(instance_b).await.unwrap();
    assert_eq!(jobs_b.len(), 1);
    engine
        .fail_job(
            &jobs_b[0].job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "CODE_B".to_string(),
            },
            "attempt B",
        )
        .await
        .unwrap();
    engine.tick_instance(instance_b).await.unwrap();
    let handler_b_jobs = store
        .dequeue_jobs(
            &["handler_b".to_string()],
            10,
            &bpmn_lite_types::TenantId::default(),
            "w",
            300_000,
        )
        .await
        .unwrap();
    assert!(
        !handler_b_jobs.is_empty(),
        "CODE_B must activate handler_b's job"
    );
    let handler_a_jobs_after_b = store
        .dequeue_jobs(
            &["handler_a".to_string()],
            10,
            &bpmn_lite_types::TenantId::default(),
            "w",
            300_000,
        )
        .await
        .unwrap();
    assert!(
        handler_a_jobs_after_b.is_empty(),
        "CODE_B must NOT activate handler_a's job"
    );
}

/// T-ERR-7: nested guards — an outer catch-all must NOT catch an inner
/// guard's unmatched failure. Regression for the independent blind-review
/// finding (2026-07-23): `apply_job_failure`'s error-route search used a
/// single `find_map` whose closure returned `None` for both "not a guard,
/// keep looking outward" and "is the guard, but no matching route" —
/// collapsing those two cases let the outward walk continue past the
/// innermost armed guard to an outer one. Live-repro'd before the fix: a
/// rejection code matching neither the inner guard's specific route nor
/// (correctly) requiring a match wrongly activated the OUTER catch-all's
/// handler instead of falling through to an `Incident`. The fix stops the
/// search at the first armed `Guard`-kind record unconditionally (mirroring
/// `innermost_guard`'s own "stop at the first Guard-kind record regardless"
/// rule below in the same function) and only then looks for a match inside
/// that one record — a miss there must mean Incident, never "try the next
/// guard out."
#[tokio::test]
async fn t_err_7_nested_guard_miss_does_not_fall_through_to_outer_catch_all() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Bytecode — each of the three paths (normal, outer-handler,
    // inner-handler) gets its own dedicated `End`: sharing a merge point
    // across paths whose control-stack depth genuinely differs (the inner
    // handler's static entry state, per V-4, inherits the outer guard's
    // still-open token; the outer handler's does not) is a real V-2 CFG
    // conflict, not a fixture-authoring convenience — so this fixture
    // deliberately avoids any merge at all.
    // 0: V2Guard(handler: 8)                       — outer, catch-all
    // 1: V2GuardArmError(None, handler: 8)
    // 2: V2Guard(handler: 10)                       — inner, specific only
    // 3: V2GuardArmError(INNER_CODE, handler: 10)
    // 4: ExecNative(risky_task)
    // 5: V2GuardEnd                                 — close inner
    // 6: V2GuardEnd                                 — close outer
    // 7: End                                        — normal path
    // 8: ExecNative(outer_handler)                  — must NEVER fire here
    // 9: End
    // 10: ExecNative(inner_handler)                 — must NEVER fire here
    // 11: V2GuardEnd  — inner-handler entry (V-4: PRE-push relative to the
    //                    inner guard only) still statically carries the
    //                    outer guard's still-open token; must retire it
    //                    before reaching a scope-external End (V-1).
    // 12: End
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [56u8; 32],
        program: vec![
            Instr::V2Guard { handler: 8.into() }, // 0 — outer
            Instr::V2GuardArmError {
                error_code: None,
                handler: 8.into(),
            }, // 1 — outer catch-all
            Instr::V2Guard { handler: 10.into() }, // 2 — inner
            Instr::V2GuardArmError {
                error_code: Some("INNER_CODE".to_string().into_boxed_str()),
                handler: 10.into(),
            }, // 3 — inner specific-only
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 4
            Instr::V2GuardEnd, // 5 — close inner
            Instr::V2GuardEnd, // 6 — close outer
            Instr::End,        // 7 — normal path
            Instr::ExecNative {
                task_type: 1,
                argc: 0,
                retc: 0,
            }, // 8: outer catch-all handler — must never fire
            Instr::End, // 9
            Instr::ExecNative {
                task_type: 2,
                argc: 0,
                retc: 0,
            }, // 10: inner handler — must never fire (wrong code)
            Instr::V2GuardEnd, // 11 — retire the still-open outer token
            Instr::End,        // 12
        ],
        debug_map: BTreeMap::from([
            (4.into(), "risky_task".to_string()),
            (8.into(), "outer_handler".to_string()),
            (10.into(), "inner_handler".to_string()),
        ]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![
            "risky_task".to_string(),
            "outer_handler".to_string(),
            "inner_handler".to_string(),
        ],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-7",
        )
        .await
        .unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 1);

    // Fail with a code the INNER guard (innermost, so the only one
    // consulted) does not arm — must fall through to Incident, never reach
    // the outer catch-all.
    engine
        .fail_job(
            &jobs[0].job_key,
            ErrorClass::BusinessRejection {
                rejection_code: "UNRELATED_CODE".to_string(),
            },
            "unrelated failure",
        )
        .await
        .unwrap();

    let outer_handler_jobs = store
        .dequeue_jobs(
            &["outer_handler".to_string()],
            10,
            &bpmn_lite_types::TenantId::default(),
            "w",
            300_000,
        )
        .await
        .unwrap();
    assert!(
        outer_handler_jobs.is_empty(),
        "an inner guard's miss must NOT fall through to an outer guard's catch-all"
    );
    let inner_handler_jobs = store
        .dequeue_jobs(
            &["inner_handler".to_string()],
            10,
            &bpmn_lite_types::TenantId::default(),
            "w",
            300_000,
        )
        .await
        .unwrap();
    assert!(
        inner_handler_jobs.is_empty(),
        "the inner guard itself has no matching route either"
    );

    let events = store
        .read_events(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
            0,
        )
        .await
        .unwrap();
    let has_incident = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::IncidentCreated { .. }));
    assert!(
        has_incident,
        "unmatched-at-innermost-guard must create an Incident"
    );
    let has_routed = events
        .iter()
        .any(|(_, e)| matches!(e, RuntimeEvent::ErrorRouted { .. }));
    assert!(
        !has_routed,
        "must not emit ErrorRouted for a miss at the innermost guard"
    );

    let instance = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(instance.state, ProcessState::Incidented { .. }));
}

// ═══════════════════════════════════════════════════════════
//  Phase 5.3: Bounded loops
// ═══════════════════════════════════════════════════════════

/// T-LOOP-1: IncCounter + BrCounterLt retry loop executes exactly N times.
#[tokio::test]
async fn t_loop_1_bounded_retry_executes_n_times() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Simulates: task_a fails → V2GuardArmError route → IncCounter →
    // BrCounterLt(limit=3) → retry (loops back to guard-open, re-arming a
    // FRESH guard scope each iteration) or end. BoundaryError v2 migration:
    // a match fires by spawning a new handler fibre (retiring the failed
    // one), not resuming the same fibre in place — see T-ERR-1's comment
    // for the full mechanism.
    // Bytecode:
    // 0: V2Guard { handler: 5 }                — opens the guard
    // 1: V2GuardArmError(RETRY_ME, handler: 5)  — arms the retry route
    // 2: ExecNative(task_a)                     — parks fiber
    // 3: V2GuardEnd
    // 4: Jump(8)                                 — normal end (skip error handler)
    // 5: IncCounter(0)                           — error handler: bump counter
    // 6: BrCounterLt(0, 3, 0)                   — if counter<3, retry (reopens guard)
    // 7: End                                     — counter exhausted, escalation end
    // 8: End                                     — normal end
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [60u8; 32],
        program: vec![
            Instr::V2Guard { handler: 5.into() }, // 0
            Instr::V2GuardArmError {
                error_code: Some("RETRY_ME".to_string().into_boxed_str()),
                handler: 5.into(),
            }, // 1
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 2
            Instr::V2GuardEnd,                // 3
            Instr::Jump { target: 8.into() }, // 4
            Instr::IncCounter { counter_id: 0 }, // 5
            Instr::BrCounterLt {
                counter_id: 0,
                limit: 3,
                target: 0.into(),
            }, // 6
            Instr::End,                          // 7
            Instr::End,                          // 8
        ],
        debug_map: BTreeMap::from([(2.into(), "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-1",
        )
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
    let instance = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(instance.state, ProcessState::Completed { .. }),
        "Expected Completed after counter exhaustion, got {:?}",
        instance.state
    );

    // Assert: counter value is 3
    assert_eq!(instance.counters.get(&0), Some(&3));

    // Assert: 3 ErrorRouted events
    let events = store
        .read_events(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
            0,
        )
        .await
        .unwrap();
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

    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [61u8; 32],
        program: vec![
            Instr::V2Guard { handler: 5.into() }, // 0
            Instr::V2GuardArmError {
                error_code: None, // catch-all
                handler: 5.into(),
            }, // 1
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 2
            Instr::V2GuardEnd,                // 3
            Instr::Jump { target: 8.into() }, // 4
            Instr::IncCounter { counter_id: 0 }, // 5
            Instr::BrCounterLt {
                counter_id: 0,
                limit: 2,
                target: 0.into(),
            }, // 6
            Instr::End,                          // 7
            Instr::End,                          // 8
        ],
        debug_map: BTreeMap::from([(2.into(), "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    let instance_id = engine
        .start(
            "test",
            program.bytecode_version(),
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-2",
        )
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
#[test]
fn t_loop_3_counter_starts_at_zero() {
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [62u8; 32],
        program: vec![
            Instr::BrCounterLt { counter_id: 5, limit: 1, target: 2.into() },
            Instr::Fail { code: 99 },
            Instr::End,
        ],
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    let artifact = ExecutableWorkflow::from_verified_envelope(
        ArtifactEnvelope::from_legacy_program(program, "test").unwrap(),
    )
    .unwrap();
    let instance_id = Uuid::from_u128(1);
    let fiber_id = Uuid::from_u128(2);
    let instance = ProcessInstance {
        instance_id,
        process_key: "test".to_string(),
        bytecode_version: artifact.hash().into_bytes(),
        tenant_id: "default".to_string(),
        domain_payload: "{}".to_string().into(),
        domain_payload_hash: [0u8; 32],
        session_stack: SessionStackState::default(),
        flags: BTreeMap::new(),
        counters: BTreeMap::new(),
        join_expected: BTreeMap::new(),
        state: ProcessState::Running,
        correlation_id: "corr".to_string(),
        entry_id: Uuid::nil(),
        runbook_id: Uuid::nil(),
        created_at: 0,
        integrity_hash: None,
        quarantine_state: None,
        plan_hash: None,
        current_node_id: None,
        placeholder_values: None,
    };
    let snapshot = Snapshot::new(instance, [Fiber::new(fiber_id, 0)]);
    let transition = bpmn_lite_kernel::apply(
        &artifact,
        &snapshot,
        &Command::Tick {
            fiber_id: Some(fiber_id),
        },
        &bpmn_lite_kernel::DeterministicContext::new(10, Uuid::from_u128(3), 1),
    )
    .unwrap();
    assert_eq!(transition.fibers_delete(), &[fiber_id]);
    assert!(matches!(
        transition.next_snapshot().state,
        ProcessState::Completed { .. }
    ));
}

/// T-LOOP-4: Bytecode verifier rejects unguarded backward Jump.
#[tokio::test]
async fn t_loop_4_verifier_rejects_backward_jump() {
    let program = bpmn_lite_types::legacy_program! {
        bytecode_version: [63u8; 32],
        program: vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 0
            Instr::Jump { target: 0.into() }, // 1: backward jump! infinite loop
            Instr::End,                // 2: unreachable
        ],
        debug_map: BTreeMap::from([(0.into(), "task_a".to_string())]),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };

    let errors = bpmn_lite_compiler::verify_bytecode(&program);
    assert!(!errors.is_empty(), "Should reject backward Jump");
    assert!(
        errors[0].message.contains("Backward jump"),
        "Error should mention backward jump: {}",
        errors[0].message
    );
}

// T-LOOP-5 (retired, G3.3, 2026-08-11): "Bytecode verifier allows
// BrCounterLt backward jump" asserted the whitelist carve-out
// `bpmn-lite-compiler/src/verifier.rs::verify_bytecode` used to grant
// `BrCounterLt`. That carve-out is deleted — both front-ends now emit
// acyclic output unconditionally (`bpmn-lite-compiler/src/dsl/unroll.rs`
// expands every bounded loop to forward-only copies before verification
// ever runs), so there is no legitimate producer of a backward
// `BrCounterLt` left, and the assertion this test made ("should be
// allowed") is now the wrong theorem. Replaced below with the proof that
// supersedes it: a backward `BrCounterLt` is rejected exactly like any
// other backward branch, same diagnostic as T-LOOP-4.
/// T-LOOP-5 (replacement): bytecode verifier rejects a backward
/// `BrCounterLt` the same as any other backward branch — the whitelist
/// this used to require is gone (G3.3).
#[tokio::test]
async fn t_loop_5_verifier_rejects_br_counter_lt_backward_post_g3() {
    let program = bpmn_lite_types::legacy_program! {
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
                target: 0.into(),
            }, // 2: backward — no longer whitelisted
            Instr::End,                          // 3
        ],
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec!["task_a".to_string()],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };

    let errors = bpmn_lite_compiler::verify_bytecode(&program);
    assert!(!errors.is_empty(), "backward BrCounterLt should now be rejected");
    assert!(
        errors[0].message.contains("Backward jump"),
        "Error should mention backward jump: {}",
        errors[0].message
    );
}

// ═══════════════════════════════════════════════════════════
//  Phase 5A: Inclusive gateway
// ═══════════════════════════════════════════════════════════

// V5.3 (§18, landed 2026-07-23): T-IG-1 through T-IG-5 are deleted along
// with `Instr::ForkInclusive`/`JoinDynamic` (v1) — both variants are gone
// from the `Instr` enum entirely this step, and these five tests each
// hand-assembled a `legacy_program!` fixture constructing one directly
// (via the real engine, not through `lower()`), so there is no mechanical
// way to keep them compiling, let alone passing. Checked before deleting,
// not assumed harmless: each has an already-landed v2 engine-level test
// (§18 ruling I item (e), landed the same day) proving the identical
// behavioural property against the mechanism that is now the only one
// that exists —
//   T-IG-1 (all conditions truthy → all branches run → join waits for
//     all → completes)            → t_ig_v2_all_matched_branches_run_concurrently_and_join_completes
//   T-IG-2 (one of three truthy → one branch runs, others skip to join)
//                                  → t_ig_v2_single_matched_branch_skips_the_other_to_join
//   T-IG-3 (zero match, no default → Incident, not a hard error)
//                                  → t_ig_v2_zero_match_no_default_raises_incident
//   T-IG-4 (zero match, default present → default branch runs)
//                                  → t_ig_v2_zero_match_with_default_runs_default_branch
//   T-IG-5 (join releases at exactly the dynamic arrival count, not a
//     hardcoded one) — no single same-named v2 test, but the property is
//     inherent to `V2Join`'s barrier-arity mechanism (not a per-gateway
//     count computed and compared, the way v1's `JoinDynamic` needed
//     `join_expected` bookkeeping to reproduce) and is exercised by every
//     one of the four tests above, each of which drives a `V2Join` to
//     release at its own branch count (1, 3, or the default-branch count
//     of 1) and asserts completion — the same "waits for exactly this
//     many, not a stale hardcoded number" property, proven structurally
//     rather than by a dedicated count-varying fixture.
// This is a consolidation, not a silent coverage loss — the same
// properties, proven against the only mechanism that runs after this
// step's `lower()` default flip, per the same "if redundant, consolidate
// honestly" discipline this landing's own T-IG-6/t_auth_6_boundary_timer_
// yaml relocks follow.

/// T-IG-6: Full compiler pipeline — parse inclusiveGateway from BPMN XML.
/// Relocked in place (V5.3, §18, landed 2026-07-23, `lower()` default
/// flip, Part A): previously locked `lower()`'s (v1) literal output —
/// `Instr::ForkInclusive`/`JoinDynamic` present in the compiled program.
/// `lower()` now emits v2 words unconditionally for every construct
/// (Part A's core change) and both v1 variants are deleted entirely
/// (Part B) — same structural-lock rigor, new shape: `Instr::V2Fork`/
/// `V2Join` present, with `V2Fork`'s target count matching the gateway's
/// declared branch count (2: `task_a` unconditional, `task_b`
/// conditional).
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

    // Should contain V2Fork and V2Join instructions (v2 dynamic-arity
    // skip-to-join lowering, ruling H) — v1's ForkInclusive/JoinDynamic no
    // longer exist as types to construct.
    let fork_targets = program.program().iter().find_map(|i| match i {
        Instr::V2Fork { targets, .. } => Some(targets.len()),
        _ => None,
    });
    assert_eq!(
        fork_targets,
        Some(2),
        "Should contain a V2Fork with 2 targets (task_a unconditional, task_b conditional)"
    );

    let has_join = program
        .program()
        .iter()
        .any(|i| matches!(i, Instr::V2Join { .. }));
    assert!(has_join, "Should contain a V2Join instruction");
}

// ═══════════════════════════════════════════════════════════
//  V5 post-close (§18 rulings H/I/J) — v2 inclusive-gateway lowering
//  (`lowering::lower_v2`), driven end-to-end through the real engine.
//  `lower()`/`engine.compile` (T-IG-6 above) are untouched by this step —
//  these fixtures exercise the separate `lower_v2` entry point instead,
//  proving the SAME BPMN construct lowers to genuinely different, both
//  independently correct, bytecode depending which function compiles it.
// ═══════════════════════════════════════════════════════════

fn inclusive_gateway_v2_xml(default_flow: bool) -> String {
    let default_edge = if default_flow {
        r#"<bpmn:sequenceFlow id="f_default" sourceRef="ig_fork" targetRef="always"/>"#
    } else {
        ""
    };
    let default_task = if default_flow {
        r#"<bpmn:serviceTask id="always"><bpmn:extensionElements><zeebe:taskDefinition type="always_task"/></bpmn:extensionElements></bpmn:serviceTask>
           <bpmn:sequenceFlow id="f_always_join" sourceRef="always" targetRef="ig_join"/>"#
    } else {
        ""
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:inclusiveGateway id="ig_fork" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_a"><bpmn:extensionElements><zeebe:taskDefinition type="identity_check"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_b"><bpmn:extensionElements><zeebe:taskDefinition type="edd_check"/></bpmn:extensionElements></bpmn:serviceTask>
    {default_task}
    <bpmn:inclusiveGateway id="ig_join" gatewayDirection="Converging"/>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="ig_fork"/>
    <bpmn:sequenceFlow id="f2" sourceRef="ig_fork" targetRef="task_a">
      <bpmn:conditionExpression>= high_risk == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f3" sourceRef="ig_fork" targetRef="task_b">
      <bpmn:conditionExpression>= pep_flagged == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    {default_edge}
    <bpmn:sequenceFlow id="f4" sourceRef="task_a" targetRef="ig_join"/>
    <bpmn:sequenceFlow id="f5" sourceRef="task_b" targetRef="ig_join"/>
    <bpmn:sequenceFlow id="f6" sourceRef="ig_join" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#
    )
}

async fn compile_inclusive_gateway_v2(default_flow: bool) -> (Arc<MemoryStore>, [u8; 32]) {
    let store = Arc::new(MemoryStore::new());
    let xml = inclusive_gateway_v2_xml(default_flow);
    let graph = bpmn_lite_compiler::parse_bpmn(&xml).unwrap();
    let workflow = bpmn_lite_compiler::Compiler::lower_v2(&graph)
        .expect("v2 inclusive-gateway lowering must verify");
    store.store_artifact(&workflow).await.unwrap();
    (store, workflow.hash().into_bytes())
}

/// (a)/(b) — two branches truthy: real concurrent work happens on both
/// matched branches (2 jobs activated), the shared `V2Join` releases only
/// once both arrive, and the instance completes.
#[tokio::test]
async fn t_ig_v2_all_matched_branches_run_concurrently_and_join_completes() {
    let (store, bytecode_version) = compile_inclusive_gateway_v2(false).await;
    let engine = BpmnLiteEngine::new(store.clone());

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-v2-ig-1",
        )
        .await
        .unwrap();
    {
        let mut inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
        inst.flags.insert(0, Value::Bool(true)); // high_risk
        inst.flags.insert(1, Value::Bool(true)); // pep_flagged
        bpmn_lite_store::store::commit_snapshot(store.as_ref(), "test", inst)
            .await
            .unwrap();
    }

    engine.tick_instance(instance_id).await.unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(
        jobs.len(),
        2,
        "both matched branches must do real concurrent work"
    );

    for job in &jobs {
        let payload = "{}";
        let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
        engine
            .complete_job(&job.job_key, payload, hash, BTreeMap::new())
            .await
            .unwrap();
    }

    let mut inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    for _ in 0..5 {
        if inst.state.is_terminal() {
            break;
        }
        engine.tick_instance(instance_id).await.unwrap();
        inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
    }
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// (b) — only one of two conditions truthy: one branch does real work, the
/// other skips straight to the shared `V2Join` (the proven dynamic-arity
/// pattern) — one job activated, not two, and the instance still completes.
#[tokio::test]
async fn t_ig_v2_single_matched_branch_skips_the_other_to_join() {
    let (store, bytecode_version) = compile_inclusive_gateway_v2(false).await;
    let engine = BpmnLiteEngine::new(store.clone());

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-v2-ig-2",
        )
        .await
        .unwrap();
    {
        let mut inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
        inst.flags.insert(0, Value::Bool(true)); // high_risk only
        bpmn_lite_store::store::commit_snapshot(store.as_ref(), "test", inst)
            .await
            .unwrap();
    }

    engine.tick_instance(instance_id).await.unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 1, "only the matched branch should do real work");

    let payload = "{}";
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    engine
        .complete_job(&jobs[0].job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();

    let mut inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    for _ in 0..5 {
        if inst.state.is_terminal() {
            break;
        }
        engine.tick_instance(instance_id).await.unwrap();
        inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
    }
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// (c) — zero branches truthy, no default/always-live edge: the
/// synchronous pre-`V2Fork` zero-match check (`V2RouteZeroMatch`) raises
/// an Incident — `ProcessState::Incidented`, matching what `T-IG-3` (v1)
/// proves for `ForkInclusive`'s own zero-match arm.
#[tokio::test]
async fn t_ig_v2_zero_match_no_default_raises_incident() {
    let (store, bytecode_version) = compile_inclusive_gateway_v2(false).await;
    let engine = BpmnLiteEngine::new(store.clone());

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-v2-ig-3",
        )
        .await
        .unwrap();
    // Neither flag set — both conditions false, no default edge.
    engine.tick_instance(instance_id).await.unwrap();

    let inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(inst.state, ProcessState::Incidented { .. }),
        "expected Incidented, got {:?}",
        inst.state
    );
}

/// (d) — zero branches truthy WITH an always-live (default) edge: the
/// zero-match precheck is never emitted at all (compile-time proof the
/// fork's target set is non-empty), and the default branch runs for real.
#[tokio::test]
async fn t_ig_v2_zero_match_with_default_runs_default_branch() {
    let (store, bytecode_version) = compile_inclusive_gateway_v2(true).await;
    let engine = BpmnLiteEngine::new(store.clone());

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-v2-ig-4",
        )
        .await
        .unwrap();
    // Neither flag set — the always-live branch is taken regardless.
    engine.tick_instance(instance_id).await.unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(
        jobs.len(),
        1,
        "only the always-live/default branch should run"
    );
    assert_eq!(jobs[0].task_type, "always_task");

    let payload = "{}";
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    engine
        .complete_job(&jobs[0].job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();

    let mut inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    for _ in 0..5 {
        if inst.state.is_terminal() {
            break;
        }
        engine.tick_instance(instance_id).await.unwrap();
        inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
    }
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

// ═══════════════════════════════════════════════════════════
//  Multi-pair GatewayInclusive — end-to-end runtime proof (Direction A,
//  `docs/todo/EOP-VS-BPMN-ISA-002.md` §19; verifier.rs's "9. Inclusive
//  gateway validation" count-based rejection and
//  `bpmn-lite-authoring/src/validate.rs`'s V10 both lifted). The tests
//  above (`t_ig_v2_*`) each drive exactly ONE `GatewayInclusive` pair
//  through the real engine; `lowering.rs`'s
//  `test_two_sequential_inclusive_pairs_lower_correctly` and
//  `test_two_independently_nested_inclusive_pairs_pair_correctly` prove
//  multi-pair COMPILATION correctness (bytecode-level `V2Join.pairing`
//  assertions) but never execute a single instruction. These tests close
//  that gap: real `tick_instance`/`run_instance`/`complete_job` traces,
//  asserting on job activation counts, PER-ROUND activation ordering (to
//  prove sequential/independent routing, not just eventual completion),
//  and final `ProcessState`.
// ═══════════════════════════════════════════════════════════

/// Drains an instance to completion, driving `run_instance`/`complete_job`
/// in a loop (each `run_instance` call ticks once and dequeues whatever
/// became runnable), completing every activated job immediately. Returns
/// `(round, task_type)` for every job activated, in activation order — the
/// round number is what lets a test assert TWO pairs' branches were
/// activated in different rounds (proving one pair's join gated the next
/// pair's fork opening), not merely that both eventually ran.
async fn drain_and_complete_all(
    engine: &BpmnLiteEngine,
    store: &MemoryStore,
    instance_id: Uuid,
) -> Vec<(usize, String)> {
    let mut activations = Vec::new();
    for round in 0..20usize {
        let jobs = engine.run_instance(instance_id).await.unwrap();
        if jobs.is_empty() {
            let inst = store
                .load_instance(
                    &bpmn_lite_types::TenantId::new("default").unwrap(),
                    instance_id,
                )
                .await
                .unwrap()
                .unwrap();
            if inst.state.is_terminal() {
                break;
            }
            continue;
        }
        for job in &jobs {
            activations.push((round, job.task_type.clone()));
            let payload = "{}";
            let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
            engine
                .complete_job(&job.job_key, payload, hash, BTreeMap::new())
                .await
                .unwrap();
        }
    }
    activations
}

/// Two SEQUENTIAL `GatewayInclusive` pairs in one process (pair 1 fully
/// resolves and joins, THEN pair 2 opens and resolves independently) —
/// driven through the REAL frontend (`engine.compile`, parsing BPMN XML),
/// matching `T-IG-6`'s "prove the real frontend reaches this, not just
/// hand-assembled bytecode" discipline, and then executed end-to-end
/// through the real engine (not just compiled). All four branch flags are
/// set truthy up front; the proof is in the ROUND STRUCTURE of job
/// activation, not just eventual completion: pair 1's two branches
/// (`task_a1`/`task_b1`) must both be activated (and both completed)
/// strictly before EITHER of pair 2's branches (`task_a2`/`task_b2`) is
/// activated — pair 2's `GatewayInclusive` fork cannot open until pair 1's
/// `GatewayInclusive` join has released, exactly the barrier semantics
/// `V2Join`/`V2Fork` are supposed to provide, now proven for two
/// INDEPENDENT pairs sharing one process rather than one pair alone.
#[tokio::test]
async fn t_ig_v2_two_sequential_pairs_route_and_join_independently() {
    let bpmn_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:inclusiveGateway id="ig_fork1" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_a1"><bpmn:extensionElements><zeebe:taskDefinition type="task_a1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_b1"><bpmn:extensionElements><zeebe:taskDefinition type="task_b1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:inclusiveGateway id="ig_join1" gatewayDirection="Converging"/>
    <bpmn:inclusiveGateway id="ig_fork2" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_a2"><bpmn:extensionElements><zeebe:taskDefinition type="task_a2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_b2"><bpmn:extensionElements><zeebe:taskDefinition type="task_b2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:inclusiveGateway id="ig_join2" gatewayDirection="Converging"/>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="ig_fork1"/>
    <bpmn:sequenceFlow id="f2" sourceRef="ig_fork1" targetRef="task_a1">
      <bpmn:conditionExpression>= flag_a1 == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f3" sourceRef="ig_fork1" targetRef="task_b1">
      <bpmn:conditionExpression>= flag_b1 == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f4" sourceRef="task_a1" targetRef="ig_join1"/>
    <bpmn:sequenceFlow id="f5" sourceRef="task_b1" targetRef="ig_join1"/>
    <bpmn:sequenceFlow id="f6" sourceRef="ig_join1" targetRef="ig_fork2"/>
    <bpmn:sequenceFlow id="f7" sourceRef="ig_fork2" targetRef="task_a2">
      <bpmn:conditionExpression>= flag_a2 == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f8" sourceRef="ig_fork2" targetRef="task_b2">
      <bpmn:conditionExpression>= flag_b2 == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f9" sourceRef="task_a2" targetRef="ig_join2"/>
    <bpmn:sequenceFlow id="f10" sourceRef="task_b2" targetRef="ig_join2"/>
    <bpmn:sequenceFlow id="f11" sourceRef="ig_join2" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#;

    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let compiled = engine
        .compile(bpmn_xml)
        .await
        .expect("two sequential inclusive-gateway pairs must compile via the real frontend");

    let flag_key = |name: &str| -> FlagKey {
        *compiled
            .flag_symbol_table
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(k, _)| k)
            .unwrap_or_else(|| panic!("{name} must be interned as a flag"))
    };

    let instance_id = engine
        .start(
            "test",
            compiled.bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-v2-ig-seq",
        )
        .await
        .unwrap();
    {
        let mut inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
        inst.flags.insert(flag_key("flag_a1"), Value::Bool(true));
        inst.flags.insert(flag_key("flag_b1"), Value::Bool(true));
        inst.flags.insert(flag_key("flag_a2"), Value::Bool(true));
        inst.flags.insert(flag_key("flag_b2"), Value::Bool(true));
        bpmn_lite_store::store::commit_snapshot(store.as_ref(), "test", inst)
            .await
            .unwrap();
    }

    let activations = drain_and_complete_all(&engine, &store, instance_id).await;

    let round_of = |task_type: &str| -> usize {
        activations
            .iter()
            .find(|(_, t)| t == task_type)
            .map(|(r, _)| *r)
            .unwrap_or_else(|| panic!("{task_type} was never activated, got: {activations:?}"))
    };
    assert_eq!(
        activations.len(),
        4,
        "all four branch tasks (and only those four) must be activated exactly once, got: {activations:?}"
    );
    let pair1_last_round = round_of("task_a1").max(round_of("task_b1"));
    let pair2_first_round = round_of("task_a2").min(round_of("task_b2"));
    assert!(
        pair1_last_round < pair2_first_round,
        "pair 1's branches (rounds {}/{}) must both complete strictly before pair 2's fork opens \
         (rounds {}/{}) — pair 2's GatewayInclusive must not open until pair 1's join releases, \
         got: {activations:?}",
        round_of("task_a1"),
        round_of("task_b1"),
        round_of("task_a2"),
        round_of("task_b2")
    );

    let inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// FIXED regression test (was a known K-1 bug; see the FIXED note below).
/// Runs live — no longer `#[ignore]`d — and is a permanent reproduction.
///
/// Two INDEPENDENTLY-NESTED `GatewayInclusive` pairs, one inside EACH
/// branch of an outer `GatewayAnd` fork — the runtime equivalent of
/// `lowering.rs`'s `test_two_independently_nested_inclusive_pairs_pair_
/// correctly` (which proves only compilation) and `verifier.rs`'s
/// `test_two_nested_inclusive_pairs_in_and_branches_now_admitted` (which
/// proves only admission). Neither of those tests ever executes a single
/// instruction; driving this exact topology through the REAL engine
/// (built for this task, per the brief's item 4) surfaced a genuine,
/// reproducible kernel-level defect, INDEPENDENT of Direction A's
/// pairing-derivation fix (`compute_gateway_pairing`/
/// `check_gateway_and_nesting`, both of which are innocent here — this is
/// not a mispairing, and it reproduces with branch flags set either way):
///
/// Once a `GatewayInclusive` nested inside one branch of an outer
/// `GatewayAnd` fork takes its dynamic-arity "skip-to-join" path (fewer
/// branches matched than declared — the same mechanism
/// `t_ig_v2_single_matched_branch_skips_the_other_to_join` proves correct
/// for a STANDALONE (non-nested) inclusive gateway), completing that
/// branch's job and ticking again raises `Ring 3 runtime integrity
/// violation: K-1 violated: record <id> (armed) has member <id>, no live
/// fibre` (`bpmn-lite-kernel/src/lib.rs`'s `check_k_invariants`) — some
/// concurrency-table record is left listing a fibre that no longer exists.
/// Confirmed by hand-reduction: reproduces with EITHER branch (or both)
/// taking the skip-to-join path, so it is not specific to the
/// single-match/all-match ASYMMETRY between sibling branches — the trigger
/// is "a nested inclusive gateway under an AND fork resolves with dynamic
/// arity less than its declared branch count," full stop. Not reproduced
/// (and not expected to reproduce, per the passing
/// `t_ig_v2_two_sequential_pairs_route_and_join_independently` test above)
/// for sequential (non-nested) multi-pair topology — this looks specific
/// to the interaction between the outer AND-fork's own barrier/fibre
/// bookkeeping and an inner inclusive gateway's dynamic-arity skip, in
/// `bpmn-lite-kernel`, not to anything `verifier.rs`/`lowering.rs` control.
///
/// A separate, unrelated, pre-existing bug was ALSO found while isolating
/// this one (`lowering.rs`'s `topo_order`, the naive-BFS address-layout
/// backward-jump bug) — fixed separately, see the address-layout fix's own
/// post-close entry; this test's branch shapes remain length-balanced from
/// that investigation but no longer need to be, now that both bugs are
/// fixed.
///
/// **FIXED 2026-07-24 — root cause confirmed the SAME as
/// `t_and_v2_nested_gateway_inside_branch_compiles_and_completes`'s
/// (`bpmn-lite-kernel/src/lib.rs`), not a separate inclusive-gateway-
/// specific defect**, exactly per Adam's own prediction ("it may be a
/// symptom rather than a separate defect"): `apply_tick` runs one fibre
/// across potentially many instructions in a single transition without
/// re-snapshotting between them. A nested barrier's survivor can pop an
/// inner `V2Join` (correctly reconciling the OUTER barrier's own
/// membership via `v2_reconcile_ancestor_membership`, staged into
/// `changes`) and then, without blocking, immediately execute the OUTER
/// `V2Join` in the SAME transition — which used to read the outer
/// record straight from `snapshot` (fixed at the transition's start,
/// blind to this transition's own in-flight writes) and silently
/// overwrite the just-staged reconciliation with its own stale
/// re-`Insert` (last-write-wins on `Insert`-by-key). Fixed by
/// `fetch_record_in_transition`, a shared pending-aware lookup (mirroring
/// `v2_reconcile_ancestor_membership`'s own pre-existing "check `changes`
/// before `snapshot`" idiom) now used at every mid-transition record read.
/// Independently confirmed via 100 standalone process runs post-fix, zero
/// failures (pre-fix: intermittent, ~1-in-15 to 1-in-40, since which fibre
/// is selected first — and hence whether both `V2Join`s land in one
/// transition — depends on `BTreeMap`-order-of-derived-UUIDs, which varies
/// by process run).
#[tokio::test]
async fn t_ig_v2_two_nested_inclusive_pairs_in_and_branches_route_independently() {
    let bpmn_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:parallelGateway id="and_fork" gatewayDirection="Diverging"/>
    <bpmn:inclusiveGateway id="ig_fork_a" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_a1"><bpmn:extensionElements><zeebe:taskDefinition type="task_a1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_a2"><bpmn:extensionElements><zeebe:taskDefinition type="task_a2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:inclusiveGateway id="ig_join_a" gatewayDirection="Converging"/>
    <bpmn:inclusiveGateway id="ig_fork_b" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_b1"><bpmn:extensionElements><zeebe:taskDefinition type="task_b1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_b2"><bpmn:extensionElements><zeebe:taskDefinition type="task_b2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:inclusiveGateway id="ig_join_b" gatewayDirection="Converging"/>
    <bpmn:parallelGateway id="and_join" gatewayDirection="Converging"/>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f0" sourceRef="start" targetRef="and_fork"/>
    <bpmn:sequenceFlow id="fa0" sourceRef="and_fork" targetRef="ig_fork_a"/>
    <bpmn:sequenceFlow id="fa1" sourceRef="ig_fork_a" targetRef="task_a1">
      <bpmn:conditionExpression>= flag_a1 == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="fa2" sourceRef="ig_fork_a" targetRef="task_a2">
      <bpmn:conditionExpression>= flag_a2 == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="fa3" sourceRef="task_a1" targetRef="ig_join_a"/>
    <bpmn:sequenceFlow id="fa4" sourceRef="task_a2" targetRef="ig_join_a"/>
    <bpmn:sequenceFlow id="fa5" sourceRef="ig_join_a" targetRef="and_join"/>
    <bpmn:sequenceFlow id="fb0" sourceRef="and_fork" targetRef="ig_fork_b"/>
    <bpmn:sequenceFlow id="fb1" sourceRef="ig_fork_b" targetRef="task_b1">
      <bpmn:conditionExpression>= flag_b1 == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="fb2" sourceRef="ig_fork_b" targetRef="task_b2">
      <bpmn:conditionExpression>= flag_b2 == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="fb3" sourceRef="task_b1" targetRef="ig_join_b"/>
    <bpmn:sequenceFlow id="fb4" sourceRef="task_b2" targetRef="ig_join_b"/>
    <bpmn:sequenceFlow id="fb5" sourceRef="ig_join_b" targetRef="and_join"/>
    <bpmn:sequenceFlow id="fend" sourceRef="and_join" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#;

    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let compiled = engine.compile(bpmn_xml).await.expect(
        "GatewayAnd fork with an independently-nested GatewayInclusive pair in EACH branch must compile",
    );

    let flag_key = |name: &str| -> FlagKey {
        *compiled
            .flag_symbol_table
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(k, _)| k)
            .unwrap_or_else(|| panic!("{name} must be interned as a flag"))
    };

    let instance_id = engine
        .start(
            "test",
            compiled.bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-v2-ig-nested-and",
        )
        .await
        .unwrap();
    {
        let mut inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
        // Branch A: single match (task_a2's flag left unset) — its inner
        // GatewayInclusive must skip task_a2 straight to ig_join_a.
        inst.flags.insert(flag_key("flag_a1"), Value::Bool(true));
        // Branch B: both match — its inner GatewayInclusive must run both
        // branches concurrently before ig_join_b releases.
        inst.flags.insert(flag_key("flag_b1"), Value::Bool(true));
        inst.flags.insert(flag_key("flag_b2"), Value::Bool(true));
        bpmn_lite_store::store::commit_snapshot(store.as_ref(), "test", inst)
            .await
            .unwrap();
    }

    let activations = drain_and_complete_all(&engine, &store, instance_id).await;
    let activated: std::collections::BTreeSet<&str> =
        activations.iter().map(|(_, t)| t.as_str()).collect();

    assert_eq!(
        activations.len(),
        3,
        "branch A must activate exactly 1 job (single-match skip-to-join) and branch B exactly \
         2 (all-match concurrent) — 3 total, got: {activations:?}"
    );
    assert!(
        activated.contains("task_a1") && !activated.contains("task_a2"),
        "branch A's inner GatewayInclusive must run only task_a1 (its own routing decision), \
         got: {activations:?}"
    );
    assert!(
        activated.contains("task_b1") && activated.contains("task_b2"),
        "branch B's inner GatewayInclusive must run both task_b1 and task_b2 (its own, \
         independent routing decision — unaffected by branch A's single-match outcome), \
         got: {activations:?}"
    );

    let inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed (outer GatewayAnd join must still release once both inner \
         GatewayInclusive joins have, each at its own address) — got {:?}",
        inst.state
    );
}

// ═══════════════════════════════════════════════════════════════════════
// `lowering::topo_order` backward-jump fix (structured/region-aware
// address layout, replacing plain BFS) — see `structured_order`'s doc
// comment in `bpmn-lite-compiler/src/lowering.rs` for the full design
// record. The fixture immediately below is the EXACT minimal reproduction
// reported alongside `t_ig_v2_two_nested_inclusive_pairs_in_and_branches_
// route_independently` above (that test's own doc comment references it):
// a bare `GatewayAnd` fork, one 3-task branch, one 1-task sibling branch —
// no `GatewayInclusive`, no multiple pairs — compiled via `engine.compile`
// (the real XML frontend, which runs `verify_bytecode`). Before the fix
// this failed outright with "Backward jump at addr 12 to 6 — only
// BrCounterLt may jump backward"; confirmed genuinely red by temporarily
// reverting `structured_order`/`compute_region_map` back to the old
// `topo_order` BFS and re-running this exact test.
// ═══════════════════════════════════════════════════════════════════════

/// Red-before/green-after reproduction for the reported bug: a bare
/// `GatewayAnd` fork with unequal-length branches (3 tasks vs. 1 task) must
/// compile through the real XML frontend without a backward-jump rejection.
#[tokio::test]
async fn t_and_v2_unequal_branch_lengths_compiles_and_completes() {
    let bpmn_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:parallelGateway id="and_fork" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_a1"><bpmn:extensionElements><zeebe:taskDefinition type="task_a1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_a2"><bpmn:extensionElements><zeebe:taskDefinition type="task_a2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_a3"><bpmn:extensionElements><zeebe:taskDefinition type="task_a3"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_b1"><bpmn:extensionElements><zeebe:taskDefinition type="task_b1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:parallelGateway id="and_join" gatewayDirection="Converging"/>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f0" sourceRef="start" targetRef="and_fork"/>
    <bpmn:sequenceFlow id="fa0" sourceRef="and_fork" targetRef="task_a1"/>
    <bpmn:sequenceFlow id="fa1" sourceRef="task_a1" targetRef="task_a2"/>
    <bpmn:sequenceFlow id="fa2" sourceRef="task_a2" targetRef="task_a3"/>
    <bpmn:sequenceFlow id="fa3" sourceRef="task_a3" targetRef="and_join"/>
    <bpmn:sequenceFlow id="fb0" sourceRef="and_fork" targetRef="task_b1"/>
    <bpmn:sequenceFlow id="fb1" sourceRef="task_b1" targetRef="and_join"/>
    <bpmn:sequenceFlow id="fend" sourceRef="and_join" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#;

    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let compiled = engine
        .compile(bpmn_xml)
        .await
        .expect("bare GatewayAnd fork with unequal-length branches (3 tasks vs. 1) must compile");

    let instance_id = engine
        .start(
            "test",
            compiled.bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-and-unequal-1",
        )
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();
    let first_jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(
        first_jobs.len(),
        2,
        "both branches must activate concurrent work"
    );

    let mut inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    let mut pending = first_jobs;
    for _ in 0..10 {
        if inst.state.is_terminal() {
            break;
        }
        for job in &pending {
            let payload = "{}";
            let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
            engine
                .complete_job(&job.job_key, payload, hash, BTreeMap::new())
                .await
                .unwrap();
        }
        engine.tick_instance(instance_id).await.unwrap();
        pending = engine.run_instance(instance_id).await.unwrap();
        inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
    }
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// Broader sweep (1): THREE branches of three DIFFERENT lengths (1, 2, and
/// 4 tasks) off a single `GatewayAnd` fork — not just the minimal two-branch
/// case above.
#[tokio::test]
async fn t_and_v2_three_branches_three_different_lengths_compiles_and_completes() {
    let bpmn_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:parallelGateway id="and_fork" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_a1"><bpmn:extensionElements><zeebe:taskDefinition type="task_a1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_b1"><bpmn:extensionElements><zeebe:taskDefinition type="task_b1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_b2"><bpmn:extensionElements><zeebe:taskDefinition type="task_b2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_c1"><bpmn:extensionElements><zeebe:taskDefinition type="task_c1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_c2"><bpmn:extensionElements><zeebe:taskDefinition type="task_c2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_c3"><bpmn:extensionElements><zeebe:taskDefinition type="task_c3"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_c4"><bpmn:extensionElements><zeebe:taskDefinition type="task_c4"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:parallelGateway id="and_join" gatewayDirection="Converging"/>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f0" sourceRef="start" targetRef="and_fork"/>
    <bpmn:sequenceFlow id="fa0" sourceRef="and_fork" targetRef="task_a1"/>
    <bpmn:sequenceFlow id="fa1" sourceRef="task_a1" targetRef="and_join"/>
    <bpmn:sequenceFlow id="fb0" sourceRef="and_fork" targetRef="task_b1"/>
    <bpmn:sequenceFlow id="fb1" sourceRef="task_b1" targetRef="task_b2"/>
    <bpmn:sequenceFlow id="fb2" sourceRef="task_b2" targetRef="and_join"/>
    <bpmn:sequenceFlow id="fc0" sourceRef="and_fork" targetRef="task_c1"/>
    <bpmn:sequenceFlow id="fc1" sourceRef="task_c1" targetRef="task_c2"/>
    <bpmn:sequenceFlow id="fc2" sourceRef="task_c2" targetRef="task_c3"/>
    <bpmn:sequenceFlow id="fc3" sourceRef="task_c3" targetRef="task_c4"/>
    <bpmn:sequenceFlow id="fc4" sourceRef="task_c4" targetRef="and_join"/>
    <bpmn:sequenceFlow id="fend" sourceRef="and_join" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#;

    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let compiled = engine
        .compile(bpmn_xml)
        .await
        .expect("GatewayAnd fork with three differently-lengthed branches must compile");

    let instance_id = engine
        .start(
            "test",
            compiled.bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-and-3way-1",
        )
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    // Drain jobs across ticks until all three branches have completed and
    // the instance reaches a terminal state (branch C needs 4 sequential
    // jobs, one per tick-drain round).
    let mut inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    for _ in 0..20 {
        if inst.state.is_terminal() {
            break;
        }
        let jobs = engine.run_instance(instance_id).await.unwrap();
        for job in &jobs {
            let payload = "{}";
            let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
            engine
                .complete_job(&job.job_key, payload, hash, BTreeMap::new())
                .await
                .unwrap();
        }
        engine.tick_instance(instance_id).await.unwrap();
        inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
    }
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// Broader sweep (2): a branch containing a NESTED further `GatewayAnd`
/// fork/join pair (own unequal-length sub-branches), sibling to a plain
/// longer linear branch — exercises the region-boundary recursion, not
/// just a flat two-branch shape.
///
/// **Independently confirmed 2026-07-24, INTERMITTENT (not deterministic —
/// an earlier note calling this "5/5 deterministic" was a misread; actual
/// behavior is ~1-in-15 to 1-in-40 process runs, since it depends on
/// which fibre `apply_tick` selects first, which in turn depends on
/// `BTreeMap`-order-of-derived-UUIDs, which varies by process run):**
/// `engine.compile` and the first several execution rounds succeed —
/// proving the address-layout fix itself works correctly here, this
/// topology could not even compile before that fix — but a later tick
/// used to raise `Ring 3 runtime integrity violation: K-1 violated: record
/// <id> (armed) has member <id>, no live fibre`, the SAME error class as
/// `t_ig_v2_two_nested_inclusive_pairs_in_and_branches_route_
/// independently`.
///
/// **This significantly broadened that bug's known scope**, exactly as
/// Adam predicted ("its trigger is broader than I guessed"). The prior
/// finding was framed as "a `GatewayInclusive` nested under a `GatewayAnd`
/// branch, resolving with dynamic arity BELOW its declared count" —
/// implicitly scoping it to inclusive-gateway dynamic-arity mismatch. This
/// fixture has NO `GatewayInclusive` anywhere and NO arity mismatch —
/// every branch of both the outer and inner `GatewayAnd` forks always
/// runs, nothing skips. The trigger is "a `GatewayAnd` fork nested inside
/// one branch of another `GatewayAnd` fork" — a barrier nested inside a
/// barrier — full stop.
///
/// **FIXED 2026-07-24** (`bpmn-lite-kernel/src/lib.rs`): confirmed, by
/// adding temporary tracing and running to a captured failure, that the
/// root cause is `apply_tick` running one fibre across many instructions
/// in a single transition without re-snapshotting between them — a nested
/// barrier's survivor pops an inner `V2Join` (correctly reconciling the
/// OUTER barrier's own membership into `changes`), then, without blocking,
/// immediately executes the OUTER `V2Join` in the SAME transition, which
/// used to read the outer record straight from the transition-start
/// `snapshot` (blind to this transition's own in-flight writes) and
/// silently overwrite the just-staged reconciliation with its own stale
/// re-`Insert`. Fixed by `fetch_record_in_transition`, a shared
/// pending-aware lookup used at every mid-transition record read,
/// mirroring `v2_reconcile_ancestor_membership`'s own pre-existing
/// "check `changes` before `snapshot`" idiom. Independently confirmed via
/// 100 standalone process runs post-fix, zero failures.
#[tokio::test]
async fn t_and_v2_nested_gateway_inside_branch_compiles_and_completes() {
    let bpmn_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:parallelGateway id="outer_fork" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_a1"><bpmn:extensionElements><zeebe:taskDefinition type="task_a1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_a2"><bpmn:extensionElements><zeebe:taskDefinition type="task_a2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_a3"><bpmn:extensionElements><zeebe:taskDefinition type="task_a3"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="b_pre"><bpmn:extensionElements><zeebe:taskDefinition type="b_pre"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:parallelGateway id="inner_fork" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_b1"><bpmn:extensionElements><zeebe:taskDefinition type="task_b1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_b2"><bpmn:extensionElements><zeebe:taskDefinition type="task_b2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_b3"><bpmn:extensionElements><zeebe:taskDefinition type="task_b3"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:parallelGateway id="inner_join" gatewayDirection="Converging"/>
    <bpmn:parallelGateway id="outer_join" gatewayDirection="Converging"/>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f0" sourceRef="start" targetRef="outer_fork"/>
    <bpmn:sequenceFlow id="fa0" sourceRef="outer_fork" targetRef="task_a1"/>
    <bpmn:sequenceFlow id="fa1" sourceRef="task_a1" targetRef="task_a2"/>
    <bpmn:sequenceFlow id="fa2" sourceRef="task_a2" targetRef="task_a3"/>
    <bpmn:sequenceFlow id="fa3" sourceRef="task_a3" targetRef="outer_join"/>
    <bpmn:sequenceFlow id="fb_pre" sourceRef="outer_fork" targetRef="b_pre"/>
    <bpmn:sequenceFlow id="fb0" sourceRef="b_pre" targetRef="inner_fork"/>
    <bpmn:sequenceFlow id="fb1" sourceRef="inner_fork" targetRef="task_b1"/>
    <bpmn:sequenceFlow id="fb2" sourceRef="task_b1" targetRef="task_b2"/>
    <bpmn:sequenceFlow id="fb3" sourceRef="task_b2" targetRef="inner_join"/>
    <bpmn:sequenceFlow id="fb4" sourceRef="inner_fork" targetRef="task_b3"/>
    <bpmn:sequenceFlow id="fb5" sourceRef="task_b3" targetRef="inner_join"/>
    <bpmn:sequenceFlow id="fb6" sourceRef="inner_join" targetRef="outer_join"/>
    <bpmn:sequenceFlow id="fend" sourceRef="outer_join" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#;

    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let compiled = engine
        .compile(bpmn_xml)
        .await
        .expect("GatewayAnd branch containing a further nested GatewayAnd pair must compile");

    let instance_id = engine
        .start(
            "test",
            compiled.bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-and-nested-1",
        )
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();

    let mut inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    for _ in 0..20 {
        if inst.state.is_terminal() {
            break;
        }
        let jobs = engine.run_instance(instance_id).await.unwrap();
        for job in &jobs {
            let payload = "{}";
            let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
            engine
                .complete_job(&job.job_key, payload, hash, BTreeMap::new())
                .await
                .unwrap();
        }
        engine.tick_instance(instance_id).await.unwrap();
        inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
    }
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// `GatewayXor` investigation: this IR has no dedicated "converging
/// GatewayXor" node — two XOR branches can reconverge directly on a shared
/// downstream `ServiceTask` with no gateway element at the merge point at
/// all. Proves that shape, with unequal-length branches (skewing raw BFS
/// discovery order the same way the GatewayAnd reproduction above does),
/// compiles and routes correctly through the real XML frontend.
#[tokio::test]
async fn t_xor_v2_merge_unequal_branch_lengths_compiles_and_completes() {
    let bpmn_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:exclusiveGateway id="xor_split"/>
    <bpmn:serviceTask id="task_a1"><bpmn:extensionElements><zeebe:taskDefinition type="task_a1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_a2"><bpmn:extensionElements><zeebe:taskDefinition type="task_a2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_a3"><bpmn:extensionElements><zeebe:taskDefinition type="task_a3"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="merge_task"><bpmn:extensionElements><zeebe:taskDefinition type="merge_task"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f0" sourceRef="start" targetRef="xor_split"/>
    <bpmn:sequenceFlow id="fa0" sourceRef="xor_split" targetRef="task_a1">
      <bpmn:conditionExpression>= take_a == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="fa1" sourceRef="task_a1" targetRef="task_a2"/>
    <bpmn:sequenceFlow id="fa2" sourceRef="task_a2" targetRef="task_a3"/>
    <bpmn:sequenceFlow id="fa3" sourceRef="task_a3" targetRef="merge_task"/>
    <bpmn:sequenceFlow id="fb0" sourceRef="xor_split" targetRef="merge_task"/>
    <bpmn:sequenceFlow id="fend" sourceRef="merge_task" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#;

    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let compiled = engine.compile(bpmn_xml).await.expect(
        "GatewayXor with unequal-length branches reconverging on a shared task must compile",
    );

    // Take the DEFAULT (short) branch straight to merge_task — the flag is
    // left unset, so `take_a` is false.
    let instance_id = engine
        .start(
            "test",
            compiled.bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-xor-merge-1",
        )
        .await
        .unwrap();
    engine.tick_instance(instance_id).await.unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(
        jobs.len(),
        1,
        "default branch should reach merge_task directly"
    );
    assert_eq!(jobs[0].task_type, "merge_task");

    let payload = "{}";
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    engine
        .complete_job(&jobs[0].job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();

    let mut inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    for _ in 0..5 {
        if inst.state.is_terminal() {
            break;
        }
        engine.tick_instance(instance_id).await.unwrap();
        inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
    }
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// V5 post-close (§18 ruling I): a v2-lowered interrupting boundary timer
/// — `V2Guard` + `GUARD-TIMER>` wrapping the host task (`lowering::
/// lower_boundary_guarded_task_v2`) — actually fires end-to-end through
/// the real timer-claim path (`tick_due_timers` → `Command::TimerFired` →
/// `TimerKind::V2GuardTimer` → `apply_v2_trigger_guard`), the same shape
/// `t_auth_6_boundary_timer_yaml` proves for the v1 `race_plan` mechanism
/// (untouched, above): the host task's own job is never completed, the
/// timer fires instead, and the escalation task's job is activated.
#[tokio::test]
async fn t_boundary_timer_v2_guard_timer_fires_and_activates_escalation_job() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:serviceTask id="host"><bpmn:extensionElements><zeebe:taskDefinition type="long_work"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:boundaryEvent id="timeout" attachedToRef="host" cancelActivity="true">
      <bpmn:timerEventDefinition><bpmn:timeDuration>PT2S</bpmn:timeDuration></bpmn:timerEventDefinition>
    </bpmn:boundaryEvent>
    <bpmn:serviceTask id="escalate"><bpmn:extensionElements><zeebe:taskDefinition type="escalate_work"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:endEvent id="normal_end"/>
    <bpmn:endEvent id="timeout_end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="host"/>
    <bpmn:sequenceFlow id="f2" sourceRef="host" targetRef="normal_end"/>
    <bpmn:sequenceFlow id="f3" sourceRef="timeout" targetRef="escalate"/>
    <bpmn:sequenceFlow id="f4" sourceRef="escalate" targetRef="timeout_end"/>
  </bpmn:process>
</bpmn:definitions>"#;
    let graph = bpmn_lite_compiler::parse_bpmn(xml).unwrap();
    let workflow = bpmn_lite_compiler::Compiler::lower_v2(&graph)
        .expect("v2 boundary-timer lowering must verify");
    store.store_artifact(&workflow).await.unwrap();
    let bytecode_version = workflow.hash().into_bytes();

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-v2-bt-1",
        )
        .await
        .unwrap();

    engine.tick_instance(instance_id).await.unwrap();
    let host_jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(
        host_jobs.len(),
        1,
        "the host task's own job must be activated"
    );

    assert_eq!(
        engine
            .tick_due_timers("timer-test", FAR_FUTURE_TIMER_MS, 10, 30_000)
            .await
            .unwrap(),
        1,
        "GUARD-TIMER>'s scheduled timer must fire"
    );
    // The fired timer spawns the handler fibre parked; a further tick
    // advances it through its own ExecNative to actually enqueue the job
    // (mirrors t_auth_6_boundary_timer_yaml's identical second tick).
    engine.tick_instance(instance_id).await.unwrap();

    let escalation_jobs = engine
        .activate_jobs(&["escalate_work".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(
        escalation_jobs.len(),
        1,
        "the interrupting guard's handler (escalation task) must be spawned on fire"
    );
}

/// WS-D follow-up (Adam's ruling 2026-08-03, `JobMutation::Cancel`): when
/// the interrupting guard fires while the host task's job is still
/// PENDING (enqueued, never dequeued — exactly the designer-advance
/// scenario that surfaced this), the unwind must cancel the activation so
/// `dequeue_jobs` NEVER hands it out as a ghost. Before the fix this
/// dequeue returned two jobs — the escalation's AND the unwound host's,
/// whose completion the kernel then refused ("completion has no parked
/// fiber"). Same workflow as
/// `t_boundary_timer_v2_guard_timer_fires_and_activates_escalation_job`;
/// the only difference is the host job is left un-dequeued at fire time.
#[tokio::test]
async fn t_guard_fire_cancels_pending_host_job_activation() {
    let store = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_1" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:serviceTask id="host"><bpmn:extensionElements><zeebe:taskDefinition type="long_work"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:boundaryEvent id="timeout" attachedToRef="host" cancelActivity="true">
      <bpmn:timerEventDefinition><bpmn:timeDuration>PT2S</bpmn:timeDuration></bpmn:timerEventDefinition>
    </bpmn:boundaryEvent>
    <bpmn:serviceTask id="escalate"><bpmn:extensionElements><zeebe:taskDefinition type="escalate_work"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:endEvent id="normal_end"/>
    <bpmn:endEvent id="timeout_end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="host"/>
    <bpmn:sequenceFlow id="f2" sourceRef="host" targetRef="normal_end"/>
    <bpmn:sequenceFlow id="f3" sourceRef="timeout" targetRef="escalate"/>
    <bpmn:sequenceFlow id="f4" sourceRef="escalate" targetRef="timeout_end"/>
  </bpmn:process>
</bpmn:definitions>"#;
    let graph = bpmn_lite_compiler::parse_bpmn(xml).unwrap();
    let workflow = bpmn_lite_compiler::Compiler::lower_v2(&graph)
        .expect("v2 boundary-timer lowering must verify");
    store.store_artifact(&workflow).await.unwrap();
    let bytecode_version = workflow.hash().into_bytes();

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-v2-bt-cancel-1",
        )
        .await
        .unwrap();

    // Enqueue the host job (park the fibre on it) — but DO NOT dequeue.
    engine.tick_instance(instance_id).await.unwrap();

    // Fire the guard while the host activation is still pending.
    assert_eq!(
        engine
            .tick_due_timers("timer-test", FAR_FUTURE_TIMER_MS, 10, 30_000)
            .await
            .unwrap(),
        1,
        "GUARD-TIMER>'s scheduled timer must fire"
    );
    engine.tick_instance(instance_id).await.unwrap();

    // The receipt: dequeue over BOTH task types returns exactly the
    // escalation job — the cancelled host activation is gone, not a ghost.
    let jobs = engine
        .activate_jobs(&["long_work".to_string(), "escalate_work".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(
        jobs.len(),
        1,
        "only the escalation job may be activated; the unwound host's \
         activation must have been cancelled: {jobs:?}"
    );
    assert_eq!(jobs[0].task_type, "escalate_work");
}

// ═══════════════════════════════════════════════════════════
//  §18 ruling K: multi-instance runtime tests
// ═══════════════════════════════════════════════════════════

fn multi_instance_v2_xml(declared_max: u32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="proc_mi" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:dataObject id="doc_count" name="doc_count"></bpmn:dataObject>
    <bpmn:serviceTask id="verify_docs">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="verify_doc"/>
      </bpmn:extensionElements>
      <bpmn:multiInstanceLoopCharacteristics isSequential="false">
        <bpmn:extensionElements>
          <zeebe:loopCharacteristics inputCollection="doc_count" maxInstances="{declared_max}"/>
        </bpmn:extensionElements>
      </bpmn:multiInstanceLoopCharacteristics>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="verify_docs"/>
    <bpmn:sequenceFlow id="f2" sourceRef="verify_docs" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#
    )
}

/// Returns the workflow's `collection_flag` (the flag `V2MiIndexLive`/
/// `V2MiArityCheck` read as a `Value::Array`, §18 ruling K Part 2 — was an
/// `I64` length flag before this landing) plus the full artifact so
/// per-branch element-flag keys can be looked up by name afterward.
async fn compile_multi_instance_v2(
    declared_max: u32,
) -> (Arc<MemoryStore>, ExecutableWorkflow, FlagKey) {
    let store = Arc::new(MemoryStore::new());
    let xml = multi_instance_v2_xml(declared_max);
    let graph = bpmn_lite_compiler::parse_bpmn(&xml).unwrap();
    let workflow = bpmn_lite_compiler::Compiler::lower_v2(&graph)
        .expect("v2 multi-instance lowering must verify");
    let collection_flag = *workflow
        .envelope()
        .metadata()
        .flag_symbol_table()
        .iter()
        .find(|(_, name)| name.as_str() == "doc_count")
        .map(|(key, _)| key)
        .expect("doc_count must be interned as a flag");
    store.store_artifact(&workflow).await.unwrap();
    (store, workflow, collection_flag)
}

/// Looks up the `FlagKey` interned for MI branch `index`'s per-branch
/// element flag (`<node_id>_mi_element_<index>`, `lowering.rs`'s
/// `lower_multi_instance_v2`). The XML fixture's `serviceTask` id is
/// `verify_docs`.
fn mi_element_flag_key(workflow: &ExecutableWorkflow, index: u32) -> FlagKey {
    let name = format!("verify_docs_mi_element_{index}");
    *workflow
        .envelope()
        .metadata()
        .flag_symbol_table()
        .iter()
        .find(|(_, n)| n.as_str() == name)
        .map(|(key, _)| key)
        .unwrap_or_else(|| panic!("{name} must be interned as a flag"))
}

/// Sets the MI collection to a `Value::Array` of `I64` elements (§18
/// ruling K Part 2 — was `Value::I64(length)` before this landing; length
/// is now derived from the array itself, not tracked separately).
async fn set_collection(
    store: &MemoryStore,
    instance_id: Uuid,
    collection_flag: FlagKey,
    elements: &[i64],
) {
    let mut inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    inst.flags.insert(
        collection_flag,
        Value::Array(elements.iter().map(|n| Value::I64(*n)).collect()),
    );
    bpmn_lite_store::store::commit_snapshot(store, "test", inst)
        .await
        .unwrap();
}

async fn drain_to_terminal(
    engine: &BpmnLiteEngine,
    store: &MemoryStore,
    instance_id: Uuid,
) -> ProcessInstance {
    let mut inst = store
        .load_instance(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            instance_id,
        )
        .await
        .unwrap()
        .unwrap();
    for _ in 0..5 {
        if inst.state.is_terminal() {
            break;
        }
        engine.tick_instance(instance_id).await.unwrap();
        inst = store
            .load_instance(
                &bpmn_lite_types::TenantId::new("default").unwrap(),
                instance_id,
            )
            .await
            .unwrap()
            .unwrap();
    }
    inst
}

/// (a) full collection (length == declared_max): all `n` fibres do real
/// work, the barrier retires normally, the instance completes.
#[tokio::test]
async fn t_mi_v2_full_collection_all_fibres_do_real_work() {
    let (store, workflow, collection_flag) = compile_multi_instance_v2(3).await;
    let bytecode_version = workflow.hash().into_bytes();
    let engine = BpmnLiteEngine::new(store.clone());

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-mi-1",
        )
        .await
        .unwrap();
    set_collection(&store, instance_id, collection_flag, &[100, 200, 300]).await;

    engine.tick_instance(instance_id).await.unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(jobs.len(), 3, "all 3 declared-max fibres must do real work");
    for job in &jobs {
        assert_eq!(job.task_type, "verify_doc");
    }

    for job in &jobs {
        let payload = "{}";
        let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
        engine
            .complete_job(&job.job_key, payload, hash, BTreeMap::new())
            .await
            .unwrap();
    }

    let inst = drain_to_terminal(&engine, &store, instance_id).await;
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// §18 ruling K Part 2 (the reason this landing exists): each branch must
/// receive its OWN element's value, not merely know whether its index is
/// live. Proven end to end through the real `orch_flags` job-dispatch
/// pipeline (`ExecNative`'s handler snapshots `instance.flags` into
/// `JobActivation.orch_flags` at the moment it runs) — not by inspecting
/// kernel-internal state directly. Each branch's `ExecNative` sits at a
/// distinct compiled address (branches are laid out at fixed, increasing
/// offsets in index order), and no `debug_map` entry exists for it
/// specifically (only the MI node's own `base` address is recorded), so
/// the kernel's `service_task_id` fallback is `"pc_<address>"` — this is
/// used here only to recover which job came from which branch index (by
/// sorting on the numeric pc), not asserted as meaningful behavior in its
/// own right.
#[tokio::test]
async fn t_mi_v2_delivers_distinct_per_branch_element_values() {
    let (store, workflow, collection_flag) = compile_multi_instance_v2(2).await;
    let bytecode_version = workflow.hash().into_bytes();
    let engine = BpmnLiteEngine::new(store.clone());

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-mi-elements",
        )
        .await
        .unwrap();
    let elements = [111i64, 222i64];
    set_collection(&store, instance_id, collection_flag, &elements).await;

    engine.tick_instance(instance_id).await.unwrap();
    let mut jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(
        jobs.len(),
        2,
        "both branches must do real work (full collection)"
    );

    // Recover branch index via the "pc_<address>" service_task_id
    // fallback — branch 0's ExecNative compiles to a lower address than
    // branch 1's, since branches are synthesized in index order.
    jobs.sort_by_key(|job| {
        job.service_task_id
            .strip_prefix("pc_")
            .and_then(|n| n.parse::<u32>().ok())
            .expect("MI branch ExecNative has no debug_map entry of its own, so service_task_id must be the pc_<addr> fallback")
    });

    for (index, job) in jobs.iter().enumerate() {
        let element_flag = mi_element_flag_key(&workflow, index as u32);
        let expected_key = format!("flag_{element_flag}");
        assert_eq!(
            job.orch_flags.get(&expected_key),
            Some(&Value::I64(elements[index])),
            "branch {index}'s job must carry its OWN element ({}) under {expected_key}, not \
             some other branch's — got orch_flags = {:?}",
            elements[index],
            job.orch_flags
        );
    }
    // The two branches' own element flags must differ — proving this
    // isn't "some value arrived" but genuinely distinct per-branch
    // delivery.
    let flag_0 = mi_element_flag_key(&workflow, 0);
    let flag_1 = mi_element_flag_key(&workflow, 1);
    assert_ne!(
        flag_0, flag_1,
        "each branch must write its OWN element flag key"
    );

    for job in &jobs {
        let payload = "{}";
        let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
        engine
            .complete_job(&job.job_key, payload, hash, BTreeMap::new())
            .await
            .unwrap();
    }
    let inst = drain_to_terminal(&engine, &store, instance_id).await;
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// (b) partial collection (length < declared_max): some fibres do real
/// work, the rest skip straight to `V2Join` — the barrier still retires
/// correctly on the reduced arrival count that matters (real-work count),
/// and the instance completes.
#[tokio::test]
async fn t_mi_v2_partial_collection_some_fibres_skip() {
    let (store, workflow, collection_flag) = compile_multi_instance_v2(3).await;
    let bytecode_version = workflow.hash().into_bytes();
    let engine = BpmnLiteEngine::new(store.clone());

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-mi-2",
        )
        .await
        .unwrap();
    set_collection(&store, instance_id, collection_flag, &[100]).await;

    engine.tick_instance(instance_id).await.unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(
        jobs.len(),
        1,
        "only index 0 is live; indices 1,2 skip to V2Join"
    );

    let payload = "{}";
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    engine
        .complete_job(&jobs[0].job_key, payload, hash, BTreeMap::new())
        .await
        .unwrap();

    let inst = drain_to_terminal(&engine, &store, instance_id).await;
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// (c) empty collection (length == 0): every fibre skips, the barrier
/// still retires (all `n` "arrived," just instantly), execution continues
/// past the join — **no Incident raised**, in deliberate contrast to a
/// gateway's zero-match (ruling J), which IS an incident. This is the test
/// that actually distinguishes the two rules — see
/// `t_ig_v2_zero_match_no_default_raises_incident` immediately above for
/// the gateway-side behavior this must differ from.
#[tokio::test]
async fn t_mi_v2_empty_collection_completes_without_incident() {
    let (store, workflow, collection_flag) = compile_multi_instance_v2(3).await;
    let bytecode_version = workflow.hash().into_bytes();
    let engine = BpmnLiteEngine::new(store.clone());

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-mi-3",
        )
        .await
        .unwrap();
    set_collection(&store, instance_id, collection_flag, &[]).await;

    engine.tick_instance(instance_id).await.unwrap();
    let jobs = engine.run_instance(instance_id).await.unwrap();
    assert_eq!(
        jobs.len(),
        0,
        "an empty collection does no real work at all"
    );

    let inst = drain_to_terminal(&engine, &store, instance_id).await;
    assert!(
        !matches!(inst.state, ProcessState::Incidented { .. }),
        "empty collection must NOT raise an Incident — ruling K item (c), \
         deliberately not unified with a gateway's zero-match (ruling J): {:?}",
        inst.state
    );
    assert!(
        matches!(inst.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        inst.state
    );
}

/// (d) collection length exceeds declared_max: a typed, hard reject
/// (`TransitionError::ResourceLimitExceeded`, the SAME shape
/// `validate_snapshot_limits` already uses for fiber/stack/register
/// overflow) — not silent truncation, not a panic, and NOT an Incident
/// (deliberately distinct from ruling J's zero-match handling — see
/// `Instr::V2MiArityCheck`'s doc comment).
#[tokio::test]
async fn t_mi_v2_exceeds_declared_max_is_typed_error_not_silent_truncation() {
    let (store, workflow, collection_flag) = compile_multi_instance_v2(2).await;
    let bytecode_version = workflow.hash().into_bytes();
    let engine = BpmnLiteEngine::new(store.clone());

    let instance_id = engine
        .start(
            "test",
            bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "corr-mi-4",
        )
        .await
        .unwrap();
    set_collection(
        &store,
        instance_id,
        collection_flag,
        &[100, 200, 300, 400, 500],
    )
    .await;

    let err = engine
        .tick_instance(instance_id)
        .await
        .expect_err("exceeding declared_max must be a hard reject, not silent truncation");
    let message = format!("{err:?}");
    assert!(
        message.contains("multi-instance collection length")
            || message.contains("ResourceLimitExceeded"),
        "error must name the resource-limit violation: {message}"
    );
}

// ═══════════════════════════════════════════════════════════
//  V5.5 (§18, landed 2026-07-23): corpus recompile-and-verify sweep.
//  "All demo/test workflows recompiled; full verifier pass over the
//  recompiled corpus is itself a test" — not a one-time manual claim, a
//  standing regression gate. Every workflow here compiles through the
//  now-v2-default `lower()` (XML) / `dsl::compile` (DSL) frontend and
//  must pass the full pipeline: `verify_or_err` (V4a structural checks),
//  lowering, `verify_bytecode`, and `ArtifactEnvelope::from_legacy_program`
//  → `ExecutableWorkflow::from_verified_envelope`'s `verify_program`
//  (V-1..V-11, K-1..K-3's static control-stack half). Any future v1
//  residue, a broken lowering path, or a verifier regression on any of
//  these fixtures fails this test, not just the fixture's own narrower
//  test elsewhere in this file.
//
//  `corpus_sweep_demo_source_lowers_and_verifies` (the §10 demo-workflow
//  half of this sweep) moved to
//  `xtask/tests/demo_corpus_vertical.rs` (H2, EOP-PLAN-CRATE-HYGIENE-001)
//  — `build_demo_plan` no longer lives in this crate, it moved to
//  `bpmn-lite-server-runner::demo` (rest.rs's real demo-mode consumer).
// ═══════════════════════════════════════════════════════════

/// Every hand-built XML fixture in this file that exercises a distinct
/// BPMN construct (parallel/exclusive/inclusive gateways, standalone and
/// boundary timers — interrupting and non-interrupting, multi-instance,
/// send/message tasks) recompiles cleanly through `lower()`'s v2-default
/// path and passes the full verifier. `t_ig_6_parse_inclusive_gateway`
/// and `t_auth_6_boundary_timer_yaml` already prove their own fixtures
/// individually (with tighter, shape-specific assertions); this sweep is
/// deliberately broader and shallower — one line per fixture, "does the
/// whole pipeline still admit this," not a re-assertion of each
/// fixture's own specific behavior.
#[test]
fn corpus_sweep_xml_fixtures_lower_and_verify() {
    let fixtures: &[(&str, &str)] = &[
        ("ORDINARY_TIMER_BPMN", ORDINARY_TIMER_BPMN),
        ("ABSOLUTE_TIMER_BPMN", ABSOLUTE_TIMER_BPMN),
        ("SINGLE_TASK_BPMN", SINGLE_TASK_BPMN),
        ("NI_BOUNDARY_BPMN", NI_BOUNDARY_BPMN),
    ];
    for (name, xml) in fixtures {
        let graph = bpmn_lite_compiler::parse_bpmn(xml)
            .unwrap_or_else(|e| panic!("{name}: parse failed: {e}"));
        bpmn_lite_compiler::Compiler::lower(&graph)
            .unwrap_or_else(|e| panic!("{name}: lower+verify failed: {e}"));
    }
}

// The inclusive-gateway/boundary-timer/multi-instance corpus (T-IG-6's own
// XML, the v2 boundary-timer/inclusive-gateway/MI fixtures built by their
// own `make_*`-style helper functions elsewhere in this file) already
// recompiles and verifies inside each of their own tests
// (`t_ig_6_parse_inclusive_gateway`, `t_ig_v2_*`, `t_boundary_timer_v2_*`,
// `t_mi_v2_*` — every one of them calls `engine.compile`/
// `Compiler::lower_v2` and unwraps the result, which is exactly this
// sweep's own check). Not duplicated here — see this test module's own
// `T-IG`/`T-NI`/`t_mi_v2`/`t_boundary_timer_v2` sections for that
// coverage; `corpus_sweep_xml_fixtures_lower_and_verify` above exists so
// the two fixtures with no other dedicated lowering-through-verifier test
// (`ORDINARY_TIMER_BPMN`/`ABSOLUTE_TIMER_BPMN`, used only for
// scheduler-behavior tests that compile via `engine.compile` and never
// separately assert on the lowered program) get swept too.

/// F-08 remediation (Phase 2 follow-up,
/// `docs/todo/PHASE2-tokenised-transition-release.md`): the per-instance
/// single-flight guard must reject a second acquisition for an instance
/// already held, and admit it again once released. Tested directly
/// against the guard primitive (`with_instance_guard` is a private
/// method, reachable here since `tests` is an inner module of the same
/// crate) rather than by racing two real engine calls through
/// `tokio::spawn`: the guarded critical section for an in-memory store
/// is fast enough (no real `.await` suspension inside it) that an
/// actual OS-thread race reliably resolves before the second task is
/// even polled, making a timing-based version of this test flaky by
/// construction rather than a meaningful proof. This is deliberately
/// the inverse of what the a11 FFI end-to-end tests prove: those confirm
/// a NESTED same-chain call (`tick_instance_inner`'s own effect-response
/// application, for the SAME instance it's already ticking) is correctly
/// allowed through; this test confirms a second, independent acquisition
/// for the same instance is not — until the first releases.
#[tokio::test]
async fn instance_guard_rejects_second_acquisition_until_released() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store);
    let instance_id = Uuid::now_v7();

    engine
        .try_acquire_instance_guard(instance_id)
        .expect("first acquisition must succeed");
    let second = engine.try_acquire_instance_guard(instance_id);
    assert!(
        second.is_err(),
        "a second acquisition for an already-held instance must be rejected"
    );
    assert!(
        second
            .unwrap_err()
            .to_string()
            .contains("already in flight"),
        "rejection must be attributable to the single-flight guard, not some other failure"
    );

    // A different instance is unaffected — the guard is per-instance,
    // not a global lock.
    let other_instance_id = Uuid::now_v7();
    engine
        .try_acquire_instance_guard(other_instance_id)
        .expect("an unrelated instance must not be blocked");

    engine.release_instance_guard(instance_id);
    engine
        .try_acquire_instance_guard(instance_id)
        .expect("acquisition must succeed again once released");
}

// ── Phase 3C: activation-queue-driven scheduler dispatch ──

#[tokio::test]
async fn tick_activated_batch_drains_an_instance_start_leaves_runnable() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());
    let tenant = bpmn_lite_types::TenantId::default();

    let bpmn = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
          <bpmn:process id="drain_proc" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:endEvent id="end" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="end" />
          </bpmn:process>
        </bpmn:definitions>"#;
    let compile_result = engine.compile(bpmn).await.unwrap();
    let payload = r#"{"case":"drain"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
    let instance_id = engine
        .start(
            "drain_proc",
            compile_result.bytecode_version,
            payload,
            hash,
            "corr-drain",
        )
        .await
        .unwrap();

    // Phase 3B's dual-write must already have produced a ready activation
    // from the start commit itself, with nothing calling tick yet.
    let before = store
        .claim_ready_activations(&tenant, "peek", 10, 30_000)
        .await
        .unwrap();
    assert_eq!(
        before.len(),
        1,
        "instance start leaving a runnable fiber must dual-write an activation"
    );
    store
        .release_activation_to_ready(&before[0], None)
        .await
        .unwrap();

    // Bounded loop: each tick_activated_batch call drains whatever's
    // ready, and a drain can itself leave one more activation behind if
    // the kernel needed more than one internal step — this workflow
    // shouldn't, but the loop is here so the test documents the
    // "run until dry" shape a real scheduler pass repeats, not a
    // hardcoded step count.
    let mut total_processed = 0u32;
    for _ in 0..5 {
        let processed = engine
            .tick_activated_batch("scheduler-1", 10, 30_000)
            .await
            .unwrap();
        total_processed += processed;
        if processed == 0 {
            break;
        }
    }
    assert_eq!(
        total_processed, 1,
        "a straight-through start->end workflow must drain in exactly one activation"
    );

    let instance = store
        .load_instance(&tenant, instance_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(instance.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        instance.state
    );

    let after = store
        .claim_ready_activations(&tenant, "peek-after", 10, 30_000)
        .await
        .unwrap();
    assert!(
        after.is_empty(),
        "a completed instance must leave no activation behind for consume_activation to have missed"
    );
}

#[tokio::test]
async fn tick_activated_batch_releases_to_ready_on_tick_failure() {
    // A claimed activation whose drain fails (e.g. the instance's
    // transition lease is held elsewhere) must go back to `ready`, not
    // vanish — otherwise one transient failure permanently strands the
    // instance with no activation ever claimable again.
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());
    let tenant = bpmn_lite_types::TenantId::default();
    let instance_id = Uuid::now_v7();

    let mut instance = bpmn_lite_types::ProcessInstance {
        instance_id,
        process_key: "no_artifact_proc".to_string(),
        bytecode_version: [7u8; 32],
        tenant_id: tenant.as_str().to_string(),
        domain_payload: "{}".to_string().into(),
        domain_payload_hash: [0u8; 32],
        session_stack: SessionStackState::default(),
        flags: BTreeMap::new(),
        counters: BTreeMap::new(),
        join_expected: BTreeMap::new(),
        state: ProcessState::Running,
        correlation_id: "corr-fail".to_string(),
        entry_id: Uuid::new_v4(),
        runbook_id: Uuid::new_v4(),
        created_at: 0,
        integrity_hash: None,
        quarantine_state: None,
        plan_hash: None,
        current_node_id: None,
        placeholder_values: None,
    };
    instance.domain_payload_hash = bpmn_lite_types::EffectId::content_hash(b"{}");
    let claim = bpmn_lite_types::Claim::new(tenant.clone(), instance_id, 0, 0, "");
    let transition = bpmn_lite_types::TransitionBuilder::new(instance)
        .upsert_fiber(Fiber::new(Uuid::now_v7(), 0u32))
        .build();
    store.commit_transition(&claim, &transition).await.unwrap();

    let before = store
        .claim_ready_activations(&tenant, "peek", 10, 30_000)
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    store
        .release_activation_to_ready(&before[0], None)
        .await
        .unwrap();

    // No artifact was ever stored for [7u8; 32], so the drain must fail
    // (load_artifact_cached errors) rather than panic.
    let processed = engine
        .tick_activated_batch("scheduler-1", 10, 30_000)
        .await
        .unwrap();
    assert_eq!(
        processed, 1,
        "tick_activated_batch counts every claimed activation, success or failure"
    );

    let after = store
        .claim_ready_activations(&tenant, "peek-after", 10, 30_000)
        .await
        .unwrap();
    assert_eq!(
        after.len(),
        1,
        "a failed drain must release its activation back to ready, not drop it"
    );
}

// ── Phase 4: activation-queue metrics ──

#[tokio::test]
async fn tick_activated_batch_records_claimed_and_consumed_metrics() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let bpmn = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
          <bpmn:process id="metrics_proc" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:endEvent id="end" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="end" />
          </bpmn:process>
        </bpmn:definitions>"#;
    let compile_result = engine.compile(bpmn).await.unwrap();
    let payload = r#"{"case":"metrics"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());

    let before = engine.activation_metrics();
    engine
        .start(
            "metrics_proc",
            compile_result.bytecode_version,
            payload,
            hash,
            "corr-metrics",
        )
        .await
        .unwrap();
    engine
        .tick_activated_batch("scheduler-1", 10, 30_000)
        .await
        .unwrap();

    let after = engine.activation_metrics();
    assert_eq!(
        after.claimed_total,
        before.claimed_total + 1,
        "claimed_total must record every activation claim_ready_activations returns"
    );
    assert_eq!(
        after.consumed_total,
        before.consumed_total + 1,
        "consumed_total must record a successful drain"
    );
    assert_eq!(
        after.released_total, before.released_total,
        "a successful drain must not also count as released"
    );
}

#[tokio::test]
async fn tick_activated_batch_records_released_metric_on_failure() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());
    let tenant = bpmn_lite_types::TenantId::default();
    let instance_id = Uuid::now_v7();

    let mut instance = bpmn_lite_types::ProcessInstance {
        instance_id,
        process_key: "no_artifact_proc".to_string(),
        bytecode_version: [9u8; 32],
        tenant_id: tenant.as_str().to_string(),
        domain_payload: "{}".to_string().into(),
        domain_payload_hash: [0u8; 32],
        session_stack: SessionStackState::default(),
        flags: BTreeMap::new(),
        counters: BTreeMap::new(),
        join_expected: BTreeMap::new(),
        state: ProcessState::Running,
        correlation_id: "corr-metrics-fail".to_string(),
        entry_id: Uuid::new_v4(),
        runbook_id: Uuid::new_v4(),
        created_at: 0,
        integrity_hash: None,
        quarantine_state: None,
        plan_hash: None,
        current_node_id: None,
        placeholder_values: None,
    };
    instance.domain_payload_hash = bpmn_lite_types::EffectId::content_hash(b"{}");
    let claim = bpmn_lite_types::Claim::new(tenant.clone(), instance_id, 0, 0, "");
    let transition = bpmn_lite_types::TransitionBuilder::new(instance)
        .upsert_fiber(Fiber::new(Uuid::now_v7(), 0u32))
        .build();
    store.commit_transition(&claim, &transition).await.unwrap();

    let before = engine.activation_metrics();
    engine
        .tick_activated_batch("scheduler-1", 10, 30_000)
        .await
        .unwrap();
    let after = engine.activation_metrics();

    assert_eq!(after.claimed_total, before.claimed_total + 1);
    assert_eq!(
        after.released_total,
        before.released_total + 1,
        "a failed drain must record released_total"
    );
    assert_eq!(
        after.consumed_total, before.consumed_total,
        "a failed drain must not also count as consumed"
    );
}
