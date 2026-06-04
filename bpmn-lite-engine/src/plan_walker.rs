//! Plan walker — advances a `WorkflowExecutionPlan`-based process
//! instance through its nodes, dispatching cross-domain verb
//! invocations over the federated bus (T3).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use bpmn_lite_compiler::dsl::plan::{
    ExecutionNode, SplitExecNode, WorkflowExecutionPlan
};
use bpmn_lite_store::pending::{PendingInvocation, PendingInvocationStore};
use bpmn_lite_store::store::{ProcessStore, TickOperation};
use bpmn_lite_types::types::{ProcessInstance, ProcessState};
use dsl_bus_client::BusClient;
use dsl_bus_protocol::v1::{
    typed_value::Value as ProtoValueKind, InvocationRequest, ResolvedBinding,
    TypedValue as ProtoTypedValue, Uuid as ProtoUuid,
};
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
    pub async fn advance(&self, instance_id: Uuid, owner: &str) -> Result<AdvanceOutcome> {
        let mut instance = self
            .store
            .load_instance(instance_id)
            .await?
            .ok_or_else(|| anyhow!("plan_walker: instance {} not found", instance_id))?;

        let plan_hash = match instance.plan_hash {
            Some(h) => h,
            None => return Ok(AdvanceOutcome::NotRunnable),
        };
        let plan_json = self
            .store
            .load_plan(plan_hash)
            .await?
            .ok_or_else(|| anyhow!("plan_walker: plan hash not found"))?;
        let plan: WorkflowExecutionPlan = serde_json::from_str(&plan_json)?;

        let mut ops = Vec::new();

        if let ProcessState::WaitingOnInvocation { execution_id: child_id, ref node_id } = instance.state {
            if let Some(child_instance) = self.store.load_instance(child_id).await? {
                match child_instance.state {
                    ProcessState::Completed { .. } => {
                        if let Some(ref child_vals) = child_instance.placeholder_values {
                            instance.placeholder_values = Some(child_vals.clone());
                        }
                        if let Some(ExecutionNode::Task(t)) = plan.nodes.get(node_id) {
                            instance.current_node_id = Some(t.next.clone());
                        }
                        instance.state = ProcessState::Running;
                        ops.push(TickOperation::SaveInstance { instance: instance.clone() });
                    }
                    ProcessState::Failed { incident_id } => {
                        instance.state = ProcessState::Failed { incident_id };
                        ops.push(TickOperation::SaveInstance { instance: instance.clone() });
                        self.store.commit_tick(instance_id, &instance.tenant_id, owner, &ops).await?;
                        return Ok(AdvanceOutcome::NotRunnable);
                    }
                    _ => {
                        return Ok(AdvanceOutcome::NotRunnable);
                    }
                }
            } else {
                return Err(anyhow!("plan_walker: waiting on non-existent child instance {}", child_id));
            }
        }

        if !matches!(instance.state, ProcessState::Running) {
            return Ok(AdvanceOutcome::NotRunnable);
        }

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
                ExecutionNode::Start(n) => {
                    instance.current_node_id = Some(n.next.clone());
                }

                ExecutionNode::Split(sp) => {
                    let placeholder_vals =
                        deserialize_placeholder_values(instance.placeholder_values.as_ref());
                    
                    // Phase 1: If split has a plug, execute decision first and wait for result.
                    if let Some(ref plug) = sp.routing_socket {
                        // We check if the plug produces a placeholder and whether it is populated yet.
                        // For the demo: cbu_type_routing produces @cbu-type.
                        let produces_placeholder = if plug.contains("cbu_type_routing") {
                            Some("@cbu-type".to_owned())
                        } else {
                            None
                        };

                        if let Some(ref prod_placeholder) = produces_placeholder {
                            if !placeholder_vals.contains_key(prod_placeholder) {
                                // Not yet populated. Submit callout to the bus!
                                let outcome = self
                                    .dispatch_callout(
                                        &mut instance,
                                        sp.id.clone(),
                                        plug.clone(),
                                        HashMap::new(),
                                        &mut ops,
                                    )
                                    .await?;
                                self.store.commit_tick(instance_id, &instance.tenant_id, owner, &ops).await?;
                                if let AdvanceOutcome::Submitted { .. } = &outcome {
                                    self.bus_client.outbox_notifier().notify();
                                }
                                return Ok(outcome);
                            }
                        }
                    }

                    // Phase 2: If plug is populated or split has no plug, evaluate branches.
                    match evaluate_split(sp, &placeholder_vals) {
                        Ok(next) => {
                            instance.current_node_id = Some(next.to_owned());
                        }
                        Err(reason) => {
                            let incident_id = Uuid::now_v7();
                            instance.state =
                                ProcessState::Failed { incident_id };
                            ops.push(TickOperation::SaveInstance { instance: instance.clone() });
                            self.store.commit_tick(instance_id, &instance.tenant_id, owner, &ops).await?;
                            tracing::error!(
                                instance_id = %instance_id,
                                reason = %reason,
                                "plan_walker: split evaluation miss — instance failed"
                            );
                            return Ok(AdvanceOutcome::NotRunnable);
                        }
                    }
                }

                ExecutionNode::Join(jn) => {
                    instance.current_node_id = Some(jn.next.clone());
                }

                ExecutionNode::Loop(lp) => {
                    let key = crc32(lp.id.as_bytes());
                    let current_val = instance.counters.get(&key).copied().unwrap_or(0);

                    if current_val < lp.ceiling {
                        instance.counters.insert(key, current_val + 1);
                        if let Some(first_body_id) = lp.body.first() {
                            instance.current_node_id = Some(first_body_id.clone());
                        } else {
                            instance.current_node_id = Some(lp.next.clone());
                        }
                    } else {
                        instance.counters.remove(&key);
                        instance.current_node_id = Some(lp.next.clone());
                    }
                }

                ExecutionNode::Task(task) => {
                    let mut is_child = false;
                    let mut child_plan_opt = None;

                    if is_child_workflow_hash(&task.plug) {
                        if let Some(child_hash) = decode_hash(&task.plug) {
                            if let Ok(Some(child_plan_json)) = self.store.load_plan(child_hash).await {
                                if let Ok(child_plan) = serde_json::from_str::<WorkflowExecutionPlan>(&child_plan_json) {
                                    child_plan_opt = Some(child_plan);
                                    is_child = true;
                                }
                            }
                        }
                    }

                    if is_child {
                        let child_plan = child_plan_opt.unwrap();
                        let parent_placeholders = deserialize_placeholder_values(instance.placeholder_values.as_ref());
                        let mut child_vars = HashMap::new();
                        for (k, v) in parent_placeholders {
                            child_vars.insert(k.clone(), v.clone());
                        }

                        let child_id = self.start_process(
                            owner,
                            &child_plan,
                            &instance.tenant_id,
                            child_vars,
                            HashMap::new(),
                        ).await?;

                        if let Some(mut child_inst) = self.store.load_instance(child_id).await? {
                            child_inst.correlation_id = format!("{}:{}", instance.instance_id, task.id);
                            self.store.save_instance(owner, &child_inst).await?;
                        }

                        instance.state = ProcessState::WaitingOnInvocation {
                            execution_id: child_id,
                            node_id: task.id.clone(),
                        };
                        ops.push(TickOperation::SaveInstance { instance: instance.clone() });
                        self.store.commit_tick(instance_id, &instance.tenant_id, owner, &ops).await?;

                        return Ok(AdvanceOutcome::Submitted {
                            callout_id: child_id,
                            node_id: task.id.clone(),
                            verb_fqn: task.plug.clone(),
                        });
                    }

                    let outcome = self
                        .dispatch_callout(
                            &mut instance,
                            task.id.clone(),
                            task.plug.clone(),
                            task.static_args.clone(),
                            &mut ops,
                        )
                        .await?;
                    self.store.commit_tick(instance_id, &instance.tenant_id, owner, &ops).await?;
                    if let AdvanceOutcome::Submitted { .. } = &outcome {
                        self.bus_client.outbox_notifier().notify();
                    }
                    return Ok(outcome);
                }

                ExecutionNode::End(end) => {
                    let now = chrono::Utc::now().timestamp_millis();
                    instance.state = ProcessState::Completed { at: now };
                    instance.current_node_id = Some(end.id.clone());
                    ops.push(TickOperation::SaveInstance { instance: instance.clone() });
                    
                    self.store.commit_tick(instance_id, &instance.tenant_id, owner, &ops).await?;
                    return Ok(AdvanceOutcome::Completed {
                        node_id: end.id.clone(),
                        status: end.status.clone(),
                    });
                }
            }
        }
    }

    /// Dispatch a single callout to the bus.
    async fn dispatch_callout(
        &self,
        instance: &mut ProcessInstance,
        node_id: String,
        fqn: String,
        static_args: HashMap<String, String>,
        ops: &mut Vec<TickOperation>,
    ) -> Result<AdvanceOutcome> {
        let (target_domain, verb_id) = split_verb_fqn(&fqn)?;

        let retry_count = {
            let pv = deserialize_placeholder_values(instance.placeholder_values.as_ref());
            pv.get("__retry_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        };
        const MAX_RETRIES: u64 = 3;
        if retry_count >= MAX_RETRIES {
            let incident_id = Uuid::now_v7();
            instance.state = ProcessState::Failed { incident_id };
            ops.push(TickOperation::SaveInstance { instance: instance.clone() });
            return Ok(AdvanceOutcome::NotRunnable);
        }

        let pending_rows = self.pending_store.list_for_process(instance.instance_id).await?;
        let matching_row = pending_rows.iter().find(|r| r.node_id == node_id);

        let attempt_count = retry_count;

        let (callout_id, idempotency_key) = if let Some(row) = matching_row {
            (row.callout_id, row.idempotency_key)
        } else {
            let callout_id = derive_uuid("callout_id", instance.instance_id, &node_id, attempt_count);
            let idempotency_key = derive_uuid("idempotency_key", instance.instance_id, &node_id, attempt_count);
            (callout_id, idempotency_key)
        };

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

        let pending = PendingInvocation::new(
            callout_id,
            instance.instance_id,
            node_id.clone(),
            target_domain,
            verb_id,
            idempotency_key,
        );
        ops.push(TickOperation::InsertPendingInvocation { pending });

        let payload = req.encode_to_vec();
        let outbox_id = Uuid::now_v7();
        ops.push(TickOperation::InsertOutbox {
            id: outbox_id,
            target_domain: target_domain.to_owned(),
            target_endpoint: "invocation".to_owned(),
            payload,
            idempotency_key,
            callout_id,
        });

        instance.state = ProcessState::WaitingOnSubmission {
            callout_id,
            node_id: node_id.clone(),
        };
        instance.current_node_id = Some(node_id.clone());
        ops.push(TickOperation::SaveInstance { instance: instance.clone() });

        Ok(AdvanceOutcome::Submitted {
            callout_id,
            node_id,
            verb_fqn: fqn,
        })
    }

    /// Start a new plan-based process instance.
    pub async fn start_process(
        &self,
        lease_owner: &str,
        plan: &WorkflowExecutionPlan,
        tenant_id: impl Into<String>,
        initial_variables: HashMap<String, serde_json::Value>,
        expected_preconditions: HashMap<String, String>,
    ) -> Result<Uuid> {
        let platform_regime = std::env::var("BPMN_LITE_REGIME_VERSION").unwrap_or_else(|_| "1.0".to_string());
        if let Some(plan_regime) = &plan.regime_version {
            if plan_regime != &platform_regime {
                return Err(anyhow!("RegimeMismatch: plan built under regime {}, but platform is running {}", plan_regime, platform_regime));
            }
        }

        for (key, expected_val) in &expected_preconditions {
            let norm_key = key.trim_start_matches('@');
            let actual_val_str = initial_variables.get(key)
                .or_else(|| initial_variables.get(norm_key))
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    v => v.to_string().replace('"', ""),
                })
                .unwrap_or_default();

            if actual_val_str != *expected_val {
                return Err(anyhow!("PreconditionConflict: expected {} to be {}, got '{}'", key, expected_val, actual_val_str));
            }
        }

        // 1. Asynchronously preload nested workflow dependencies
        let child_plans = preload_workflow_dependencies(plan, self.store.as_ref()).await?;

        // 2. Populate the in-memory linter registry with child signatures and paths
        let mut stub_reg = bpmn_lite_compiler::dsl::linter::StubPlaceholderRegistry::new().with_demo_bindings();
        for (hash, child_plan) in child_plans {
            // Extract signature
            let mut consumes = Vec::new();
            for slot in child_plan.placeholder_schema.slots.values() {
                if slot.produced_by == "start" || slot.produced_by.is_empty() {
                    consumes.push(slot.name.clone());
                }
            }
            stub_reg.register_workflow(hash.clone(), bpmn_lite_compiler::dsl::linter::BindingDecl {
                produces: None,
                consumes,
                effect_class: Some("idempotent_ensure".into()),
            }, true);

            // Extract child calls
            let mut child_calls = Vec::new();
            for node in child_plan.nodes.values() {
                if let ExecutionNode::Task(t) = node {
                    if is_child_workflow_hash(&t.plug) {
                        child_calls.push(t.plug.clone());
                    }
                }
            }
            stub_reg.register_workflow_child_calls(hash, child_calls);
        }

        let mut registry = bpmn_lite_compiler::dsl::manifest_registry::ManifestPlaceholderRegistry::new(stub_reg);
        let paths = vec![
            "manifests/bpmn-v1.0.0.yaml",
            "manifests/dmn-lite-v1.0.0.yaml",
            "manifests/ob-poc-v1.0.0.yaml",
        ];
        for p in paths {
            let path = std::path::Path::new(p);
            let m = if path.exists() {
                dsl_manifest::Manifest::load_from_path(path)
                    .map_err(|e| anyhow!("Failed to load manifest at {}: {:?}", p, e))?
            } else {
                let alt_path = format!("../{}", p);
                let alt = std::path::Path::new(&alt_path);
                if alt.exists() {
                    dsl_manifest::Manifest::load_from_path(alt)
                        .map_err(|e| anyhow!("Failed to load manifest at {}: {:?}", alt_path, e))?
                } else {
                    return Err(anyhow!("Manifest file not found: {} or {}", p, alt_path));
                }
            };
            registry.import(m);
        }

        let diagnostics = bpmn_lite_compiler::dsl::closure::validate_path_family(plan, &registry);
        if !diagnostics.is_empty() {
            return Err(anyhow!("Path-family validation failed: {:?}", diagnostics));
        }

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
        self.store.save_instance(lease_owner, &instance).await?;
        Ok(instance_id)
    }
}

pub async fn preload_workflow_dependencies(
    root_plan: &WorkflowExecutionPlan,
    store: &dyn ProcessStore,
) -> Result<HashMap<String, WorkflowExecutionPlan>> {
    let mut preloaded = HashMap::new();
    let mut visiting = HashSet::new();

    resolve_deps(root_plan, store, &mut visiting, &mut preloaded).await?;

    Ok(preloaded)
}

fn resolve_deps<'a>(
    plan: &'a WorkflowExecutionPlan,
    store: &'a dyn ProcessStore,
    visiting: &'a mut HashSet<String>,
    preloaded: &'a mut HashMap<String, WorkflowExecutionPlan>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        for node in plan.nodes.values() {
            if let ExecutionNode::Task(t) = node {
                if is_child_workflow_hash(&t.plug) {
                    let child_hash = t.plug.clone();
                    if preloaded.contains_key(&child_hash) {
                        continue;
                    }
                    if !visiting.insert(child_hash.clone()) {
                        return Err(anyhow!("Cyclic workflow dependency detected: {}", child_hash));
                    }

                    let hash_bytes = decode_hash(&child_hash)
                        .ok_or_else(|| anyhow!("Invalid child workflow hash: {}", child_hash))?;

                    if let Some(plan_json) = store.load_plan(hash_bytes).await? {
                        let child_plan: WorkflowExecutionPlan = serde_json::from_str(&plan_json)?;
                        // Recurse
                        resolve_deps(&child_plan, store, visiting, preloaded).await?;
                        preloaded.insert(child_hash.clone(), child_plan);
                    } else {
                        return Err(anyhow!("Child plan not found for hash: {}", child_hash));
                    }

                    visiting.remove(&child_hash);
                }
            }
        }
        Ok(())
    })
}

fn decode_hash(hash: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hash).ok()?;
    if bytes.len() == 32 {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Some(arr)
    } else {
        None
    }
}

fn is_child_workflow_hash(plug: &str) -> bool {
    plug.len() == 64 && plug.chars().all(|c| c.is_ascii_hexdigit())
}

// ── helpers ─────────────────────────────────────────────────────────

fn split_verb_fqn(fqn: &str) -> Result<(&str, &str)> {
    if let Some((domain, local)) = fqn.split_once(':') {
        Ok((domain, local))
    } else {
        Ok(("bpmn", fqn))
    }
}

fn evaluate_split<'a>(
    sp: &'a SplitExecNode,
    placeholder_values: &HashMap<String, serde_json::Value>,
) -> Result<&'a str> {
    for flow in &sp.flows {
        if let Some(ref placeholder) = flow.placeholder {
            if let Some(val) = placeholder_values.get(placeholder) {
                if let Some(ref expected_val) = flow.expected_value {
                    if val.as_str() == Some(expected_val) {
                        return Ok(&flow.next);
                    }
                }
            }
        } else {
            return Ok(&flow.next);
        }
    }
    Err(anyhow!(
        "no split flow matched for split '{}'",
        sp.id
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
            other => other.to_string().replace('"', ""),
        };
        inputs.push(string_binding(k.clone(), string_val));
    }
    inputs
}

fn string_binding(name: String, val: String) -> ResolvedBinding {
    ResolvedBinding {
        name,
        value: Some(ProtoTypedValue {
            value: Some(ProtoValueKind::StringValue(val)),
            type_name: "String".to_string(),
        }),
    }
}

fn uuid_to_proto(id: Uuid) -> ProtoUuid {
    let bytes = id.into_bytes().to_vec();
    ProtoUuid { value: bytes }
}

fn derive_uuid(salt: &str, instance_id: Uuid, node_id: &str, attempt: u64) -> Uuid {
    let mut hasher = blake3::Hasher::new();
    hasher.update(salt.as_bytes());
    hasher.update(instance_id.as_bytes());
    hasher.update(node_id.as_bytes());
    hasher.update(&attempt.to_le_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    Uuid::from_bytes(bytes)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ── unit tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use bpmn_lite_compiler::dsl::plan::{EndExecNode, PlaceholderSchema, StartExecNode};
    use bpmn_lite_store::pending::MemoryPendingInvocationStore;
    use bpmn_lite_store::store_memory::MemoryStore;

    /// Minimal plan: Start → End.
    fn simple_plan(workflow_id: &str) -> WorkflowExecutionPlan {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "start".to_owned(),
            ExecutionNode::Start(StartExecNode {
                id: "start".to_owned(),
                next: "end".to_owned(),
                span: None,
            }),
        );
        nodes.insert(
            "end".to_owned(),
            ExecutionNode::End(EndExecNode {
                id: "end".to_owned(),
                status: "Operational".to_owned(),
                span: None,
            }),
        );
        WorkflowExecutionPlan {
            workflow_id: workflow_id.to_owned(),
            nodes,
            start_node: "start".to_owned(),
            placeholder_schema: PlaceholderSchema::default(),
            closure_manifest: None,
            regime_version: None,
            mathematically_proved: true,
            unsafe_breeches: vec![],
            compiled_bytecode: None,
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
            .start_process("default", &plan, "t1", HashMap::new(), HashMap::new())
            .await
            .unwrap();

        let outcome = walker.advance(id, "default").await.unwrap();
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
            .start_process("default", &plan, "t1", HashMap::new(), HashMap::new())
            .await
            .unwrap();

        let mut inst = store.load_instance(id).await.unwrap().unwrap();
        inst.state = ProcessState::Completed { at: 0 };
        store.save_instance("default", &inst).await.unwrap();

        let outcome = walker.advance(id, "default").await.unwrap();
        assert!(matches!(outcome, AdvanceOutcome::NotRunnable));
    }

    #[tokio::test]
    async fn no_plan_hash_returns_not_runnable() {
        let store = Arc::new(MemoryStore::new());
        let (_pending, walker) = memory_walker_no_bus(store.clone()).await;
        let plan = simple_plan("test-wf3");
        let id = walker
            .start_process("default", &plan, "t1", HashMap::new(), HashMap::new())
            .await
            .unwrap();

        let mut inst = store.load_instance(id).await.unwrap().unwrap();
        inst.plan_hash = None;
        store.save_instance("default", &inst).await.unwrap();

        let outcome = walker.advance(id, "default").await.unwrap();
        assert!(matches!(outcome, AdvanceOutcome::NotRunnable));
    }

    #[test]
    fn test_t1_3_deterministic_idempotency_key() {
        let instance_id = Uuid::now_v7();
        let node_id = "service-task-1";
        let attempt_count = 2;

        let key1 = derive_uuid("idempotency_key", instance_id, node_id, attempt_count);
        let key2 = derive_uuid("idempotency_key", instance_id, node_id, attempt_count);

        assert_eq!(key1, key2, "Derived idempotency keys must be identical");

        let callout1 = derive_uuid("callout_id", instance_id, node_id, attempt_count);
        let callout2 = derive_uuid("callout_id", instance_id, node_id, attempt_count);

        assert_eq!(callout1, callout2, "Derived callout IDs must be identical");
        assert_ne!(key1, callout1, "Different salts must produce different UUIDs");
    }

    #[test]
    fn test_t2_4_attempt_count_semantics() {
        let instance_id = Uuid::now_v7();
        let node_id = "service-task-1";

        let attempt1_retry0 = derive_uuid("idempotency_key", instance_id, node_id, 0);
        let attempt1_retry0_replay = derive_uuid("idempotency_key", instance_id, node_id, 0);
        assert_eq!(attempt1_retry0, attempt1_retry0_replay, "Crash-replay must yield byte-identical keys");

        let attempt2_retry1 = derive_uuid("idempotency_key", instance_id, node_id, 1);
        assert_ne!(attempt1_retry0, attempt2_retry1, "Deliberate retry must yield a new key");
    }
}
