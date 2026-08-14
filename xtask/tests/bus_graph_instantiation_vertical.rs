//! Multi-crate application vertical: graph-authored plan → bus-handler
//! define-template/spawn-instance dispatch (compiler + store + engine +
//! dsl-bus-protocol + dsl-bus-server). Moved from
//! bpmn-lite-bus-handler/tests/graph_authored_plan_instantiation.rs under
//! EOP-PLAN-CRATE-HYGIENE-001 H1.
//!
//! G2 BLOCKER-2 receipt (i): a `WorkflowExecutionPlan` produced by
//! `bpmn_lite_compiler::dsl::project_ir` (the IRGraph projection a
//! graph-authored session's save endpoint uses) is not a second-class
//! artifact — it defines a template ("define-template") and instantiates
//! a running process ("spawn-instance") through the SAME
//! `BpmnLiteBusHandler` dispatch path a DSL-authored plan goes through.
//! Mirrors the existing `sage_macro_assembly_tests.rs` setup (same
//! `PgPool::connect_lazy` + `MemoryStore`-backed engine).

use bpmn_lite_bus_handler::BpmnLiteBusHandler;
use dsl_bus_protocol::v1::{typed_value, ResolvedBinding, TypedValue};
use dsl_bus_server::{InvocationContext, InvocationDispatcher};
use std::sync::Arc;
use uuid::Uuid;

struct DummyAdvancer;
#[async_trait::async_trait]
impl bpmn_lite_bus_handler::ProcessAdvancer for DummyAdvancer {
    async fn advance(
        &self,
        _input: bpmn_lite_bus_handler::ProcessAdvanceInput,
    ) -> Result<(), bpmn_lite_bus_handler::ProcessAdvancerError> {
        Ok(())
    }
}

fn string_binding(name: &str, value: String) -> ResolvedBinding {
    ResolvedBinding {
        name: name.to_string(),
        value: Some(TypedValue {
            value: Some(typed_value::Value::StringValue(value)),
            type_name: "String".into(),
        }),
    }
}

fn uuid_binding(name: &str, value: Uuid) -> ResolvedBinding {
    ResolvedBinding {
        name: name.to_string(),
        value: Some(TypedValue {
            value: Some(typed_value::Value::UuidValue(dsl_bus_protocol::v1::Uuid {
                value: value.as_bytes().to_vec(),
            })),
            type_name: "Uuid".into(),
        }),
    }
}

fn auth(role: &str) -> dsl_bus_protocol::v1::AuthorityContext {
    dsl_bus_protocol::v1::AuthorityContext {
        service_identity: "ob-poc".into(),
        user_identity: "analyst@example.com".into(),
        roles: vec![role.into()],
        signed_token: vec![],
    }
}

fn ctx(local_verb_id: &str, role: &str) -> InvocationContext {
    InvocationContext {
        idempotency_key: Uuid::now_v7(),
        source_domain: "ob-poc".into(),
        catalogue_version: "v1.0.0".into(),
        local_verb_id: local_verb_id.into(),
        result_callback_endpoint: String::new(),
        authority: Some(auth(role)),
        tenant_id: "default".into(),
        snapshot_pin: None,
    }
}

/// A minimal graph-authored plan: Start -> ServiceTask -> End, built
/// directly as an `IRGraph` (no `designer-graph` dependency in this crate
/// — the point under test is that `project_ir`'s OUTPUT instantiates, not
/// the session/DAG machinery upstream of it, which WS-B.4's own tests
/// already cover).
fn build_graph_authored_plan() -> bpmn_lite_compiler::dsl::WorkflowExecutionPlan {
    use bpmn_lite_compiler::{IREdge, IRGraph, IRNode};
    let mut g: IRGraph = IRGraph::new();
    let s = g.add_node(IRNode::Start { id: "start".into() });
    let t = g.add_node(IRNode::ServiceTask {
        id: "t1".into(),
        name: "t1".into(),
        task_type: "noop".into(),
        loop_origin: None,
    });
    let e = g.add_node(IRNode::End { id: "end".into(), terminate: false });
    g.add_edge(s, t, IREdge { id: "e1".into(), condition: None });
    g.add_edge(t, e, IREdge { id: "e2".into(), condition: None });
    bpmn_lite_compiler::dsl::project_ir(&g, "graph-authored-wf".into())
        .expect("minimal linear chain must project")
}

/// GREEN: a `WorkflowExecutionPlan` built by `project_ir` defines a
/// template through the SAME `BpmnLiteBusHandler` "define-template" path a
/// DSL-authored plan uses — including `validate_path_family`'s closure
/// checks, which run unconditionally regardless of which compiler path
/// produced the plan. This is the part of the receipt this environment can
/// actually prove; see `graph_authored_plan_spawns_a_real_instance` below
/// for what's still blocked and why.
#[tokio::test]
async fn graph_authored_plan_defines_a_template() {
    let plan_hash_hex = define_template(build_graph_authored_plan()).await;
    assert_eq!(plan_hash_hex.len(), 64, "plan_hash must be a 32-byte hex digest");
}

async fn define_template(
    plan: bpmn_lite_compiler::dsl::WorkflowExecutionPlan,
) -> String {
    let plan_body = serde_json::to_string(&plan).unwrap();
    let store = Arc::new(bpmn_lite_store::MemoryStore::new());
    let engine = bpmn_lite_engine::BpmnLiteEngine::new(store.clone());
    let pool = sqlx::PgPool::connect_lazy("postgresql:///data_designer").unwrap();
    let handler = BpmnLiteBusHandler::new_with_engine(DummyAdvancer, Arc::new(engine), pool);

    let define_ctx = ctx("define-template", "bpmn.template.write");
    let define_inputs = vec![
        string_binding("template_key", "graph-authored-wf".into()),
        string_binding("plan_body", plan_body),
    ];
    let define_outcome = handler
        .dispatch(define_ctx, define_inputs)
        .await
        .expect("define-template on a graph-projected plan must succeed");
    define_outcome
        .outcome
        .bindings
        .iter()
        .find(|b| b.name == "plan_hash")
        .and_then(|b| b.value.as_ref())
        .and_then(|v| v.value.as_ref())
        .map(|v| match v {
            typed_value::Value::StringValue(s) => s.clone(),
            other => panic!("plan_hash binding has unexpected shape: {other:?}"),
        })
        .expect("define-template must return plan_hash")
}

/// BLOCKED — not by this work, by a pre-existing local-environment gap:
/// the `data_designer` Postgres database this test connects to (the SAME
/// connection string `sage_macro_assembly_tests.rs` already uses) has a
/// FOREIGN migration history (`_sqlx_migrations` shows versions `202412`
/// "workflow tables" / `6` "temporal ownership chains" — not this
/// workspace's numbered migration chain at all) and is missing
/// `bpmn_spawn_idempotency` (added in `042_verb_defect_and_isolation_fixes.sql`,
/// altered in `051_canonical_start_commands.sql`). `sqlx migrate run`
/// refuses to proceed (`migration 202412 was previously applied but is
/// missing in the resolved migrations`) rather than silently reconciling —
/// correctly so; forcing it through without knowing what else depends on
/// that DB's current state would be exactly the kind of destructive
/// action this session's working contract asks to check with Adam about,
/// not push through solo. No prior test in this crate ever exercised
/// "spawn-instance" (only "define-template"), which is why this gap was
/// never hit before now.
///
/// `#[ignore]`d rather than deleted: the assertions below are what SHOULD
/// run once `data_designer` (or a dedicated test DB) carries this
/// workspace's real migration chain — this is the concrete, re-runnable
/// spec for closing that gap, not a placeholder.
#[tokio::test]
#[ignore = "data_designer's migration history is foreign to this workspace \
            (see doc comment) — needs a DB carrying this repo's real \
            migrations before bpmn_spawn_idempotency exists"]
async fn graph_authored_plan_spawns_a_real_instance() {
    let plan_hash_hex = define_template(build_graph_authored_plan()).await;

    let store = Arc::new(bpmn_lite_store::MemoryStore::new());
    let engine = bpmn_lite_engine::BpmnLiteEngine::new(store.clone());
    let pool = sqlx::PgPool::connect_lazy("postgresql:///data_designer").unwrap();
    let handler = BpmnLiteBusHandler::new_with_engine(DummyAdvancer, Arc::new(engine), pool);

    let spawn_ctx = ctx("spawn-instance", "bpmn.instance.write");
    let spawn_inputs = vec![
        string_binding("template_key", plan_hash_hex),
        uuid_binding("idempotency_key", Uuid::now_v7()),
        uuid_binding("entry_id", Uuid::now_v7()),
        uuid_binding("runbook_id", Uuid::now_v7()),
    ];
    let spawn_outcome = handler
        .dispatch(spawn_ctx, spawn_inputs)
        .await
        .expect("spawn-instance on a graph-projected plan must succeed");
    assert!(
        !spawn_outcome.execution_id.is_nil(),
        "a real process instance must have been spawned"
    );
}
