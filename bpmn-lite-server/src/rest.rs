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

#[derive(Serialize)]
pub(crate) struct VisualGraphDto {
    workflow_id: String,
    nodes: Vec<VisualNodeDto>,
    edges: Vec<VisualEdgeDto>,
}

#[derive(Serialize)]
pub(crate) struct VisualNodeDto {
    id: String,
    label: String,
    kind: String,
    plug: Option<String>,
    span: Option<bpmn_lite_types::SourceSpan>,
}

#[derive(Serialize)]
pub(crate) struct VisualEdgeDto {
    from: String,
    to: String,
    condition: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct CallStackFrameDto {
    instance_id: String,
    workflow_id: String,
    node_id: String,
    plug: Option<String>,
    span: Option<bpmn_lite_types::SourceSpan>,
    status: String,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct StartBody {
    cbu_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bpmn_dsl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    variables: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Deserialize, Serialize, Default)]
pub(crate) struct NextStepBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    outputs: Option<HashMap<String, serde_json::Value>>,
}

// ── Router ──────────────────────────────────────────────────────────────

pub(crate) fn demo_router(state: Arc<DemoState>) -> Router {
    Router::new()
        .route("/bpmn/health", get(health))
        .route("/bpmn/instances", get(list_instances).delete(reset_instances))
        .route("/bpmn/instances/start", post(start_instance))
        .route("/bpmn/instances/:id", get(get_instance))
        .route("/bpmn/instances/:id/next-step", post(next_step))
        .route("/bpmn/instances/:id/graph", get(get_instance_graph))
        .route("/bpmn/instances/:id/stack", get(get_instance_stack))
        .route("/bpmn/instances/:id/sage", get(list_sage_stub))
        .route("/bpmn/instances/:id/events", get(events_stub))
        .route("/bpmn/compile/preview", post(compile_bpmn_preview))
        .route("/dmn/compile/preview", post(compile_dmn_preview))
        .route("/dmn/decisions/:id", get(get_dmn_decision))
        .route("/api/dsl/macro/apply", post(apply_dsl_macro))
        .route("/api/dsl/diagnostics/resolve", post(resolve_dsl_diagnostics))
        .route("/api/dsl/sage/utter", post(sage_utterance_gate))
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
    let plan = if let Some(plan_hash) = inst.plan_hash {
        if let Ok(Some(plan_json)) = demo.store.load_plan(plan_hash).await {
            serde_json::from_str::<WorkflowExecutionPlan>(&plan_json).ok().map(Arc::new)
        } else {
            None
        }
    } else {
        None
    }.unwrap_or_else(|| demo.plan.clone());

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
        nodes: build_node_infos(&plan),
        sage_records: vec![],
    };
    Json(detail).into_response()
}

async fn start_instance(
    State(demo): State<Arc<DemoState>>,
    Json(body): Json<StartBody>,
) -> impl IntoResponse {
    let plan = if let Some(ref dsl) = body.bpmn_dsl {
        let registry = get_preview_registry();
        match bpmn_lite_compiler::dsl::compile(dsl, &registry) {
            Ok(p) => Arc::new(p),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": format!("Compilation failed: {}", e),
                        "diagnostics": vec![format!("{}", e)]
                    }))
                ).into_response();
            }
        }
    } else {
        demo.plan.clone()
    };

    let client_type_input = match body.cbu_type.as_str() {
        "fund" => "FUND_MANDATE",
        "corporate" => "CORPORATE",
        "trust" => "TRUST",
        other => other,
    };
    
    let mut vars = demo_initial_vars("Demo Client", client_type_input);
    if let Some(ref custom_vars) = body.variables {
        for (k, v) in custom_vars {
            vars.insert(k.clone(), v.clone());
        }
    }

    match create_instance(&demo.store, &plan, "demo", vars).await {
        Ok(id) => {
            demo.set_cbu_type(id, body.cbu_type.clone());
            // Walk past StartEvent to the first callout node.
            drive_forward(&demo.store, &plan, id).await;
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
    body: Option<Json<NextStepBody>>,
) -> impl IntoResponse {
    let inst = match demo.store.load_instance(id).await {
        Ok(Some(i)) => i,
        _ => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"})))
                .into_response()
        }
    };

    let plan = if let Some(plan_hash) = inst.plan_hash {
        if let Ok(Some(plan_json)) = demo.store.load_plan(plan_hash).await {
            serde_json::from_str::<WorkflowExecutionPlan>(&plan_json).ok().map(Arc::new)
        } else {
            None
        }
    } else {
        None
    }.unwrap_or_else(|| demo.plan.clone());

    let node_id = inst.current_node_id.clone().unwrap_or_default();
    let cbu_type = demo.cbu_type(id);
    let next_step_body = body.map(|Json(b)| b).unwrap_or_default();

    // Simulate result delivery for callout nodes, then drive forward through
    // any immediately following gateways/end events without touching the bus.
    if let Some(ExecutionNode::Task(t)) = plan.nodes.get(&node_id) {
        if let Some(ref placeholder_name) = t.produces_placeholder {
            let val = if let Some(ref outputs) = next_step_body.outputs {
                outputs.get(placeholder_name).cloned()
            } else {
                None
            };
            
            let placeholder = val.map(|v| (placeholder_name.as_str(), v)).or_else(|| {
                // Default fallback logic
                if t.plug.starts_with("dmn-lite:") {
                    let cbu_type_val = match cbu_type.as_str() {
                        "fund" => "fund",
                        "corporate" => "corporate",
                        "trust" => "trust",
                        _ => "fund",
                    };
                    Some((placeholder_name.as_str(), serde_json::Value::String(cbu_type_val.to_owned())))
                } else {
                    let default_val = if node_id == "create-cbu" {
                        serde_json::Value::String(Uuid::now_v7().to_string())
                    } else {
                        serde_json::Value::String(format!("{node_id}-done"))
                    };
                    Some((placeholder_name.as_str(), default_val))
                }
            });
            
            apply_step(&demo.store, id, t.next.clone(), placeholder).await;
        } else {
            apply_step(&demo.store, id, t.next.clone(), None).await;
        }
    }

    // Drive forward through gateways and end events without the bus.
    drive_forward(&demo.store, &plan, id).await;

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
    store.save_instance("default", &instance).await?;
    Ok(instance_id)
}

/// Walk forward through non-callout nodes (Start, Split,
/// End) without touching the bus. Stops at the first Task
/// so the user can click "Next Step" there.
async fn drive_forward(store: &MemoryStore, plan: &WorkflowExecutionPlan, id: Uuid) {
    use bpmn_lite_compiler::dsl::plan::SplitMode;
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
            Some(ExecutionNode::Start(n)) => {
                inst.current_node_id = Some(n.next.clone());
                let _ = store.save_instance("default", &inst).await;
            }
            Some(ExecutionNode::Split(gw)) => {
                match gw.mode {
                    SplitMode::Exclusive => {
                        let chosen = gw.flows.iter().find(|f| {
                            if let (Some(ph), Some(exp)) = (&f.placeholder, &f.expected_value) {
                                pv.get(ph).and_then(|v| v.as_str()) == Some(exp.as_str())
                            } else {
                                false
                            }
                        });
                        if let Some(flow) = chosen {
                            inst.current_node_id = Some(flow.next.clone());
                            let _ = store.save_instance("default", &inst).await;
                        } else {
                            break;
                        }
                    }
                    SplitMode::Parallel | SplitMode::Inclusive => {
                        let mut targets = Vec::new();
                        for flow in &gw.flows {
                            if gw.mode == SplitMode::Parallel {
                                targets.push(flow.next.clone());
                            } else {
                                let condition_matches = if let (Some(ph), Some(exp)) = (&flow.placeholder, &flow.expected_value) {
                                    pv.get(ph).and_then(|v| v.as_str()) == Some(exp.as_str())
                                } else {
                                    true
                                };
                                if condition_matches {
                                    targets.push(flow.next.clone());
                                }
                            }
                        }

                        if targets.is_empty() {
                            break;
                        }

                        let first = targets[0].clone();
                        let rest = &targets[1..];
                        
                        let mut updated_pv = pv.clone();
                        if !rest.is_empty() {
                            let rest_vals: Vec<serde_json::Value> = rest.iter().map(|s| serde_json::Value::String(s.clone())).collect();
                            updated_pv.insert("__pending_branches".to_string(), serde_json::Value::Array(rest_vals));
                        } else {
                            updated_pv.remove("__pending_branches");
                        }
                        
                        inst.placeholder_values = serde_json::to_value(&updated_pv).ok();
                        inst.current_node_id = Some(first);
                        let _ = store.save_instance("default", &inst).await;
                    }
                }
            }
            Some(ExecutionNode::Join(j)) => {
                let pending = pv.get("__pending_branches")
                    .and_then(|v| v.as_array());
                
                if let Some(branches) = pending {
                    if !branches.is_empty() {
                        let next_branch = branches[0].as_str().unwrap_or_default().to_string();
                        let rest = &branches[1..];
                        
                        let mut updated_pv = pv.clone();
                        if !rest.is_empty() {
                            updated_pv.insert("__pending_branches".to_string(), serde_json::Value::Array(rest.to_vec()));
                        } else {
                            updated_pv.remove("__pending_branches");
                        }
                        
                        inst.placeholder_values = serde_json::to_value(&updated_pv).ok();
                        inst.current_node_id = Some(next_branch);
                        let _ = store.save_instance("default", &inst).await;
                    } else {
                        let mut updated_pv = pv.clone();
                        updated_pv.remove("__pending_branches");
                        inst.placeholder_values = serde_json::to_value(&updated_pv).ok();
                        inst.current_node_id = Some(j.next.clone());
                        let _ = store.save_instance("default", &inst).await;
                    }
                } else {
                    inst.current_node_id = Some(j.next.clone());
                    let _ = store.save_instance("default", &inst).await;
                }
            }
            Some(ExecutionNode::Loop(l)) => {
                let counter_key = format!("__loop_count_{}", l.id);
                let current_count = pv.get(&counter_key)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;

                if current_count < l.ceiling {
                    let next_node = if let Some(first_body) = l.body.first() {
                        first_body.clone()
                    } else {
                        l.next.clone()
                    };
                    
                    let mut updated_pv = pv.clone();
                    updated_pv.insert(counter_key, serde_json::Value::Number((current_count + 1).into()));
                    inst.placeholder_values = serde_json::to_value(&updated_pv).ok();
                    inst.current_node_id = Some(next_node);
                    let _ = store.save_instance("default", &inst).await;
                } else {
                    let mut updated_pv = pv.clone();
                    updated_pv.remove(&counter_key);
                    inst.placeholder_values = serde_json::to_value(&updated_pv).ok();
                    inst.current_node_id = Some(l.next.clone());
                    let _ = store.save_instance("default", &inst).await;
                }
            }
            Some(ExecutionNode::End(end)) => {
                inst.state = ProcessState::Completed {
                    at: chrono::Utc::now().timestamp_millis(),
                };
                inst.current_node_id = Some(end.id.clone());
                let _ = store.save_instance("default", &inst).await;
                break;
            }
            _ => break,
        }
    }
}

fn build_node_infos(plan: &WorkflowExecutionPlan) -> Vec<NodeInfo> {
    use bpmn_lite_compiler::dsl::plan::SplitMode;
    use bpmn_lite_compiler::dsl::plan::JoinMode;
    let mut ordered = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(plan.start_node.clone());
    while let Some(curr) = queue.pop_front() {
        if !visited.insert(curr.clone()) {
            continue;
        }
        ordered.push(curr.clone());
        if let Some(node) = plan.nodes.get(&curr) {
            match node {
                ExecutionNode::Start(n) => { queue.push_back(n.next.clone()); }
                ExecutionNode::Task(t) => { queue.push_back(t.next.clone()); }
                ExecutionNode::Split(s) => {
                    for flow in &s.flows {
                        queue.push_back(flow.next.clone());
                    }
                }
                ExecutionNode::Join(j) => { queue.push_back(j.next.clone()); }
                ExecutionNode::Loop(l) => {
                    for body_node in &l.body {
                        queue.push_back(body_node.clone());
                    }
                    queue.push_back(l.next.clone());
                }
                ExecutionNode::End(_) => {}
            }
        }
    }
    // Just in case we missed any nodes, add them
    for id in plan.nodes.keys() {
        if !visited.contains(id) {
            ordered.push(id.clone());
        }
    }

    ordered
        .iter()
        .filter_map(|id| {
            let node = plan.nodes.get(id)?;
            Some(match node {
                ExecutionNode::Start(_) => NodeInfo {
                    id: id.clone(),
                    label: "Start".into(),
                    fqn: None,
                    target_domain: None,
                    kind: "start".into(),
                },
                ExecutionNode::Task(t) => {
                    if t.plug.starts_with("dmn-lite:") {
                        let (domain, dec_id) = split_fqn(&t.plug);
                        NodeInfo {
                            id: id.clone(),
                            label: format!("↗ Evaluating {domain}: {dec_id}"),
                            fqn: Some(t.plug.clone()),
                            target_domain: Some(domain.to_owned()),
                            kind: "business_rule_task".into(),
                        }
                    } else {
                        let (domain, verb_id) = split_fqn(&t.plug);
                        NodeInfo {
                            id: id.clone(),
                            label: format!("↗ Calling {domain}: {verb_id}"),
                            fqn: Some(t.plug.clone()),
                            target_domain: Some(domain.to_owned()),
                            kind: "service_task".into(),
                        }
                    }
                }
                ExecutionNode::Split(s) => {
                    let label = match s.mode {
                        SplitMode::Exclusive => "◇ Exclusive Gateway (XOR)".to_string(),
                        SplitMode::Inclusive => "◇ Inclusive Gateway (OR)".to_string(),
                        SplitMode::Parallel => "◇ Parallel Gateway (AND)".to_string(),
                    };
                    NodeInfo {
                        id: id.clone(),
                        label,
                        fqn: None,
                        target_domain: None,
                        kind: "gateway".into(),
                    }
                }
                ExecutionNode::Join(j) => {
                    let label = match j.mode {
                        JoinMode::Exclusive => "◇ Merge (XOR)".to_string(),
                        JoinMode::Inclusive => "◇ Join (OR)".to_string(),
                        JoinMode::Parallel => "◇ Join (AND)".to_string(),
                    };
                    NodeInfo {
                        id: id.clone(),
                        label,
                        fqn: None,
                        target_domain: None,
                        kind: "join".into(),
                    }
                }
                ExecutionNode::Loop(l) => NodeInfo {
                    id: id.clone(),
                    label: format!("Loop (Max {})", l.ceiling),
                    fqn: None,
                    target_domain: None,
                    kind: "loop".into(),
                },
                ExecutionNode::End(end) => NodeInfo {
                    id: id.clone(),
                    label: format!("✓ End: {}", end.status),
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
    let _ = store.save_instance("default", &inst).await;
}

async fn get_instance_graph(
    State(demo): State<Arc<DemoState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let inst = match demo.store.load_instance(id).await {
        Ok(Some(i)) => i,
        _ => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "instance not found"}))).into_response()
    };
    let plan_hash = match inst.plan_hash {
        Some(h) => h,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "plan not found"}))).into_response()
    };
    let plan_json = match demo.store.load_plan(plan_hash).await {
        Ok(Some(p)) => p,
        _ => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "plan not found"}))).into_response()
    };
    let plan: WorkflowExecutionPlan = match serde_json::from_str(&plan_json) {
        Ok(p) => p,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "invalid plan"}))).into_response()
    };

    let graph = plan_to_visual_graph(&plan);
    Json(graph).into_response()
}

async fn get_instance_stack(
    State(demo): State<Arc<DemoState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let mut current = match demo.store.load_instance(id).await {
        Ok(Some(i)) => i,
        _ => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "instance not found"}))).into_response()
    };

    let mut frames = Vec::new();
    let mut visited = std::collections::HashSet::new();
    visited.insert(current.instance_id);
    loop {
        let mut span = None;
        let mut plug = None;
        
        if let Some(plan_hash) = current.plan_hash {
            if let Ok(Some(plan_json)) = demo.store.load_plan(plan_hash).await {
                if let Ok(plan) = serde_json::from_str::<WorkflowExecutionPlan>(&plan_json) {
                    if let Some(node_id) = &current.current_node_id {
                        if let Some(node) = plan.nodes.get(node_id) {
                            match node {
                                ExecutionNode::Task(t) => {
                                    span = t.span;
                                    plug = Some(t.plug.clone());
                                }
                                ExecutionNode::Start(st) => span = st.span,
                                ExecutionNode::End(e) => span = e.span,
                                ExecutionNode::Split(sp) => span = sp.span,
                                ExecutionNode::Join(j) => span = j.span,
                                ExecutionNode::Loop(l) => span = l.span,
                            }
                        }
                    }
                }
            }
        }

        frames.push(CallStackFrameDto {
            instance_id: current.instance_id.to_string(),
            workflow_id: current.process_key.clone(),
            node_id: current.current_node_id.clone().unwrap_or_default(),
            plug,
            span,
            status: format_state(&current.state),
        });

        let correlation = current.correlation_id.clone();
        if correlation.is_empty() {
            break;
        }

        if let Some((parent_id_str, _)) = correlation.split_once(':') {
            if let Ok(parent_id) = uuid::Uuid::parse_str(parent_id_str) {
                if visited.insert(parent_id) {
                    if let Ok(Some(parent)) = demo.store.load_instance(parent_id).await {
                        current = parent;
                        continue;
                    }
                }
            }
        }
        break;
    }
    
    frames.reverse();
    Json(frames).into_response()
}

fn plan_to_visual_graph(plan: &WorkflowExecutionPlan) -> VisualGraphDto {
    use bpmn_lite_compiler::dsl::plan::SplitMode;
    use bpmn_lite_compiler::dsl::plan::JoinMode;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for (id, node) in &plan.nodes {
        let (kind, label, plug, span) = match node {
            ExecutionNode::Start(st) => ("start".to_owned(), "Start".to_owned(), None, st.span),
            ExecutionNode::End(e) => ("end".to_owned(), format!("End ({})", e.status), None, e.span),
            ExecutionNode::Task(t) => {
                let plug_name = t.plug.clone();
                let display_label = if plug_name.starts_with("dmn-lite:") {
                    format!("↗ Evaluate {}", plug_name.trim_start_matches("dmn-lite:"))
                } else {
                    format!("↗ Call {}", plug_name)
                };
                ("task".to_owned(), display_label, Some(plug_name), t.span)
            }
            ExecutionNode::Split(s) => {
                let mode_str = match s.mode {
                    SplitMode::Exclusive => "Exclusive Gateway (XOR)",
                    SplitMode::Inclusive => "Inclusive Gateway (OR)",
                    SplitMode::Parallel => "Parallel Gateway (AND)",
                };
                ("split".to_owned(), mode_str.to_owned(), None, s.span)
            }
            ExecutionNode::Join(j) => {
                let mode_str = match j.mode {
                    JoinMode::Exclusive => "Merge (XOR)",
                    JoinMode::Inclusive => "Join (OR)",
                    JoinMode::Parallel => "Join (AND)",
                };
                ("join".to_owned(), mode_str.to_owned(), None, j.span)
            }
            ExecutionNode::Loop(l) => {
                ("loop".to_owned(), format!("Loop (Max {})", l.ceiling), None, l.span)
            }
        };

        nodes.push(VisualNodeDto {
            id: id.clone(),
            label,
            kind,
            plug,
            span,
        });

        // Outgoing edge extraction
        match node {
            ExecutionNode::Start(n) => {
                edges.push(VisualEdgeDto { from: id.clone(), to: n.next.clone(), condition: None });
            }
            ExecutionNode::Task(t) => {
                edges.push(VisualEdgeDto { from: id.clone(), to: t.next.clone(), condition: None });
            }
            ExecutionNode::Split(s) => {
                for flow in &s.flows {
                    let cond_str = if let (Some(ph), Some(val)) = (&flow.placeholder, &flow.expected_value) {
                        Some(format!("{} == {:?}", ph, val))
                    } else {
                        None
                    };
                    edges.push(VisualEdgeDto { from: id.clone(), to: flow.next.clone(), condition: cond_str });
                }
            }
            ExecutionNode::Join(j) => {
                edges.push(VisualEdgeDto { from: id.clone(), to: j.next.clone(), condition: None });
            }
            ExecutionNode::Loop(l) => {
                if let Some(first_body) = l.body.first() {
                    edges.push(VisualEdgeDto { from: id.clone(), to: first_body.clone(), condition: Some("Loop Body".into()) });
                }
                edges.push(VisualEdgeDto { from: id.clone(), to: l.next.clone(), condition: Some("Loop Exit".into()) });
            }
            ExecutionNode::End(_) => {}
        }
    }

    VisualGraphDto {
        workflow_id: plan.workflow_id.clone(),
        nodes,
        edges,
    }
}

// ── Preview Compilation and DMN handlers ──────────────────────────────────

const OB_POC_MANIFEST_YAML: &str = include_str!(
    concat!(env!("CARGO_MANIFEST_DIR"), "/../manifests/ob-poc-v1.0.0.yaml")
);
const DMN_LITE_MANIFEST_YAML: &str = include_str!(
    concat!(env!("CARGO_MANIFEST_DIR"), "/../manifests/dmn-lite-v1.0.0.yaml")
);
const BPMN_MANIFEST_YAML: &str = include_str!(
    concat!(env!("CARGO_MANIFEST_DIR"), "/../manifests/bpmn-v1.0.0.yaml")
);

fn get_preview_registry() -> bpmn_lite_compiler::dsl::ManifestPlaceholderRegistry<bpmn_lite_compiler::dsl::StubPlaceholderRegistry> {
    use dsl_manifest::Manifest;
    use bpmn_lite_compiler::dsl::{ManifestPlaceholderRegistry, StubPlaceholderRegistry};

    let ob_poc = Manifest::load_from_yaml(OB_POC_MANIFEST_YAML)
        .expect("ob-poc manifest must load");
    let dmn_lite = Manifest::load_from_yaml(DMN_LITE_MANIFEST_YAML)
        .expect("dmn-lite manifest must load");
    let bpmn = Manifest::load_from_yaml(BPMN_MANIFEST_YAML)
        .expect("bpmn manifest must load");

    let mut registry = ManifestPlaceholderRegistry::new(StubPlaceholderRegistry::new().with_demo_bindings());
    registry.import(ob_poc);
    registry.import(dmn_lite);
    registry.import(bpmn);
    registry
}

#[derive(Serialize)]
pub(crate) struct CompilePreviewResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nodes: Option<Vec<VisualNodeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edges: Option<Vec<VisualEdgeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    diagnostics: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct CompilePreviewBody {
    bpmn_dsl: String,
}

#[derive(Deserialize)]
pub(crate) struct DmnPreviewRequest {
    dmn_dsl: String,
}

#[derive(Serialize)]
pub(crate) struct DmnInputSchema {
    name: String,
    #[serde(rename = "type")]
    type_ref: String,
    domain: String,
}

#[derive(Serialize)]
pub(crate) struct DmnOutputSchema {
    name: String,
    #[serde(rename = "type")]
    type_ref: String,
    domain: String,
}

#[derive(Serialize)]
pub(crate) struct DmnRuleInputCell {
    op: String,
    value: String,
}

#[derive(Serialize)]
pub(crate) struct DmnRuleDto {
    id: String,
    inputs: Vec<DmnRuleInputCell>,
    outputs: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct DmnSchemaDto {
    decision_name: String,
    hit_policy: String,
    inputs: Vec<DmnInputSchema>,
    outputs: Vec<DmnOutputSchema>,
    rules: Vec<DmnRuleDto>,
}

#[derive(Serialize)]
pub(crate) struct DmnPreviewResponse {
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    schema: Option<DmnSchemaDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    diagnostics: Vec<String>,
}

async fn compile_bpmn_preview(
    Json(body): Json<CompilePreviewBody>,
) -> impl IntoResponse {
    use bpmn_lite_compiler::dsl::compile;
    let registry = get_preview_registry();
    match compile(&body.bpmn_dsl, &registry) {
        Ok(plan) => {
            let visual = plan_to_visual_graph(&plan);
            let resp = CompilePreviewResponse {
                workflow_id: Some(visual.workflow_id),
                nodes: Some(visual.nodes),
                edges: Some(visual.edges),
                error: None,
                diagnostics: Vec::new(),
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(err) => {
            let (error_msg, diagnostics) = match err {
                bpmn_lite_compiler::dsl::CompileError::Parse(errs) => {
                    ("Parsing failed".to_owned(), errs)
                }
                bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                    let mut formatted = Vec::new();
                    for e in errs {
                        let msg = format!("{}", e);
                        formatted.push(msg);
                        let symbol = if e.message.starts_with("unresolved symbol '") && e.message.ends_with("'") {
                            Some(e.message.trim_start_matches("unresolved symbol '").trim_end_matches("'"))
                        } else if e.message.starts_with("verb '") {
                            let remaining = e.message.trim_start_matches("verb '");
                            remaining.find('\'').map(|idx| &remaining[..idx])
                        } else if e.message.starts_with("decision '") {
                            let remaining = e.message.trim_start_matches("decision '");
                            remaining.find('\'').map(|idx| &remaining[..idx])
                        } else {
                            None
                        };

                        if let Some(sym) = symbol {
                            formatted.push(format!(
                                "Suggestion: Would you like me to import {} to fix the unresolved verb error?",
                                sym
                            ));
                        }
                    }
                    ("Linting failed".to_owned(), formatted)
                }
                bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                    let mut formatted = Vec::new();
                    for e in errs {
                        let msg = format!("{}", e);
                        formatted.push(msg);
                        if e.message.starts_with("cycle detected:") {
                            formatted.push("Suggestion: Try structuring the cyclic path within a bounded 'loop' block.".to_owned());
                        } else if e.message.ends_with("is unreachable from start") {
                            formatted.push("Suggestion: Connect this node from a preceding gateway or task by updating the ':next' attribute.".to_owned());
                        }
                    }
                    ("DAG validation failed".to_owned(), formatted)
                }
            };
            let resp = CompilePreviewResponse {
                workflow_id: None,
                nodes: None,
                edges: None,
                error: Some(error_msg),
                diagnostics,
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
    }
}

async fn compile_dmn_preview(
    Json(body): Json<DmnPreviewRequest>,
) -> impl IntoResponse {
    match parse_dmn_to_dto(&body.dmn_dsl) {
        Ok(schema) => {
            let resp = DmnPreviewResponse {
                schema: Some(schema),
                error: None,
                diagnostics: Vec::new(),
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(err_msg) => {
            let resp = DmnPreviewResponse {
                schema: None,
                error: Some("DMN compilation failed".to_owned()),
                diagnostics: vec![err_msg],
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
    }
}

async fn get_dmn_decision(
    Path(decision_id): Path<String>,
) -> impl IntoResponse {
    // Sanitize path parameter to prevent path traversal
    if !decision_id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid decision ID format" }))).into_response();
    }

    use std::path::PathBuf;
    let decisions_dir = std::env::var("DMN_DECISIONS_DIR")
        .unwrap_or_else(|_| format!("{}/../dmn-lite-decisions", env!("CARGO_MANIFEST_DIR")));
    let path = PathBuf::from(decisions_dir).join(format!("{}.dmn-lite", decision_id));

    match std::fs::read_to_string(&path) {
        Ok(source_text) => {
            match parse_dmn_to_dto(&source_text) {
                Ok(schema) => (StatusCode::OK, Json(schema)).into_response(),
                Err(err_msg) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": err_msg }))).into_response()
                }
            }
        }
        Err(err) => {
            (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("Decision not found: {}", err) }))).into_response()
        }
    }
}

fn parse_dmn_to_dto(source_text: &str) -> Result<DmnSchemaDto, String> {
    use dmn_lite_parser::{parse, HitPolicyAst, TypeRefAst, WhenAst};

    let source = parse(source_text).map_err(|e| format!("{}", e))?;
    let decision = source.decisions.first().ok_or_else(|| "No decision defined in DSL".to_owned())?;

    let decision_name = decision.name.name.clone();
    let hit_policy = match &decision.hit_policy {
        HitPolicyAst::Unique(_) => "unique".to_owned(),
        HitPolicyAst::First(_) => "first".to_owned(),
    };

    let inputs: Vec<DmnInputSchema> = decision.inputs.iter().map(|input| {
        let type_ref = match &input.type_ref {
            TypeRefAst::Enum(_) => "enum",
            TypeRefAst::Bool(_) => "bool",
            TypeRefAst::Integer(_) => "integer",
            TypeRefAst::Decimal(_) => "decimal",
            TypeRefAst::String(_) => "string",
        }.to_owned();
        DmnInputSchema {
            name: input.name.name.clone(),
            type_ref,
            domain: input.domain_ref.name.clone(),
        }
    }).collect();

    let outputs: Vec<DmnOutputSchema> = decision.outputs.iter().map(|output| {
        let type_ref = match &output.type_ref {
            TypeRefAst::Enum(_) => "enum",
            TypeRefAst::Bool(_) => "bool",
            TypeRefAst::Integer(_) => "integer",
            TypeRefAst::Decimal(_) => "decimal",
            TypeRefAst::String(_) => "string",
        }.to_owned();
        DmnOutputSchema {
            name: output.name.name.clone(),
            type_ref,
            domain: output.domain_ref.name.clone(),
        }
    }).collect();

    let rules: Vec<DmnRuleDto> = decision.rules.iter().map(|rule| {
        // Build inputs array in same order as inputs list
        let mut rule_inputs = Vec::new();
        for input_schema in &inputs {
            let mut matched_cell = DmnRuleInputCell {
                op: "-".to_owned(),
                value: "-".to_owned(),
            };

            match &rule.when {
                WhenAst::CatchAll(_) => {}
                WhenAst::Predicates(preds, _) => {
                    if let Some(pred) = find_predicate_for_field(preds, &input_schema.name) {
                        let (op, val) = format_predicate_cell(pred);
                        matched_cell = DmnRuleInputCell { op, value: val };
                    }
                }
            }
            rule_inputs.push(matched_cell);
        }

        // Build outputs array in same order as outputs list
        let mut rule_outputs = Vec::new();
        for output_schema in &outputs {
            let mut matched_val = "-".to_owned();
            if let Some(assign) = rule.then.iter().find(|a| a.output.name == output_schema.name) {
                matched_val = format_literal(&assign.value);
            }
            rule_outputs.push(matched_val);
        }

        DmnRuleDto {
            id: rule.id.name.clone(),
            inputs: rule_inputs,
            outputs: rule_outputs,
        }
    }).collect();

    Ok(DmnSchemaDto {
        decision_name,
        hit_policy,
        inputs,
        outputs,
        rules,
    })
}

fn find_predicate_for_field<'a>(
    preds: &'a [dmn_lite_parser::PredicateAst],
    field_name: &str,
) -> Option<&'a dmn_lite_parser::PredicateAst> {
    for pred in preds {
        if let Some(f) = get_predicate_field(pred) {
            if f == field_name {
                return Some(pred);
            }
        }
    }
    None
}

fn get_predicate_field(pred: &dmn_lite_parser::PredicateAst) -> Option<&str> {
    use dmn_lite_parser::PredicateAst;
    match pred {
        PredicateAst::Eq { field, .. } => Some(&field.name),
        PredicateAst::NotEq { field, .. } => Some(&field.name),
        PredicateAst::Lt { field, .. } => Some(&field.name),
        PredicateAst::Le { field, .. } => Some(&field.name),
        PredicateAst::Gt { field, .. } => Some(&field.name),
        PredicateAst::Ge { field, .. } => Some(&field.name),
        PredicateAst::InSet { field, .. } => Some(&field.name),
        PredicateAst::Range { field, .. } => Some(&field.name),
        PredicateAst::IsNull { field, .. } => Some(&field.name),
        PredicateAst::IsNotNull { field, .. } => Some(&field.name),
        PredicateAst::Not { inner, .. } => get_predicate_field(inner),
        PredicateAst::And { items, .. } | PredicateAst::Or { items, .. } => {
            let mut field = None;
            for item in items {
                if let Some(f) = get_predicate_field(item) {
                    if let Some(prev) = field {
                        if prev != f {
                            return None;
                        }
                    } else {
                        field = Some(f);
                    }
                } else {
                    return None;
                }
            }
            field
        }
    }
}

fn format_predicate_cell(pred: &dmn_lite_parser::PredicateAst) -> (String, String) {
    use dmn_lite_parser::PredicateAst;
    match pred {
        PredicateAst::Eq { value, .. } => ("==".to_string(), format_literal(value)),
        PredicateAst::NotEq { value, .. } => ("!=".to_string(), format_literal(value)),
        PredicateAst::Lt { value, .. } => ("<".to_string(), format_number_literal(value)),
        PredicateAst::Le { value, .. } => ("<=".to_string(), format_number_literal(value)),
        PredicateAst::Gt { value, .. } => (">".to_string(), format_number_literal(value)),
        PredicateAst::Ge { value, .. } => (">=".to_string(), format_number_literal(value)),
        PredicateAst::IsNull { .. } => ("is-null".to_string(), "".to_string()),
        PredicateAst::IsNotNull { .. } => ("is-not-null".to_string(), "".to_string()),
        PredicateAst::InSet { values, .. } => {
            let formatted_vals: Vec<String> = values.iter().map(format_literal).collect();
            ("in".to_string(), format!("[{}]", formatted_vals.join(", ")))
        }
        PredicateAst::Range { lower, upper, lower_inclusive, upper_inclusive, .. } => {
            let left_bracket = if *lower_inclusive { "[" } else { "(" };
            let right_bracket = if *upper_inclusive { "]" } else { ")" };
            let lower_str = format_range_bound(lower);
            let upper_str = format_range_bound(upper);
            ("in".to_string(), format!("{}{} .. {}{}", left_bracket, lower_str, upper_str, right_bracket))
        }
        PredicateAst::Not { inner, .. } => {
            let (op, val) = format_predicate_cell(inner);
            (format!("not {}", op), val)
        }
        PredicateAst::And { items, .. } => {
            let formatted_vals: Vec<String> = items.iter().map(|item| format_predicate_cell(item).1).collect();
            ("and".to_string(), formatted_vals.join(" and "))
        }
        PredicateAst::Or { items, .. } => {
            let formatted_vals: Vec<String> = items.iter().map(|item| format_predicate_cell(item).1).collect();
            ("or".to_string(), formatted_vals.join(" or "))
        }
    }
}

fn format_literal(lit: &dmn_lite_parser::LiteralAst) -> String {
    use dmn_lite_parser::LiteralAst;
    match lit {
        LiteralAst::Symbol(s) => s.name.clone(),
        LiteralAst::String(s) => s.value.clone(),
        LiteralAst::Number(n) => n.text.clone(),
        LiteralAst::Boolean { value, .. } => value.to_string(),
    }
}

fn format_number_literal(n: &dmn_lite_parser::NumberLitAst) -> String {
    n.text.clone()
}

fn format_range_bound(bound: &dmn_lite_parser::RangeBound) -> String {
    use dmn_lite_parser::RangeBound;
    match bound {
        RangeBound::Unbounded(_) => "*".to_string(),
        RangeBound::Value(n) => n.text.clone(),
    }
}

// ── DSL Macro & Diagnostics Handlers ─────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub(crate) struct MacroApplyRequest {
    source_code: String,
    macro_type: String,
    parameters: HashMap<String, String>,
}

#[derive(Serialize)]
pub(crate) struct MacroApplyResponse {
    source_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nodes: Option<Vec<VisualNodeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edges: Option<Vec<VisualEdgeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    diagnostics: Vec<String>,
}

fn get_macros_config() -> Option<bpmn_lite_compiler::dsl::macros::MacroConfigList> {
    let workspace_macros = format!("{}/../macros.yaml", env!("CARGO_MANIFEST_DIR"));
    if std::path::Path::new(&workspace_macros).exists() {
        if let Ok(config) = bpmn_lite_compiler::dsl::macros::MacroConfigList::load_from_file(&workspace_macros) {
            return Some(config);
        }
    }
    let server_macros = format!("{}/macros.yaml", env!("CARGO_MANIFEST_DIR"));
    if std::path::Path::new(&server_macros).exists() {
        if let Ok(config) = bpmn_lite_compiler::dsl::macros::MacroConfigList::load_from_file(&server_macros) {
            return Some(config);
        }
    }
    None
}

async fn apply_dsl_macro(
    Json(body): Json<MacroApplyRequest>,
) -> impl IntoResponse {
    use bpmn_lite_compiler::dsl::{
        parse_workflow_str,
        refactor::{AstMutator, ToSexpr},
        macros::{create_bounded_retry_macro, create_xor_split_join, create_parallel_split_join, XorBranchConfig},
        ast::{NodeAst},
    };

    let mut workflow = match parse_workflow_str(&body.source_code) {
        Ok(w) => w,
        Err(e) => {
            let resp = MacroApplyResponse {
                source_code: body.source_code.clone(),
                workflow_id: None,
                nodes: None,
                edges: None,
                error: Some(format!("Failed to parse source: {}", e)),
                diagnostics: vec![e],
            };
            return (StatusCode::BAD_REQUEST, Json(resp)).into_response();
        }
    };

    let result = match body.macro_type.as_str() {
        "BoundedRetry" => {
            let target_node_id = match body.parameters.get("target_node_id") {
                Some(id) => id,
                None => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Missing parameter target_node_id"}))).into_response(),
            };
            let ceiling: u32 = body.parameters.get("ceiling")
                .and_then(|c| c.parse().ok())
                .unwrap_or(3);
            let loop_id = body.parameters.get("custom_id")
                .cloned()
                .unwrap_or_else(|| format!("{}-retry-loop", target_node_id));

            // Extract target task to wrap
            let target_task = {
                let mut mutator = AstMutator::new(&mut workflow);
                match mutator.remove_node(target_node_id) {
                    Some(NodeAst::Task(t)) => t,
                    _ => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": format!("Task '{}' not found or is not a task node", target_node_id)}))).into_response(),
                }
            };

            let exit_next = target_task.next.clone();

            let pred_ids = find_all_predecessors_id_in_workflow(&workflow, target_node_id);
            {
                let mut mutator = AstMutator::new(&mut workflow);
                for pred in &pred_ids {
                    let _ = mutator.rewire_next(pred, &exit_next);
                }
            }

            let loop_node = NodeAst::Loop(create_bounded_retry_macro(
                target_task,
                ceiling,
                &loop_id,
                &exit_next,
            ));

            let pred_ids_exit = find_all_predecessors_id_in_workflow(&workflow, &exit_next);
            let first_id = if workflow.nodes.is_empty() {
                None
            } else {
                Some(workflow.nodes[0].id().to_string())
            };

            let mut mutator = AstMutator::new(&mut workflow);
            if !pred_ids_exit.is_empty() {
                mutator.insert_after(&pred_ids_exit[0], loop_node)
            } else if let Some(first) = first_id {
                mutator.insert_after(&first, loop_node)
            } else {
                Err("Empty workflow scope".to_string())
            }
        }
        "XorSplit" => {
            let split_id = body.parameters.get("split_id").map(|s| s.as_str()).unwrap_or("xor-split");
            let placeholder = body.parameters.get("placeholder").map(|s| s.as_str()).unwrap_or("@decision_val");
            let join_id = body.parameters.get("join_id").map(|s| s.as_str()).unwrap_or("xor-join");
            let join_next = body.parameters.get("join_next").map(|s| s.as_str()).unwrap_or("end");
            let predecessor_id = body.parameters.get("predecessor_id").map(|s| s.as_str()).unwrap_or("start");

            let branch_val = body.parameters.get("branch_value").map(|s| s.as_str()).unwrap_or("yes");
            let branch_target = body.parameters.get("branch_target").map(|s| s.as_str()).unwrap_or("end");

            let branches = vec![
                XorBranchConfig { condition_value: branch_val.to_string(), target_next: branch_target.to_string() },
                XorBranchConfig { condition_value: "default".to_string(), target_next: join_id.to_string() }
            ];

            let (split, join) = create_xor_split_join(split_id, placeholder, branches, join_id, join_next);
            
            let mut mutator = AstMutator::new(&mut workflow);
            mutator.insert_after(predecessor_id, NodeAst::Split(split)).and_then(|_| {
                let mut mutator = AstMutator::new(&mut workflow);
                mutator.insert_after(split_id, NodeAst::Join(join))
            })
        }
        "ParallelSplit" => {
            let split_id = body.parameters.get("split_id").map(|s| s.as_str()).unwrap_or("and-split");
            let join_id = body.parameters.get("join_id").map(|s| s.as_str()).unwrap_or("and-join");
            let join_next = body.parameters.get("join_next").map(|s| s.as_str()).unwrap_or("end");
            let predecessor_id = body.parameters.get("predecessor_id").map(|s| s.as_str()).unwrap_or("start");

            let branch_target = body.parameters.get("branch_target").map(|s| s.as_str()).unwrap_or("end");

            let branch_entries = vec![branch_target.to_string(), join_id.to_string()];
            let (split, join) = create_parallel_split_join(split_id, branch_entries, join_id, join_next);

            let mut mutator = AstMutator::new(&mut workflow);
            mutator.insert_after(predecessor_id, NodeAst::Split(split)).and_then(|_| {
                let mut mutator = AstMutator::new(&mut workflow);
                mutator.insert_after(split_id, NodeAst::Join(join))
            })
        }
        "Custom" => {
            let macro_id = match body.parameters.get("macro_id") {
                Some(id) => id,
                None => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Missing parameter macro_id for Custom macro"}))).into_response(),
            };
            let predecessor_id = body.parameters.get("predecessor_id").map(|s| s.as_str()).unwrap_or("start");

            let config = get_macros_config().ok_or_else(|| "No macros.yaml config found on disk".to_string());
            let node = config.and_then(|c| {
                let macro_cfg = c.macros.into_iter().find(|m| &m.id == macro_id)
                    .ok_or_else(|| format!("Custom macro '{}' not found in macros.yaml", macro_id))?;
                macro_cfg.instantiate(&body.parameters)
            });

            match node {
                Ok(n) => {
                    let mut mutator = AstMutator::new(&mut workflow);
                    mutator.insert_after(predecessor_id, n)
                }
                Err(e) => Err(e),
            }
        }
        other => Err(format!("Unknown macro type: {}", other)),
    };

    if let Err(e) = result {
        let resp = MacroApplyResponse {
            source_code: body.source_code.clone(),
            workflow_id: None,
            nodes: None,
            edges: None,
            error: Some(format!("Macro application failed: {}", e)),
            diagnostics: vec![e],
        };
        return (StatusCode::OK, Json(resp)).into_response();
    }

    let new_dsl = workflow.to_sexpr(0);
    let registry = get_preview_registry();
    
    match bpmn_lite_compiler::dsl::compile(&new_dsl, &registry) {
        Ok(plan) => {
            let visual = plan_to_visual_graph(&plan);
            let resp = MacroApplyResponse {
                source_code: new_dsl,
                workflow_id: Some(visual.workflow_id),
                nodes: Some(visual.nodes),
                edges: Some(visual.edges),
                error: None,
                diagnostics: Vec::new(),
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(err) => {
            let diagnostics = match err {
                bpmn_lite_compiler::dsl::CompileError::Parse(errs) => errs,
                bpmn_lite_compiler::dsl::CompileError::Lint(errs) => errs.iter().map(|e| format!("{}", e)).collect(),
                bpmn_lite_compiler::dsl::CompileError::Dag(errs) => errs.iter().map(|e| format!("{}", e)).collect(),
            };
            let resp = MacroApplyResponse {
                source_code: new_dsl,
                workflow_id: None,
                nodes: None,
                edges: None,
                error: Some("Compilation failed after macro application".to_string()),
                diagnostics,
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
    }
}

fn find_all_predecessors_id_in_workflow(workflow: &bpmn_lite_compiler::dsl::ast::WorkflowSource, target_id: &str) -> Vec<String> {
    let mut preds = Vec::new();
    find_all_predecessors_id_rec(&workflow.nodes, target_id, &mut preds);
    preds
}

fn find_all_predecessors_id_rec(nodes: &[bpmn_lite_compiler::dsl::ast::NodeAst], target_id: &str, acc: &mut Vec<String>) {
    for node in nodes {
        match node {
            bpmn_lite_compiler::dsl::ast::NodeAst::Start(s) => {
                if s.next == target_id { acc.push(s.id.clone()); }
            }
            bpmn_lite_compiler::dsl::ast::NodeAst::Task(t) => {
                if t.next == target_id { acc.push(t.id.clone()); }
            }
            bpmn_lite_compiler::dsl::ast::NodeAst::Join(j) => {
                if j.next == target_id { acc.push(j.id.clone()); }
            }
            bpmn_lite_compiler::dsl::ast::NodeAst::Loop(l) => {
                if l.next == target_id { acc.push(l.id.clone()); }
                find_all_predecessors_id_rec(&l.body, target_id, acc);
            }
            bpmn_lite_compiler::dsl::ast::NodeAst::Split(s) => {
                for flow in &s.flows {
                    if flow.next == target_id { acc.push(s.id.clone()); }
                }
            }
            bpmn_lite_compiler::dsl::ast::NodeAst::End(_) => {}
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct DiagnosticsResolveRequest {
    source_code: String,
    action: bpmn_lite_authoring::diagnostics_executor::FixAction,
}

#[derive(Serialize)]
pub(crate) struct DiagnosticsResolveResponse {
    source_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nodes: Option<Vec<VisualNodeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edges: Option<Vec<VisualEdgeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    diagnostics: Vec<String>,
}

async fn resolve_dsl_diagnostics(
    Json(body): Json<DiagnosticsResolveRequest>,
) -> impl IntoResponse {
    let manifests_path = std::env::var("SAGE_MANIFESTS_DIR")
        .unwrap_or_else(|_| format!("{}/../manifests", env!("CARGO_MANIFEST_DIR")));
    let manifests_dir = std::path::PathBuf::from(manifests_path);
    match bpmn_lite_authoring::diagnostics_executor::execute_autofix(&body.source_code, &body.action, &manifests_dir) {
        Ok(new_dsl) => {
            let registry = get_preview_registry();
            let new_dsl_str = new_dsl.clone();
            match bpmn_lite_compiler::dsl::compile(&new_dsl_str, &registry) {
                Ok(plan) => {
                    let visual = plan_to_visual_graph(&plan);
                    let resp = DiagnosticsResolveResponse {
                        source_code: new_dsl_str,
                        workflow_id: Some(visual.workflow_id),
                        nodes: Some(visual.nodes),
                        edges: Some(visual.edges),
                        error: None,
                        diagnostics: Vec::new(),
                    };
                    (StatusCode::OK, Json(resp)).into_response()
                }
                Err(err) => {
                    let diagnostics = match err {
                        bpmn_lite_compiler::dsl::CompileError::Parse(errs) => errs,
                        bpmn_lite_compiler::dsl::CompileError::Lint(errs) => errs.iter().map(|e| format!("{}", e)).collect(),
                        bpmn_lite_compiler::dsl::CompileError::Dag(errs) => errs.iter().map(|e| format!("{}", e)).collect(),
                    };
                    let resp = DiagnosticsResolveResponse {
                        source_code: new_dsl_str,
                        workflow_id: None,
                        nodes: None,
                        edges: None,
                        error: Some("Compilation failed after diagnostic resolution".to_string()),
                        diagnostics,
                    };
                    (StatusCode::OK, Json(resp)).into_response()
                }
            }
        }
        Err(e) => {
            let resp = DiagnosticsResolveResponse {
                source_code: body.source_code.clone(),
                workflow_id: None,
                nodes: None,
                edges: None,
                error: Some(format!("Diagnostic resolution failed: {}", e)),
                diagnostics: vec![e],
            };
            (StatusCode::BAD_REQUEST, Json(resp)).into_response()
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct UtteranceRequest {
    utterance: String,
    _current_dsl: String,
}

#[derive(Serialize)]
pub(crate) struct UtteranceResponse {
    escape_intent_detected: bool,
    suggested_action: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_payload: Option<serde_json::Value>,
}

async fn sage_utterance_gate(
    Json(body): Json<UtteranceRequest>,
) -> impl IntoResponse {
    let text = body.utterance.trim().to_lowercase();
    
    let is_escape = text.contains("exit") || text.contains("close editor") || text.contains("go back") || text.contains("quit") 
                    || text.contains("deploy") || text.contains("push release") || text.contains("staging")
                    || text.contains("check database") || text.contains("list active manifests");
                    
    let (suggested_action, msg, payload) = if text.contains("exit") || text.contains("close editor") || text.contains("go back") || text.contains("quit") {
        ("exit", "Autosaved current workspace. Exiting designer session.", None)
    } else if text.contains("deploy") || text.contains("push release") || text.contains("staging") {
        ("chat", "It looks like you want to perform a deployment or navigate away. Should we save your workflow draft and transition out of the designer session?", None)
    } else if text.contains("retry loop") || text.contains("wrap") || text.contains("retry") {
        let mut params = serde_json::Map::new();
        params.insert("target_node_id".to_string(), serde_json::Value::String("create-cbu".to_string()));
        params.insert("ceiling".to_string(), serde_json::Value::String("3".to_string()));
        ("apply_macro", "We can wrap your task in a retry loop. Apply macro?", Some(serde_json::json!({
            "macro_type": "BoundedRetry",
            "parameters": params
        })))
    } else if text.starts_with("import ") || text.contains("unknown verb") {
        let verb = if text.starts_with("import ") {
            body.utterance.trim()[7..].to_string()
        } else {
            "ob-poc:cbu.create".to_string()
        };
        let domain = if verb.contains(':') {
            verb.split(':').collect::<Vec<&str>>()[0].to_string()
        } else {
            "ob-poc".to_string()
        };
        ("resolve_diagnostic", "Resolving unresolved symbol by adding signature stub to manifest.", Some(serde_json::json!({
            "type": "AddVerbStub",
            "domain": domain,
            "verb": verb
        })))
    } else {
        ("none", "Utterance processed. Continues editing mode.", None)
    };

    Json(UtteranceResponse {
        escape_intent_detected: is_escape,
        suggested_action: suggested_action.to_string(),
        message: msg.to_string(),
        action_payload: payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt; // for `oneshot`
    use serde_json::Value;

    #[tokio::test]
    async fn test_rest_graph_and_stack_endpoints() {
        let state = DemoState::new();
        let app = demo_router(state.clone());

        // 1. Start a demo instance
        let start_body = StartBody { cbu_type: "fund".to_owned(), bpmn_dsl: None, variables: None };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/instances/start")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_string(&start_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let start_res: Value = serde_json::from_slice(&body_bytes).unwrap();
        let instance_id_str = start_res["instance_id"].as_str().unwrap();
        let instance_id = Uuid::parse_str(instance_id_str).unwrap();

        // 2. Query /graph endpoint
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/bpmn/instances/{}/graph", instance_id))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 100000).await.unwrap();
        let graph: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(graph["workflow_id"], "custody-cbu-onboarding");
        let nodes = graph["nodes"].as_array().unwrap();
        assert!(!nodes.is_empty());
        let edges = graph["edges"].as_array().unwrap();
        assert!(!edges.is_empty());

        // Verify start node exists and has a span (since it was compiled from DSL!)
        let start_node = nodes.iter().find(|n| n["id"] == "start").unwrap();
        assert_eq!(start_node["kind"], "start");
        assert!(start_node["span"].is_object(), "start node should carry parsed source span metadata");

        // 3. Query /stack endpoint
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/bpmn/instances/{}/stack", instance_id))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let stack: Value = serde_json::from_slice(&body_bytes).unwrap();
        let frames = stack.as_array().unwrap();
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert_eq!(frame["instance_id"], instance_id_str);
        assert_eq!(frame["workflow_id"], "custody-cbu-onboarding");
        assert_eq!(frame["node_id"], "create-cbu"); // Drive forward stops at the first Task node
        assert!(frame["span"].is_object(), "active stack frame should carry parsed source span metadata");
    }

    #[tokio::test]
    async fn test_compile_and_preview_endpoints() {
        let state = DemoState::new();
        let app = demo_router(state.clone());

        // Test BPMN compilation preview
        let bpmn_src = r#"(workflow test-wf
  (start-event :id start :next end)
  (end-event :id end :status "OK"))"#;
        let bpmn_body = serde_json::json!({ "bpmn_dsl": bpmn_src });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/compile/preview")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_string(&bpmn_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["workflow_id"], "test-wf");
        assert!(!res["nodes"].as_array().unwrap().is_empty());

        // Test BPMN compilation preview with unresolved verb error suggestion
        let bpmn_err_src = r#"(workflow err-wf
  (start-event :id start :next my-task)
  (service-task :id my-task :verb unknown-domain:unknown-verb :next end)
  (end-event :id end :status "OK"))"#;
        let bpmn_err_body = serde_json::json!({ "bpmn_dsl": bpmn_err_src });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/compile/preview")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_string(&bpmn_err_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["error"], "Linting failed");
        let diagnostics = res["diagnostics"].as_array().unwrap();
        assert!(diagnostics.iter().any(|d| d.as_str().unwrap().contains("Suggestion: Would you like me to import unknown-domain:unknown-verb to fix the unresolved verb error?")));

        // Test BPMN compilation preview with valid bpmn:message-wait and ob-poc verbs
        let bpmn_wait_src = r#"(workflow custody-cbu-onboarding
          (start-event :id start :next create-cbu)
          (service-task :id create-cbu :verb ob-poc:cbu.create :next message-wait)
          (service-task :id message-wait :verb bpmn:message-wait :next type-decision)
          (business-rule-task :id type-decision :decision dmn-lite:cbu_type_routing :next type-gateway)
          (exclusive-gateway :id type-gateway
            (flow :condition (= @cbu-type "fund")      :next add-fund)
            (flow :condition (= @cbu-type "corporate") :next add-corp)
            (flow :condition (= @cbu-type "trust")     :next add-trust))
          (service-task :id add-fund  :verb ob-poc:cbu.add-product :args (:product "fund")      :next attach-im)
          (service-task :id add-corp  :verb ob-poc:cbu.add-product :args (:product "corporate") :next attach-im)
          (service-task :id add-trust :verb ob-poc:cbu.add-product :args (:product "trust")     :next attach-im)
          (service-task :id attach-im :verb ob-poc:instrument-matrix.attach :next end)
          (end-event :id end :status "Operational"))"#;
        let bpmn_wait_body = serde_json::json!({ "bpmn_dsl": bpmn_wait_src });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/compile/preview")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_string(&bpmn_wait_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 100000).await.unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["workflow_id"].as_str().unwrap(), "custody-cbu-onboarding");
        assert!(res["error"].is_null());

        // Test DMN compilation preview
        let dmn_src = r#"(define-decision test_dec
  :hit-policy unique
  :inputs  ((client_type :type enum :domain ClientType))
  :outputs ((cbu_type    :type enum :domain CbuType))
  :rules
    ((rule r1
       :when ((client_type = FUND_MANDATE))
       :then ((cbu_type = fund)))))"#;
        let dmn_body = serde_json::json!({ "dmn_dsl": dmn_src });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dmn/compile/preview")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_string(&dmn_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["decision_name"], "test_dec");
        assert_eq!(res["hit_policy"], "unique");
        assert_eq!(res["inputs"][0]["name"], "client_type");
        assert_eq!(res["rules"][0]["id"], "r1");
        assert_eq!(res["rules"][0]["inputs"][0]["op"], "==");
        assert_eq!(res["rules"][0]["inputs"][0]["value"], "FUND_MANDATE");
        assert_eq!(res["rules"][0]["outputs"][0], "fund");

        // Test GET DMN decision
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/dmn/decisions/cbu_type_routing")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["decision_name"], "cbu_type_routing");
        assert_eq!(res["hit_policy"], "first");
        assert_eq!(res["inputs"][0]["name"], "cbu-client-type");
    }

    #[tokio::test]
    async fn test_dsl_macro_and_diagnostics_endpoints() {
        let state = DemoState::new();
        let app = demo_router(state);

        // 1. Test /api/dsl/macro/apply (BoundedRetry)
        let source = r#"(workflow test
  (start-event :id start :next my-task)
  (service-task :id my-task :verb ob-poc:cbu.create :next end)
  (end-event :id end :status "completed")
)"#;
        let mut params = HashMap::new();
        params.insert("target_node_id".to_string(), "my-task".to_string());
        params.insert("ceiling".to_string(), "5".to_string());
        params.insert("custom_id".to_string(), "my-retry-loop".to_string());

        let body = MacroApplyRequest {
            source_code: source.to_string(),
            macro_type: "BoundedRetry".to_string(),
            parameters: params,
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/dsl/macro/apply")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        let modified_dsl = res["source_code"].as_str().unwrap();
        assert!(modified_dsl.contains("(loop :id my-retry-loop :ceiling 5"));

        // 2. Test /api/dsl/diagnostics/resolve (WireDeadEnd)
        let unresolved_source = r#"(workflow test
  (start-event :id start :next my-task)
  (service-task :id my-task :verb ob-poc:cbu.create :next dead-end)
  (end-event :id end :status "completed")
)"#;
        let fix_action = bpmn_lite_authoring::diagnostics_executor::FixAction::WireDeadEnd {
            node_id: "my-task".to_string(),
            target_id: "end".to_string(),
        };
        let resolve_body = DiagnosticsResolveRequest {
            source_code: unresolved_source.to_string(),
            action: fix_action,
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/dsl/diagnostics/resolve")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_string(&resolve_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        let fixed_dsl = res["source_code"].as_str().unwrap();
        assert!(fixed_dsl.contains("(service-task :id my-task :verb ob-poc:cbu.create :next end)"));

        // 3. Test /api/dsl/sage/utter (Global Intent Escape & Fallback)
        let utter_body = UtteranceRequest {
            utterance: "exit and go back".to_string(),
            _current_dsl: source.to_string(),
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/dsl/sage/utter")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_string(&utter_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000).await.unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(res["escape_intent_detected"].as_bool().unwrap());
        assert_eq!(res["suggested_action"].as_str().unwrap(), "exit");
    }
}

