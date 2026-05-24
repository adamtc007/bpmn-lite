//! REST + SSE demo API for the bpmn-lite federated stack (T6).
//!
//! Runs on port 8080 alongside the existing gRPC server (50051).
//! Backed by `MemoryStore` — demo-mode only, no Postgres required.
//! For production process queries use the gRPC surface.
//!
//! ## Endpoints
//!
//! ```text
//! GET  /bpmn/health
//! GET  /bpmn/instances               → Vec<WorkflowInstanceSummary>
//! GET  /bpmn/instances/:id           → WorkflowInstanceDetail
//! POST /bpmn/instances/start         → { cbu_type: "fund"|"corporate"|"trust" }
//! POST /bpmn/instances/:id/next-step → advance one demo step
//! DELETE /bpmn/instances             → reset demo state
//! ```
//!
//! **Cross-domain visibility (T6):** every `NodeInfo` in the response
//! includes `target_domain` (e.g., `"ob-poc"`, `"dmn-lite"`) and `fqn`
//! (e.g., `"ob-poc:cbu.create"`) derived from the plan node. The React
//! `WorkflowPanel` renders this as "Calling ob-poc:cbu.create" etc.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use bpmn_lite_compiler::dsl::plan::{ExecutionNode, WorkflowExecutionPlan};
use bpmn_lite_engine::demo::{build_demo_plan, demo_initial_vars};
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::types::{ProcessInstance, ProcessState};
use bpmn_lite_types::session_stack::SessionStackState;

// ── Demo state ─────────────────────────────────────────────────────────

pub(crate) struct DemoState {
    store: Arc<MemoryStore>,
    plan: Arc<WorkflowExecutionPlan>,
    cbu_types: Mutex<HashMap<Uuid, String>>,
}

impl DemoState {
    pub(crate) fn new() -> Arc<Self> {
        let plan = build_demo_plan().expect("§10 demo plan must compile");
        Arc::new(Self {
            store: Arc::new(MemoryStore::new()),
            plan: Arc::new(plan),
            cbu_types: Mutex::new(HashMap::new()),
        })
    }

    fn cbu_type(&self, id: Uuid) -> String {
        self.cbu_types.lock().unwrap().get(&id).cloned().unwrap_or_default()
    }

    fn set_cbu_type(&self, id: Uuid, t: String) {
        self.cbu_types.lock().unwrap().insert(id, t);
    }
}

// ── Wire-types ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct WorkflowInstanceSummary {
    id: String,
    workflow_id: String,
    current_node: String,
    status: String,
    cbu_type: String,
}

#[derive(Serialize)]
pub(crate) struct NodeInfo {
    id: String,
    label: String,
    fqn: Option<String>,
    target_domain: Option<String>,
    kind: String,
}

#[derive(Serialize)]
pub(crate) struct WorkflowInstanceDetail {
    id: String,
    workflow_id: String,
    current_node: String,
    status: String,
    variables: serde_json::Value,
    cbu_type: String,
    nodes: Vec<NodeInfo>,
    sage_records: Vec<()>,
}

#[derive(Deserialize)]
pub(crate) struct StartBody {
    cbu_type: String,
}

// ── Router ──────────────────────────────────────────────────────────────

pub(crate) fn demo_router(state: Arc<DemoState>) -> Router {
    Router::new()
        .route("/bpmn/health", get(health))
        .route("/bpmn/instances", get(list_instances).delete(reset_instances))
        .route("/bpmn/instances/start", post(start_instance))
        .route("/bpmn/instances/:id", get(get_instance))
        .route("/bpmn/instances/:id/next-step", post(next_step))
        .route("/bpmn/instances/:id/sage", get(list_sage_stub))
        .route("/bpmn/instances/:id/events", get(events_stub))
        .with_state(state)
}

// ── Handlers ────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok", "service": "bpmn-lite-demo" }))
}

async fn list_instances(State(demo): State<Arc<DemoState>>) -> impl IntoResponse {
    let ids = demo.store.list_running_instances("demo").await.unwrap_or_default();
    let mut result: Vec<WorkflowInstanceSummary> = Vec::new();
    for id in ids {
        if let Ok(Some(inst)) = demo.store.load_instance(id).await {
            result.push(WorkflowInstanceSummary {
                id: id.to_string(),
                workflow_id: inst.process_key.clone(),
                current_node: inst.current_node_id.clone().unwrap_or_default(),
                status: format_state(&inst.state),
                cbu_type: demo.cbu_type(id),
            });
        }
    }
    Json(result)
}

async fn get_instance(
    State(demo): State<Arc<DemoState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let inst = match demo.store.load_instance(id).await {
        Ok(Some(i)) => i,
        _ => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"})))
                .into_response()
        }
    };
    let variables = inst
        .placeholder_values
        .clone()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let detail = WorkflowInstanceDetail {
        id: id.to_string(),
        workflow_id: inst.process_key.clone(),
        current_node: inst.current_node_id.clone().unwrap_or_default(),
        status: format_state(&inst.state),
        variables,
        cbu_type: demo.cbu_type(id),
        nodes: build_node_infos(&demo.plan),
        sage_records: vec![],
    };
    Json(detail).into_response()
}

async fn start_instance(
    State(demo): State<Arc<DemoState>>,
    Json(body): Json<StartBody>,
) -> impl IntoResponse {
    let client_type_input = match body.cbu_type.as_str() {
        "fund" => "FUND_MANDATE",
        "corporate" => "CORPORATE",
        "trust" => "TRUST",
        other => other,
    };
    let vars = demo_initial_vars("Demo Client", client_type_input);
    match create_instance(&demo.store, &demo.plan, "demo", vars).await {
        Ok(id) => {
            demo.set_cbu_type(id, body.cbu_type);
            // Walk past StartEvent to the first callout node.
            drive_forward(&demo.store, &demo.plan, id).await;
            Json(serde_json::json!({ "instance_id": id.to_string() })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn next_step(
    State(demo): State<Arc<DemoState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let inst = match demo.store.load_instance(id).await {
        Ok(Some(i)) => i,
        _ => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"})))
                .into_response()
        }
    };

    let node_id = inst.current_node_id.clone().unwrap_or_default();
    let cbu_type = demo.cbu_type(id);

    // Simulate result delivery for callout nodes, then drive forward through
    // any immediately following gateways/end events without touching the bus.
    match demo.plan.nodes.get(&node_id) {
        Some(ExecutionNode::ServiceTask(t)) => {
            let placeholder = t.produces_placeholder.as_deref().map(|name| {
                let val = if node_id == "create-cbu" {
                    serde_json::Value::String(Uuid::now_v7().to_string())
                } else {
                    serde_json::Value::String(format!("{node_id}-done"))
                };
                (name, val)
            });
            apply_step(&demo.store, id, t.next.clone(), placeholder).await;
        }
        Some(ExecutionNode::BusinessRuleTask(t)) => {
            let cbu_type_val = match cbu_type.as_str() {
                "fund" => "fund",
                "corporate" => "corporate",
                "trust" => "trust",
                _ => "fund",
            };
            let placeholder = t
                .produces_placeholder
                .as_deref()
                .map(|name| (name, serde_json::Value::String(cbu_type_val.to_owned())));
            apply_step(&demo.store, id, t.next.clone(), placeholder).await;
        }
        _ => {}
    }

    // Drive forward through gateways and end events without the bus.
    drive_forward(&demo.store, &demo.plan, id).await;

    let updated = demo.store.load_instance(id).await.ok().flatten();
    let (current, status) = updated
        .map(|i| (
            i.current_node_id.clone().unwrap_or_default(),
            format_state(&i.state),
        ))
        .unwrap_or_default();

    Json(serde_json::json!({
        "node": current,
        "status": status,
        "message": format!("Advanced to {current}")
    }))
    .into_response()
}

/// T5 placeholder — Sage reasoning records.
async fn list_sage_stub(Path(_id): Path<Uuid>) -> impl IntoResponse {
    Json(Vec::<serde_json::Value>::new())
}

/// SSE event stream stub — emits a single heartbeat then idles.
/// The React client polls on a 2s interval anyway; this just keeps EventSource
/// connected without raising console errors.
async fn events_stub(Path(_id): Path<Uuid>) -> impl IntoResponse {
    use axum::http::header;
    (
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        ": heartbeat\n\n",
    )
}

async fn reset_instances(State(demo): State<Arc<DemoState>>) -> impl IntoResponse {
    demo.cbu_types.lock().unwrap().clear();
    tracing::info!("Demo state reset (in-memory)");
    StatusCode::NO_CONTENT
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn format_state(s: &ProcessState) -> String {
    match s {
        ProcessState::Running => "Running".into(),
        ProcessState::WaitingOnSubmission { node_id, .. } => {
            format!("WaitingOnSubmission({})", node_id)
        }
        ProcessState::WaitingOnInvocation { node_id, .. } => {
            format!("WaitingOnInvocation({})", node_id)
        }
        ProcessState::Completed { .. } => "Completed".into(),
        ProcessState::Failed { .. } => "Failed".into(),
        ProcessState::Cancelled { .. } => "Cancelled".into(),
        ProcessState::Terminated { .. } => "Terminated".into(),
    }
}

/// Inline equivalent of PlanWalker::start_process that doesn't require a
/// BusClient — the REST demo never dispatches over the bus.
async fn create_instance(
    store: &MemoryStore,
    plan: &WorkflowExecutionPlan,
    tenant_id: &str,
    initial_variables: HashMap<String, serde_json::Value>,
) -> anyhow::Result<Uuid> {
    let plan_json = serde_json::to_string(plan)?;
    let hash = *blake3::hash(plan_json.as_bytes()).as_bytes();
    store.store_plan(hash, &plan_json).await?;

    let instance_id = Uuid::now_v7();
    let placeholder_values = if initial_variables.is_empty() {
        None
    } else {
        Some(serde_json::to_value(&initial_variables)?)
    };

    let instance = ProcessInstance {
        instance_id,
        tenant_id: tenant_id.to_owned(),
        process_key: plan.workflow_id.clone(),
        bytecode_version: [0u8; 32],
        domain_payload: "{}".into(),
        domain_payload_hash: [0u8; 32],
        session_stack: SessionStackState::default(),
        flags: Default::default(),
        counters: Default::default(),
        join_expected: Default::default(),
        state: ProcessState::Running,
        correlation_id: String::new(),
        entry_id: Uuid::nil(),
        runbook_id: Uuid::nil(),
        created_at: chrono::Utc::now().timestamp_millis(),
        integrity_hash: None,
        quarantine_state: None,
        plan_hash: Some(hash),
        current_node_id: Some(plan.start_node.clone()),
        placeholder_values,
    };
    store.save_instance(&instance).await?;
    Ok(instance_id)
}

/// Walk forward through non-callout nodes (StartEvent, ExclusiveGateway,
/// EndEvent) without touching the bus. Stops at the first ServiceTask or
/// BusinessRuleTask so the user can click "Next Step" there.
async fn drive_forward(store: &MemoryStore, plan: &WorkflowExecutionPlan, id: Uuid) {
    loop {
        let Ok(Some(mut inst)) = store.load_instance(id).await else { break };
        if !matches!(inst.state, ProcessState::Running) {
            break;
        }
        let node_id = match inst.current_node_id.clone() {
            Some(n) => n,
            None => break,
        };
        let pv: HashMap<String, serde_json::Value> = inst
            .placeholder_values
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        match plan.nodes.get(&node_id) {
            Some(ExecutionNode::StartEvent(n)) => {
                inst.current_node_id = Some(n.next.clone());
                let _ = store.save_instance(&inst).await;
            }
            Some(ExecutionNode::ExclusiveGateway(gw)) => {
                let chosen = gw.flows.iter().find(|f| {
                    pv.get(&f.placeholder).and_then(|v| v.as_str())
                        == Some(f.expected_value.as_str())
                });
                if let Some(flow) = chosen {
                    inst.current_node_id = Some(flow.next.clone());
                    let _ = store.save_instance(&inst).await;
                } else {
                    break;
                }
            }
            Some(ExecutionNode::EndEvent(end)) => {
                inst.state = ProcessState::Completed {
                    at: chrono::Utc::now().timestamp_millis(),
                };
                inst.current_node_id = Some(end.id.clone());
                let _ = store.save_instance(&inst).await;
                break;
            }
            // ServiceTask / BusinessRuleTask — stop here, user drives next step.
            _ => break,
        }
    }
}

fn build_node_infos(plan: &WorkflowExecutionPlan) -> Vec<NodeInfo> {
    let ordered = [
        "start",
        "create-cbu",
        "type-decision",
        "type-gateway",
        "add-fund",
        "add-corp",
        "add-trust",
        "attach-im",
        "end",
    ];
    ordered
        .iter()
        .filter_map(|id| {
            let node = plan.nodes.get(*id)?;
            Some(match node {
                ExecutionNode::StartEvent(_) => NodeInfo {
                    id: (*id).to_owned(),
                    label: "Start".into(),
                    fqn: None,
                    target_domain: None,
                    kind: "start".into(),
                },
                ExecutionNode::ServiceTask(t) => {
                    let (domain, verb_id) = split_fqn(&t.verb_fqn);
                    NodeInfo {
                        id: (*id).to_owned(),
                        label: format!("↗ Calling {domain}: {verb_id}"),
                        fqn: Some(t.verb_fqn.clone()),
                        target_domain: Some(domain.to_owned()),
                        kind: "service_task".into(),
                    }
                }
                ExecutionNode::BusinessRuleTask(t) => {
                    let (domain, dec_id) = split_fqn(&t.decision_id);
                    NodeInfo {
                        id: (*id).to_owned(),
                        label: format!("↗ Evaluating {domain}: {dec_id}"),
                        fqn: Some(t.decision_id.clone()),
                        target_domain: Some(domain.to_owned()),
                        kind: "business_rule_task".into(),
                    }
                }
                ExecutionNode::ExclusiveGateway(_) => NodeInfo {
                    id: (*id).to_owned(),
                    label: "◇ CBU Type Gateway".into(),
                    fqn: None,
                    target_domain: None,
                    kind: "gateway".into(),
                },
                ExecutionNode::EndEvent(_) => NodeInfo {
                    id: (*id).to_owned(),
                    label: "✓ End: CBU Operational".into(),
                    fqn: None,
                    target_domain: None,
                    kind: "end".into(),
                },
            })
        })
        .collect()
}

fn split_fqn(fqn: &str) -> (&str, &str) {
    fqn.split_once(':').unwrap_or(("", fqn))
}

async fn apply_step(
    store: &MemoryStore,
    id: Uuid,
    next_node: String,
    placeholder: Option<(&str, serde_json::Value)>,
) {
    let Ok(Some(mut inst)) = store.load_instance(id).await else {
        return;
    };
    inst.state = ProcessState::Running;
    inst.current_node_id = Some(next_node);

    let mut pv: HashMap<String, serde_json::Value> = inst
        .placeholder_values
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    pv.remove("__retry_count");
    if let Some((name, val)) = placeholder {
        pv.insert(name.to_owned(), val);
    }
    inst.placeholder_values = serde_json::to_value(&pv).ok();
    let _ = store.save_instance(&inst).await;
}
