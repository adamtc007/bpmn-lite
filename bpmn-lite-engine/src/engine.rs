// authoring dep removed in Phase 2.7 — see lib.rs
use anyhow::{anyhow, Result};
use bpmn_lite_compiler::{lowering, parser, verifier};
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_types::events::RuntimeEvent;
use bpmn_lite_types::ffi_bindings::{BindingSource, BindingTarget, Literal};
use bpmn_lite_types::*;
use bpmn_lite_vm::{apply_completion, compute_hash, json_path, TickOutcome, Vm};
use ffi_dispatcher::FfiDispatcher;
use ffi_types::wire::{FfiCall, FfiIncidentClass, FfiResult};
use bpmn_lite_types::session_stack::SessionStackState;
use std::collections::BTreeMap;
use std::sync::Arc;
#[allow(unused_imports)]
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

/// BpmnLiteEngine is the top-level facade that wires together the compiler,
/// VM, and store. gRPC handlers delegate to this.
pub struct BpmnLiteEngine {
    store: Arc<dyn ProcessStore>,
    tenant_id: String,
    transition_owner: String,
    transition_lease_ms: u64,
    /// Optional in-process FFI dispatcher. None = ExecFfi produces an incident.
    ffi_dispatcher: Option<Arc<FfiDispatcher>>,
    /// T3 — bus client for plan-based process execution. When set, Running
    /// instances with plan_hash are dispatched to PlanWalker instead of the
    /// bytecode fiber VM.
    bus_client: Option<Arc<dsl_bus_client::BusClient>>,
    /// T3 — pending invocation store for plan-based callouts.
    pending_store: Option<Arc<dyn bpmn_lite_store::pending::PendingInvocationStore>>,
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

fn enforce_ir_limits(ir: &bpmn_lite_compiler::ir::IRGraph) -> Result<()> {
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
    if program.program.len() > MAX_BYTECODE_INSTRUCTIONS {
        return Err(anyhow!(
            "compiled bytecode has too many instructions: {} > {}",
            program.program.len(),
            MAX_BYTECODE_INSTRUCTIONS
        ));
    }
    if program.task_manifest.len() > MAX_TASK_MANIFEST {
        return Err(anyhow!(
            "compiled task manifest has too many task types: {} > {}",
            program.task_manifest.len(),
            MAX_TASK_MANIFEST
        ));
    }
    Ok(())
}

impl BpmnLiteEngine {
    pub fn new(store: Arc<dyn ProcessStore>) -> Self {
        Self::new_with_tenant(store, "default")
    }

    pub fn new_with_tenant(store: Arc<dyn ProcessStore>, tenant_id: impl Into<String>) -> Self {
        Self {
            store,
            tenant_id: tenant_id.into(),
            transition_owner: format!("engine-{}", Uuid::now_v7()),
            transition_lease_ms: DEFAULT_TRANSITION_LEASE_MS,
            ffi_dispatcher: None,
            bus_client: None,
            pending_store: None,
        }
    }

    /// Attach an FFI dispatcher. Call before starting instances that contain
    /// `ExecFfi` instructions; without a dispatcher ExecFfi produces an incident.
    pub fn with_ffi_dispatcher(mut self, dispatcher: Arc<FfiDispatcher>) -> Self {
        self.ffi_dispatcher = Some(dispatcher);
        self
    }

    /// T3 — attach a bus client + pending store for plan-based process
    /// execution. When set, Running instances with `plan_hash` populated
    /// are dispatched to `PlanWalker::advance` instead of the bytecode
    /// fiber loop.
    pub fn with_bus_client(
        mut self,
        client: Arc<dsl_bus_client::BusClient>,
        pending_store: Arc<dyn bpmn_lite_store::pending::PendingInvocationStore>,
    ) -> Self {
        self.bus_client = Some(client);
        self.pending_store = Some(pending_store);
        self
    }

    pub fn for_tenant(&self, tenant_id: impl Into<String>) -> Self {
        Self {
            store: self.store.clone(),
            tenant_id: tenant_id.into(),
            transition_owner: format!("engine-{}", Uuid::now_v7()),
            transition_lease_ms: self.transition_lease_ms,
            ffi_dispatcher: self.ffi_dispatcher.clone(),
            bus_client: self.bus_client.clone(),
            pending_store: self.pending_store.clone(),
        }
    }

    fn ensure_loaded_instance_belongs_to_tenant(
        &self,
        instance: &ProcessInstance,
        instance_id: Uuid,
    ) -> Result<()> {
        if instance.tenant_id != self.tenant_id {
            return Err(anyhow!("Instance not found: {}", instance_id));
        }
        Ok(())
    }

    /// A19 — Combined pickup guard: tenant check then integrity verification.
    async fn claim_transition_as(&self, instance_id: Uuid, owner: &str) -> Result<()> {
        let claimed = self
            .store
            .claim_instance_for_transition(
                &self.tenant_id,
                instance_id,
                owner,
                self.transition_lease_ms,
            )
            .await?;
        if !claimed {
            return Err(anyhow!(
                "process instance mutation is already leased or not found: {}",
                instance_id
            ));
        }
        Ok(())
    }

    async fn release_transition_as(&self, instance_id: Uuid, owner: &str) -> Result<()> {
        self.store
            .release_instance_transition(&self.tenant_id, instance_id, owner)
            .await
    }

    async fn run_guarded_transition<T, F, Fut>(
        &self,
        instance_id: Uuid,
        owner: &str,
        operation: F,
    ) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        self.claim_transition_as(instance_id, owner).await?;
        let result = operation().await;
        let release_result = self.release_transition_as(instance_id, owner).await;
        match (result, release_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(err), Ok(())) => Err(err),
            (Ok(_), Err(err)) => Err(err),
            (Err(err), Err(release_err)) => {
                tracing::warn!(
                    instance_id = %instance_id,
                    error = %release_err,
                    "failed to release instance transition guard after transition error"
                );
                Err(err)
            }
        }
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
        let ir = parser::parse_bpmn(bpmn_xml)?;
        enforce_ir_limits(&ir)?;
        let errors = verifier::verify(&ir);
        if !errors.is_empty() {
            let msgs: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();
            return Err(anyhow!("Verification failed:\n{}", msgs.join("\n")));
        }
        let program = lowering::lower(&ir)?;
        enforce_program_limits(&program)?;

        // Bytecode verification (bounded loop enforcement)
        let bytecode_errors = verifier::verify_bytecode(&program);
        if !bytecode_errors.is_empty() {
            let msgs: Vec<String> = bytecode_errors.iter().map(|e| e.message.clone()).collect();
            return Err(anyhow!(
                "Bytecode verification failed:\n{}",
                msgs.join("\n")
            ));
        }

        let bytecode_version = program.bytecode_version;
        let task_types = program.task_manifest.clone();
        let flag_symbol_table = program.flag_symbol_table.clone();

        self.store.store_program(bytecode_version, &program).await?;

        Ok(CompileResult {
            bytecode_version,
            task_types,
            diagnostics: vec![],
            flag_symbol_table,
        })
    }

    /// Load a previously compiled program by its bytecode_version hash.
    pub async fn load_program(
        &self,
        bytecode_version: [u8; 32],
    ) -> Result<Option<CompiledProgram>> {
        self.store.load_program(bytecode_version).await
    }

    /// Persist an already-compiled program through this engine's
    /// `ProcessStore` and surface the same `CompileResult` shape the
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

        let bytecode_errors = verifier::verify_bytecode(&program);
        if !bytecode_errors.is_empty() {
            let msgs: Vec<String> = bytecode_errors.iter().map(|e| e.message.clone()).collect();
            return Err(anyhow!(
                "Bytecode verification failed:\n{}",
                msgs.join("\n")
            ));
        }

        let bytecode_version = program.bytecode_version;
        let task_types = program.task_manifest.clone();
        let flag_symbol_table = program.flag_symbol_table.clone();

        self.store.store_program(bytecode_version, &program).await?;

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

        let instance_id = Uuid::now_v7();
        let instance = ProcessInstance {
            instance_id,
            tenant_id: self.tenant_id.clone(),
            process_key: params.process_key,
            bytecode_version: params.bytecode_version,
            domain_payload: params.domain_payload.into(),
            domain_payload_hash: params.domain_payload_hash,
            session_stack: params.session_stack,
            flags: BTreeMap::new(),
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: params.correlation_id,
            entry_id: params.entry_id,
            runbook_id: params.runbook_id,
            created_at: now_ms(),
            // integrity_hash and quarantine_state are set/managed by the store.
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: None,
        };
        let fiber_id = Uuid::now_v7();
        let root_fiber = Fiber::new(fiber_id, 0);

        self.store
            .atomic_start(
                &instance,
                &root_fiber,
                &RuntimeEvent::InstanceStarted {
                    instance_id,
                    bytecode_version: params.bytecode_version,
                },
            )
            .await?;

        Ok(instance_id)
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

    async fn tick_instance_ids(&self, ids: Vec<Uuid>) -> Result<u32> {
        let owner = self.transition_owner.clone();
        self.tick_instance_ids_as_owner(ids, &owner).await
    }

    async fn tick_instance_ids_as_owner(&self, ids: Vec<Uuid>, owner: &str) -> Result<u32> {
        let mut ticked = 0u32;
        for id in ids {
            if let Err(e) = self.tick_instance_as_owner(id, owner).await {
                tracing::warn!(instance_id = %id, error = %e, "tick_instance_ids: instance tick failed");
            }
            ticked += 1;
        }
        Ok(ticked)
    }

    /// Advance all runnable fibers for a specific instance.
    /// Jobs are left in the queue — use `activate_jobs()` to dequeue them.
    pub async fn tick_instance(&self, instance_id: Uuid) -> Result<()> {
        let owner = self.transition_owner.clone();
        self.tick_instance_as_owner(instance_id, &owner).await
    }

    async fn tick_instance_as_owner(&self, instance_id: Uuid, owner: &str) -> Result<()> {
        self.run_guarded_transition(instance_id, owner, || async {
            self.tick_instance_inner(instance_id).await
        })
        .await
    }

    async fn tick_instance_inner(&self, instance_id: Uuid) -> Result<()> {
        // T3 — plan-based instances bypass the fiber VM entirely.
        // The discriminator is plan_hash: Some = plan path, None = bytecode path.
        // WaitingOnSubmission / WaitingOnInvocation instances are skipped by the
        // ProcessState::Running guard inside PlanWalker::advance.
        if let (Some(bus_client), Some(pending_store)) =
            (&self.bus_client, &self.pending_store)
        {
            // Peek at the instance state before full load.
            if let Some(inst) = self.store.load_instance(instance_id).await? {
                if inst.plan_hash.is_some() {
                    if matches!(inst.state, ProcessState::Running) {
                        let walker = crate::plan_walker::PlanWalker::new(
                            self.store.clone(),
                            pending_store.clone(),
                            bus_client.clone(),
                        );
                        let _ = walker.advance(instance_id).await?;
                    }
                    return Ok(());
                }
            }
        }

        let mut instance = self
            .store
            .load_instance(instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;

        let program = self
            .store
            .load_program(instance.bytecode_version)
            .await?
            .ok_or_else(|| anyhow!("Program not found for instance {}", instance_id))?;

        let fibers = self.store.load_fibers(instance_id).await?;

        'fiber_loop: for fiber in fibers {
            if fiber.wait != WaitState::Running {
                continue;
            }

            let mut fiber = fiber;
            let vm = Vm::new(self.store.clone());

            // Inner loop allows re-running the fiber after an in-process FFI call.
            loop {
                let outcome = vm
                    .run_fiber(&mut fiber, &mut instance, &program, 1000)
                    .await?;

                // Save updated instance (flags + counters)
                self.store.save_instance(&instance).await?;

                match outcome {
                    TickOutcome::ExecFfi {
                        template_id: _,
                        pc,
                        invocation_id,
                    } => {
                        let incident = self
                            .handle_ffi_dispatch(
                                &mut instance,
                                &mut fiber,
                                &program,
                                invocation_id,
                                pc,
                            )
                            .await?;
                        if incident {
                            // Fiber parked on incident — stop inner loop.
                            break;
                        }
                        // Dispatch succeeded (Success or NoMatch) — fiber.pc advanced.
                        // Continue inner loop to keep running the fiber.
                        continue;
                    }
                    TickOutcome::Parked(WaitState::Job { .. }) => {
                        // Job enqueued by VM — leave in queue for activate_jobs()
                        break;
                    }
                    TickOutcome::Ended => {
                        // Fiber ended — check if all fibers are done
                        let remaining = self.store.load_fibers(instance_id).await?;
                        if remaining.is_empty() {
                            self.store
                                .update_instance_state(
                                    instance_id,
                                    ProcessState::Completed { at: now_ms() },
                                )
                                .await?;
                            self.store
                                .append_event(
                                    instance_id,
                                    &RuntimeEvent::Completed { at: now_ms() },
                                )
                                .await?;
                        }
                        break;
                    }
                    TickOutcome::Terminated => {
                        // EndTerminate: kill the entire instance immediately.
                        let terminating_fiber_id = fiber.fiber_id;

                        // 1. Emit WaitCancelled for all OTHER fibers with active waits
                        let all_fibers = self.store.load_fibers(instance_id).await?;
                        for other in &all_fibers {
                            if other.fiber_id == terminating_fiber_id {
                                continue;
                            }
                            let wait_desc = describe_wait(&other.wait);
                            if !wait_desc.is_empty() {
                                self.store
                                    .append_event(
                                        instance_id,
                                        &RuntimeEvent::WaitCancelled {
                                            fiber_id: other.fiber_id,
                                            wait_desc,
                                            reason: "terminate_end_event".to_string(),
                                        },
                                    )
                                    .await?;
                            }
                        }

                        // 2. Purge pending/inflight jobs
                        self.store.cancel_jobs_for_instance(instance_id).await?;

                        // 3. Delete ALL fibers (including the terminating one)
                        self.store.delete_all_fibers(instance_id).await?;

                        // 4. Set state → Terminated
                        let at = now_ms();
                        self.store
                            .update_instance_state(instance_id, ProcessState::Terminated { at })
                            .await?;

                        // 5. Emit Terminated event
                        self.store
                            .append_event(
                                instance_id,
                                &RuntimeEvent::Terminated {
                                    at,
                                    fiber_id: terminating_fiber_id,
                                },
                            )
                            .await?;

                        // 6. BREAK — no more fibers to process
                        break 'fiber_loop;
                    }
                    _ => {
                        // Continue → hit max_steps; parked on timer/msg/join/race/incident → break inner loop
                        break;
                    }
                }
            } // end inner FFI-retry loop
        } // end 'fiber_loop

        // --- Boundary timer promotion pass: Job → Race ---
        let fibers_for_promotion = self.store.load_fibers(instance_id).await?;
        let now_promo = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        for fiber in &fibers_for_promotion {
            if let WaitState::Job { ref job_key } = fiber.wait {
                if let Some(&race_id) = program.boundary_map.get(&fiber.pc) {
                    if let Some(race_entry) = program.race_plan.get(&race_id) {
                        // Compute absolute deadline from timer arm
                        let timer_deadline_ms = race_entry.arms.iter().find_map(|arm| match arm {
                            WaitArm::Timer { duration_ms, .. } => Some(now_promo + duration_ms),
                            WaitArm::Deadline { deadline_ms, .. } => Some(*deadline_ms),
                            _ => None,
                        });

                        // Compute timer_arm_index for non-interrupting logic
                        let timer_arm_index = race_entry.arms.iter().position(|arm| {
                            matches!(arm, WaitArm::Timer { .. } | WaitArm::Deadline { .. })
                        });

                        // Read interrupting flag from timer arm (default true for safety)
                        let interrupting = race_entry
                            .arms
                            .iter()
                            .find_map(|arm| match arm {
                                WaitArm::Timer { interrupting, .. } => Some(*interrupting),
                                _ => None,
                            })
                            .unwrap_or(true);

                        // Read cycle spec from timer arm
                        let cycle_remaining = race_entry.arms.iter().find_map(|arm| match arm {
                            WaitArm::Timer { cycle: Some(c), .. } => Some(c.max_fires),
                            _ => None,
                        });

                        // Promote: Job → Race, PRESERVING the exact job_key
                        let mut promoted = fiber.clone();
                        promoted.wait = WaitState::Race {
                            race_id,
                            timer_deadline_ms,
                            job_key: Some(job_key.clone()),
                            interrupting,
                            timer_arm_index,
                            cycle_remaining,
                            cycle_fired_count: 0,
                        };
                        self.store.save_fiber(instance_id, &promoted).await?;

                        // Emit RaceRegistered event
                        let arm_descs: Vec<bpmn_lite_types::events::WaitArmDesc> =
                            race_entry.arms.iter().map(|a| a.into()).collect();
                        self.store
                            .append_event(
                                instance_id,
                                &RuntimeEvent::RaceRegistered {
                                    race_id,
                                    fiber_id: fiber.fiber_id,
                                    arms: arm_descs,
                                },
                            )
                            .await?;
                    }
                }
            }
        }

        // --- Race timer check pass ---
        let fibers_after = self.store.load_fibers(instance_id).await?;
        let now_race = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        for fiber in fibers_after {
            if let WaitState::Race {
                race_id,
                timer_deadline_ms,
                ref job_key,
                interrupting,
                timer_arm_index,
                cycle_remaining,
                cycle_fired_count,
            } = &fiber.wait
            {
                let race_id = *race_id;
                let stored_job_key = job_key.clone();
                let is_interrupting = *interrupting;
                let stored_timer_arm_index = *timer_arm_index;
                let stored_cycle_remaining = *cycle_remaining;
                let stored_cycle_fired_count = *cycle_fired_count;

                // Check explicit timer_deadline_ms first (boundary timer promotion path)
                if let Some(deadline) = timer_deadline_ms {
                    if now_race >= *deadline {
                        if let Some(race_entry) = program.race_plan.get(&race_id) {
                            // Use stored timer_arm_index if available, else compute
                            let timer_arm_idx = stored_timer_arm_index.or_else(|| {
                                race_entry.arms.iter().position(|arm| {
                                    matches!(arm, WaitArm::Timer { .. } | WaitArm::Deadline { .. })
                                })
                            });

                            if let Some(idx) = timer_arm_idx {
                                if is_interrupting {
                                    // ── INTERRUPTING: resolve race (existing behavior) ──
                                    let mut fiber = fiber.clone();
                                    let mut instance =
                                        self.store.load_instance(instance_id).await?.ok_or_else(
                                            || anyhow!("Instance not found: {}", instance_id),
                                        )?;
                                    let vm = Vm::new(self.store.clone());
                                    vm.resolve_race(
                                        &mut instance,
                                        &mut fiber,
                                        race_id,
                                        idx,
                                        &race_entry.arms,
                                    )
                                    .await?;

                                    // Ack the pending job
                                    if let Some(jk) = &stored_job_key {
                                        let _ = self.store.ack_job(jk).await;
                                    }
                                } else {
                                    // ── NON-INTERRUPTING: fork child fiber, main stays in Race ──
                                    let resume_at = match &race_entry.arms[idx] {
                                        WaitArm::Timer { resume_at, .. } => *resume_at,
                                        WaitArm::Deadline { resume_at, .. } => *resume_at,
                                        _ => continue,
                                    };

                                    // Spawn child fiber at escalation path
                                    let child_fiber_id = Uuid::now_v7();
                                    let child_fiber = Fiber::new(child_fiber_id, resume_at);
                                    self.store.save_fiber(instance_id, &child_fiber).await?;

                                    // Emit BoundaryFired event
                                    let boundary_element_id =
                                        race_entry.boundary_element_id.clone().unwrap_or_default();
                                    self.store
                                        .append_event(
                                            instance_id,
                                            &RuntimeEvent::BoundaryFired {
                                                race_id,
                                                fiber_id: fiber.fiber_id,
                                                spawned_fiber_id: child_fiber_id,
                                                boundary_element_id,
                                                resume_at,
                                            },
                                        )
                                        .await?;

                                    let new_fired_count = stored_cycle_fired_count + 1;

                                    // Emit TimerCycleIteration
                                    let new_remaining =
                                        stored_cycle_remaining.map(|r| r.saturating_sub(1));
                                    self.store
                                        .append_event(
                                            instance_id,
                                            &RuntimeEvent::TimerCycleIteration {
                                                race_id,
                                                fiber_id: fiber.fiber_id,
                                                iteration: new_fired_count,
                                                remaining: new_remaining.unwrap_or(0),
                                            },
                                        )
                                        .await?;

                                    // Check if cycle exhausted
                                    let cycle_exhausted = new_remaining == Some(0);

                                    if cycle_exhausted {
                                        // All cycles consumed — emit exhausted, resolve race
                                        self.store
                                            .append_event(
                                                instance_id,
                                                &RuntimeEvent::TimerCycleExhausted {
                                                    race_id,
                                                    fiber_id: fiber.fiber_id,
                                                    total_fired: new_fired_count,
                                                },
                                            )
                                            .await?;

                                        // Remove timer from race — fiber goes back to plain Job wait
                                        let mut updated_fiber = fiber.clone();
                                        if let Some(jk) = &stored_job_key {
                                            updated_fiber.wait = WaitState::Job {
                                                job_key: jk.clone(),
                                            };
                                        }
                                        self.store.save_fiber(instance_id, &updated_fiber).await?;
                                    } else {
                                        // Re-register timer with new deadline + updated counts
                                        let new_deadline = match &race_entry.arms[idx] {
                                            WaitArm::Timer { duration_ms, .. } => {
                                                now_race + duration_ms
                                            }
                                            _ => now_race + 60_000, // fallback 1min
                                        };

                                        let mut updated_fiber = fiber.clone();
                                        updated_fiber.wait = WaitState::Race {
                                            race_id,
                                            timer_deadline_ms: Some(new_deadline),
                                            job_key: stored_job_key.clone(),
                                            interrupting: false,
                                            timer_arm_index: stored_timer_arm_index,
                                            cycle_remaining: new_remaining,
                                            cycle_fired_count: new_fired_count,
                                        };
                                        self.store.save_fiber(instance_id, &updated_fiber).await?;
                                    }
                                }
                            }
                        }
                        continue;
                    }
                }

                // Fallback: check Deadline arms in race_plan (WaitAny opcode path)
                if let Some(race_entry) = program.race_plan.get(&race_id) {
                    for (i, arm) in race_entry.arms.iter().enumerate() {
                        let expired = match arm {
                            WaitArm::Deadline { deadline_ms, .. } => now_race >= *deadline_ms,
                            _ => false,
                        };

                        if expired {
                            let mut fiber = fiber.clone();
                            let mut instance = self
                                .store
                                .load_instance(instance_id)
                                .await?
                                .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;
                            let vm = Vm::new(self.store.clone());
                            vm.resolve_race(
                                &mut instance,
                                &mut fiber,
                                race_id,
                                i,
                                &race_entry.arms,
                            )
                            .await?;
                            break; // Only one winner per race
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Advance all runnable fibers and return any job activations.
    /// Convenience method for in-process use — dequeues jobs immediately.
    pub async fn run_instance(&self, instance_id: Uuid) -> Result<Vec<JobActivation>> {
        self.tick_instance(instance_id).await?;

        let instance = self
            .store
            .load_instance(instance_id)
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
                &program.task_manifest,
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
    async fn emit_late(&self, instance_id: Uuid, desc: String) -> Result<()> {
        self.store
            .append_event(
                instance_id,
                &RuntimeEvent::SignalIgnored { signal_desc: desc },
            )
            .await?;
        Ok(())
    }

    /// Dispatch an in-process FFI call (ExecFfi opcode path).
    ///
    /// Serialises input bindings from instance state, calls the registered
    /// FfiDispatcher, applies output bindings, writes audit events, and
    /// advances `fiber.pc`.
    ///
    /// Returns `true` if the call produced an incident (fiber is now
    /// parked on `WaitState::Incident`), `false` for Success/NoMatch.
    async fn handle_ffi_dispatch(
        &self,
        instance: &mut ProcessInstance,
        fiber: &mut Fiber,
        program: &CompiledProgram,
        invocation_id: Uuid,
        pc: Addr,
    ) -> Result<bool> {
        // No dispatcher → create an incident.
        let Some(dispatcher) = &self.ffi_dispatcher else {
            let incident_id = self
                .create_incident(
                    instance,
                    fiber,
                    pc,
                    ErrorClass::ContractViolation,
                    "ExecFfi reached engine with no FfiDispatcher configured",
                )
                .await?;
            instance.state = ProcessState::Failed { incident_id };
            self.store.save_instance(instance).await?;
            return Ok(true); // incident
        };

        // Get the compiled task declaration for this instruction.
        let task_decl = program
            .ffi_task_decls
            .get(&pc)
            .ok_or_else(|| anyhow!("ExecFfi at pc={} has no FfiTaskDecl in CompiledProgram", pc))?;

        // Caller task id for audit.
        let caller_task_id = program
            .debug_map
            .get(&pc)
            .cloned()
            .unwrap_or_else(|| format!("pc_{}", pc));

        // Serialise input bindings.
        let input_obj = self.build_ffi_input_payload(instance, task_decl, program)?;
        let input_payload = serde_json::to_vec(&input_obj)?;

        // Look up owner_type for audit.
        let owner_type = dispatcher.owner_type_for(&task_decl.template_id).await;

        // Write Pending audit event (before dispatch — A2 §9).
        self.store
            .append_event(
                instance.instance_id,
                &RuntimeEvent::FfiInvocationPending {
                    invocation_id,
                    template_id_hex: bytes_to_hex(&task_decl.template_id),
                    caller_task_id: caller_task_id.clone(),
                    caller_pc: pc,
                    owner_type: owner_type.clone(),
                },
            )
            .await?;

        // Dispatch.
        let call = FfiCall {
            invocation_id,
            template_id: task_decl.template_id,
            tenant_id: instance.tenant_id.clone(),
            process_instance_id: instance.instance_id,
            caller_task_id,
            input_payload,
        };
        let result = dispatcher.dispatch(call).await;

        match result {
            Err(e) => {
                // Transport-level error (owner panicked, dispatch failed internally).
                let msg = format!("FFI dispatch error: {}", e);
                self.store
                    .append_event(
                        instance.instance_id,
                        &RuntimeEvent::FfiInvocationCompleted {
                            invocation_id,
                            outcome_kind: "incident".to_string(),
                            error_message: Some(msg.clone()),
                        },
                    )
                    .await?;
                let incident_id = self
                    .create_incident(instance, fiber, pc, ErrorClass::Transient, &msg)
                    .await?;
                instance.state = ProcessState::Failed { incident_id };
                self.store.save_instance(instance).await?;
                Ok(true)
            }
            Ok(FfiResult::Success {
                output_payload,
                trace_payload: _,
                new_domain_payload,
            }) => {
                // Apply output bindings.
                if let Some(new_payload) = new_domain_payload {
                    instance.domain_payload = Arc::<str>::from(new_payload.as_str());
                    instance.domain_payload_hash = compute_hash(&new_payload);
                } else {
                    self.apply_ffi_outputs(instance, task_decl, &output_payload)?;
                }
                self.store
                    .append_event(
                        instance.instance_id,
                        &RuntimeEvent::FfiInvocationCompleted {
                            invocation_id,
                            outcome_kind: "success".to_string(),
                            error_message: None,
                        },
                    )
                    .await?;
                fiber.pc += 1;
                fiber.wait = WaitState::Running;
                self.store.save_fiber(instance.instance_id, fiber).await?;
                Ok(false)
            }
            Ok(FfiResult::NoMatch { trace_payload: _ }) => {
                // Business "no result" — advance fiber, no output bindings.
                self.store
                    .append_event(
                        instance.instance_id,
                        &RuntimeEvent::FfiInvocationCompleted {
                            invocation_id,
                            outcome_kind: "no_match".to_string(),
                            error_message: None,
                        },
                    )
                    .await?;
                fiber.pc += 1;
                fiber.wait = WaitState::Running;
                self.store.save_fiber(instance.instance_id, fiber).await?;
                Ok(false)
            }
            Ok(FfiResult::Incident {
                error_class,
                message,
                retry_hint_ms: _,
            }) => {
                let ec = ffi_incident_class_to_error_class(error_class);
                // Check error_route_map for BusinessRejection routing.
                if let ErrorClass::BusinessRejection { ref rejection_code } = ec {
                    if let Some(routes) = program.error_route_map.get(&pc) {
                        let route = routes
                            .iter()
                            .find(|r| r.error_code.as_deref() == Some(rejection_code.as_str()))
                            .or_else(|| routes.iter().find(|r| r.error_code.is_none()));
                        if let Some(r) = route {
                            self.store
                                .append_event(
                                    instance.instance_id,
                                    &RuntimeEvent::FfiInvocationCompleted {
                                        invocation_id,
                                        outcome_kind: "incident".to_string(),
                                        error_message: Some(message.clone()),
                                    },
                                )
                                .await?;
                            self.store
                                .append_event(
                                    instance.instance_id,
                                    &RuntimeEvent::ErrorRouted {
                                        job_key: format!("ffi:{}", invocation_id),
                                        error_code: rejection_code.clone(),
                                        boundary_id: r.boundary_element_id.clone(),
                                        resume_at: r.resume_at,
                                    },
                                )
                                .await?;
                            fiber.pc = r.resume_at;
                            fiber.wait = WaitState::Running;
                            self.store.save_fiber(instance.instance_id, fiber).await?;
                            return Ok(false); // routed, not an incident
                        }
                    }
                }
                self.store
                    .append_event(
                        instance.instance_id,
                        &RuntimeEvent::FfiInvocationCompleted {
                            invocation_id,
                            outcome_kind: "incident".to_string(),
                            error_message: Some(message.clone()),
                        },
                    )
                    .await?;
                let incident_id = self
                    .create_incident(instance, fiber, pc, ec, &message)
                    .await?;
                instance.state = ProcessState::Failed { incident_id };
                self.store.save_instance(instance).await?;
                Ok(true)
            }
        }
    }

    /// Create an Incident record and park the fiber.
    async fn create_incident(
        &self,
        instance: &mut ProcessInstance,
        fiber: &mut Fiber,
        pc: Addr,
        error_class: ErrorClass,
        message: &str,
    ) -> Result<Uuid> {
        let incident_id = Uuid::now_v7();
        let service_task_id = format!("pc_{}", pc);
        let incident = Incident {
            incident_id,
            process_instance_id: instance.instance_id,
            fiber_id: fiber.fiber_id,
            service_task_id: service_task_id.clone(),
            bytecode_addr: pc,
            error_class,
            message: message.to_string(),
            retry_count: 0,
            created_at: now_ms(),
            resolved_at: None,
            resolution: None,
        };
        self.store.save_incident(&incident).await?;
        self.store
            .append_event(
                instance.instance_id,
                &RuntimeEvent::IncidentCreated {
                    incident_id,
                    service_task_id,
                    job_key: None,
                },
            )
            .await?;
        fiber.wait = WaitState::Incident { incident_id };
        self.store.save_fiber(instance.instance_id, fiber).await?;
        Ok(incident_id)
    }

    /// Serialise FFI input bindings from the current instance state.
    fn build_ffi_input_payload(
        &self,
        instance: &ProcessInstance,
        task_decl: &bpmn_lite_types::ffi_bindings::FfiTaskDecl,
        program: &CompiledProgram,
    ) -> Result<serde_json::Value> {
        let mut obj = serde_json::Map::new();
        let parsed_payload: Option<serde_json::Value> = if task_decl
            .inputs
            .iter()
            .any(|b| matches!(&b.source, BindingSource::DomainPayloadRef(_)))
        {
            Some(json_path::parse_json(&instance.domain_payload)?)
        } else {
            None
        };
        for binding in &task_decl.inputs {
            let value: serde_json::Value = match &binding.source {
                BindingSource::Literal(lit) => literal_to_json(lit),
                BindingSource::FlagRef(key) => match instance.flags.get(key) {
                    Some(Value::Bool(b)) => serde_json::Value::Bool(*b),
                    Some(Value::I64(n)) => serde_json::Value::Number((*n).into()),
                    Some(Value::Str(_)) | Some(Value::Ref(_)) | None => serde_json::Value::Null,
                },
                BindingSource::DomainPayloadRef(path) => {
                    if let Some(ref root) = parsed_payload {
                        json_path::read(root, path).unwrap_or(serde_json::Value::Null)
                    } else {
                        serde_json::Value::Null
                    }
                }
            };
            obj.insert(binding.target_field.clone(), value);
        }
        // suppress unused-variable warning — program only needed for future use
        let _ = program;
        Ok(serde_json::Value::Object(obj))
    }

    /// Apply FFI output bindings back into instance state.
    fn apply_ffi_outputs(
        &self,
        instance: &mut ProcessInstance,
        task_decl: &bpmn_lite_types::ffi_bindings::FfiTaskDecl,
        output_payload_bytes: &[u8],
    ) -> Result<()> {
        if task_decl.outputs.is_empty() {
            return Ok(());
        }
        let output: serde_json::Value = serde_json::from_slice(output_payload_bytes)
            .map_err(|e| anyhow!("FFI output_payload is not valid JSON: {}", e))?;

        // For DomainPayloadWrite: parse domain_payload once.
        let needs_domain_write = task_decl
            .outputs
            .iter()
            .any(|b| matches!(&b.target, BindingTarget::DomainPayloadWrite(_)));
        let mut parsed_domain: Option<serde_json::Value> = if needs_domain_write {
            Some(json_path::parse_json(&instance.domain_payload)?)
        } else {
            None
        };

        for binding in &task_decl.outputs {
            let field_value = output.get(&binding.source_field);
            match &binding.target {
                BindingTarget::FlagWrite(key) => {
                    let v = match field_value {
                        Some(serde_json::Value::Bool(b)) => Value::Bool(*b),
                        Some(serde_json::Value::Number(n)) if n.is_i64() => {
                            Value::I64(n.as_i64().unwrap())
                        }
                        _ => continue, // skip non-flag-compatible values
                    };
                    instance.flags.insert(*key, v);
                }
                BindingTarget::DomainPayloadWrite(path) => {
                    if let Some(ref mut root) = parsed_domain {
                        if let Some(field_val) = field_value {
                            json_path::write_at_path(root, path, field_val.clone())?;
                        }
                    }
                }
            }
        }

        if let Some(new_domain) = parsed_domain {
            let canonical = json_path::canonicalise_json(&new_domain);
            instance.domain_payload = Arc::<str>::from(canonical.as_str());
            instance.domain_payload_hash = compute_hash(&canonical);
        }
        Ok(())
    }

    /// Complete a job — resume the parked fiber.
    ///
    /// Three guards protect against ghost signals:
    ///   1. Dedupe — already-processed job_key is silently accepted
    ///   2. State  — Cancelled/Completed instance → SignalIgnored event
    ///   3. Hash   — worker-supplied expected/current hash must match instance snapshot
    pub async fn complete_job(
        &self,
        job_key: &str,
        domain_payload: &str,
        expected_instance_payload_hash: [u8; 32],
        orch_flags: BTreeMap<String, Value>,
    ) -> Result<()> {
        let (instance_id, _task_type_id, _pc) = parse_job_key(job_key)?;
        let owner = self.transition_owner.clone();
        self.run_guarded_transition(instance_id, &owner, || async {
            self.complete_job_inner(
                job_key,
                domain_payload,
                expected_instance_payload_hash,
                orch_flags,
            )
            .await
        })
        .await
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
        if !self
            .store
            .validate_job_claim(job_key, worker_id, claim_token)
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

    async fn complete_job_inner(
        &self,
        job_key: &str,
        domain_payload: &str,
        expected_instance_payload_hash: [u8; 32],
        orch_flags: BTreeMap<String, Value>,
    ) -> Result<()> {
        let (instance_id, _task_type_id, pc) = parse_job_key(job_key)?;

        // ── Guard 1: dedupe ──
        if self.store.dedupe_get(job_key).await?.is_some() {
            return Ok(());
        }

        // ── Guard 2: instance state ──
        let mut instance = self
            .store
            .load_instance(instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;

        if instance.state.is_terminal() {
            self.emit_late(
                instance_id,
                format!("complete_job on {:?} instance: {}", instance.state, job_key),
            )
            .await?;
            return Ok(());
        }

        // ── Guard 3: payload hash ──
        let expected = compute_hash(&instance.domain_payload);
        if expected_instance_payload_hash != expected {
            return Err(anyhow!(
                "Payload hash mismatch on complete_job: job_key={}",
                job_key
            ));
        }

        let program = self
            .store
            .load_program(instance.bytecode_version)
            .await?
            .ok_or_else(|| anyhow!("Program not found"))?;

        let completion = JobCompletion {
            job_key: job_key.to_string(),
            domain_payload: domain_payload.to_string(),
            expected_instance_payload_hash,
            orch_flags,
        };

        let vm = Vm::new(self.store.clone());

        // Check if this job's ExecNative has a boundary timer (race)
        let boundary_race_id = program.boundary_map.get(&pc).copied();
        let resumed = if let Some(race_id) = boundary_race_id {
            // Race-aware completion: find fiber in Race state
            let fibers = self.store.load_fibers(instance_id).await?;
            let race_fiber = fibers.iter().find(
                |f| matches!(&f.wait, WaitState::Race { race_id: rid, .. } if *rid == race_id),
            );

            if let Some(parked) = race_fiber {
                let mut fiber = parked.clone();
                if let Some(race_entry) = program.race_plan.get(&race_id) {
                    let internal_idx = race_entry
                        .arms
                        .iter()
                        .position(|arm| matches!(arm, WaitArm::Internal { .. }))
                        .unwrap_or(0);

                    vm.resolve_race(
                        &mut instance,
                        &mut fiber,
                        race_id,
                        internal_idx,
                        &race_entry.arms,
                    )
                    .await?;
                }
                true // race path always means fiber was resumed
            } else {
                // Fiber still in Job state (promotion hasn't happened)
                // or race already resolved (late arrival) — try vm path
                let maybe_fid = vm
                    .complete_job(&mut instance, &completion, &program)
                    .await?;
                maybe_fid.is_some()
            }
        } else {
            // No boundary timer — standard complete_job path
            let maybe_fid = vm
                .complete_job(&mut instance, &completion, &program)
                .await?;
            maybe_fid.is_some()
        };

        if resumed {
            // Mutation ownership: engine applies completion + persists atomically
            let payload_hash_before = instance.domain_payload_hash;
            apply_completion(&mut instance, &completion);
            let events = if boundary_race_id.is_none() {
                vec![RuntimeEvent::JobCompleted {
                    job_key: completion.job_key.clone(),
                    payload_hash_before,
                    payload_hash_after: instance.domain_payload_hash,
                    orch_flags_out: completion.orch_flags.clone(),
                    pc_next: pc + 1,
                }]
            } else {
                Vec::new()
            };
            self.store
                .atomic_complete(&instance, &completion, &events)
                .await?;
        } else {
            // No fiber matched — ghost signal
            self.emit_late(
                instance_id,
                format!("complete_job: no matching fiber for job_key={}", job_key),
            )
            .await?;
        }

        Ok(())
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
        let (instance_id, _task_type_id, _pc) = parse_job_key(job_key)?;
        let owner = self.transition_owner.clone();
        self.run_guarded_transition(instance_id, &owner, || async {
            self.fail_job_inner(job_key, error_class, message, None)
                .await
        })
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
        if !self
            .store
            .validate_job_claim(job_key, worker_id, claim_token)
            .await?
        {
            return Err(anyhow!("job claim does not match worker ownership"));
        }
        let (instance_id, _task_type_id, _pc) = parse_job_key(job_key)?;
        let owner = self.transition_owner.clone();
        self.run_guarded_transition(instance_id, &owner, || async {
            self.fail_job_inner(
                job_key,
                error_class,
                message,
                Some((worker_id, claim_token)),
            )
            .await
        })
        .await
    }

    async fn fail_job_inner(
        &self,
        job_key: &str,
        error_class: ErrorClass,
        message: &str,
        worker_claim: Option<(&str, &str)>,
    ) -> Result<()> {
        let (instance_id, _task_type_id, _pc) = parse_job_key(job_key)?;

        let instance = self
            .store
            .load_instance(instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;

        // Guard 1: terminal state
        if instance.state.is_terminal() {
            self.emit_late(
                instance_id,
                format!("fail_job(key={}, state={:?})", job_key, instance.state),
            )
            .await?;
            self.store.ack_job(job_key).await?;
            return Ok(());
        }

        let program = self
            .store
            .load_program(instance.bytecode_version)
            .await?
            .ok_or_else(|| anyhow!("Program not found"))?;

        // Find parked fiber
        let fibers = self.store.load_fibers(instance_id).await?;
        let parked = fibers
            .iter()
            .find(|f| matches!(&f.wait, WaitState::Job { job_key: jk } if jk == job_key));

        let Some(parked_fiber) = parked else {
            // Guard 2: no matching fiber (ghost)
            self.emit_late(instance_id, format!("fail_job(key={}, no fiber)", job_key))
                .await?;
            self.store.ack_job(job_key).await?;
            return Ok(());
        };

        let mut fiber = parked_fiber.clone();

        if matches!(error_class, ErrorClass::Transient) {
            if let Some((worker_id, claim_token)) = worker_claim {
                let retry_at = now_ms() + 1;
                if self
                    .store
                    .retry_claimed_job(
                        job_key,
                        worker_id,
                        claim_token,
                        error_class_label(&error_class),
                        message,
                        retry_at,
                    )
                    .await?
                {
                    self.store
                        .append_event(
                            instance_id,
                            &RuntimeEvent::JobRetryScheduled {
                                job_key: job_key.to_string(),
                                retry_at,
                                retries_remaining: 0,
                            },
                        )
                        .await?;
                    return Ok(());
                }
            }
        }

        // Check error routing (only for BusinessRejection)
        if let ErrorClass::BusinessRejection { rejection_code } = &error_class {
            if let Some(routes) = program.error_route_map.get(&fiber.pc) {
                let matched = routes.iter().find(|r| match &r.error_code {
                    Some(code) => code == rejection_code,
                    None => true, // catch-all
                });

                if let Some(route) = matched {
                    // ERROR ROUTE PATH: advance fiber to escalation
                    fiber.pc = route.resume_at;
                    fiber.wait = WaitState::Running;
                    self.store.save_fiber(instance_id, &fiber).await?;
                    self.store.ack_job(job_key).await?;
                    self.store
                        .append_event(
                            instance_id,
                            &RuntimeEvent::ErrorRouted {
                                job_key: job_key.to_string(),
                                error_code: rejection_code.clone(),
                                boundary_id: route.boundary_element_id.clone(),
                                resume_at: route.resume_at,
                            },
                        )
                        .await?;
                    return Ok(());
                }
            }
        }

        // INCIDENT PATH: no route matched (or not BusinessRejection)
        let incident_id = Uuid::now_v7();
        let service_task_id = program
            .debug_map
            .get(&fiber.pc)
            .cloned()
            .unwrap_or_else(|| format!("pc_{}", fiber.pc));

        let incident = Incident {
            incident_id,
            process_instance_id: instance_id,
            fiber_id: fiber.fiber_id,
            service_task_id: service_task_id.clone(),
            bytecode_addr: fiber.pc,
            error_class,
            message: message.to_string(),
            retry_count: 0,
            created_at: now_ms(),
            resolved_at: None,
            resolution: None,
        };

        self.store.save_incident(&incident).await?;
        self.store
            .append_event(
                instance_id,
                &RuntimeEvent::IncidentCreated {
                    incident_id,
                    service_task_id,
                    job_key: Some(job_key.to_string()),
                },
            )
            .await?;

        fiber.wait = WaitState::Incident { incident_id };
        self.store.save_fiber(instance_id, &fiber).await?;

        let mut instance = instance;
        instance.state = ProcessState::Failed { incident_id };
        self.store.save_instance(&instance).await?;
        if let Some((worker_id, claim_token)) = worker_claim {
            let _ = self
                .store
                .dead_letter_claimed_job(
                    job_key,
                    worker_id,
                    claim_token,
                    error_class_label(&incident.error_class),
                    message,
                    incident_id,
                )
                .await?;
            self.store
                .append_event(
                    instance_id,
                    &RuntimeEvent::JobDeadLettered {
                        job_key: job_key.to_string(),
                        incident_id,
                    },
                )
                .await?;
        } else {
            self.store.ack_job(job_key).await?;
        }

        Ok(())
    }

    /// Signal a waiting fiber (message correlation).
    /// Handles both plain WaitMsg and Race (WaitAny with Msg arm).
    ///
    /// Guards: Cancelled/Completed instances get SignalIgnored.
    /// No-match (no fiber waiting for a message) also emits SignalIgnored.
    pub async fn signal(
        &self,
        instance_id: Uuid,
        _msg_name: &str,
        corr_key: &str,
        domain_payload: Option<&str>,
        domain_payload_hash: Option<[u8; 32]>,
        _msg_id: Option<&str>,
    ) -> Result<()> {
        let corr_key = parse_signal_corr_key(corr_key);
        self.signal_with_value(
            instance_id,
            _msg_name,
            corr_key,
            domain_payload,
            domain_payload_hash,
            _msg_id,
        )
        .await
    }

    pub async fn signal_with_value(
        &self,
        instance_id: Uuid,
        msg_name: &str,
        corr_key: Value,
        domain_payload: Option<&str>,
        domain_payload_hash: Option<[u8; 32]>,
        msg_id: Option<&str>,
    ) -> Result<()> {
        let owner = self.transition_owner.clone();
        self.run_guarded_transition(instance_id, &owner, || async {
            self.signal_inner(
                instance_id,
                msg_name,
                corr_key,
                domain_payload,
                domain_payload_hash,
                msg_id,
            )
            .await
        })
        .await
    }

    async fn signal_inner(
        &self,
        instance_id: Uuid,
        msg_name: &str,
        corr_key: Value,
        domain_payload: Option<&str>,
        domain_payload_hash: Option<[u8; 32]>,
        msg_id: Option<&str>,
    ) -> Result<()> {
        let msg_id = msg_id
            .filter(|msg_id| !msg_id.is_empty())
            .ok_or_else(|| anyhow!("msg_id is required for idempotent signal delivery"))?;
        let mut instance = self
            .store
            .load_instance(instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;

        // ── State guard ──
        if instance.state.is_terminal() {
            if !self
                .store
                .record_message_delivery(&self.tenant_id, instance_id, msg_id)
                .await?
            {
                return Ok(());
            }
            self.emit_late(
                instance_id,
                format!("signal on {:?} instance: msg={}", instance.state, msg_name),
            )
            .await?;
            return Ok(());
        }

        let correlation_key = value_key(&corr_key);
        let payload = domain_payload.map(str::as_bytes).unwrap_or(&[]);
        let buffer_result = self
            .store
            .buffer_message(
                &self.tenant_id,
                msg_name,
                &correlation_key,
                msg_id,
                payload,
                domain_payload_hash,
                DEFAULT_MESSAGE_TTL_MS,
                Some(instance_id),
            )
            .await?;
        let buffered_event = matches!(buffer_result, BufferMessageResult::Inserted).then(|| {
            RuntimeEvent::MessageBuffered {
                message_name: msg_name.to_string(),
                correlation_key: correlation_key.clone(),
                msg_id: msg_id.to_string(),
                expires_at: now_ms() + DEFAULT_MESSAGE_TTL_MS as i64,
            }
        });

        let program = self
            .store
            .load_program(instance.bytecode_version)
            .await?
            .ok_or_else(|| anyhow!("Program not found"))?;

        let fibers = self.store.load_fibers(instance_id).await?;

        for fiber in fibers {
            match fiber.wait.clone() {
                // Existing: plain WaitMsg
                WaitState::Msg {
                    name,
                    corr_key: waiting_corr_key,
                    ..
                } if signal_name_matches(&program, msg_name, name)
                    && waiting_corr_key == corr_key =>
                {
                    let mut fiber = fiber;
                    fiber.wait = WaitState::Running;
                    if let Some(claimed) = self
                        .store
                        .claim_buffered_message(
                            &self.tenant_id,
                            msg_name,
                            &correlation_key,
                            DEFAULT_MESSAGE_CLAIM_MS,
                        )
                        .await?
                    {
                        let payload_update = match (domain_payload, domain_payload_hash) {
                            (Some(payload), Some(hash)) => Some(PayloadUpdate {
                                payload: payload.to_string(),
                                payload_hash: hash,
                            }),
                            _ => None,
                        };
                        let mut events = Vec::new();
                        if let Some(event) = buffered_event.clone() {
                            events.push(event);
                        }
                        events.push(RuntimeEvent::BufferedMessageConsumed {
                            message_name: msg_name.to_string(),
                            correlation_key: correlation_key.clone(),
                            msg_id: msg_id.to_string(),
                            fiber_id: fiber.fiber_id,
                        });
                        events.push(RuntimeEvent::MsgReceived {
                            name,
                            corr_key: corr_key.clone(),
                            msg_ref: None,
                        });
                        if self
                            .store
                            .atomic_consume_buffered_message(
                                &instance,
                                &fiber,
                                &claimed,
                                payload_update.as_ref(),
                                &events,
                            )
                            .await?
                        {
                            if let Some(payload_update) = payload_update {
                                instance.domain_payload =
                                    Arc::from(payload_update.payload.as_str());
                                instance.domain_payload_hash = payload_update.payload_hash;
                            }
                            return Ok(());
                        }
                        let _ = self.store.release_buffered_message_claim(&claimed).await?;
                    }
                }

                // Race — check if any Msg arm matches
                WaitState::Race { race_id, .. } => {
                    if let Some(race_entry) = program.race_plan.get(&race_id) {
                        for (i, arm) in race_entry.arms.iter().enumerate() {
                            if let WaitArm::Msg { name, corr_reg, .. } = arm {
                                let waiting_corr_key = if (*corr_reg as usize) < fiber.regs.len() {
                                    fiber.regs[*corr_reg as usize].clone()
                                } else {
                                    Value::Bool(false)
                                };
                                if !signal_name_matches(&program, msg_name, *name)
                                    || waiting_corr_key != corr_key
                                {
                                    continue;
                                }
                                let Some(claimed) = self
                                    .store
                                    .claim_buffered_message(
                                        &self.tenant_id,
                                        msg_name,
                                        &correlation_key,
                                        DEFAULT_MESSAGE_CLAIM_MS,
                                    )
                                    .await?
                                else {
                                    continue;
                                };
                                let mut fiber = fiber.clone();
                                let payload_update = match (domain_payload, domain_payload_hash) {
                                    (Some(payload), Some(hash)) => Some(PayloadUpdate {
                                        payload: payload.to_string(),
                                        payload_hash: hash,
                                    }),
                                    _ => None,
                                };
                                let resume_at = arm.resume_at();
                                fiber.pc = resume_at;
                                fiber.wait = WaitState::Running;
                                let mut events = Vec::new();
                                if let Some(event) = buffered_event.clone() {
                                    events.push(event);
                                }
                                events.push(RuntimeEvent::BufferedMessageConsumed {
                                    message_name: msg_name.to_string(),
                                    correlation_key: correlation_key.clone(),
                                    msg_id: msg_id.to_string(),
                                    fiber_id: fiber.fiber_id,
                                });
                                events.push(RuntimeEvent::MsgReceived {
                                    name: *name,
                                    corr_key: corr_key.clone(),
                                    msg_ref: None,
                                });
                                events.push(RuntimeEvent::RaceWon {
                                    race_id,
                                    fiber_id: fiber.fiber_id,
                                    winner_index: i,
                                    resume_at,
                                });
                                let cancelled_indices: Vec<usize> =
                                    (0..race_entry.arms.len()).filter(|idx| *idx != i).collect();
                                if !cancelled_indices.is_empty() {
                                    events.push(RuntimeEvent::RaceCancelled {
                                        race_id,
                                        cancelled_indices,
                                    });
                                }
                                if self
                                    .store
                                    .atomic_consume_buffered_message(
                                        &instance,
                                        &fiber,
                                        &claimed,
                                        payload_update.as_ref(),
                                        &events,
                                    )
                                    .await?
                                {
                                    if let Some(payload_update) = payload_update {
                                        instance.domain_payload =
                                            Arc::from(payload_update.payload.as_str());
                                        instance.domain_payload_hash = payload_update.payload_hash;
                                    }
                                    return Ok(());
                                }
                                let _ = self.store.release_buffered_message_claim(&claimed).await?;
                            }
                        }
                    }
                }

                _ => continue,
            }
        }

        if let Some(event) = buffered_event {
            self.store.append_event(instance_id, &event).await?;
        }
        Ok(())
    }

    /// Cancel a process instance.
    ///
    /// Emits WaitCancelled per parked fiber, purges pending/inflight jobs,
    /// then deletes all fibers and marks instance Cancelled.
    pub async fn cancel(&self, instance_id: Uuid, reason: &str) -> Result<()> {
        let owner = self.transition_owner.clone();
        self.run_guarded_transition(instance_id, &owner, || async {
            self.cancel_inner(instance_id, reason).await
        })
        .await
    }

    async fn cancel_inner(&self, instance_id: Uuid, reason: &str) -> Result<()> {
        // 1. Emit WaitCancelled per parked fiber (before deletion)
        let fibers = self.store.load_fibers(instance_id).await?;
        for fiber in &fibers {
            let wait_desc = describe_wait(&fiber.wait);
            if !wait_desc.is_empty() {
                self.store
                    .append_event(
                        instance_id,
                        &RuntimeEvent::WaitCancelled {
                            fiber_id: fiber.fiber_id,
                            wait_desc,
                            reason: reason.to_string(),
                        },
                    )
                    .await?;
            }
        }

        // 2. Purge pending + inflight jobs for this instance
        self.store.cancel_jobs_for_instance(instance_id).await?;

        // 3. Update state, delete fibers, emit Cancelled
        self.store
            .update_instance_state(
                instance_id,
                ProcessState::Cancelled {
                    reason: reason.to_string(),
                    at: now_ms(),
                },
            )
            .await?;
        self.store.delete_all_fibers(instance_id).await?;
        self.store
            .append_event(
                instance_id,
                &RuntimeEvent::Cancelled {
                    reason: reason.to_string(),
                },
            )
            .await?;
        Ok(())
    }

    /// Inspect a process instance.
    pub async fn inspect(&self, instance_id: Uuid) -> Result<ProcessInspection> {
        let instance = self
            .store
            .load_instance(instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;
        self.ensure_loaded_instance_belongs_to_tenant(&instance, instance_id)?;

        let fibers = self.store.load_fibers(instance_id).await?;
        let fiber_inspections: Vec<FiberInspection> = fibers
            .iter()
            .map(|f| FiberInspection {
                fiber_id: f.fiber_id,
                pc: f.pc,
                wait_state: f.wait.clone(),
                stack_depth: f.stack.len(),
            })
            .collect();

        let incidents = self.store.load_incidents(instance_id).await?;

        Ok(ProcessInspection {
            instance_id,
            tenant_id: instance.tenant_id,
            process_key: instance.process_key,
            bytecode_version: instance.bytecode_version,
            domain_payload_hash: instance.domain_payload_hash,
            state: instance.state,
            fibers: fiber_inspections,
            incidents,
        })
    }

    pub async fn scan_recoverable_inconsistencies(&self) -> Result<Vec<RecoveryIssue>> {
        let mut issues = Vec::new();
        let instance_ids = self.store.list_running_instances(&self.tenant_id).await?;

        for instance_id in instance_ids {
            let Some(instance) = self.store.load_instance(instance_id).await? else {
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
                .load_program(instance.bytecode_version)
                .await?
                .is_none()
            {
                issues.push(RecoveryIssue {
                    instance_id,
                    kind: "missing_program".to_string(),
                    detail: "compiled program for running instance is absent".to_string(),
                });
            }

            if self.store.load_fibers(instance_id).await?.is_empty() {
                issues.push(RecoveryIssue {
                    instance_id,
                    kind: "missing_fibers".to_string(),
                    detail: "running instance has no fibers to resume".to_string(),
                });
            }

            let has_started_event = self
                .store
                .read_events(instance_id, 1)
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
                self.store
                    .append_event(
                        job.process_instance_id,
                        &RuntimeEvent::JobClaimed {
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
            .load_instance(instance_id)
            .await?
            .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;
        self.ensure_loaded_instance_belongs_to_tenant(&instance, instance_id)?;
        self.store.read_events(instance_id, from_seq).await
    }

    pub async fn health_check(&self) -> Result<()> {
        self.store.health_check().await
    }

    /// A17 — Scan running instances for interrupted FFI calls and recover.
    ///
    /// - **Idempotent / IdempotentWithKey:** no action — tick loop re-invokes.
    /// - **NonIdempotent:** creates an Incident on the stalled fiber and marks
    ///   the instance Failed. Operator must resolve the incident manually.
    ///
    /// Returns the number of interrupted calls processed.
    pub async fn detect_interrupted_ffi_calls(&self, tenant_id: &str) -> Result<usize> {
        use bpmn_lite_types::events::RuntimeEvent;
        use ffi_types::Idempotency;
        use std::collections::HashSet;

        let Some(dispatcher) = &self.ffi_dispatcher else {
            return Ok(0);
        };

        let running = self.store.list_running_instances(tenant_id).await?;
        let mut count = 0usize;

        for instance_id in running {
            // A19 note: integrity verification is NOT performed here.
            // This scan runs at startup before the scheduler loop; the
            // scheduler's first tick of each instance fires Boundary (a)
            // verification within seconds. Adding load_instance here would
            // double DB calls for 10k+ running instances (~10s extra startup
            // latency). Boundary (a) + (b) provide sufficient coverage.
            let events = self.store.read_events(instance_id, 0).await?;

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
                    caller_pc,
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
                            "A17: non-idempotent FFI call interrupted — creating incident"
                        );

                        let result: Result<()> = async {
                            let mut instance = self.store.load_instance(instance_id).await?
                                .ok_or_else(|| anyhow!("A17: instance {} not found", instance_id))?;

                            if !matches!(instance.state, ProcessState::Running) {
                                return Ok(());
                            }

                            // Fibers are stored separately from the instance.
                            let mut fibers = self.store.load_fibers(instance_id).await?;
                            let fiber_idx = fibers.iter().position(|f| f.pc == *caller_pc);
                            let Some(idx) = fiber_idx else {
                                tracing::warn!(%instance_id, "A17: fiber at pc={} not found; skipping", caller_pc);
                                return Ok(());
                            };

                            let incident_id = Uuid::now_v7();
                            let fiber = &fibers[idx];
                            let incident = Incident {
                                incident_id,
                                process_instance_id: instance_id,
                                fiber_id: fiber.fiber_id,
                                service_task_id: caller_task_id.clone(),
                                bytecode_addr: *caller_pc,
                                error_class: ErrorClass::ContractViolation,
                                message: format!(
                                    "non-idempotent FFI call to {} interrupted at restart; manual resolution required",
                                    &template_id_hex[..16]
                                ),
                                retry_count: 0,
                                created_at: now_ms(),
                                resolved_at: None,
                                resolution: None,
                            };
                            self.store.save_incident(&incident).await?;
                            self.store.append_event(instance_id, &RuntimeEvent::IncidentCreated {
                                incident_id,
                                service_task_id: caller_task_id.clone(),
                                job_key: None,
                            }).await?;

                            fibers[idx].wait = WaitState::Incident { incident_id };
                            self.store.save_fiber(instance_id, &fibers[idx]).await?;
                            instance.state = ProcessState::Failed { incident_id };
                            self.store.save_instance(&instance).await?;
                            Ok(())
                        }.await;

                        if let Err(e) = result {
                            tracing::warn!(%instance_id, error = %e, "A17: incident creation failed");
                        }
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

fn parse_signal_corr_key(corr_key: &str) -> Value {
    if corr_key.is_empty() {
        return Value::Bool(false);
    }
    if corr_key == "true" {
        return Value::Bool(true);
    }
    if corr_key == "false" {
        return Value::Bool(false);
    }
    if let Ok(n) = corr_key.parse::<i64>() {
        return Value::I64(n);
    }
    if let Some(rest) = corr_key.strip_prefix("str_") {
        if let Ok(n) = rest.parse::<u32>() {
            return Value::Str(n);
        }
    }
    if let Some(rest) = corr_key.strip_prefix("ref_") {
        if let Ok(n) = rest.parse::<u32>() {
            return Value::Ref(n);
        }
    }
    Value::Bool(false)
}

fn signal_name_matches(program: &CompiledProgram, requested_name: &str, waiting_name: u32) -> bool {
    if program
        .message_name_map
        .get(&waiting_name)
        .map(|raw_name| raw_name == requested_name)
        .unwrap_or(false)
    {
        return true;
    }
    requested_name.is_empty()
        || requested_name == "*"
        || requested_name
            .parse::<u32>()
            .map(|name| name == waiting_name)
            .unwrap_or(false)
}

fn value_key(value: &Value) -> String {
    match value {
        Value::Bool(b) => format!("b:{b}"),
        Value::I64(n) => format!("i:{n}"),
        Value::Str(s) => format!("s:{s}"),
        Value::Ref(r) => format!("r:{r}"),
    }
}

fn error_class_label(error_class: &ErrorClass) -> &str {
    match error_class {
        ErrorClass::Transient => "Transient",
        ErrorClass::ContractViolation => "ContractViolation",
        ErrorClass::BusinessRejection { .. } => "BusinessRejection",
    }
}

/// Human-readable description of a fiber's wait state for audit events.
/// Returns empty string for Running (not actually waiting).
fn describe_wait(wait: &WaitState) -> String {
    match wait {
        WaitState::Running => String::new(),
        WaitState::Timer { .. } => "Timer".to_string(),
        WaitState::Msg { .. } => "Msg".to_string(),
        WaitState::Job { job_key } => format!("Job({})", job_key),
        WaitState::Join { .. } => "Join".to_string(),
        WaitState::Incident { .. } => "Incident".to_string(),
        WaitState::Race {
            race_id, job_key, ..
        } => {
            if let Some(jk) = job_key {
                format!("Race(id={},job={})", race_id, jk)
            } else {
                format!("Race(id={})", race_id)
            }
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ── A8 FFI helpers ────────────────────────────────────────────────────────────

fn bytes_to_hex(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn ffi_incident_class_to_error_class(c: FfiIncidentClass) -> ErrorClass {
    match c {
        FfiIncidentClass::Transient => ErrorClass::Transient,
        FfiIncidentClass::ContractViolation => ErrorClass::ContractViolation,
        FfiIncidentClass::BusinessRejection { rejection_code } => {
            ErrorClass::BusinessRejection { rejection_code }
        }
    }
}

fn literal_to_json(lit: &Literal) -> serde_json::Value {
    match lit {
        Literal::Bool(b) => serde_json::Value::Bool(*b),
        Literal::I64(n) => serde_json::Value::Number((*n).into()),
        Literal::F64(f) => serde_json::json!(*f),
        Literal::String(s) => serde_json::Value::String(s.clone()),
    }
}
