#![forbid(unsafe_code)]

use bpmn_lite_types::ffi_bindings::{apply_ffi_outputs, encode_ffi_inputs};
use bpmn_lite_types::{
    Addr, BufferedMessageMutation, Command, CommandEnvelope, DedupeWrite, DurableEffect, EffectId,
    EffectMutation, EffectOutput, EffectTerminalState, ErrorClass, ExecutableWorkflow, Fiber,
    Incident, Instr, JobActivation, JobMutation, JoinId, JoinMutation, JournalCommand,
    JournalRecord, PersistedSnapshotState, ProcessInstance, ProcessState, RuntimeEvent, Snapshot,
    SnapshotEnvelope, TerminalCleanup, TimerKind, TimerMutation, TimerRepeatSpec, Transition,
    TransitionBuilder, Uuid, Value, WaitArm, WaitState,
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
    if transition.terminal_cleanup().delete_all_fibers() {
        fibers.clear();
    }
    if transition.terminal_cleanup().delete_all_joins() {
        joins.clear();
    }
    SnapshotEnvelope::new(
        artifact_abi,
        revision,
        PersistedSnapshotState::new(
            instance,
            fibers.into_values(),
            joins,
            incidents.into_values(),
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
    if fiber.pc != Addr::from(*pc)
        || !matches!(fiber.wait, WaitState::Effect { effect_id: parked } if parked == *effect_id)
    {
        return Err(TransitionError::InvalidCommand(
            "FFI completion does not match parked effect",
        ));
    }
    let mut instance = snapshot.instance().clone();
    if !*no_match {
        if let Some(payload) = new_domain_payload.as_deref() {
            instance.domain_payload = payload.to_string().into();
            instance.domain_payload_hash = EffectId::content_hash(payload.as_bytes());
        } else {
            let declaration = workflow
                .envelope()
                .metadata()
                .ffi_task_decls()
                .get(&Addr::from(*pc))
                .ok_or(TransitionError::MissingMetadata("FFI task declaration"))?;
            apply_ffi_outputs(&mut instance, declaration, output_payload)
                .map_err(|_| TransitionError::InvalidCommand("FFI output contract violation"))?;
        }
    }
    fiber.pc = fiber.pc.saturating_add(1);
    fiber.wait = WaitState::Running;
    Ok(TransitionBuilder::new(instance)
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
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
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
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
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
            0,
            PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
            ),
        );
        let transition = apply(&workflow, &snapshot, &command, &context).unwrap();
        let record = JournalRecord::new(
            transition.command_envelope().unwrap().clone(),
            0,
            1,
            workflow.hash().into_bytes(),
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
}
