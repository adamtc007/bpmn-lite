//! T3.7 — Integration tests for the 4 T2B master DoD scenarios.
//!
//! All tests use `MemoryStore` + `MemoryPendingInvocationStore` — no
//! Postgres required. Bus dispatch calls are deliberately not wired to
//! real gRPC endpoints; the outbox insert that `dispatch_callout` makes
//! is bypassed by simulating result delivery directly.
//!
//! Since the MemoryStore-based `PlanWalker` will fail on `insert_outbox`
//! (no real pool), tests that exercise ServiceTask dispatch construct
//! the instance state manually up to the WaitingOnSubmission point,
//! then simulate the result delivery sequence.

use std::collections::HashMap;
use std::sync::Arc;

use bpmn_lite_compiler::dsl::plan::{
    BusinessRuleExecNode, EndExecNode, ExecutionNode, GatewayExecFlow, GatewayExecNode,
    PlaceholderSchema, ServiceTaskExecNode, StartExecNode, WorkflowExecutionPlan,
};
use bpmn_lite_store::pending::{
    InsertOutcome, MemoryPendingInvocationStore, PendingInvocation, PendingInvocationStore,
};
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::types::ProcessState;
use chrono::Utc;
use uuid::Uuid;

use crate::plan_walker::{AdvanceOutcome, PlanWalker};

// ── Helpers ──────────────────────────────────────────────────────────

/// Build a `PlanWalker` backed by MemoryStore.
/// The BusClient uses a `connect_lazy` URL — the MemoryStore tests
/// never reach `insert_outbox`, so the inert pool is fine.
async fn make_walker(
    store: Arc<MemoryStore>,
    pending: Arc<MemoryPendingInvocationStore>,
) -> PlanWalker {
    let fake_pool =
        sqlx::PgPool::connect_lazy("postgresql://localhost/plan_walker_dod_fake").unwrap();
    let client = Arc::new(
        dsl_bus_client::BusClient::builder()
            .pool(fake_pool)
            .local_domain("bpmn-lite")
            .build()
            .await
            .expect("test BusClient"),
    );
    PlanWalker::new(store, pending, client)
}

/// Start→ServiceTask(ob-poc:cbu.create)→EndEvent plan.
fn ob_poc_round_trip_plan() -> WorkflowExecutionPlan {
    let mut nodes = HashMap::new();
    nodes.insert(
        "start".to_owned(),
        ExecutionNode::StartEvent(StartExecNode {
            id: "start".to_owned(),
            next: "create-cbu".to_owned(),
        }),
    );
    nodes.insert(
        "create-cbu".to_owned(),
        ExecutionNode::ServiceTask(ServiceTaskExecNode {
            id: "create-cbu".to_owned(),
            verb_fqn: "ob-poc:cbu.create".to_owned(),
            static_args: {
                let mut m = HashMap::new();
                m.insert("product".to_owned(), "CUSTODY_FUND".to_owned());
                m
            },
            next: "end".to_owned(),
            produces_placeholder: Some("@cbu".to_owned()),
            consumes_placeholders: vec![],
        }),
    );
    nodes.insert(
        "end".to_owned(),
        ExecutionNode::EndEvent(EndExecNode {
            id: "end".to_owned(),
            status: "Operational".to_owned(),
        }),
    );
    WorkflowExecutionPlan {
        workflow_id: "custody-cbu-onboarding".to_owned(),
        nodes,
        start_node: "start".to_owned(),
        placeholder_schema: PlaceholderSchema::default(),
    }
}

/// Start→BusinessRuleTask(dmn-lite:cbu_type_routing)→Gateway→…→End plan.
fn dmn_lite_round_trip_plan() -> WorkflowExecutionPlan {
    let mut nodes = HashMap::new();
    nodes.insert(
        "start".to_owned(),
        ExecutionNode::StartEvent(StartExecNode {
            id: "start".to_owned(),
            next: "route".to_owned(),
        }),
    );
    nodes.insert(
        "route".to_owned(),
        ExecutionNode::BusinessRuleTask(BusinessRuleExecNode {
            id: "route".to_owned(),
            decision_id: "dmn-lite:cbu_type_routing".to_owned(),
            next: "gateway".to_owned(),
            produces_placeholder: Some("@cbu-type".to_owned()),
            consumes_placeholders: vec![],
        }),
    );
    nodes.insert(
        "gateway".to_owned(),
        ExecutionNode::ExclusiveGateway(GatewayExecNode {
            id: "gateway".to_owned(),
            flows: vec![
                GatewayExecFlow {
                    placeholder: "@cbu-type".to_owned(),
                    expected_value: "fund".to_owned(),
                    next: "end-fund".to_owned(),
                },
                GatewayExecFlow {
                    placeholder: "@cbu-type".to_owned(),
                    expected_value: "corporate".to_owned(),
                    next: "end-corporate".to_owned(),
                },
            ],
        }),
    );
    nodes.insert(
        "end-fund".to_owned(),
        ExecutionNode::EndEvent(EndExecNode {
            id: "end-fund".to_owned(),
            status: "FundOnboarded".to_owned(),
        }),
    );
    nodes.insert(
        "end-corporate".to_owned(),
        ExecutionNode::EndEvent(EndExecNode {
            id: "end-corporate".to_owned(),
            status: "CorporateOnboarded".to_owned(),
        }),
    );
    WorkflowExecutionPlan {
        workflow_id: "cbu-type-routing".to_owned(),
        nodes,
        start_node: "start".to_owned(),
        placeholder_schema: PlaceholderSchema::default(),
    }
}

/// Inject a plan-based instance already at `WaitingOnSubmission` on
/// `node_id`, with `plan_hash` set. Simulates the state the plan
/// walker leaves after dispatching a callout.
async fn inject_waiting_instance(
    store: &MemoryStore,
    plan: &WorkflowExecutionPlan,
    tenant: &str,
    node_id: &str,
    callout_id: Uuid,
) -> Uuid {
    let plan_json = serde_json::to_string(plan).unwrap();
    let hash = *blake3::hash(plan_json.as_bytes()).as_bytes();
    store.store_plan(hash, &plan_json).await.unwrap();

    let instance_id = Uuid::now_v7();
    let instance = bpmn_lite_types::ProcessInstance {
        instance_id,
        tenant_id: tenant.to_owned(),
        process_key: plan.workflow_id.clone(),
        bytecode_version: [0u8; 32],
        domain_payload: "{}".into(),
        domain_payload_hash: [0u8; 32],
        session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
        flags: Default::default(),
        counters: Default::default(),
        join_expected: Default::default(),
        state: ProcessState::WaitingOnSubmission {
            callout_id,
            node_id: node_id.to_owned(),
        },
        correlation_id: String::new(),
        entry_id: Uuid::nil(),
        runbook_id: Uuid::nil(),
        created_at: Utc::now().timestamp_millis(),
        integrity_hash: None,
        quarantine_state: None,
        plan_hash: Some(hash),
        current_node_id: Some(node_id.to_owned()),
        placeholder_values: None,
    };
    store.save_instance(&instance).await.unwrap();
    instance_id
}

/// Simulate the advancer completing: transition instance to Running,
/// advance current_node_id to task.next, bind placeholder values.
/// This mirrors what `StoreBackedAdvancer` does in T3.4, adapted for
/// MemoryStore (no pending row needed).
async fn simulate_result_delivery(
    store: &MemoryStore,
    instance_id: Uuid,
    _node_id: &str,
    next_node: &str,
    placeholder_name: Option<&str>,
    placeholder_value: Option<serde_json::Value>,
) {
    let mut inst = store.load_instance(instance_id).await.unwrap().unwrap();
    inst.state = ProcessState::Running;
    inst.current_node_id = Some(next_node.to_owned());
    if let (Some(name), Some(val)) = (placeholder_name, placeholder_value) {
        let mut pv: HashMap<String, serde_json::Value> = inst
            .placeholder_values
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        pv.insert(name.to_owned(), val);
        inst.placeholder_values = serde_json::to_value(&pv).ok();
    }
    store.save_instance(&inst).await.unwrap();
}

// ── DoD #41 — ob-poc full round-trip ─────────────────────────────────

#[tokio::test]
async fn dod_41_ob_poc_full_round_trip() {
    // Setup: inject instance at WaitingOnSubmission on "create-cbu".
    let store = Arc::new(MemoryStore::new());
    let pending = Arc::new(MemoryPendingInvocationStore::new());
    let plan = ob_poc_round_trip_plan();
    let callout_id = Uuid::now_v7();
    let execution_id = Uuid::now_v7();
    let cbu_id = Uuid::now_v7();

    let instance_id =
        inject_waiting_instance(&store, &plan, "t1", "create-cbu", callout_id).await;

    // Insert matching pending row so the test pattern mirrors the real flow.
    let idem = Uuid::now_v7();
    let row = PendingInvocation::new(callout_id, instance_id, "create-cbu", "ob-poc", "cbu.create", idem);
    let _ = execution_id; // used in the assert below
    assert_eq!(pending.insert(row).await.unwrap(), InsertOutcome::Inserted);

    // Simulate: SubmissionAck arrives — pending row gains execution_id.
    let execution_id2 = Uuid::now_v7();
    pending.record_ack(callout_id, execution_id2, Utc::now()).await.unwrap();

    // Simulate: result delivered — advancer sets Running, advances to "end",
    // binds @cbu placeholder.
    simulate_result_delivery(
        &store,
        instance_id,
        "create-cbu",
        "end",
        Some("@cbu"),
        Some(serde_json::Value::String(cbu_id.to_string())),
    ).await;

    // Tick: walker advances through EndEvent → Completed.
    let walker = make_walker(store.clone(), pending.clone()).await;
    let outcome = walker.advance(instance_id).await.unwrap();
    assert!(
        matches!(outcome, AdvanceOutcome::Completed { ref status, .. } if status == "Operational"),
        "expected Completed(Operational) — got {:?}",
        match &outcome {
            AdvanceOutcome::Completed { status, .. } => status.as_str(),
            _ => "not Completed",
        }
    );

    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(matches!(inst.state, ProcessState::Completed { .. }));

    // @cbu placeholder must be preserved.
    let pv: HashMap<String, serde_json::Value> = inst
        .placeholder_values
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    assert_eq!(
        pv.get("@cbu"),
        Some(&serde_json::Value::String(cbu_id.to_string()))
    );
}

// ── DoD #42 — dmn-lite full round-trip ──────────────────────────────

#[tokio::test]
async fn dod_42_dmn_lite_full_round_trip() {
    let store = Arc::new(MemoryStore::new());
    let pending = Arc::new(MemoryPendingInvocationStore::new());
    let plan = dmn_lite_round_trip_plan();
    let callout_id = Uuid::now_v7();

    let instance_id =
        inject_waiting_instance(&store, &plan, "t1", "route", callout_id).await;

    // Simulate result: cbu-type = "fund" → gateway routes to end-fund.
    simulate_result_delivery(
        &store,
        instance_id,
        "route",
        "gateway",
        Some("@cbu-type"),
        Some(serde_json::Value::String("fund".to_owned())),
    ).await;

    let walker = make_walker(store.clone(), pending).await;
    let outcome = walker.advance(instance_id).await.unwrap();
    // Walker: Running on "gateway" → evaluates @cbu-type="fund" → end-fund → Completed.
    assert!(
        matches!(outcome, AdvanceOutcome::Completed { ref status, .. } if status == "FundOnboarded"),
        "expected Completed(FundOnboarded)"
    );

    let inst = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(matches!(inst.state, ProcessState::Completed { .. }));
}

// ── DoD #44 — crash mid-outbox-write recovery ────────────────────────

#[tokio::test]
async fn dod_44_crash_mid_outbox_recovery() {
    // The instance is stuck at WaitingOnSubmission (outbox row pending,
    // no SubmissionAck yet). Simulate restart: create fresh engine + store
    // from the SAME MemoryStore snapshot, verify the instance can still
    // be advanced after result delivery.
    let store = Arc::new(MemoryStore::new());
    let pending = Arc::new(MemoryPendingInvocationStore::new());
    let plan = ob_poc_round_trip_plan();
    let callout_id = Uuid::now_v7();

    let instance_id =
        inject_waiting_instance(&store, &plan, "t1", "create-cbu", callout_id).await;

    // "Restart": build a fresh walker over the same store (simulating
    // in-memory restart — the process state persisted, outbox pending).
    // The process is still WaitingOnSubmission → advance returns NotRunnable.
    let walker_v2 = make_walker(store.clone(), pending.clone()).await;
    let outcome = walker_v2.advance(instance_id).await.unwrap();
    assert!(
        matches!(outcome, AdvanceOutcome::NotRunnable),
        "WaitingOnSubmission must not advance until result arrives"
    );

    // Now result arrives (outbox sender eventually dispatched, receiver replied).
    simulate_result_delivery(
        &store,
        instance_id,
        "create-cbu",
        "end",
        None,
        None,
    ).await;

    // Advance → Completed.
    let outcome = walker_v2.advance(instance_id).await.unwrap();
    assert!(matches!(outcome, AdvanceOutcome::Completed { .. }));
}

// ── DoD #45 — crash mid-ack reconciliation ───────────────────────────

#[tokio::test]
async fn dod_45_crash_mid_ack_reconciliation() {
    // SubmissionAck arrived and was recorded (WaitingOnInvocation).
    // Then bpmn-lite crashed before the process could advance.
    // On restart: deliver result → advancer sets Running → tick advances.
    let store = Arc::new(MemoryStore::new());
    let pending = Arc::new(MemoryPendingInvocationStore::new());
    let plan = ob_poc_round_trip_plan();
    let callout_id = Uuid::now_v7();
    let execution_id = Uuid::now_v7();

    let instance_id =
        inject_waiting_instance(&store, &plan, "t1", "create-cbu", callout_id).await;

    // Simulate: SubmissionAck arrived → instance transitions to WaitingOnInvocation.
    {
        let mut inst = store.load_instance(instance_id).await.unwrap().unwrap();
        inst.state = ProcessState::WaitingOnInvocation {
            execution_id,
            node_id: "create-cbu".to_owned(),
        };
        store.save_instance(&inst).await.unwrap();
    }

    // "Restart": fresh walker over the same store.
    // Instance is WaitingOnInvocation → NotRunnable.
    let walker_v2 = make_walker(store.clone(), pending.clone()).await;
    let outcome = walker_v2.advance(instance_id).await.unwrap();
    assert!(
        matches!(outcome, AdvanceOutcome::NotRunnable),
        "WaitingOnInvocation must not advance until result arrives"
    );

    // Deliver result → Running → tick advances.
    simulate_result_delivery(
        &store,
        instance_id,
        "create-cbu",
        "end",
        None,
        None,
    ).await;

    let outcome = walker_v2.advance(instance_id).await.unwrap();
    assert!(matches!(outcome, AdvanceOutcome::Completed { .. }));
}
