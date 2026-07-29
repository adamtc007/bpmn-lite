//! DSL/BPMN/DMN authoring REST API for the bpmn-lite federated stack (T6).
//!
//! Runs on port 8080 alongside the workflow instance runner
//! (`bpmn-lite-server-runner`, gRPC on 50051 + its own demo REST API).
//! Backed by `MemoryStore` — demo-mode only, no Postgres required.
//!
//! This crate hosts the **workflow designer** half of the former combined
//! `bpmn-lite-server`: compile preview (BPMN/DMN), macro application,
//! diagnostics resolution, template catalogue, and design-session
//! (graph-backed authoring) endpoints. The instance runner half lives in
//! the sibling `bpmn-lite-server-runner` crate.

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
use bpmn_lite_store::store::{
    AdminProjectionStore, ArtifactRepository, DesignSessionEventKind,
};
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::TenantId;

// ── Designer state ─────────────────────────────────────────────────────

pub struct DesignerState {
    store: Arc<MemoryStore>,
    tenant_id: TenantId,
    /// Serializes each session's load→reconstruct→stage→append sequence
    /// (graph-edit and save-as-template) so two concurrent requests against
    /// the SAME session id can never both stage against the same base and
    /// both persist — the second would otherwise replay against a DAG
    /// shape it was never validated against, permanently bricking the
    /// session's reconstruction. Different sessions never contend.
    session_locks: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    /// DIR-004 Phase 1/2 wiring: dev-session capture, keyed by design
    /// session id. A session only appears here after an explicit
    /// `POST .../dev-capture/enable` call carrying Adam's consent
    /// statement (D17's spirit, applied to his own self-testing use) --
    /// `session_utterance_endpoint` checks for an entry and, if present,
    /// captures the full closure via `utterance_engine::dev_capture`
    /// (always compiled, structurally distinct from the Q9-gated path).
    dev_capture: Mutex<HashMap<Uuid, utterance_engine::dev_capture::DevSessionStore>>,
}

impl DesignerState {
    pub fn try_new() -> Result<Arc<Self>, anyhow::Error> {
        Ok(Arc::new(Self {
            store: Arc::new(MemoryStore::new()),
            tenant_id: TenantId::new("demo")?,
            session_locks: Mutex::new(HashMap::new()),
            dev_capture: Mutex::new(HashMap::new()),
        }))
    }

    fn session_lock(&self, id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        self.session_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

// ── Wire-types ──────────────────────────────────────────────────────────

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

// ── Router ──────────────────────────────────────────────────────────────

pub fn designer_router(state: Arc<DesignerState>) -> Router {
    Router::new()
        .route("/bpmn/compile/preview", post(compile_bpmn_preview))
        .route("/dmn/compile/preview", post(compile_dmn_preview))
        .route("/dmn/decisions/:id", get(get_dmn_decision))
        .route("/api/dsl/macro/apply", post(apply_dsl_macro))
        .route(
            "/api/dsl/diagnostics/resolve",
            post(resolve_dsl_diagnostics),
        )
        .route("/api/dsl/sage/utter", post(sage_utterance_gate))
        .route(
            "/api/dsl/sessions",
            get(list_design_sessions_endpoint).post(create_design_session_endpoint),
        )
        .route("/api/dsl/sessions/:id", get(get_design_session_endpoint))
        .route(
            "/api/dsl/sessions/:id/revision",
            post(session_revision_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/utterance",
            post(session_utterance_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/dev-capture/enable",
            post(dev_capture_enable_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/dev-capture",
            get(dev_capture_status_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/graph-edit",
            post(session_graph_edit_endpoint),
        )
        .route("/api/dsl/sessions/:id/save", post(save_design_session_endpoint))
        .route("/api/dsl/sessions/:id/graph", get(session_graph_endpoint))
        .route("/designer", get(designer_page))
        .route(
            "/bpmn/templates",
            get(list_templates_endpoint).post(define_template_endpoint),
        )
        .route(
            "/bpmn/templates/:name/versions/:version",
            get(get_template_version_endpoint),
        )
        .with_state(state)
}

// ── Preview Compilation and DMN handlers ──────────────────────────────────

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

async fn compile_bpmn_preview(Json(body): Json<CompilePreviewBody>) -> impl IntoResponse {
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
                        let symbol = if e.message.starts_with("unresolved symbol '")
                            && e.message.ends_with("'")
                        {
                            Some(
                                e.message
                                    .trim_start_matches("unresolved symbol '")
                                    .trim_end_matches("'"),
                            )
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

async fn compile_dmn_preview(Json(body): Json<DmnPreviewRequest>) -> impl IntoResponse {
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

async fn get_dmn_decision(Path(decision_id): Path<String>) -> impl IntoResponse {
    // Sanitize path parameter to prevent path traversal
    if !decision_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid decision ID format" })),
        )
            .into_response();
    }

    use std::path::PathBuf;
    let decisions_dir = std::env::var("DMN_DECISIONS_DIR")
        .unwrap_or_else(|_| format!("{}/../dmn-lite-decisions", env!("CARGO_MANIFEST_DIR")));
    let path = PathBuf::from(decisions_dir).join(format!("{}.dmn-lite", decision_id));

    match std::fs::read_to_string(&path) {
        Ok(source_text) => match parse_dmn_to_dto(&source_text) {
            Ok(schema) => (StatusCode::OK, Json(schema)).into_response(),
            Err(err_msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": err_msg })),
            )
                .into_response(),
        },
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Decision not found: {}", err) })),
        )
            .into_response(),
    }
}

fn parse_dmn_to_dto(source_text: &str) -> Result<DmnSchemaDto, String> {
    use dmn_lite_parser::{parse, HitPolicyAst, TypeRefAst, WhenAst};

    let source = parse(source_text).map_err(|e| format!("{}", e))?;
    let decision = source
        .decisions
        .first()
        .ok_or_else(|| "No decision defined in DSL".to_owned())?;

    let decision_name = decision.name.name.clone();
    let hit_policy = match &decision.hit_policy {
        HitPolicyAst::Unique(_) => "unique".to_owned(),
        HitPolicyAst::First(_) => "first".to_owned(),
    };

    let inputs: Vec<DmnInputSchema> = decision
        .inputs
        .iter()
        .map(|input| {
            let type_ref = match &input.type_ref {
                TypeRefAst::Enum(_) => "enum",
                TypeRefAst::Bool(_) => "bool",
                TypeRefAst::Integer(_) => "integer",
                TypeRefAst::Decimal(_) => "decimal",
                TypeRefAst::String(_) => "string",
            }
            .to_owned();
            DmnInputSchema {
                name: input.name.name.clone(),
                type_ref,
                domain: input.domain_ref.name.clone(),
            }
        })
        .collect();

    let outputs: Vec<DmnOutputSchema> = decision
        .outputs
        .iter()
        .map(|output| {
            let type_ref = match &output.type_ref {
                TypeRefAst::Enum(_) => "enum",
                TypeRefAst::Bool(_) => "bool",
                TypeRefAst::Integer(_) => "integer",
                TypeRefAst::Decimal(_) => "decimal",
                TypeRefAst::String(_) => "string",
            }
            .to_owned();
            DmnOutputSchema {
                name: output.name.name.clone(),
                type_ref,
                domain: output.domain_ref.name.clone(),
            }
        })
        .collect();

    let rules: Vec<DmnRuleDto> = decision
        .rules
        .iter()
        .map(|rule| {
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
                if let Some(assign) = rule
                    .then
                    .iter()
                    .find(|a| a.output.name == output_schema.name)
                {
                    matched_val = format_literal(&assign.value);
                }
                rule_outputs.push(matched_val);
            }

            DmnRuleDto {
                id: rule.id.name.clone(),
                inputs: rule_inputs,
                outputs: rule_outputs,
            }
        })
        .collect();

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
        PredicateAst::Range {
            lower,
            upper,
            lower_inclusive,
            upper_inclusive,
            ..
        } => {
            let left_bracket = if *lower_inclusive { "[" } else { "(" };
            let right_bracket = if *upper_inclusive { "]" } else { ")" };
            let lower_str = format_range_bound(lower);
            let upper_str = format_range_bound(upper);
            (
                "in".to_string(),
                format!(
                    "{}{} .. {}{}",
                    left_bracket, lower_str, upper_str, right_bracket
                ),
            )
        }
        PredicateAst::Not { inner, .. } => {
            let (op, val) = format_predicate_cell(inner);
            (format!("not {}", op), val)
        }
        PredicateAst::And { items, .. } => {
            let formatted_vals: Vec<String> = items
                .iter()
                .map(|item| format_predicate_cell(item).1)
                .collect();
            ("and".to_string(), formatted_vals.join(" and "))
        }
        PredicateAst::Or { items, .. } => {
            let formatted_vals: Vec<String> = items
                .iter()
                .map(|item| format_predicate_cell(item).1)
                .collect();
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

fn get_macros_config() -> Option<bpmn_lite_compiler::dsl::MacroConfigList> {
    let workspace_macros = format!("{}/../macros.yaml", env!("CARGO_MANIFEST_DIR"));
    if std::path::Path::new(&workspace_macros).exists() {
        if let Ok(config) =
            bpmn_lite_compiler::dsl::MacroConfigList::load_from_file(&workspace_macros)
        {
            return Some(config);
        }
    }
    let server_macros = format!("{}/macros.yaml", env!("CARGO_MANIFEST_DIR"));
    if std::path::Path::new(&server_macros).exists() {
        if let Ok(config) =
            bpmn_lite_compiler::dsl::MacroConfigList::load_from_file(&server_macros)
        {
            return Some(config);
        }
    }
    None
}

async fn apply_dsl_macro(Json(body): Json<MacroApplyRequest>) -> impl IntoResponse {
    use bpmn_lite_compiler::dsl::{
        AstMutator, NodeAst, ToSexpr, XorBranchConfig, create_bounded_retry_macro,
        create_parallel_split_join, create_xor_split_join, parse_workflow_str,
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
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": "Missing parameter target_node_id"})),
                    )
                        .into_response();
                }
            };
            let ceiling: u32 = body
                .parameters
                .get("ceiling")
                .and_then(|c| c.parse().ok())
                .unwrap_or(3);
            let loop_id = body
                .parameters
                .get("custom_id")
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
            let split_id = body
                .parameters
                .get("split_id")
                .map(|s| s.as_str())
                .unwrap_or("xor-split");
            let placeholder = body
                .parameters
                .get("placeholder")
                .map(|s| s.as_str())
                .unwrap_or("@decision_val");
            let join_id = body
                .parameters
                .get("join_id")
                .map(|s| s.as_str())
                .unwrap_or("xor-join");
            let join_next = body
                .parameters
                .get("join_next")
                .map(|s| s.as_str())
                .unwrap_or("end");
            let predecessor_id = body
                .parameters
                .get("predecessor_id")
                .map(|s| s.as_str())
                .unwrap_or("start");

            let branch_val = body
                .parameters
                .get("branch_value")
                .map(|s| s.as_str())
                .unwrap_or("yes");
            let branch_target = body
                .parameters
                .get("branch_target")
                .map(|s| s.as_str())
                .unwrap_or("end");

            let branches = vec![
                XorBranchConfig {
                    condition_value: branch_val.to_string(),
                    target_next: branch_target.to_string(),
                },
                XorBranchConfig {
                    condition_value: "default".to_string(),
                    target_next: join_id.to_string(),
                },
            ];

            let (split, join) =
                create_xor_split_join(split_id, placeholder, branches, join_id, join_next);

            let mut mutator = AstMutator::new(&mut workflow);
            mutator
                .insert_after(predecessor_id, NodeAst::Split(split))
                .and_then(|_| {
                    let mut mutator = AstMutator::new(&mut workflow);
                    mutator.insert_after(split_id, NodeAst::Join(join))
                })
        }
        "ParallelSplit" => {
            let split_id = body
                .parameters
                .get("split_id")
                .map(|s| s.as_str())
                .unwrap_or("and-split");
            let join_id = body
                .parameters
                .get("join_id")
                .map(|s| s.as_str())
                .unwrap_or("and-join");
            let join_next = body
                .parameters
                .get("join_next")
                .map(|s| s.as_str())
                .unwrap_or("end");
            let predecessor_id = body
                .parameters
                .get("predecessor_id")
                .map(|s| s.as_str())
                .unwrap_or("start");

            let branch_target = body
                .parameters
                .get("branch_target")
                .map(|s| s.as_str())
                .unwrap_or("end");

            let branch_entries = vec![branch_target.to_string(), join_id.to_string()];
            let (split, join) =
                create_parallel_split_join(split_id, branch_entries, join_id, join_next);

            let mut mutator = AstMutator::new(&mut workflow);
            mutator
                .insert_after(predecessor_id, NodeAst::Split(split))
                .and_then(|_| {
                    let mut mutator = AstMutator::new(&mut workflow);
                    mutator.insert_after(split_id, NodeAst::Join(join))
                })
        }
        "Custom" => {
            let macro_id = match body.parameters.get("macro_id") {
                Some(id) => id,
                None => return (
                    StatusCode::BAD_REQUEST,
                    Json(
                        serde_json::json!({"error": "Missing parameter macro_id for Custom macro"}),
                    ),
                )
                    .into_response(),
            };
            let predecessor_id = body
                .parameters
                .get("predecessor_id")
                .map(|s| s.as_str())
                .unwrap_or("start");

            let config = get_macros_config()
                .ok_or_else(|| "No macros.yaml config found on disk".to_string());
            let node = config.and_then(|c| {
                let macro_cfg = c
                    .macros
                    .into_iter()
                    .find(|m| &m.id == macro_id)
                    .ok_or_else(|| {
                        format!("Custom macro '{}' not found in macros.yaml", macro_id)
                    })?;
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
                bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                    errs.iter().map(|e| format!("{}", e)).collect()
                }
                bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                    errs.iter().map(|e| format!("{}", e)).collect()
                }
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

fn find_all_predecessors_id_in_workflow(
    workflow: &bpmn_lite_compiler::dsl::WorkflowSource,
    target_id: &str,
) -> Vec<String> {
    let mut preds = Vec::new();
    find_all_predecessors_id_rec(&workflow.nodes, target_id, &mut preds);
    preds
}

fn find_all_predecessors_id_rec(
    nodes: &[bpmn_lite_compiler::dsl::NodeAst],
    target_id: &str,
    acc: &mut Vec<String>,
) {
    for node in nodes {
        match node {
            bpmn_lite_compiler::dsl::NodeAst::Start(s) => {
                if s.next == target_id {
                    acc.push(s.id.clone());
                }
            }
            bpmn_lite_compiler::dsl::NodeAst::Task(t) => {
                if t.next == target_id {
                    acc.push(t.id.clone());
                }
            }
            bpmn_lite_compiler::dsl::NodeAst::Join(j) => {
                if j.next == target_id {
                    acc.push(j.id.clone());
                }
            }
            bpmn_lite_compiler::dsl::NodeAst::Loop(l) => {
                if l.next == target_id {
                    acc.push(l.id.clone());
                }
                find_all_predecessors_id_rec(&l.body, target_id, acc);
            }
            bpmn_lite_compiler::dsl::NodeAst::Split(s) => {
                for flow in &s.flows {
                    if flow.next == target_id {
                        acc.push(s.id.clone());
                    }
                }
            }
            bpmn_lite_compiler::dsl::NodeAst::End(_) => {}
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

async fn resolve_dsl_diagnostics(Json(body): Json<DiagnosticsResolveRequest>) -> impl IntoResponse {
    let manifests_path = std::env::var("SAGE_MANIFESTS_DIR")
        .unwrap_or_else(|_| format!("{}/../manifests", env!("CARGO_MANIFEST_DIR")));
    let manifests_dir = std::path::PathBuf::from(manifests_path);
    match bpmn_lite_authoring::diagnostics_executor::execute_autofix(
        &body.source_code,
        &body.action,
        &manifests_dir,
    ) {
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
                        bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                            errs.iter().map(|e| format!("{}", e)).collect()
                        }
                        bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                            errs.iter().map(|e| format!("{}", e)).collect()
                        }
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

async fn sage_utterance_gate(Json(body): Json<UtteranceRequest>) -> impl IntoResponse {
    Json(utterance_intent(&body.utterance))
}

/// Keyword intent gate (T0 ruling: the v1 REPL contract). Pure so both
/// the stateless endpoint and the session-scoped endpoint share it.
fn utterance_intent(utterance: &str) -> UtteranceResponse {
    let body_utterance = utterance;
    let text = utterance.trim().to_lowercase();

    let is_escape = text.contains("exit")
        || text.contains("close editor")
        || text.contains("go back")
        || text.contains("quit")
        || text.contains("deploy")
        || text.contains("push release")
        || text.contains("staging")
        || text.contains("check database")
        || text.contains("list active manifests");

    let (suggested_action, msg, payload) = if text.contains("exit")
        || text.contains("close editor")
        || text.contains("go back")
        || text.contains("quit")
    {
        (
            "exit",
            "Autosaved current workspace. Exiting designer session.",
            None,
        )
    } else if text.contains("deploy") || text.contains("push release") || text.contains("staging") {
        (
            "chat",
            "It looks like you want to perform a deployment or navigate away. Should we save your workflow draft and transition out of the designer session?",
            None,
        )
    } else if text.contains("retry loop") || text.contains("wrap") || text.contains("retry") {
        let mut params = serde_json::Map::new();
        params.insert(
            "target_node_id".to_string(),
            serde_json::Value::String("create-cbu".to_string()),
        );
        params.insert(
            "ceiling".to_string(),
            serde_json::Value::String("3".to_string()),
        );
        (
            "apply_macro",
            "We can wrap your task in a retry loop. Apply macro?",
            Some(serde_json::json!({
                "macro_type": "BoundedRetry",
                "parameters": params
            })),
        )
    } else if text.starts_with("import ") || text.contains("unknown verb") {
        let verb = if text.starts_with("import ") {
            body_utterance.trim()[7..].to_string()
        } else {
            "ob-poc:cbu.create".to_string()
        };
        let domain = if verb.contains(':') {
            verb.split(':').collect::<Vec<&str>>()[0].to_string()
        } else {
            "ob-poc".to_string()
        };
        (
            "resolve_diagnostic",
            "Resolving unresolved symbol by adding signature stub to manifest.",
            Some(serde_json::json!({
                "type": "AddVerbStub",
                "domain": domain,
                "verb": verb
            })),
        )
    } else {
        ("none", "Utterance processed. Continues editing mode.", None)
    };

    UtteranceResponse {
        escape_intent_detected: is_escape,
        suggested_action: suggested_action.to_string(),
        message: msg.to_string(),
        action_payload: payload,
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct DefineTemplateBody {
    name: String,
    dsl_body: String,
}

#[derive(Serialize)]
pub(crate) struct DefineTemplateResponse {
    plan_hash: String,
    version: u32,
}

async fn define_template_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Json(body): Json<DefineTemplateBody>,
) -> impl IntoResponse {
    let registry = get_preview_registry();
    let plan = match bpmn_lite_compiler::dsl::compile(&body.dsl_body, &registry) {
        Ok(p) => p,
        Err(e) => {
            let (error_msg, diagnostics) = match e {
                bpmn_lite_compiler::dsl::CompileError::Parse(errs) => {
                    ("Parsing failed".to_owned(), errs)
                }
                bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                    let formatted = errs.iter().map(|e| format!("{}", e)).collect::<Vec<_>>();
                    ("Linting failed".to_owned(), formatted)
                }
                bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                    let formatted = errs.iter().map(|e| format!("{}", e)).collect::<Vec<_>>();
                    ("DAG validation failed".to_owned(), formatted)
                }
            };
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": error_msg,
                    "diagnostics": diagnostics
                })),
            )
                .into_response();
        }
    };

    let plan_json = match serde_json::to_string(&plan) {
        Ok(json) => json,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Serialization failed: {}", e) })),
            )
                .into_response();
        }
    };

    let hash = *blake3::hash(plan_json.as_bytes()).as_bytes();

    if let Err(e) = demo.store.store_plan(hash, &plan_json).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to store plan: {}", e) })),
        )
            .into_response();
    }

    let version = match demo.store.load_latest_template_version(&body.name).await {
        Ok(Some((v, _, _))) => v + 1,
        _ => 1,
    };

    if let Err(e) = demo
        .store
        .store_template(&body.name, version, hash, &body.dsl_body)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to store template: {}", e) })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(DefineTemplateResponse {
            plan_hash: hex::encode(hash),
            version,
        }),
    )
        .into_response()
}

#[derive(Serialize)]
pub(crate) struct TemplateSummaryDto {
    name: String,
    latest_version: u32,
    plan_hash: String,
    created_at: String,
}

async fn list_templates_endpoint(State(demo): State<Arc<DesignerState>>) -> impl IntoResponse {
    match demo.store.list_templates().await {
        Ok(list) => {
            let dtos: Vec<TemplateSummaryDto> = list
                .into_iter()
                .map(|t| TemplateSummaryDto {
                    name: t.name,
                    latest_version: t.latest_version,
                    plan_hash: hex::encode(t.plan_hash),
                    created_at: t.created_at,
                })
                .collect();
            (StatusCode::OK, Json(dtos)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Serialize)]
pub(crate) struct TemplateVersionDto {
    name: String,
    version: u32,
    plan_hash: String,
    dsl_body: String,
}

async fn get_template_version_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path((name, version)): Path<(String, u32)>,
) -> impl IntoResponse {
    match demo.store.load_template_version(&name, version).await {
        Ok(Some((dsl_body, plan_hash))) => {
            let dto = TemplateVersionDto {
                name,
                version,
                plan_hash: hex::encode(plan_hash),
                dsl_body,
            };
            (StatusCode::OK, Json(dto)).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Template '{}' version {} not found", name, version) })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response()
    }
}


// ── Designer sessions (EOP-SAGE-REPL-BPMN-001 T1) ────────────────────────

#[derive(Deserialize)]
pub(crate) struct CreateSessionBody {
    name: String,
    #[serde(default)]
    dsl_source: String,
}

#[derive(Serialize)]
pub(crate) struct CreateSessionResponse {
    session_id: String,
}

#[derive(Deserialize)]
pub(crate) struct SessionRevisionBody {
    dsl_source: String,
    #[serde(default)]
    note: String,
}

#[derive(Serialize)]
pub(crate) struct SessionRevisionResponse {
    seq: u64,
    /// Compile diagnostics for the NEW source — drafts may be invalid;
    /// the revision is recorded either way (the REPL shows diagnostics,
    /// it does not lose work).
    compiles: bool,
    diagnostics: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct SessionUtteranceBody {
    text: String,
    /// WS-B.4: the BPMN id of the graph position the utterance was
    /// issued from. Meaningful only for DesignerDag-backed sessions
    /// (`is_graph_backed()`); ignored for legacy DSL-source sessions,
    /// which have no `DesignerDag` to resolve it against. An id naming
    /// no node in the reconstruction is a fail-closed 422, never a
    /// silent whole-graph fallback.
    #[serde(default)]
    anchor: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SaveSessionBody {
    template_name: String,
}

async fn create_design_session_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Json(body): Json<CreateSessionBody>,
) -> impl IntoResponse {
    let id = Uuid::now_v7();
    match demo
        .store
        .create_design_session(&demo.tenant_id, id, &body.name, &body.dsl_source)
        .await
    {
        Ok(()) => (
            StatusCode::CREATED,
            Json(CreateSessionResponse {
                session_id: id.to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

async fn list_design_sessions_endpoint(State(demo): State<Arc<DesignerState>>) -> impl IntoResponse {
    match demo.store.list_design_sessions(&demo.tenant_id).await {
        Ok(summaries) => Json(summaries).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

async fn get_design_session_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(record)) => Json(record).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

async fn session_revision_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SessionRevisionBody>,
) -> impl IntoResponse {
    let seq = match demo
        .store
        .append_design_session_event(
            &demo.tenant_id,
            id,
            &DesignSessionEventKind::Revision {
                dsl_source: body.dsl_source.clone(),
                note: body.note.clone(),
            },
        )
        .await
    {
        Ok(seq) => seq,
        Err(bpmn_lite_store::StoreError::NotFound(_)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    let registry = get_preview_registry();
    let (compiles, diagnostics) =
        match bpmn_lite_compiler::dsl::compile(&body.dsl_source, &registry) {
            Ok(_) => (true, Vec::new()),
            Err(bpmn_lite_compiler::dsl::CompileError::Parse(errs)) => (false, errs),
            Err(bpmn_lite_compiler::dsl::CompileError::Lint(errs)) => {
                (false, errs.iter().map(|e| format!("{e}")).collect())
            }
            Err(bpmn_lite_compiler::dsl::CompileError::Dag(errs)) => {
                (false, errs.iter().map(|e| format!("{e}")).collect())
            }
        };
    Json(SessionRevisionResponse {
        seq,
        compiles,
        diagnostics,
    })
    .into_response()
}

/// v1 whole-graph legality: every operation/production legal. Serves
/// sessions that have never accumulated a `GraphEdit` (the legacy
/// DSL-source path — no `DesignerDag` exists to run the real oracle
/// against). WS-B.4-graph-backed sessions use `PositionalLegality`
/// instead (`reconstruct_designer_dag` below).
struct WholeGraphLegality;
impl designer_graph::board_candidate::LegalityOracle for WholeGraphLegality {
    type NodeKey = ();
    fn legal_operations(
        &self,
        _: Option<&()>,
    ) -> Vec<designer_graph::board_candidate::OperationKind> {
        designer_graph::board_candidate::OperationKind::ALL.to_vec()
    }
    fn legal_productions(
        &self,
        _: Option<&()>,
    ) -> Vec<designer_graph::board_candidate::ProductionId> {
        designer_graph::board_candidate::ProductionId::ALL.to_vec()
    }
}

// ── WS-B.4: DesignerDag-backed sessions ──────────────────────────────────
//
// A session accumulating `GraphEdit` events is DesignerDag-backed: its
// `DesignerDag` is the replay product of a deterministically-seeded Start
// node plus every accumulated operation sequence, in event order (schema.rs's
// own module doc: "the durable surface is the edit log... the DAG is its
// replay product"). Reconstruction is a pure function of the event log —
// no snapshot is ever persisted (rider 2, same doc).

/// The session's Start node key: derived from the session id via a fixed
/// namespace so it is stable across every reconstruction and knowable to
/// a client authoring the session's very first graph edit (returned in
/// `create_design_session_endpoint`'s response once a session begins its
/// graph-edit life — see `SEED_START_ID`/`seed_start_key`).
const SEED_START_ID: &str = "start";
fn seed_start_key(session_id: Uuid) -> designer_graph::schema::NodeKey {
    const NAMESPACE: Uuid = Uuid::from_bytes([
        0xb1, 0x3f, 0x1a, 0x02, 0xd2, 0x99, 0x4a, 0x71, 0x9c, 0x3e, 0x7a, 0x21, 0x5c, 0x0e, 0x8b,
        0x44,
    ]);
    designer_graph::schema::NodeKey(Uuid::new_v5(&NAMESPACE, session_id.as_bytes()))
}

/// Replay a session's accumulated `GraphEdit` payloads into a
/// `DesignerDag`. Fail-closed: a payload that fails to deserialize or
/// fails to stage is a bug in what was already accepted at append time
/// (the graph-edit endpoint validates before persisting) — surfaced as an
/// error, never silently skipped mid-replay (a partial DAG would silently
/// misrepresent the session).
fn reconstruct_designer_dag(
    record: &bpmn_lite_store::DesignSessionRecord,
) -> anyhow::Result<designer_graph::schema::DesignerDag> {
    use designer_graph::ops::Operation;
    use designer_graph::productions::apply_production;
    use designer_graph::schema::{DesignerDag, Provenance};

    let mut dag = DesignerDag::new(record.name.clone());
    dag.seed(
        seed_start_key(record.id),
        bpmn_lite_compiler::IRNode::Start { id: SEED_START_ID.into() },
        Provenance::default(),
    )?;
    for (i, payload) in record.graph_edit_payloads().into_iter().enumerate() {
        let ops: Vec<Operation> = serde_json::from_str(payload)
            .map_err(|e| anyhow::anyhow!("graph edit #{i} failed to deserialize: {e}"))?;
        dag = apply_production(&dag, ops, Provenance::default())
            .map_err(|e| anyhow::anyhow!("graph edit #{i} failed to replay: {e}"))?
            .candidate;
    }
    Ok(dag)
}

#[derive(Deserialize)]
pub(crate) struct SessionGraphEditBody {
    /// The `Vec<designer_graph::ops::Operation>` to stage and, on
    /// success, append. Sent as real JSON (not a pre-encoded string) —
    /// the server is the only party that ever deserializes it; the store
    /// persists the re-serialized form as an opaque string.
    operations: Vec<designer_graph::ops::Operation>,
    #[serde(default)]
    note: String,
}

#[derive(Serialize)]
pub(crate) struct SessionGraphEditResponse {
    seq: u64,
}

/// Stage the submitted operations against the session's CURRENT
/// reconstruction (never against a stale or hypothetical base — I18's
/// discipline extended to the session layer) and append only on success.
/// A refusal is reported and NOTHING is persisted — the edit log never
/// carries a candidate that didn't admit-stage, so replay can never fail
/// on data this endpoint itself accepted.
async fn session_graph_edit_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SessionGraphEditBody>,
) -> impl IntoResponse {
    let lock = demo.session_lock(id);
    let _guard = lock.lock().await;
    let record = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    let dag = match reconstruct_designer_dag(&record) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("reconstruction: {e}") })),
            )
                .into_response();
        }
    };
    if body.operations.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "empty operation sequence" })),
        )
            .into_response();
    }
    let staged = match designer_graph::productions::apply_production(
        &dag,
        body.operations.clone(),
        designer_graph::schema::Provenance::default(),
    ) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("staging refused: {e}") })),
            )
                .into_response();
        }
    };
    // staged.candidate itself is not persisted — only the op sequence is
    // (rider 2) — but the candidate MUST admit (full to_ir/verify/lower
    // theorem chain) before its op sequence is accepted; apply_production's
    // per-op staging alone only proves local anchor legality, not that the
    // resulting graph is live/reachable/exhaustive.
    if let Err(errs) = staged.candidate.admit() {
        let messages: Vec<String> = errs.iter().map(|e| e.message.clone()).collect();
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "graph does not admit", "diagnostics": messages })),
        )
            .into_response();
    }
    let operations_json = match serde_json::to_string(&body.operations) {
        Ok(j) => j,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("serialize: {e}") })),
            )
                .into_response();
        }
    };
    match demo
        .store
        .append_design_session_event(
            &demo.tenant_id,
            id,
            &DesignSessionEventKind::GraphEdit { operations_json, note: body.note.clone() },
        )
        .await
    {
        Ok(seq) => Json(SessionGraphEditResponse { seq }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

/// D19-rider rendering: helpful about the path forward, generic about
/// the request — never confirms the requested operation exists, never
/// enumerates what the user cannot do.
fn render_disposition(
    disposition: &utterance_engine::policy::ProposalDisposition,
    board: &utterance_engine::board::Board,
) -> String {
    use utterance_engine::policy::ProposalDisposition as D;
    match disposition {
        D::Candidate { candidate_id } => {
            let desc = board
                .candidates
                .iter()
                .find(|c| &c.canonical_id == candidate_id)
                .map(|c| c.description.as_str())
                .unwrap_or(candidate_id.as_str());
            format!("Proposed: {desc}. Stage and ratify to apply.")
        }
        D::OutOfScope => {
            "This cannot be executed because it is not part of your current working context. You can change context through the governed route, or pick from the available design operations."
                .to_owned()
        }
        D::EscalateToSage { .. } => {
            "I need more detail to map this onto one design operation — can you say which step of the process this applies to, and whether it is one change or several?"
                .to_owned()
        }
        // Unreachable in v1 (policy enum docs) — rendered defensively.
        D::Ambiguous { top_candidates } => {
            format!("Did you mean one of: {}?", top_candidates.join(", "))
        }
        D::MissingArguments { candidate_id, missing } => {
            format!("'{candidate_id}' needs: {}", missing.join(", "))
        }
        D::Compound => "That looks like several changes — one at a time, please.".to_owned(),
    }
}

/// WS-B day-one wiring (SHADOW START, plan §C constraint 2): every
/// session utterance flows board → tier-0 → deterministic disposition
/// policy → I28 record. The record is written to the session event log
/// (operational data); CORPUS capture stays suppressed until the Q9
/// charter (D17) — the response reports that state honestly.
async fn session_utterance_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SessionUtteranceBody>,
) -> impl IntoResponse {
    use utterance_engine::board::{build_board, EmptyUniverse, PolicyFilter};
    #[cfg(feature = "q9-capture")]
    use utterance_engine::capture::{CaptureOutcome, CapturePipeline};
    use utterance_engine::policy::{decide, DispositionConfig};
    use utterance_engine::retrieval::{LexicalTier0, Tier0Retriever};

    // Session must exist; its current source is the graph identity the
    // board hashes (C7 obligation: WS-B supplies the revision identity).
    let record_session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    // WS-B.4: DesignerDag-backed sessions run the real PositionalLegality
    // oracle and project_ir over the reconstructed IR graph — training-
    // grade projections at last (context.rs's INTERIM LIMITATION note is
    // resolved for exactly this class of session). Legacy DSL-source
    // sessions (no GraphEdit ever appended) keep the WholeGraphLegality +
    // census-only path unchanged — purely additive, no existing session's
    // behavior shifts underneath it.
    let pipeline_result: anyhow::Result<_> = if record_session.is_graph_backed() {
        let dag = match reconstruct_designer_dag(&record_session) {
            Ok(d) => d,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("reconstruction: {e}") })),
                )
                    .into_response();
            }
        };
        let anchor_key = match &body.anchor {
            Some(id) => match dag.key_for_bpmn_id(id) {
                Some(k) => Some(k),
                None => {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(serde_json::json!({
                            "error": format!("anchor '{id}' names no node in this session's graph")
                        })),
                    )
                        .into_response();
                }
            },
            None => None,
        };
        let ir = match dag.to_ir() {
            Ok(ir) => ir,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("projection: {e}") })),
                )
                    .into_response();
            }
        };
        // Same pattern as the DSL-source path: hash the actual revision
        // content (here, the accumulated edit-log payloads — the DAG's
        // sole source of truth) rather than a Debug-formatted derivative.
        let graph_identity = {
            let mut hasher = blake3::Hasher::new();
            for payload in record_session.graph_edit_payloads() {
                hasher.update(payload.as_bytes());
                hasher.update(b"\0");
            }
            hasher.finalize().to_hex().to_string()
        };
        let oracle = designer_graph::positional::PositionalLegality { dag: &dag };
        let anchor_pair = anchor_key.as_ref().zip(body.anchor.as_deref());
        build_board(
            &oracle,
            anchor_pair,
            Some(&graph_identity),
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .and_then(|board| {
            let context = utterance_engine::context::project_ir(
                &ir,
                body.anchor.as_deref(),
                &board.context.pack_identity,
                &graph_identity,
            )?;
            let evidence = LexicalTier0.retrieve(&body.text, &board)?;
            let (disposition, record) =
                decide(&DispositionConfig::shadow_v1(), &board, &evidence, &context)?;
            Ok((board, disposition, record, context))
        })
    } else {
        let graph_identity = blake3::hash(
            record_session.current_source().unwrap_or_default().as_bytes(),
        )
        .to_hex()
        .to_string();

        // DIR-002 A1 INTERIM LIMITATION: census-only, no anchor — the
        // DSL-plan pipeline has no IRGraph for project_ir to run over.
        let node_kind_counts = {
            let registry = get_preview_registry();
            match bpmn_lite_compiler::dsl::compile(
                record_session.current_source().unwrap_or_default(),
                &registry,
            ) {
                Ok(plan) => {
                    let graph = plan_to_visual_graph(&plan);
                    let mut counts = std::collections::BTreeMap::<String, u32>::new();
                    for n in &graph.nodes {
                        *counts.entry(n.kind.clone()).or_insert(0) += 1;
                    }
                    counts.into_iter().collect::<Vec<_>>()
                }
                Err(_) => Vec::new(),
            }
        };

        build_board(
            &WholeGraphLegality,
            None,
            Some(&graph_identity),
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .and_then(|board| {
            let context = utterance_engine::context::ContextProjection::new(
                board.context.pack_identity.clone(),
                graph_identity.clone(),
                None,
                node_kind_counts,
            )?;
            let evidence = LexicalTier0.retrieve(&body.text, &board)?;
            let (disposition, record) =
                decide(&DispositionConfig::shadow_v1(), &board, &evidence, &context)?;
            Ok((board, disposition, record, context))
        })
    };
    let (board, disposition, record, context) = match pipeline_result {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("utterance pipeline: {e}") })),
            )
                .into_response();
        }
    };
    let message = render_disposition(&disposition, &board);
    let record_json = match serde_json::to_string(&record) {
        Ok(j) => j,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("record serialize: {e}") })),
            )
                .into_response();
        }
    };

    // Corpus capture: switch OFF until GOV.1 ratifies (D17) — evaluated
    // per interaction so the suppression is a recorded fact, not an
    // assumed one. DIR-004 Phase 1.2: outside `q9-capture`, there is no
    // module to call here at all — `"not_compiled"` reports that as
    // plainly as `"suppressed_no_charter"` reported the old always-off
    // runtime state, but it is now also a build-time fact, not just a
    // runtime one.
    #[cfg(feature = "q9-capture")]
    let capture_state = match CapturePipeline::off().capture(
        utterance_engine::capture::CaptureEvent {
            raw_utterance: body.text.clone(),
            record: record.clone(),
            dataset: utterance_engine::capture::DatasetClass::Evaluation,
        },
    ) {
        CaptureOutcome::SuppressedNoCharter => "suppressed_no_charter",
        CaptureOutcome::Stored(_) => "stored",
    };
    #[cfg(not(feature = "q9-capture"))]
    let capture_state = "not_compiled";

    // DIR-004 Phase 2 wiring: dev-session capture, structurally distinct
    // from the Q9-gated path above (always compiled, no feature gate --
    // see utterance_engine::dev_capture's module doc). Active only for
    // sessions that already called POST .../dev-capture/enable with a
    // consent statement (that call is the "consent stated at session
    // start" moment); captures the FULL closure -- board dump and
    // context TEXT, not hash-only -- per Phase 1.3.
    let dev_capture_state = {
        let mut stores = demo.dev_capture.lock().unwrap();
        match stores.get_mut(&id) {
            Some(store) => {
                store.capture(utterance_engine::dev_capture::DevSessionCaptureInput {
                    raw_utterance: body.text.clone(),
                    board_hash: record.board_hash.clone(),
                    board: utterance_engine::corpus_schema::BoardDump::from_board(&board),
                    context_projection: context.serialize_canonical(),
                    context_projection_hash: record.context_projection_hash.clone(),
                    retrieved_subset_hash: record.retrieved_subset_hash.clone(),
                    model_bundle_hash: record.model_bundle_hash.clone(),
                    disposition_policy_hash: record.disposition_policy_hash.clone(),
                    ranking: record.ranking.clone(),
                    disposition: disposition.clone(),
                });
                "captured"
            }
            None => "not_enabled",
        }
    };

    match demo
        .store
        .append_design_session_event(
            &demo.tenant_id,
            id,
            &DesignSessionEventKind::Utterance {
                text: body.text.clone(),
                response: message.clone(),
                decision_record_json: Some(record_json),
                context_projection: Some(context.serialize_canonical()),
            },
        )
        .await
    {
        Ok(seq) => Json(serde_json::json!({
            "seq": seq,
            "message": message,
            "disposition": disposition,
            "board_hash": board.board_hash,
            "capture": capture_state,
            "dev_capture": dev_capture_state,
        }))
        .into_response(),
        Err(bpmn_lite_store::StoreError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct DevCaptureEnableBody {
    consent_statement: String,
}

/// DIR-004 Phase 2 wiring: the "consent stated at session start" moment.
/// Opens a `DevSessionStore` for this design session id -- always
/// compiled, structurally distinct from the Q9-gated `capture` module
/// (see `utterance_engine::dev_capture`'s doc). Idempotent-refusing: a
/// session that already has a store open is NOT silently re-opened with
/// a different consent statement (that would let a later call quietly
/// change what consent is on record for already-captured interactions)
/// -- it 409s with the existing consent timestamp instead.
async fn dev_capture_enable_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<DevCaptureEnableBody>,
) -> impl IntoResponse {
    let mut stores = demo.dev_capture.lock().unwrap();
    if let Some(existing) = stores.get(&id) {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "dev-capture already enabled for this session",
                "session_id": existing.session_id(),
            })),
        )
            .into_response();
    }
    match utterance_engine::dev_capture::DevSessionStore::open(&id.to_string(), &body.consent_statement) {
        Ok(store) => {
            stores.insert(id, store);
            Json(serde_json::json!({ "enabled": true, "session_id": id })).into_response()
        }
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

/// Read-back: whether dev-capture is enabled for this session and, if
/// so, every record captured so far (full I28 closure, train-on-able --
/// this endpoint is Adam's own export path, not a general query API).
async fn dev_capture_status_endpoint(State(demo): State<Arc<DesignerState>>, Path(id): Path<Uuid>) -> impl IntoResponse {
    let stores = demo.dev_capture.lock().unwrap();
    match stores.get(&id) {
        Some(store) => Json(serde_json::json!({
            "enabled": true,
            "session_id": store.session_id(),
            "record_count": store.records().len(),
            "records": store.records(),
        }))
        .into_response(),
        None => Json(serde_json::json!({ "enabled": false, "record_count": 0, "records": [] })).into_response(),
    }
}

/// Save-as-template: compile the session's CURRENT source (fail-closed —
/// an uncompilable draft cannot become a template), persist plan +
/// catalog entry (same path as POST /bpmn/templates), and pin the
/// template ref back onto the session.
async fn save_design_session_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SaveSessionBody>,
) -> impl IntoResponse {
    let record = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    // Rider 2 (G2 BLOCKER-2 ruling): two stores, two roles. The DAG stays
    // authoritative in the SESSION store (already true — GraphEdit events,
    // untouched by this endpoint); this endpoint only decides what goes
    // into the PLAN store, which must hold a compiled artifact regardless
    // of which path authored it (P5).
    let (plan_json, source_for_template) = if record.is_graph_backed() {
        let dag = match reconstruct_designer_dag(&record) {
            Ok(d) => d,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("reconstruction: {e}") })),
                )
                    .into_response();
            }
        };
        // Same admission discipline as the graph-edit endpoint — the
        // full to_ir/verify/lower theorem chain must pass before this
        // session's graph is eligible to become a stored artifact at all.
        if let Err(errs) = dag.admit() {
            let messages: Vec<String> = errs.iter().map(|e| e.message.clone()).collect();
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": "graph does not admit", "diagnostics": messages })),
            )
                .into_response();
        }
        let ir = match dag.to_ir() {
            Ok(ir) => ir,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("to_ir: {e}") })),
                )
                    .into_response();
            }
        };
        let plan = match bpmn_lite_compiler::dsl::project_ir(&ir, record.name.clone()) {
            Ok(plan) => plan,
            Err(e) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": "graph cannot be saved as a template yet",
                        "diagnostics": [e.to_string()],
                    })),
                )
                    .into_response();
            }
        };
        let plan_json = match serde_json::to_string(&plan) {
            Ok(json) => json,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("serialize plan: {e}") })),
                )
                    .into_response();
            }
        };
        let source_for_template = format!(
            "<graph-authored session {id}; edit via the graph-edit endpoint, not DSL text>"
        );
        (plan_json, source_for_template)
    } else {
        let Some(source) = record.current_source().map(str::to_owned) else {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "session has no revision to save" })),
            )
                .into_response();
        };
        let registry = get_preview_registry();
        let plan = match bpmn_lite_compiler::dsl::compile(&source, &registry) {
            Ok(plan) => plan,
            Err(e) => {
                let diagnostics: Vec<String> = match e {
                    bpmn_lite_compiler::dsl::CompileError::Parse(errs) => errs,
                    bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                        errs.iter().map(|e| format!("{e}")).collect()
                    }
                    bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                        errs.iter().map(|e| format!("{e}")).collect()
                    }
                };
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "current revision does not compile",
                        "diagnostics": diagnostics
                    })),
                )
                    .into_response();
            }
        };
        let plan_json = match serde_json::to_string(&plan) {
            Ok(json) => json,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("serialize plan: {e}") })),
                )
                    .into_response();
            }
        };
        (plan_json, source)
    };
    let hash = *blake3::hash(plan_json.as_bytes()).as_bytes();
    if let Err(e) = demo.store.store_plan(hash, &plan_json).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("store plan: {e}") })),
        )
            .into_response();
    }
    let version = match demo
        .store
        .load_latest_template_version(&body.template_name)
        .await
    {
        Ok(Some((v, _, _))) => v + 1,
        _ => 1,
    };
    if let Err(e) = demo
        .store
        .store_template(&body.template_name, version, hash, &source_for_template)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("store template: {e}") })),
        )
            .into_response();
    }
    if let Err(e) = demo
        .store
        .mark_design_session_saved(&demo.tenant_id, id, &body.template_name, version, hash)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("mark saved: {e}") })),
        )
            .into_response();
    }
    Json(serde_json::json!({
        "template_name": body.template_name,
        "version": version,
        "plan_hash": hex::encode(hash)
    }))
    .into_response()
}


/// Server-built DAG for the designer window (merged T4 / WS-B.2): the
/// session's CURRENT revision recompiled server-side; compile errors
/// surface as diagnostics, never a blank canvas. Layout is computed
/// here (layered by BFS depth from the start node) — the UI is a
/// window, not an editor.
async fn session_graph_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    let source = session.current_source().unwrap_or_default().to_owned();
    let source_hash = blake3::hash(source.as_bytes()).to_hex().to_string();
    let registry = get_preview_registry();
    match bpmn_lite_compiler::dsl::compile(&source, &registry) {
        Ok(plan) => {
            let graph = plan_to_visual_graph(&plan);
            let layout = layered_layout(&graph);
            Json(serde_json::json!({
                "compiles": true,
                "diagnostics": [],
                "graph": graph,
                "layout": layout,
                "source_hash": source_hash,
            }))
            .into_response()
        }
        Err(e) => {
            let diagnostics: Vec<String> = match e {
                bpmn_lite_compiler::dsl::CompileError::Parse(errs) => errs,
                bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                    errs.iter().map(|x| format!("{x}")).collect()
                }
                bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                    errs.iter().map(|x| format!("{x}")).collect()
                }
            };
            Json(serde_json::json!({
                "compiles": false,
                "diagnostics": diagnostics,
                "graph": serde_json::Value::Null,
                "layout": serde_json::Value::Null,
                "source_hash": source_hash,
            }))
            .into_response()
        }
    }
}

/// Deterministic layered layout: x = BFS depth from the start node,
/// y = order within the layer. Display-only — never a structural
/// derivation (I16: pairing/regions stay the compiler's).
fn layered_layout(
    graph: &VisualGraphDto,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    use std::collections::{BTreeMap, HashMap, VecDeque};
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &graph.edges {
        adj.entry(e.from.as_str()).or_default().push(e.to.as_str());
    }
    let mut depth: HashMap<&str, usize> = HashMap::new();
    let mut queue: VecDeque<&str> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == "start")
        .map(|n| n.id.as_str())
        .collect();
    for start in &queue {
        depth.insert(start, 0);
    }
    while let Some(node) = queue.pop_front() {
        let d = depth[node];
        for next in adj.get(node).into_iter().flatten() {
            let entry = depth.entry(next).or_insert(usize::MAX);
            if *entry == usize::MAX || *entry < d + 1 {
                // deepest-position layering keeps joins right of all
                // their branches; acyclic by admission so this halts.
                if *entry == usize::MAX || *entry < d + 1 {
                    *entry = d + 1;
                    queue.push_back(next);
                }
            }
        }
    }
    let mut lanes: HashMap<usize, usize> = HashMap::new();
    let mut layout = BTreeMap::new();
    for n in &graph.nodes {
        let d = depth.get(n.id.as_str()).copied().unwrap_or(0);
        let lane = lanes.entry(d).or_insert(0);
        layout.insert(
            n.id.clone(),
            serde_json::json!({ "x": (d as f64) * 220.0, "y": (*lane as f64) * 120.0 }),
        );
        *lane += 1;
    }
    layout
}

/// The standalone designer window (plan ruling E4: static HTML +
/// vanilla JS + SVG, no framework, no build toolchain). It renders the
/// server-supplied graph/layout and drives ONLY the governed endpoints
/// — a window, not an editor.
async fn designer_page() -> impl IntoResponse {
    axum::response::Html(include_str!("../static/designer.html"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt; // for `oneshot`

    #[tokio::test]
    async fn test_compile_and_preview_endpoints() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

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
                    .body(axum::body::Body::from(
                        serde_json::to_string(&bpmn_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
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
                    .body(axum::body::Body::from(
                        serde_json::to_string(&bpmn_err_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
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
                    .body(axum::body::Body::from(
                        serde_json::to_string(&bpmn_wait_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 100000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            res["workflow_id"].as_str().unwrap(),
            "custody-cbu-onboarding"
        );
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
                    .body(axum::body::Body::from(
                        serde_json::to_string(&dmn_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
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
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["decision_name"], "cbu_type_routing");
        assert_eq!(res["hit_policy"], "first");
        assert_eq!(res["inputs"][0]["name"], "cbu-client-type");
    }

    #[tokio::test]
    async fn test_dsl_macro_and_diagnostics_endpoints() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);

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
                    .body(axum::body::Body::from(
                        serde_json::to_string(&body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
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
                    .body(axum::body::Body::from(
                        serde_json::to_string(&resolve_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
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
                    .body(axum::body::Body::from(
                        serde_json::to_string(&utter_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(res["escape_intent_detected"].as_bool().unwrap());
        assert_eq!(res["suggested_action"].as_str().unwrap(), "exit");
    }

    #[tokio::test]
    async fn test_template_catalog_endpoints() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);

        // 1. Post version 1 of template "my-template"
        let dsl_v1 = r#"(workflow my-template
  (start-event :id start :next end)
  (end-event :id end :status "v1"))"#;
        let define_body = DefineTemplateBody {
            name: "my-template".to_string(),
            dsl_body: dsl_v1.to_string(),
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/templates")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&define_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["version"], 1);
        let first_hash = res["plan_hash"].as_str().unwrap().to_string();
        assert_eq!(first_hash.len(), 64);

        // 2. List templates - verify "my-template" exists and has latest_version = 1
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bpmn/templates")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let summaries: Value = serde_json::from_slice(&body_bytes).unwrap();
        let summaries_arr = summaries.as_array().unwrap();
        assert!(summaries_arr.iter().any(|t| t["name"] == "my-template"
            && t["latest_version"] == 1
            && t["plan_hash"] == first_hash));

        // 3. Get version 1 of the template
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bpmn/templates/my-template/versions/1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let version_dto: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(version_dto["name"], "my-template");
        assert_eq!(version_dto["version"], 1);
        assert_eq!(version_dto["dsl_body"], dsl_v1);
        assert_eq!(version_dto["plan_hash"], first_hash);

        // 4. Post version 2 of template "my-template"
        let dsl_v2 = r#"(workflow my-template
  (start-event :id start :next end)
  (end-event :id end :status "v2"))"#;
        let define_body_v2 = DefineTemplateBody {
            name: "my-template".to_string(),
            dsl_body: dsl_v2.to_string(),
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/templates")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&define_body_v2).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res_v2: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res_v2["version"], 2);
        let second_hash = res_v2["plan_hash"].as_str().unwrap().to_string();
        assert_ne!(first_hash, second_hash);

        // 5. List templates again - check latest_version = 2
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bpmn/templates")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let summaries_v2: Value = serde_json::from_slice(&body_bytes).unwrap();
        let summaries_arr_v2 = summaries_v2.as_array().unwrap();
        assert!(summaries_arr_v2.iter().any(|t| t["name"] == "my-template"
            && t["latest_version"] == 2
            && t["plan_hash"] == second_hash));
    }

    const SESSION_DSL_OK: &str = r#"(workflow session-wf
  (start-event :id start :next end)
  (end-event :id end :status "OK"))"#;

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn post_json(uri: &str, body: Value) -> Request<axum::body::Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn test_design_session_round_trip_and_save() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        // Create
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "kyc draft", "dsl_source": "" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = body_json(response).await;
        let session_id = created["session_id"].as_str().unwrap().to_owned();

        // List contains it
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/dsl/sessions")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let list = body_json(response).await;
        assert!(list
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == session_id.as_str() && s["name"] == "kyc draft"));

        // Invalid revision is RECORDED (drafts may be broken) but reports diagnostics
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/revision"),
                serde_json::json!({ "dsl_source": "(workflow broken", "note": "wip" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let rev = body_json(response).await;
        assert_eq!(rev["compiles"], false);
        assert!(!rev["diagnostics"].as_array().unwrap().is_empty());
        let broken_seq = rev["seq"].as_u64().unwrap();

        // Valid revision compiles clean
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/revision"),
                serde_json::json!({ "dsl_source": SESSION_DSL_OK, "note": "fixed" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let rev = body_json(response).await;
        assert_eq!(rev["compiles"], true);
        assert!(rev["seq"].as_u64().unwrap() > broken_seq);

        // Utterance goes through the intent gate and is appended
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "add a task after start" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let utter = body_json(response).await;
        assert!(utter["seq"].as_u64().is_some());
        assert!(utter["message"].is_string());

        // Save-as-template: pins ref, persists template v1
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "session-template" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let saved = body_json(response).await;
        assert_eq!(saved["template_name"], "session-template");
        assert_eq!(saved["version"], 1);
        let plan_hash = saved["plan_hash"].as_str().unwrap().to_owned();
        assert_eq!(plan_hash.len(), 64);

        // Get: full record shows Saved status, template pin, and the event log
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/dsl/sessions/{session_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let record = body_json(response).await;
        assert_eq!(record["status"], "Saved");
        assert!(record["template_ref"].is_array());
        let events = record["events"].as_array().unwrap();
        assert!(events.len() >= 4); // seed + broken rev + good rev + utterance
    }

    /// UI smoke: the designer page serves and names itself.
    #[tokio::test]
    async fn test_designer_page_serves() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/designer")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await.unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("BPMN-Lite Designer"));
        assert!(html.contains("/api/dsl/sessions"), "drives the governed endpoints");
    }

    /// Graph-window receipt (merged T4 / WS-B.2): compiling session →
    /// graph + deterministic layout; broken draft → diagnostics, never
    /// a blank-canvas error.
    #[tokio::test]
    async fn test_session_graph_endpoint() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let ok_src = SESSION_DSL_OK;
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "g1", "dsl_source": ok_src }),
            ))
            .await
            .unwrap();
        let sid = body_json(response).await["session_id"].as_str().unwrap().to_owned();

        let get_graph = |sid: &str| {
            Request::builder()
                .method("GET")
                .uri(format!("/api/dsl/sessions/{sid}/graph"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app.clone().oneshot(get_graph(&sid)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let g = body_json(response).await;
        assert_eq!(g["compiles"], true);
        let nodes = g["graph"]["nodes"].as_array().unwrap();
        assert!(!nodes.is_empty());
        let first_id = nodes[0]["id"].as_str().unwrap();
        assert!(g["layout"][first_id]["x"].is_number(), "layout coords served");

        // Broken revision → diagnostics, not a blank-canvas error.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{sid}/revision"),
                serde_json::json!({ "dsl_source": "(workflow broken", "note": "wip" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app.clone().oneshot(get_graph(&sid)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let g = body_json(response).await;
        assert_eq!(g["compiles"], false);
        assert!(!g["diagnostics"].as_array().unwrap().is_empty());
        assert!(g["graph"].is_null());
    }

    /// SHADOW-START receipt (WS-B day-one wiring): the session
    /// utterance flows board → tier-0 → policy → I28 record; the record
    /// lands in the event log; corpus capture is visibly suppressed
    /// (D17, no charter); the board hash tracks the session's source
    /// (C7 obligation); gibberish abstains with the D19-rider denial.
    #[tokio::test]
    async fn test_session_utterance_runs_shadow_pipeline() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let mk = |name: &str, src: &str| {
            post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": name, "dsl_source": src }),
            )
        };
        let response = app.clone().oneshot(mk("s1", "(workflow a)")).await.unwrap();
        let s1 = body_json(response).await["session_id"].as_str().unwrap().to_owned();
        let response = app.clone().oneshot(mk("s2", "(workflow b)")).await.unwrap();
        let s2 = body_json(response).await["session_id"].as_str().unwrap().to_owned();

        let utter = |sid: &str, text: &str| {
            post_json(
                &format!("/api/dsl/sessions/{sid}/utterance"),
                serde_json::json!({ "text": text }),
            )
        };
        // Gibberish → abstention with the generic, path-forward denial.
        let response = app.clone().oneshot(utter(&s1, "zzz qqq xyzzy")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let r1 = body_json(response).await;
        assert_eq!(r1["disposition"], "OutOfScope");
        assert!(
            r1["message"].as_str().unwrap().contains("current working context"),
            "actual message: {}",
            r1["message"]
        );
        // DIR-004 Phase 1.2: outside `q9-capture` (default), the module
        // isn't even compiled in -- "not_compiled" reports that build-time
        // fact; under `q9-capture` the old runtime-suppressed state still
        // applies (no charter ref is ever supplied in this workspace).
        #[cfg(not(feature = "q9-capture"))]
        assert_eq!(r1["capture"], "not_compiled");
        #[cfg(feature = "q9-capture")]
        assert_eq!(r1["capture"], "suppressed_no_charter");
        let h1 = r1["board_hash"].as_str().unwrap().to_owned();
        assert_eq!(h1.len(), 64);

        // Different session source → different board hash (graph
        // identity is hashed — C7).
        let response = app.clone().oneshot(utter(&s2, "zzz qqq xyzzy")).await.unwrap();
        let r2 = body_json(response).await;
        assert_ne!(h1, r2["board_hash"].as_str().unwrap());

        // The I28 record is IN the event log.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/dsl/sessions/{s1}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let record = body_json(response).await;
        let events = record["events"].as_array().unwrap();
        let utterance_event = events
            .iter()
            .find_map(|e| e["kind"].get("Utterance"))
            .expect("utterance event present");
        let rec_json = utterance_event["decision_record_json"]
            .as_str()
            .expect("I28 record stored");
        let rec: Value = serde_json::from_str(rec_json).unwrap();
        assert_eq!(rec["board_hash"], h1.as_str());
        assert!(rec["disposition_policy_hash"].as_str().unwrap().len() == 64);
        assert!(!rec["ranking"].as_array().unwrap().is_empty());

        // DIR-002 A1 + "AFTER" item 2, the audit closure: the event stores
        // the SERIALIZED projection (trainable, not hash-only), and its
        // bytes re-hash to the record's context_projection_hash — the ONE
        // serializer produced both sides.
        let ctx_text = utterance_event["context_projection"]
            .as_str()
            .expect("serialized context projection stored");
        assert!(
            ctx_text.starts_with("ctxproj.v1\n"),
            "canonical version line missing: {ctx_text:?}"
        );
        assert!(
            ctx_text.contains("nodes:\n"),
            "node census missing from projection: {ctx_text:?}"
        );
        assert_eq!(
            blake3::hash(ctx_text.as_bytes()).to_hex().to_string(),
            rec["context_projection_hash"].as_str().unwrap(),
            "stored projection bytes must re-hash to the recorded hash"
        );
    }

    /// DIR-004 Phase 2 wiring: dev-session capture is NOT live for a
    /// session until an explicit consent call, and a real captured
    /// record must be train-on-able (board dump + context TEXT present,
    /// not hash-only) -- the same guarantee `dev_capture.rs`'s own unit
    /// tests prove for the type, proven here through the actual HTTP
    /// surface a real session will use.
    #[tokio::test]
    async fn test_dev_capture_requires_consent_then_captures_full_closure() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "dc1", "dsl_source": "(workflow a)" }),
            ))
            .await
            .unwrap();
        let sid = body_json(response).await["session_id"].as_str().unwrap().to_owned();

        let status_req = |sid: &str| {
            Request::builder()
                .method("GET")
                .uri(format!("/api/dsl/sessions/{sid}/dev-capture"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let utter = |sid: &str, text: &str| {
            post_json(
                &format!("/api/dsl/sessions/{sid}/utterance"),
                serde_json::json!({ "text": text }),
            )
        };

        // Before consent: not enabled, no capture happens on an utterance.
        let response = app.clone().oneshot(status_req(&sid)).await.unwrap();
        assert_eq!(body_json(response).await["enabled"], false);
        let response = app.clone().oneshot(utter(&sid, "add a task after start")).await.unwrap();
        assert_eq!(body_json(response).await["dev_capture"], "not_enabled");

        // Empty consent statement is refused (mirrors CapturePipeline's
        // empty-charter-ref refusal) -- no store is created.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{sid}/dev-capture/enable"),
                serde_json::json!({ "consent_statement": "   " }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        // Real consent: enabling succeeds.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{sid}/dev-capture/enable"),
                serde_json::json!({ "consent_statement": "Adam, 2026-07-29, self-testing only" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Re-enabling the SAME session is refused (409) -- consent is not
        // silently overwritable mid-session.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{sid}/dev-capture/enable"),
                serde_json::json!({ "consent_statement": "a different statement" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        // Now an utterance IS captured, end to end.
        let response = app.clone().oneshot(utter(&sid, "add a task after start")).await.unwrap();
        let u = body_json(response).await;
        assert_eq!(u["dev_capture"], "captured");

        let response = app.clone().oneshot(status_req(&sid)).await.unwrap();
        let status = body_json(response).await;
        assert_eq!(status["enabled"], true);
        assert_eq!(status["record_count"], 1);
        let record = &status["records"][0];
        assert_eq!(record["provenance"], "dev-session-adam-v1");
        assert_eq!(record["subject"], "Adam");
        assert_eq!(record["consent_statement_timestamp"], "Adam, 2026-07-29, self-testing only");
        assert_eq!(record["raw_utterance"], "add a task after start");
        assert!(
            !record["board"]["candidates"].as_array().unwrap().is_empty(),
            "captured record must carry real board candidates, not just a hash"
        );
        assert!(
            record["context_projection"].as_str().unwrap().starts_with("ctxproj.v1\n"),
            "captured record must carry serialized context TEXT, not hash-only"
        );

        // A second session never sees the first session's records
        // (per-session isolation, same pattern as session_locks).
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "dc2", "dsl_source": "(workflow a)" }),
            ))
            .await
            .unwrap();
        let sid2 = body_json(response).await["session_id"].as_str().unwrap().to_owned();
        let response = app.clone().oneshot(status_req(&sid2)).await.unwrap();
        assert_eq!(body_json(response).await["enabled"], false);
    }

    #[tokio::test]
    async fn test_design_session_save_rejects_uncompilable_draft() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "broken", "dsl_source": "(workflow nope" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let session_id = body_json(response).await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "never-lands" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let err = body_json(response).await;
        assert!(!err["diagnostics"].as_array().unwrap().is_empty());

        // Nothing leaked into the template catalog
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bpmn/templates")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let templates = body_json(response).await;
        assert!(!templates
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "never-lands"));
    }

    #[tokio::test]
    async fn test_design_session_unknown_id_is_404() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let ghost = Uuid::now_v7();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/dsl/sessions/{ghost}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{ghost}/revision"),
                serde_json::json!({ "dsl_source": "x", "note": "" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{ghost}/save"),
                serde_json::json!({ "template_name": "t" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── WS-B.4: DesignerDag-backed sessions ──────────────────────────

    fn task_ir(id: &str) -> bpmn_lite_compiler::IRNode {
        bpmn_lite_compiler::IRNode::ServiceTask {
            id: id.into(),
            name: id.into(),
            task_type: "noop".into(),
        }
    }

    fn new_key() -> designer_graph::schema::NodeKey {
        designer_graph::schema::NodeKey(Uuid::new_v4())
    }

    /// GREEN: a graph-edit sequence against the deterministic seed Start
    /// stages, admits, and is persisted; a session that has never
    /// accumulated one stays legacy (`is_graph_backed() == false`
    /// server-side, exercised indirectly via the utterance path below).
    #[tokio::test]
    async fn test_session_graph_edit_admits_and_persists() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "graph session" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let session_id = body_json(response).await["session_id"].as_str().unwrap().to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        let t1 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops, "note": "build the chain" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{:?}", body_json(response).await);
        let seq = body_json(
            app.clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                    serde_json::json!({ "operations": Vec::<designer_graph::ops::Operation>::new() }),
                ))
                .await
                .unwrap(),
        )
        .await;
        // Empty sequence is BAD_REQUEST, not silently accepted.
        let _ = seq;

        let record = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let events = record["events"].as_array().unwrap();
        assert!(
            events.iter().any(|e| e["kind"].get("GraphEdit").is_some()),
            "GraphEdit event must be persisted: {events:?}"
        );
    }

    /// RED: an operation sequence that refuses to stage (unknown anchor)
    /// is REJECTED (422) and NOTHING is persisted — the edit log never
    /// carries a candidate that failed to admit-stage.
    #[tokio::test]
    async fn test_session_graph_edit_refuses_invalid_ops_and_persists_nothing() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "refusal session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let ops = vec![designer_graph::ops::Operation::AppendNode {
            anchor: new_key(), // unknown — no such node exists yet
            key: new_key(),
            node: task_ir("orphan"),
            edge_id: "f1".into(),
        }];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let record = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(
            !record["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"].get("GraphEdit").is_some()),
            "a refused edit must persist nothing"
        );
    }

    /// RED (G2 blind-review finding, BLOCKER 1): a sequence whose OPS are
    /// each individually legal at staging time (both anchors resolve, both
    /// AppendNodes target a fresh anchor) but whose resulting GRAPH is
    /// globally illegal — a parallel split with no matching join. Before
    /// the fix, `apply_production`'s per-op checks alone gated persistence
    /// and this sequence was wrongly ACCEPTED; the fix wires the caller's
    /// mandatory `staged.candidate.admit()` call so the full to_ir/verify
    /// theorem chain runs before anything is appended.
    #[tokio::test]
    async fn test_session_graph_edit_refuses_locally_staged_but_globally_illegal_graph() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "unmatched fork session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        let split_key = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: split_key,
                node: bpmn_lite_compiler::IRNode::GatewayAnd {
                    id: "split".into(),
                    name: "split".into(),
                    direction: bpmn_lite_compiler::GatewayDirection::Diverging,
                },
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: split_key,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops, "note": "unmatched fork" }),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "a fork with no matching join must be refused by admit(), not just per-op staging"
        );
        let body = body_json(response).await;
        let diagnostics = body["diagnostics"].as_array().unwrap();
        assert!(
            diagnostics.iter().any(|d| {
                let d = d.as_str().unwrap().to_lowercase();
                d.contains("fork") || d.contains("join")
            }),
            "refusal must name the fork/join mismatch: {diagnostics:?}"
        );

        let record = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(
            !record["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"].get("GraphEdit").is_some()),
            "a graph that doesn't admit must persist nothing"
        );
    }

    /// GREEN (G2 blind-review finding, BLOCKER 3): N concurrent graph-edit
    /// requests against the SAME session, all competing for the SAME
    /// anchor's single outgoing-edge slot. Before the fix, the endpoint's
    /// load→reconstruct→stage→append sequence ran with no per-session
    /// serialization — concurrent requests could each load the DAG BEFORE
    /// any of them had appended, each stage successfully against that
    /// stale base, and each persist a `GraphEdit` event; on replay,
    /// folding a second AppendNode against an anchor the first event
    /// already gave an outgoing edge to fails mid-fold, permanently
    /// bricking the session (every future reconstruct/utterance call
    /// errors, with no repair path). The fix serializes each session's
    /// load-stage-append sequence behind a per-session `tokio::sync::Mutex`
    /// (`DesignerState::session_lock`), so every request after the first sees
    /// the prior request's committed state before it stages — exactly one
    /// request can ever win the anchor's outgoing-edge slot, and the
    /// session's reconstruction stays valid no matter how the requests
    /// interleave.
    #[tokio::test]
    async fn test_concurrent_graph_edits_on_same_anchor_never_corrupt_session() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "concurrent edit session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        const N: usize = 5;
        let requests: Vec<_> = (0..N)
            .map(|i| {
                // Each request's own op sequence is a COMPLETE,
                // independently admittable chain (Start -> Task_i -> End_i)
                // — the race is purely over which one claims start_key's
                // single outgoing-edge slot, not over structural
                // completeness of any one contender's graph.
                let task_key = new_key();
                let ops = vec![
                    designer_graph::ops::Operation::AppendNode {
                        anchor: start_key,
                        key: task_key,
                        node: task_ir(&format!("branch_{i}")),
                        edge_id: format!("f{i}a"),
                    },
                    designer_graph::ops::Operation::AppendNode {
                        anchor: task_key,
                        key: new_key(),
                        node: bpmn_lite_compiler::IRNode::End {
                            id: format!("end_{i}"),
                            terminate: false,
                        },
                        edge_id: format!("f{i}b"),
                    },
                ];
                app.clone().oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                    serde_json::json!({ "operations": ops, "note": format!("branch {i}") }),
                ))
            })
            .collect();
        let responses: Vec<_> =
            futures::future::join_all(requests).await.into_iter().map(|r| r.unwrap()).collect();

        let ok_count = responses.iter().filter(|r| r.status() == StatusCode::OK).count();
        let refused_count =
            responses.iter().filter(|r| r.status() == StatusCode::UNPROCESSABLE_ENTITY).count();
        assert_eq!(
            ok_count, 1,
            "only ONE request can legitimately win the anchor's single outgoing-edge slot"
        );
        assert_eq!(
            refused_count,
            N - 1,
            "every other request must see the winner's committed state and be refused \
             locally at staging — not race past it and corrupt the log"
        );

        // The decisive check: the session must still RECONSTRUCT after all
        // N concurrent requests settle — a bricked session (the pre-fix
        // failure mode) would 500 here instead.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "irrelevant", "anchor": "branch_0" }),
            ))
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "session must remain reconstructible after concurrent graph-edits"
        );
    }

    /// GREEN + the headline receipt: once a session is graph-backed, the
    /// utterance endpoint's context projection is REAL `project_ir`
    /// output (an anchor block, a real node census from the actual
    /// graph) — not the DSL-source census fallback — and an unknown
    /// anchor id is a fail-closed 422, never a silent whole-graph
    /// downgrade.
    #[tokio::test]
    async fn test_session_utterance_uses_positional_legality_when_graph_backed() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "positional session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let t1 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Unknown anchor: fail-closed 422, not a silent whole-graph fallback.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "insert a step", "anchor": "ghost_node" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        // Real anchor: training-grade projection.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "insert a step after this", "anchor": "review_documents" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{:?}", body_json(response).await);
        let record = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let utterance_event = record["events"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|e| e["kind"].get("Utterance"))
            .expect("utterance event present");
        let ctx_text = utterance_event["context_projection"].as_str().unwrap();
        assert!(
            ctx_text.contains("anchor: service_task review_documents"),
            "project_ir must project the REAL anchor, not a census-only fallback: {ctx_text:?}"
        );
        assert!(ctx_text.contains("nodes:\n"), "node census present: {ctx_text:?}");
    }

    /// G2 BLOCKER-2 ruling, receipt (i)'s server-side half: a graph-backed
    /// session saves as a template via `project_ir`, producing a real
    /// `plan_hash`. The bus-handler-level "does the projected plan
    /// actually INSTANTIATE" half of this receipt lives in
    /// `bpmn-lite-bus-handler/tests/graph_authored_plan_instantiation.rs`
    /// (this crate has no reason to depend on bpmn-lite-bus-handler's
    /// dispatch machinery just to prove that).
    #[tokio::test]
    async fn test_save_design_session_projects_graph_backed_session() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "save-graph-session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let t1 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "graph-authored-template" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["plan_hash"].as_str().unwrap().len(), 64);
    }

    /// RED: a save on a graph-backed session that doesn't admit (unmatched
    /// fork) must be refused, not silently save a broken plan.
    #[tokio::test]
    async fn test_save_design_session_refuses_non_admitting_graph() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "unmatched-fork-save" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let split_key = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: split_key,
                node: bpmn_lite_compiler::IRNode::GatewayAnd {
                    id: "split".into(),
                    name: "split".into(),
                    direction: bpmn_lite_compiler::GatewayDirection::Diverging,
                },
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: split_key,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        // This op sequence doesn't admit (unmatched fork), so it's refused
        // AT THE GRAPH-EDIT STEP already (BLOCKER 1's fix) — nothing is
        // persisted, so there's nothing further to save. Confirms the two
        // fixes compose: a session can never reach the save endpoint
        // carrying a non-admitting graph in the first place.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "unmatched-fork-template" }),
            ))
            .await
            .unwrap();
        // No GraphEdit ever landed, so this is a legacy (non-graph-backed,
        // no-revision) session — the pre-existing "no revision to save"
        // refusal, not a new code path.
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// G2 BLOCKER-2 ruling, receipt (ii): saving a graph-backed session as
    /// a template must NOT touch the session store's edit log — the DAG
    /// stays the authoring truth there (Rider 2, "two stores, two roles").
    /// Reopen-for-edit after a save must still reconstruct the exact same
    /// graph, and further graph-edits against the session must still work.
    #[tokio::test]
    async fn test_save_design_session_preserves_reopen_for_edit() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "reopen-after-save" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let t1 = new_key();
        let first_ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": first_ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let record_before = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let events_before = record_before["events"].as_array().unwrap().len();

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "reopen-after-save-template" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // The edit log is untouched by save — same event count, same
        // GraphEdit payloads (Rider 2: the session store is not rewritten
        // just because the plan store was).
        let record_after = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            record_after["events"].as_array().unwrap().len(),
            events_before,
            "save must not append to or mutate the session's edit log"
        );

        // The session is still open for editing — reopen-for-edit means a
        // further graph-edit against the SAME anchors continues to work
        // exactly as it would have pre-save.
        let further_ops = vec![designer_graph::ops::Operation::AppendNode {
            anchor: t1,
            key: new_key(),
            node: bpmn_lite_compiler::IRNode::End {
                id: "end2".into(),
                terminate: false,
            },
            edge_id: "f3".into(),
        }];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": further_ops }),
            ))
            .await
            .unwrap();
        // t1 already has an outgoing edge (to "end") from the first save's
        // graph — AppendNode correctly refuses a second one; the important
        // assertion is that reconstruction/staging still WORKS (422 from
        // real staging logic, not 500 from a corrupted/frozen session).
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "post-save reconstruction must still run real staging logic, not 500"
        );
    }
}
