//! Multi-crate application vertical: sage-macro template registration and
//! spawn through the bus handler backed by a real Postgres store
//! (compiler + engine + store + bpmn-lite-store-postgres + dsl-bus).
//! Moved from bpmn-lite-bus-handler/tests/sage_macro_assembly_tests.rs
//! under EOP-PLAN-CRATE-HYGIENE-001 H1.

use anyhow::Result;
use bpmn_lite_bus_handler::BpmnLiteBusHandler;
use bpmn_lite_store::ArtifactRepository;
use dsl_bus_protocol::v1::ResolvedBinding;
use dsl_bus_server::{InvocationContext, InvocationDispatcher};
use std::sync::Arc;
use uuid::Uuid;

/// Test-only helper to fabricate a plan's governance state directly,
/// bypassing `analyze_safety()` — mirrors how bus-handler receives a
/// foreign, already-compiled `plan_body` over the wire: as data, via
/// deserialization, not via `WorkflowExecutionPlan::new()`.
fn plan_with_forced_governance(
    plan: bpmn_lite_compiler::dsl::WorkflowExecutionPlan,
    mathematically_proved: bool,
    unsafe_breeches: Vec<String>,
) -> bpmn_lite_compiler::dsl::WorkflowExecutionPlan {
    let mut value = serde_json::to_value(&plan).unwrap();
    value["mathematically_proved"] = serde_json::json!(mathematically_proved);
    value["unsafe_breeches"] = serde_json::json!(unsafe_breeches);
    serde_json::from_value(value).unwrap()
}

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

#[tokio::test]
async fn test_sage_template_registration_via_bus() {
    let raw_sexpr = r#"(workflow onboarding-test
      (start-event :id start :next task1)
      (service-task :id task1 :verb cbu.create :next end)
      (end-event :id end :status "completed"))"#;

    // 1. Compile S-expression to plan
    let registry = bpmn_lite_compiler::dsl::StubPlaceholderRegistry::new().with_demo_bindings();
    let plan = bpmn_lite_compiler::dsl::compile(raw_sexpr, &registry).expect("Compile failed");
    let plan_body = serde_json::to_string(&plan).unwrap();

    // 2. Initialize engine with MemoryStore and PgPool lazily
    let store = Arc::new(bpmn_lite_store::MemoryStore::new());
    let engine = bpmn_lite_engine::BpmnLiteEngine::new(store.clone());
    let pool = sqlx::PgPool::connect_lazy("postgresql:///data_designer").unwrap();
    let handler = BpmnLiteBusHandler::new_with_engine(DummyAdvancer, Arc::new(engine), pool);

    // 3. Dispatch define-template invocation
    let auth = dsl_bus_protocol::v1::AuthorityContext {
        service_identity: "ob-poc".into(),
        user_identity: "analyst@example.com".into(),
        roles: vec!["bpmn.template.write".into()],
        signed_token: vec![],
    };

    let ctx = InvocationContext {
        idempotency_key: Uuid::now_v7(),
        source_domain: "ob-poc".into(),
        catalogue_version: "v1.0.0".into(),
        local_verb_id: "define-template".into(),
        result_callback_endpoint: String::new(),
        authority: Some(auth),
        tenant_id: "default".into(),
        snapshot_pin: None,
    };

    let inputs = vec![
        ResolvedBinding {
            name: "template_key".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    "onboarding-test".into(),
                )),
                type_name: "String".into(),
            }),
        },
        ResolvedBinding {
            name: "plan_body".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    plan_body.clone(),
                )),
                type_name: "String".into(),
            }),
        },
    ];

    let outcome = handler.dispatch(ctx, inputs).await;
    println!("Registration outcome: {:?}", outcome);
    outcome.expect("Define template failed");

    println!("Test original plan_body: {}", plan_body);
    let original_hash = *blake3::hash(plan_body.as_bytes()).as_bytes();
    println!("Original hash hex: {}", hex::encode(original_hash));

    let saved_opt = store.load_plan(original_hash).await.expect("load plan");
    if let Some(ref saved) = saved_opt {
        println!("Found in store: {}", saved);
    } else {
        println!("NOT found in store!");
    }
    let saved = saved_opt.expect("plan should be stored");
    assert_eq!(saved, plan_body);
}
#[tokio::test]
async fn test_sage_transitive_validation_propagation() {
    let store = Arc::new(bpmn_lite_store::MemoryStore::new());
    let engine = bpmn_lite_engine::BpmnLiteEngine::new(store.clone());
    let pool = sqlx::PgPool::connect_lazy("postgresql:///data_designer").unwrap();
    let handler = BpmnLiteBusHandler::new_with_engine(DummyAdvancer, Arc::new(engine), pool);

    // 1. Construct and store an unproved child plan
    let mut child_nodes = std::collections::HashMap::new();
    child_nodes.insert(
        "start".to_string(),
        bpmn_lite_compiler::dsl::ExecutionNode::Start(
            bpmn_lite_compiler::dsl::StartExecNode {
                id: "start".to_string(),
                next: "end".to_string(),
                span: None,
            },
        ),
    );
    child_nodes.insert(
        "end".to_string(),
        bpmn_lite_compiler::dsl::ExecutionNode::End(
            bpmn_lite_compiler::dsl::EndExecNode {
                id: "end".to_string(),
                status: "completed".to_string(),
                span: None,
            },
        ),
    );
    let child_plan = plan_with_forced_governance(
        bpmn_lite_compiler::dsl::WorkflowExecutionPlan::new(
            "unproved-child".to_string(),
            child_nodes.into_iter().collect(),
            "start".to_string(),
            bpmn_lite_compiler::dsl::PlaceholderSchema::default(),
            None,
            None,
        ),
        false,
        vec!["BPMN_BOUNDARY_EVENT_BYPASS".to_string()],
    );
    let child_body = serde_json::to_string(&child_plan).unwrap();
    let child_hash = *blake3::hash(child_body.as_bytes()).as_bytes();
    let child_hash_hex = hex::encode(child_hash);
    store
        .store_plan(child_hash, &child_body)
        .await
        .expect("Store child failed");

    // 2. Build a parent plan calling the unproved child
    let mut parent_nodes = std::collections::HashMap::new();
    parent_nodes.insert(
        "start".to_string(),
        bpmn_lite_compiler::dsl::ExecutionNode::Start(
            bpmn_lite_compiler::dsl::StartExecNode {
                id: "start".to_string(),
                next: "call-child".to_string(),
                span: None,
            },
        ),
    );
    parent_nodes.insert(
        "call-child".to_string(),
        bpmn_lite_compiler::dsl::ExecutionNode::Task(
            bpmn_lite_compiler::dsl::TaskExecNode {
                id: "call-child".to_string(),
                plug: child_hash_hex.clone(),
                delivery_mode: bpmn_lite_compiler::dsl::DeliveryMode::Blocking,
                static_args: std::collections::HashMap::new(),
                next: "end".to_string(),
                produces_placeholder: None,
                consumes_placeholders: vec![],
                guards: Vec::new(),
                span: None,
                loop_origin: None,
            },
        ),
    );
    parent_nodes.insert(
        "end".to_string(),
        bpmn_lite_compiler::dsl::ExecutionNode::End(
            bpmn_lite_compiler::dsl::EndExecNode {
                id: "end".to_string(),
                status: "completed".to_string(),
                span: None,
            },
        ),
    );

    // A. Parent is strictly proved (mathematically_proved = true)
    let parent_plan_strict = bpmn_lite_compiler::dsl::WorkflowExecutionPlan::new(
        "strict-parent".to_string(),
        parent_nodes.clone().into_iter().collect(),
        "start".to_string(),
        bpmn_lite_compiler::dsl::PlaceholderSchema::default(),
        None,
        None,
    );
    let strict_body = serde_json::to_string(&parent_plan_strict).unwrap();

    let auth = dsl_bus_protocol::v1::AuthorityContext {
        service_identity: "ob-poc".into(),
        user_identity: "analyst@example.com".into(),
        roles: vec!["bpmn.template.write".into()],
        signed_token: vec![],
    };

    let ctx = InvocationContext {
        idempotency_key: Uuid::now_v7(),
        source_domain: "ob-poc".into(),
        catalogue_version: "v1.0.0".into(),
        local_verb_id: "define-template".into(),
        result_callback_endpoint: String::new(),
        authority: Some(auth.clone()),
        tenant_id: "default".into(),
        snapshot_pin: None,
    };

    let inputs_strict = vec![
        ResolvedBinding {
            name: "template_key".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    "strict-parent".into(),
                )),
                type_name: "String".into(),
            }),
        },
        ResolvedBinding {
            name: "plan_body".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    strict_body,
                )),
                type_name: "String".into(),
            }),
        },
    ];

    // Must fail due to calling unproved child
    let strict_outcome = handler.dispatch(ctx.clone(), inputs_strict).await;
    assert!(
        strict_outcome.is_err(),
        "Expected strict parent calling unproved child to fail registration"
    );
    let err_msg = strict_outcome.unwrap_err().to_string();
    assert!(
        err_msg.contains("Transitive validation failed"),
        "Unexpected error: {}",
        err_msg
    );

    // B. Parent is permissive (mathematically_proved = false)
    let parent_plan_permissive = plan_with_forced_governance(
        bpmn_lite_compiler::dsl::WorkflowExecutionPlan::new(
            "permissive-parent".to_string(),
            parent_nodes.into_iter().collect(),
            "start".to_string(),
            bpmn_lite_compiler::dsl::PlaceholderSchema::default(),
            None,
            None,
        ),
        false,
        vec![],
    );
    let permissive_body = serde_json::to_string(&parent_plan_permissive).unwrap();

    let inputs_permissive = vec![
        ResolvedBinding {
            name: "template_key".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    "permissive-parent".into(),
                )),
                type_name: "String".into(),
            }),
        },
        ResolvedBinding {
            name: "plan_body".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    permissive_body,
                )),
                type_name: "String".into(),
            }),
        },
    ];

    let _permissive_outcome = handler
        .dispatch(ctx, inputs_permissive)
        .await
        .expect("Permissive parent registration failed");

    // Stored plan should have TRANSITIVE_UNPROVED_CALL breech
    let mut expected_parent_plan = parent_plan_permissive;
    expected_parent_plan.mark_transitive_unproved_call();
    let expected_parent_body = serde_json::to_string(&expected_parent_plan).unwrap();
    let expected_parent_hash = *blake3::hash(expected_parent_body.as_bytes()).as_bytes();

    let saved_parent = store
        .load_plan(expected_parent_hash)
        .await
        .expect("load plan")
        .expect("plan should be stored");
    let saved_plan: bpmn_lite_compiler::dsl::WorkflowExecutionPlan =
        serde_json::from_str(&saved_parent).unwrap();
    assert!(!saved_plan.mathematically_proved());
    assert!(
        saved_plan
            .unsafe_breeches()
            .contains(&"TRANSITIVE_UNPROVED_CALL".to_string())
    );
}

#[tokio::test]
async fn test_postgres_store_and_retrieve_via_bus_handler() {
    let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgresql://postgres@localhost:5435/bpmn_lite".to_string());

    let pool = match sqlx::PgPool::connect(&url).await {
        Ok(p) => p,
        Err(e) => {
            println!(
                "Skipping test_postgres_store_and_retrieve_via_bus_handler because PG connection failed: {}",
                e
            );
            return;
        }
    };

    // 1. Run migrations to ensure DB schema is populated
    let migrator = sqlx::migrate!("../bpmn-lite-store-postgres/migrations");
    migrator.run(&pool).await.expect("run migrations");

    // 2. Initialize the canonical process store.
    let postgres_store = Arc::new(bpmn_lite_store_postgres::PostgresWorkflowStore::new(
        pool.clone(),
    ));

    // 3. Initialize BpmnLiteEngine and BpmnLiteBusHandler
    let engine = Arc::new(bpmn_lite_engine::BpmnLiteEngine::new_with_tenant(
        postgres_store.clone(),
        bpmn_lite_types::TenantId::default(),
    ));
    let handler = BpmnLiteBusHandler::new_with_engine(DummyAdvancer, engine.clone(), pool.clone());

    // 5. Compile a test S-expression macro
    let raw_sexpr = r#"(workflow onboarding-postgres-test
      (start-event :id start :next task1)
      (service-task :id task1 :verb cbu.create :next end)
      (end-event :id end :status "completed"))"#;

    let registry = bpmn_lite_compiler::dsl::StubPlaceholderRegistry::new().with_demo_bindings();
    let plan = bpmn_lite_compiler::dsl::compile(raw_sexpr, &registry).expect("Compile failed");
    let plan_body = serde_json::to_string(&plan).unwrap();
    let original_hash = *blake3::hash(plan_body.as_bytes()).as_bytes();
    let plan_hash_hex = hex::encode(original_hash);
    let template_key = format!("onboarding-postgres-test-{}", Uuid::now_v7());

    // 6. Define template via InvocationDispatcher
    let auth = dsl_bus_protocol::v1::AuthorityContext {
        service_identity: "ob-poc".into(),
        user_identity: "analyst@example.com".into(),
        roles: vec!["bpmn.template.write".into()],
        signed_token: vec![],
    };

    let ctx = InvocationContext {
        idempotency_key: Uuid::now_v7(),
        source_domain: "ob-poc".into(),
        catalogue_version: "v1.0.0".into(),
        local_verb_id: "define-template".into(),
        result_callback_endpoint: String::new(),
        authority: Some(auth),
        tenant_id: "default".into(),
        snapshot_pin: None,
    };

    let inputs = vec![
        ResolvedBinding {
            name: "template_key".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    template_key,
                )),
                type_name: "String".into(),
            }),
        },
        ResolvedBinding {
            name: "plan_body".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    plan_body.clone(),
                )),
                type_name: "String".into(),
            }),
        },
    ];

    let outcome = handler
        .dispatch(ctx, inputs)
        .await
        .expect("Define template failed");
    assert_eq!(outcome.outcome.detail, "Template registered as version 1");

    // 7. Verify plan was stored and is retrievable
    let saved = postgres_store
        .load_plan(original_hash)
        .await
        .expect("load plan")
        .expect("plan should be stored");
    let saved_val: serde_json::Value = serde_json::from_str(&saved).unwrap();
    let original_val: serde_json::Value = serde_json::from_str(&plan_body).unwrap();
    assert_eq!(saved_val, original_val);

    // 8. Spawn instance via InvocationDispatcher
    let auth_spawn = dsl_bus_protocol::v1::AuthorityContext {
        service_identity: "ob-poc".into(),
        user_identity: "analyst@example.com".into(),
        roles: vec!["bpmn.instance.write".into()],
        signed_token: vec![],
    };

    let ctx_spawn = InvocationContext {
        idempotency_key: Uuid::now_v7(),
        source_domain: "ob-poc".into(),
        catalogue_version: "v1.0.0".into(),
        local_verb_id: "spawn-instance".into(),
        result_callback_endpoint: String::new(),
        authority: Some(auth_spawn),
        tenant_id: "default".into(),
        snapshot_pin: None,
    };

    let instance_idempotency_key = Uuid::now_v7();
    let inputs_spawn = vec![
        ResolvedBinding {
            name: "template_key".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    plan_hash_hex,
                )),
                type_name: "String".into(),
            }),
        },
        ResolvedBinding {
            name: "idempotency_key".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::UuidValue(
                    dsl_bus_protocol::v1::Uuid {
                        value: instance_idempotency_key.into_bytes().to_vec(),
                    },
                )),
                type_name: "Uuid".into(),
            }),
        },
        ResolvedBinding {
            name: "entry_id".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::UuidValue(
                    dsl_bus_protocol::v1::Uuid {
                        value: Uuid::now_v7().into_bytes().to_vec(),
                    },
                )),
                type_name: "Uuid".into(),
            }),
        },
        ResolvedBinding {
            name: "runbook_id".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::UuidValue(
                    dsl_bus_protocol::v1::Uuid {
                        value: Uuid::now_v7().into_bytes().to_vec(),
                    },
                )),
                type_name: "Uuid".into(),
            }),
        },
        ResolvedBinding {
            name: "expected_preconditions".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    "{}".to_string(),
                )),
                type_name: "String".into(),
            }),
        },
    ];

    let outcome_spawn = handler
        .dispatch(ctx_spawn, inputs_spawn)
        .await
        .expect("Spawn instance failed");
    let spawned_iid = outcome_spawn.execution_id;
    assert!(!spawned_iid.is_nil());

    // 9. Query database directly to verify the instance exists in pg
    let iid_stored: Option<Uuid> =
        sqlx::query_scalar("SELECT instance_id FROM workflow_instances WHERE instance_id = $1")
            .bind(spawned_iid)
            .fetch_optional(&pool)
            .await
            .expect("query instance");
    assert_eq!(iid_stored, Some(spawned_iid));
}

/// Reconstructed 2026-07-14 after the original working-tree copy was lost
/// to concurrent-session interference (a parallel agent editing this same
/// repo checkout during the same window); rebuilt from the setup pattern
/// already established by `test_postgres_store_and_retrieve_via_bus_handler`
/// above and the fix's own doc comment in `lib.rs::spawn_process_with_
/// idempotency` (see `docs/idempotency-race-fix-2026-07-14.md`).
///
/// Fires N concurrent `spawn-instance` dispatches sharing one
/// `idempotency_key` and asserts they converge on exactly one instance:
/// the transaction-scoped `pg_advisory_xact_lock` in `spawn_process_
/// with_idempotency` serializes concurrent callers for the same key, so
/// exactly one call actually spawns (`was_replay == false`, reflected as
/// `detail == "Instance spawned"`) and every other call finds the
/// already-committed bookkeeping row and replays it
/// (`detail == "idempotency replay"`) rather than either erroring on the
/// underlying `bpmn_spawn_idempotency` PRIMARY KEY violation or spawning
/// a second, orphaned process instance.
#[tokio::test]
async fn test_concurrent_spawn_instance_same_idempotency_key_creates_exactly_one_instance() {
    let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgresql://postgres@localhost:5435/bpmn_lite".to_string());

    let pool = match sqlx::PgPool::connect(&url).await {
        Ok(p) => p,
        Err(e) => {
            println!(
                "Skipping test_concurrent_spawn_instance_same_idempotency_key_creates_exactly_one_instance \
                 because PG connection failed: {}",
                e
            );
            return;
        }
    };

    let migrator = sqlx::migrate!("../bpmn-lite-store-postgres/migrations");
    migrator.run(&pool).await.expect("run migrations");

    let postgres_store = Arc::new(bpmn_lite_store_postgres::PostgresWorkflowStore::new(
        pool.clone(),
    ));

    let engine = Arc::new(bpmn_lite_engine::BpmnLiteEngine::new_with_tenant(
        postgres_store.clone(),
        bpmn_lite_types::TenantId::default(),
    ));
    let handler = Arc::new(BpmnLiteBusHandler::new_with_engine(
        DummyAdvancer,
        engine.clone(),
        pool.clone(),
    ));

    // Define a template to spawn instances from — same minimal workflow
    // shape as the sibling Postgres test above.
    let raw_sexpr = r#"(workflow onboarding-concurrency-test
      (start-event :id start :next task1)
      (service-task :id task1 :verb cbu.create :next end)
      (end-event :id end :status "completed"))"#;

    let registry = bpmn_lite_compiler::dsl::StubPlaceholderRegistry::new().with_demo_bindings();
    let plan = bpmn_lite_compiler::dsl::compile(raw_sexpr, &registry).expect("Compile failed");
    let plan_body = serde_json::to_string(&plan).unwrap();
    let original_hash = *blake3::hash(plan_body.as_bytes()).as_bytes();
    let plan_hash_hex = hex::encode(original_hash);

    let auth = dsl_bus_protocol::v1::AuthorityContext {
        service_identity: "ob-poc".into(),
        user_identity: "analyst@example.com".into(),
        roles: vec!["bpmn.template.write".into()],
        signed_token: vec![],
    };

    let ctx_define = InvocationContext {
        idempotency_key: Uuid::now_v7(),
        source_domain: "ob-poc".into(),
        catalogue_version: "v1.0.0".into(),
        local_verb_id: "define-template".into(),
        result_callback_endpoint: String::new(),
        authority: Some(auth),
        tenant_id: "default".into(),
        snapshot_pin: None,
    };

    let inputs_define = vec![
        ResolvedBinding {
            name: "template_key".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    "onboarding-concurrency-test".into(),
                )),
                type_name: "String".into(),
            }),
        },
        ResolvedBinding {
            name: "plan_body".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    plan_body.clone(),
                )),
                type_name: "String".into(),
            }),
        },
    ];

    handler
        .dispatch(ctx_define, inputs_define)
        .await
        .expect("Define template failed");

    // One shared idempotency key for the whole burst — this is the
    // property under test.
    let shared_idempotency_key = Uuid::now_v7();

    let mut join_set = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let handler = handler.clone();
        let plan_hash_hex = plan_hash_hex.clone();
        join_set.spawn(async move {
            let auth_spawn = dsl_bus_protocol::v1::AuthorityContext {
                service_identity: "ob-poc".into(),
                user_identity: "analyst@example.com".into(),
                roles: vec!["bpmn.instance.write".into()],
                signed_token: vec![],
            };

            let ctx_spawn = InvocationContext {
                idempotency_key: Uuid::now_v7(),
                source_domain: "ob-poc".into(),
                catalogue_version: "v1.0.0".into(),
                local_verb_id: "spawn-instance".into(),
                result_callback_endpoint: String::new(),
                authority: Some(auth_spawn),
                tenant_id: "default".into(),
                snapshot_pin: None,
            };

            let inputs_spawn = vec![
                ResolvedBinding {
                    name: "template_key".into(),
                    value: Some(dsl_bus_protocol::v1::TypedValue {
                        value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                            plan_hash_hex,
                        )),
                        type_name: "String".into(),
                    }),
                },
                ResolvedBinding {
                    name: "idempotency_key".into(),
                    value: Some(dsl_bus_protocol::v1::TypedValue {
                        value: Some(dsl_bus_protocol::v1::typed_value::Value::UuidValue(
                            dsl_bus_protocol::v1::Uuid {
                                value: shared_idempotency_key.into_bytes().to_vec(),
                            },
                        )),
                        type_name: "Uuid".into(),
                    }),
                },
                ResolvedBinding {
                    name: "entry_id".into(),
                    value: Some(dsl_bus_protocol::v1::TypedValue {
                        value: Some(dsl_bus_protocol::v1::typed_value::Value::UuidValue(
                            dsl_bus_protocol::v1::Uuid {
                                value: Uuid::now_v7().into_bytes().to_vec(),
                            },
                        )),
                        type_name: "Uuid".into(),
                    }),
                },
                ResolvedBinding {
                    name: "runbook_id".into(),
                    value: Some(dsl_bus_protocol::v1::TypedValue {
                        value: Some(dsl_bus_protocol::v1::typed_value::Value::UuidValue(
                            dsl_bus_protocol::v1::Uuid {
                                value: Uuid::now_v7().into_bytes().to_vec(),
                            },
                        )),
                        type_name: "Uuid".into(),
                    }),
                },
                ResolvedBinding {
                    name: "expected_preconditions".into(),
                    value: Some(dsl_bus_protocol::v1::TypedValue {
                        value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                            "{}".to_string(),
                        )),
                        type_name: "String".into(),
                    }),
                },
            ];

            handler.dispatch(ctx_spawn, inputs_spawn).await
        });
    }

    let mut execution_ids = std::collections::HashSet::new();
    let mut spawned_count = 0usize;
    let mut replay_count = 0usize;
    while let Some(res) = join_set.join_next().await {
        let outcome = res
            .expect("task panicked")
            .expect("spawn-instance dispatch failed");
        execution_ids.insert(outcome.execution_id);
        match outcome.outcome.detail.as_str() {
            "Instance spawned" => spawned_count += 1,
            "idempotency replay" => replay_count += 1,
            other => panic!("unexpected outcome detail: {other}"),
        }
    }

    assert_eq!(
        execution_ids.len(),
        1,
        "all 8 concurrent same-key spawns must converge on exactly one execution_id"
    );
    assert_eq!(
        spawned_count, 1,
        "exactly one caller must have actually spawned the instance"
    );
    assert_eq!(
        replay_count, 7,
        "the other 7 callers must have replayed the already-spawned instance"
    );

    let sole_iid = *execution_ids.iter().next().unwrap();

    let instance_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow_instances WHERE instance_id = $1")
            .bind(sole_iid)
            .fetch_one(&pool)
            .await
            .expect("count instance rows");
    assert_eq!(
        instance_rows, 1,
        "exactly one process-instance row must exist for the converged id"
    );

    let idempotency_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM bpmn_spawn_idempotency WHERE idempotency_key = $1",
    )
    .bind(shared_idempotency_key)
    .fetch_one(&pool)
    .await
    .expect("count idempotency rows");
    assert_eq!(
        idempotency_rows, 1,
        "exactly one bookkeeping row must exist for the shared idempotency key"
    );
}
