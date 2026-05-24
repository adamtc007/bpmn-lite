//! Plan walker — advances a `WorkflowExecutionPlan`-based process
//! instance through its nodes, dispatching cross-domain verb
//! invocations over the federated bus (T3).
//!
//! Entry points:
//! - [`PlanWalker::advance`] — called from the tick loop for every
//!   Running plan-based instance.
//! - [`PlanWalker::start_process`] — creates a new plan-based
//!   process instance ready for the tick loop.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use bpmn_lite_compiler::dsl::plan::{ExecutionNode, GatewayExecNode, WorkflowExecutionPlan};
use bpmn_lite_store::pending::{PendingInvocation, PendingInvocationStore};
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_types::types::{ProcessInstance, ProcessState};
use dsl_bus_client::BusClient;
use dsl_bus_protocol::v1::{
    typed_value::Value as ProtoValueKind, InvocationRequest, ResolvedBinding,
    TypedValue as ProtoTypedValue, Uuid as ProtoUuid,
};
use dsl_bus_storage::{insert_outbox, BusEndpoint, OutboxEntry};
use bpmn_lite_types::session_stack::SessionStackState;
use prost::Message;
use uuid::Uuid;

/// Result of one `advance()` cycle.
#[must_use]
pub enum AdvanceOutcome {
    /// Reached a callout node — submitted to bus, now WaitingOnSubmission.
    Submitted {
        callout_id: Uuid,
        node_id: String,
        verb_fqn: String,
    },
    /// Reached an end event — process is now Completed.
    Completed { node_id: String, status: String },
    /// Process is not in a walkable state (already waiting/failed/etc).
    NotRunnable,
}

/// Walks a `WorkflowExecutionPlan`-based process instance.
pub struct PlanWalker {
    store: Arc<dyn ProcessStore>,
    pending_store: Arc<dyn PendingInvocationStore>,
    bus_client: Arc<BusClient>,
}

impl PlanWalker {
    pub fn new(
        store: Arc<dyn ProcessStore>,
        pending_store: Arc<dyn PendingInvocationStore>,
        bus_client: Arc<BusClient>,
    ) -> Self {
        Self {
            store,
            pending_store,
            bus_client,
        }
    }

    /// Advance instance `instance_id` until the next callout or end event.
    pub async fn advance(&self, instance_id: Uuid) -> Result<AdvanceOutcome> {
        let mut instance = self
            .store
            .load_instance(instance_id)
            .await?
            .ok_or_else(|| anyhow!("plan_walker: instance {} not found", instance_id))?;

        if !matches!(instance.state, ProcessState::Running) || instance.plan_hash.is_none() {
            return Ok(AdvanceOutcome::NotRunnable);
        }

        let plan_hash = instance.plan_hash.unwrap();
        let plan_json = self
            .store
            .load_plan(plan_hash)
            .await?
            .ok_or_else(|| anyhow!("plan_walker: plan hash not found"))?;
        let plan: WorkflowExecutionPlan = serde_json::from_str(&plan_json)?;

        loop {
            let current = instance
                .current_node_id
                .clone()
                .ok_or_else(|| anyhow!("plan_walker: current_node_id is None"))?;

            let node = plan
                .nodes
                .get(&current)
                .ok_or_else(|| anyhow!("plan_walker: node '{}' not in plan", current))?;

            match node {
                ExecutionNode::StartEvent(n) => {
                    instance.current_node_id = Some(n.next.clone());
                    self.store.save_instance(&instance).await?;
                }

                ExecutionNode::ExclusiveGateway(gw) => {
                    let placeholder_vals =
                        deserialize_placeholder_values(instance.placeholder_values.as_ref());
                    match evaluate_gateway(gw, &placeholder_vals) {
                        Ok(next) => {
                            instance.current_node_id = Some(next.to_owned());
                            self.store.save_instance(&instance).await?;
                        }
                        Err(reason) => {
                            instance.state =
                                ProcessState::Failed { incident_id: Uuid::now_v7() };
                            self.store.save_instance(&instance).await?;
                            tracing::error!(
                                instance_id = %instance_id,
                                reason = %reason,
                                "plan_walker: gateway miss — instance failed"
                            );
                            return Ok(AdvanceOutcome::NotRunnable);
                        }
                    }
                }

                ExecutionNode::ServiceTask(task) => {
                    return self
                        .dispatch_callout(
                            &mut instance,
                            task.id.clone(),
                            task.verb_fqn.clone(),
                            task.static_args.clone(),
                        )
                        .await;
                }

                ExecutionNode::BusinessRuleTask(task) => {
                    return self
                        .dispatch_callout(
                            &mut instance,
                            task.id.clone(),
                            task.decision_id.clone(),
                            HashMap::new(),
                        )
                        .await;
                }

                ExecutionNode::EndEvent(end) => {
                    let now = chrono::Utc::now().timestamp_millis();
                    instance.state = ProcessState::Completed { at: now };
                    instance.current_node_id = Some(end.id.clone());
                    self.store.save_instance(&instance).await?;
                    return Ok(AdvanceOutcome::Completed {
                        node_id: end.id.clone(),
                        status: end.status.clone(),
                    });
                }
            }
        }
    }

    /// Dispatch a single callout (ServiceTask or BusinessRuleTask) to the bus.
    /// On transport failure retries once with a fresh idempotency_key; if the
    /// second attempt also fails the instance is marked `Failed`.
    async fn dispatch_callout(
        &self,
        instance: &mut ProcessInstance,
        node_id: String,
        fqn: String,
        static_args: HashMap<String, String>,
    ) -> Result<AdvanceOutcome> {
        let (target_domain, verb_id) = split_verb_fqn(&fqn)?;

        // Retry cap for transient failures (T3.6). We store the attempt
        // count in placeholder_values under the reserved "__retry_count"
        // key so no schema migration is needed.
        let retry_count = {
            let pv = deserialize_placeholder_values(instance.placeholder_values.as_ref());
            pv.get("__retry_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        };
        const MAX_RETRIES: u64 = 3;
        if retry_count >= MAX_RETRIES {
            instance.state = ProcessState::Failed { incident_id: Uuid::now_v7() };
            self.store.save_instance(instance).await?;
            return Ok(AdvanceOutcome::NotRunnable);
        }

        let callout_id = Uuid::now_v7();
        let idempotency_key = Uuid::now_v7();

        let placeholder_vals =
            deserialize_placeholder_values(instance.placeholder_values.as_ref());
        let inputs = build_inputs(&static_args, &placeholder_vals);

        let req = InvocationRequest {
            idempotency_key: Some(uuid_to_proto(idempotency_key)),
            verb_id: verb_id.to_owned(),
            inputs,
            authority: None,
            source_domain: "bpmn-lite".to_owned(),
            catalogue_version: "v1.0.0".to_owned(),
            snapshot_pin: None,
            result_callback_endpoint: String::new(),
            timeout_at: None,
        };

        // Insert pending invocation row so the advancer can look it up.
        let pending = PendingInvocation::new(
            callout_id,
            instance.instance_id,
            node_id.clone(),
            target_domain,
            verb_id,
            idempotency_key,
        );
        self.pending_store.insert(pending).await?;

        // Write outbox row — sender will dispatch to the peer.
        let payload = req.encode_to_vec();
        let entry = OutboxEntry::new_pending(
            Uuid::now_v7(),
            target_domain.to_owned(),
            BusEndpoint::Invocation,
            payload,
            idempotency_key,
        )
        .with_callout_id(callout_id);
        if let Err(e) = insert_outbox(self.bus_client.pool(), &entry).await {
            // Increment retry counter and surface as transient failure.
            let mut pv = deserialize_placeholder_values(instance.placeholder_values.as_ref());
            pv.insert(
                "__retry_count".to_owned(),
                serde_json::Value::Number((retry_count + 1).into()),
            );
            instance.placeholder_values = serde_json::to_value(&pv).ok();
            instance.state = ProcessState::Running; // tick will retry
            self.store.save_instance(instance).await?;
            tracing::warn!(
                instance_id = %instance.instance_id,
                node_id = %node_id,
                retry_count = retry_count + 1,
                error = %e,
                "plan_walker: outbox insert failed; will retry on next tick"
            );
            return Ok(AdvanceOutcome::NotRunnable);
        }
        self.bus_client.outbox_notifier().notify();

        instance.state = ProcessState::WaitingOnSubmission {
            callout_id,
            node_id: node_id.clone(),
        };
        instance.current_node_id = Some(node_id.clone());
        self.store.save_instance(instance).await?;

        Ok(AdvanceOutcome::Submitted {
            callout_id,
            node_id,
            verb_fqn: fqn,
        })
    }

    /// Start a new plan-based process instance.
    pub async fn start_process(
        &self,
        plan: &WorkflowExecutionPlan,
        tenant_id: impl Into<String>,
        initial_variables: HashMap<String, serde_json::Value>,
    ) -> Result<Uuid> {
        let plan_json = serde_json::to_string(plan)?;
        let hash = *blake3::hash(plan_json.as_bytes()).as_bytes();
        self.store.store_plan(hash, &plan_json).await?;

        let instance_id = Uuid::now_v7();
        let placeholder_values = if initial_variables.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&initial_variables)?)
        };

        let instance = ProcessInstance {
            instance_id,
            tenant_id: tenant_id.into(),
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
        self.store.save_instance(&instance).await?;
        Ok(instance_id)
    }
}

// ── helpers ─────────────────────────────────────────────────────────

fn split_verb_fqn(fqn: &str) -> Result<(&str, &str)> {
    fqn.split_once(':').ok_or_else(|| {
        anyhow!(
            "verb_fqn missing domain prefix (expected 'domain:verb.id'): {}",
            fqn
        )
    })
}

fn evaluate_gateway<'a>(
    gw: &'a GatewayExecNode,
    placeholder_values: &HashMap<String, serde_json::Value>,
) -> Result<&'a str> {
    for flow in &gw.flows {
        if let Some(val) = placeholder_values.get(&flow.placeholder) {
            if val.as_str() == Some(&flow.expected_value) {
                return Ok(&flow.next);
            }
        }
    }
    Err(anyhow!(
        "no gateway flow matched for gateway '{}'",
        gw.id
    ))
}

fn deserialize_placeholder_values(
    raw: Option<&serde_json::Value>,
) -> HashMap<String, serde_json::Value> {
    raw.and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn build_inputs(
    static_args: &HashMap<String, String>,
    placeholder_values: &HashMap<String, serde_json::Value>,
) -> Vec<ResolvedBinding> {
    let mut inputs = Vec::new();
    for (k, v) in static_args {
        inputs.push(string_binding(k.clone(), v.clone()));
    }
    for (k, v) in placeholder_values {
        let string_val = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        inputs.push(string_binding(k.clone(), string_val));
    }
    inputs
}

fn string_binding(name: String, value: String) -> ResolvedBinding {
    ResolvedBinding {
        name,
        value: Some(ProtoTypedValue {
            value: Some(ProtoValueKind::StringValue(value)),
            type_name: "string".to_owned(),
        }),
    }
}

fn uuid_to_proto(id: Uuid) -> ProtoUuid {
    ProtoUuid {
        value: id.as_bytes().to_vec(),
    }
}

// ── unit tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_compiler::dsl::plan::{EndExecNode, PlaceholderSchema, StartExecNode};
    use bpmn_lite_store::pending::MemoryPendingInvocationStore;
    use bpmn_lite_store::store_memory::MemoryStore;

    /// Minimal plan: StartEvent → EndEvent.
    fn simple_plan(workflow_id: &str) -> WorkflowExecutionPlan {
        let mut nodes = HashMap::new();
        nodes.insert(
            "start".to_owned(),
            ExecutionNode::StartEvent(StartExecNode {
                id: "start".to_owned(),
                next: "end".to_owned(),
            }),
        );
        nodes.insert(
            "end".to_owned(),
            ExecutionNode::EndEvent(EndExecNode {
                id: "end".to_owned(),
                status: "Operational".to_owned(),
            }),
        );
        WorkflowExecutionPlan {
            workflow_id: workflow_id.to_owned(),
            nodes,
            start_node: "start".to_owned(),
            placeholder_schema: PlaceholderSchema::default(),
        }
    }

    async fn memory_walker_no_bus(
        store: Arc<MemoryStore>,
    ) -> (Arc<MemoryPendingInvocationStore>, PlanWalker) {
        let pending = Arc::new(MemoryPendingInvocationStore::new());
        let walker = build_no_callout(store.clone(), pending.clone()).await;
        (pending, walker)
    }

    async fn build_no_callout(
        store: Arc<MemoryStore>,
        pending: Arc<MemoryPendingInvocationStore>,
    ) -> PlanWalker {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://localhost/plan_walker_test_fake",
        )
        .unwrap();
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

    #[tokio::test]
    async fn start_event_then_end_event_completes() {
        let store = Arc::new(MemoryStore::new());
        let (_pending, walker) = memory_walker_no_bus(store.clone()).await;
        let plan = simple_plan("test-wf");
        let id = walker
            .start_process(&plan, "t1", HashMap::new())
            .await
            .unwrap();

        let outcome = walker.advance(id).await.unwrap();
        assert!(
            matches!(outcome, AdvanceOutcome::Completed { .. }),
            "expected Completed"
        );
        let inst = store.load_instance(id).await.unwrap().unwrap();
        assert!(matches!(inst.state, ProcessState::Completed { .. }));
    }

    #[tokio::test]
    async fn not_runnable_for_non_running_instance() {
        let store = Arc::new(MemoryStore::new());
        let (_pending, walker) = memory_walker_no_bus(store.clone()).await;
        let plan = simple_plan("test-wf2");
        let id = walker
            .start_process(&plan, "t1", HashMap::new())
            .await
            .unwrap();

        let mut inst = store.load_instance(id).await.unwrap().unwrap();
        inst.state = ProcessState::Completed { at: 0 };
        store.save_instance(&inst).await.unwrap();

        let outcome = walker.advance(id).await.unwrap();
        assert!(matches!(outcome, AdvanceOutcome::NotRunnable));
    }

    #[tokio::test]
    async fn no_plan_hash_returns_not_runnable() {
        let store = Arc::new(MemoryStore::new());
        let (_pending, walker) = memory_walker_no_bus(store.clone()).await;
        let plan = simple_plan("test-wf3");
        let id = walker
            .start_process(&plan, "t1", HashMap::new())
            .await
            .unwrap();

        // Clear plan_hash so advance treats it as a bytecode instance.
        let mut inst = store.load_instance(id).await.unwrap().unwrap();
        inst.plan_hash = None;
        store.save_instance(&inst).await.unwrap();

        let outcome = walker.advance(id).await.unwrap();
        assert!(matches!(outcome, AdvanceOutcome::NotRunnable));
    }
}
