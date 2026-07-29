// authoring dep removed in Phase 2.7 — see lib.rs
use crate::runtime_context::{RuntimeContext, SystemRuntimeContext};
use anyhow::{anyhow, Result};
use bpmn_lite_compiler::{parse_bpmn_with_meta, verify_bytecode};
use bpmn_lite_kernel::{apply as apply_kernel, DeterministicContext};
use bpmn_lite_store::store::WorkflowStore;
use bpmn_lite_store::CommitOutcome;
use bpmn_lite_types::RuntimeEvent;
use bpmn_lite_types::session_stack::SessionStackState;
use bpmn_lite_types::*;
use ffi_dispatcher::FfiDispatcher;
use ffi_types::{FfiCall, FfiIncidentClass, FfiResult};
use futures::{stream, StreamExt};
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const MAX_BPMN_XML_BYTES: usize = 2_000_000;
const MAX_IR_NODES: usize = 2_048;
const MAX_IR_EDGES: usize = 4_096;
const MAX_BYTECODE_INSTRUCTIONS: usize = 10_000;
const MAX_TASK_MANIFEST: usize = 512;
const DEFAULT_TRANSITION_LEASE_MS: u64 = 5_000;
const DEFAULT_WORKER_ID: &str = "engine-default-worker";
const DEFAULT_JOB_LEASE_MS: u64 = 300_000;
const DEFAULT_MESSAGE_TTL_MS: u64 = 300_000;
const DEFAULT_MESSAGE_CLAIM_MS: u64 = 30_000;
const DEFAULT_ARTIFACT_CACHE_CAPACITY: usize = 128;
const MAX_SCHEDULER_IN_FLIGHT: usize = 32;

#[derive(Default)]
struct ArtifactCache {
    entries: BTreeMap<ArtifactHash, Arc<ExecutableWorkflow>>,
    recency: VecDeque<ArtifactHash>,
}

impl ArtifactCache {
    fn get(&mut self, hash: ArtifactHash) -> Option<Arc<ExecutableWorkflow>> {
        let artifact = self.entries.get(&hash).cloned()?;
        self.recency.retain(|candidate| *candidate != hash);
        self.recency.push_back(hash);
        Some(artifact)
    }

    fn insert(&mut self, hash: ArtifactHash, artifact: Arc<ExecutableWorkflow>, capacity: usize) {
        if capacity == 0 {
            return;
        }
        self.entries.insert(hash, artifact);
        self.recency.retain(|candidate| *candidate != hash);
        self.recency.push_back(hash);
        while self.entries.len() > capacity {
            if let Some(evicted) = self.recency.pop_front() {
                self.entries.remove(&evicted);
            }
        }
    }
}

/// BpmnLiteEngine is the top-level facade that wires together the compiler,
/// VM, and store. gRPC handlers delegate to this.
pub struct BpmnLiteEngine {
    store: Arc<dyn WorkflowStore>,
    tenant_id: TenantId,
    transition_owner: String,
    transition_lease_ms: u64,
    runtime_context: Arc<dyn RuntimeContext>,
    artifact_cache: Arc<Mutex<ArtifactCache>>,
    artifact_cache_capacity: usize,
    /// Optional in-process FFI dispatcher. None = ExecFfi produces an incident.
    ffi_dispatcher: Option<Arc<FfiDispatcher>>,
}

/// Parameters for starting a new process instance.
pub struct StartParams {
    pub process_key: String,
    pub bytecode_version: [u8; 32],
    pub domain_payload: String,
    pub domain_payload_hash: [u8; 32],
    pub correlation_id: String,
    pub session_stack: SessionStackState,
    pub entry_id: Uuid,
    pub runbook_id: Uuid,
    pub expected_preconditions: Option<std::collections::HashMap<String, String>>,
}

/// Result of compiling BPMN XML.
#[derive(Debug, Clone)]
pub struct CompileResult {
    pub bytecode_version: [u8; 32],
    pub task_types: Vec<String>,
    pub diagnostics: Vec<String>,
    /// Maps FlagKey (u32) → symbolic data-object name.
    /// Clients use this to address flags by name via `orch_flags` keys like `"flag_<N>"`.
    pub flag_symbol_table: std::collections::BTreeMap<u32, String>,
}

/// Snapshot of a process instance for the Inspect RPC.
#[derive(Debug, Clone)]
pub struct ProcessInspection {
    pub instance_id: Uuid,
    pub tenant_id: String,
    pub process_key: String,
    pub bytecode_version: [u8; 32],
    pub domain_payload_hash: [u8; 32],
    pub state: ProcessState,
    pub fibers: Vec<FiberInspection>,
    pub incidents: Vec<Incident>,
    pub current_node_id: Option<String>,
    pub placeholder_values: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct FiberInspection {
    pub fiber_id: Uuid,
    pub pc: Addr,
    pub wait_state: WaitState,
    pub stack_depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryIssue {
    pub instance_id: Uuid,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub tenants_scanned: usize,
    pub instances_verified: usize,
    pub timers_applied: u32,
    pub interrupted_effects: usize,
    pub stale_jobs_reclaimed: u32,
}

fn enforce_ir_limits(ir: &bpmn_lite_compiler::IRGraph) -> Result<()> {
    if ir.node_count() > MAX_IR_NODES {
        return Err(anyhow!(
            "BPMN graph has too many nodes: {} > {}",
            ir.node_count(),
            MAX_IR_NODES
        ));
    }
    if ir.edge_count() > MAX_IR_EDGES {
        return Err(anyhow!(
            "BPMN graph has too many sequence flows: {} > {}",
            ir.edge_count(),
            MAX_IR_EDGES
        ));
    }
    Ok(())
}

fn enforce_program_limits(program: &CompiledProgram) -> Result<()> {
    if program.program().len() > MAX_BYTECODE_INSTRUCTIONS {
        return Err(anyhow!(
            "compiled bytecode has too many instructions: {} > {}",
            program.program().len(),
            MAX_BYTECODE_INSTRUCTIONS
        ));
    }
    if program.task_manifest().len() > MAX_TASK_MANIFEST {
        return Err(anyhow!(
            "compiled task manifest has too many task types: {} > {}",
            program.task_manifest().len(),
            MAX_TASK_MANIFEST
        ));
    }
    Ok(())
}

fn enforce_artifact_limits(artifact: &ExecutableWorkflow) -> Result<()> {
    let envelope = artifact.envelope();
    if envelope.instructions().len() > MAX_BYTECODE_INSTRUCTIONS {
        return Err(anyhow!(
            "compiled bytecode has too many instructions: {} > {}",
            envelope.instructions().len(),
            MAX_BYTECODE_INSTRUCTIONS
        ));
    }
    if envelope.metadata().task_manifest().len() > MAX_TASK_MANIFEST {
        return Err(anyhow!(
            "compiled task manifest has too many task types: {} > {}",
            envelope.metadata().task_manifest().len(),
            MAX_TASK_MANIFEST
        ));
    }
    Ok(())
}

impl BpmnLiteEngine {
    pub fn new(store: Arc<dyn WorkflowStore>) -> Self {
        Self::new_with_tenant(store, TenantId::default())
    }

    pub fn new_with_tenant(store: Arc<dyn WorkflowStore>, tenant_id: TenantId) -> Self {
        Self::new_with_runtime_context(store, tenant_id, Arc::new(SystemRuntimeContext))
    }

    pub fn new_with_runtime_context(
        store: Arc<dyn WorkflowStore>,
        tenant_id: TenantId,
        runtime_context: Arc<dyn RuntimeContext>,
    ) -> Self {
        let transition_owner = format!("engine-{}", runtime_context.new_id());
        Self {
            store,
            tenant_id,
            transition_owner,
            transition_lease_ms: DEFAULT_TRANSITION_LEASE_MS,
            runtime_context,
            artifact_cache: Arc::new(Mutex::new(ArtifactCache::default())),
            artifact_cache_capacity: DEFAULT_ARTIFACT_CACHE_CAPACITY,
            ffi_dispatcher: None,
        }
    }

    /// Attach an FFI dispatcher. Call before starting instances that contain
    /// `ExecFfi` instructions; without a dispatcher ExecFfi produces an incident.
    pub fn with_ffi_dispatcher(mut self, dispatcher: Arc<FfiDispatcher>) -> Self {
        self.ffi_dispatcher = Some(dispatcher);
        self
    }

    pub fn for_tenant(&self, tenant_id: TenantId) -> Self {
        Self {
            store: self.store.clone(),
            tenant_id,
            transition_owner: format!("engine-{}", self.runtime_context.new_id()),
            transition_lease_ms: self.transition_lease_ms,
            runtime_context: self.runtime_context.clone(),
            artifact_cache: self.artifact_cache.clone(),
            artifact_cache_capacity: self.artifact_cache_capacity,
            ffi_dispatcher: self.ffi_dispatcher.clone(),
        }
    }

    pub fn store(&self) -> Arc<dyn WorkflowStore> {
        self.store.clone()
    }

    async fn load_artifact_cached(&self, hash: ArtifactHash) -> Result<Arc<ExecutableWorkflow>> {
        if let Some(artifact) = self
            .artifact_cache
            .lock()
            .map_err(|_| anyhow!("artifact cache lock poisoned"))?
            .get(hash)
        {
            return Ok(artifact);
        }
        let artifact = Arc::new(
            self.store
                .load_artifact(hash)
                .await?
                .ok_or_else(|| anyhow!("artifact {} not found", hex::encode(hash.as_bytes())))?,
        );
        self.artifact_cache
            .lock()
            .map_err(|_| anyhow!("artifact cache lock poisoned"))?
            .insert(hash, artifact.clone(), self.artifact_cache_capacity);
        Ok(artifact)
    }

    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    fn logical_time(&self) -> Result<i64> {
        self.runtime_context
            .logical_time()
            .map_err(|error| anyhow!(error))
    }

    fn ensure_loaded_instance_belongs_to_tenant(
        &self,
        instance: &ProcessInstance,
        instance_id: Uuid,
    ) -> Result<()> {
        if instance.tenant_id != self.tenant_id.as_str() {
            return Err(anyhow!("Instance not found: {}", instance_id));
        }
        Ok(())
    }

    #[tracing::instrument(
        name = "workflow.transition",
        skip(self, command),
        fields(tenant_id = %self.tenant_id, instance_id = %instance_id)
    )]
    async fn apply_and_commit_command(&self, instance_id: Uuid, command: Command) -> Result<()> {
        let owner = self.transition_owner.clone();
        let work = self
            .store
            .claim_work_for_transition(
                &self.tenant_id,
                instance_id,
                &owner,
                self.transition_lease_ms,
                command,
            )
            .await?
            .ok_or_else(|| anyhow!("instance transition is leased or missing: {instance_id}"))?;
        let result = async {
            let (claim, snapshot, command) = work.into_parts();
            let instance = snapshot.instance();
            self.ensure_loaded_instance_belongs_to_tenant(instance, instance_id)?;
            let artifact = self
                .load_artifact_cached(ArtifactHash::from_bytes(instance.bytecode_version))
                .await?;
            let revision = claim.expected_revision().saturating_add(1);
            let logical_time = u64::try_from(self.logical_time()?)
                .map_err(|_| anyhow!("logical time cannot be negative"))?;
            let context = DeterministicContext::new(
                logical_time,
                EffectId::for_transition(instance_id, revision, 0).as_uuid(),
                revision,
            );
            let transition = apply_kernel(&artifact, &snapshot, &command, &context)
                .map_err(|error| anyhow!(error))?;
            self.store
                .commit_transition(&claim, &transition)
                .await
                .map_err(|error| anyhow!(error))?;
            Ok(())
        }
        .await;
        let release = self
            .store
            .release_instance_transition(&self.tenant_id, instance_id, &owner)
            .await;
        match (result, release) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
        }
    }

    /// Dispatch committed effects without holding an instance or database
    /// transaction, then apply durable inbox responses through the normal
    /// fenced command path.
    #[tracing::instrument(
        name = "workflow.effect.dispatch_batch",
        skip(self),
        fields(tenant_id = %self.tenant_id, owner, limit)
    )]
    pub async fn dispatch_pending_effects(
        &self,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> Result<usize> {
        let logical_time = u64::try_from(self.logical_time()?)
            .map_err(|_| anyhow!("effect dispatcher clock cannot be negative"))?;
        let claimed = self
            .store
            .claim_pending_effects(&self.tenant_id, owner, logical_time, limit, lease_ms)
            .await?;
        let mut dispatched = 0usize;
        for claimed_effect in claimed {
            let DurableEffect::Invoke {
                effect_id,
                template_id,
                input,
                operation,
                ..
            } = claimed_effect.effect()
            else {
                self.store.release_effect_claim(&claimed_effect).await?;
                continue;
            };
            let response = if let Some(dispatcher) = &self.ffi_dispatcher {
                match dispatcher
                    .dispatch(FfiCall {
                        invocation_id: effect_id.as_uuid(),
                        template_id: *template_id,
                        tenant_id: self.tenant_id.to_string(),
                        process_instance_id: claimed_effect.instance_id(),
                        caller_task_id: operation.clone(),
                        input_payload: input.clone(),
                    })
                    .await
                {
                    Ok(FfiResult::Success {
                        output_payload,
                        new_domain_payload,
                        ..
                    }) => EffectResponse::FfiSuccess {
                        output_payload,
                        new_domain_payload,
                    },
                    Ok(FfiResult::NoMatch { .. }) => EffectResponse::FfiNoMatch,
                    Ok(FfiResult::Incident {
                        error_class,
                        message,
                        ..
                    }) => {
                        let error_class = ffi_incident_class_to_error_class(error_class);
                        if matches!(error_class, ErrorClass::Transient) {
                            let Some(response) = self
                                .schedule_transient_effect(
                                    &claimed_effect,
                                    logical_time,
                                    message.clone(),
                                )
                                .await?
                            else {
                                continue;
                            };
                            response
                        } else {
                            EffectResponse::Failed {
                                error_class,
                                message,
                                attempt: claimed_effect.attempt(),
                            }
                        }
                    }
                    Err(error) => {
                        let Some(response) = self
                            .schedule_transient_effect(
                                &claimed_effect,
                                logical_time,
                                format!("FFI dispatch error: {error}"),
                            )
                            .await?
                        else {
                            continue;
                        };
                        response
                    }
                }
            } else {
                EffectResponse::Failed {
                    error_class: ErrorClass::ContractViolation,
                    message: "ExecFfi reached dispatcher boundary with no FfiDispatcher configured"
                        .to_string(),
                    attempt: claimed_effect.attempt(),
                }
            };
            self.store
                .record_effect_response(&claimed_effect, &response)
                .await?;
            dispatched = dispatched.saturating_add(1);
        }
        self.apply_pending_effect_responses(limit).await?;
        Ok(dispatched)
    }

    async fn schedule_transient_effect(
        &self,
        effect: &ClaimedEffect,
        logical_time: u64,
        message: String,
    ) -> Result<Option<EffectResponse>> {
        // R5 (F4, §31 treatment): the retry ceiling is artifact-resident —
        // read from the effect's instance's pinned artifact, retiring the
        // engine's hardcoded RetryPolicy::new(1,5,1_000,60_000). Fail closed
        // on a missing instance/artifact (same rule as the guard budget):
        // guessing a policy for an unresolvable instance is the hazard, not
        // the failure.
        let instance = self
            .store
            .load_instance(effect.tenant_id(), effect.instance_id())
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "transient effect retry for unknown instance {}: refusing to guess a policy",
                    effect.instance_id()
                )
            })?;
        let artifact_hash = ArtifactHash::from_bytes(instance.bytecode_version);
        let policy = self
            .store
            .load_artifact(artifact_hash)
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "transient effect retry: pinned artifact is absent for instance {}; \
                     refusing to guess a policy",
                    effect.instance_id()
                )
            })?
            .envelope()
            .metadata()
            .default_retry_policy();
        if effect.policy_version() != policy.version() {
            return Err(anyhow!(
                "effect retry policy version {} is unavailable",
                effect.policy_version()
            ));
        }
        let decision = policy.decision(
            effect.attempt(),
            None,
            &ErrorClass::Transient,
            logical_time,
            effect.effect().effect_id().as_uuid(),
        );
        match decision {
            RetryDecision::At { .. } => {
                self.store
                    .schedule_effect_retry(effect, decision, &message)
                    .await?;
                Ok(None)
            }
            RetryDecision::Exhausted => Ok(Some(EffectResponse::Failed {
                error_class: ErrorClass::Transient,
                message: format!("effect retry budget exhausted: {message}"),
                // Mirrors the store's own `schedule_effect_retry` handling
                // of `RetryDecision::Exhausted` (attempt + 1: the attempt
                // that just exhausted the budget counts).
                attempt: effect.attempt().saturating_add(1),
            })),
            RetryDecision::Terminal => Ok(Some(EffectResponse::Failed {
                error_class: ErrorClass::ContractViolation,
                message,
                // Mirrors the store's own handling of `RetryDecision::Terminal`
                // (attempt unchanged: this error class was never retriable,
                // so no new attempt was consumed deciding that).
                attempt: effect.attempt(),
            })),
        }
    }

    async fn apply_pending_effect_responses(&self, limit: usize) -> Result<()> {
        for pending in self
            .store
            .load_effect_responses(&self.tenant_id, limit)
            .await?
        {
            let DurableEffect::Invoke { fiber_id, pc, .. } = pending.effect() else {
                return Err(anyhow!("non-invocation effect produced an inbox response"));
            };
            let command = match pending.response() {
                EffectResponse::FfiSuccess {
                    output_payload,
                    new_domain_payload,
                } => Command::EffectCompleted {
                    effect_id: pending.effect_id(),
                    output: EffectOutput::Ffi {
                        fiber_id: *fiber_id,
                        pc: *pc,
                        output_payload: output_payload.clone(),
                        new_domain_payload: new_domain_payload.clone(),
                        no_match: false,
                    },
                },
                EffectResponse::FfiNoMatch => Command::EffectCompleted {
                    effect_id: pending.effect_id(),
                    output: EffectOutput::Ffi {
                        fiber_id: *fiber_id,
                        pc: *pc,
                        output_payload: Vec::new(),
                        new_domain_payload: None,
                        no_match: true,
                    },
                },
                EffectResponse::Failed {
                    error_class,
                    message,
                    attempt,
                } => Command::EffectFailed {
                    effect_id: pending.effect_id(),
                    job_key: String::new(),
                    error_class: error_class.clone(),
                    message: message.clone(),
                    retry: None,
                    attempt: *attempt,
                },
            };
            self.apply_and_commit_command(pending.instance_id(), command)
                .await?;
        }
        Ok(())
    }

    /// Compile BPMN XML → verified IR → bytecode, store the program.
    pub async fn compile(&self, bpmn_xml: &str) -> Result<CompileResult> {
        if bpmn_xml.len() > MAX_BPMN_XML_BYTES {
            return Err(anyhow!(
                "BPMN XML exceeds max size: {} bytes > {}",
                bpmn_xml.len(),
                MAX_BPMN_XML_BYTES
            ));
        }
        let (ir, process_meta) = parse_bpmn_with_meta(bpmn_xml)?;
        enforce_ir_limits(&ir)?;
        let artifact = bpmn_lite_compiler::Compiler::lower_with_default(
            &ir,
            process_meta.default_failure_budget,
        )?;
        enforce_artifact_limits(&artifact)?;
        let bytecode_version = artifact.hash().into_bytes();
        let task_types = artifact.envelope().metadata().task_manifest().to_vec();
        let flag_symbol_table = artifact.envelope().metadata().flag_symbol_table().clone();

        self.store.store_artifact(&artifact).await?;

        Ok(CompileResult {
            bytecode_version,
            task_types,
            diagnostics: vec![],
            flag_symbol_table,
        })
    }

    /// Lower a validated DSL plan through the same verifier and artifact
    /// repository used by the BPMN XML frontend.
    pub async fn compile_dsl(
        &self,
        plan: &bpmn_lite_compiler::dsl::WorkflowExecutionPlan,
    ) -> Result<CompileResult> {
        let artifact =
            bpmn_lite_compiler::Compiler::lower_dsl(plan).map_err(|error| anyhow!(error))?;
        enforce_artifact_limits(&artifact)?;
        let bytecode_version = artifact.hash().into_bytes();
        let task_types = artifact.envelope().metadata().task_manifest().to_vec();
        let flag_symbol_table = artifact.envelope().metadata().flag_symbol_table().clone();
        self.store.store_artifact(&artifact).await?;
        Ok(CompileResult {
            bytecode_version,
            task_types,
            diagnostics: Vec::new(),
            flag_symbol_table,
        })
    }

    /// Load a previously compiled program by its bytecode_version hash.
    pub async fn load_program(
        &self,
        bytecode_version: [u8; 32],
    ) -> Result<Option<CompiledProgram>> {
        Ok(self.store.load_program(bytecode_version).await?)
    }

    /// Persist an already-compiled program through this engine's
    /// `WorkflowStore` and surface the same `CompileResult` shape the
    /// retired `compile_from_dto` / `compile_from_yaml` produced.
    /// Used by the `bpmn-lite-authoring` free-function pipeline
    /// (`authoring::compile_from_dto` / `compile_from_yaml`) after
    /// it has lowered IR → bytecode.
    ///
    /// Phase 2.7 (2026-05-14) edge inversion landed here: the
    /// engine no longer parses YAML or DTOs. It accepts an
    /// already-lowered `CompiledProgram`, applies the same engine-
    /// level limit checks, and persists. Callers that need
    /// authoring-side compilation pull in `bpmn-lite-authoring`
    /// directly.
    pub async fn store_compiled_program(&self, program: CompiledProgram) -> Result<CompileResult> {
        enforce_program_limits(&program)?;

        let bytecode_errors = verify_bytecode(&program);
        if !bytecode_errors.is_empty() {
            let msgs: Vec<String> = bytecode_errors.iter().map(|e| e.message.clone()).collect();
            return Err(anyhow!(
                "Bytecode verification failed:\n{}",
                msgs.join("\n")
            ));
        }

        let envelope = ArtifactEnvelope::from_legacy_program(program, env!("CARGO_PKG_VERSION"))?;
        let artifact = ExecutableWorkflow::from_verified_envelope(envelope)?;
        enforce_artifact_limits(&artifact)?;
        let bytecode_version = artifact.hash().into_bytes();
        let task_types = artifact.envelope().metadata().task_manifest().to_vec();
        let flag_symbol_table = artifact.envelope().metadata().flag_symbol_table().clone();

        self.store.store_artifact(&artifact).await?;

        Ok(CompileResult {
            bytecode_version,
            task_types,
            diagnostics: vec![],
            flag_symbol_table,
        })
    }

    /// Start a new process instance.
    pub async fn start(
        &self,
        process_key: &str,
        bytecode_version: [u8; 32],
        domain_payload: &str,
        domain_payload_hash: [u8; 32],
        correlation_id: &str,
    ) -> Result<Uuid> {
        self.start_with_params(StartParams {
            process_key: process_key.to_string(),
            bytecode_version,
            domain_payload: domain_payload.to_string(),
            domain_payload_hash,
            correlation_id: correlation_id.to_string(),
            session_stack: SessionStackState::default(),
            entry_id: Uuid::nil(),
            runbook_id: Uuid::nil(),
            expected_preconditions: None,
        })
        .await
    }

    /// Start a new process instance with full parameters including session stack.
    pub async fn start_with_params(&self, params: StartParams) -> Result<Uuid> {
        let _program = self
            .store
            .load_program(params.bytecode_version)
            .await?
            .ok_or_else(|| anyhow!("No program found for bytecode version"))?;

        if let Some(ref preconditions) = params.expected_preconditions {
            if !params.domain_payload.is_empty() {
                let json_val: serde_json::Value = serde_json::from_str(&params.domain_payload)?;
                if let Some(obj) = json_val.as_object() {
                    for (key, expected_val) in preconditions {
                        let norm_key = key.trim_start_matches('@');
                        let actual_val_str = obj
                            .get(key)
                            .or_else(|| obj.get(norm_key))
                            .map(|v| match v {
                                serde_json::Value::String(s) => s.clone(),
                                v => v.to_string().replace('"', ""),
                            })
                            .unwrap_or_default();

                        if actual_val_str != *expected_val {
                            return Err(anyhow!(
                                "PreconditionConflict: expected {} to be {}, got '{}'",
                                key,
                                expected_val,
                                actual_val_str
                            ));
                        }
                    }
                } else {
                    return Err(anyhow!(
                        "PreconditionConflict: domain payload is not a JSON object"
                    ));
                }
            } else if !preconditions.is_empty() {
                return Err(anyhow!(
                    "PreconditionConflict: domain payload is empty but preconditions were specified"
                ));
            }
        }

        let computed_payload_hash = EffectId::content_hash(params.domain_payload.as_bytes());
        if computed_payload_hash != params.domain_payload_hash {
            return Err(anyhow!("initial payload hash does not match payload bytes"));
        }
        let instance_id = self.runtime_context.new_id();
        let tenant_id = self.tenant_id.clone();
        let command = StartCommand::builder(tenant_id, instance_id, params.bytecode_version)
            .process_key(params.process_key)
            .lineage(params.entry_id, params.runbook_id)
            .correlation_id(params.correlation_id)
            .idempotency_key(instance_id)
            .initial_payload(params.domain_payload)
            .session_stack(params.session_stack)
            .logical_time(self.logical_time()?)
            .build()
            .map_err(|error| anyhow!(error))?;
        self.start_command(command).await
    }

    /// Commit an idempotent start command and its initial snapshot/event/fiber
    /// in one transition. A concurrent replay returns the already committed id.
    pub async fn start_command(&self, command: StartCommand) -> Result<Uuid> {
        if command.tenant_id() != &self.tenant_id {
            return Err(anyhow!("start command tenant does not match engine tenant"));
        }
        let artifact_hash = ArtifactHash::from_bytes(command.artifact_hash());
        if self.store.load_artifact(artifact_hash).await?.is_none() {
            return Err(anyhow!("start command references an unknown artifact"));
        }
        if let Some(instance_id) = self
            .store
            .lookup_start_instance(self.tenant_id(), command.idempotency_key())
            .await?
        {
            return Ok(instance_id);
        }
        let instance_id = command.instance_id();
        // Fail closed at admission: the Ring 2 frame hash requires
        // `domain_payload` to parse as JSON, so a malformed payload accepted
        // here would only surface as an integrity error at first hash time —
        // reject it now instead. Any VALID JSON is legal (object-ness gates
        // only placeholder extraction below, not admission).
        let payload_value = serde_json::from_str::<serde_json::Value>(command.initial_payload())
            .map_err(|error| {
                anyhow!("start command initial_payload is not valid JSON: {error}")
            })?;
        let initial_placeholders = Some(payload_value).filter(serde_json::Value::is_object);
        let instance = ProcessInstance {
            instance_id,
            tenant_id: self.tenant_id.to_string(),
            process_key: command.process_key().to_string(),
            bytecode_version: command.artifact_hash(),
            domain_payload: command.initial_payload().to_string().into(),
            domain_payload_hash: command.initial_payload_hash(),
            session_stack: command.session_stack().clone(),
            flags: BTreeMap::new(),
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: command.correlation_id().to_string(),
            entry_id: command.entry_id(),
            runbook_id: command.runbook_id(),
            created_at: command.logical_time(),
            // integrity_hash and quarantine_state are set/managed by the store.
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: initial_placeholders,
        };
        let fiber_id = EffectId::for_command(command.idempotency_key(), 0, 0).as_uuid();
        let root_fiber = Fiber::new(fiber_id, 0);

        let event = RuntimeEvent::InstanceStarted {
            instance_id,
            bytecode_version: command.artifact_hash(),
        };
        let command_envelope = CommandEnvelope::new(
            command.idempotency_key(),
            command.logical_time(),
            JournalCommand::Start(command.clone()),
        );
        let transition = TransitionBuilder::new(instance)
            .upsert_fiber(root_fiber)
            .event(event)
            .start_dedupe(StartDedupe::new(command.clone()))
            .command_envelope(command_envelope)
            .build();
        let claim = Claim::new(command.tenant_id().clone(), instance_id, 0, 0);
        let outcome = self
            .store
            .commit_transition(&claim, &transition)
            .await
            .map_err(|error| anyhow!(error))?;
        match outcome {
            CommitOutcome::Committed { .. } => Ok(instance_id),
            CommitOutcome::IdempotentNoOp => self
                .store
                .lookup_start_instance(self.tenant_id(), command.idempotency_key())
                .await?
                .ok_or_else(|| anyhow!("idempotent start committed without a durable owner")),
        }
    }

    /// Tick all running instances. Returns count ticked.
    pub async fn tick_all(&self) -> Result<u32> {
        let ids = self.store.list_running_instances(&self.tenant_id).await?;
        self.tick_instance_ids(ids).await
    }

    /// Claim and tick a bounded batch of running instances for scheduler loops.
    pub async fn tick_claimed_batch(
        &self,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> Result<u32> {
        let ids = self
            .store
            .claim_running_instances(&self.tenant_id, owner, limit, lease_ms)
            .await?;
        self.tick_instance_ids_as_owner(ids, owner).await
    }

    /// Claim and apply a bounded batch of durable timer commands.
    pub async fn tick_due_timers(
        &self,
        owner: &str,
        logical_time: u64,
        limit: usize,
        lease_ms: u64,
    ) -> Result<u32> {
        let timers = self
            .store
            .claim_due_timers(&self.tenant_id, owner, logical_time, limit, lease_ms)
            .await?;
        let mut applied = 0u32;
        for timer in timers {
            let command = Command::TimerFired {
                timer: timer.clone(),
                fired_at: logical_time,
            };
            match self.apply_timer_command(command, owner).await {
                Ok(TimerFireOutcome::Applied) => applied += 1,
                Ok(TimerFireOutcome::AlreadyConsumed | TimerFireOutcome::Stale) => {}
                Err(error) => {
                    self.store.release_timer_claim(&timer).await?;
                    return Err(error);
                }
            }
        }
        Ok(applied)
    }

    #[tracing::instrument(
        name = "workflow.timer.apply",
        skip(self, command),
        fields(tenant_id = %self.tenant_id, owner)
    )]
    pub async fn apply_timer_command(
        &self,
        command: Command,
        owner: &str,
    ) -> Result<TimerFireOutcome> {
        let Command::TimerFired { timer, fired_at } = &command else {
            return Err(anyhow!("apply_timer_command requires TimerFired"));
        };
        if timer.tenant_id() != &self.tenant_id {
            return Err(anyhow!("timer command tenant does not match engine tenant"));
        }
        if *fired_at < timer.due_at() {
            return Err(anyhow!("timer command fired before its durable due_at"));
        }
        let work = self
            .store
            .claim_work_for_transition(
                &self.tenant_id,
                timer.instance_id(),
                owner,
                self.transition_lease_ms,
                command.clone(),
            )
            .await?
            .ok_or_else(|| anyhow!("timer instance is leased or missing"))?;
        let result = async {
            let (claim, snapshot, command) = work.into_parts();
            let instance = snapshot.instance();
            let artifact = self
                .load_artifact_cached(ArtifactHash::from_bytes(instance.bytecode_version))
                .await?;
            let context = DeterministicContext::new(
                *fired_at,
                timer.timer_id().as_uuid(),
                claim.expected_revision().saturating_add(1),
            );
            let transition = apply_kernel(&artifact, &snapshot, &command, &context)
                .map_err(|error| anyhow!(error))?;
            let semantic = if transition
                .events()
                .iter()
                .any(|event| matches!(event, RuntimeEvent::TimerFired { .. }))
            {
                TimerFireOutcome::Applied
            } else {
                TimerFireOutcome::Stale
            };
            let committed = self
                .store
                .commit_transition(&claim, &transition)
                .await
                .map_err(|error| anyhow!(error))?;
            Ok(match committed {
                CommitOutcome::IdempotentNoOp => TimerFireOutcome::AlreadyConsumed,
                _ => semantic,
            })
        }
        .await;
        let release = self
            .store
            .release_instance_transition(&self.tenant_id, timer.instance_id(), owner)
            .await;
        match (result, release) {
            (Ok(outcome), Ok(())) => Ok(outcome),
            (Err(error), _) => {
                self.store.release_timer_claim(timer).await?;
                Err(error)
            }
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    async fn tick_instance_ids(&self, ids: Vec<Uuid>) -> Result<u32> {
        let owner = self.transition_owner.clone();
        self.tick_instance_ids_as_owner(ids, &owner).await
    }

    async fn tick_instance_ids_as_owner(&self, ids: Vec<Uuid>, owner: &str) -> Result<u32> {
        let total = u32::try_from(ids.len()).unwrap_or(u32::MAX);
        stream::iter(ids)
            .map(|id| async move {
                if let Err(error) = self.tick_instance_as_owner(id, owner).await {
                    tracing::warn!(instance_id = %id, %error, "scheduled instance tick failed");
                }
            })
            .buffer_unordered(MAX_SCHEDULER_IN_FLIGHT)
            .collect::<Vec<()>>()
            .await;
        Ok(total)
    }

    /// Advance all runnable fibers for a specific instance.
    /// Jobs are left in the queue — use `activate_jobs()` to dequeue them.
    pub async fn tick_instance(&self, instance_id: Uuid) -> Result<()> {
        let owner = self.transition_owner.clone();
        self.tick_instance_as_owner(instance_id, &owner).await
    }

    async fn tick_instance_as_owner(&self, instance_id: Uuid, owner: &str) -> Result<()> {
        self.tick_instance_inner(instance_id, owner).await
    }

    #[tracing::instrument(
        name = "workflow.instance.tick",
        skip(self),
        fields(tenant_id = %self.tenant_id, instance_id = %instance_id, owner)
    )]
    async fn tick_instance_inner(&self, instance_id: Uuid, owner: &str) -> Result<()> {
        for _ in 0..MAX_BYTECODE_INSTRUCTIONS {
            let work = self
                .store
                .claim_work_for_transition(
                    &self.tenant_id,
                    instance_id,
                    owner,
                    self.transition_lease_ms,
                    Command::Tick { fiber_id: None },
                )
                .await?
                .ok_or_else(|| {
                    anyhow!("instance transition is leased or missing: {instance_id}")
                })?;

            let result = async {
                let (claim, mut snapshot, command) = work.into_parts();
                let instance = snapshot.instance();
                if instance.state != ProcessState::Running {
                    return Ok(None);
                }
                let artifact = self
                    .load_artifact_cached(ArtifactHash::from_bytes(instance.bytecode_version))
                    .await?;
                if !snapshot
                    .fibers()
                    .values()
                    .any(|fiber| fiber.wait == WaitState::Running)
                {
                    return Ok(None);
                }
                let mut buffered_messages = Vec::new();
                for fiber in snapshot
                    .fibers()
                    .values()
                    .filter(|fiber| fiber.wait == WaitState::Running)
                {
                    let Some(Instr::V2WaitMsg { name }) =
                        artifact.envelope().instructions().get(fiber.pc.index())
                    else {
                        continue;
                    };
                    // §28: resolve this wait's correlation key from process
                    // data via its `v2_corr_sources` entry; a source that
                    // can't resolve to a scalar claims nothing (fail closed),
                    // rather than claiming on a wrong/default key.
                    let Some(source) = artifact
                        .envelope()
                        .metadata()
                        .v2_corr_sources()
                        .get(&fiber.pc)
                    else {
                        continue;
                    };
                    let Ok(correlation_key) =
                        bpmn_lite_types::resolve_correlation_key(instance, source)
                    else {
                        continue;
                    };
                    let message_name = artifact
                        .envelope()
                        .metadata()
                        .message_name_map()
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| name.to_string());
                    if let Some(message) = self
                        .store
                        .claim_buffered_message(
                            &TenantId::new(instance.tenant_id.clone())?,
                            &message_name,
                            &correlation_key,
                            DEFAULT_MESSAGE_CLAIM_MS,
                        )
                        .await?
                    {
                        buffered_messages.push(message);
                    }
                }
                snapshot = snapshot.with_buffered_messages(buffered_messages);
                let logical_time = u64::try_from(self.logical_time()?)
                    .map_err(|_| anyhow!("logical time cannot be negative"))?;
                let command_id = EffectId::for_transition(
                    instance_id,
                    claim.expected_revision().saturating_add(1),
                    0,
                )
                .as_uuid();
                let context = DeterministicContext::new(
                    logical_time,
                    command_id,
                    claim.expected_revision().saturating_add(1),
                );
                let transition = apply_kernel(&artifact, &snapshot, &command, &context)
                    .map_err(|error| anyhow!(error))?;
                self.store
                    .commit_transition(&claim, &transition)
                    .await
                    .map_err(|error| anyhow!(error))?;
                Ok(Some(()))
            }
            .await;
            let release = self
                .store
                .release_instance_transition(&self.tenant_id, instance_id, owner)
                .await;
            match (result, release) {
                (Ok(Some(())), Ok(())) => {
                    self.dispatch_pending_effects(owner, 32, self.transition_lease_ms)
                        .await?;
                    continue;
                }
                (Ok(None), Ok(())) => return Ok(()),
                (Err(error), _) => return Err(error),
                (Ok(_), Err(error)) => return Err(error.into()),
            }
        }
        Err(anyhow!(
            "instance {instance_id} exceeded transition scheduling bound"
        ))
    }

    /// Advance all runnable fibers and return any job activations.
    /// Convenience method for in-process use — dequeues jobs immediately.
    pub async fn run_instance(&self, instance_id: Uuid) -> Result<Vec<JobActivation>> {
        self.tick_instance(instance_id).await?;

        let instance = self
            .store
            .load_instance(&self.tenant_id, instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;

        let program = self
            .store
            .load_program(instance.bytecode_version)
            .await?
            .ok_or_else(|| anyhow!("Program not found for instance {}", instance_id))?;

        let jobs = self
            .store
            .dequeue_jobs(
                program.task_manifest(),
                100,
                &self.tenant_id,
                DEFAULT_WORKER_ID,
                DEFAULT_JOB_LEASE_MS,
            )
            .await?;
        self.emit_job_claimed_events(&jobs).await?;
        Ok(jobs)
    }

    /// Emit a SignalIgnored audit event for late/ghost completions.
    #[tracing::instrument(
        name = "workflow.job.complete",
        skip(self, domain_payload, orch_flags),
        fields(tenant_id = %self.tenant_id, job_key)
    )]
    pub async fn complete_job(
        &self,
        job_key: &str,
        domain_payload: &str,
        expected_instance_payload_hash: [u8; 32],
        orch_flags: BTreeMap<String, Value>,
    ) -> Result<()> {
        let (instance_id, _task_type_id, _pc) = parse_job_key(job_key)?;
        let owner = self.transition_owner.clone();
        let work = self
            .store
            .claim_work_for_transition(
                &self.tenant_id,
                instance_id,
                &owner,
                self.transition_lease_ms,
                Command::Tick { fiber_id: None },
            )
            .await?
            .ok_or_else(|| anyhow!("instance transition is leased or missing: {instance_id}"))?;
        let result = async {
            let (claim, snapshot, _) = work.into_parts();
            let instance = snapshot.instance();
            if self
                .store
                .dedupe_get(&TenantId::new(instance.tenant_id.clone())?, job_key)
                .await?
                .is_some()
            {
                return Ok(());
            }
            let artifact = self
                .load_artifact_cached(ArtifactHash::from_bytes(instance.bytecode_version))
                .await?;
            let completion = JobCompletion {
                job_key: job_key.to_string(),
                domain_payload: domain_payload.to_string(),
                expected_instance_payload_hash,
                orch_flags,
            };
            let revision = claim.expected_revision().saturating_add(1);
            let context = DeterministicContext::new(
                u64::try_from(self.logical_time()?)
                    .map_err(|_| anyhow!("logical time cannot be negative"))?,
                EffectId::for_transition(instance_id, revision, 0).as_uuid(),
                revision,
            );
            let transition = apply_kernel(
                &artifact,
                &snapshot,
                &Command::EffectCompleted {
                    effect_id: EffectId::for_transition(instance_id, revision, 0),
                    output: EffectOutput::Job(completion),
                },
                &context,
            )
            .map_err(|error| anyhow!(error))?;
            self.store
                .commit_transition(&claim, &transition)
                .await
                .map_err(|error| anyhow!(error))?;
            Ok(())
        }
        .await;
        let release = self
            .store
            .release_instance_transition(&self.tenant_id, instance_id, &owner)
            .await;
        match (result, release) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
        }
    }

    pub async fn complete_job_with_claim(
        &self,
        job_key: &str,
        domain_payload: &str,
        expected_instance_payload_hash: [u8; 32],
        orch_flags: BTreeMap<String, Value>,
        worker_id: &str,
        claim_token: &str,
    ) -> Result<()> {
        if worker_id.is_empty() || claim_token.is_empty() {
            return Err(anyhow!("worker_id and claim_token are required"));
        }
        let (instance_id, _, _) = parse_job_key(job_key)?;
        let instance = self
            .store
            .load_instance(&self.tenant_id, instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found for job: {}", job_key))?;
        if !self
            .store
            .validate_job_claim(
                &TenantId::new(instance.tenant_id.clone())?,
                job_key,
                worker_id,
                claim_token,
            )
            .await?
        {
            return Err(anyhow!("job claim does not match worker ownership"));
        }
        self.complete_job(
            job_key,
            domain_payload,
            expected_instance_payload_hash,
            orch_flags,
        )
        .await
    }

    /// Fail a job — route to error boundary handler or create an incident.
    ///
    /// Only `BusinessRejection` errors trigger error routing. `Transient` and
    /// `ContractViolation` always create incidents.
    pub async fn fail_job(
        &self,
        job_key: &str,
        error_class: ErrorClass,
        message: &str,
    ) -> Result<()> {
        let (instance_id, _task_type_id, pc) = parse_job_key(job_key)?;
        self.apply_and_commit_command(
            instance_id,
            Command::EffectFailed {
                effect_id: EffectId::for_instruction(instance_id, Uuid::nil(), pc),
                job_key: job_key.to_string(),
                error_class,
                message: message.to_string(),
                retry: None,
                // No RetryPolicy bookkeeping is consulted on this path
                // (V&S §15 ruling E): honest absence, not a lie.
                attempt: 0,
            },
        )
        .await
    }

    pub async fn fail_job_with_claim(
        &self,
        job_key: &str,
        error_class: ErrorClass,
        message: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> Result<()> {
        if worker_id.is_empty() || claim_token.is_empty() {
            return Err(anyhow!("worker_id and claim_token are required"));
        }
        let (instance_id, _task_type_id, pc) = parse_job_key(job_key)?;
        let instance = self
            .store
            .load_instance(&self.tenant_id, instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found for job: {}", job_key))?;
        if !self
            .store
            .validate_job_claim(
                &TenantId::new(instance.tenant_id.clone())?,
                job_key,
                worker_id,
                claim_token,
            )
            .await?
        {
            return Err(anyhow!("job claim does not match worker ownership"));
        }
        self.apply_and_commit_command(
            instance_id,
            Command::EffectFailed {
                effect_id: EffectId::for_instruction(instance_id, Uuid::nil(), pc),
                job_key: job_key.to_string(),
                error_class,
                message: message.to_string(),
                retry: Some(JobRetry::new(
                    worker_id,
                    claim_token,
                    self.logical_time()?
                        .checked_add(1)
                        .ok_or_else(|| anyhow!("retry timestamp overflow"))?,
                )),
                // No RetryPolicy bookkeeping is consulted on this path
                // (V&S §15 ruling E): honest absence, not a lie.
                attempt: 0,
            },
        )
        .await
    }

    pub async fn signal(
        &self,
        instance_id: Uuid,
        _msg_name: &str,
        corr_key: &str,
        domain_payload: Option<&str>,
        domain_payload_hash: Option<[u8; 32]>,
        _msg_id: Option<&str>,
    ) -> Result<()> {
        self.signal_with_value(
            instance_id,
            _msg_name,
            corr_key.to_string(),
            domain_payload,
            domain_payload_hash,
            _msg_id,
        )
        .await
    }

    /// `corr_key` is the content correlation key (§28) — matched by string
    /// equality against the key a waiting subscription resolved from its own
    /// process data. Callers at the gRPC boundary canonicalize the wire value
    /// via `correlation_key_string` before this point.
    pub async fn signal_with_value(
        &self,
        instance_id: Uuid,
        msg_name: &str,
        corr_key: String,
        domain_payload: Option<&str>,
        domain_payload_hash: Option<[u8; 32]>,
        msg_id: Option<&str>,
    ) -> Result<()> {
        let msg_id = msg_id
            .filter(|msg_id| !msg_id.is_empty())
            .ok_or_else(|| anyhow!("msg_id is required for idempotent signal delivery"))?;
        let now = self.logical_time()?;
        self.apply_and_commit_command(
            instance_id,
            Command::MessageDelivered {
                message_id: msg_id.to_string(),
                name: msg_name.to_string(),
                correlation_key: corr_key,
                payload: domain_payload.map(str::as_bytes).unwrap_or(&[]).to_vec(),
                payload_hash: domain_payload_hash,
                expires_at: now + DEFAULT_MESSAGE_TTL_MS as i64,
            },
        )
        .await
    }

    pub async fn cancel(&self, instance_id: Uuid, reason: &str) -> Result<()> {
        self.apply_and_commit_command(
            instance_id,
            Command::Cancel {
                reason: reason.to_string(),
            },
        )
        .await
    }

    pub async fn inspect(&self, instance_id: Uuid) -> Result<ProcessInspection> {
        let instance = self
            .store
            .load_instance(&self.tenant_id, instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;
        self.ensure_loaded_instance_belongs_to_tenant(&instance, instance_id)?;

        let fibers = self.store.load_fibers(&self.tenant_id, instance_id).await?;
        let fiber_inspections: Vec<FiberInspection> = fibers
            .iter()
            .map(|f| FiberInspection {
                fiber_id: f.fiber_id,
                pc: f.pc,
                wait_state: f.wait.clone(),
                stack_depth: f.stack.len(),
            })
            .collect();

        let incidents = self
            .store
            .load_incidents(&self.tenant_id, instance_id)
            .await?;

        let program = self.store.load_program(instance.bytecode_version).await?;
        let current_node_id = if let Some(prog) = &program {
            if let Some(first_fiber) = fibers.first() {
                prog.debug_map().get(&first_fiber.pc).cloned()
            } else {
                None
            }
        } else {
            None
        };

        let placeholder_values =
            serde_json::from_str::<serde_json::Value>(&instance.domain_payload).ok();

        Ok(ProcessInspection {
            instance_id,
            tenant_id: instance.tenant_id,
            process_key: instance.process_key,
            bytecode_version: instance.bytecode_version,
            domain_payload_hash: instance.domain_payload_hash,
            state: instance.state,
            fibers: fiber_inspections,
            incidents,
            current_node_id,
            placeholder_values,
        })
    }

    pub async fn scan_recoverable_inconsistencies(&self) -> Result<Vec<RecoveryIssue>> {
        let mut issues = Vec::new();
        let instance_ids = self.store.list_running_instances(&self.tenant_id).await?;

        for instance_id in instance_ids {
            let Some(instance) = self
                .store
                .load_instance(&self.tenant_id, instance_id)
                .await?
            else {
                issues.push(RecoveryIssue {
                    instance_id,
                    kind: "missing_instance".to_string(),
                    detail: "running instance id was listed but cannot be loaded".to_string(),
                });
                continue;
            };
            self.ensure_loaded_instance_belongs_to_tenant(&instance, instance_id)?;

            if self
                .store
                .load_artifact(ArtifactHash::from_bytes(instance.bytecode_version))
                .await?
                .is_none()
            {
                issues.push(RecoveryIssue {
                    instance_id,
                    kind: "missing_artifact".to_string(),
                    detail: "verified artifact for running instance is absent".to_string(),
                });
            }

            if self
                .store
                .load_fibers(&self.tenant_id, instance_id)
                .await?
                .is_empty()
            {
                issues.push(RecoveryIssue {
                    instance_id,
                    kind: "missing_fibers".to_string(),
                    detail: "running instance has no fibers to resume".to_string(),
                });
            }

            let has_started_event = self
                .store
                .read_events(&self.tenant_id, instance_id, 1)
                .await?
                .iter()
                .any(|(_, event)| matches!(event, RuntimeEvent::InstanceStarted { .. }));
            if !has_started_event {
                issues.push(RecoveryIssue {
                    instance_id,
                    kind: "missing_start_event".to_string(),
                    detail: "running instance has no InstanceStarted audit event".to_string(),
                });
            }
        }

        Ok(issues)
    }

    /// Activate jobs — dequeue from the job queue.
    pub async fn activate_jobs(
        &self,
        task_types: &[String],
        max_jobs: usize,
    ) -> Result<Vec<JobActivation>> {
        self.activate_jobs_for_worker(task_types, max_jobs, DEFAULT_WORKER_ID)
            .await
    }

    pub async fn activate_jobs_for_worker(
        &self,
        task_types: &[String],
        max_jobs: usize,
        worker_id: &str,
    ) -> Result<Vec<JobActivation>> {
        self.activate_jobs_for_worker_with_lease(
            task_types,
            max_jobs,
            worker_id,
            DEFAULT_JOB_LEASE_MS,
        )
        .await
    }

    pub async fn activate_jobs_for_worker_with_lease(
        &self,
        task_types: &[String],
        max_jobs: usize,
        worker_id: &str,
        lease_ms: u64,
    ) -> Result<Vec<JobActivation>> {
        if worker_id.is_empty() {
            return Err(anyhow!("worker_id is required"));
        }
        let jobs = self
            .store
            .dequeue_jobs(task_types, max_jobs, &self.tenant_id, worker_id, lease_ms)
            .await?;
        self.emit_job_claimed_events(&jobs).await?;
        Ok(jobs)
    }

    async fn emit_job_claimed_events(&self, jobs: &[JobActivation]) -> Result<()> {
        for job in jobs {
            if let Some(claim_expires_at) = job.claim_expires_at {
                self.apply_and_commit_command(
                    job.process_instance_id,
                    Command::JobClaimed {
                        job_key: job.job_key.clone(),
                        worker_id: job.worker_id.clone(),
                        claim_expires_at,
                    },
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Read events from the event log.
    pub async fn read_events(
        &self,
        instance_id: Uuid,
        from_seq: u64,
    ) -> Result<Vec<(u64, RuntimeEvent)>> {
        let instance = self
            .store
            .load_instance(&self.tenant_id, instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;
        self.ensure_loaded_instance_belongs_to_tenant(&instance, instance_id)?;
        Ok(self
            .store
            .read_events(&self.tenant_id, instance_id, from_seq)
            .await?)
    }

    pub async fn health_check(&self) -> Result<()> {
        Ok(self.store.health_check().await?)
    }

    /// Fail-closed startup reconciliation across every registered tenant.
    #[tracing::instrument(name = "workflow.recovery", skip(self), fields(owner, batch_size))]
    pub async fn recover_all_tenants(
        &self,
        owner: &str,
        batch_size: usize,
        lease_ms: u64,
    ) -> Result<RecoveryReport> {
        self.store.health_check().await?;
        let stale_jobs_reclaimed = self.store.reclaim_stale_jobs(5 * 60 * 1_000).await?;
        self.store.reclaim_stale_buffered_message_claims().await?;
        let tenants = self.store.list_tenants().await?;
        let logical_time = u64::try_from(self.logical_time()?)
            .map_err(|_| anyhow!("recovery logical time cannot be negative"))?;
        let mut report = RecoveryReport {
            tenants_scanned: tenants.len(),
            instances_verified: 0,
            timers_applied: 0,
            interrupted_effects: 0,
            stale_jobs_reclaimed,
        };

        for tenant_id in tenants {
            let tenant = TenantId::new(&tenant_id)?;
            let tenant_engine = self.for_tenant(tenant.clone());
            let instances = self.store.list_running_instances(&tenant).await?;
            for instance_id in instances {
                let work = self
                    .store
                    .claim_work_for_transition(
                        &tenant,
                        instance_id,
                        owner,
                        lease_ms,
                        Command::Tick { fiber_id: None },
                    )
                    .await?
                    .ok_or_else(|| {
                        anyhow!(
                            "recovery could not fence tenant {tenant_id} instance {instance_id}"
                        )
                    })?;
                let (claim, snapshot, _) = work.into_parts();
                let verification = async {
                    let instance = snapshot.instance();
                    let artifact = self
                        .load_artifact_cached(ArtifactHash::from_bytes(instance.bytecode_version))
                        .await?;
                    if artifact.envelope().abi_version() != CURRENT_ARTIFACT_ABI {
                        return Err(anyhow!("recovery artifact ABI mismatch: {instance_id}"));
                    }
                    if snapshot.fibers().is_empty() {
                        return Err(anyhow!("recovery fibers are missing: {instance_id}"));
                    }
                    Ok::<(), anyhow::Error>(())
                }
                .await;
                if let Err(error) = verification {
                    let mut instance = snapshot.instance().clone();
                    instance.quarantine_state = Some("recovery_invariant_failure".to_string());
                    let transition = TransitionBuilder::new(instance)
                        .command_envelope(CommandEnvelope::new(
                            EffectId::for_transition(
                                instance_id,
                                claim.expected_revision().saturating_add(1),
                                u32::MAX,
                            )
                            .as_uuid(),
                            self.logical_time()?,
                            JournalCommand::Administrative {
                                kind: "recovery_quarantine".to_string(),
                            },
                        ))
                        .build();
                    self.store.commit_transition(&claim, &transition).await?;
                    self.store
                        .release_instance_transition(&tenant, instance_id, owner)
                        .await?;
                    return Err(error);
                }
                self.store
                    .release_instance_transition(&tenant, instance_id, owner)
                    .await?;
                report.instances_verified += 1;
            }

            report.timers_applied = report.timers_applied.saturating_add(
                tenant_engine
                    .tick_due_timers(owner, logical_time, batch_size, lease_ms)
                    .await?,
            );
            tenant_engine
                .dispatch_pending_effects(owner, batch_size, lease_ms)
                .await?;
            report.interrupted_effects = report
                .interrupted_effects
                .saturating_add(tenant_engine.detect_interrupted_ffi_calls(&tenant).await?);
            let issues = tenant_engine.scan_recoverable_inconsistencies().await?;
            if !issues.is_empty() {
                return Err(anyhow!("recovery invariant failures: {issues:?}"));
            }
        }
        Ok(report)
    }

    /// A17 — Scan running instances for interrupted FFI calls and recover.
    ///
    /// - **Idempotent / IdempotentWithKey:** no action — tick loop re-invokes.
    /// - **NonIdempotent:** creates an Incident on the stalled fiber and marks
    ///   the instance Failed. Operator must resolve the incident manually.
    ///
    /// Returns the number of interrupted calls processed.
    pub async fn detect_interrupted_ffi_calls(&self, tenant_id: &TenantId) -> Result<usize> {
        use bpmn_lite_types::RuntimeEvent;
        use ffi_types::Idempotency;
        use std::collections::HashSet;

        let Some(dispatcher) = &self.ffi_dispatcher else {
            return Ok(0);
        };

        let running = self.store.list_running_instances(tenant_id).await?;
        let mut count = 0usize;

        for instance_id in running {
            let events = self.store.read_events(tenant_id, instance_id, 0).await?;

            let completed_ids: HashSet<Uuid> = events
                .iter()
                .filter_map(|(_, ev)| match ev {
                    RuntimeEvent::FfiInvocationCompleted { invocation_id, .. } => {
                        Some(*invocation_id)
                    }
                    _ => None,
                })
                .collect();

            for (_, ev) in &events {
                let RuntimeEvent::FfiInvocationPending {
                    invocation_id,
                    template_id_hex,
                    caller_task_id,
                    caller_pc: _,
                    owner_type,
                } = ev
                else {
                    continue;
                };

                if completed_ids.contains(invocation_id) {
                    continue;
                }
                count += 1;

                // Decode template_id from 64-char hex.
                let template_id: Option<[u8; 32]> = (template_id_hex.len() == 64)
                    .then(|| {
                        (0..32)
                            .map(|i| {
                                u8::from_str_radix(&template_id_hex[i * 2..i * 2 + 2], 16).ok()
                            })
                            .collect::<Option<Vec<u8>>>()
                    })
                    .flatten()
                    .and_then(|v| v.try_into().ok());

                let idempotency = if let Some(tid) = &template_id {
                    dispatcher.idempotency_for(tid).await
                } else {
                    None
                };

                match &idempotency {
                    Some(Idempotency::NonIdempotent) => {
                        tracing::warn!(
                            %instance_id, %invocation_id,
                            template_id = %&template_id_hex[..16],
                            %owner_type, %caller_task_id,
                            "non-idempotent FFI effect interrupted; creating a fenced incident transition"
                        );
                        self.apply_and_commit_command(
                            instance_id,
                            Command::EffectFailed {
                                effect_id: EffectId::from_uuid(*invocation_id),
                                job_key: String::new(),
                                error_class: ErrorClass::ContractViolation,
                                message: format!(
                                    "non-idempotent FFI call to {} interrupted at restart; manual resolution required",
                                    &template_id_hex[..16]
                                ),
                                retry: None,
                                // No RetryPolicy bookkeeping is consulted
                                // on this path (V&S §15 ruling E): honest
                                // absence, not a lie.
                                attempt: 0,
                            },
                        ).await?;
                    }
                    _ => {
                        tracing::info!(
                            %instance_id, %invocation_id,
                            template_id = %&template_id_hex[..16],
                            %owner_type, %caller_task_id,
                            "A17: idempotent FFI call interrupted — tick loop will re-invoke"
                        );
                    }
                }
            }
        }

        Ok(count)
    }
}

/// Parse a job_key in format "instance_id:service_task_id:pc:epoch"
/// where service_task_id is a string (BPMN element ID from debug_map).
/// The epoch is parsed but discarded — only instance_id, task_id, and pc are returned.
fn parse_job_key(job_key: &str) -> Result<(Uuid, String, u32)> {
    // Format: uuid:service_task_id:pc:epoch
    // Strategy: split from the RIGHT for epoch (last), then pc (second-to-last),
    // then from the LEFT for UUID (first 36 chars), remainder is service_task_id.
    let mut parts = job_key.rsplitn(2, ':');
    let _epoch_str = parts
        .next()
        .ok_or_else(|| anyhow!("Invalid job_key format: {}", job_key))?;
    let rest = parts
        .next()
        .ok_or_else(|| anyhow!("Invalid job_key format: {}", job_key))?;

    // rest is now "uuid:service_task_id:pc"
    let mut parts2 = rest.rsplitn(2, ':');
    let pc_str = parts2
        .next()
        .ok_or_else(|| anyhow!("Invalid job_key format: {}", job_key))?;
    let rest2 = parts2
        .next()
        .ok_or_else(|| anyhow!("Invalid job_key format: {}", job_key))?;

    // rest2 is now "uuid:service_task_id"
    let mut parts3 = rest2.splitn(2, ':');
    let uuid_str = parts3
        .next()
        .ok_or_else(|| anyhow!("Invalid job_key format: {}", job_key))?;
    let service_task_id = parts3
        .next()
        .ok_or_else(|| anyhow!("Invalid job_key format: {}", job_key))?
        .to_string();

    let instance_id =
        Uuid::parse_str(uuid_str).map_err(|e| anyhow!("Invalid UUID in job_key: {}", e))?;
    let pc: u32 = pc_str
        .parse()
        .map_err(|e| anyhow!("Invalid pc in job_key: {}", e))?;
    Ok((instance_id, service_task_id, pc))
}

// ── A8 FFI helpers ────────────────────────────────────────────────────────────

fn ffi_incident_class_to_error_class(c: FfiIncidentClass) -> ErrorClass {
    match c {
        FfiIncidentClass::Transient => ErrorClass::Transient,
        FfiIncidentClass::ContractViolation => ErrorClass::ContractViolation,
        FfiIncidentClass::BusinessRejection { rejection_code } => {
            ErrorClass::BusinessRejection { rejection_code }
        }
    }
}
