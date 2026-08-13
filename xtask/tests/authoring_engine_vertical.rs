//! Multi-crate application vertical: `bpmn-lite-authoring`'s YAML → DTO →
//! IR → bytecode pipeline feeding `bpmn-lite-engine`'s runtime end to end.
//! Moved from `bpmn-lite-engine/src/tests.rs` (EOP-PLAN-CRATE-HYGIENE-001,
//! H1 follow-up) — `bpmn-lite-engine`'s own Phase-0 boundary is "does NOT
//! depend on bpmn-lite-authoring"; these tests are exactly the kind of
//! cross-crate contract R3 assigns to `xtask/tests/`, not the engine
//! crate's own unit-test module.

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_store::store::WorkflowStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::{ErrorClass, ProcessState, RuntimeEvent, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

const FAR_FUTURE_TIMER_MS: u64 = 4_070_908_800_000;

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
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    // Compile from YAML
    let dto = bpmn_lite_authoring::parse_workflow_yaml(yaml).unwrap();
    let program = bpmn_lite_authoring::compile_program_from_dto(&dto).unwrap();
    let cr = engine.store_compiled_program(program).await.unwrap();
    assert!(cr.task_types.contains(&"do_a".to_string()));
    assert!(cr.task_types.contains(&"do_b".to_string()));

    // Start instance
    let payload = r#"{"test":"auth1"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
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
    let hash_b = bpmn_lite_types::EffectId::content_hash((r#"{"a":"done"}"#).as_bytes());
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
    use bpmn_lite_authoring::*;
    use bpmn_lite_compiler::GatewayDirection;

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
    default_guard_budget: None,
    default_retry_policy: None,
    };

    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let program = bpmn_lite_authoring::compile_program_from_dto(&dto).unwrap();
    let cr = engine.store_compiled_program(program).await.unwrap();

    // Start with flag_a=true, flag_b=false
    let payload = r#"{"test":"ig"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
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
        let mut inst = store
            .load_instance(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
            .await
            .unwrap()
            .unwrap();
        inst.flags.insert(0, Value::Bool(true));
        // flag_b (key 1) not set = defaults to false
        bpmn_lite_store::store::commit_snapshot(store.as_ref(), "test", inst)
            .await
            .unwrap();
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
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let dto = bpmn_lite_authoring::parse_workflow_yaml(yaml).unwrap();
    let program = bpmn_lite_authoring::compile_program_from_dto(&dto).unwrap();
    let cr = engine.store_compiled_program(program).await.unwrap();

    let payload = r#"{"test":"err"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
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
    let events = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap();
    let has_routed = events.iter().any(|(_, e)| {
            matches!(e, RuntimeEvent::ErrorRouted { error_code, .. } if error_code == "BIZ_FAIL")
        });
    assert!(has_routed, "Should route error to escalation handler");

    // Instance should still be Running (not Failed)
    let inst = store
        .load_instance(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap()
        .unwrap();
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
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let dto = bpmn_lite_authoring::parse_workflow_yaml(yaml).unwrap();
    let program = bpmn_lite_authoring::compile_program_from_dto(&dto).unwrap();
    let cr = engine.store_compiled_program(program).await.unwrap();

    let payload = r#"{"test":"xor"}"#;
    let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
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
async fn t_auth_6_boundary_timer_yaml() {
    let yaml = r#"
id: yaml-boundary-timer
nodes:
  - kind: Start
    id: start
  - kind: ServiceTask
    id: host
    task_type: long_work
  - kind: BoundaryTimer
    id: timeout
    host: host
    duration_ms: 2000
    interrupting: true
  - kind: ServiceTask
    id: escalate
    task_type: escalate_work
  - kind: End
    id: normal_end
  - kind: End
    id: timeout_end
edges:
  - from: start
    to: host
  - from: host
    to: normal_end
  - from: timeout
    to: escalate
  - from: escalate
    to: timeout_end
"#;
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store);
    let dto = bpmn_lite_authoring::parse_workflow_yaml(yaml).unwrap();
    let program = bpmn_lite_authoring::compile_program_from_dto(&dto).unwrap();
    let compiled = engine.store_compiled_program(program).await.unwrap();
    let instance_id = engine
        .start(
            "yaml-boundary-timer",
            compiled.bytecode_version,
            "{}",
            bpmn_lite_types::EffectId::content_hash(("{}").as_bytes()),
            "yaml-boundary",
        )
        .await
        .unwrap();

    engine.tick_instance(instance_id).await.unwrap();
    assert_eq!(
        engine
            .tick_due_timers("timer-test", FAR_FUTURE_TIMER_MS, 10, 30_000)
            .await
            .unwrap(),
        1
    );
    engine.tick_instance(instance_id).await.unwrap();
    let escalations = engine
        .activate_jobs(&["escalate_work".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(escalations.len(), 1);
}

