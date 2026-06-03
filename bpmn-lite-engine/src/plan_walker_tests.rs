use std::collections::{HashMap, BTreeMap};
use std::sync::Arc;
use bpmn_lite_compiler::dsl::plan::{
    ExecutionNode, WorkflowExecutionPlan, EndExecNode, PlaceholderSchema, StartExecNode, TaskExecNode, SplitExecNode, SplitExecFlow, DeliveryMode, SplitMode
};
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_store::pending::PendingInvocationStore;
use crate::plan_walker::{PlanWalker, AdvanceOutcome};
use bpmn_lite_store::pending::MemoryPendingInvocationStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::types::ProcessState;
use uuid::Uuid;

async fn make_walker(
    store: Arc<MemoryStore>,
    pending: Arc<MemoryPendingInvocationStore>,
) -> PlanWalker {
    let pool = sqlx::PgPool::connect_lazy("postgresql://localhost/plan_walker_test_fake").unwrap();
    let client = Arc::new(
        dsl_bus_client::BusClient::builder()
            .pool(pool)
            .local_domain("bpmn-lite")
            .build()
            .await
            .unwrap()
    );
    PlanWalker::new(store, pending, client)
}

/// Start→Task(ob-poc:cbu.create)→End plan.
fn ob_poc_round_trip_plan() -> WorkflowExecutionPlan {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "start".to_owned(),
        ExecutionNode::Start(StartExecNode {
            id: "start".to_owned(),
            next: "create-cbu".to_owned(),
        }),
    );
    nodes.insert(
        "create-cbu".to_owned(),
        ExecutionNode::Task(TaskExecNode {
            id: "create-cbu".to_owned(),
            plug: "ob-poc:cbu.create".to_owned(),
            delivery_mode: DeliveryMode::Blocking,
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
        ExecutionNode::End(EndExecNode {
            id: "end".to_owned(),
            status: "Operational".to_owned(),
        }),
    );
    WorkflowExecutionPlan {
        workflow_id: "custody-cbu-onboarding".to_owned(),
        nodes,
        start_node: "start".to_owned(),
        placeholder_schema: PlaceholderSchema::default(),
        closure_manifest: None,
        regime_version: None,
        mathematically_proved: true,
        unsafe_breeches: vec![],
        compiled_bytecode: None,
    }
}

/// Start→Task(dmn-lite:cbu_type_routing)→Gateway→…→End plan.
fn dmn_lite_round_trip_plan() -> WorkflowExecutionPlan {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "start".to_owned(),
        ExecutionNode::Start(StartExecNode {
            id: "start".to_owned(),
            next: "route".to_owned(),
        }),
    );
    nodes.insert(
        "route".to_owned(),
        ExecutionNode::Task(TaskExecNode {
            id: "route".to_owned(),
            plug: "dmn-lite:cbu_type_routing".to_owned(),
            delivery_mode: DeliveryMode::Blocking,
            static_args: HashMap::new(),
            next: "gateway".to_owned(),
            produces_placeholder: Some("@cbu-type".to_owned()),
            consumes_placeholders: vec![],
        }),
    );
    nodes.insert(
        "gateway".to_owned(),
        ExecutionNode::Split(SplitExecNode {
            id: "gateway".to_owned(),
            mode: SplitMode::Exclusive,
            routing_socket: None,
            flows: vec![
                SplitExecFlow {
                    placeholder: Some("@cbu-type".to_owned()),
                    expected_value: Some("fund".to_owned()),
                    next: "end-fund".to_owned(),
                },
                SplitExecFlow {
                    placeholder: Some("@cbu-type".to_owned()),
                    expected_value: Some("corporate".to_owned()),
                    next: "end-corporate".to_owned(),
                },
            ],
            join: "end".to_owned(),
            produces_placeholder: None,
        }),
    );
    nodes.insert(
        "end-fund".to_owned(),
        ExecutionNode::End(EndExecNode {
            id: "end-fund".to_owned(),
            status: "FundOnboarded".to_owned(),
        }),
    );
    nodes.insert(
        "end-corporate".to_owned(),
        ExecutionNode::End(EndExecNode {
            id: "end-corporate".to_owned(),
            status: "CorporateOnboarded".to_owned(),
        }),
    );
    WorkflowExecutionPlan {
        workflow_id: "cbu-type-routing".to_owned(),
        nodes,
        start_node: "start".to_owned(),
        placeholder_schema: PlaceholderSchema::default(),
        closure_manifest: None,
        regime_version: None,
        mathematically_proved: true,
        unsafe_breeches: vec![],
        compiled_bytecode: None,
    }
}

async fn simulate_result_delivery(
    store: &Arc<MemoryStore>,
    instance_id: Uuid,
    _node_id: &str,
    target_next_node_id: &str,
    produces_placeholder: Option<&str>,
    placeholder_val: Option<&str>,
) {
    let mut inst = store.load_instance(instance_id).await.unwrap().unwrap();
    inst.state = ProcessState::Running;
    inst.current_node_id = Some(target_next_node_id.to_string());
    if let Some(placeholder) = produces_placeholder {
        let mut pv = inst.placeholder_values
            .as_ref()
            .and_then(|v| serde_json::from_value::<HashMap<String, serde_json::Value>>(v.clone()).ok())
            .unwrap_or_default();
        if let Some(val) = placeholder_val {
            pv.insert(placeholder.to_string(), serde_json::Value::String(val.to_string()));
        }
        inst.placeholder_values = Some(serde_json::to_value(pv).unwrap());
    }
    store.save_instance("default", &inst).await.unwrap();
}

#[tokio::test]
async fn test_ob_poc_round_trip_success() {
    let store = Arc::new(MemoryStore::new());
    let pending = Arc::new(MemoryPendingInvocationStore::new());
    let walker = make_walker(store.clone(), pending.clone()).await;

    let plan = ob_poc_round_trip_plan();
    let instance_id = walker.start_process("default", &plan, "default", HashMap::new(), HashMap::new()).await.unwrap();

    let outcome = walker.advance(instance_id, "default").await.unwrap();
    assert!(matches!(outcome, AdvanceOutcome::Submitted { .. }));
    if let AdvanceOutcome::Submitted { callout_id, node_id, verb_fqn } = outcome {
        assert_eq!(node_id, "create-cbu");
        assert_eq!(verb_fqn, "ob-poc:cbu.create");
        let row = store.pending_store.list_for_process(instance_id).await.unwrap()[0].clone();
        assert_eq!(row.callout_id, callout_id);
    }

    simulate_result_delivery(
        &store,
        instance_id,
        "create-cbu",
        "end",
        Some("@cbu"),
        Some("cbu-123"),
    ).await;

    let outcome2 = walker.advance(instance_id, "default").await.unwrap();
    assert!(matches!(outcome2, AdvanceOutcome::Completed { .. }));
    if let AdvanceOutcome::Completed { node_id, status } = outcome2 {
        assert_eq!(node_id, "end");
        assert_eq!(status, "Operational");
    }
}

#[tokio::test]
async fn test_dmn_lite_round_trip_fund_path() {
    let store = Arc::new(MemoryStore::new());
    let pending = Arc::new(MemoryPendingInvocationStore::new());
    let walker = make_walker(store.clone(), pending.clone()).await;

    let plan = dmn_lite_round_trip_plan();
    let instance_id = walker.start_process("default", &plan, "default", HashMap::new(), HashMap::new()).await.unwrap();

    let outcome = walker.advance(instance_id, "default").await.unwrap();
    assert!(matches!(outcome, AdvanceOutcome::Submitted { .. }));
    if let AdvanceOutcome::Submitted { node_id, verb_fqn, .. } = outcome {
        assert_eq!(node_id, "route");
        assert_eq!(verb_fqn, "dmn-lite:cbu_type_routing");
    }

    simulate_result_delivery(
        &store,
        instance_id,
        "route",
        "gateway",
        Some("@cbu-type"),
        Some("fund"),
    ).await;

    let outcome2 = walker.advance(instance_id, "default").await.unwrap();
    assert!(matches!(outcome2, AdvanceOutcome::Completed { .. }));
    if let AdvanceOutcome::Completed { node_id, status } = outcome2 {
        assert_eq!(node_id, "end-fund");
        assert_eq!(status, "FundOnboarded");
    }
}

#[tokio::test]
async fn test_dmn_lite_round_trip_corporate_path() {
    let store = Arc::new(MemoryStore::new());
    let pending = Arc::new(MemoryPendingInvocationStore::new());
    let walker = make_walker(store.clone(), pending.clone()).await;

    let plan = dmn_lite_round_trip_plan();
    let instance_id = walker.start_process("default", &plan, "default", HashMap::new(), HashMap::new()).await.unwrap();

    let _outcome = walker.advance(instance_id, "default").await.unwrap();
    simulate_result_delivery(
        &store,
        instance_id,
        "route",
        "gateway",
        Some("@cbu-type"),
        Some("corporate"),
    ).await;

    let outcome2 = walker.advance(instance_id, "default").await.unwrap();
    assert!(matches!(outcome2, AdvanceOutcome::Completed { .. }));
    if let AdvanceOutcome::Completed { node_id, status } = outcome2 {
        assert_eq!(node_id, "end-corporate");
        assert_eq!(status, "CorporateOnboarded");
    }
}

#[tokio::test]
async fn test_start_process_rejects_invalid_plan() {
    let store = Arc::new(bpmn_lite_store::store_memory::MemoryStore::new());
    let pending = Arc::new(bpmn_lite_store::pending::MemoryPendingInvocationStore::new());
    let walker = make_walker(store, pending).await;

    let mut plan = ob_poc_round_trip_plan();
    if let Some(ExecutionNode::Task(ref mut st)) = plan.nodes.get_mut("create-cbu") {
        st.consumes_placeholders = vec!["@unproduced_placeholder".to_string()];
    }

    let result = walker.start_process("default", &plan, "default", HashMap::new(), HashMap::new()).await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(err_msg.contains("Path-family validation failed"), "got: {}", err_msg);
}

#[tokio::test]
async fn test_precondition_violation() {
    let store = Arc::new(bpmn_lite_store::store_memory::MemoryStore::new());
    let pending = Arc::new(bpmn_lite_store::pending::MemoryPendingInvocationStore::new());
    let walker = make_walker(store, pending).await;

    let plan = ob_poc_round_trip_plan();

    let mut preconditions = HashMap::new();
    preconditions.insert("@cbu-status".to_string(), "Draft".to_string());

    let mut initial_vars = HashMap::new();
    initial_vars.insert("@cbu-status".to_string(), serde_json::Value::String("Approved".to_string()));

    let result = walker.start_process("default", &plan, "default", initial_vars, preconditions).await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(err_msg.contains("PreconditionConflict"), "got: {}", err_msg);
}

#[tokio::test]
async fn test_regime_mismatch() {
    let store = Arc::new(bpmn_lite_store::store_memory::MemoryStore::new());
    let pending = Arc::new(bpmn_lite_store::pending::MemoryPendingInvocationStore::new());
    let walker = make_walker(store, pending).await;

    let mut plan = ob_poc_round_trip_plan();
    plan.regime_version = Some("v1.2".to_string());

    std::env::set_var("BPMN_LITE_REGIME_VERSION", "v1.3");

    let result = walker.start_process("default", &plan, "default", HashMap::new(), HashMap::new()).await;
    std::env::remove_var("BPMN_LITE_REGIME_VERSION");

    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(err_msg.contains("RegimeMismatch"), "got: {}", err_msg);
}

#[tokio::test]
async fn test_call_activity_in_process_execution() {
    let store = Arc::new(bpmn_lite_store::store_memory::MemoryStore::new());
    let pending = Arc::new(bpmn_lite_store::pending::MemoryPendingInvocationStore::new());
    let walker = make_walker(store.clone(), pending).await;

    // 1. Build and store the child plan
    let mut child_nodes = BTreeMap::new();
    child_nodes.insert(
        "start".to_owned(),
        ExecutionNode::Start(StartExecNode {
            id: "start".to_owned(),
            next: "end".to_owned(),
        }),
    );
    child_nodes.insert(
        "end".to_owned(),
        ExecutionNode::End(EndExecNode {
            id: "end".to_owned(),
            status: "ChildDone".to_owned(),
        }),
    );
    let child_plan = WorkflowExecutionPlan {
        workflow_id: "child-wf".to_owned(),
        nodes: child_nodes,
        start_node: "start".to_owned(),
        placeholder_schema: PlaceholderSchema::default(),
        closure_manifest: None,
        regime_version: None,
        mathematically_proved: true,
        unsafe_breeches: vec![],
        compiled_bytecode: None,
    };
    let child_plan_json = serde_json::to_string(&child_plan).unwrap();
    let child_hash_bytes = blake3::hash(child_plan_json.as_bytes());
    let child_hash = *child_hash_bytes.as_bytes();
    let child_hash_hex = hex::encode(child_hash);
    store.store_plan(child_hash, &child_plan_json).await.unwrap();

    // 2. Build and store the parent plan invoking child by hash hex
    let mut parent_nodes = BTreeMap::new();
    parent_nodes.insert(
        "start".to_owned(),
        ExecutionNode::Start(StartExecNode {
            id: "start".to_owned(),
            next: "call-child".to_owned(),
        }),
    );
    parent_nodes.insert(
        "call-child".to_owned(),
        ExecutionNode::Task(TaskExecNode {
            id: "call-child".to_owned(),
            plug: child_hash_hex.clone(),
            delivery_mode: DeliveryMode::Blocking,
            static_args: HashMap::new(),
            next: "end".to_owned(),
            produces_placeholder: None,
            consumes_placeholders: vec![],
        }),
    );
    parent_nodes.insert(
        "end".to_owned(),
        ExecutionNode::End(EndExecNode {
            id: "end".to_owned(),
            status: "ParentDone".to_owned(),
        }),
    );
    let parent_plan = WorkflowExecutionPlan {
        workflow_id: "parent-wf".to_owned(),
        nodes: parent_nodes,
        start_node: "start".to_owned(),
        placeholder_schema: PlaceholderSchema::default(),
        closure_manifest: None,
        regime_version: None,
        mathematically_proved: true,
        unsafe_breeches: vec![],
        compiled_bytecode: None,
    };

    // 3. Start parent process
    let parent_id = walker.start_process("default", &parent_plan, "default", HashMap::new(), HashMap::new()).await.unwrap();
    
    // 4. Advance parent - should spawn child and transition parent to WaitingOnInvocation
    let outcome = walker.advance(parent_id, "default").await.unwrap();
    let child_id = match outcome {
        AdvanceOutcome::Submitted { callout_id, node_id, verb_fqn } => {
            assert_eq!(node_id, "call-child");
            assert_eq!(verb_fqn, child_hash_hex);
            callout_id
        }
        _ => panic!("Expected Submitted outcome"),
    };

    // Verify parent is WaitingOnInvocation and child is Running
    let parent_inst = store.load_instance(parent_id).await.unwrap().unwrap();
    assert_eq!(parent_inst.state, ProcessState::WaitingOnInvocation {
        execution_id: child_id,
        node_id: "call-child".to_string(),
    });

    let child_inst = store.load_instance(child_id).await.unwrap().unwrap();
    assert_eq!(child_inst.state, ProcessState::Running);
    assert_eq!(child_inst.correlation_id, format!("{}:call-child", parent_id));

    // 5. Advance child - completes child
    let child_outcome = walker.advance(child_id, "default").await.unwrap();
    assert!(matches!(child_outcome, AdvanceOutcome::Completed { .. }));

    let child_inst_post = store.load_instance(child_id).await.unwrap().unwrap();
    assert!(matches!(child_inst_post.state, ProcessState::Completed { .. }));

    // Verify parent is still WaitingOnInvocation before we advance it
    let parent_inst_mid = store.load_instance(parent_id).await.unwrap().unwrap();
    assert!(matches!(parent_inst_mid.state, ProcessState::WaitingOnInvocation { .. }));

    // 6. Resumer / Scheduler wakes up parent. Calling advance on parent 
    // will now evaluate the child's completion state, transition parent to Running,
    // and complete the parent process.
    let final_outcome = walker.advance(parent_id, "default").await.unwrap();
    assert!(matches!(final_outcome, AdvanceOutcome::Completed { .. }));

    let parent_inst_final = store.load_instance(parent_id).await.unwrap().unwrap();
    assert!(matches!(parent_inst_final.state, ProcessState::Completed { .. }));
}

#[tokio::test]
async fn test_dependency_cycle_detection() {
    let store = Arc::new(bpmn_lite_store::store_memory::MemoryStore::new());

    // 1. We construct parent plan A and child plan B.
    // Plan A will call Plan B's hash hex.
    // Plan B will call Plan A's hash hex.

    let hash_a_hex = "a".repeat(64);
    let hash_b_hex = "b".repeat(64);

    let mut nodes_a = BTreeMap::new();
    nodes_a.insert(
        "start".to_owned(),
        ExecutionNode::Start(StartExecNode {
            id: "start".to_owned(),
            next: "call-b".to_owned(),
        }),
    );
    nodes_a.insert(
        "call-b".to_owned(),
        ExecutionNode::Task(TaskExecNode {
            id: "call-b".to_owned(),
            plug: hash_b_hex.clone(),
            delivery_mode: DeliveryMode::Blocking,
            static_args: HashMap::new(),
            next: "end".to_owned(),
            produces_placeholder: None,
            consumes_placeholders: vec![],
        }),
    );
    nodes_a.insert(
        "end".to_owned(),
        ExecutionNode::End(EndExecNode {
            id: "end".to_owned(),
            status: "Completed".to_owned(),
        }),
    );

    let plan_a = WorkflowExecutionPlan {
        workflow_id: "plan-a".to_owned(),
        nodes: nodes_a,
        start_node: "start".to_owned(),
        placeholder_schema: PlaceholderSchema::default(),
        closure_manifest: None,
        regime_version: None,
        mathematically_proved: true,
        unsafe_breeches: vec![],
        compiled_bytecode: None,
    };

    let mut nodes_b = BTreeMap::new();
    nodes_b.insert(
        "start".to_owned(),
        ExecutionNode::Start(StartExecNode {
            id: "start".to_owned(),
            next: "call-a".to_owned(),
        }),
    );
    nodes_b.insert(
        "call-a".to_owned(),
        ExecutionNode::Task(TaskExecNode {
            id: "call-a".to_owned(),
            plug: hash_a_hex.clone(),
            delivery_mode: DeliveryMode::Blocking,
            static_args: HashMap::new(),
            next: "end".to_owned(),
            produces_placeholder: None,
            consumes_placeholders: vec![],
        }),
    );
    nodes_b.insert(
        "end".to_owned(),
        ExecutionNode::End(EndExecNode {
            id: "end".to_owned(),
            status: "Completed".to_owned(),
        }),
    );

    let plan_b = WorkflowExecutionPlan {
        workflow_id: "plan-b".to_owned(),
        nodes: nodes_b,
        start_node: "start".to_owned(),
        placeholder_schema: PlaceholderSchema::default(),
        closure_manifest: None,
        regime_version: None,
        mathematically_proved: true,
        unsafe_breeches: vec![],
        compiled_bytecode: None,
    };

    let hash_a_bytes = hex::decode(&hash_a_hex).unwrap();
    let hash_b_bytes = hex::decode(&hash_b_hex).unwrap();
    let mut arr_a = [0u8; 32];
    arr_a.copy_from_slice(&hash_a_bytes);
    let mut arr_b = [0u8; 32];
    arr_b.copy_from_slice(&hash_b_bytes);

    let plan_a_json = serde_json::to_string(&plan_a).unwrap();
    let plan_b_json = serde_json::to_string(&plan_b).unwrap();

    store.store_plan(arr_a, &plan_a_json).await.unwrap();
    store.store_plan(arr_b, &plan_b_json).await.unwrap();

    let res = crate::plan_walker::preload_workflow_dependencies(&plan_a, store.as_ref()).await;
    assert!(res.is_err());
    let err_msg = res.err().unwrap().to_string();
    assert!(err_msg.contains("Cyclic workflow dependency detected"), "got: {}", err_msg);
}
