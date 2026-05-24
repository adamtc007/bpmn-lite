//! T4 — Integration tests for the §10 custody-cbu-onboarding demo workflow.
//!
//! These tests exercise the parts of the walker that can be verified without
//! a real Postgres pool (no live gRPC bus required):
//!
//! 1. `PlanWalker::start_process` creates the correct initial instance state.
//! 2. The ExclusiveGateway correctly routes each of the three demo paths
//!    (fund / corporate / trust) based on `@cbu-type`.
//! 3. The EndEvent produces `Completed("Operational")`.
//!
//! The callout dispatch steps (create-cbu, type-decision, add-*, attach-im)
//! require a Postgres pool for `insert_outbox`; those are bypassed here by
//! injecting the instance state that the advancer would produce after result
//! delivery. The dispatch mechanics themselves are covered by T3.7 (DoD #41
//! and #42) which already proves the full callout lifecycle end-to-end.

use std::collections::HashMap;
use std::sync::Arc;

use bpmn_lite_store::pending::MemoryPendingInvocationStore;
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::types::ProcessState;
use uuid::Uuid;

use crate::demo::{build_demo_plan, demo_initial_vars};
use crate::plan_walker::{AdvanceOutcome, PlanWalker};

// ── Test helper ──────────────────────────────────────────────────────

async fn make_walker(
    store: Arc<MemoryStore>,
    pending: Arc<MemoryPendingInvocationStore>,
) -> PlanWalker {
    let pool =
        sqlx::PgPool::connect_lazy("postgresql://localhost/demo_integration_test_fake").unwrap();
    let client = Arc::new(
        dsl_bus_client::BusClient::builder()
            .pool(pool)
            .local_domain("bpmn-lite")
            .build()
            .await
            .expect("test BusClient"),
    );
    PlanWalker::new(store, pending, client)
}

/// Inject `instance.state = Running`, `current_node_id = node`, and
/// `placeholder_values = placeholders` into the store. Mirrors what
/// `StoreBackedAdvancer` does after result delivery.
async fn inject_running(
    store: &MemoryStore,
    instance_id: Uuid,
    node_id: &str,
    placeholders: HashMap<String, serde_json::Value>,
) {
    let mut inst = store.load_instance(instance_id).await.unwrap().unwrap();
    inst.state = ProcessState::Running;
    inst.current_node_id = Some(node_id.to_owned());
    if !placeholders.is_empty() {
        let existing: HashMap<String, serde_json::Value> = inst
            .placeholder_values
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let mut merged = existing;
        merged.extend(placeholders);
        inst.placeholder_values = serde_json::to_value(&merged).ok();
    }
    store.save_instance(&inst).await.unwrap();
}

// ── Tests ────────────────────────────────────────────────────────────

/// T4 assertion 1: `start_process` correctly initialises the instance.
#[tokio::test]
async fn t4_start_process_creates_correct_initial_state() {
    let store = Arc::new(MemoryStore::new());
    let pending = Arc::new(MemoryPendingInvocationStore::new());
    let plan = build_demo_plan().expect("§10 compile");
    let walker = make_walker(store.clone(), pending).await;

    let id = walker
        .start_process(&plan, "demo", demo_initial_vars("Allianz AM", "FUND_MANDATE"))
        .await
        .unwrap();

    let inst = store.load_instance(id).await.unwrap().unwrap();
    assert!(inst.plan_hash.is_some(), "plan_hash must be set");
    assert_eq!(inst.current_node_id.as_deref(), Some("start"));
    assert!(matches!(inst.state, ProcessState::Running));

    // Initial vars must be in placeholder_values.
    let pv: HashMap<String, serde_json::Value> = inst
        .placeholder_values
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    assert!(pv.contains_key("@input-name"));
    assert!(pv.contains_key("@input-client-type"));
}

/// T4 core test — runs the full §10 demo workflow for one client type.
///
/// Callout dispatch nodes (create-cbu, type-decision, add-*, attach-im) are
/// bypassed by injecting the post-result-delivery state directly. The walker
/// is called for nodes that don't dispatch: `StartEvent`, `ExclusiveGateway`,
/// and `EndEvent`.
///
/// Assertions:
/// - Gateway correctly routes @cbu-type to the right product task.
/// - EndEvent produces `Completed("Operational")`.
/// - @cbu and @cbu-type placeholders are preserved at completion.
async fn run_full_path_demo(client_type_input: &str, cbu_type_output: &str) {
    let store = Arc::new(MemoryStore::new());
    let pending = Arc::new(MemoryPendingInvocationStore::new());
    let plan = build_demo_plan().expect("§10 compile");
    let walker = make_walker(store.clone(), pending).await;

    let cbu_id = Uuid::now_v7();
    let id = walker
        .start_process(&plan, "demo", demo_initial_vars("Test Client", client_type_input))
        .await
        .unwrap();

    // Bypass callout nodes by injecting the state that advancer would produce:
    //   create-cbu result → @cbu = cbu_id, advance to type-decision
    //   type-decision result → @cbu-type = cbu_type_output, advance to type-gateway
    inject_running(
        &store, id, "type-gateway",
        HashMap::from([
            ("@cbu".to_owned(), serde_json::Value::String(cbu_id.to_string())),
            ("@cbu-type".to_owned(), serde_json::Value::String(cbu_type_output.to_owned())),
        ]),
    ).await;

    // `advance()` processes the ExclusiveGateway — evaluates @cbu-type,
    // moves current_node_id to the matching add-* task, then tries to
    // dispatch that task (which fails on MemoryStore). NotRunnable is
    // returned but the current_node was already advanced by the gateway.
    let _ = walker.advance(id).await.unwrap();
    let inst_after_gw = store.load_instance(id).await.unwrap().unwrap();
    let expected_add = match cbu_type_output {
        "corporate" => "add-corp".to_owned(),
        other => format!("add-{other}"),
    };
    assert_eq!(
        inst_after_gw.current_node_id.as_deref(),
        Some(expected_add.as_str()),
        "gateway routed to wrong product task for cbu_type={cbu_type_output}"
    );

    // Bypass the add-* and attach-im callout nodes; inject Running at "end".
    inject_running(&store, id, "end", HashMap::new()).await;

    // `advance()` processes EndEvent → Completed("Operational").
    let outcome = walker.advance(id).await.unwrap();
    assert!(
        matches!(outcome, AdvanceOutcome::Completed { ref status, .. } if status == "Operational"),
        "expected Completed(Operational), got {:?}",
        match &outcome {
            AdvanceOutcome::Completed { status, .. } => format!("Completed({status})"),
            AdvanceOutcome::Submitted { node_id, .. } => format!("Submitted({node_id})"),
            AdvanceOutcome::NotRunnable => "NotRunnable".into(),
        }
    );

    let inst = store.load_instance(id).await.unwrap().unwrap();
    assert!(matches!(inst.state, ProcessState::Completed { .. }));

    // Placeholders must survive the full walk.
    let pv: HashMap<String, serde_json::Value> = inst
        .placeholder_values
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    assert_eq!(
        pv.get("@cbu"),
        Some(&serde_json::Value::String(cbu_id.to_string())),
        "@cbu lost during walk"
    );
    assert_eq!(
        pv.get("@cbu-type"),
        Some(&serde_json::Value::String(cbu_type_output.to_owned())),
        "@cbu-type lost during walk"
    );
}

#[tokio::test]
async fn t4_fund_path_gateway_routes_correctly_and_completes() {
    run_full_path_demo("FUND_MANDATE", "fund").await;
}

#[tokio::test]
async fn t4_corporate_path_gateway_routes_correctly_and_completes() {
    run_full_path_demo("CORPORATE", "corporate").await;
}

#[tokio::test]
async fn t4_trust_path_gateway_routes_correctly_and_completes() {
    run_full_path_demo("TRUST", "trust").await;
}

/// T4 reset helper stub — documented for T7 Docker reset script.
#[test]
fn t4_reset_demo_state_is_documented() {
    assert_eq!(
        crate::demo::reset_demo_state_comment(),
        "For Postgres: truncate bpmn_process_instance, bpmn_pending_invocation, outbox, inbox. \
         For MemoryStore: create a fresh Arc<MemoryStore>."
    );
}
