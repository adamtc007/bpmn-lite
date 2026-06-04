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
    if let Some(ExecutionNode::Task(t)) = demo.plan.nodes.get(&node_id) {
        if t.plug.starts_with("dmn-lite:") {
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
        } else {
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
    store.save_instance("default", &instance).await?;
    Ok(instance_id)
}

/// Walk forward through non-callout nodes (Start, Split,
/// End) without touching the bus. Stops at the first Task
/// so the user can click "Next Step" there.
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
            Some(ExecutionNode::Start(n)) => {
                inst.current_node_id = Some(n.next.clone());
                let _ = store.save_instance("default", &inst).await;
            }
            Some(ExecutionNode::Split(gw)) => {
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
            Some(ExecutionNode::End(end)) => {
                inst.state = ProcessState::Completed {
                    at: chrono::Utc::now().timestamp_millis(),
                };
                inst.current_node_id = Some(end.id.clone());
                let _ = store.save_instance("default", &inst).await;
                break;
            }
            // Task / Join / Loop — stop here, user drives next step.
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
                ExecutionNode::Start(_) => NodeInfo {
                    id: (*id).to_owned(),
                    label: "Start".into(),
                    fqn: None,
                    target_domain: None,
                    kind: "start".into(),
                },
                ExecutionNode::Task(t) => {
                    if t.plug.starts_with("dmn-lite:") {
                        let (domain, dec_id) = split_fqn(&t.plug);
                        NodeInfo {
                            id: (*id).to_owned(),
                            label: format!("↗ Evaluating {domain}: {dec_id}"),
                            fqn: Some(t.plug.clone()),
                            target_domain: Some(domain.to_owned()),
                            kind: "business_rule_task".into(),
                        }
                    } else {
                        let (domain, verb_id) = split_fqn(&t.plug);
                        NodeInfo {
                            id: (*id).to_owned(),
                            label: format!("↗ Calling {domain}: {verb_id}"),
                            fqn: Some(t.plug.clone()),
                            target_domain: Some(domain.to_owned()),
                            kind: "service_task".into(),
                        }
                    }
                }
                ExecutionNode::Split(_) => NodeInfo {
                    id: (*id).to_owned(),
                    label: "◇ CBU Type Gateway".into(),
                    fqn: None,
                    target_domain: None,
                    kind: "gateway".into(),
                },
                ExecutionNode::End(_) => NodeInfo {
                    id: (*id).to_owned(),
                    label: "✓ End: CBU Operational".into(),
                    fqn: None,
                    target_domain: None,
                    kind: "end".into(),
                },
                ExecutionNode::Join(_) => NodeInfo {
                    id: (*id).to_owned(),
                    label: "Join".into(),
                    fqn: None,
                    target_domain: None,
                    kind: "join".into(),
                },
                ExecutionNode::Loop(_) => NodeInfo {
                    id: (*id).to_owned(),
                    label: "Loop".into(),
                    fqn: None,
                    target_domain: None,
                    kind: "loop".into(),
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

fn get_preview_registry() -> bpmn_lite_compiler::dsl::ManifestPlaceholderRegistry<bpmn_lite_compiler::dsl::StubPlaceholderRegistry> {
    use dsl_manifest::Manifest;
    use bpmn_lite_compiler::dsl::{ManifestPlaceholderRegistry, StubPlaceholderRegistry};

    let ob_poc = Manifest::load_from_yaml(OB_POC_MANIFEST_YAML)
        .expect("ob-poc manifest must load");
    let dmn_lite = Manifest::load_from_yaml(DMN_LITE_MANIFEST_YAML)
        .expect("dmn-lite manifest must load");

    let mut registry = ManifestPlaceholderRegistry::new(StubPlaceholderRegistry::new().with_demo_bindings());
    registry.import(ob_poc);
    registry.import(dmn_lite);
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
                            if let Some(idx) = remaining.find('\'') {
                                Some(&remaining[..idx])
                            } else {
                                None
                            }
                        } else if e.message.starts_with("decision '") {
                            let remaining = e.message.trim_start_matches("decision '");
                            if let Some(idx) = remaining.find('\'') {
                                Some(&remaining[..idx])
                            } else {
                                None
                            }
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
        let start_body = StartBody { cbu_type: "fund".to_owned() };
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
}

