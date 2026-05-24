//! Federated DSL bus runtime for `bpmn-lite-server` (v0.6 §T2B.9 + T3.4).
//!
//! T3.4 update: `StoreBackedAdvancer` now operates on `ProcessStore`
//! (the bytecode engine store with plan_hash support) instead of the
//! separate `BpmnProcessInstanceStore`. When a result arrives for a
//! plan-based instance it:
//!
//! 1. Takes the pending invocation row (establishes which node fired).
//! 2. Loads the `ProcessInstance` via `ProcessStore`.
//! 3. If the instance has a `plan_hash`, loads the plan and advances
//!    `current_node_id` to the completed node's `next` neighbour.
//! 4. Populates `placeholder_values` from the result bindings.
//! 5. Sets `instance.state = ProcessState::Running` so the tick loop
//!    picks it up and walks the next plan node.

#![cfg(feature = "postgres")]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bpmn_lite_bus_handler::{
    BpmnLiteBusHandler, ProcessAdvanceInput, ProcessAdvancer, ProcessAdvancerError,
    RejectInvocationDispatcher,
};
use bpmn_lite_compiler::dsl::plan::{ExecutionNode, WorkflowExecutionPlan};
use bpmn_lite_store::pending::PendingInvocationStore;
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_store_postgres::PostgresPendingInvocationStore;
use bpmn_lite_types::types::ProcessState;
use dsl_bus_client::BusClient;
use dsl_bus_protocol::v1::{typed_value::Value as ProtoValueKind, ExecutionOutcomeKind};
use dsl_bus_server::{BusServer, ServerHandle};
use sqlx::PgPool;
use uuid::Uuid;

/// Owned bus runtime.
pub(crate) struct BusRuntime {
    server: ServerHandle,
    sender: dsl_bus_client::SenderHandle,
    client: Arc<BusClient>,
}

impl BusRuntime {
    /// Clone of the bus client for wiring into BpmnLiteEngine (T3.3).
    pub(crate) fn bus_client(&self) -> Arc<BusClient> {
        self.client.clone()
    }

    pub(crate) async fn shutdown(self) -> anyhow::Result<()> {
        let _ = self.server.shutdown().await;
        let _ = self.sender.shutdown().await;
        Ok(())
    }
}

/// Configuration plumbed in by `main`.
pub(crate) struct BusRuntimeConfig {
    pub(crate) pool: PgPool,
    pub(crate) bind_addr: SocketAddr,
    /// Pre-built bus client (T3.3 — built before engine so it can be wired in).
    pub(crate) client: Arc<BusClient>,
    /// T3.4 — engine's ProcessStore for loading/saving plan-based instances.
    pub(crate) store: Arc<dyn ProcessStore>,
}

pub(crate) async fn start(config: BusRuntimeConfig) -> anyhow::Result<BusRuntime> {
    dsl_bus_storage::migrate(&config.pool).await?;

    let client = config.client;
    let notifier = client.outbox_notifier();
    let sender = client.start_sender();

    let advancer = StoreBackedAdvancer {
        pending: Arc::new(PostgresPendingInvocationStore::new(config.pool.clone())),
        store: config.store,
    };

    let server = BusServer::builder()
        .pool(config.pool)
        .local_domain("bpmn-lite")
        .invocation_dispatcher(RejectInvocationDispatcher)
        .result_dispatcher(BpmnLiteBusHandler::new(advancer))
        .outbox_notifier(notifier)
        .bind(config.bind_addr)
        .build()
        .serve()
        .await?;

    tracing::info!(
        bind_addr = %server.local_addr(),
        "bpmn-lite bus server listening (result receiver)"
    );

    Ok(BusRuntime { server, sender, client })
}

// ── StoreBackedAdvancer ──────────────────────────────────────────────

/// T3.4 — advances plan-based process instances on result arrival.
///
/// Flow:
/// 1. Take pending invocation row (establishes node_id + process_instance_id).
/// 2. Load ProcessInstance from ProcessStore.
/// 3. For plan-based instances (plan_hash.is_some()):
///    a. Load the WorkflowExecutionPlan.
///    b. Advance current_node_id to the completed node's `next` neighbour
///       so the next tick walks past the completed service/business-rule node.
///    c. Bind placeholder values from result bindings.
/// 4. Set state = Running (tick loop will call PlanWalker.advance() on next cycle).
/// 5. For terminal outcomes (VerbFailed etc.) set state = Failed.
struct StoreBackedAdvancer {
    pending: Arc<PostgresPendingInvocationStore>,
    store: Arc<dyn ProcessStore>,
}

#[async_trait]
impl ProcessAdvancer for StoreBackedAdvancer {
    async fn advance(&self, input: ProcessAdvanceInput) -> Result<(), ProcessAdvancerError> {
        let row = self
            .pending
            .take_by_execution_id(input.execution_id)
            .await
            .map_err(|e| ProcessAdvancerError::Internal(format!("take pending: {e}")))?;

        let Some(row) = row else {
            return Err(ProcessAdvancerError::UnknownExecution(input.execution_id));
        };

        let mut instance = self
            .store
            .load_instance(row.process_instance_id)
            .await
            .map_err(|e| ProcessAdvancerError::Internal(format!("load instance: {e}")))?
            .ok_or_else(|| {
                ProcessAdvancerError::Internal(format!(
                    "pending row referenced unknown instance {}",
                    row.process_instance_id
                ))
            })?;

        let is_success = matches!(
            input.outcome_kind,
            ExecutionOutcomeKind::Committed | ExecutionOutcomeKind::IdempotentReplayReturned
        );
        let is_transient = matches!(
            input.outcome_kind,
            ExecutionOutcomeKind::OptimisticConflict | ExecutionOutcomeKind::LockTimeout
        );

        if is_success || is_transient {
            // T3.4 — plan-based: advance node + bind placeholders.
            if let Some(plan_hash) = instance.plan_hash {
                if let Ok(Some(plan_json)) = self.store.load_plan(plan_hash).await {
                    if let Ok(plan) = serde_json::from_str::<WorkflowExecutionPlan>(&plan_json) {
                        if let Some(node) = plan.nodes.get(&row.node_id) {
                            advance_node_and_bind(
                                &mut instance,
                                node,
                                &input,
                            );
                        }
                    }
                }
            }
            instance.state = ProcessState::Running;
        } else if let ExecutionOutcomeKind::OutcomeUnspecified = input.outcome_kind {
            return Err(ProcessAdvancerError::Malformed(
                "ExecutionOutcomeKind::OutcomeUnspecified — peer must populate kind".to_owned(),
            ));
        } else {
            // Terminal failure.
            instance.state = ProcessState::Failed { incident_id: Uuid::now_v7() };
        }

        self.store
            .save_instance(&instance)
            .await
            .map_err(|e| ProcessAdvancerError::Internal(format!("save instance: {e}")))?;

        tracing::info!(
            execution_id = %input.execution_id,
            callout_id = %row.callout_id,
            process_instance_id = %row.process_instance_id,
            node_id = %row.node_id,
            source_domain = %input.source_domain,
            outcome = ?input.outcome_kind,
            has_plan = instance.plan_hash.is_some(),
            "bus result received; instance set to Running for tick loop"
        );
        Ok(())
    }
}

/// Advance `current_node_id` to the completed node's successor and
/// populate `placeholder_values` from the result bindings.
fn advance_node_and_bind(
    instance: &mut bpmn_lite_types::ProcessInstance,
    node: &ExecutionNode,
    input: &ProcessAdvanceInput,
) {
    let (produces, next) = match node {
        ExecutionNode::ServiceTask(t) => (t.produces_placeholder.as_deref(), t.next.as_str()),
        ExecutionNode::BusinessRuleTask(t) => {
            (t.produces_placeholder.as_deref(), t.next.as_str())
        }
        _ => return,
    };

    // Advance to next node so the tick walker doesn't re-dispatch.
    instance.current_node_id = Some(next.to_owned());

    // Bind placeholder value from result bindings.
    if let Some(placeholder_name) = produces {
        let value = extract_binding_value(input);
        let mut placeholders: HashMap<String, serde_json::Value> = instance
            .placeholder_values
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        placeholders.insert(placeholder_name.to_owned(), value);
        instance.placeholder_values =
            serde_json::to_value(&placeholders).ok();
    }
}

/// Extract the primary output value from the result, preferring the
/// "result" binding if present, then falling back to the first binding,
/// then to `outcome_detail`.
fn extract_binding_value(input: &ProcessAdvanceInput) -> serde_json::Value {
    // Look for "result" binding first (ob-poc verb output convention),
    // then first binding, then string-encode outcome_detail.
    let binding = input
        .bindings
        .iter()
        .find(|b| b.name == "result")
        .or_else(|| input.bindings.first());

    if let Some(b) = binding {
        if let Some(tv) = b.value.as_ref() {
            let val: Option<serde_json::Value> = match tv.value.as_ref() {
                Some(ProtoValueKind::StringValue(s)) => {
                    Some(serde_json::Value::String(s.clone()))
                }
                Some(ProtoValueKind::UuidValue(u)) => {
                    if u.value.len() == 16 {
                        let mut arr = [0u8; 16];
                        arr.copy_from_slice(&u.value);
                        Some(serde_json::Value::String(
                            Uuid::from_bytes(arr).to_string(),
                        ))
                    } else {
                        None
                    }
                }
                Some(ProtoValueKind::BoolValue(b)) => Some(serde_json::Value::Bool(*b)),
                Some(ProtoValueKind::IntValue(n)) => Some(serde_json::Value::from(*n)),
                _ => None,
            };
            if let Some(v) = val {
                return v;
            }
        }
    }
    serde_json::Value::String(input.outcome_detail.clone())
}
