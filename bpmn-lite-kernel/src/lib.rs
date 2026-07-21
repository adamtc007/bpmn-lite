#![forbid(unsafe_code)]

use bpmn_lite_types::ffi_bindings::{apply_ffi_outputs, encode_ffi_inputs};
use bpmn_lite_types::{
    Addr, BufferedMessageMutation, Command, CommandEnvelope, ConcurrencyMutation,
    ConcurrencyRecord, ControlStackDelta, DedupeWrite, DurableEffect, EffectId, EffectMutation,
    EffectOutput, EffectTerminalState, ErrorClass, ExecutableWorkflow, Fiber, Incident, Instr,
    JobActivation, JobMutation, JoinId, JoinMutation, JournalCommand, JournalRecord,
    PersistedSnapshotState, ProcessInstance, ProcessState, RecordCounters, RecordId, RecordKind,
    RecordState, RuntimeEvent, Snapshot, SnapshotEnvelope, TerminalCleanup, TimerKind,
    TimerMutation, TimerRepeatSpec, Transition, TransitionBuilder, Uuid, Value, WaitArm,
    WaitState,
};
use std::fmt;

const PUBLISHED_MESSAGE_TTL_MS: u64 = 300_000;

/// All nondeterministic inputs for one transition, supplied by the command boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeterministicContext {
    logical_time: u64,
    command_id: Uuid,
    next_revision: u64,
}

impl DeterministicContext {
    pub fn new(logical_time: u64, command_id: Uuid, next_revision: u64) -> Self {
        Self {
            logical_time,
            command_id,
            next_revision,
        }
    }

    pub fn logical_time(&self) -> u64 {
        self.logical_time
    }
    pub fn command_id(&self) -> Uuid {
        self.command_id
    }
    pub fn next_revision(&self) -> u64 {
        self.next_revision
    }

    pub fn derived_id(&self, ordinal: u32) -> Uuid {
        EffectId::for_command(self.command_id, self.next_revision, ordinal).as_uuid()
    }

    pub fn effect_id(&self, instance_id: Uuid, ordinal: u32) -> EffectId {
        EffectId::for_transition(instance_id, self.next_revision, ordinal)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransitionError {
    UnsupportedCommand(&'static str),
    InvalidCommand(&'static str),
    MissingFiber(Uuid),
    ProgramCounterOutOfBounds(u32),
    StackUnderflow(&'static str),
    MissingMetadata(&'static str),
    ResourceLimitExceeded {
        resource: &'static str,
        actual: usize,
        limit: u64,
    },
    StepLimitExceeded(u64),
    NumericOverflow(&'static str),
    OptimisticConflict,
    RouteNotMatched(String),
}

impl fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCommand(command) => {
                write!(formatter, "unsupported command: {command}")
            }
            Self::InvalidCommand(reason) => write!(formatter, "invalid command: {reason}"),
            Self::MissingFiber(fiber_id) => write!(formatter, "missing fiber: {fiber_id}"),
            Self::ProgramCounterOutOfBounds(pc) => {
                write!(formatter, "program counter is out of bounds: {pc}")
            }
            Self::StackUnderflow(instruction) => {
                write!(formatter, "stack underflow at {instruction}")
            }
            Self::MissingMetadata(metadata) => {
                write!(formatter, "missing verified metadata: {metadata}")
            }
            Self::ResourceLimitExceeded {
                resource,
                actual,
                limit,
            } => write!(
                formatter,
                "verified {resource} limit exceeded: {actual} > {limit}"
            ),
            Self::StepLimitExceeded(limit) => {
                write!(formatter, "transition step limit exceeded: {limit}")
            }
            Self::NumericOverflow(value) => write!(formatter, "numeric overflow: {value}"),
            Self::OptimisticConflict => write!(formatter, "optimistic payload conflict"),
            Self::RouteNotMatched(node) => {
                write!(formatter, "no deterministic route matched at {node}")
            }
        }
    }
}

impl std::error::Error for TransitionError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    ArtifactMismatch,
    ArtifactAbiMismatch { expected: u32, actual: u32 },
    RevisionChain { expected: i64, actual: i64 },
    NonReplayableCommand(&'static str),
    Transition(TransitionError),
    Envelope(String),
    StateDivergence { revision: u64 },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArtifactMismatch => write!(formatter, "journal artifact does not match workflow"),
            Self::ArtifactAbiMismatch { expected, actual } => write!(
                formatter,
                "snapshot artifact ABI mismatch: expected {expected}, got {actual}"
            ),
            Self::RevisionChain { expected, actual } => write!(
                formatter,
                "journal revision chain mismatch: expected prior {expected}, got {actual}"
            ),
            Self::NonReplayableCommand(kind) => {
                write!(formatter, "journal command is not replayable: {kind}")
            }
            Self::Transition(error) => write!(formatter, "replay transition failed: {error}"),
            Self::Envelope(error) => write!(formatter, "replay envelope failed: {error}"),
            Self::StateDivergence { revision } => {
                write!(formatter, "replay state diverged at revision {revision}")
            }
        }
    }
}

impl std::error::Error for ReplayError {}

/// Replay a journal tail from a versioned genesis snapshot or checkpoint.
pub fn replay(
    workflow: &ExecutableWorkflow,
    checkpoint: &SnapshotEnvelope,
    journal_tail: &[JournalRecord],
) -> Result<SnapshotEnvelope, ReplayError> {
    let workflow_hash = workflow.hash().into_bytes();
    let workflow_abi = workflow.envelope().abi_version();
    if checkpoint.state().instance().bytecode_version != workflow_hash {
        return Err(ReplayError::ArtifactMismatch);
    }
    if checkpoint.artifact_abi() != workflow_abi {
        return Err(ReplayError::ArtifactAbiMismatch {
            expected: workflow_abi,
            actual: checkpoint.artifact_abi(),
        });
    }

    let mut current = checkpoint.clone();
    for record in journal_tail {
        if record.artifact_hash() != workflow_hash {
            return Err(ReplayError::ArtifactMismatch);
        }
        let expected_prior = i64::try_from(current.revision())
            .map_err(|error| ReplayError::Envelope(error.to_string()))?;
        if record.prior_revision() != expected_prior
            || record.new_revision() != current.revision().saturating_add(1)
        {
            return Err(ReplayError::RevisionChain {
                expected: expected_prior,
                actual: record.prior_revision(),
            });
        }
        let JournalCommand::Kernel(command) = record.command().command() else {
            return Err(ReplayError::NonReplayableCommand(
                record.command().command_type(),
            ));
        };
        let logical_time = u64::try_from(record.command().logical_time())
            .map_err(|error| ReplayError::Envelope(error.to_string()))?;
        let context = DeterministicContext::new(
            logical_time,
            record.command().command_id(),
            record.new_revision(),
        );
        let snapshot = current.state().to_runtime_snapshot();
        let transition =
            apply(workflow, &snapshot, command, &context).map_err(ReplayError::Transition)?;
        current = materialize_snapshot(
            current.state(),
            &transition,
            workflow_abi,
            record.new_revision(),
        );
        let replayed_hash = current
            .state_hash()
            .map_err(|error| ReplayError::Envelope(error.to_string()))?;
        if replayed_hash != record.state_hash() {
            return Err(ReplayError::StateDivergence {
                revision: record.new_revision(),
            });
        }
    }
    Ok(current)
}

fn materialize_snapshot(
    prior: &PersistedSnapshotState,
    transition: &Transition,
    artifact_abi: u32,
    revision: u64,
) -> SnapshotEnvelope {
    let mut instance = transition.next_snapshot().clone();
    if let Some(state) = transition.state_override() {
        instance.state = state.clone();
    }
    let mut fibers = prior.fibers().clone();
    for fiber in transition.fibers_upsert() {
        fibers.insert(fiber.fiber_id, fiber.clone());
    }
    for fiber_id in transition.fibers_delete() {
        fibers.remove(fiber_id);
    }
    let mut joins = prior.join_counts().clone();
    for mutation in transition.join_mutations() {
        match mutation {
            JoinMutation::Arrive(join_id) => {
                let count = joins.entry(*join_id).or_insert(0);
                *count = count.saturating_add(1);
            }
            JoinMutation::Reset(join_id) => {
                joins.insert(*join_id, 0);
            }
        }
    }
    let mut incidents = prior.incidents().clone();
    for incident in transition.incidents() {
        incidents.insert(incident.incident_id, incident.clone());
    }
    let mut concurrency_table = prior.concurrency_table().clone();
    for mutation in transition.concurrency_mutations() {
        match mutation {
            ConcurrencyMutation::Insert(record) => concurrency_table.insert(record.clone()),
            ConcurrencyMutation::Retire(id) => {
                if let Some(record) = concurrency_table.get_mut(*id) {
                    record.state = RecordState::Retired;
                }
            }
            ConcurrencyMutation::Remove(id) => {
                concurrency_table.remove(*id);
            }
        }
    }
    if transition.terminal_cleanup().delete_all_fibers() {
        fibers.clear();
    }
    if transition.terminal_cleanup().delete_all_joins() {
        joins.clear();
    }
    let artifact_hash = instance.bytecode_version;
    SnapshotEnvelope::new(
        artifact_abi,
        artifact_hash,
        revision,
        PersistedSnapshotState::new(
            instance,
            fibers.into_values(),
            joins,
            incidents.into_values(),
            concurrency_table,
            prior.pending_effects().iter().copied(),
        ),
    )
}

/// Pure transition function. It performs no I/O and obtains time and identity only
/// from `DeterministicContext` or the durable command.
pub fn apply(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    command: &Command,
    context: &DeterministicContext,
) -> Result<Transition, TransitionError> {
    validate_snapshot_limits(workflow, snapshot)?;
    let transition = match command {
        Command::TimerFired { timer, fired_at } => apply_timer(snapshot, timer, *fired_at),
        Command::Cancel { reason } => {
            let mut next = snapshot.instance().clone();
            next.state = ProcessState::Cancelled {
                reason: reason.clone(),
                at: context.logical_time() as i64,
            };
            let mut builder = TransitionBuilder::new(next);
            for fiber in snapshot.fibers().values() {
                let wait_desc = describe_wait(&fiber.wait);
                if !wait_desc.is_empty() {
                    builder = builder.event(RuntimeEvent::WaitCancelled {
                        fiber_id: fiber.fiber_id,
                        wait_desc,
                        reason: reason.clone(),
                    });
                }
            }
            Ok(builder
                .event(RuntimeEvent::Cancelled {
                    reason: reason.clone(),
                })
                .terminal_cleanup(TerminalCleanup::new(true, true, true))
                .build())
        }
        Command::Tick { .. } => apply_tick(workflow, snapshot, command, context),
        Command::EffectCompleted {
            output: EffectOutput::Job(completion),
            ..
        } => apply_job_completion(workflow, snapshot, completion),
        Command::EffectCompleted {
            output:
                EffectOutput::Ffi {
                    fiber_id: _,
                    pc: _,
                    output_payload: _,
                    new_domain_payload: _,
                    no_match: _,
                },
            ..
        } => apply_ffi_completion(workflow, snapshot, command),
        Command::EffectCompleted {
            output: EffectOutput::Json(_),
            ..
        } => Err(TransitionError::UnsupportedCommand(
            "JSON effect completion requires T8",
        )),
        Command::EffectFailed { .. } => apply_job_failure(workflow, snapshot, command, context),
        Command::MessageDelivered { .. } => apply_message(workflow, snapshot, command, context),
        Command::Terminate => {
            let at = logical_timestamp(context)?;
            let fiber_id = snapshot
                .fibers()
                .keys()
                .next()
                .copied()
                .unwrap_or_else(Uuid::nil);
            let mut next = snapshot.instance().clone();
            next.state = ProcessState::Terminated { at };
            Ok(TransitionBuilder::new(next)
                .event(RuntimeEvent::Terminated { at, fiber_id })
                .terminal_cleanup(TerminalCleanup::new(true, true, true))
                .build())
        }
        Command::ResolveIncident {
            incident_id,
            resolution,
        } => {
            let mut incident = snapshot
                .incident(*incident_id)
                .cloned()
                .ok_or(TransitionError::InvalidCommand("incident does not exist"))?;
            if incident.resolved_at.is_some() {
                return Ok(TransitionBuilder::new(snapshot.instance().clone()).build());
            }
            incident.resolved_at = Some(logical_timestamp(context)?);
            incident.resolution = Some(resolution.clone());
            let mut next = snapshot.instance().clone();
            next.state = ProcessState::Running;
            let mut builder = TransitionBuilder::new(next).incident(incident).event(
                RuntimeEvent::IncidentResolved {
                    incident_id: *incident_id,
                    resolution: resolution.clone(),
                },
            );
            if let Some(mut fiber) = snapshot.fibers().values()
                .find(|fiber| matches!(fiber.wait, WaitState::Incident { incident_id: id } if id == *incident_id))
                .cloned()
            {
                fiber.wait = WaitState::Running;
                builder = builder.upsert_fiber(fiber);
            }
            Ok(builder.build())
        }
        Command::StartChildResult {
            idempotency_key,
            child_instance_id,
            accepted,
        } => apply_start_child_result(
            snapshot,
            idempotency_key,
            *child_instance_id,
            *accepted,
            context,
        ),
        Command::JobClaimed {
            job_key,
            worker_id,
            claim_expires_at,
        } => Ok(TransitionBuilder::new(snapshot.instance().clone())
            .event(RuntimeEvent::JobClaimed {
                job_key: job_key.clone(),
                worker_id: worker_id.clone(),
                claim_expires_at: *claim_expires_at,
            })
            .build()),
        Command::V2TriggerGuard { .. } => apply_v2_trigger_guard(snapshot, command, context),
    }?;
    let logical_time = i64::try_from(context.logical_time())
        .map_err(|_| TransitionError::NumericOverflow("logical time"))?;
    Ok(transition.with_command_envelope(CommandEnvelope::new(
        context.command_id(),
        logical_time,
        JournalCommand::Kernel(command.clone()),
    )))
}

fn validate_snapshot_limits(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
) -> Result<(), TransitionError> {
    let limits = workflow.envelope().limits();
    if snapshot.fibers().len() > limits.max_fibers() as usize {
        return Err(TransitionError::ResourceLimitExceeded {
            resource: "fiber count",
            actual: snapshot.fibers().len(),
            limit: u64::from(limits.max_fibers()),
        });
    }
    for fiber in snapshot.fibers().values() {
        if fiber.stack.len() > limits.max_stack() as usize {
            return Err(TransitionError::ResourceLimitExceeded {
                resource: "operand stack",
                actual: fiber.stack.len(),
                limit: u64::from(limits.max_stack()),
            });
        }
        if fiber.regs.len() > limits.max_registers() as usize {
            return Err(TransitionError::ResourceLimitExceeded {
                resource: "register count",
                actual: fiber.regs.len(),
                limit: u64::from(limits.max_registers()),
            });
        }
    }
    Ok(())
}

fn apply_start_child_result(
    snapshot: &Snapshot,
    idempotency_key: &str,
    child_instance_id: Uuid,
    accepted: bool,
    context: &DeterministicContext,
) -> Result<Transition, TransitionError> {
    if matches!(
        snapshot.instance().state,
        ProcessState::WaitingOnInvocation { execution_id, .. } if execution_id == child_instance_id
    ) {
        return Ok(TransitionBuilder::new(snapshot.instance().clone()).build());
    }

    let ProcessState::WaitingOnSubmission { node_id, .. } = &snapshot.instance().state else {
        return Err(TransitionError::InvalidCommand(
            "child-start result requires WaitingOnSubmission",
        ));
    };

    if accepted {
        let mut next = snapshot.instance().clone();
        next.state = ProcessState::WaitingOnInvocation {
            execution_id: child_instance_id,
            node_id: node_id.clone(),
        };
        return Ok(TransitionBuilder::new(next)
            .event(RuntimeEvent::ChildStartAccepted {
                idempotency_key: idempotency_key.to_string(),
                child_instance_id,
            })
            .build());
    }

    let mut fiber =
        snapshot
            .fibers()
            .values()
            .next()
            .cloned()
            .ok_or(TransitionError::InvalidCommand(
                "child-start rejection requires an active fiber",
            ))?;
    let incident_id = context.derived_id(0);
    let incident = Incident {
        incident_id,
        process_instance_id: snapshot.instance().instance_id,
        fiber_id: fiber.fiber_id,
        service_task_id: node_id.clone(),
        bytecode_addr: fiber.pc,
        error_class: ErrorClass::ContractViolation,
        message: format!("child start rejected: {idempotency_key}"),
        retry_count: 0,
        created_at: logical_timestamp(context)?,
        resolved_at: None,
        resolution: None,
    };
    fiber.wait = WaitState::Incident { incident_id };
    let mut next = snapshot.instance().clone();
    next.state = ProcessState::Failed { incident_id };
    Ok(TransitionBuilder::new(next)
        .upsert_fiber(fiber)
        .incident(incident)
        .event(RuntimeEvent::ChildStartRejected {
            idempotency_key: idempotency_key.to_string(),
            child_instance_id,
            incident_id,
        })
        .event(RuntimeEvent::IncidentCreated {
            incident_id,
            service_task_id: node_id.clone(),
            job_key: None,
        })
        .build())
}

fn apply_tick(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    command: &Command,
    context: &DeterministicContext,
) -> Result<Transition, TransitionError> {
    let Command::Tick { fiber_id } = command else {
        return Err(TransitionError::InvalidCommand("apply_tick requires Tick"));
    };
    if snapshot.instance().state != ProcessState::Running {
        return Err(TransitionError::InvalidCommand(
            "Tick requires a running instance",
        ));
    }
    let selected = match fiber_id {
        Some(id) => *id,
        None => snapshot
            .fibers()
            .values()
            .filter(|fiber| fiber.wait == WaitState::Running)
            .map(|fiber| fiber.fiber_id)
            .next()
            .ok_or(TransitionError::InvalidCommand("no running fiber"))?,
    };
    let mut fiber = snapshot
        .fiber(selected)
        .cloned()
        .ok_or(TransitionError::MissingFiber(selected))?;
    if fiber.wait != WaitState::Running {
        return Err(TransitionError::InvalidCommand(
            "selected fiber is not running",
        ));
    }

    let envelope = workflow.envelope();
    let instructions = envelope.instructions();
    let metadata = envelope.metadata();
    let mut instance = snapshot.instance().clone();
    let mut changes = Changes::default();
    let mut ordinal = 0u32;
    // V4.1: accumulates `V2ArmTimer`/`V2ArmMsg` descriptors between a
    // `V2RaceOpen` and its `V2RaceClose`, both of which run within the
    // same tick (only `V2RaceClose` parks) — mirrors the `ordinal`
    // accumulator's own within-tick-only scope, no persisted state needed.
    let mut race_arms: Vec<bpmn_lite_types::V2RaceArm> = Vec::new();
    let step_limit = envelope.limits().max_steps();

    for _ in 0..step_limit {
        let instruction = instructions
            .get(fiber.pc.index())
            .ok_or(TransitionError::ProgramCounterOutOfBounds(fiber.pc.into()))?;
        match instruction {
            Instr::Jump { target } => fiber.pc = *target,
            Instr::BrIf { target } | Instr::BrIfNot { target } => {
                let value = fiber
                    .stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("branch"))?;
                let take = is_truthy(&value) == matches!(instruction, Instr::BrIf { .. });
                fiber.pc = if take {
                    *target
                } else {
                    fiber.pc.saturating_add(1)
                };
            }
            Instr::PushBool(value) => {
                fiber.stack.push(Value::Bool(*value));
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::PushI64(value) => {
                fiber.stack.push(Value::I64(*value));
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::Pop => {
                fiber
                    .stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("Pop"))?;
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::LoadFlag { key } => {
                fiber.stack.push(
                    instance
                        .flags
                        .get(key)
                        .cloned()
                        .unwrap_or(Value::Bool(false)),
                );
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::StoreFlag { key } => {
                let value = fiber
                    .stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("StoreFlag"))?;
                instance.flags.insert(*key, value.clone());
                changes
                    .events
                    .push(RuntimeEvent::FlagSet { key: *key, value });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::ExecNative {
                task_type, retc, ..
            } => {
                let instruction_pc = fiber.pc;
                let task_type = metadata
                    .task_manifest()
                    .get(*task_type as usize)
                    .cloned()
                    .ok_or(TransitionError::MissingMetadata("task manifest"))?;
                let service_task_id = metadata
                    .debug_map()
                    .get(&instruction_pc)
                    .cloned()
                    .unwrap_or_else(|| format!("pc_{instruction_pc}"));
                let job_key = format!(
                    "{}:{}:{}:{}",
                    instance.instance_id, service_task_id, instruction_pc, fiber.loop_epoch
                );
                if let Some(completion) = snapshot.dedupe_completion(&job_key) {
                    apply_completion(&mut instance, completion);
                    for _ in 0..*retc {
                        fiber.stack.push(Value::Bool(true));
                    }
                    fiber.pc = fiber.pc.saturating_add(1);
                    continue;
                }
                let orch_flags = instance
                    .flags
                    .iter()
                    .map(|(key, value)| (format!("flag_{key}"), value.clone()))
                    .collect();
                changes.jobs_enqueue.push(JobActivation {
                    job_key: job_key.clone(),
                    tenant_id: instance.tenant_id.clone(),
                    process_instance_id: instance.instance_id,
                    task_type: task_type.clone(),
                    service_task_id: service_task_id.clone(),
                    domain_payload: instance.domain_payload.to_string(),
                    domain_payload_hash: instance.domain_payload_hash,
                    session_stack: instance.session_stack.clone(),
                    orch_flags,
                    retries_remaining: 3,
                    entry_id: instance.entry_id,
                    runbook_id: instance.runbook_id,
                    worker_id: String::new(),
                    claim_token: String::new(),
                    claim_expires_at: None,
                    attempt_count: 0,
                    failure_count: 0,
                    not_before: None,
                });
                changes.events.push(RuntimeEvent::JobActivated {
                    job_key: job_key.clone(),
                    task_type,
                    service_task_id,
                    pc: instruction_pc,
                });
                if let Some(race_id) = metadata.boundary_map().get(&instruction_pc).copied() {
                    let race = metadata
                        .race_plan()
                        .get(&race_id)
                        .ok_or(TransitionError::MissingMetadata("boundary race plan"))?;
                    let arm_index = race.arms.iter().position(|arm| {
                        matches!(arm, WaitArm::Timer { .. } | WaitArm::Deadline { .. })
                    });
                    let due_at = arm_index.and_then(|index| match &race.arms[index] {
                        WaitArm::Timer { duration_ms, .. } => {
                            context.logical_time().checked_add(*duration_ms)
                        }
                        WaitArm::Deadline { deadline_ms, .. } => Some(*deadline_ms),
                        _ => None,
                    });
                    let interrupting = arm_index
                        .and_then(|index| match &race.arms[index] {
                            WaitArm::Timer { interrupting, .. } => Some(*interrupting),
                            WaitArm::Deadline { .. } => Some(true),
                            _ => None,
                        })
                        .unwrap_or(true);
                    fiber.wait = WaitState::Race {
                        race_id,
                        timer_deadline_ms: due_at,
                        job_key: Some(job_key.clone()),
                        interrupting,
                        timer_arm_index: arm_index,
                        cycle_remaining: arm_index.and_then(|index| match &race.arms[index] {
                            WaitArm::Timer { cycle, .. } => {
                                cycle.as_ref().map(|cycle| cycle.max_fires)
                            }
                            _ => None,
                        }),
                        cycle_fired_count: 0,
                    };
                    changes.events.push(RuntimeEvent::RaceRegistered {
                        race_id,
                        fiber_id: fiber.fiber_id,
                        arms: race.arms.iter().map(Into::into).collect(),
                    });
                    if let (Some(due_at), Some(arm_index)) = (due_at, arm_index) {
                        let (resume_at, repeat_spec) = timer_arm(&race.arms[arm_index])?;
                        changes.effects.push(DurableEffect::schedule_timer(
                            EffectId::for_instruction(
                                instance.instance_id,
                                fiber.fiber_id,
                                instruction_pc.into(),
                            ),
                            fiber.fiber_id,
                            due_at,
                            TimerKind::Race {
                                race_id,
                                arm_index,
                                resume_at: resume_at.into(),
                                interrupting,
                                job_key: Some(job_key),
                                boundary_element_id: race.boundary_element_id.clone(),
                                arm_count: race.arms.len(),
                            },
                            repeat_spec,
                        ));
                    }
                } else {
                    fiber.wait = WaitState::Job { job_key };
                }
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::ExecDslTask {
                task_type,
                static_args: _,
                produces_placeholder,
            } => {
                let instruction_pc = fiber.pc;
                let task_type = metadata
                    .task_manifest()
                    .get(*task_type as usize)
                    .cloned()
                    .ok_or(TransitionError::MissingMetadata("task manifest"))?;
                let service_task_id = metadata
                    .debug_map()
                    .get(&instruction_pc)
                    .cloned()
                    .unwrap_or_else(|| format!("pc_{instruction_pc}"));
                let job_key = format!(
                    "{}:{}:{}:{}",
                    instance.instance_id, service_task_id, instruction_pc, fiber.loop_epoch
                );
                if let Some(completion) = snapshot.dedupe_completion(&job_key) {
                    apply_completion(&mut instance, completion);
                    if let Some(placeholder) = produces_placeholder {
                        instance
                            .bind_placeholder_from_payload(placeholder)
                            .map_err(TransitionError::InvalidCommand)?;
                    }
                    fiber.pc = fiber.pc.saturating_add(1);
                    continue;
                }
                let orch_flags = instance
                    .flags
                    .iter()
                    .map(|(key, value)| (format!("flag_{key}"), value.clone()))
                    .collect();
                changes.jobs_enqueue.push(JobActivation {
                    job_key: job_key.clone(),
                    tenant_id: instance.tenant_id.clone(),
                    process_instance_id: instance.instance_id,
                    task_type: task_type.clone(),
                    service_task_id: service_task_id.clone(),
                    domain_payload: instance.domain_payload.to_string(),
                    domain_payload_hash: instance.domain_payload_hash,
                    session_stack: instance.session_stack.clone(),
                    orch_flags,
                    retries_remaining: 3,
                    entry_id: instance.entry_id,
                    runbook_id: instance.runbook_id,
                    worker_id: String::new(),
                    claim_token: String::new(),
                    claim_expires_at: None,
                    attempt_count: 0,
                    failure_count: 0,
                    not_before: None,
                });
                changes.events.push(RuntimeEvent::JobActivated {
                    job_key: job_key.clone(),
                    task_type,
                    service_task_id,
                    pc: instruction_pc,
                });
                fiber.wait = WaitState::Job { job_key };
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::RoutePayload {
                branches,
                default_target,
            } => {
                let target = branches.iter().find_map(|branch| {
                    instance
                        .placeholder_matches(&branch.placeholder, &branch.expected_value)
                        .then_some(branch.target)
                });
                fiber.pc = target.or(*default_target).ok_or_else(|| {
                    TransitionError::RouteNotMatched(
                        metadata
                            .debug_map()
                            .get(&fiber.pc)
                            .cloned()
                            .unwrap_or_else(|| format!("pc_{}", fiber.pc)),
                    )
                })?;
            }
            Instr::ForkPayload {
                branches,
                join_id,
                default_target,
            } => {
                let mut targets: Vec<_> = branches
                    .iter()
                    .filter(|branch| {
                        instance.placeholder_matches(&branch.placeholder, &branch.expected_value)
                    })
                    .map(|branch| branch.target)
                    .collect();
                if targets.is_empty() {
                    if let Some(target) = default_target {
                        targets.push(*target);
                    } else {
                        return Err(TransitionError::RouteNotMatched(
                            metadata
                                .debug_map()
                                .get(&fiber.pc)
                                .cloned()
                                .unwrap_or_else(|| format!("pc_{}", fiber.pc)),
                        ));
                    }
                }
                instance
                    .join_expected
                    .insert(*join_id, targets.len() as u16);
                for target in &targets {
                    let child_id = context.derived_id(ordinal);
                    ordinal = ordinal.saturating_add(1);
                    changes.fibers_upsert.push(Fiber::new(child_id, *target));
                    changes.events.push(RuntimeEvent::FiberSpawned {
                        fiber_id: child_id,
                        pc: *target,
                        parent: Some(fiber.fiber_id),
                    });
                }
                changes.events.push(RuntimeEvent::InclusiveForkTaken {
                    gateway_id: metadata
                        .debug_map()
                        .get(&fiber.pc)
                        .cloned()
                        .unwrap_or_else(|| format!("pc_{}", fiber.pc)),
                    branches_taken: targets.clone(),
                    join_id: *join_id,
                    expected: targets.len() as u16,
                });
                changes.fibers_delete.push(fiber.fiber_id);
                return Ok(changes.finish(instance));
            }
            Instr::ExecFfi { template_id, .. } => {
                let pc = fiber.pc;
                let declaration = metadata
                    .ffi_task_decls()
                    .get(&pc)
                    .ok_or(TransitionError::MissingMetadata("FFI task declaration"))?;
                if declaration.template_id != *template_id {
                    return Err(TransitionError::MissingMetadata("FFI template identity"));
                }
                let input = encode_ffi_inputs(&instance, declaration)
                    .map_err(|_| TransitionError::InvalidCommand("FFI input contract violation"))?;
                let effect_id = EffectId::for_instruction(instance.instance_id, fiber.fiber_id, pc.into());
                let operation = template_id
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                changes.effects.push(DurableEffect::Invoke {
                    effect_id,
                    fiber_id: fiber.fiber_id,
                    pc: pc.into(),
                    operation,
                    template_id: *template_id,
                    input,
                    idempotency_key: effect_id.as_uuid().to_string(),
                });
                changes.events.push(RuntimeEvent::FfiInvocationPending {
                    invocation_id: effect_id.as_uuid(),
                    template_id_hex: template_id
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect(),
                    caller_task_id: metadata
                        .debug_map()
                        .get(&pc)
                        .cloned()
                        .unwrap_or_else(|| format!("pc_{pc}")),
                    caller_pc: pc,
                    owner_type: String::new(),
                });
                fiber.wait = WaitState::Effect { effect_id };
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::V2AwaitEffect { template_id, .. } => {
                // `AWAIT-EFFECT` — shares `WaitState::Effect`/`EffectOutput::Ffi`
                // completion machinery with v1 `ExecFfi` (V&S §5: same
                // `DurableEffect::Invoke` + park-on-completion shape); the
                // v2/v1 split lives only in which binding table
                // (`v2_ffi_task_decls` vs `ffi_task_decls`) supplies the
                // declaration — see `apply_ffi_completion`.
                let pc = fiber.pc;
                let declaration = metadata
                    .v2_ffi_task_decls()
                    .get(&pc)
                    .ok_or(TransitionError::MissingMetadata("v2 FFI effect declaration"))?;
                if declaration.template_id != *template_id {
                    return Err(TransitionError::MissingMetadata("v2 FFI effect template identity"));
                }
                let input = encode_ffi_inputs(&instance, declaration)
                    .map_err(|_| TransitionError::InvalidCommand("FFI input contract violation"))?;
                let effect_id = EffectId::for_instruction(instance.instance_id, fiber.fiber_id, pc.into());
                let operation = template_id
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                changes.effects.push(DurableEffect::Invoke {
                    effect_id,
                    fiber_id: fiber.fiber_id,
                    pc: pc.into(),
                    operation,
                    template_id: *template_id,
                    input,
                    idempotency_key: effect_id.as_uuid().to_string(),
                });
                changes.events.push(RuntimeEvent::FfiInvocationPending {
                    invocation_id: effect_id.as_uuid(),
                    template_id_hex: template_id
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect(),
                    caller_task_id: metadata
                        .debug_map()
                        .get(&pc)
                        .cloned()
                        .unwrap_or_else(|| format!("pc_{pc}")),
                    caller_pc: pc,
                    owner_type: String::new(),
                });
                fiber.wait = WaitState::Effect { effect_id };
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::Fork { targets } => {
                let mut ids = Vec::with_capacity(targets.len());
                for target in targets.iter().copied() {
                    let child_id = context.derived_id(ordinal);
                    ordinal = ordinal.saturating_add(1);
                    changes.fibers_upsert.push(Fiber::new(child_id, target));
                    changes.events.push(RuntimeEvent::FiberSpawned {
                        fiber_id: child_id,
                        pc: target,
                        parent: Some(fiber.fiber_id),
                    });
                    ids.push(child_id);
                }
                changes.events.push(RuntimeEvent::Forked {
                    fork_id: format!("pc_{}", fiber.pc),
                    child_fibers: ids,
                    targets: targets.to_vec(),
                });
                changes.fibers_delete.push(fiber.fiber_id);
                return Ok(changes.finish(instance));
            }
            Instr::Join { id, expected, next } => {
                if join_arrive(snapshot, &changes, *id) >= *expected {
                    changes.join_mutations.push(JoinMutation::Reset(*id));
                    changes.events.push(RuntimeEvent::JoinReleased {
                        join_id: *id,
                        next_pc: *next,
                        released_fiber_id: fiber.fiber_id,
                    });
                    fiber.pc = *next;
                } else {
                    changes.join_mutations.push(JoinMutation::Arrive(*id));
                    changes.events.push(RuntimeEvent::JoinArrived {
                        join_id: *id,
                        fiber_id: fiber.fiber_id,
                    });
                    changes.fibers_delete.push(fiber.fiber_id);
                    return Ok(changes.finish(instance));
                }
            }
            Instr::WaitFor { ms } => {
                let pc = fiber.pc;
                let deadline = context
                    .logical_time()
                    .checked_add(*ms)
                    .ok_or(TransitionError::NumericOverflow("WaitFor deadline"))?;
                fiber.pc = fiber.pc.saturating_add(1);
                fiber.wait = WaitState::Timer {
                    deadline_ms: deadline,
                };
                changes.events.push(RuntimeEvent::WaitTimerSet {
                    fiber_id: fiber.fiber_id,
                    deadline_ms: deadline,
                });
                changes.effects.push(DurableEffect::schedule_timer(
                    EffectId::for_instruction(instance.instance_id, fiber.fiber_id, pc.into()),
                    fiber.fiber_id,
                    deadline,
                    TimerKind::Wait,
                    None,
                ));
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::WaitUntil { deadline_ms } => {
                let pc = fiber.pc;
                fiber.pc = fiber.pc.saturating_add(1);
                fiber.wait = WaitState::Timer {
                    deadline_ms: *deadline_ms,
                };
                changes.events.push(RuntimeEvent::WaitTimerSet {
                    fiber_id: fiber.fiber_id,
                    deadline_ms: *deadline_ms,
                });
                changes.effects.push(DurableEffect::schedule_timer(
                    EffectId::for_instruction(instance.instance_id, fiber.fiber_id, pc.into()),
                    fiber.fiber_id,
                    *deadline_ms,
                    TimerKind::Wait,
                    None,
                ));
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::WaitMsg {
                wait_id,
                name,
                corr_reg,
            } => {
                let corr_key = fiber
                    .regs
                    .get(*corr_reg as usize)
                    .cloned()
                    .unwrap_or(Value::Bool(false));
                if let Some(buffered) = snapshot.buffered_messages().first() {
                    if let Some(hash) = buffered.message.payload_hash {
                        let payload =
                            std::str::from_utf8(&buffered.message.payload).map_err(|_| {
                                TransitionError::InvalidCommand(
                                    "buffered message payload is not UTF-8",
                                )
                            })?;
                        instance.domain_payload = payload.to_string().into();
                        instance.domain_payload_hash = hash;
                    }
                    fiber.pc = fiber.pc.saturating_add(1);
                    changes
                        .buffered_messages
                        .push(BufferedMessageMutation::Consume(buffered.clone()));
                    changes.events.push(RuntimeEvent::BufferedMessageConsumed {
                        message_name: buffered.message.message_name.clone(),
                        correlation_key: buffered.message.correlation_key.clone(),
                        msg_id: buffered.message.msg_id.clone(),
                        fiber_id: fiber.fiber_id,
                    });
                    changes.events.push(RuntimeEvent::MsgReceived {
                        name: *name,
                        corr_key,
                        msg_ref: None,
                    });
                    continue;
                }
                fiber.pc = fiber.pc.saturating_add(1);
                fiber.wait = WaitState::Msg {
                    wait_id: *wait_id,
                    name: *name,
                    corr_key: corr_key.clone(),
                };
                changes.events.push(RuntimeEvent::WaitMsgSubscribed {
                    fiber_id: fiber.fiber_id,
                    name: *name,
                    corr_key,
                });
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::PublishMessage { name, corr_reg } => {
                let corr_key = fiber
                    .regs
                    .get(*corr_reg as usize)
                    .cloned()
                    .unwrap_or(Value::Bool(false));
                let message_name = workflow
                    .envelope()
                    .metadata()
                    .message_name_map()
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| name.to_string());
                let correlation_key = value_key(&corr_key);
                let msg_id = format!(
                    "publish:{}:{}:{}",
                    instance.instance_id, fiber.fiber_id, fiber.pc
                );
                let expires_at = context
                    .logical_time()
                    .checked_add(PUBLISHED_MESSAGE_TTL_MS)
                    .ok_or(TransitionError::NumericOverflow("published message expiry"))?;
                let expires_at = i64::try_from(expires_at)
                    .map_err(|_| TransitionError::NumericOverflow("published message expiry"))?;
                changes
                    .buffered_messages
                    .push(BufferedMessageMutation::Insert(
                        bpmn_lite_types::BufferedMessage {
                            tenant_id: instance.tenant_id.clone(),
                            message_name: message_name.clone(),
                            correlation_key: correlation_key.clone(),
                            msg_id: msg_id.clone(),
                            payload: Vec::new(),
                            payload_hash: None,
                            process_instance_id: Some(instance.instance_id),
                            received_at: logical_timestamp(context)?,
                            expires_at,
                        },
                    ));
                changes.events.push(RuntimeEvent::MessageBuffered {
                    message_name,
                    correlation_key,
                    msg_id,
                    expires_at,
                });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::WaitAny { race_id, arms } => {
                changes.events.push(RuntimeEvent::RaceRegistered {
                    race_id: *race_id,
                    fiber_id: fiber.fiber_id,
                    arms: arms.iter().map(Into::into).collect(),
                });
                let mut first_deadline = None;
                for (arm_index, arm) in arms.iter().enumerate() {
                    if let WaitArm::Msg { name, corr_reg, .. } = arm {
                        let corr_key = fiber
                            .regs
                            .get(*corr_reg as usize)
                            .cloned()
                            .unwrap_or(Value::Bool(false));
                        changes.events.push(RuntimeEvent::WaitMsgSubscribed {
                            fiber_id: fiber.fiber_id,
                            name: *name,
                            corr_key,
                        });
                    }
                    let due_at = match arm {
                        WaitArm::Timer { duration_ms, .. } => context
                            .logical_time()
                            .checked_add(*duration_ms)
                            .ok_or(TransitionError::NumericOverflow("race timer deadline"))?,
                        WaitArm::Deadline { deadline_ms, .. } => *deadline_ms,
                        _ => continue,
                    };
                    if first_deadline.is_none() {
                        first_deadline = Some(due_at);
                    }
                    let (resume_at, repeat_spec) = timer_arm(arm)?;
                    let interrupting = !matches!(
                        arm,
                        WaitArm::Timer {
                            interrupting: false,
                            ..
                        }
                    );
                    changes.effects.push(DurableEffect::schedule_timer(
                        EffectId::for_instruction_ordinal(
                            instance.instance_id,
                            fiber.fiber_id,
                            fiber.pc.into(),
                            arm_index as u32,
                        ),
                        fiber.fiber_id,
                        due_at,
                        TimerKind::Race {
                            race_id: *race_id,
                            arm_index,
                            resume_at: resume_at.into(),
                            interrupting,
                            job_key: None,
                            boundary_element_id: None,
                            arm_count: arms.len(),
                        },
                        repeat_spec,
                    ));
                }
                fiber.wait = WaitState::Race {
                    race_id: *race_id,
                    timer_deadline_ms: first_deadline,
                    job_key: None,
                    interrupting: true,
                    timer_arm_index: None,
                    cycle_remaining: None,
                    cycle_fired_count: 0,
                };
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::CancelWait { .. } => fiber.pc = fiber.pc.saturating_add(1),
            Instr::IncCounter { counter_id } => {
                let count = instance.counters.entry(*counter_id).or_insert(0);
                *count = count.saturating_add(1);
                fiber.loop_epoch = fiber.loop_epoch.saturating_add(1);
                changes.events.push(RuntimeEvent::CounterIncremented {
                    counter_id: *counter_id,
                    new_value: *count,
                    loop_epoch: fiber.loop_epoch,
                });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::BrCounterLt {
                counter_id,
                limit,
                target,
            } => {
                fiber.pc = if instance.counters.get(counter_id).copied().unwrap_or(0) < *limit {
                    *target
                } else {
                    fiber.pc.saturating_add(1)
                };
            }
            Instr::ForkInclusive {
                branches,
                join_id,
                default_target,
            } => {
                let mut targets: Vec<_> = branches
                    .iter()
                    .filter(|branch| {
                        branch
                            .condition_flag
                            .map(|key| {
                                is_truthy(instance.flags.get(&key).unwrap_or(&Value::Bool(false)))
                            })
                            .unwrap_or(true)
                    })
                    .map(|branch| branch.target)
                    .collect();
                if targets.is_empty() {
                    if let Some(target) = default_target {
                        targets.push(*target);
                    } else {
                        return fail_contract(
                            instance,
                            fiber,
                            changes,
                            context,
                            "inclusive gateway has no matching or default branch",
                        );
                    }
                }
                instance
                    .join_expected
                    .insert(*join_id, targets.len() as u16);
                for target in &targets {
                    let child_id = context.derived_id(ordinal);
                    ordinal = ordinal.saturating_add(1);
                    changes.fibers_upsert.push(Fiber::new(child_id, *target));
                    changes.events.push(RuntimeEvent::FiberSpawned {
                        fiber_id: child_id,
                        pc: *target,
                        parent: Some(fiber.fiber_id),
                    });
                }
                changes.events.push(RuntimeEvent::InclusiveForkTaken {
                    gateway_id: format!("pc_{}", fiber.pc),
                    branches_taken: targets.clone(),
                    join_id: *join_id,
                    expected: targets.len() as u16,
                });
                changes.fibers_delete.push(fiber.fiber_id);
                return Ok(changes.finish(instance));
            }
            Instr::JoinDynamic { id, next } => {
                let expected = instance.join_expected.get(id).copied().ok_or(
                    TransitionError::MissingMetadata("dynamic join expected count"),
                )?;
                if join_arrive(snapshot, &changes, *id) >= expected {
                    changes.join_mutations.push(JoinMutation::Reset(*id));
                    instance.join_expected.remove(id);
                    changes.events.push(RuntimeEvent::JoinReleased {
                        join_id: *id,
                        next_pc: *next,
                        released_fiber_id: fiber.fiber_id,
                    });
                    fiber.pc = *next;
                } else {
                    changes.join_mutations.push(JoinMutation::Arrive(*id));
                    changes.events.push(RuntimeEvent::JoinArrived {
                        join_id: *id,
                        fiber_id: fiber.fiber_id,
                    });
                    changes.fibers_delete.push(fiber.fiber_id);
                    return Ok(changes.finish(instance));
                }
            }
            Instr::End => {
                changes.fibers_delete.push(fiber.fiber_id);
                if snapshot.fibers().len() == 1 {
                    let at = logical_timestamp(context)?;
                    instance.state = ProcessState::Completed { at };
                    changes.events.push(RuntimeEvent::Completed { at });
                    changes.cleanup = Some(TerminalCleanup::new(true, true, true));
                }
                return Ok(changes.finish(instance));
            }
            Instr::EndTerminate => {
                let at = logical_timestamp(context)?;
                instance.state = ProcessState::Terminated { at };
                changes.events.push(RuntimeEvent::Terminated {
                    at,
                    fiber_id: fiber.fiber_id,
                });
                changes.cleanup = Some(TerminalCleanup::new(true, true, true));
                return Ok(changes.finish(instance));
            }
            Instr::Fail { code } => {
                return fail_contract(
                    instance,
                    fiber,
                    changes,
                    context,
                    &format!("process failed with code {code}"),
                );
            }
            // V4.1: core scope words (V&S §5, plan Tranche V4 4.1). Guard
            // open/close and the fork/barrier/join pair. Remaining v2 words
            // (race, wait, cancel) stay in the not-yet-interpretable arm
            // below until their own V4.1 sub-steps land.
            Instr::V2Guard { handler } => {
                let record_id = RecordId::new(context.derived_id(ordinal));
                ordinal = ordinal.saturating_add(1);
                let record = ConcurrencyRecord {
                    handler: Some(*handler),
                    // V4.1, Adam-ratified: every guard scope's opening
                    // word captures a rollback snapshot as standard
                    // lifecycle behaviour — not opt-in — so `V2CancelScope`
                    // can restore it later. See `ConcurrencyRecord`'s doc
                    // comment for the full rationale.
                    rollback_domain_payload: Some(instance.domain_payload.to_string().into_boxed_str()),
                    rollback_domain_payload_hash: Some(instance.domain_payload_hash),
                    ..ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: true })
                };
                changes
                    .concurrency_mutations
                    .push(ConcurrencyMutation::Insert(record));
                fiber.control_stack.push(record_id);
                changes.control_stack_deltas.push(ControlStackDelta::Push {
                    fiber_id: fiber.fiber_id,
                    handle: record_id,
                });
                changes.events.push(RuntimeEvent::V2GuardOpened {
                    record_id,
                    fiber_id: fiber.fiber_id,
                    handler: *handler,
                });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::V2GuardEnd => {
                let handle = fiber
                    .control_stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2GuardEnd"))?;
                changes.control_stack_deltas.push(ControlStackDelta::Pop {
                    fiber_id: fiber.fiber_id,
                    handle,
                });
                changes
                    .concurrency_mutations
                    .push(ConcurrencyMutation::Retire(handle));
                changes.events.push(RuntimeEvent::V2GuardRetired {
                    record_id: handle,
                    fiber_id: fiber.fiber_id,
                });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::V2Fork { targets, pairing: _ } => {
                let record_id = RecordId::new(context.derived_id(ordinal));
                ordinal = ordinal.saturating_add(1);
                let mut record = ConcurrencyRecord::new(record_id, RecordKind::Barrier);
                record.counters = RecordCounters {
                    arity: targets.len() as u32,
                    count: targets.len() as u32,
                };
                let mut ids = Vec::with_capacity(targets.len());
                for target in targets.iter().copied() {
                    let child_id = context.derived_id(ordinal);
                    ordinal = ordinal.saturating_add(1);
                    let mut child = Fiber::new(child_id, target);
                    child.control_stack = fiber.control_stack.clone();
                    child.control_stack.push(record_id);
                    record.members.insert(child_id);
                    changes.control_stack_deltas.push(ControlStackDelta::Push {
                        fiber_id: child_id,
                        handle: record_id,
                    });
                    changes.events.push(RuntimeEvent::FiberSpawned {
                        fiber_id: child_id,
                        pc: target,
                        parent: Some(fiber.fiber_id),
                    });
                    changes.fibers_upsert.push(child);
                    ids.push(child_id);
                }
                changes.events.push(RuntimeEvent::V2Forked {
                    record_id,
                    fork_fiber_id: fiber.fiber_id,
                    child_fibers: ids,
                    targets: targets.to_vec(),
                });
                changes
                    .concurrency_mutations
                    .push(ConcurrencyMutation::Insert(record));
                changes.fibers_delete.push(fiber.fiber_id);
                return Ok(changes.finish(instance));
            }
            Instr::V2Join { pairing: _ } => {
                let handle = fiber
                    .control_stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2Join"))?;
                changes.control_stack_deltas.push(ControlStackDelta::Pop {
                    fiber_id: fiber.fiber_id,
                    handle,
                });
                let mut record = snapshot
                    .concurrency_table()
                    .get(handle)
                    .cloned()
                    .ok_or(TransitionError::InvalidCommand(
                        "V2Join: unknown barrier handle",
                    ))?;
                record.counters.count = record.counters.count.saturating_sub(1);
                if record.counters.count == 0 {
                    // Last arrival (V&S v0.4 §5/§12 ruling B): sole survivor,
                    // continues in place; every other member is cancelled
                    // now, at the moment the barrier retires — not before.
                    let cancelled: Vec<Uuid> = record
                        .members
                        .iter()
                        .copied()
                        .filter(|member| *member != fiber.fiber_id)
                        .collect();
                    for member in &cancelled {
                        changes.fibers_delete.push(*member);
                    }
                    changes
                        .concurrency_mutations
                        .push(ConcurrencyMutation::Retire(handle));
                    changes.events.push(RuntimeEvent::V2JoinReleased {
                        record_id: handle,
                        survivor_fiber_id: fiber.fiber_id,
                        next_pc: fiber.pc.saturating_add(1),
                        cancelled_fibers: cancelled,
                    });
                    fiber.pc = fiber.pc.saturating_add(1);
                } else {
                    changes
                        .concurrency_mutations
                        .push(ConcurrencyMutation::Insert(record));
                    changes.events.push(RuntimeEvent::V2JoinArrived {
                        record_id: handle,
                        fiber_id: fiber.fiber_id,
                    });
                    fiber.wait = WaitState::V2Barrier { record_id: handle };
                    changes.fibers_upsert.push(fiber);
                    return Ok(changes.finish(instance));
                }
            }
            Instr::V2RaceOpen { arm_count } => {
                let record_id = RecordId::new(context.derived_id(ordinal));
                ordinal = ordinal.saturating_add(1);
                let mut record = ConcurrencyRecord::new(record_id, RecordKind::Race);
                record.members.insert(fiber.fiber_id);
                record.counters = RecordCounters {
                    arity: u32::from(*arm_count),
                    count: u32::from(*arm_count),
                };
                changes
                    .concurrency_mutations
                    .push(ConcurrencyMutation::Insert(record));
                fiber.control_stack.push(record_id);
                changes.control_stack_deltas.push(ControlStackDelta::Push {
                    fiber_id: fiber.fiber_id,
                    handle: record_id,
                });
                changes.events.push(RuntimeEvent::V2RaceOpened {
                    record_id,
                    fiber_id: fiber.fiber_id,
                    arm_count: *arm_count,
                });
                race_arms.clear();
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::V2ArmTimer { target } => {
                let value = fiber
                    .stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2ArmTimer"))?;
                let Value::I64(duration) = value else {
                    return Err(TransitionError::InvalidCommand(
                        "V2ArmTimer: duration must be I64",
                    ));
                };
                let record_id = *fiber.control_stack.last().ok_or(
                    TransitionError::InvalidCommand("V2ArmTimer: no open race"),
                )?;
                let due_at = context.logical_time().saturating_add(duration.max(0) as u64);
                let effect_id =
                    EffectId::for_instruction(instance.instance_id, fiber.fiber_id, fiber.pc.into());
                changes.effects.push(DurableEffect::schedule_timer(
                    effect_id,
                    fiber.fiber_id,
                    due_at,
                    TimerKind::V2Race {
                        record_id,
                        resume_at: (*target).into(),
                    },
                    None,
                ));
                race_arms.push(bpmn_lite_types::V2RaceArm::Timer { target: *target });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::V2ArmMsg {
                target,
                name,
                corr_reg,
            } => {
                race_arms.push(bpmn_lite_types::V2RaceArm::Msg {
                    target: *target,
                    name: *name,
                    corr_reg: *corr_reg,
                });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::V2ArmEffect {
                target,
                template_id,
                ..
            } => {
                // Arms immediately (as `V2ArmTimer` schedules its timer
                // immediately) — the effect_id is this arm's resolution
                // key, matched by `apply_ffi_completion` the same way a
                // message delivery matches `V2ArmMsg`'s `name`/`corr_reg`.
                let declaration = metadata
                    .v2_ffi_task_decls()
                    .get(&fiber.pc)
                    .ok_or(TransitionError::MissingMetadata("v2 FFI effect declaration"))?;
                if declaration.template_id != *template_id {
                    return Err(TransitionError::MissingMetadata("v2 FFI effect template identity"));
                }
                let input = encode_ffi_inputs(&instance, declaration)
                    .map_err(|_| TransitionError::InvalidCommand("FFI input contract violation"))?;
                let effect_id = EffectId::for_instruction(
                    instance.instance_id,
                    fiber.fiber_id,
                    fiber.pc.into(),
                );
                let operation = template_id
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                changes.effects.push(DurableEffect::Invoke {
                    effect_id,
                    fiber_id: fiber.fiber_id,
                    pc: fiber.pc.into(),
                    operation,
                    template_id: *template_id,
                    input,
                    idempotency_key: effect_id.as_uuid().to_string(),
                });
                changes.events.push(RuntimeEvent::FfiInvocationPending {
                    invocation_id: effect_id.as_uuid(),
                    template_id_hex: template_id
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect(),
                    caller_task_id: metadata
                        .debug_map()
                        .get(&fiber.pc)
                        .cloned()
                        .unwrap_or_else(|| format!("pc_{}", fiber.pc)),
                    caller_pc: fiber.pc,
                    owner_type: String::new(),
                });
                race_arms.push(bpmn_lite_types::V2RaceArm::Effect {
                    target: *target,
                    effect_id,
                    template_id: *template_id,
                });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::V2RaceClose => {
                let handle = fiber
                    .control_stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2RaceClose"))?;
                changes.control_stack_deltas.push(ControlStackDelta::Pop {
                    fiber_id: fiber.fiber_id,
                    handle,
                });
                changes.events.push(RuntimeEvent::V2RaceClosed {
                    record_id: handle,
                    fiber_id: fiber.fiber_id,
                    arm_count: race_arms.len() as u16,
                });
                fiber.wait = WaitState::V2Race {
                    record_id: handle,
                    arms: std::mem::take(&mut race_arms),
                };
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::V2GuardN { handler } => {
                // As V2Guard, `RecordKind::Guard { interrupting: false }`.
                // Q2 (EOP-VS-BPMN-ISA-002 §10, Adam-ratified): a
                // non-interrupting guard re-arms after its trigger — the
                // trigger path itself (not yet implemented for GuardN)
                // must re-`Insert` an `Armed` record rather than retiring
                // it, once built.
                let record_id = RecordId::new(context.derived_id(ordinal));
                ordinal = ordinal.saturating_add(1);
                let record = ConcurrencyRecord {
                    handler: Some(*handler),
                    rollback_domain_payload: Some(instance.domain_payload.to_string().into_boxed_str()),
                    rollback_domain_payload_hash: Some(instance.domain_payload_hash),
                    ..ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: false })
                };
                changes
                    .concurrency_mutations
                    .push(ConcurrencyMutation::Insert(record));
                fiber.control_stack.push(record_id);
                changes.control_stack_deltas.push(ControlStackDelta::Push {
                    fiber_id: fiber.fiber_id,
                    handle: record_id,
                });
                changes.events.push(RuntimeEvent::V2GuardOpened {
                    record_id,
                    fiber_id: fiber.fiber_id,
                    handler: *handler,
                });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::V2GuardNEnd => {
                let handle = fiber
                    .control_stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2GuardNEnd"))?;
                changes.control_stack_deltas.push(ControlStackDelta::Pop {
                    fiber_id: fiber.fiber_id,
                    handle,
                });
                changes
                    .concurrency_mutations
                    .push(ConcurrencyMutation::Retire(handle));
                changes.events.push(RuntimeEvent::V2GuardRetired {
                    record_id: handle,
                    fiber_id: fiber.fiber_id,
                });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            Instr::V2WaitFor => {
                let value = fiber
                    .stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2WaitFor"))?;
                let Value::I64(duration) = value else {
                    return Err(TransitionError::InvalidCommand(
                        "V2WaitFor: duration must be I64",
                    ));
                };
                let pc = fiber.pc;
                let deadline_ms = context
                    .logical_time()
                    .checked_add(duration.max(0) as u64)
                    .ok_or(TransitionError::NumericOverflow("V2WaitFor deadline"))?;
                fiber.pc = fiber.pc.saturating_add(1);
                fiber.wait = WaitState::Timer { deadline_ms };
                changes.events.push(RuntimeEvent::WaitTimerSet {
                    fiber_id: fiber.fiber_id,
                    deadline_ms,
                });
                changes.effects.push(DurableEffect::schedule_timer(
                    EffectId::for_instruction(instance.instance_id, fiber.fiber_id, pc.into()),
                    fiber.fiber_id,
                    deadline_ms,
                    TimerKind::Wait,
                    None,
                ));
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::V2WaitUntil => {
                let value = fiber
                    .stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2WaitUntil"))?;
                let Value::I64(deadline_ms) = value else {
                    return Err(TransitionError::InvalidCommand(
                        "V2WaitUntil: deadline must be I64",
                    ));
                };
                let deadline_ms = deadline_ms.max(0) as u64;
                let pc = fiber.pc;
                fiber.pc = fiber.pc.saturating_add(1);
                fiber.wait = WaitState::Timer { deadline_ms };
                changes.events.push(RuntimeEvent::WaitTimerSet {
                    fiber_id: fiber.fiber_id,
                    deadline_ms,
                });
                changes.effects.push(DurableEffect::schedule_timer(
                    EffectId::for_instruction(instance.instance_id, fiber.fiber_id, pc.into()),
                    fiber.fiber_id,
                    deadline_ms,
                    TimerKind::Wait,
                    None,
                ));
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::V2WaitMsg { name, corr_reg } => {
                let corr_key = fiber
                    .regs
                    .get(*corr_reg as usize)
                    .cloned()
                    .unwrap_or(Value::Bool(false));
                changes.events.push(RuntimeEvent::WaitMsgSubscribed {
                    fiber_id: fiber.fiber_id,
                    name: *name,
                    corr_key: corr_key.clone(),
                });
                // wait_id is v1's `CancelWait` bookkeeping identifier; it's
                // inert in resolution (apply_message matches on
                // name/corr_key only) and CancelWait itself is a no-op in
                // the kernel today, so 0 is a safe placeholder here.
                fiber.wait = WaitState::Msg {
                    wait_id: 0,
                    name: *name,
                    corr_key,
                };
                changes.fibers_upsert.push(fiber);
                return Ok(changes.finish(instance));
            }
            Instr::V2CancelScope => {
                // Adam-ratified (V&S §13 amendment v0.5, ruling B): reuses
                // the compensation op via the shared `v2_rollback_guard_scope`
                // — "all roads lead to Rome" with the automatic
                // rollback-on-failure path in `apply_job_failure`. In-line
                // (unlike that external-failure path), so the calling
                // fiber continues past it, not dies.
                let handle = fiber
                    .control_stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2CancelScope"))?;
                changes.control_stack_deltas.push(ControlStackDelta::Pop {
                    fiber_id: fiber.fiber_id,
                    handle,
                });
                let (rollback_payload, rollback_hash) = v2_rollback_guard_scope(
                    snapshot,
                    handle,
                    RollbackCaller::Continues(fiber.fiber_id),
                    &mut changes,
                )?;
                instance.domain_payload = rollback_payload.to_string().into();
                instance.domain_payload_hash = rollback_hash;
                fiber.pc = fiber.pc.saturating_add(1);
            }
        }
    }
    Err(TransitionError::StepLimitExceeded(step_limit))
}

#[derive(Default)]
struct Changes {
    fibers_upsert: Vec<Fiber>,
    fibers_delete: Vec<Uuid>,
    events: Vec<RuntimeEvent>,
    effects: Vec<DurableEffect>,
    effect_mutations: Vec<EffectMutation>,
    timer_mutations: Vec<TimerMutation>,
    jobs_enqueue: Vec<JobActivation>,
    jobs_ack: Vec<String>,
    job_mutations: Vec<JobMutation>,
    dedupe: Vec<DedupeWrite>,
    incidents: Vec<Incident>,
    join_mutations: Vec<JoinMutation>,
    buffered_messages: Vec<BufferedMessageMutation>,
    cleanup: Option<TerminalCleanup>,
    /// D1 deltas (V4's words are the sole producers, V1 only declared the
    /// surface). V&S v0.4 §12 ruling C: never populated for a fibre also
    /// present in `fibers_delete` in the same transition — deletion is
    /// the complete statement for a gone fibre's stack.
    concurrency_mutations: Vec<ConcurrencyMutation>,
    control_stack_deltas: Vec<ControlStackDelta>,
}

impl Changes {
    fn finish(self, instance: ProcessInstance) -> Transition {
        let mut builder = TransitionBuilder::new(instance);
        for fiber in self.fibers_upsert {
            builder = builder.upsert_fiber(fiber);
        }
        for fiber_id in self.fibers_delete {
            builder = builder.delete_fiber(fiber_id);
        }
        for event in self.events {
            builder = builder.event(event);
        }
        for effect in self.effects {
            builder = builder.effect(effect);
        }
        for mutation in self.effect_mutations {
            builder = builder.effect_mutation(mutation);
        }
        for mutation in self.timer_mutations {
            builder = builder.timer_mutation(mutation);
        }
        for job in self.jobs_enqueue {
            builder = builder.enqueue_job(job);
        }
        for job_key in self.jobs_ack {
            builder = builder.ack_job(job_key);
        }
        for mutation in self.job_mutations {
            builder = builder.job_mutation(mutation);
        }
        for dedupe in self.dedupe {
            builder = builder.dedupe(dedupe);
        }
        for incident in self.incidents {
            builder = builder.incident(incident);
        }
        for mutation in self.join_mutations {
            builder = builder.join_mutation(mutation);
        }
        for mutation in self.buffered_messages {
            builder = builder.buffered_message(mutation);
        }
        if let Some(cleanup) = self.cleanup {
            builder = builder.terminal_cleanup(cleanup);
        }
        for mutation in self.concurrency_mutations {
            builder = builder.concurrency_mutation(mutation);
        }
        for delta in self.control_stack_deltas {
            builder = builder.control_stack_delta(delta);
        }
        builder.build()
    }
}

fn apply_job_failure(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    command: &Command,
    context: &DeterministicContext,
) -> Result<Transition, TransitionError> {
    let Command::EffectFailed {
        effect_id,
        job_key,
        error_class,
        message,
        retry,
    } = command
    else {
        return Err(TransitionError::InvalidCommand(
            "job failure handler requires EffectFailed",
        ));
    };
    let retry = retry.as_ref();
    let mut changes = Changes::default();
    if snapshot.instance().state.is_terminal() {
        changes.events.push(RuntimeEvent::SignalIgnored {
            signal_desc: format!(
                "fail_job(key={job_key}, state={:?})",
                snapshot.instance().state
            ),
        });
        if !job_key.is_empty() {
            changes.jobs_ack.push(job_key.to_string());
        }
        return Ok(changes.finish(snapshot.instance().clone()));
    }
    let Some(mut fiber) = snapshot.fibers().values().find(|fiber| {
        matches!(&fiber.wait, WaitState::Job { job_key: parked } if parked == job_key)
            || matches!(&fiber.wait, WaitState::Race { job_key: Some(parked), .. } if parked == job_key)
            || matches!(fiber.wait, WaitState::Effect { effect_id: parked } if parked == *effect_id)
    }).cloned() else {
        changes.events.push(RuntimeEvent::SignalIgnored {
            signal_desc: format!("fail_job(key={job_key}, no fiber)"),
        });
        if !job_key.is_empty() { changes.jobs_ack.push(job_key.to_string()); }
        return Ok(changes.finish(snapshot.instance().clone()));
    };

    if let (ErrorClass::Transient, Some(retry)) = (error_class, retry) {
        changes.job_mutations.push(JobMutation::RetryClaimed {
            job_key: job_key.to_string(),
            worker_id: retry.worker_id().to_string(),
            claim_token: retry.claim_token().to_string(),
            error_class: "transient".to_string(),
            error_message: message.to_string(),
            not_before_ms: retry.not_before_ms(),
        });
        changes.events.push(RuntimeEvent::JobRetryScheduled {
            job_key: job_key.to_string(),
            retry_at: retry.not_before_ms(),
            retries_remaining: 0,
        });
        return Ok(changes.finish(snapshot.instance().clone()));
    }

    let rejection_route = match error_class {
        ErrorClass::BusinessRejection { rejection_code } => workflow
            .envelope()
            .metadata()
            .error_route_map()
            .get(&fiber.pc)
            .and_then(|routes| {
                routes.iter().find(|route| {
                    route
                        .error_code
                        .as_deref()
                        .map(|code| code == rejection_code)
                        .unwrap_or(true)
                })
            })
            .map(|route| (rejection_code, route)),
        _ => None,
    };
    if let Some((rejection_code, route)) = rejection_route {
        fiber.pc = route.resume_at;
        fiber.wait = WaitState::Running;
        changes.fibers_upsert.push(fiber);
        if !job_key.is_empty() {
            changes.jobs_ack.push(job_key.to_string());
        }
        changes.events.push(RuntimeEvent::ErrorRouted {
            job_key: job_key.to_string(),
            error_code: rejection_code.clone(),
            boundary_id: route.boundary_element_id.clone(),
            resume_at: route.resume_at,
        });
        return Ok(changes.finish(snapshot.instance().clone()));
    }

    // Adam-ratified (V&S §13 amendment v0.5, ruling C): a *definitive*
    // job failure (reached here — no retry token left, no matching
    // error_route_map entry) for a fibre sitting inside an armed
    // *interrupting* V2Guard scope bypasses the v1 error-route/incident
    // path entirely — "any fail whatsoever" inside an interrupting guard
    // is binary (go/stay), not a taxonomy. A transient failure that's
    // still retriable (handled above) is not "a fail" yet — it can still
    // succeed. This does not apply to V2GuardN (non-interrupting)
    // scopes, or outside any guard scope, or to a timer/wait resolving
    // normally (not reachable through this function at all — TimerFired
    // has its own handler). Uses the same shared `v2_rollback_guard_scope`
    // as `V2CancelScope` — "all roads lead to Rome" — but the triggering
    // fibre is killed (not continued in place): there's no "next
    // instruction" for an externally-surfaced job failure to fall
    // through to, and per Adam: "kill the fibre... can simply be re-run"
    // — the instance is left exactly as it was at scope-open, ready for
    // an external actor to retry the whole scope, not auto-respawned.
    if let Some(guard_handle) = fiber.control_stack.iter().rev().find(|id| {
        matches!(
            snapshot.concurrency_table().get(**id),
            Some(record)
                if matches!(record.kind, RecordKind::Guard { interrupting: true })
                    && record.state == RecordState::Armed
        )
    }) {
        let guard_handle = *guard_handle;
        let (rollback_payload, rollback_hash) = v2_rollback_guard_scope(
            snapshot,
            guard_handle,
            RollbackCaller::Dies(fiber.fiber_id),
            &mut changes,
        )?;
        let mut instance = snapshot.instance().clone();
        instance.domain_payload = rollback_payload.to_string().into();
        instance.domain_payload_hash = rollback_hash;
        if !job_key.is_empty() {
            changes.jobs_ack.push(job_key.to_string());
        }
        return Ok(changes.finish(instance));
    }

    let incident_id = context.derived_id(0);
    let service_task_id = workflow
        .envelope()
        .metadata()
        .debug_map()
        .get(&fiber.pc)
        .cloned()
        .unwrap_or_else(|| format!("pc_{}", fiber.pc));
    let incident = Incident {
        incident_id,
        process_instance_id: snapshot.instance().instance_id,
        fiber_id: fiber.fiber_id,
        service_task_id: service_task_id.clone(),
        bytecode_addr: fiber.pc,
        error_class: error_class.clone(),
        message: message.to_string(),
        retry_count: 0,
        created_at: logical_timestamp(context)?,
        resolved_at: None,
        resolution: None,
    };
    fiber.wait = WaitState::Incident { incident_id };
    let mut instance = snapshot.instance().clone();
    instance.state = ProcessState::Failed { incident_id };
    changes.incidents.push(incident);
    changes.fibers_upsert.push(fiber);
    changes.events.push(RuntimeEvent::IncidentCreated {
        incident_id,
        service_task_id,
        job_key: (!job_key.is_empty()).then(|| job_key.to_string()),
    });
    if job_key.is_empty() {
        changes.effect_mutations.push(EffectMutation::terminal(
            *effect_id,
            EffectTerminalState::Failed,
        ));
    }
    if let Some(retry) = retry {
        changes.job_mutations.push(JobMutation::DeadLetterClaimed {
            job_key: job_key.to_string(),
            worker_id: retry.worker_id().to_string(),
            claim_token: retry.claim_token().to_string(),
            error_class: error_class_label(error_class).to_string(),
            error_message: message.to_string(),
            incident_id,
        });
        changes.events.push(RuntimeEvent::JobDeadLettered {
            job_key: job_key.to_string(),
            incident_id,
        });
    } else {
        if !job_key.is_empty() {
            changes.jobs_ack.push(job_key.to_string());
        }
    }
    Ok(changes.finish(instance))
}

fn apply_ffi_completion(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    command: &Command,
) -> Result<Transition, TransitionError> {
    let Command::EffectCompleted {
        effect_id,
        output:
            EffectOutput::Ffi {
                fiber_id,
                pc,
                output_payload,
                new_domain_payload,
                no_match,
            },
    } = command
    else {
        return Err(TransitionError::InvalidCommand(
            "FFI completion handler requires an FFI effect result",
        ));
    };
    let mut fiber = snapshot
        .fiber(*fiber_id)
        .cloned()
        .ok_or(TransitionError::MissingFiber(*fiber_id))?;

    // `WaitState::Effect` (v1 `ExecFfi`/v2 `V2AwaitEffect`, standalone) and
    // `WaitState::V2Race` (v2 `V2ArmEffect`, one alternative among several)
    // both resolve through this one completion path (V&S §5: same
    // `DurableEffect::Invoke`/park-on-completion shape).
    //
    // A `V2ArmEffect` alternative can legitimately complete *after* the
    // race it belonged to already resolved via a different arm — by then
    // `fiber.wait` is plain `Running`, not `V2Race` (the transient arm list
    // lived only in that wait state and is gone once the fibre resumed).
    // That is not corruption: it's the same "late signal after the race
    // already resolved" shape `apply_timer`'s `TimerKind::V2Race` branch
    // already tolerates as a no-op. To tell a genuine late arm-completion
    // apart from a bogus command with no such history, recompute the
    // effect_id `V2ArmEffect` would have derived for `(instance, fiber,
    // pc)` — `EffectId::for_instruction` is a pure function of that triple,
    // so a match proves this effect really was armed at that instruction
    // for this fiber, independent of whether the race record survived.
    let is_v2_arm_effect = matches!(
        workflow.envelope().instructions().get(Addr::from(*pc).index()),
        Some(Instr::V2ArmEffect { .. })
    );
    let race_win = match &fiber.wait {
        WaitState::Effect { effect_id: parked } if *parked == *effect_id && fiber.pc == Addr::from(*pc) => {
            None
        }
        WaitState::V2Race { record_id, arms } => {
            match arms.iter().find_map(|arm| match arm {
                bpmn_lite_types::V2RaceArm::Effect {
                    target,
                    effect_id: arm_effect_id,
                    ..
                } if *arm_effect_id == *effect_id => Some(*target),
                _ => None,
            }) {
                Some(target) => Some((*record_id, target)),
                None if is_v2_arm_effect => {
                    // A different arm of *this same* still-parked race won
                    // — legitimate late arrival, no-op.
                    return Ok(TransitionBuilder::new(snapshot.instance().clone())
                        .effect_mutation(EffectMutation::terminal(
                            *effect_id,
                            EffectTerminalState::Completed,
                        ))
                        .build());
                }
                None => {
                    return Err(TransitionError::InvalidCommand(
                        "FFI completion does not match parked effect",
                    ));
                }
            }
        }
        _ if is_v2_arm_effect
            && *effect_id
                == EffectId::for_instruction(snapshot.instance().instance_id, *fiber_id, *pc) =>
        {
            // The race already resolved via a different arm (or fully
            // unwound) and this fibre moved on — legitimate late arrival.
            return Ok(TransitionBuilder::new(snapshot.instance().clone())
                .effect_mutation(EffectMutation::terminal(
                    *effect_id,
                    EffectTerminalState::Completed,
                ))
                .build());
        }
        _ => {
            return Err(TransitionError::InvalidCommand(
                "FFI completion does not match parked effect",
            ));
        }
    };

    let mut instance = snapshot.instance().clone();
    if !*no_match {
        if let Some(payload) = new_domain_payload.as_deref() {
            instance.domain_payload = payload.to_string().into();
            instance.domain_payload_hash = EffectId::content_hash(payload.as_bytes());
        } else {
            let metadata = workflow.envelope().metadata();
            let addr = Addr::from(*pc);
            // `WaitState::Effect` is shared between v1 `ExecFfi` and v2
            // `V2AwaitEffect`/`V2ArmEffect` (V&S §5) — the parked
            // instruction's own identity, not the wait-state shape, decides
            // which binding table supplies the declaration.
            let is_v2 = matches!(
                workflow.envelope().instructions().get(addr.index()),
                Some(Instr::V2AwaitEffect { .. } | Instr::V2ArmEffect { .. })
            );
            let declaration = if is_v2 {
                metadata
                    .v2_ffi_task_decls()
                    .get(&addr)
                    .ok_or(TransitionError::MissingMetadata("v2 FFI effect declaration"))?
            } else {
                metadata
                    .ffi_task_decls()
                    .get(&addr)
                    .ok_or(TransitionError::MissingMetadata("FFI task declaration"))?
            };
            apply_ffi_outputs(&mut instance, declaration, output_payload)
                .map_err(|_| TransitionError::InvalidCommand("FFI output contract violation"))?;
        }
    }

    let mut builder = TransitionBuilder::new(instance);
    match race_win {
        Some((record_id, target)) => {
            fiber.pc = target;
            fiber.wait = WaitState::Running;
            builder = builder
                .concurrency_mutation(ConcurrencyMutation::Retire(record_id))
                .event(RuntimeEvent::V2RaceWon {
                    record_id,
                    fiber_id: fiber.fiber_id,
                })
                .timer_mutation(TimerMutation::V2CancelRace {
                    fiber_id: fiber.fiber_id,
                    record_id,
                    except: *effect_id,
                });
        }
        None => {
            fiber.pc = fiber.pc.saturating_add(1);
            fiber.wait = WaitState::Running;
        }
    }
    Ok(builder
        .upsert_fiber(fiber)
        .effect_mutation(EffectMutation::terminal(
            *effect_id,
            EffectTerminalState::Completed,
        ))
        .event(RuntimeEvent::FfiInvocationCompleted {
            invocation_id: effect_id.as_uuid(),
            outcome_kind: if *no_match { "no_match" } else { "success" }.to_string(),
            error_message: None,
        })
        .build())
}

fn apply_message(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    command: &Command,
    context: &DeterministicContext,
) -> Result<Transition, TransitionError> {
    let Command::MessageDelivered {
        message_id,
        name,
        correlation_key,
        payload,
        payload_hash,
        expires_at,
    } = command
    else {
        return Err(TransitionError::InvalidCommand(
            "message handler requires MessageDelivered",
        ));
    };
    let received_at = logical_timestamp(context)?;
    let message = bpmn_lite_types::BufferedMessage {
        tenant_id: snapshot.instance().tenant_id.clone(),
        message_name: name.to_string(),
        correlation_key: correlation_key.to_string(),
        msg_id: message_id.to_string(),
        payload: payload.to_vec(),
        payload_hash: *payload_hash,
        process_instance_id: Some(snapshot.instance().instance_id),
        received_at,
        expires_at: *expires_at,
    };
    let mut changes = Changes::default();
    if snapshot.instance().state.is_terminal() {
        changes
            .buffered_messages
            .push(BufferedMessageMutation::Deliver(message));
        changes.events.push(RuntimeEvent::SignalIgnored {
            signal_desc: format!(
                "signal on {:?} instance: msg={name}",
                snapshot.instance().state
            ),
        });
        return Ok(changes.finish(snapshot.instance().clone()));
    }

    let metadata = workflow.envelope().metadata();
    let mut matched = None;
    let mut v2_race_record = None;
    for fiber in snapshot.fibers().values() {
        match &fiber.wait {
            WaitState::Msg {
                name: waiting_name,
                corr_key,
                ..
            } if message_name_matches(metadata.message_name_map(), name, *waiting_name)
                && value_key(corr_key) == correlation_key.as_str() =>
            {
                let mut resumed = fiber.clone();
                resumed.wait = WaitState::Running;
                matched = Some((resumed, None));
                break;
            }
            WaitState::V2Race { record_id, arms } => {
                for arm in arms {
                    let bpmn_lite_types::V2RaceArm::Msg {
                        target,
                        name: waiting_name,
                        corr_reg,
                    } = arm
                    else {
                        continue;
                    };
                    let corr = fiber
                        .regs
                        .get(*corr_reg as usize)
                        .cloned()
                        .unwrap_or(Value::Bool(false));
                    if message_name_matches(metadata.message_name_map(), name, *waiting_name)
                        && value_key(&corr) == correlation_key.as_str()
                    {
                        let mut resumed = fiber.clone();
                        resumed.pc = *target;
                        resumed.wait = WaitState::Running;
                        matched = Some((resumed, None));
                        v2_race_record = Some(*record_id);
                        break;
                    }
                }
                if matched.is_some() {
                    break;
                }
            }
            WaitState::Race { race_id, .. } => {
                if let Some(race) = metadata.race_plan().get(race_id) {
                    for (index, arm) in race.arms.iter().enumerate() {
                        let WaitArm::Msg {
                            name: waiting_name,
                            corr_reg,
                            resume_at,
                        } = arm
                        else {
                            continue;
                        };
                        let corr = fiber
                            .regs
                            .get(*corr_reg as usize)
                            .cloned()
                            .unwrap_or(Value::Bool(false));
                        if message_name_matches(metadata.message_name_map(), name, *waiting_name)
                            && value_key(&corr) == correlation_key.as_str()
                        {
                            let mut resumed = fiber.clone();
                            resumed.pc = *resume_at;
                            resumed.wait = WaitState::Running;
                            matched = Some((
                                resumed,
                                Some((*race_id, index, race.arms.len(), *resume_at)),
                            ));
                            break;
                        }
                    }
                }
                if matched.is_some() {
                    break;
                }
            }
            _ => {}
        }
    }

    changes.events.push(RuntimeEvent::MessageBuffered {
        message_name: name.to_string(),
        correlation_key: correlation_key.to_string(),
        msg_id: message_id.to_string(),
        expires_at: *expires_at,
    });
    let Some((fiber, race)) = matched else {
        changes
            .buffered_messages
            .push(BufferedMessageMutation::Insert(message));
        return Ok(changes.finish(snapshot.instance().clone()));
    };
    let mut instance = snapshot.instance().clone();
    if let Some(hash) = *payload_hash {
        let text = std::str::from_utf8(payload)
            .map_err(|_| TransitionError::InvalidCommand("message payload is not UTF-8"))?;
        instance.domain_payload = text.to_string().into();
        instance.domain_payload_hash = hash;
    }
    changes
        .buffered_messages
        .push(BufferedMessageMutation::Deliver(message));
    changes.events.push(RuntimeEvent::BufferedMessageConsumed {
        message_name: name.to_string(),
        correlation_key: correlation_key.to_string(),
        msg_id: message_id.to_string(),
        fiber_id: fiber.fiber_id,
    });
    let message_name_id = metadata
        .message_name_map()
        .iter()
        .find_map(|(id, candidate)| (candidate == name).then_some(*id))
        .unwrap_or(0);
    changes.events.push(RuntimeEvent::MsgReceived {
        name: message_name_id,
        corr_key: parse_value_key(correlation_key),
        msg_ref: None,
    });
    if let Some((race_id, winner_index, arm_count, resume_at)) = race {
        changes.events.push(RuntimeEvent::RaceWon {
            race_id,
            fiber_id: fiber.fiber_id,
            winner_index,
            resume_at,
        });
        let cancelled_indices = (0..arm_count)
            .filter(|index| *index != winner_index)
            .collect();
        changes.events.push(RuntimeEvent::RaceCancelled {
            race_id,
            cancelled_indices,
        });
        changes.timer_mutations.push(TimerMutation::CancelRace {
            fiber_id: fiber.fiber_id,
            race_id,
            except: EffectId::for_instruction(
                snapshot.instance().instance_id,
                fiber.fiber_id,
                fiber.pc.into(),
            ),
        });
    }
    if let Some(record_id) = v2_race_record {
        changes
            .concurrency_mutations
            .push(ConcurrencyMutation::Retire(record_id));
        changes.events.push(RuntimeEvent::V2RaceWon {
            record_id,
            fiber_id: fiber.fiber_id,
        });
        changes.timer_mutations.push(TimerMutation::V2CancelRace {
            fiber_id: fiber.fiber_id,
            record_id,
            except: EffectId::for_instruction(
                snapshot.instance().instance_id,
                fiber.fiber_id,
                fiber.pc.into(),
            ),
        });
    }
    changes.fibers_upsert.push(fiber);
    Ok(changes.finish(instance))
}

fn message_name_matches(
    names: &std::collections::BTreeMap<u32, String>,
    requested: &str,
    waiting: u32,
) -> bool {
    names
        .get(&waiting)
        .map(|name| name == requested)
        .unwrap_or_else(|| requested == waiting.to_string())
}

fn value_key(value: &Value) -> String {
    match value {
        Value::Bool(value) => format!("b:{value}"),
        Value::I64(value) => format!("i:{value}"),
        Value::Str(value) => format!("s:{value}"),
        Value::Ref(value) => format!("r:{value}"),
    }
}

fn parse_value_key(value: &str) -> Value {
    if let Some(raw) = value.strip_prefix("b:") {
        return Value::Bool(raw == "true");
    }
    if let Some(raw) = value.strip_prefix("i:").and_then(|raw| raw.parse().ok()) {
        return Value::I64(raw);
    }
    if let Some(raw) = value.strip_prefix("s:").and_then(|raw| raw.parse().ok()) {
        return Value::Str(raw);
    }
    if let Some(raw) = value.strip_prefix("r:").and_then(|raw| raw.parse().ok()) {
        return Value::Ref(raw);
    }
    Value::Bool(false)
}

fn error_class_label(error_class: &ErrorClass) -> &'static str {
    match error_class {
        ErrorClass::Transient => "transient",
        ErrorClass::ContractViolation => "contract_violation",
        ErrorClass::BusinessRejection { .. } => "business_rejection",
    }
}

fn apply_job_completion(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    completion: &bpmn_lite_types::JobCompletion,
) -> Result<Transition, TransitionError> {
    let current_hash = blake3_hash(snapshot.instance().domain_payload.as_bytes());
    if completion.expected_instance_payload_hash != current_hash {
        return Err(TransitionError::OptimisticConflict);
    }
    if snapshot.instance().state.is_terminal() {
        return Ok(TransitionBuilder::new(snapshot.instance().clone())
            .event(RuntimeEvent::SignalIgnored {
                signal_desc: format!(
                    "complete_job(key={}, state={:?})",
                    completion.job_key,
                    snapshot.instance().state
                ),
            })
            .ack_job(completion.job_key.clone())
            .build());
    }
    let mut fiber = snapshot
        .fibers()
        .values()
        .find(|fiber| matches!(&fiber.wait, WaitState::Job { job_key } if job_key == &completion.job_key)
            || matches!(&fiber.wait, WaitState::Race { job_key: Some(job_key), .. } if job_key == &completion.job_key))
        .cloned()
        .ok_or(TransitionError::InvalidCommand("completion has no parked fiber"))?;
    let before = current_hash;
    let mut instance = snapshot.instance().clone();
    apply_completion(&mut instance, completion);
    let mut changes = Changes::default();
    let current_pc = fiber.pc;
    if let WaitState::Race { race_id, .. } = fiber.wait.clone() {
        let race = workflow
            .envelope()
            .metadata()
            .race_plan()
            .get(&race_id)
            .ok_or(TransitionError::MissingMetadata("job boundary race"))?;
        let (winner_index, resume_at) = race
            .arms
            .iter()
            .enumerate()
            .find_map(|(index, arm)| {
                matches!(arm, WaitArm::Internal { .. }).then(|| (index, arm.resume_at()))
            })
            .ok_or(TransitionError::MissingMetadata(
                "job boundary internal arm",
            ))?;
        fiber.pc = resume_at;
        changes.events.push(RuntimeEvent::RaceWon {
            race_id,
            fiber_id: fiber.fiber_id,
            winner_index,
            resume_at,
        });
        changes.events.push(RuntimeEvent::RaceCancelled {
            race_id,
            cancelled_indices: (0..race.arms.len())
                .filter(|index| *index != winner_index)
                .collect(),
        });
        changes.timer_mutations.push(TimerMutation::CancelRace {
            fiber_id: fiber.fiber_id,
            race_id,
            except: EffectId::for_instruction(instance.instance_id, fiber.fiber_id, current_pc.into()),
        });
    } else {
        fiber.pc = fiber.pc.saturating_add(1);
    }
    fiber.wait = WaitState::Running;
    changes.events.push(RuntimeEvent::JobCompleted {
        job_key: completion.job_key.clone(),
        payload_hash_before: before,
        payload_hash_after: instance.domain_payload_hash,
        orch_flags_out: completion.orch_flags.clone(),
        pc_next: fiber.pc,
    });
    changes.fibers_upsert.push(fiber);
    changes.jobs_ack.push(completion.job_key.clone());
    changes.dedupe.push(DedupeWrite::new(
        completion.job_key.clone(),
        completion.clone(),
    ));
    Ok(changes.finish(instance))
}

/// V4.1: resolve an interrupting guard's trigger (V&S v0.4 §4/§12 ruling
/// A). No v2 word records a record's parent — nesting is discovered by
/// scanning every fibre's control stack for `record_id`, since a child
/// scope's handle only ever appears further along the same fibres'
/// stacks that carry the parent's handle (`V2Fork`/`V2Guard` inherit-then-
/// push). A parked fibre's `WaitState` (`V2Barrier`/`V2Race`) names one
/// more implicit level of nesting beyond what's left on its control
/// stack, since `V2Join`/`V2RaceClose` pop their handle at park time
/// (V4.1 design note: the popped handle is not lost information — the
/// record tree is reconstructed here from the union of every live
/// fibre's stack-plus-wait-state, not from any single fibre's history).
fn apply_v2_trigger_guard(
    snapshot: &Snapshot,
    command: &Command,
    context: &DeterministicContext,
) -> Result<Transition, TransitionError> {
    let Command::V2TriggerGuard { record_id } = command else {
        return Err(TransitionError::InvalidCommand(
            "v2 guard trigger requires V2TriggerGuard",
        ));
    };
    let record = snapshot
        .concurrency_table()
        .get(*record_id)
        .cloned()
        .ok_or(TransitionError::InvalidCommand(
            "V2TriggerGuard: unknown guard handle",
        ))?;
    let interrupting = match record.kind {
        RecordKind::Guard { interrupting } => interrupting,
        _ => {
            return Err(TransitionError::InvalidCommand(
                "V2TriggerGuard: handle is not a guard",
            ));
        }
    };
    if record.state != RecordState::Armed {
        return Err(TransitionError::InvalidCommand(
            "V2TriggerGuard: guard is not armed",
        ));
    }
    let handler_target = record.handler.ok_or(TransitionError::InvalidCommand(
        "V2TriggerGuard: guard has no handler address",
    ))?;

    // V-4: interrupting `V2Guard`'s handler entry state is PRE-push (the
    // guard is being torn down, so the handler inherits whatever existed
    // before it). Non-interrupting `V2GuardN` is the opposite — POST-push
    // (fixture-proven independently in v2_verifier.rs,
    // `v4_guardn_handler_entry_state_is_post_push_inherits_own_token`):
    // since nothing unwinds, the handler inherits the guard's own
    // still-armed token too, mirroring `V2Fork`'s
    // children-inherit-parent-stack-plus-own-token pattern. Every fibre
    // still inside the guard shares the same prefix up to this handle
    // (inherited unchanged through `V2Fork`'s `control_stack.clone()`),
    // so any one of them fixes it.
    let handler_stack = snapshot
        .fibers()
        .values()
        .find_map(|fiber| {
            fiber
                .control_stack
                .iter()
                .position(|id| *id == *record_id)
                .map(|pos| {
                    let end = if interrupting { pos } else { pos + 1 };
                    fiber.control_stack[..end].to_vec()
                })
        })
        .unwrap_or_default();

    let mut changes = Changes::default();
    let handler_fiber_id = context.derived_id(0);
    let mut handler_fiber = Fiber::new(handler_fiber_id, handler_target);
    handler_fiber.control_stack = handler_stack;
    changes.fibers_upsert.push(handler_fiber);
    changes.events.push(RuntimeEvent::FiberSpawned {
        fiber_id: handler_fiber_id,
        pc: handler_target,
        parent: None,
    });

    if interrupting {
        let mut retire_order = Vec::new();
        let mut cancelled_fibers = Vec::new();
        v2_cancel_guard_scope(snapshot, *record_id, &mut retire_order, &mut cancelled_fibers);
        for id in &retire_order {
            changes
                .concurrency_mutations
                .push(ConcurrencyMutation::Retire(*id));
        }
        for fiber_id in &cancelled_fibers {
            changes.fibers_delete.push(*fiber_id);
        }
        changes.events.push(RuntimeEvent::V2GuardTriggered {
            record_id: *record_id,
            handler_fiber_id,
            cancelled_records: retire_order,
            cancelled_fibers,
        });
    } else {
        // Q2 ratified (V&S §13 amendment v0.5, ruling A): a
        // non-interrupting guard re-arms — spawn the handler, unwind
        // nothing, leave the record exactly as it was (still `Armed`;
        // no mutation to emit, it never changed).
        changes.events.push(RuntimeEvent::V2GuardNTriggered {
            record_id: *record_id,
            handler_fiber_id,
        });
    }
    Ok(changes.finish(snapshot.instance().clone()))
}

/// Post-order (deepest-first) walk of the record tree rooted at `parent`,
/// per ruling A. `retire_order` accumulates records in cancellation
/// order; `leaf_fibers` accumulates fibres with no further nesting under
/// `parent`, in fibre-ID order (`snapshot.fibers()` is `BTreeMap`-backed,
/// so `.values()` already yields ascending `Uuid` order — ruling A's
/// within-record tiebreak).
fn v2_cancel_guard_scope(
    snapshot: &Snapshot,
    parent: RecordId,
    retire_order: &mut Vec<RecordId>,
    leaf_fibers: &mut Vec<Uuid>,
) {
    let mut children = std::collections::BTreeSet::new();
    let mut direct_leaves = Vec::new();
    for fiber in snapshot.fibers().values() {
        let mut chain = fiber.control_stack.clone();
        match &fiber.wait {
            WaitState::V2Barrier { record_id } => chain.push(*record_id),
            WaitState::V2Race { record_id, .. } => chain.push(*record_id),
            _ => {}
        }
        let Some(pos) = chain.iter().position(|id| *id == parent) else {
            continue;
        };
        match chain.get(pos + 1) {
            Some(child) => {
                children.insert(*child);
            }
            None => direct_leaves.push(fiber.fiber_id),
        }
    }
    for child in children {
        v2_cancel_guard_scope(snapshot, child, retire_order, leaf_fibers);
    }
    leaf_fibers.extend(direct_leaves);
    retire_order.push(parent);
}

/// Whether the fibre that triggered a `v2_rollback_guard_scope` call
/// continues past it or dies. `V2CancelScope` (an in-line instruction)
/// continues; automatic rollback-on-definitive-failure (an externally
/// surfaced job/effect failure with no "next instruction" to fall
/// through to) dies. See V&S §13 amendment v0.5, ruling C.
enum RollbackCaller {
    Continues(Uuid),
    Dies(Uuid),
}

/// V&S §13 amendment v0.5, ruling B/C — "all roads lead to Rome": the one
/// shared rollback op behind both `V2CancelScope` (in-line, ruling B) and
/// automatic rollback-on-definitive-job-failure inside an interrupting
/// guard (`apply_job_failure`, ruling C). Restores `guard_handle`'s
/// captured `domain_payload` snapshot, unwinds nested records/members
/// deepest-first (`v2_cancel_guard_scope`), and emits the shared
/// `V2ScopeCancelled` audit event. Returns the restored payload/hash for
/// the caller to apply to its own `instance` (this function doesn't own
/// `instance` — `V2CancelScope` mutates the in-flight `apply_tick`
/// local, `apply_job_failure` clones fresh from `snapshot`).
fn v2_rollback_guard_scope(
    snapshot: &Snapshot,
    guard_handle: RecordId,
    caller: RollbackCaller,
    changes: &mut Changes,
) -> Result<(Box<str>, [u8; 32]), TransitionError> {
    let record = snapshot
        .concurrency_table()
        .get(guard_handle)
        .cloned()
        .ok_or(TransitionError::InvalidCommand(
            "rollback: unknown scope handle",
        ))?;
    let (rollback_payload, rollback_hash) = match (
        record.rollback_domain_payload,
        record.rollback_domain_payload_hash,
    ) {
        (Some(payload), Some(hash)) => (payload, hash),
        _ => {
            return Err(TransitionError::InvalidCommand(
                "rollback: handle carries no rollback snapshot",
            ));
        }
    };
    let mut retire_order = Vec::new();
    let mut cancelled_fibers = Vec::new();
    v2_cancel_guard_scope(snapshot, guard_handle, &mut retire_order, &mut cancelled_fibers);
    let fiber_id = match caller {
        RollbackCaller::Continues(id) => {
            // The walk (reading pre-tick `snapshot`) still sees
            // `guard_handle` on this fibre's own control stack and would
            // otherwise treat it as a leaf member to delete — it isn't,
            // it continues past this call.
            cancelled_fibers.retain(|cancelled| *cancelled != id);
            id
        }
        RollbackCaller::Dies(id) => id,
    };
    for id in &retire_order {
        changes
            .concurrency_mutations
            .push(ConcurrencyMutation::Retire(*id));
    }
    for fiber_id in &cancelled_fibers {
        changes.fibers_delete.push(*fiber_id);
    }
    changes.events.push(RuntimeEvent::V2ScopeCancelled {
        record_id: guard_handle,
        fiber_id,
        cancelled_records: retire_order,
        cancelled_fibers,
    });
    Ok((rollback_payload, rollback_hash))
}

fn join_arrive(snapshot: &Snapshot, changes: &Changes, join_id: JoinId) -> u16 {
    changes.join_mutations.iter().fold(
        snapshot.join_count(join_id).saturating_add(1),
        |count, mutation| match mutation {
            JoinMutation::Arrive(id) if *id == join_id => count.saturating_add(1),
            JoinMutation::Reset(id) if *id == join_id => 1,
            _ => count,
        },
    )
}

fn timer_arm(arm: &WaitArm) -> Result<(Addr, Option<TimerRepeatSpec>), TransitionError> {
    match arm {
        WaitArm::Timer {
            resume_at, cycle, ..
        } => Ok((
            *resume_at,
            cycle
                .as_ref()
                .map(|cycle| TimerRepeatSpec::new(cycle.interval_ms, cycle.max_fires, 0)),
        )),
        WaitArm::Deadline { resume_at, .. } => Ok((*resume_at, None)),
        _ => Err(TransitionError::InvalidCommand(
            "timer arm metadata is not a timer",
        )),
    }
}

fn logical_timestamp(context: &DeterministicContext) -> Result<i64, TransitionError> {
    i64::try_from(context.logical_time())
        .map_err(|_| TransitionError::NumericOverflow("logical timestamp"))
}

fn fail_contract(
    mut instance: ProcessInstance,
    mut fiber: Fiber,
    mut changes: Changes,
    context: &DeterministicContext,
    message: &str,
) -> Result<Transition, TransitionError> {
    let incident_id = context.derived_id(0);
    let incident = Incident {
        incident_id,
        process_instance_id: instance.instance_id,
        fiber_id: fiber.fiber_id,
        service_task_id: format!("pc_{}", fiber.pc),
        bytecode_addr: fiber.pc,
        error_class: ErrorClass::ContractViolation,
        message: message.to_string(),
        retry_count: 0,
        created_at: logical_timestamp(context)?,
        resolved_at: None,
        resolution: None,
    };
    fiber.wait = WaitState::Incident { incident_id };
    instance.state = ProcessState::Failed { incident_id };
    changes.incidents.push(incident);
    changes.events.push(RuntimeEvent::IncidentCreated {
        incident_id,
        service_task_id: format!("pc_{}", fiber.pc),
        job_key: None,
    });
    changes.fibers_upsert.push(fiber);
    Ok(changes.finish(instance))
}

fn apply_completion(instance: &mut ProcessInstance, completion: &bpmn_lite_types::JobCompletion) {
    instance.domain_payload = completion.domain_payload.clone().into();
    instance.domain_payload_hash = blake3_hash(completion.domain_payload.as_bytes());
    for (name, value) in &completion.orch_flags {
        let key = name
            .strip_prefix("flag_")
            .and_then(|raw| raw.parse::<u32>().ok());
        if let Some(key) = key {
            instance.flags.insert(key, value.clone());
        }
    }
}

fn blake3_hash(bytes: &[u8]) -> [u8; 32] {
    // Hashing remains centralized in the leaf types crate to keep the kernel's
    // dependency edge singular.
    EffectId::content_hash(bytes)
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::I64(value) => *value != 0,
        Value::Str(value) => *value != 0,
        Value::Ref(value) => *value != 0,
    }
}

fn describe_wait(wait: &WaitState) -> String {
    match wait {
        WaitState::Running => String::new(),
        WaitState::Timer { deadline_ms } => format!("Timer({deadline_ms})"),
        WaitState::Msg { name, corr_key, .. } => format!("Msg({name}, {corr_key:?})"),
        WaitState::Job { job_key } => format!("Job({job_key})"),
        WaitState::Effect { effect_id } => format!("Effect({})", effect_id.as_uuid()),
        WaitState::Race { race_id, .. } => format!("Race({race_id})"),
        WaitState::Join { join_id, .. } => format!("Join({join_id})"),
        WaitState::V2Barrier { record_id } => format!("V2Barrier({record_id})"),
        WaitState::V2Race { record_id, arms } => {
            format!("V2Race({record_id}, {} arms)", arms.len())
        }
        WaitState::Incident { incident_id } => format!("Incident({incident_id})"),
    }
}

fn apply_timer(
    snapshot: &Snapshot,
    timer: &bpmn_lite_types::ClaimedTimer,
    fired_at: u64,
) -> Result<Transition, TransitionError> {
    if timer.tenant_id().as_str() != snapshot.instance().tenant_id {
        return Err(TransitionError::InvalidCommand(
            "timer tenant does not match snapshot",
        ));
    }
    if timer.instance_id() != snapshot.instance().instance_id {
        return Err(TransitionError::InvalidCommand(
            "timer instance does not match snapshot",
        ));
    }
    if fired_at < timer.due_at() {
        return Err(TransitionError::InvalidCommand("timer fired before due_at"));
    }

    let mut builder = TransitionBuilder::new(snapshot.instance().clone());
    let mut rearmed = false;
    match (timer.kind(), snapshot.fiber(timer.fiber_id()).cloned()) {
        (TimerKind::Wait, Some(mut fiber)) if matches!(fiber.wait, WaitState::Timer { deadline_ms } if deadline_ms <= fired_at) =>
        {
            fiber.wait = WaitState::Running;
            builder = builder.upsert_fiber(fiber).event(RuntimeEvent::TimerFired {
                timer_id: timer.timer_id(),
                fiber_id: timer.fiber_id(),
                fired_at,
            });
        }
        (
            TimerKind::Race {
                race_id,
                arm_index,
                resume_at,
                interrupting,
                job_key,
                boundary_element_id,
                arm_count,
            },
            Some(mut fiber),
        ) if matches!(&fiber.wait, WaitState::Race { race_id: current, .. } if current == race_id) =>
        {
            builder = builder.event(RuntimeEvent::TimerFired {
                timer_id: timer.timer_id(),
                fiber_id: timer.fiber_id(),
                fired_at,
            });
            if *interrupting {
                fiber.pc = Addr::from(*resume_at);
                fiber.wait = WaitState::Running;
                builder = builder
                    .upsert_fiber(fiber)
                    .event(RuntimeEvent::RaceWon {
                        race_id: *race_id,
                        fiber_id: timer.fiber_id(),
                        winner_index: *arm_index,
                        resume_at: Addr::from(*resume_at),
                    })
                    .event(RuntimeEvent::RaceCancelled {
                        race_id: *race_id,
                        cancelled_indices: (0..*arm_count)
                            .filter(|index| *index != *arm_index)
                            .collect(),
                    })
                    .timer_mutation(TimerMutation::CancelRace {
                        fiber_id: timer.fiber_id(),
                        race_id: *race_id,
                        except: timer.timer_id(),
                    });
                if let Some(job_key) = job_key {
                    builder = builder.ack_job(job_key.clone());
                }
            } else {
                let next_fire_count = timer
                    .repeat_spec()
                    .map(|spec| spec.fired_count().saturating_add(1))
                    .unwrap_or(1);
                let child_id =
                    EffectId::for_timer_fire(timer.timer_id(), next_fire_count).as_uuid();
                builder = builder
                    .upsert_fiber(Fiber::new(child_id, Addr::from(*resume_at)))
                    .event(RuntimeEvent::BoundaryFired {
                        race_id: *race_id,
                        fiber_id: timer.fiber_id(),
                        spawned_fiber_id: child_id,
                        boundary_element_id: boundary_element_id.clone().unwrap_or_default(),
                        resume_at: Addr::from(*resume_at),
                    });
                if let Some(spec) = timer.repeat_spec() {
                    let remaining = spec.remaining().saturating_sub(1);
                    if remaining > 0 {
                        let next =
                            TimerRepeatSpec::new(spec.interval_ms(), remaining, next_fire_count);
                        let next_due_at = fired_at.saturating_add(spec.interval_ms());
                        fiber.wait = WaitState::Race {
                            race_id: *race_id,
                            timer_deadline_ms: Some(next_due_at),
                            job_key: job_key.clone(),
                            interrupting: false,
                            timer_arm_index: Some(*arm_index),
                            cycle_remaining: Some(next.remaining()),
                            cycle_fired_count: next.fired_count(),
                        };
                        builder = builder
                            .timer_mutation(TimerMutation::Rearm {
                                timer_id: timer.timer_id(),
                                claim_token: timer.claim_token(),
                                due_at: next_due_at,
                                repeat_spec: next.clone(),
                            })
                            .event(RuntimeEvent::TimerCycleIteration {
                                race_id: *race_id,
                                fiber_id: timer.fiber_id(),
                                iteration: next_fire_count,
                                remaining: next.remaining(),
                            });
                        rearmed = true;
                    } else {
                        builder = builder
                            .event(RuntimeEvent::TimerCycleIteration {
                                race_id: *race_id,
                                fiber_id: timer.fiber_id(),
                                iteration: next_fire_count,
                                remaining: 0,
                            })
                            .event(RuntimeEvent::TimerCycleExhausted {
                                race_id: *race_id,
                                fiber_id: timer.fiber_id(),
                                total_fired: next_fire_count,
                            });
                        set_post_cycle_wait(
                            &mut fiber,
                            job_key,
                            *race_id,
                            *arm_index,
                            next_fire_count,
                        );
                    }
                } else {
                    set_post_cycle_wait(&mut fiber, job_key, *race_id, *arm_index, next_fire_count);
                }
                builder = builder.upsert_fiber(fiber);
            }
        }
        (
            TimerKind::V2Race { record_id, resume_at },
            Some(mut fiber),
        ) if matches!(&fiber.wait, WaitState::V2Race { record_id: current, .. } if current == record_id) =>
        {
            fiber.pc = Addr::from(*resume_at);
            fiber.wait = WaitState::Running;
            builder = builder
                .upsert_fiber(fiber)
                .event(RuntimeEvent::TimerFired {
                    timer_id: timer.timer_id(),
                    fiber_id: timer.fiber_id(),
                    fired_at,
                })
                .event(RuntimeEvent::V2RaceWon {
                    record_id: *record_id,
                    fiber_id: timer.fiber_id(),
                })
                .concurrency_mutation(ConcurrencyMutation::Retire(*record_id))
                .timer_mutation(TimerMutation::V2CancelRace {
                    fiber_id: timer.fiber_id(),
                    record_id: *record_id,
                    except: timer.timer_id(),
                });
        }
        _ => {}
    }
    if !rearmed {
        builder = builder.timer_mutation(TimerMutation::Consume {
            timer_id: timer.timer_id(),
            claim_token: timer.claim_token(),
        });
    }
    Ok(builder.build())
}

fn set_post_cycle_wait(
    fiber: &mut Fiber,
    job_key: &Option<String>,
    race_id: u32,
    arm_index: usize,
    fire_count: u32,
) {
    fiber.wait = if let Some(job_key) = job_key {
        WaitState::Job {
            job_key: job_key.clone(),
        }
    } else {
        WaitState::Race {
            race_id,
            timer_deadline_ms: None,
            job_key: None,
            interrupting: false,
            timer_arm_index: Some(arm_index),
            cycle_remaining: Some(0),
            cycle_fired_count: fire_count,
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_types::{ArtifactEnvelope, session_stack::SessionStackState};
    use std::collections::BTreeMap;

    fn fixture() -> (ExecutableWorkflow, Snapshot, DeterministicContext) {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [7u8; 32],
            program: vec![Instr::Fork { targets: vec![Addr::new(1), Addr::new(2)].into() }, Instr::End, Instr::End],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "kernel-golden").unwrap(),
        )
        .unwrap();
        let instance_id = Uuid::from_u128(11);
        let fiber_id = Uuid::from_u128(12);
        let instance = ProcessInstance {
            instance_id,
            tenant_id: "tenant-a".to_string(),
            process_key: "golden".to_string(),
            bytecode_version: workflow.hash().into_bytes(),
            domain_payload: "{}".into(),
            domain_payload_hash: EffectId::content_hash(b"{}"),
            session_stack: SessionStackState::default(),
            flags: BTreeMap::new(),
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: "corr".to_string(),
            entry_id: Uuid::nil(),
            runbook_id: Uuid::nil(),
            created_at: 1,
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: None,
        };
        (
            workflow,
            Snapshot::new(instance, [Fiber::new(fiber_id, 0)]),
            DeterministicContext::new(100, Uuid::from_u128(13), 1),
        )
    }

    #[test]
    fn same_inputs_produce_byte_identical_transition() {
        let (workflow, snapshot, context) = fixture();
        let command = Command::Tick { fiber_id: None };
        let first = apply(&workflow, &snapshot, &command, &context).unwrap();
        let second = apply(&workflow, &snapshot, &command, &context).unwrap();
        assert_eq!(
            first.canonical_bytes().unwrap(),
            second.canonical_bytes().unwrap()
        );
    }

    #[test]
    fn apply_without_commit_replays_identically() {
        let (workflow, snapshot, context) = fixture();
        let command = Command::Tick { fiber_id: None };
        let before_crash = apply(&workflow, &snapshot, &command, &context).unwrap();
        let after_restart = apply(&workflow, &snapshot, &command, &context).unwrap();
        assert_eq!(
            before_crash.canonical_bytes().unwrap(),
            after_restart.canonical_bytes().unwrap()
        );
    }

    #[test]
    fn verified_fiber_limit_is_enforced_before_interpretation() {
        let (workflow, snapshot, context) = fixture();
        let fibers = (0..=workflow.envelope().limits().max_fibers())
            .map(|ordinal| Fiber::new(Uuid::from_u128(1_000 + u128::from(ordinal)), 0));
        let oversized = Snapshot::new(snapshot.instance().clone(), fibers);
        let error = apply(
            &workflow,
            &oversized,
            &Command::Tick { fiber_id: None },
            &context,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            TransitionError::ResourceLimitExceeded {
                resource: "fiber count",
                ..
            }
        ));
    }

    #[test]
    fn journal_replay_reconstructs_byte_identical_snapshot() {
        let (workflow, snapshot, context) = fixture();
        let command = Command::Tick { fiber_id: None };
        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );
        let transition = apply(&workflow, &snapshot, &command, &context).unwrap();
        let expected = materialize_snapshot(
            genesis.state(),
            &transition,
            workflow.envelope().abi_version(),
            1,
        );
        let record = JournalRecord::new(
            transition.command_envelope().unwrap().clone(),
            0,
            1,
            workflow.hash().into_bytes(),
            genesis.state_hash().unwrap(),
            expected.state_hash().unwrap(),
            transition.events(),
            transition.effects(),
        );

        let replayed = replay(&workflow, &genesis, &[record]).unwrap();
        assert_eq!(
            replayed.canonical_bytes().unwrap(),
            expected.canonical_bytes().unwrap()
        );
    }

    #[test]
    fn replay_state_hash_is_stable_across_one_hundred_runs() {
        let (workflow, snapshot, context) = fixture();
        let command = Command::Tick { fiber_id: None };
        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );
        let transition = apply(&workflow, &snapshot, &command, &context).unwrap();
        let expected = materialize_snapshot(
            genesis.state(),
            &transition,
            workflow.envelope().abi_version(),
            1,
        );
        let record = JournalRecord::new(
            transition.command_envelope().unwrap().clone(),
            0,
            1,
            workflow.hash().into_bytes(),
            genesis.state_hash().unwrap(),
            expected.state_hash().unwrap(),
            transition.events(),
            transition.effects(),
        );
        let expected_hash = expected.state_hash().unwrap();
        for _ in 0..100 {
            assert_eq!(
                replay(&workflow, &genesis, std::slice::from_ref(&record))
                    .unwrap()
                    .state_hash()
                    .unwrap(),
                expected_hash
            );
        }
    }

    #[test]
    fn journal_replay_rejects_state_hash_divergence() {
        let (workflow, snapshot, context) = fixture();
        let command = Command::Tick { fiber_id: None };
        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );
        let transition = apply(&workflow, &snapshot, &command, &context).unwrap();
        let record = JournalRecord::new(
            transition.command_envelope().unwrap().clone(),
            0,
            1,
            workflow.hash().into_bytes(),
            [0u8; 32],
            [0xA5; 32],
            transition.events(),
            transition.effects(),
        );

        assert!(matches!(
            replay(&workflow, &genesis, &[record]),
            Err(ReplayError::StateDivergence { revision: 1 })
        ));
    }

    #[test]
    fn payload_route_is_deterministic_and_missing_value_is_typed_error() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [8u8; 32],
            program: vec![
                Instr::RoutePayload {
                    branches: vec![
                        bpmn_lite_types::PayloadRouteBranch {
                            placeholder: "@kind".to_string(),
                            expected_value: "fund".to_string(),
                            target: Addr::new(1),
                        },
                        bpmn_lite_types::PayloadRouteBranch {
                            placeholder: "@kind".to_string(),
                            expected_value: "trust".to_string(),
                            target: Addr::new(2),
                        },
                    ]
                    .into(),
                    default_target: None,
                },
                Instr::End,
                Instr::EndTerminate,
            ],
            debug_map: BTreeMap::from([(Addr::new(0), "route".to_string())]),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "kernel-route").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let mut instance = base_snapshot.instance().clone();
        instance.domain_payload = r#"{"kind":"fund"}"#.into();
        instance.bind_placeholder_from_payload("@kind").unwrap();
        let snapshot = Snapshot::new(instance, base_snapshot.fibers().values().cloned());
        let transition = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: None },
            &context,
        )
        .unwrap();
        assert!(matches!(
            transition.next_snapshot().state,
            ProcessState::Completed { .. }
        ));

        let mut instance = base_snapshot.instance().clone();
        instance.domain_payload = r#"{"kind":"unknown"}"#.into();
        instance.bind_placeholder_from_payload("@kind").unwrap();
        let snapshot = Snapshot::new(instance, base_snapshot.fibers().values().cloned());
        assert!(matches!(
            apply(
                &workflow,
                &snapshot,
                &Command::Tick { fiber_id: None },
                &context,
            ),
            Err(TransitionError::RouteNotMatched(node)) if node == "route"
        ));
    }

    /// V4.1 core scope words (`V2Guard`/`V2GuardEnd`/`V2Fork`/`V2Join`)
    /// reproduce the guard-wrapping-a-fork/barrier/join shape at the heart
    /// of the locked oracle (`docs/todo/EOP-EX-BPMN-ISA-002.md`), including
    /// V&S v0.4's ruling B survivor semantics: last arrival continues in
    /// place, the other member is deleted at that same moment, not before.
    #[test]
    fn v2_guard_fork_join_reproduces_oracle_survivor_shape() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [9u8; 32],
            program: vec![
                /* 0 */ Instr::V2Guard { handler: Addr::new(8) },
                /* 1 */ Instr::V2Fork {
                    targets: Box::new([Addr::new(2), Addr::new(4)]),
                    pairing: Addr::new(1),
                },
                /* 2 */ Instr::V2Join { pairing: Addr::new(1) },
                /* 3 */ Instr::Jump { target: Addr::new(6) },
                /* 4 */ Instr::V2Join { pairing: Addr::new(1) },
                /* 5 */ Instr::Jump { target: Addr::new(6) },
                /* 6 */ Instr::V2GuardEnd,
                /* 7 */ Instr::End,
                /* 8 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-guard-fork-join").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let mut snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        // Tick 1: V2Guard then V2Fork run in the same tick (neither parks).
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
        assert_eq!(t1.fibers_delete(), &[root_fiber_id]);
        assert_eq!(t1.fibers_upsert().len(), 2);
        let (child_a, child_b) = (t1.fibers_upsert()[0].clone(), t1.fibers_upsert()[1].clone());
        assert_eq!(child_a.control_stack.len(), 2);
        assert_eq!(child_b.control_stack, child_a.control_stack);
        let guard_handle = child_a.control_stack[0];
        let barrier_handle = child_a.control_stack[1];
        assert_eq!(t1.concurrency_mutations().len(), 2);
        assert!(matches!(
            &t1.concurrency_mutations()[0],
            ConcurrencyMutation::Insert(record) if record.id == guard_handle
                && record.handler == Some(Addr::new(8))
        ));
        assert!(matches!(
            &t1.concurrency_mutations()[1],
            ConcurrencyMutation::Insert(record) if record.id == barrier_handle
                && record.counters.arity == 2
                && record.counters.count == 2
        ));
        assert_eq!(t1.control_stack_deltas().len(), 3);

        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        snapshot = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        // Tick 2: child_a arrives at V2Join first — not last, parks.
        let t2 = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: Some(child_a.fiber_id) },
            &context,
        )
        .unwrap();
        assert!(t2.fibers_delete().is_empty());
        assert_eq!(t2.fibers_upsert().len(), 1);
        assert!(matches!(
            t2.fibers_upsert()[0].wait,
            WaitState::V2Barrier { record_id } if record_id == barrier_handle
        ));
        assert_eq!(
            t2.control_stack_deltas().len(),
            1,
            "non-last arrival still pops its own control stack this transition"
        );
        assert!(matches!(
            &t2.concurrency_mutations()[0],
            ConcurrencyMutation::Insert(record) if record.id == barrier_handle
                && record.counters.count == 1
        ));

        let after_t2 = materialize_snapshot(after_t1.state(), &t2, workflow.envelope().abi_version(), 2);
        snapshot = Snapshot::new(
            after_t2.state().instance().clone(),
            after_t2.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t2.state().concurrency_table().clone());

        // Tick 3: child_b is the last arrival — sole survivor, continues
        // through V2GuardEnd to End in the same transition; child_a
        // (parked, non-last) is cancelled now, not before (ruling B).
        let t3 = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: Some(child_b.fiber_id) },
            &context,
        )
        .unwrap();
        assert_eq!(t3.fibers_delete(), &[child_a.fiber_id, child_b.fiber_id]);
        assert!(
            t3.fibers_upsert().is_empty(),
            "the survivor is deleted too — it runs straight through GuardEnd to End, not parked"
        );
        assert_eq!(
            t3.concurrency_mutations().len(),
            2,
            "Retire(BAR) then Retire(G)"
        );
        assert!(matches!(
            &t3.concurrency_mutations()[0],
            ConcurrencyMutation::Retire(id) if *id == barrier_handle
        ));
        assert!(matches!(
            &t3.concurrency_mutations()[1],
            ConcurrencyMutation::Retire(id) if *id == guard_handle
        ));
        assert_eq!(
            t3.control_stack_deltas().len(),
            2,
            "child_b's own Pop(BAR) then Pop(G) — child_a's stack was already fully popped in tick 2"
        );
    }

    /// V4.1 race words (`V2RaceOpen`/`V2ArmTimer`/`V2ArmMsg`/`V2RaceClose`)
    /// reproduce the oracle's Scenario 1 message-wins shape: both a timer
    /// and a message alternative are armed, the message wins, the race
    /// record retires, and the timer arm's durable effect is cancelled via
    /// `TimerMutation::V2CancelRace`.
    #[test]
    fn v2_race_message_arm_wins_and_cancels_timer_arm() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [10u8; 32],
            program: vec![
                /* 0 */ Instr::V2RaceOpen { arm_count: 2 },
                /* 1 */ Instr::PushI64(30_000),
                /* 2 */ Instr::V2ArmTimer { target: Addr::new(5) },
                /* 3 */ Instr::V2ArmMsg { target: Addr::new(6), name: 100, corr_reg: 0 },
                /* 4 */ Instr::V2RaceClose,
                /* 5 */ Instr::Jump { target: Addr::new(7) },
                /* 6 */ Instr::Jump { target: Addr::new(7) },
                /* 7 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-race-msg-wins").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        // Tick 1: open, arm timer, arm msg, close — all in one tick, only
        // the close parks.
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
        assert!(t1.fibers_delete().is_empty());
        assert_eq!(t1.fibers_upsert().len(), 1);
        let parked = t1.fibers_upsert()[0].clone();
        let record_id = match &parked.wait {
            WaitState::V2Race { record_id, arms } => {
                assert_eq!(arms.len(), 2);
                *record_id
            }
            other => panic!("expected V2Race, got {other:?}"),
        };
        assert_eq!(t1.effects().len(), 1, "V2ArmTimer schedules one timer effect");
        assert_eq!(t1.concurrency_mutations().len(), 1);
        assert!(matches!(
            &t1.concurrency_mutations()[0],
            ConcurrencyMutation::Insert(record) if record.id == record_id
                && record.counters.arity == 2
        ));

        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        let snapshot2 = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        // The message arm wins — correlation register 0 defaults to
        // Value::Bool(false), so "b:false" is the matching correlation key.
        let message_command = Command::MessageDelivered {
            message_id: "m1".to_string(),
            name: "100".to_string(),
            correlation_key: "b:false".to_string(),
            payload: b"{}".to_vec(),
            payload_hash: None,
            expires_at: 0,
        };
        let t2 = apply(&workflow, &snapshot2, &message_command, &context).unwrap();
        assert_eq!(t2.fibers_upsert().len(), 1);
        let resumed = &t2.fibers_upsert()[0];
        assert_eq!(resumed.pc, Addr::new(6), "message arm's own target");
        assert_eq!(resumed.wait, WaitState::Running);
        assert_eq!(t2.concurrency_mutations().len(), 1);
        assert!(matches!(
            &t2.concurrency_mutations()[0],
            ConcurrencyMutation::Retire(id) if *id == record_id
        ));
        assert_eq!(t2.timer_mutations().len(), 1);
        assert!(matches!(
            &t2.timer_mutations()[0],
            TimerMutation::V2CancelRace { record_id: id, .. } if *id == record_id
        ));
    }

    /// `V2ArmEffect` winning a race: arms a timer and an FFI effect, the
    /// effect completes first, the race retires, the timer arm's durable
    /// effect is cancelled — same shape as
    /// `v2_race_message_arm_wins_and_cancels_timer_arm`, but resolved
    /// through `apply_ffi_completion`'s `WaitState::V2Race` branch instead
    /// of `apply_message`.
    #[test]
    fn v2_race_effect_arm_wins_and_cancels_timer_arm() {
        use bpmn_lite_types::ffi_bindings::FfiTaskDecl;

        let template_id = [21u8; 32];
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [17u8; 32],
            program: vec![
                /* 0 */ Instr::V2RaceOpen { arm_count: 2 },
                /* 1 */ Instr::PushI64(30_000),
                /* 2 */ Instr::V2ArmTimer { target: Addr::new(5) },
                /* 3 */ Instr::V2ArmEffect { target: Addr::new(6), template_id, argc: 0, retc: 0 },
                /* 4 */ Instr::V2RaceClose,
                /* 5 */ Instr::Jump { target: Addr::new(7) },
                /* 6 */ Instr::Jump { target: Addr::new(7) },
                /* 7 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        }
        .with_v2_ffi_task_decls(BTreeMap::from([(
            Addr::new(3),
            FfiTaskDecl {
                template_id,
                inputs: vec![],
                outputs: vec![],
            },
        )]));
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-race-effect-wins").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
        assert_eq!(t1.fibers_upsert().len(), 1);
        let parked = t1.fibers_upsert()[0].clone();
        let (record_id, effect_id) = match &parked.wait {
            WaitState::V2Race { record_id, arms } => {
                assert_eq!(arms.len(), 2);
                let effect_id = arms
                    .iter()
                    .find_map(|arm| match arm {
                        bpmn_lite_types::V2RaceArm::Effect { effect_id, .. } => Some(*effect_id),
                        _ => None,
                    })
                    .expect("effect arm present");
                (*record_id, effect_id)
            }
            other => panic!("expected V2Race, got {other:?}"),
        };
        assert_eq!(t1.effects().len(), 2, "one ScheduleTimer, one Invoke");

        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        let snapshot2 = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        let t2 = apply(
            &workflow,
            &snapshot2,
            &Command::EffectCompleted {
                effect_id,
                output: EffectOutput::Ffi {
                    fiber_id: root_fiber_id,
                    pc: 3,
                    output_payload: b"{}".to_vec(),
                    new_domain_payload: None,
                    no_match: false,
                },
            },
            &context,
        )
        .unwrap();
        assert_eq!(t2.fibers_upsert().len(), 1);
        let resumed = &t2.fibers_upsert()[0];
        assert_eq!(resumed.pc, Addr::new(6), "effect arm's own target");
        assert_eq!(resumed.wait, WaitState::Running);
        assert_eq!(t2.concurrency_mutations().len(), 1);
        assert!(matches!(
            &t2.concurrency_mutations()[0],
            ConcurrencyMutation::Retire(id) if *id == record_id
        ));
        assert_eq!(t2.timer_mutations().len(), 1);
        assert!(matches!(
            &t2.timer_mutations()[0],
            TimerMutation::V2CancelRace { record_id: id, .. } if *id == record_id
        ));
        assert!(matches!(
            t2.events().iter().find(|e| matches!(e, RuntimeEvent::V2RaceWon { .. })),
            Some(RuntimeEvent::V2RaceWon { record_id: id, .. }) if *id == record_id
        ));
    }

    /// Late `EffectCompleted` for a race's losing `V2ArmEffect` arm — the
    /// timer arm wins first (record retires), and the effect's own
    /// completion arrives afterward. Not corruption: `apply_timer`'s
    /// `TimerKind::V2Race` branch already tolerates this shape of lateness
    /// as a silent no-op when `fiber.wait` no longer matches; this proves
    /// `apply_ffi_completion`'s `WaitState::V2Race` branch does the same —
    /// no fibre mutation, no error, just the effect's own terminal mutation.
    #[test]
    fn v2_race_late_effect_completion_after_timer_arm_already_won_is_a_no_op() {
        use bpmn_lite_types::ffi_bindings::FfiTaskDecl;

        let template_id = [22u8; 32];
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [18u8; 32],
            program: vec![
                /* 0 */ Instr::V2RaceOpen { arm_count: 2 },
                /* 1 */ Instr::PushI64(30_000),
                /* 2 */ Instr::V2ArmTimer { target: Addr::new(5) },
                /* 3 */ Instr::V2ArmEffect { target: Addr::new(6), template_id, argc: 0, retc: 0 },
                /* 4 */ Instr::V2RaceClose,
                /* 5 */ Instr::Jump { target: Addr::new(7) },
                /* 6 */ Instr::Jump { target: Addr::new(7) },
                /* 7 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        }
        .with_v2_ffi_task_decls(BTreeMap::from([(
            Addr::new(3),
            FfiTaskDecl {
                template_id,
                inputs: vec![],
                outputs: vec![],
            },
        )]));
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-race-late-effect").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
        let parked = t1.fibers_upsert()[0].clone();
        let (record_id, effect_id) = match &parked.wait {
            WaitState::V2Race { record_id, arms } => {
                let effect_id = arms
                    .iter()
                    .find_map(|arm| match arm {
                        bpmn_lite_types::V2RaceArm::Effect { effect_id, .. } => Some(*effect_id),
                        _ => None,
                    })
                    .expect("effect arm present");
                (*record_id, effect_id)
            }
            other => panic!("expected V2Race, got {other:?}"),
        };

        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        let snapshot2 = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        // The timer arm wins first.
        let claimed_timer = bpmn_lite_types::ClaimedTimer::new(
            bpmn_lite_types::ClaimedTimerIdentity::new(
                bpmn_lite_types::TenantId::new("tenant-a").unwrap(),
                EffectId::for_instruction(root_fiber_id, root_fiber_id, 2),
                after_t1.state().instance().instance_id,
                root_fiber_id,
            ),
            30_000,
            TimerKind::V2Race { record_id, resume_at: Addr::new(5).into() },
            None,
            Uuid::nil(),
        );
        let t2 = apply(
            &workflow,
            &snapshot2,
            &Command::TimerFired { timer: claimed_timer, fired_at: 30_000 },
            &context,
        )
        .unwrap();
        assert_eq!(t2.fibers_upsert()[0].pc, Addr::new(5), "timer arm's own target");
        let after_t2 = materialize_snapshot(after_t1.state(), &t2, workflow.envelope().abi_version(), 2);
        let snapshot3 = Snapshot::new(
            after_t2.state().instance().clone(),
            after_t2.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t2.state().concurrency_table().clone());

        // The effect's own completion arrives late — the record is already
        // retired and the fibre already resumed via the timer arm.
        let t3 = apply(
            &workflow,
            &snapshot3,
            &Command::EffectCompleted {
                effect_id,
                output: EffectOutput::Ffi {
                    fiber_id: root_fiber_id,
                    pc: 3,
                    output_payload: b"{}".to_vec(),
                    new_domain_payload: None,
                    no_match: false,
                },
            },
            &context,
        )
        .unwrap();
        assert!(t3.fibers_upsert().is_empty(), "no fibre mutation for a late-arriving losing arm");
        assert!(t3.concurrency_mutations().is_empty());
        assert!(t3.timer_mutations().is_empty());
        assert_eq!(t3.effect_mutations().len(), 1, "still terminates the effect itself");
    }

    /// V4.1's guard-trigger command (`Command::V2TriggerGuard`) reproduces
    /// the locked oracle's Scenario 2 cancellation cascade
    /// (`docs/todo/EOP-EX-BPMN-ISA-002.md` §3): neither branch has
    /// reached its `JOIN`, so ruling A's record-nesting order applies —
    /// `RACE` retires before `BAR` before `G`, `F2` (RACE's own member)
    /// is cancelled with it, `F1` (BAR's direct member) is cancelled when
    /// `BAR` retires, and the handler spawns with the pre-push (empty)
    /// control stack. This is the exact 18-instruction oracle program
    /// (`ex_oracle_draft_v2_program_from_eop_ex_doc_is_admitted`'s
    /// fixture), both branches executed for real.
    #[test]
    fn v2_trigger_guard_reproduces_oracle_cancellation_cascade() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [11u8; 32],
            program: vec![
                /* 0  */ Instr::V2Guard { handler: Addr::new(16) },
                /* 1  */ Instr::V2Fork {
                    targets: Box::new([Addr::new(2), Addr::new(6)]),
                    pairing: Addr::new(1),
                },
                /* 2  */ Instr::PushI64(60_000),
                /* 3  */ Instr::V2WaitFor,
                /* 4  */ Instr::V2Join { pairing: Addr::new(1) },
                /* 5  */ Instr::Jump { target: Addr::new(14) },
                /* 6  */ Instr::V2RaceOpen { arm_count: 2 },
                /* 7  */ Instr::PushI64(30_000),
                /* 8  */ Instr::V2ArmTimer { target: Addr::new(11) },
                /* 9  */ Instr::V2ArmMsg { target: Addr::new(12), name: 100, corr_reg: 0 },
                /* 10 */ Instr::V2RaceClose,
                /* 11 */ Instr::Jump { target: Addr::new(13) },
                /* 12 */ Instr::Jump { target: Addr::new(13) },
                /* 13 */ Instr::V2Join { pairing: Addr::new(1) },
                /* 14 */ Instr::V2GuardEnd,
                /* 15 */ Instr::End,
                /* 16 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                /* 17 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["NotifyCancelled".to_string()],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-trigger-guard-cascade").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );

        // Each tick gets its own context (distinct command_id), matching
        // real dispatch — reusing one context across ticks would collide
        // `derived_id`'s ordinal-0 output between V2Guard's and
        // V2RaceOpen's own record-minting calls.
        let context1 = DeterministicContext::new(100, Uuid::from_u128(101), 1);

        // Tick 1: V2Guard + V2Fork — spawns F1 (branch A, target 2) and F2
        // (branch B, target 6), both inheriting [G, BAR].
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let (f1, f2) = (t1.fibers_upsert()[0].clone(), t1.fibers_upsert()[1].clone());
        let guard_handle = f1.control_stack[0];
        let barrier_handle = f1.control_stack[1];
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);

        // F1 runs 2 (PushI64) -> 3 (V2WaitFor) for real, parking on
        // WaitState::Timer.
        let context1b = DeterministicContext::new(100, Uuid::from_u128(1015), 1);
        let t1b = apply(
            &workflow,
            &Snapshot::new(after_t1.state().instance().clone(), after_t1.state().fibers().values().cloned())
                .with_concurrency_table(after_t1.state().concurrency_table().clone()),
            &Command::Tick { fiber_id: Some(f1.fiber_id) },
            &context1b,
        )
        .unwrap();
        assert!(matches!(
            t1b.fibers_upsert()[0].wait,
            WaitState::Timer { .. }
        ));
        let after_t1b = materialize_snapshot(after_t1.state(), &t1b, workflow.envelope().abi_version(), 1);
        let fibers_after_fork: Vec<Fiber> = after_t1b.state().fibers().values().cloned().collect();
        let snapshot_after_fork = Snapshot::new(after_t1b.state().instance().clone(), fibers_after_fork)
            .with_concurrency_table(after_t1b.state().concurrency_table().clone());

        // Tick 2: F2 runs V2RaceOpen..V2RaceClose for real — parks on
        // WaitState::V2Race, popping RACE off its own control stack.
        let context2 = DeterministicContext::new(101, Uuid::from_u128(102), 2);
        let t2 = apply(
            &workflow,
            &snapshot_after_fork,
            &Command::Tick { fiber_id: Some(f2.fiber_id) },
            &context2,
        )
        .unwrap();
        let race_handle = match &t2.fibers_upsert()[0].wait {
            WaitState::V2Race { record_id, .. } => *record_id,
            other => panic!("expected V2Race, got {other:?}"),
        };
        let after_t2 = materialize_snapshot(
            after_t1b.state(),
            &t2,
            workflow.envelope().abi_version(),
            2,
        );
        let snapshot_before_trigger = Snapshot::new(
            after_t2.state().instance().clone(),
            after_t2.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t2.state().concurrency_table().clone());
        assert_eq!(snapshot_before_trigger.fibers().len(), 2, "F1 and F2 both still live");

        // Trigger: neither branch has reached its JOIN.
        let context3 = DeterministicContext::new(102, Uuid::from_u128(103), 3);
        let trigger = apply(
            &workflow,
            &snapshot_before_trigger,
            &Command::V2TriggerGuard { record_id: guard_handle },
            &context3,
        )
        .unwrap();

        assert_eq!(
            trigger.concurrency_mutations().len(),
            3,
            "Retire(RACE), Retire(BAR), Retire(G) — ruling A order"
        );
        assert!(matches!(
            &trigger.concurrency_mutations()[0],
            ConcurrencyMutation::Retire(id) if *id == race_handle
        ));
        assert!(matches!(
            &trigger.concurrency_mutations()[1],
            ConcurrencyMutation::Retire(id) if *id == barrier_handle
        ));
        assert!(matches!(
            &trigger.concurrency_mutations()[2],
            ConcurrencyMutation::Retire(id) if *id == guard_handle
        ));

        assert_eq!(
            trigger.fibers_delete(),
            &[f2.fiber_id, f1.fiber_id],
            "F2 (RACE's own member) cancelled with the deeper record, F1 (BAR's direct member) with BAR"
        );

        assert_eq!(trigger.fibers_upsert().len(), 1, "the handler fibre spawns");
        let handler = &trigger.fibers_upsert()[0];
        assert_eq!(handler.pc, Addr::new(16));
        assert!(
            handler.control_stack.is_empty(),
            "V-4 pre-push: G was opened on the root fibre's empty control stack"
        );
    }

    /// V4.1 `V2CancelScope` (Adam-ratified V&S §10 Q4: reuses the
    /// compensation op — restore the scope's rollback snapshot, unwind
    /// nested members, no handler). `V2Guard` captures `domain_payload`
    /// at open as a standard lifecycle snapshot; something mutates it
    /// while the fibre is parked inside the scope (simulated here, as a
    /// job completion or message delivery would in real use);
    /// `V2CancelScope` must restore the pre-scope value, not the
    /// mutated one, and the cancelling fibre continues in place rather
    /// than being deleted (unlike `V2TriggerGuard`'s external-fire path).
    #[test]
    fn v2_cancel_scope_restores_rollback_snapshot_and_continues_in_place() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [12u8; 32],
            program: vec![
                /* 0 */ Instr::V2Guard { handler: Addr::new(6) },
                /* 1 */ Instr::PushI64(1_000),
                /* 2 */ Instr::V2WaitFor,
                /* 3 */ Instr::V2CancelScope,
                /* 4 */ Instr::End,
                /* 5 */ Instr::End,
                /* 6 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-cancel-scope").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let original_payload = base_snapshot.instance().domain_payload.to_string();
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );

        // Tick 1: V2Guard (captures the rollback snapshot) -> PushI64 ->
        // V2WaitFor (parks; pc already past V2WaitFor by the time it parks).
        let context1 = DeterministicContext::new(200, Uuid::from_u128(201), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let guard_handle = t1.fibers_upsert()[0].control_stack[0];
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        assert!(matches!(
            after_t1.state().fibers().get(&root_fiber_id).unwrap().wait,
            WaitState::Timer { .. }
        ));

        // Resolve the parked timer first — WaitState::Timer only accepts
        // Command::Tick once it's back to WaitState::Running.
        let context1b = DeterministicContext::new(200, Uuid::from_u128(2015), 1);
        let claimed_timer = bpmn_lite_types::ClaimedTimer::new(
            bpmn_lite_types::ClaimedTimerIdentity::new(
                bpmn_lite_types::TenantId::new("tenant-a").unwrap(),
                EffectId::for_instruction(root_fiber_id, root_fiber_id, 2),
                after_t1.state().instance().instance_id,
                root_fiber_id,
            ),
            1_200,
            TimerKind::Wait,
            None,
            Uuid::nil(),
        );
        let t1c = apply(
            &workflow,
            &Snapshot::new(after_t1.state().instance().clone(), after_t1.state().fibers().values().cloned())
                .with_concurrency_table(after_t1.state().concurrency_table().clone()),
            &Command::TimerFired { timer: claimed_timer, fired_at: 1_200 },
            &context1b,
        )
        .unwrap();
        assert_eq!(t1c.fibers_upsert()[0].wait, WaitState::Running);
        let after_t1c = materialize_snapshot(
            after_t1.state(),
            &t1c,
            workflow.envelope().abi_version(),
            1,
        );

        // Something mutates domain_payload while the fibre is running
        // again but still inside the scope (a job completion or message
        // delivery would do this for real; simulated directly here since
        // only the *shape* — payload changed after scope-open — matters).
        let mut resumed_instance = after_t1c.state().instance().clone();
        resumed_instance.domain_payload = "mutated-while-parked".to_string().into();
        resumed_instance.domain_payload_hash = EffectId::content_hash(b"mutated-while-parked");
        let snapshot_running = Snapshot::new(resumed_instance, after_t1c.state().fibers().values().cloned())
            .with_concurrency_table(after_t1c.state().concurrency_table().clone());

        // Tick 2: fibre resumes at V2CancelScope (pc already past
        // V2WaitFor) — restores domain_payload, retires G, continues to
        // End in the same transition (not deleted by V2CancelScope itself).
        let context2 = DeterministicContext::new(201, Uuid::from_u128(202), 2);
        let t2 = apply(
            &workflow,
            &snapshot_running,
            &Command::Tick { fiber_id: Some(root_fiber_id) },
            &context2,
        )
        .unwrap();

        assert_eq!(
            t2.next_snapshot().domain_payload.to_string(),
            original_payload,
            "V2CancelScope must restore the pre-scope snapshot, not the mutated value"
        );
        assert_eq!(t2.concurrency_mutations().len(), 1);
        assert!(matches!(
            &t2.concurrency_mutations()[0],
            ConcurrencyMutation::Retire(id) if *id == guard_handle
        ));
        assert_eq!(
            t2.fibers_delete(),
            &[root_fiber_id],
            "the fibre reaches End in the same transition — deleted by End, not V2CancelScope"
        );
        assert!(t2.fibers_upsert().is_empty());
    }

    /// V4.1 automatic rollback-on-fail (Adam-ratified): a *definitive*
    /// job failure (no retry token, no error_route_map match — the exact
    /// point v1 would otherwise create an `Incident`) for a fibre inside
    /// an armed interrupting `V2Guard` scope bypasses the incident path
    /// entirely, restores the scope's rollback snapshot, and kills the
    /// fibre rather than continuing or auto-respawning. Outside any
    /// guard scope, the existing v1 incident path is unchanged — proven
    /// by running the identical failure both inside and outside a guard.
    #[test]
    fn definitive_job_failure_inside_interrupting_guard_rolls_back_instead_of_incident() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [13u8; 32],
            program: vec![
                /* 0 */ Instr::V2Guard { handler: Addr::new(4) },
                /* 1 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                /* 2 */ Instr::V2GuardEnd,
                /* 3 */ Instr::End,
                /* 4 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["SomeTask".to_string()],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-rollback-on-fail").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let original_payload = base_snapshot.instance().domain_payload.to_string();
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );

        // Tick 1: V2Guard (captures snapshot) -> ExecNative parks the
        // fibre on WaitState::Job.
        let context1 = DeterministicContext::new(300, Uuid::from_u128(301), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let guard_handle = t1.fibers_upsert()[0].control_stack[0];
        let job_key = t1.jobs_enqueue()[0].job_key.clone();
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        let snapshot_running = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        // A definitive (non-retriable) failure — mutating domain_payload
        // beforehand is not needed to prove the point, but a mutated
        // instance makes the rollback observable rather than a no-op.
        let mut mutated_snapshot = snapshot_running.clone();
        let mut mutated_instance = mutated_snapshot.instance().clone();
        mutated_instance.domain_payload = "mutated-before-fail".to_string().into();
        mutated_instance.domain_payload_hash = EffectId::content_hash(b"mutated-before-fail");
        mutated_snapshot = Snapshot::new(mutated_instance, mutated_snapshot.fibers().values().cloned())
            .with_concurrency_table(mutated_snapshot.concurrency_table().clone());

        let context2 = DeterministicContext::new(301, Uuid::from_u128(302), 2);
        let fail_command = Command::EffectFailed {
            effect_id: EffectId::for_instruction(Uuid::nil(), Uuid::nil(), 0),
            job_key: job_key.clone(),
            error_class: ErrorClass::ContractViolation,
            message: "boom".to_string(),
            retry: None,
        };
        let t2 = apply(&workflow, &mutated_snapshot, &fail_command, &context2).unwrap();

        assert_eq!(
            t2.next_snapshot().domain_payload.to_string(),
            original_payload,
            "rollback must restore the pre-scope snapshot, not the mutated value"
        );
        assert!(
            t2.incidents().is_empty(),
            "the v1 incident path must not fire inside an interrupting guard scope"
        );
        assert_eq!(t2.fibers_delete(), &[root_fiber_id], "the failing fibre is killed, not continued");
        assert!(t2.fibers_upsert().is_empty(), "no auto-respawn");
        assert_eq!(t2.concurrency_mutations().len(), 1);
        assert!(matches!(
            &t2.concurrency_mutations()[0],
            ConcurrencyMutation::Retire(id) if *id == guard_handle
        ));

        // Same failure, no enclosing guard: today's v1 incident path is
        // unchanged.
        let unguarded_program = bpmn_lite_types::legacy_program! {
            bytecode_version: [14u8; 32],
            program: vec![
                Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["SomeTask".to_string()],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let unguarded_workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(unguarded_program, "v4-rollback-unguarded-control").unwrap(),
        )
        .unwrap();
        let unguarded_root = Uuid::from_u128(9_999);
        let unguarded_snapshot = Snapshot::new(
            base_snapshot.instance().clone(),
            [Fiber::new(unguarded_root, 0)],
        );
        let context3 = DeterministicContext::new(300, Uuid::from_u128(303), 1);
        let ut1 = apply(
            &unguarded_workflow,
            &unguarded_snapshot,
            &Command::Tick { fiber_id: None },
            &context3,
        )
        .unwrap();
        let unguarded_job_key = ut1.jobs_enqueue()[0].job_key.clone();
        let after_ut1 = materialize_snapshot(
            &PersistedSnapshotState::new(
                unguarded_snapshot.instance().clone(),
                unguarded_snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
            &ut1,
            unguarded_workflow.envelope().abi_version(),
            1,
        );
        let context4 = DeterministicContext::new(301, Uuid::from_u128(304), 2);
        let ut2 = apply(
            &unguarded_workflow,
            &Snapshot::new(after_ut1.state().instance().clone(), after_ut1.state().fibers().values().cloned()),
            &Command::EffectFailed {
                effect_id: EffectId::for_instruction(Uuid::nil(), Uuid::nil(), 0),
                job_key: unguarded_job_key,
                error_class: ErrorClass::ContractViolation,
                message: "boom".to_string(),
                retry: None,
            },
            &context4,
        )
        .unwrap();
        assert_eq!(ut2.incidents().len(), 1, "unchanged outside a guard scope: definitive failure still creates an Incident");
    }

    /// V4.1 `V2GuardN`'s trigger path (Q2 ratified, V&S §13 amendment
    /// v0.5 ruling A): fires the handler *without* cancelling anything,
    /// and the record re-arms — proven by triggering the same guard
    /// twice, both succeeding, with the member fibre never touched.
    #[test]
    fn v2_guardn_trigger_spawns_handler_without_unwinding_and_rearms() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [15u8; 32],
            program: vec![
                /* 0 */ Instr::V2GuardN { handler: Addr::new(5) },
                /* 1 */ Instr::PushI64(1_000),
                /* 2 */ Instr::V2WaitFor,
                /* 3 */ Instr::V2GuardNEnd,
                /* 4 */ Instr::End,
                // handler: post-push entry — pops the still-armed
                // GuardN token (v2_verifier.rs's
                // v4_guardn_handler_entry_state_is_post_push_inherits_own_token).
                /* 5 */ Instr::V2GuardNEnd,
                /* 6 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-guardn-trigger").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let genesis = SnapshotEnvelope::new(
            workflow.envelope().abi_version(),
            snapshot.instance().bytecode_version,
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );

        // Tick 1: V2GuardN opens -> PushI64 -> V2WaitFor parks.
        let context1 = DeterministicContext::new(400, Uuid::from_u128(401), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let guard_handle = t1.fibers_upsert()[0].control_stack[0];
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        assert!(matches!(
            after_t1.state().fibers().get(&root_fiber_id).unwrap().wait,
            WaitState::Timer { .. }
        ));
        let snapshot_running = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        // Trigger 1.
        let context2 = DeterministicContext::new(401, Uuid::from_u128(402), 2);
        let t2 = apply(
            &workflow,
            &snapshot_running,
            &Command::V2TriggerGuard { record_id: guard_handle },
            &context2,
        )
        .unwrap();
        assert!(t2.fibers_delete().is_empty(), "GuardN unwinds nothing");
        assert_eq!(t2.fibers_upsert().len(), 1, "only the new handler fibre");
        assert_eq!(t2.fibers_upsert()[0].pc, Addr::new(5));
        assert_eq!(
            t2.fibers_upsert()[0].control_stack,
            vec![guard_handle],
            "post-push: the handler inherits the still-armed GuardN token"
        );
        assert!(t2.concurrency_mutations().is_empty(), "record re-arms — nothing to mutate, it never changed");
        assert!(matches!(
            t2.events().iter().find(|e| matches!(e, RuntimeEvent::V2GuardNTriggered { .. })),
            Some(RuntimeEvent::V2GuardNTriggered { record_id, .. }) if *record_id == guard_handle
        ));
        let after_t2 = materialize_snapshot(after_t1.state(), &t2, workflow.envelope().abi_version(), 2);

        // The original parked member fibre is still there, untouched.
        assert!(after_t2.state().fibers().contains_key(&root_fiber_id));
        assert_eq!(after_t2.state().fibers().len(), 2, "member fibre + first handler fibre");

        // Trigger 2 — the record is still Armed, so this must succeed too.
        let snapshot_after_t2 = Snapshot::new(
            after_t2.state().instance().clone(),
            after_t2.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t2.state().concurrency_table().clone());
        let context3 = DeterministicContext::new(402, Uuid::from_u128(403), 3);
        let t3 = apply(
            &workflow,
            &snapshot_after_t2,
            &Command::V2TriggerGuard { record_id: guard_handle },
            &context3,
        )
        .unwrap();
        assert_eq!(t3.fibers_upsert().len(), 1, "second handler fibre spawns fine — the guard re-armed");
    }

    /// `V2AwaitEffect` end-to-end: arm the invocation, complete it, resume.
    /// Proves the v2/v1 split in `apply_ffi_completion` — the same
    /// `WaitState::Effect`/`EffectOutput::Ffi` shape `ExecFfi` uses, but
    /// resolved against `v2_ffi_task_decls` because the parked instruction
    /// at `pc` is `V2AwaitEffect`, not `ExecFfi`.
    #[test]
    fn v2_await_effect_invokes_and_resumes_via_shared_effect_completion() {
        use bpmn_lite_types::ffi_bindings::FfiTaskDecl;

        let template_id = [9u8; 32];
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [16u8; 32],
            program: vec![
                /* 0 */ Instr::V2AwaitEffect { template_id, argc: 0, retc: 0 },
                /* 1 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        }
        .with_v2_ffi_task_decls(BTreeMap::from([(
            Addr::new(0),
            FfiTaskDecl {
                template_id,
                inputs: vec![],
                outputs: vec![],
            },
        )]));
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-await-effect").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let context1 = DeterministicContext::new(500, Uuid::from_u128(501), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        assert_eq!(t1.effects().len(), 1, "V2AwaitEffect emits one DurableEffect::Invoke");
        let DurableEffect::Invoke { effect_id, .. } = &t1.effects()[0] else {
            panic!("expected DurableEffect::Invoke");
        };
        let effect_id = *effect_id;
        assert_eq!(t1.fibers_upsert().len(), 1);
        assert!(matches!(
            t1.fibers_upsert()[0].wait,
            WaitState::Effect { effect_id: parked } if parked == effect_id
        ));

        let snapshot_parked = Snapshot::new(
            snapshot.instance().clone(),
            t1.fibers_upsert().iter().cloned(),
        );
        let context2 = DeterministicContext::new(501, Uuid::from_u128(502), 2);
        let t2 = apply(
            &workflow,
            &snapshot_parked,
            &Command::EffectCompleted {
                effect_id,
                output: EffectOutput::Ffi {
                    fiber_id: root_fiber_id,
                    pc: 0,
                    output_payload: b"{}".to_vec(),
                    new_domain_payload: None,
                    no_match: false,
                },
            },
            &context2,
        )
        .unwrap();
        assert_eq!(t2.fibers_upsert().len(), 1);
        assert_eq!(t2.fibers_upsert()[0].pc, Addr::new(1), "resumed past the await");
        assert_eq!(t2.fibers_upsert()[0].wait, WaitState::Running);
        assert!(matches!(
            t2.events().iter().find(|e| matches!(e, RuntimeEvent::FfiInvocationCompleted { .. })),
            Some(RuntimeEvent::FfiInvocationCompleted { outcome_kind, .. }) if outcome_kind == "success"
        ));
    }
}
