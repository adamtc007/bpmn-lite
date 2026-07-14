//! bpmn-lite receiver-side bus handler.
//!
//! bpmn-lite is the *workflow* domain: it submits invocations to
//! service domains (ob-poc, dmn-lite) and receives results back via
//! `ResultService.DeliverResult`. This crate implements
//! [`dsl_bus_server::ResultDispatcher`] so the bus server, on result
//! receipt, can hand the outcome to the bpmn-lite runtime via a
//! caller-supplied [`ProcessAdvancer`] port. T2B.9 / T3 wires the real
//! runtime against this port.
//!
//! The [`RejectInvocationDispatcher`] fills the bus server's other
//! dispatcher slot: bpmn-lite doesn't accept invocations from peers in
//! the v0.6 demo flow, so every Submit gets rejected as `UnknownVerb`.

#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use dsl_bus_protocol::v1::{ExecutionOutcome, ExecutionOutcomeKind, ResolvedBinding};
use dsl_bus_server::{
    BusServerError, InvocationContext, InvocationDispatcher, InvocationOutcome, ResultContext,
    ResultDispatcher,
};
use thiserror::Error;
use uuid::Uuid;

/// Information the bpmn-lite runtime needs to advance one process
/// instance: the receiver-supplied `execution_id` (which the caller
/// recorded in `bpmn_pending_invocation` per v0.6 §8.3), the outcome
/// taxonomy + detail, and the resolved bindings the verb produced.
#[derive(Debug, Clone)]
pub struct ProcessAdvanceInput {
    pub idempotency_key: Uuid,
    pub execution_id: Uuid,
    pub source_domain: String,
    pub outcome_kind: ExecutionOutcomeKind,
    pub outcome_detail: String,
    pub bindings: Vec<ResolvedBinding>,
    pub audit_reference: String,
}

#[derive(Debug, Error)]
pub enum ProcessAdvancerError {
    /// No pending-call row matched the `execution_id`. This is the
    /// `ReceiptStatus::RejectedUnknownExecution` path.
    #[error("no pending invocation matches execution_id {0}")]
    UnknownExecution(Uuid),

    #[error("malformed result payload: {0}")]
    Malformed(String),

    #[error("internal runtime error: {0}")]
    Internal(String),
}

impl From<ProcessAdvancerError> for BusServerError {
    fn from(e: ProcessAdvancerError) -> Self {
        match e {
            // No dedicated BusServerError variant for unknown-execution
            // — surface it as `Internal` because the bus server only
            // translates UnknownVerb / Version / Authority / Malformed
            // to first-class SubmissionStatus codes. Unknown-execution
            // does have a `ReceiptStatus::RejectedUnknownExecution`
            // but that's emitted earlier (before this dispatcher
            // runs) when idempotency_key is missing.
            ProcessAdvancerError::UnknownExecution(id) => {
                BusServerError::Internal(format!("unknown execution_id {id}"))
            }
            ProcessAdvancerError::Malformed(s) => BusServerError::Malformed(s),
            ProcessAdvancerError::Internal(s) => BusServerError::Internal(s),
        }
    }
}

/// Port the app supplies — T3 wires this to the bpmn-lite runtime's
/// `advance(process_instance_id, outcome)` once the pending-call row
/// is looked up by `execution_id`.
#[async_trait]
pub trait ProcessAdvancer: Send + Sync + 'static {
    async fn advance(&self, input: ProcessAdvanceInput) -> Result<(), ProcessAdvancerError>;
}

/// `ResultDispatcher` implementation for bpmn-lite. Holds an `Arc` over
/// the caller's [`ProcessAdvancer`] so the bus server's worker threads
/// share a single advancer.
pub struct BpmnLiteBusHandler {
    advancer: Arc<dyn ProcessAdvancer>,
    engine: Option<Arc<bpmn_lite_engine::BpmnLiteEngine>>,
    pool: Option<sqlx::PgPool>,
}

impl BpmnLiteBusHandler {
    pub fn new<A: ProcessAdvancer>(advancer: A) -> Self {
        Self {
            advancer: Arc::new(advancer),
            engine: None,
            pool: None,
        }
    }

    pub fn from_arc(advancer: Arc<dyn ProcessAdvancer>) -> Self {
        Self {
            advancer,
            engine: None,
            pool: None,
        }
    }

    pub fn new_with_engine<A: ProcessAdvancer>(
        advancer: A,
        engine: Arc<bpmn_lite_engine::BpmnLiteEngine>,
        pool: sqlx::PgPool,
    ) -> Self {
        Self {
            advancer: Arc::new(advancer),
            engine: Some(engine),
            pool: Some(pool),
        }
    }

    fn assert_scope(&self, auth: &dsl_bus_protocol::v1::AuthorityContext, required_role: &str) -> Result<(), BusServerError> {
        if auth.roles.iter().any(|r| r == required_role) {
            Ok(())
        } else {
            Err(BusServerError::AuthorityDenied(format!("Missing role {}", required_role)))
        }
    }

    async fn lookup_spawn_idempotency(&self, idempotency_key: Uuid, tenant_id: &str) -> Result<Option<Uuid>, BusServerError> {
        if let Some(pool) = &self.pool {
            let mut conn = pool.acquire().await.map_err(|e| BusServerError::Internal(e.to_string()))?;
            sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
                .bind(tenant_id)
                .execute(&mut *conn)
                .await
                .map_err(|e| BusServerError::Internal(e.to_string()))?;

            let iid: Option<Uuid> = sqlx::query_scalar(
                "SELECT instance_id FROM bpmn_spawn_idempotency WHERE idempotency_key = $1"
            )
            .bind(idempotency_key)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| BusServerError::Internal(e.to_string()))?;

            Ok(iid)
        } else {
            Ok(None)
        }
    }

    /// Spawns a process instance for `idempotency_key`, or returns the
    /// instance already spawned for that key.
    ///
    /// Returns `(instance_id, was_replay)` — `was_replay` is `true` when
    /// an existing instance was found and no new instance was created.
    ///
    /// # Concurrency
    ///
    /// This closes a TOCTOU race that existed between the caller's
    /// [`Self::lookup_spawn_idempotency`] pre-check and this method's own
    /// writes: two concurrent `spawn-instance` invocations carrying the
    /// *same* `idempotency_key` could both observe "not yet spawned",
    /// both call [`bpmn_lite_engine::plan_walker::PlanWalker::start_process`]
    /// (a real, side-effecting instance creation that writes the fiber
    /// VM's `process_instances`/`fibers` rows via the engine's own store,
    /// independently of the `tx` below), and only then race to insert the
    /// `bpmn_spawn_idempotency` bookkeeping row. The `idempotency_key`
    /// PRIMARY KEY on that table stops the *second* bookkeeping insert
    /// from committing, but by then the second `start_process()` call has
    /// already produced a real, fully-persisted, orphaned process
    /// instance with no bookkeeping row pointing at it — duplicate work,
    /// not prevented, only inconsistently recorded.
    ///
    /// The fix takes a transaction-scoped Postgres advisory lock keyed on
    /// the idempotency_key *before* doing anything side-effecting, then
    /// re-checks `bpmn_spawn_idempotency` under that lock. Concurrent
    /// callers for the same key are serialized: the first to acquire the
    /// lock proceeds to spawn+insert+commit (which releases the lock);
    /// every other concurrent caller blocks on the lock, then — once
    /// unblocked — finds the row already committed and returns it as a
    /// replay without ever calling `start_process()` again. Callers for
    /// *different* idempotency keys are unaffected (advisory locks are
    /// per-key; an occasional `hashtext` collision only costs a benign
    /// extra serialization, never a correctness violation).
    async fn spawn_process_with_idempotency(
        &self,
        plan: &bpmn_lite_compiler::dsl::plan::WorkflowExecutionPlan,
        tenant_id: &str,
        idempotency_key: Uuid,
        _entry_id: Uuid,
        _runbook_id: Uuid,
        expected_preconditions: std::collections::HashMap<String, String>,
    ) -> Result<(Uuid, bool), BusServerError> {
        if let Some(pool) = &self.pool {
            let mut tx = pool.begin().await.map_err(|e| BusServerError::Internal(e.to_string()))?;
            sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
                .bind(tenant_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| BusServerError::Internal(e.to_string()))?;

            // Transaction-scoped advisory lock, released automatically on
            // commit/rollback of `tx`. MUST happen before any side-effecting
            // work (start_process) or bookkeeping reads below — see doc
            // comment above.
            sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1::text), 0)")
                .bind(idempotency_key.to_string())
                .execute(&mut *tx)
                .await
                .map_err(|e| BusServerError::Internal(e.to_string()))?;

            // Re-check under the lock: a concurrent caller for this same
            // key may have already spawned and committed while we were
            // waiting to acquire the lock above.
            let existing: Option<Uuid> = sqlx::query_scalar(
                "SELECT instance_id FROM bpmn_spawn_idempotency WHERE idempotency_key = $1"
            )
            .bind(idempotency_key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| BusServerError::Internal(e.to_string()))?;

            if let Some(existing_iid) = existing {
                tx.commit().await.map_err(|e| BusServerError::Internal(e.to_string()))?;
                return Ok((existing_iid, true));
            }

            let engine_ref = self.engine.as_ref().ok_or_else(|| BusServerError::Internal("engine missing".into()))?;
            let pending_store = engine_ref.pending_store().ok_or_else(|| BusServerError::Internal("pending store missing".into()))?;
            let bus_client = engine_ref.bus_client().ok_or_else(|| BusServerError::Internal("bus client missing".into()))?;

            let walker = bpmn_lite_engine::plan_walker::PlanWalker::new(
                engine_ref.store(),
                pending_store,
                bus_client,
            );

            let iid = walker.start_process(
                "bus-handler",
                plan,
                tenant_id,
                std::collections::HashMap::new(),
                expected_preconditions,
            ).await.map_err(|e| BusServerError::Malformed(e.to_string()))?;

            sqlx::query(
                "INSERT INTO bpmn_process_instance (id, workflow_id, current_node, status, tenant_id) VALUES ($1, $2, $3, 'Running', $4)"
            )
            .bind(iid)
            .bind(&plan.workflow_id)
            .bind(&plan.start_node)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| BusServerError::Internal(e.to_string()))?;

            sqlx::query(
                "INSERT INTO bpmn_spawn_idempotency (idempotency_key, instance_id, tenant_id) VALUES ($1, $2, $3)"
            )
            .bind(idempotency_key)
            .bind(iid)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| BusServerError::Internal(e.to_string()))?;

            tx.commit().await.map_err(|e| BusServerError::Internal(e.to_string()))?;
            Ok((iid, false))
        } else {
            let engine_ref = self.engine.as_ref().ok_or_else(|| BusServerError::Internal("engine missing".into()))?;
            let pending_store = engine_ref.pending_store().ok_or_else(|| BusServerError::Internal("pending store missing".into()))?;
            let bus_client = engine_ref.bus_client().ok_or_else(|| BusServerError::Internal("bus client missing".into()))?;

            let walker = bpmn_lite_engine::plan_walker::PlanWalker::new(
                engine_ref.store(),
                pending_store,
                bus_client,
            );

            let iid = walker.start_process(
                "bus-handler",
                plan,
                tenant_id,
                std::collections::HashMap::new(),
                expected_preconditions,
            ).await.map_err(|e| BusServerError::Malformed(e.to_string()))?;
            Ok((iid, false))
        }
    }

    async fn correlate_message(&self, _message_name: &str, correlation_key: &str, tenant_id: &str) -> Result<Uuid, BusServerError> {
        if let Some(pool) = &self.pool {
            let mut conn = pool.acquire().await.map_err(|e| BusServerError::Internal(e.to_string()))?;
            sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
                .bind(tenant_id)
                .execute(&mut *conn)
                .await
                .map_err(|e| BusServerError::Internal(e.to_string()))?;

            let iid: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM bpmn_process_instance WHERE correlation_id = $1 AND status = 'Running'"
            )
            .bind(correlation_key)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| BusServerError::Internal(e.to_string()))?;

            iid.ok_or_else(|| BusServerError::Malformed(format!("No running instance found with correlation key '{}'", correlation_key)))
        } else {
            Err(BusServerError::Internal("Memory store correlation lookup not implemented".into()))
        }
    }
}

#[async_trait]
impl ResultDispatcher for BpmnLiteBusHandler {
    async fn dispatch(
        &self,
        ctx: ResultContext,
        outcome: ExecutionOutcome,
    ) -> Result<(), BusServerError> {
        let kind = ExecutionOutcomeKind::try_from(outcome.kind)
            .unwrap_or(ExecutionOutcomeKind::OutcomeUnspecified);
        let input = ProcessAdvanceInput {
            idempotency_key: ctx.idempotency_key,
            execution_id: ctx.execution_id,
            source_domain: ctx.source_domain,
            outcome_kind: kind,
            outcome_detail: outcome.detail,
            bindings: outcome.bindings,
            audit_reference: ctx.audit_reference,
        };
        self.advancer.advance(input).await?;
        Ok(())
    }
}

#[async_trait]
impl InvocationDispatcher for BpmnLiteBusHandler {
    async fn dispatch(
        &self,
        ctx: InvocationContext,
        inputs: Vec<ResolvedBinding>,
    ) -> Result<InvocationOutcome, BusServerError> {
        let auth = ctx.authority.as_ref().ok_or_else(|| {
            BusServerError::AuthorityDenied("missing authority context".into())
        })?;

        match ctx.local_verb_id.as_str() {
            "define-template" => {
                self.assert_scope(auth, "bpmn.template.write")?;
                let template_key = get_string_binding(&inputs, "template_key")?;

                // Build manifest registry first because compilation needs it for validation
                let stub_reg = bpmn_lite_compiler::dsl::linter::StubPlaceholderRegistry::new().with_demo_bindings();
                let mut registry = bpmn_lite_compiler::dsl::manifest_registry::ManifestPlaceholderRegistry::new(stub_reg);
                let paths = vec![
                    "manifests/bpmn-v1.0.0.yaml",
                    "manifests/dmn-lite-v1.0.0.yaml",
                    "manifests/ob-poc-v1.0.0.yaml",
                ];
                for p in &paths {
                    let path = std::path::Path::new(p);
                    let m = if path.exists() {
                        dsl_manifest::Manifest::load_from_path(path)
                            .map_err(|e| BusServerError::Internal(format!("Failed to load manifest at {}: {:?}", p, e)))?
                    } else {
                        let alt_path = format!("../{}", p);
                        let alt = std::path::Path::new(&alt_path);
                        if alt.exists() {
                            dsl_manifest::Manifest::load_from_path(alt)
                                .map_err(|e| BusServerError::Internal(format!("Failed to load manifest at {}: {:?}", alt_path, e)))?
                        } else {
                            return Err(BusServerError::Internal(format!("Manifest file not found: {} or {}", p, alt_path)));
                        }
                    };
                    registry.import(m);
                }

                let (_, mut plan, dsl_to_save) = if let Ok(dsl) = get_string_binding(&inputs, "dsl_body") {
                    let compiled_plan = bpmn_lite_compiler::dsl::compile(&dsl, &registry)
                        .map_err(|e| BusServerError::Malformed(format!("Compilation failed: {}", e)))?;
                    let json = serde_json::to_string(&compiled_plan)
                        .map_err(|e| BusServerError::Internal(format!("Failed to serialize plan: {}", e)))?;
                    (json, compiled_plan, dsl)
                } else {
                    let pb = get_string_binding(&inputs, "plan_body")?;
                    let p: bpmn_lite_compiler::dsl::plan::WorkflowExecutionPlan = serde_json::from_str(&pb)
                        .map_err(|e| BusServerError::Malformed(format!("Failed to parse plan_body: {}", e)))?;
                    (pb, p, String::new())
                };

                let client_proved = plan.mathematically_proved;
                plan.analyze_safety();
                if !client_proved {
                    plan.mathematically_proved = false;
                }

                let engine_ref = self.engine.as_ref().ok_or_else(|| BusServerError::Internal("engine missing".into()))?;
                let child_plans = bpmn_lite_engine::plan_walker::preload_workflow_dependencies(&plan, engine_ref.store().as_ref()).await
                    .map_err(|e| BusServerError::Internal(format!("Failed to preload dependencies: {}", e)))?;

                let mut has_unproved_child = false;
                for child_plan in child_plans.values() {
                    if !child_plan.mathematically_proved {
                        has_unproved_child = true;
                        break;
                    }
                }

                if has_unproved_child {
                    if plan.mathematically_proved {
                        return Err(BusServerError::Malformed(
                            "Transitive validation failed: Proven parent workflow template calls unproven child sub-workflow".into()
                        ));
                    } else {
                        if !plan.unsafe_breeches.contains(&"TRANSITIVE_UNPROVED_CALL".to_string()) {
                            plan.unsafe_breeches.push("TRANSITIVE_UNPROVED_CALL".to_string());
                        }
                    }
                }

                let plan_body = serde_json::to_string(&plan)
                    .map_err(|e| BusServerError::Internal(format!("Failed to serialize plan: {}", e)))?;

                let mut stub_reg2 = bpmn_lite_compiler::dsl::linter::StubPlaceholderRegistry::new().with_demo_bindings();
                for (hash, child_plan) in &child_plans {
                    let mut consumes = Vec::new();
                    for slot in child_plan.placeholder_schema.slots.values() {
                        if slot.produced_by == "start" || slot.produced_by.is_empty() {
                            consumes.push(slot.name.clone());
                        }
                    }
                    stub_reg2.register_workflow(hash.clone(), bpmn_lite_compiler::dsl::linter::BindingDecl {
                        produces: None,
                        consumes,
                        effect_class: Some("idempotent_ensure".into()),
                    }, true);

                    let mut child_calls = Vec::new();
                    for node in child_plan.nodes.values() {
                        if let bpmn_lite_compiler::dsl::plan::ExecutionNode::Task(t) = node {
                            let is_hex_hash = t.plug.len() == 64 && t.plug.chars().all(|c| c.is_ascii_hexdigit());
                            if is_hex_hash {
                                child_calls.push(t.plug.clone());
                            }
                        }
                    }
                    stub_reg2.register_workflow_child_calls(hash, child_calls);
                }

                let mut registry2 = bpmn_lite_compiler::dsl::manifest_registry::ManifestPlaceholderRegistry::new(stub_reg2);
                for p in &paths {
                    let path = std::path::Path::new(p);
                    let m = if path.exists() {
                        dsl_manifest::Manifest::load_from_path(path)
                            .map_err(|e| BusServerError::Internal(format!("Failed to load manifest at {}: {:?}", p, e)))?
                    } else {
                        let alt_path = format!("../{}", p);
                        let alt = std::path::Path::new(&alt_path);
                        if alt.exists() {
                            dsl_manifest::Manifest::load_from_path(alt)
                                .map_err(|e| BusServerError::Internal(format!("Failed to load manifest at {}: {:?}", alt_path, e)))?
                        } else {
                            return Err(BusServerError::Internal(format!("Manifest file not found: {} or {}", p, alt_path)));
                        }
                    };
                    registry2.import(m);
                }

                let diagnostics = bpmn_lite_compiler::dsl::closure::validate_path_family(&plan, &registry2);
                if !diagnostics.is_empty() {
                    return Err(BusServerError::Malformed(format!("Path-family validation failed: {:?}", diagnostics)));
                }

                let hash = *blake3::hash(plan_body.as_bytes()).as_bytes();
                engine_ref.store().store_plan(hash, &plan_body).await
                    .map_err(|e| BusServerError::Internal(e.to_string()))?;

                // Get next version for this template catalog
                let version = match engine_ref.store().load_latest_template_version(&template_key).await {
                    Ok(Some((v, _, _))) => v + 1,
                    _ => 1,
                };

                // Store template version snapshot
                engine_ref.store().store_template(&template_key, version, hash, &dsl_to_save).await
                    .map_err(|e| BusServerError::Internal(e.to_string()))?;

                Ok(InvocationOutcome {
                    execution_id: Uuid::now_v7(),
                    outcome: ExecutionOutcome {
                        kind: dsl_bus_protocol::v1::ExecutionOutcomeKind::OutcomeUnspecified as i32,
                        detail: format!("Template registered as version {}", version),
                        bindings: vec![
                            string_binding("plan_hash".to_string(), hex::encode(hash)),
                            string_binding("version".to_string(), version.to_string()),
                        ],
                    },
                })
            }
            "spawn-instance" => {
                self.assert_scope(auth, "bpmn.instance.write")?;
                let template_key = get_string_binding(&inputs, "template_key")?;
                let idempotency_key = get_uuid_binding(&inputs, "idempotency_key")?;
                let entry_id = get_uuid_binding(&inputs, "entry_id")?;
                let runbook_id = get_uuid_binding(&inputs, "runbook_id")?;

                if entry_id.is_nil() || runbook_id.is_nil() {
                    return Err(BusServerError::Malformed("entry_id and runbook_id must be non-nil UUIDs".into()));
                }

                if let Some(existing_iid) = self.lookup_spawn_idempotency(idempotency_key, &ctx.tenant_id).await? {
                    return Ok(InvocationOutcome {
                        execution_id: existing_iid,
                        outcome: ExecutionOutcome {
                            kind: dsl_bus_protocol::v1::ExecutionOutcomeKind::Committed as i32,
                            detail: "idempotency replay".to_string(),
                            bindings: vec![
                                string_binding("instance_id".to_string(), existing_iid.to_string()),
                            ],
                        },
                    });
                }

                let engine_ref = self.engine.as_ref().ok_or_else(|| BusServerError::Internal("engine missing".into()))?;
                let is_hex = template_key.len() == 64 && template_key.chars().all(|c| c.is_ascii_hexdigit());

                let plan_json = if is_hex {
                    let plan_hash = hex::decode(&template_key)
                        .map_err(|_| BusServerError::Malformed("invalid template_key hex".into()))?;
                    if plan_hash.len() != 32 {
                        return Err(BusServerError::Malformed("invalid template_key length".into()));
                    }
                    let mut hash_arr = [0u8; 32];
                    hash_arr.copy_from_slice(&plan_hash);
                    engine_ref.store().load_plan(hash_arr).await
                        .map_err(|e| BusServerError::Internal(e.to_string()))?
                        .ok_or_else(|| BusServerError::Malformed(format!("plan not found for hash {}", template_key)))?
                } else {
                    let (_, _, hash) = engine_ref.store().load_latest_template_version(&template_key).await
                        .map_err(|e| BusServerError::Internal(e.to_string()))?
                        .ok_or_else(|| BusServerError::Malformed(format!("template name not found: {}", template_key)))?;
                    engine_ref.store().load_plan(hash).await
                        .map_err(|e| BusServerError::Internal(e.to_string()))?
                        .ok_or_else(|| BusServerError::Malformed(format!("plan not found for hash {}", hex::encode(hash))))?
                };

                let plan: bpmn_lite_compiler::dsl::plan::WorkflowExecutionPlan = serde_json::from_str(&plan_json)
                    .map_err(|e| BusServerError::Malformed(format!("invalid plan JSON: {}", e)))?;

                let expected_preconditions = get_preconditions_binding(&inputs, "expected_preconditions")?;

                let (instance_id, was_replay) = self.spawn_process_with_idempotency(
                    &plan,
                    &ctx.tenant_id,
                    idempotency_key,
                    entry_id,
                    runbook_id,
                    expected_preconditions,
                ).await?;

                Ok(InvocationOutcome {
                    execution_id: instance_id,
                    outcome: ExecutionOutcome {
                        kind: dsl_bus_protocol::v1::ExecutionOutcomeKind::Committed as i32,
                        detail: if was_replay { "idempotency replay".to_string() } else { "Instance spawned".to_string() },
                        bindings: vec![
                            string_binding("instance_id".to_string(), instance_id.to_string()),
                        ],
                    },
                })
            }
            "list-templates" => {
                self.assert_scope(auth, "bpmn.template.read")?;
                let engine_ref = self.engine.as_ref().ok_or_else(|| BusServerError::Internal("engine missing".into()))?;
                let list = engine_ref.store().list_templates().await
                    .map_err(|e| BusServerError::Internal(e.to_string()))?;
                let list_json = serde_json::to_string(&list)
                    .map_err(|e| BusServerError::Internal(e.to_string()))?;

                Ok(InvocationOutcome {
                    execution_id: Uuid::now_v7(),
                    outcome: ExecutionOutcome {
                        kind: dsl_bus_protocol::v1::ExecutionOutcomeKind::OutcomeUnspecified as i32,
                        detail: "Success".to_string(),
                        bindings: vec![
                            string_binding("templates_json".to_string(), list_json),
                        ],
                    },
                })
            }
            "get-template-version" => {
                self.assert_scope(auth, "bpmn.template.read")?;
                let template_key = get_string_binding(&inputs, "template_key")?;
                let version_opt = get_int_binding(&inputs, "version").ok();

                let engine_ref = self.engine.as_ref().ok_or_else(|| BusServerError::Internal("engine missing".into()))?;
                let (dsl_body, plan_hash) = if let Some(v) = version_opt {
                    engine_ref.store().load_template_version(&template_key, v as u32).await
                        .map_err(|e| BusServerError::Internal(e.to_string()))?
                        .ok_or_else(|| BusServerError::Malformed(format!("Template version not found: {} v{}", template_key, v)))?
                } else {
                    let (_, dsl, hash) = engine_ref.store().load_latest_template_version(&template_key).await
                        .map_err(|e| BusServerError::Internal(e.to_string()))?
                        .ok_or_else(|| BusServerError::Malformed(format!("Template not found: {}", template_key)))?;
                    (dsl, hash)
                };

                Ok(InvocationOutcome {
                    execution_id: Uuid::now_v7(),
                    outcome: ExecutionOutcome {
                        kind: dsl_bus_protocol::v1::ExecutionOutcomeKind::OutcomeUnspecified as i32,
                        detail: "Success".to_string(),
                        bindings: vec![
                            string_binding("dsl_body".to_string(), dsl_body),
                            string_binding("plan_hash".to_string(), hex::encode(plan_hash)),
                        ],
                    },
                })
            }
            "deliver-message" => {
                self.assert_scope(auth, "bpmn.instance.write")?;
                let instance_id = get_uuid_binding(&inputs, "instance_id")?;
                let message_name = get_string_binding(&inputs, "message_name")?;
                let payload = get_string_binding(&inputs, "payload")?;

                let engine_ref = self.engine.as_ref().ok_or_else(|| BusServerError::Internal("engine missing".into()))?;
                let tenant_engine = engine_ref.for_tenant(&ctx.tenant_id);
                tenant_engine.signal(instance_id, &message_name, "", Some(&payload), None, None).await
                    .map_err(|e| BusServerError::Internal(e.to_string()))?;

                Ok(InvocationOutcome {
                    execution_id: Uuid::now_v7(),
                    outcome: ExecutionOutcome {
                        kind: dsl_bus_protocol::v1::ExecutionOutcomeKind::Committed as i32,
                        detail: "Message delivered".to_string(),
                        bindings: vec![],
                    },
                })
            }
            "correlate-message" => {
                self.assert_scope(auth, "bpmn.instance.write")?;
                let message_name = get_string_binding(&inputs, "message_name")?;
                let correlation_key = get_string_binding(&inputs, "correlation_key")?;
                let payload = get_string_binding(&inputs, "payload")?;

                let instance_id = self.correlate_message(&message_name, &correlation_key, &ctx.tenant_id).await?;

                let engine_ref = self.engine.as_ref().ok_or_else(|| BusServerError::Internal("engine missing".into()))?;
                let tenant_engine = engine_ref.for_tenant(&ctx.tenant_id);
                tenant_engine.signal(instance_id, &message_name, &correlation_key, Some(&payload), None, None).await
                    .map_err(|e| BusServerError::Internal(e.to_string()))?;

                Ok(InvocationOutcome {
                    execution_id: instance_id,
                    outcome: ExecutionOutcome {
                        kind: dsl_bus_protocol::v1::ExecutionOutcomeKind::Committed as i32,
                        detail: "Message correlated and delivered".to_string(),
                        bindings: vec![],
                    },
                })
            }
            "cancel-instance" => {
                self.assert_scope(auth, "bpmn.instance.write")?;
                let instance_id = get_uuid_binding(&inputs, "instance_id")?;

                let engine_ref = self.engine.as_ref().ok_or_else(|| BusServerError::Internal("engine missing".into()))?;
                let tenant_engine = engine_ref.for_tenant(&ctx.tenant_id);
                let reason = get_string_binding(&inputs, "reason")?;
                tenant_engine.cancel(instance_id, &reason).await
                    .map_err(|e| BusServerError::Internal(e.to_string()))?;

                Ok(InvocationOutcome {
                    execution_id: instance_id,
                    outcome: ExecutionOutcome {
                        kind: dsl_bus_protocol::v1::ExecutionOutcomeKind::Committed as i32,
                        detail: "Instance cancelled".to_string(),
                        bindings: vec![],
                    },
                })
            }
            "inspect-instance" => {
                self.assert_scope(auth, "bpmn.instance.read")?;
                let instance_id = get_uuid_binding(&inputs, "instance_id")?;

                let engine_ref = self.engine.as_ref().ok_or_else(|| BusServerError::Internal("engine missing".into()))?;
                let tenant_engine = engine_ref.for_tenant(&ctx.tenant_id);
                let inspection = tenant_engine.inspect(instance_id).await
                    .map_err(|e| BusServerError::Internal(e.to_string()))?;

                let mut schema_vars = std::collections::HashMap::new();
                if let Some(map) = inspection.placeholder_values.as_ref().and_then(|v| v.as_object()) {
                    for (k, v) in map {
                        schema_vars.insert(k.clone(), v.to_string());
                    }
                }

                let details = serde_json::json!({
                    "instance_id": inspection.instance_id.to_string(),
                    "state": format!("{:?}", inspection.state),
                    "current_node": inspection.current_node_id.clone().unwrap_or_default(),
                    "active_incidents": vec![] as Vec<String>,
                    "schema_variables": schema_vars,
                });

                Ok(InvocationOutcome {
                    execution_id: instance_id,
                    outcome: ExecutionOutcome {
                        kind: dsl_bus_protocol::v1::ExecutionOutcomeKind::Committed as i32,
                        detail: "Inspection details".to_string(),
                        bindings: vec![
                            string_binding("instance_details".to_string(), details.to_string()),
                        ],
                    },
                })
            }
            _ => Err(BusServerError::UnknownVerb(ctx.local_verb_id)),
        }
    }
}

fn get_preconditions_binding(inputs: &[ResolvedBinding], name: &str) -> Result<std::collections::HashMap<String, String>, BusServerError> {
    let binding = match inputs.iter().find(|b| b.name == name) {
        Some(b) => b,
        None => return Ok(std::collections::HashMap::new()),
    };
    let val = match &binding.value {
        Some(v) => v,
        None => return Ok(std::collections::HashMap::new()),
    };
    match val.value.as_ref() {
        Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(s)) => {
            if s.is_empty() {
                Ok(std::collections::HashMap::new())
            } else {
                serde_json::from_str(s).map_err(|e| BusServerError::Malformed(format!("invalid preconditions JSON: {e}")))
            }
        }
        Some(dsl_bus_protocol::v1::typed_value::Value::NullValue(_)) => Ok(std::collections::HashMap::new()),
        None => Ok(std::collections::HashMap::new()),
        _ => Err(BusServerError::Malformed(format!("input '{}' is not a string", name))),
    }
}

fn get_string_binding(inputs: &[ResolvedBinding], name: &str) -> Result<String, BusServerError> {
    let binding = inputs.iter().find(|b| b.name == name)
        .ok_or_else(|| BusServerError::Malformed(format!("missing input '{}'", name)))?;
    let val = binding.value.as_ref()
        .ok_or_else(|| BusServerError::Malformed(format!("input '{}' has no value", name)))?;
    match val.value.as_ref() {
        Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(s)) => Ok(s.clone()),
        _ => Err(BusServerError::Malformed(format!("input '{}' is not a string", name))),
    }
}

fn get_uuid_binding(inputs: &[ResolvedBinding], name: &str) -> Result<Uuid, BusServerError> {
    let binding = inputs.iter().find(|b| b.name == name)
        .ok_or_else(|| BusServerError::Malformed(format!("missing input '{}'", name)))?;
    let val = binding.value.as_ref()
        .ok_or_else(|| BusServerError::Malformed(format!("input '{}' has no value", name)))?;
    match val.value.as_ref() {
        Some(dsl_bus_protocol::v1::typed_value::Value::UuidValue(u)) => {
            if u.value.len() == 16 {
                Uuid::from_slice(&u.value).map_err(|e| BusServerError::Malformed(e.to_string()))
            } else {
                Err(BusServerError::Malformed(format!("invalid uuid length for '{}'", name)))
            }
        }
        _ => Err(BusServerError::Malformed(format!("input '{}' is not a uuid", name))),
    }
}

fn get_int_binding(inputs: &[ResolvedBinding], name: &str) -> Result<i64, BusServerError> {
    let binding = inputs.iter().find(|b| b.name == name)
        .ok_or_else(|| BusServerError::Malformed(format!("missing input '{}'", name)))?;
    let val = binding.value.as_ref()
        .ok_or_else(|| BusServerError::Malformed(format!("input '{}' has no value", name)))?;
    match val.value.as_ref() {
        Some(dsl_bus_protocol::v1::typed_value::Value::IntValue(i)) => Ok(*i),
        _ => Err(BusServerError::Malformed(format!("input '{}' is not an integer", name))),
    }
}

fn string_binding(name: String, value: String) -> ResolvedBinding {
    ResolvedBinding {
        name,
        value: Some(dsl_bus_protocol::v1::TypedValue {
            value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(value)),
            type_name: "String".to_string(),
        }),
    }
}

/// `InvocationDispatcher` impl that rejects everything — fallback when not initialized with engine.
pub struct RejectInvocationDispatcher;

#[async_trait]
impl InvocationDispatcher for RejectInvocationDispatcher {
    async fn dispatch(
        &self,
        ctx: InvocationContext,
        _inputs: Vec<ResolvedBinding>,
    ) -> Result<InvocationOutcome, BusServerError> {
        Err(BusServerError::UnknownVerb(format!(
            "bpmn-lite does not accept invocations (rejected '{}')",
            ctx.local_verb_id
        )))
    }
}


#[cfg(test)]
mod tests;
