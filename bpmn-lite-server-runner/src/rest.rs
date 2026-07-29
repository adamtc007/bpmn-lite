//! REST + SSE runner API for the bpmn-lite federated stack (T6).
//!
//! Runs on port 8080 alongside the existing gRPC server (50051).
//! Backed by `MemoryStore` — demo-mode only, no Postgres required.
//! For production process queries use the gRPC surface.
//!
//! This crate hosts the **workflow instance runner** half of the former
//! combined `bpmn-lite-server`: starting/advancing/inspecting running
//! instances. The DSL/graph authoring half lives in the sibling
//! `bpmn-lite-server-designer` crate.
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

use bpmn_lite_compiler::dsl::{ExecutionNode, WorkflowExecutionPlan};
use bpmn_lite_engine::{build_demo_plan, demo_initial_vars};
use bpmn_lite_store::store::{ArtifactRepository, RuntimeStore};
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::session_stack::SessionStackState;
use bpmn_lite_types::{ProcessInstance, ProcessState};
use bpmn_lite_types::TenantId;

// ── Runner state ───────────────────────────────────────────────────────

pub struct RunnerState {
    store: Arc<MemoryStore>,
    tenant_id: TenantId,
    plan: Arc<WorkflowExecutionPlan>,
    cbu_types: Mutex<HashMap<Uuid, String>>,
}

impl RunnerState {
    pub fn try_new() -> Result<Arc<Self>, anyhow::Error> {
        let plan = build_demo_plan()?;
        Ok(Arc::new(Self {
            store: Arc::new(MemoryStore::new()),
            tenant_id: TenantId::new("demo")?,
            plan: Arc::new(plan),
            cbu_types: Mutex::new(HashMap::new()),
        }))
    }

    fn cbu_type(&self, id: Uuid) -> String {
        self.cbu_types
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }

    fn set_cbu_type(&self, id: Uuid, t: String) {
        self.cbu_types
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, t);
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

pub fn runner_router(state: Arc<RunnerState>) -> Router {
    Router::new()
        .route("/bpmn/health", get(health))
        .route(
            "/bpmn/instances",
            get(list_instances).delete(reset_instances),
        )
        .route("/bpmn/instances/start", post(start_instance))
        .route("/bpmn/instances/:id", get(get_instance))
        .route("/bpmn/instances/:id/next-step", post(next_step))
        .route("/bpmn/instances/:id/graph", get(get_instance_graph))
        .route("/bpmn/instances/:id/stack", get(get_instance_stack))
        .route("/bpmn/instances/:id/sage", get(list_sage_stub))
        .route("/bpmn/instances/:id/events", get(events_stub))
        .with_state(state)
}

// ── Handlers ────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok", "service": "bpmn-lite-demo" }))
}

async fn list_instances(State(demo): State<Arc<RunnerState>>) -> impl IntoResponse {
    let ids = demo
        .store
        .list_running_instances(&demo.tenant_id)
        .await
        .unwrap_or_default();
    let mut result: Vec<WorkflowInstanceSummary> = Vec::new();
    for id in ids {
        if let Ok(Some(inst)) = demo.store.load_instance(&demo.tenant_id, id).await {
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
    State(demo): State<Arc<RunnerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let inst = match demo.store.load_instance(&demo.tenant_id, id).await {
        Ok(Some(i)) => i,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "not found"})),
            )
                .into_response();
        }
    };
    let plan = demo.plan.clone();

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
    State(demo): State<Arc<RunnerState>>,
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
                    })),
                )
                    .into_response();
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
            drive_forward(&demo.store, &demo.tenant_id, &plan, id).await;
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
    State(demo): State<Arc<RunnerState>>,
    Path(id): Path<Uuid>,
    body: Option<Json<NextStepBody>>,
) -> impl IntoResponse {
    let inst = match demo.store.load_instance(&demo.tenant_id, id).await {
        Ok(Some(i)) => i,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "not found"})),
            )
                .into_response();
        }
    };

    let plan = demo.plan.clone();

    let node_id = inst.current_node_id.clone().unwrap_or_default();
    let cbu_type = demo.cbu_type(id);
    let next_step_body = body.map(|Json(b)| b).unwrap_or_default();

    // Simulate result delivery for callout nodes, then drive forward through
    // any immediately following gateways/end events without touching the bus.
    if let Some(ExecutionNode::Task(t)) = plan.nodes().get(&node_id) {
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
                    Some((
                        placeholder_name.as_str(),
                        serde_json::Value::String(cbu_type_val.to_owned()),
                    ))
                } else {
                    let default_val = if node_id == "create-cbu" {
                        serde_json::Value::String(Uuid::now_v7().to_string())
                    } else {
                        serde_json::Value::String(format!("{node_id}-done"))
                    };
                    Some((placeholder_name.as_str(), default_val))
                }
            });

            apply_step(
                &demo.store,
                &demo.tenant_id,
                id,
                t.next.clone(),
                placeholder,
            )
            .await;
        } else {
            apply_step(&demo.store, &demo.tenant_id, id, t.next.clone(), None).await;
        }
    }

    // Drive forward through gateways and end events without the bus.
    drive_forward(&demo.store, &demo.tenant_id, &plan, id).await;

    let updated = demo
        .store
        .load_instance(&demo.tenant_id, id)
        .await
        .ok()
        .flatten();
    let (current, status) = updated
        .map(|i| {
            (
                i.current_node_id.clone().unwrap_or_default(),
                format_state(&i.state),
            )
        })
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

async fn reset_instances(State(demo): State<Arc<RunnerState>>) -> impl IntoResponse {
    demo.cbu_types
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
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
        ProcessState::Incidented { .. } => "Incidented".into(),
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
        Some(serde_json::Value::Object(initial_variables.into_iter().collect()))
    };

    let instance = ProcessInstance {
        instance_id,
        tenant_id: tenant_id.to_owned(),
        process_key: plan.workflow_id().to_string(),
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
        current_node_id: Some(plan.start_node().to_string()),
        placeholder_values,
    };
    let claim = bpmn_lite_types::Claim::new(
        bpmn_lite_types::TenantId::new(tenant_id.to_owned())?,
        instance_id,
        0,
        0,
    );
    let transition = bpmn_lite_types::TransitionBuilder::new(instance)
        .event(bpmn_lite_types::RuntimeEvent::InstanceStarted {
            instance_id,
            bytecode_version: [0u8; 32],
        })
        .build();
    store.commit_transition(&claim, &transition).await?;
    Ok(instance_id)
}

async fn commit_demo_instance(
    store: &MemoryStore,
    instance: &ProcessInstance,
) -> anyhow::Result<()> {
    let owner = "demo-rest";
    let claim = store
        .claim_instance_for_transition(
            &TenantId::new(instance.tenant_id.clone())?,
            instance.instance_id,
            owner,
            5_000,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("demo instance is leased"))?;
    let transition = bpmn_lite_types::TransitionBuilder::new(instance.clone()).build();
    let result = store.commit_transition(&claim, &transition).await;
    let release = store
        .release_instance_transition(
            &TenantId::new(instance.tenant_id.clone())?,
            instance.instance_id,
            owner,
        )
        .await;
    result?;
    release?;
    Ok(())
}

/// Walk forward through non-callout nodes (Start, Split,
/// End) without touching the bus. Stops at the first Task
/// so the user can click "Next Step" there.
async fn drive_forward(
    store: &MemoryStore,
    tenant_id: &TenantId,
    plan: &WorkflowExecutionPlan,
    id: Uuid,
) {
    use bpmn_lite_compiler::dsl::SplitMode;
    loop {
        let Ok(Some(mut inst)) = store.load_instance(tenant_id, id).await else {
            break;
        };
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

        match plan.nodes().get(&node_id) {
            Some(ExecutionNode::Start(n)) => {
                inst.current_node_id = Some(n.next.clone());
                let _ = commit_demo_instance(store, &inst).await;
            }
            Some(ExecutionNode::Split(gw)) => match gw.mode {
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
                        let _ = commit_demo_instance(store, &inst).await;
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
                            let condition_matches = if let (Some(ph), Some(exp)) =
                                (&flow.placeholder, &flow.expected_value)
                            {
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
                        let rest_vals: Vec<serde_json::Value> = rest
                            .iter()
                            .map(|s| serde_json::Value::String(s.clone()))
                            .collect();
                        updated_pv.insert(
                            "__pending_branches".to_string(),
                            serde_json::Value::Array(rest_vals),
                        );
                    } else {
                        updated_pv.remove("__pending_branches");
                    }

                    inst.placeholder_values = Some(serde_json::Value::Object(updated_pv.into_iter().collect()));
                    inst.current_node_id = Some(first);
                    let _ = commit_demo_instance(store, &inst).await;
                }
            },
            Some(ExecutionNode::Join(j)) => {
                let pending = pv.get("__pending_branches").and_then(|v| v.as_array());

                if let Some(branches) = pending {
                    if !branches.is_empty() {
                        let next_branch = branches[0].as_str().unwrap_or_default().to_string();
                        let rest = &branches[1..];

                        let mut updated_pv = pv.clone();
                        if !rest.is_empty() {
                            updated_pv.insert(
                                "__pending_branches".to_string(),
                                serde_json::Value::Array(rest.to_vec()),
                            );
                        } else {
                            updated_pv.remove("__pending_branches");
                        }

                        inst.placeholder_values = Some(serde_json::Value::Object(updated_pv.into_iter().collect()));
                        inst.current_node_id = Some(next_branch);
                        let _ = commit_demo_instance(store, &inst).await;
                    } else {
                        let mut updated_pv = pv.clone();
                        updated_pv.remove("__pending_branches");
                        inst.placeholder_values = Some(serde_json::Value::Object(updated_pv.into_iter().collect()));
                        inst.current_node_id = Some(j.next.clone());
                        let _ = commit_demo_instance(store, &inst).await;
                    }
                } else {
                    inst.current_node_id = Some(j.next.clone());
                    let _ = commit_demo_instance(store, &inst).await;
                }
            }
            Some(ExecutionNode::Loop(l)) => {
                let counter_key = format!("__loop_count_{}", l.id);
                let current_count =
                    pv.get(&counter_key).and_then(|v| v.as_u64()).unwrap_or(0) as u32;

                if current_count < l.ceiling {
                    let next_node = if let Some(first_body) = l.body.first() {
                        first_body.clone()
                    } else {
                        l.next.clone()
                    };

                    let mut updated_pv = pv.clone();
                    updated_pv.insert(
                        counter_key,
                        serde_json::Value::Number((current_count + 1).into()),
                    );
                    inst.placeholder_values = Some(serde_json::Value::Object(updated_pv.into_iter().collect()));
                    inst.current_node_id = Some(next_node);
                    let _ = commit_demo_instance(store, &inst).await;
                } else {
                    let mut updated_pv = pv.clone();
                    updated_pv.remove(&counter_key);
                    inst.placeholder_values = Some(serde_json::Value::Object(updated_pv.into_iter().collect()));
                    inst.current_node_id = Some(l.next.clone());
                    let _ = commit_demo_instance(store, &inst).await;
                }
            }
            Some(ExecutionNode::End(end)) => {
                inst.state = ProcessState::Completed {
                    at: chrono::Utc::now().timestamp_millis(),
                };
                inst.current_node_id = Some(end.id.clone());
                let _ = commit_demo_instance(store, &inst).await;
                break;
            }
            _ => break,
        }
    }
}

fn build_node_infos(plan: &WorkflowExecutionPlan) -> Vec<NodeInfo> {
    use bpmn_lite_compiler::dsl::JoinMode;
    use bpmn_lite_compiler::dsl::SplitMode;
    let mut ordered = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(plan.start_node().to_string());
    while let Some(curr) = queue.pop_front() {
        if !visited.insert(curr.clone()) {
            continue;
        }
        ordered.push(curr.clone());
        if let Some(node) = plan.nodes().get(&curr) {
            match node {
                ExecutionNode::Start(n) => {
                    queue.push_back(n.next.clone());
                }
                ExecutionNode::Task(t) => {
                    queue.push_back(t.next.clone());
                }
                ExecutionNode::Split(s) => {
                    for flow in &s.flows {
                        queue.push_back(flow.next.clone());
                    }
                }
                ExecutionNode::Join(j) => {
                    queue.push_back(j.next.clone());
                }
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
    for id in plan.nodes().keys() {
        if !visited.contains(id) {
            ordered.push(id.clone());
        }
    }

    ordered
        .iter()
        .filter_map(|id| {
            let node = plan.nodes().get(id)?;
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
    tenant_id: &TenantId,
    id: Uuid,
    next_node: String,
    placeholder: Option<(&str, serde_json::Value)>,
) {
    let Ok(Some(mut inst)) = store.load_instance(tenant_id, id).await else {
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
    inst.placeholder_values = Some(serde_json::Value::Object(pv.into_iter().collect()));
    let _ = commit_demo_instance(store, &inst).await;
}

async fn get_instance_graph(
    State(demo): State<Arc<RunnerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let _instance = match demo.store.load_instance(&demo.tenant_id, id).await {
        Ok(Some(i)) => i,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "instance not found"})),
            )
                .into_response();
        }
    };
    let graph = plan_to_visual_graph(&demo.plan);
    Json(graph).into_response()
}

async fn get_instance_stack(
    State(demo): State<Arc<RunnerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let mut current = match demo.store.load_instance(&demo.tenant_id, id).await {
        Ok(Some(i)) => i,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "instance not found"})),
            )
                .into_response();
        }
    };

    let mut frames = Vec::new();
    let mut visited = std::collections::HashSet::new();
    visited.insert(current.instance_id);
    loop {
        let mut span = None;
        let mut plug = None;

        if let Some(node_id) = &current.current_node_id {
            if let Some(node) = demo.plan.nodes().get(node_id) {
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
                    if let Ok(Some(parent)) =
                        demo.store.load_instance(&demo.tenant_id, parent_id).await
                    {
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
    use bpmn_lite_compiler::dsl::JoinMode;
    use bpmn_lite_compiler::dsl::SplitMode;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for (id, node) in plan.nodes() {
        let (kind, label, plug, span) = match node {
            ExecutionNode::Start(st) => ("start".to_owned(), "Start".to_owned(), None, st.span),
            ExecutionNode::End(e) => (
                "end".to_owned(),
                format!("End ({})", e.status),
                None,
                e.span,
            ),
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
            ExecutionNode::Loop(l) => (
                "loop".to_owned(),
                format!("Loop (Max {})", l.ceiling),
                None,
                l.span,
            ),
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
                edges.push(VisualEdgeDto {
                    from: id.clone(),
                    to: n.next.clone(),
                    condition: None,
                });
            }
            ExecutionNode::Task(t) => {
                edges.push(VisualEdgeDto {
                    from: id.clone(),
                    to: t.next.clone(),
                    condition: None,
                });
            }
            ExecutionNode::Split(s) => {
                for flow in &s.flows {
                    let cond_str =
                        if let (Some(ph), Some(val)) = (&flow.placeholder, &flow.expected_value) {
                            Some(format!("{} == {:?}", ph, val))
                        } else {
                            None
                        };
                    edges.push(VisualEdgeDto {
                        from: id.clone(),
                        to: flow.next.clone(),
                        condition: cond_str,
                    });
                }
            }
            ExecutionNode::Join(j) => {
                edges.push(VisualEdgeDto {
                    from: id.clone(),
                    to: j.next.clone(),
                    condition: None,
                });
            }
            ExecutionNode::Loop(l) => {
                if let Some(first_body) = l.body.first() {
                    edges.push(VisualEdgeDto {
                        from: id.clone(),
                        to: first_body.clone(),
                        condition: Some("Loop Body".into()),
                    });
                }
                edges.push(VisualEdgeDto {
                    from: id.clone(),
                    to: l.next.clone(),
                    condition: Some("Loop Exit".into()),
                });
            }
            ExecutionNode::End(_) => {}
        }
    }

    VisualGraphDto {
        workflow_id: plan.workflow_id().to_string(),
        nodes,
        edges,
    }
}
const OB_POC_MANIFEST_YAML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../manifests/ob-poc-v1.0.0.yaml"
));
const DMN_LITE_MANIFEST_YAML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../manifests/dmn-lite-v1.0.0.yaml"
));
const BPMN_MANIFEST_YAML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../manifests/bpmn-v1.0.0.yaml"
));

fn get_preview_registry() -> bpmn_lite_compiler::dsl::ManifestPlaceholderRegistry<
    bpmn_lite_compiler::dsl::StubPlaceholderRegistry,
> {
    use bpmn_lite_compiler::dsl::{ManifestPlaceholderRegistry, StubPlaceholderRegistry};
    use dsl_manifest::Manifest;

    let ob_poc = Manifest::load_from_yaml(OB_POC_MANIFEST_YAML).expect("ob-poc manifest must load");
    let dmn_lite =
        Manifest::load_from_yaml(DMN_LITE_MANIFEST_YAML).expect("dmn-lite manifest must load");
    let bpmn = Manifest::load_from_yaml(BPMN_MANIFEST_YAML).expect("bpmn manifest must load");

    let mut registry =
        ManifestPlaceholderRegistry::new(StubPlaceholderRegistry::new().with_demo_bindings());
    registry.import(ob_poc);
    registry.import(dmn_lite);
    registry.import(bpmn);
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt; // for `oneshot`

    #[tokio::test]
    async fn test_rest_graph_and_stack_endpoints() {
        let state = RunnerState::try_new().unwrap();
        let app = runner_router(state.clone());

        // 1. Start a demo instance
        let start_body = StartBody {
            cbu_type: "fund".to_owned(),
            bpmn_dsl: None,
            variables: None,
        };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/instances/start")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&start_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
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
        let body_bytes = axum::body::to_bytes(response.into_body(), 100000)
            .await
            .unwrap();
        let graph: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(graph["workflow_id"], "custody-cbu-onboarding");
        let nodes = graph["nodes"].as_array().unwrap();
        assert!(!nodes.is_empty());
        let edges = graph["edges"].as_array().unwrap();
        assert!(!edges.is_empty());

        // Verify start node exists and has a span (since it was compiled from DSL!)
        let start_node = nodes.iter().find(|n| n["id"] == "start").unwrap();
        assert_eq!(start_node["kind"], "start");
        assert!(
            start_node["span"].is_object(),
            "start node should carry parsed source span metadata"
        );

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
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let stack: Value = serde_json::from_slice(&body_bytes).unwrap();
        let frames = stack.as_array().unwrap();
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert_eq!(frame["instance_id"], instance_id_str);
        assert_eq!(frame["workflow_id"], "custody-cbu-onboarding");
        assert_eq!(frame["node_id"], "create-cbu"); // Drive forward stops at the first Task node
        assert!(
            frame["span"].is_object(),
            "active stack frame should carry parsed source span metadata"
        );
    }
}
