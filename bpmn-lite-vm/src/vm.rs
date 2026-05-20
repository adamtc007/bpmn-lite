use anyhow::{anyhow, Result};
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_types::events::RuntimeEvent;
use bpmn_lite_types::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const MESSAGE_CLAIM_MS: u64 = 30_000;

/// Result of a single VM tick on a fiber.
#[derive(Debug)]
pub enum TickOutcome {
    /// Fiber is still running, can be ticked again.
    Continue,
    /// Fiber parked on a wait state (timer, message, job, join).
    Parked(WaitState),
    /// Fiber ended (process may complete if this was the last fiber).
    Ended,
    /// Fiber hit EndTerminate — engine must kill entire instance.
    Terminated,
    /// Fiber hit a Fail instruction.
    Failed { code: u32 },
    /// Fiber hit ExecFfi — engine must dispatch the call in-process.
    ///
    /// The VM does NOT advance `fiber.pc` or park the fiber. The engine
    /// handles dispatch, applies output bindings, advances pc, and
    /// re-runs the fiber. Per A2 §2 dispatch model.
    ExecFfi {
        template_id: [u8; 32],
        pc: Addr,
        invocation_id: Uuid,
    },
}

/// The BPMN-Lite VM. Executes bytecode fibers against a ProcessStore.
pub struct Vm {
    store: Arc<dyn ProcessStore>,
}

impl Vm {
    pub fn new(store: Arc<dyn ProcessStore>) -> Self {
        Self { store }
    }

    /// Execute a single instruction on the given fiber.
    ///
    /// Returns the outcome indicating whether to keep ticking or the fiber parked/ended.
    pub async fn tick_fiber(
        &self,
        fiber: &mut Fiber,
        instance: &mut ProcessInstance,
        program: &CompiledProgram,
    ) -> Result<TickOutcome> {
        let mut pending_events = Vec::new();
        let outcome = self
            .tick_fiber_inner(fiber, instance, program, &mut pending_events)
            .await?;
        self.flush_events(instance.instance_id, &mut pending_events)
            .await?;
        Ok(outcome)
    }

    async fn tick_fiber_inner(
        &self,
        fiber: &mut Fiber,
        instance: &mut ProcessInstance,
        program: &CompiledProgram,
        pending_events: &mut Vec<RuntimeEvent>,
    ) -> Result<TickOutcome> {
        let pc = fiber.pc as usize;
        if pc >= program.program.len() {
            return Err(anyhow!(
                "pc {pc} out of bounds (program len {})",
                program.program.len()
            ));
        }

        let instr = &program.program[pc];

        match instr {
            Instr::Jump { target } => {
                fiber.pc = *target;
                Ok(TickOutcome::Continue)
            }

            Instr::BrIf { target } => {
                let val = fiber
                    .stack
                    .pop()
                    .ok_or_else(|| anyhow!("BrIf: stack underflow"))?;
                if is_truthy(&val) {
                    fiber.pc = *target;
                } else {
                    fiber.pc += 1;
                }
                Ok(TickOutcome::Continue)
            }

            Instr::BrIfNot { target } => {
                let val = fiber
                    .stack
                    .pop()
                    .ok_or_else(|| anyhow!("BrIfNot: stack underflow"))?;
                if !is_truthy(&val) {
                    fiber.pc = *target;
                } else {
                    fiber.pc += 1;
                }
                Ok(TickOutcome::Continue)
            }

            Instr::PushBool(b) => {
                fiber.stack.push(Value::Bool(*b));
                fiber.pc += 1;
                Ok(TickOutcome::Continue)
            }

            Instr::PushI64(n) => {
                fiber.stack.push(Value::I64(*n));
                fiber.pc += 1;
                Ok(TickOutcome::Continue)
            }

            Instr::Pop => {
                fiber.stack.pop();
                fiber.pc += 1;
                Ok(TickOutcome::Continue)
            }

            Instr::LoadFlag { key } => {
                let val = instance
                    .flags
                    .get(key)
                    .cloned()
                    .unwrap_or(Value::Bool(false));
                fiber.stack.push(val);
                fiber.pc += 1;
                Ok(TickOutcome::Continue)
            }

            Instr::StoreFlag { key } => {
                let val = fiber
                    .stack
                    .pop()
                    .ok_or_else(|| anyhow!("StoreFlag: stack underflow"))?;
                instance.flags.insert(*key, val.clone());
                pending_events.push(RuntimeEvent::FlagSet {
                    key: *key,
                    value: val,
                });
                fiber.pc += 1;
                Ok(TickOutcome::Continue)
            }

            Instr::ExecNative {
                task_type,
                argc: _,
                retc,
            } => {
                // Derive deterministic job_key
                let task_type_str = program
                    .task_manifest
                    .get(*task_type as usize)
                    .cloned()
                    .unwrap_or_else(|| format!("task_{task_type}"));
                let service_task_id = program
                    .debug_map
                    .get(&fiber.pc)
                    .cloned()
                    .unwrap_or_else(|| format!("pc_{}", fiber.pc));
                let job_key = format!(
                    "{}:{}:{}:{}",
                    instance.instance_id, service_task_id, fiber.pc, fiber.loop_epoch
                );

                // Check dedupe
                if let Some(cached) = self.store.dedupe_get(&job_key).await? {
                    // Apply cached completion
                    apply_completion(instance, &cached);
                    // Push retc values (we push a single bool for simplicity)
                    for _ in 0..*retc {
                        fiber.stack.push(Value::Bool(true));
                    }
                    fiber.pc += 1;
                    return Ok(TickOutcome::Continue);
                }

                // Build orch_flags with string keys for wire format
                let orch_flags: BTreeMap<String, Value> = instance
                    .flags
                    .iter()
                    .map(|(k, v)| (format!("flag_{k}"), v.clone()))
                    .collect();

                let activation = JobActivation {
                    job_key: job_key.clone(),
                    tenant_id: instance.tenant_id.clone(),
                    process_instance_id: instance.instance_id,
                    task_type: task_type_str.clone(),
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
                };

                // Emit event
                pending_events.push(RuntimeEvent::JobActivated {
                    job_key: job_key.clone(),
                    task_type: task_type_str,
                    service_task_id,
                    pc: fiber.pc,
                });

                // Enqueue job
                self.store.enqueue_job(&activation).await?;

                // Park fiber — do NOT advance pc
                fiber.wait = WaitState::Job { job_key };
                self.store.save_fiber(instance.instance_id, fiber).await?;

                Ok(TickOutcome::Parked(fiber.wait.clone()))
            }

            Instr::Fork { targets } => {
                let mut child_ids = Vec::new();
                let mut target_addrs = Vec::new();

                for target in targets.iter().copied() {
                    let child_id = Uuid::now_v7();
                    let child = Fiber::new(child_id, target);
                    self.store.save_fiber(instance.instance_id, &child).await?;
                    pending_events.push(RuntimeEvent::FiberSpawned {
                        fiber_id: child_id,
                        pc: target,
                        parent: Some(fiber.fiber_id),
                    });
                    child_ids.push(child_id);
                    target_addrs.push(target);
                }

                pending_events.push(RuntimeEvent::Forked {
                    fork_id: format!("pc_{}", fiber.pc),
                    child_fibers: child_ids,
                    targets: target_addrs,
                });

                // Parent fiber ends after fork
                self.store
                    .delete_fiber(instance.instance_id, fiber.fiber_id)
                    .await?;

                Ok(TickOutcome::Ended)
            }

            Instr::Join { id, expected, next } => {
                let count = self.store.join_arrive(instance.instance_id, *id).await?;

                pending_events.push(RuntimeEvent::JoinArrived {
                    join_id: *id,
                    fiber_id: fiber.fiber_id,
                });

                if count >= *expected {
                    // All branches arrived — release
                    self.store.join_reset(instance.instance_id, *id).await?;
                    pending_events.push(RuntimeEvent::JoinReleased {
                        join_id: *id,
                        next_pc: *next,
                        released_fiber_id: fiber.fiber_id,
                    });
                    fiber.pc = *next;
                    fiber.wait = WaitState::Running;
                    Ok(TickOutcome::Continue)
                } else {
                    // Wait for more branches
                    fiber.wait = WaitState::Join { join_id: *id };
                    self.store.save_fiber(instance.instance_id, fiber).await?;
                    // Delete this fiber — it's consumed by the join
                    self.store
                        .delete_fiber(instance.instance_id, fiber.fiber_id)
                        .await?;
                    Ok(TickOutcome::Ended)
                }
            }

            Instr::WaitFor { ms } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                let deadline = now + *ms;
                fiber.wait = WaitState::Timer {
                    deadline_ms: deadline,
                };
                pending_events.push(RuntimeEvent::WaitTimerSet {
                    fiber_id: fiber.fiber_id,
                    deadline_ms: deadline,
                });
                fiber.pc += 1; // advance past wait so resume continues
                self.store.save_fiber(instance.instance_id, fiber).await?;
                Ok(TickOutcome::Parked(WaitState::Timer {
                    deadline_ms: deadline,
                }))
            }

            Instr::WaitUntil { deadline_ms } => {
                fiber.wait = WaitState::Timer {
                    deadline_ms: *deadline_ms,
                };
                pending_events.push(RuntimeEvent::WaitTimerSet {
                    fiber_id: fiber.fiber_id,
                    deadline_ms: *deadline_ms,
                });
                fiber.pc += 1;
                self.store.save_fiber(instance.instance_id, fiber).await?;
                Ok(TickOutcome::Parked(WaitState::Timer {
                    deadline_ms: *deadline_ms,
                }))
            }

            Instr::WaitMsg {
                wait_id,
                name,
                corr_reg,
            } => {
                let corr_key = if (*corr_reg as usize) < fiber.regs.len() {
                    fiber.regs[*corr_reg as usize].clone()
                } else {
                    Value::Bool(false)
                };
                let message_name = message_name_key(program, *name);
                let correlation_key = value_key(&corr_key);
                if let Some(buffered) = self
                    .store
                    .claim_buffered_message(
                        &instance.tenant_id,
                        &message_name,
                        &correlation_key,
                        MESSAGE_CLAIM_MS,
                    )
                    .await?
                {
                    let payload_update = buffered
                        .message
                        .payload_hash
                        .map(|hash| {
                            let payload =
                                std::str::from_utf8(&buffered.message.payload).map_err(|err| {
                                    anyhow!("buffered message payload is not utf-8: {err}")
                                })?;
                            Ok::<_, anyhow::Error>(PayloadUpdate {
                                payload: payload.to_string(),
                                payload_hash: hash,
                            })
                        })
                        .transpose()?;
                    let events = vec![
                        RuntimeEvent::BufferedMessageConsumed {
                            message_name,
                            correlation_key,
                            msg_id: buffered.message.msg_id.clone(),
                            fiber_id: fiber.fiber_id,
                        },
                        RuntimeEvent::MsgReceived {
                            name: *name,
                            corr_key: corr_key.clone(),
                            msg_ref: None,
                        },
                    ];
                    fiber.pc += 1;
                    fiber.wait = WaitState::Running;
                    if self
                        .store
                        .atomic_consume_buffered_message(
                            instance,
                            fiber,
                            &buffered,
                            payload_update.as_ref(),
                            &events,
                        )
                        .await?
                    {
                        if let Some(payload_update) = payload_update {
                            instance.domain_payload = Arc::from(payload_update.payload.as_str());
                            instance.domain_payload_hash = payload_update.payload_hash;
                        }
                        return Ok(TickOutcome::Continue);
                    }
                    let _ = self.store.release_buffered_message_claim(&buffered).await?;
                }

                fiber.wait = WaitState::Msg {
                    wait_id: *wait_id,
                    name: *name,
                    corr_key: corr_key.clone(),
                };

                pending_events.push(RuntimeEvent::WaitMsgSubscribed {
                    fiber_id: fiber.fiber_id,
                    name: *name,
                    corr_key,
                });
                fiber.pc += 1;
                self.store.save_fiber(instance.instance_id, fiber).await?;
                Ok(TickOutcome::Parked(fiber.wait.clone()))
            }

            Instr::WaitAny { race_id, arms } => {
                // Build arm descriptions for event log
                let arm_descs: Vec<bpmn_lite_types::events::WaitArmDesc> =
                    arms.iter().map(|a| a.into()).collect();

                // Emit RaceRegistered event
                pending_events.push(RuntimeEvent::RaceRegistered {
                    race_id: *race_id,
                    fiber_id: fiber.fiber_id,
                    arms: arm_descs,
                });

                for (i, arm) in arms.iter().enumerate() {
                    if let WaitArm::Msg { name, corr_reg, .. } = arm {
                        let corr_key = if (*corr_reg as usize) < fiber.regs.len() {
                            fiber.regs[*corr_reg as usize].clone()
                        } else {
                            Value::Bool(false)
                        };
                        let message_name = message_name_key(program, *name);
                        let correlation_key = value_key(&corr_key);
                        if let Some(buffered) = self
                            .store
                            .claim_buffered_message(
                                &instance.tenant_id,
                                &message_name,
                                &correlation_key,
                                MESSAGE_CLAIM_MS,
                            )
                            .await?
                        {
                            let payload_update = buffered
                                .message
                                .payload_hash
                                .map(|hash| {
                                    let payload = std::str::from_utf8(&buffered.message.payload)
                                        .map_err(|err| {
                                            anyhow!("buffered message payload is not utf-8: {err}")
                                        })?;
                                    Ok::<_, anyhow::Error>(PayloadUpdate {
                                        payload: payload.to_string(),
                                        payload_hash: hash,
                                    })
                                })
                                .transpose()?;
                            let resume_at = arm.resume_at();
                            let mut events = vec![
                                RuntimeEvent::BufferedMessageConsumed {
                                    message_name,
                                    correlation_key,
                                    msg_id: buffered.message.msg_id.clone(),
                                    fiber_id: fiber.fiber_id,
                                },
                                RuntimeEvent::MsgReceived {
                                    name: *name,
                                    corr_key,
                                    msg_ref: None,
                                },
                                RuntimeEvent::RaceWon {
                                    race_id: *race_id,
                                    fiber_id: fiber.fiber_id,
                                    winner_index: i,
                                    resume_at,
                                },
                            ];
                            let cancelled_indices: Vec<usize> =
                                (0..arms.len()).filter(|idx| *idx != i).collect();
                            if !cancelled_indices.is_empty() {
                                events.push(RuntimeEvent::RaceCancelled {
                                    race_id: *race_id,
                                    cancelled_indices,
                                });
                            }
                            fiber.pc = resume_at;
                            fiber.wait = WaitState::Running;
                            if self
                                .store
                                .atomic_consume_buffered_message(
                                    instance,
                                    fiber,
                                    &buffered,
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
                                pending_events.clear();
                                return Ok(TickOutcome::Continue);
                            }
                            let _ = self.store.release_buffered_message_claim(&buffered).await?;
                        }
                    }
                }

                // Emit WaitMsgSubscribed for each Msg arm (so signal() can find them)
                for arm in arms.iter() {
                    if let WaitArm::Msg { name, corr_reg, .. } = arm {
                        let corr_key = if (*corr_reg as usize) < fiber.regs.len() {
                            fiber.regs[*corr_reg as usize].clone()
                        } else {
                            Value::Bool(false)
                        };
                        pending_events.push(RuntimeEvent::WaitMsgSubscribed {
                            fiber_id: fiber.fiber_id,
                            name: *name,
                            corr_key,
                        });
                    }
                }

                // Park fiber in Race wait state — do NOT advance pc
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                let timer_deadline_ms = arms.iter().find_map(|arm| match arm {
                    WaitArm::Timer { duration_ms, .. } => Some(now + duration_ms),
                    WaitArm::Deadline { deadline_ms, .. } => Some(*deadline_ms),
                    _ => None,
                });
                fiber.wait = WaitState::Race {
                    race_id: *race_id,
                    timer_deadline_ms,
                    job_key: None,
                    interrupting: true,
                    timer_arm_index: None,
                    cycle_remaining: None,
                    cycle_fired_count: 0,
                };
                self.store.save_fiber(instance.instance_id, fiber).await?;

                Ok(TickOutcome::Parked(fiber.wait.clone()))
            }

            Instr::CancelWait { wait_id: _ } => {
                // CancelWait is a no-op in the VM — cancellation is handled by the engine
                // when it resolves a race winner and cancels losers.
                fiber.pc += 1;
                Ok(TickOutcome::Continue)
            }

            Instr::IncCounter { counter_id } => {
                let count = instance.counters.entry(*counter_id).or_insert(0);
                *count += 1;
                let new_value = *count;
                fiber.loop_epoch += 1;
                pending_events.push(RuntimeEvent::CounterIncremented {
                    counter_id: *counter_id,
                    new_value,
                    loop_epoch: fiber.loop_epoch,
                });
                fiber.pc += 1;
                Ok(TickOutcome::Continue)
            }

            Instr::BrCounterLt {
                counter_id,
                limit,
                target,
            } => {
                let count = instance.counters.get(counter_id).copied().unwrap_or(0);
                if count < *limit {
                    fiber.pc = *target;
                } else {
                    fiber.pc += 1;
                }
                Ok(TickOutcome::Continue)
            }

            Instr::ForkInclusive {
                branches,
                join_id,
                default_target,
            } => {
                let mut taken_targets = Vec::new();

                for branch in branches.iter() {
                    let take = match branch.condition_flag {
                        None => true, // unconditional — always taken
                        Some(key) => {
                            let val = instance
                                .flags
                                .get(&key)
                                .cloned()
                                .unwrap_or(Value::Bool(false));
                            is_truthy(&val)
                        }
                    };
                    if take {
                        taken_targets.push(branch.target);
                    }
                }

                // Zero conditions matched
                if taken_targets.is_empty() {
                    if let Some(default) = default_target {
                        taken_targets.push(*default);
                    } else {
                        // No default — incident
                        let incident_id = Uuid::now_v7();
                        let gateway_id = program
                            .debug_map
                            .get(&fiber.pc)
                            .cloned()
                            .unwrap_or_else(|| format!("inclusive_fork_pc_{}", fiber.pc));
                        let incident = Incident {
                            incident_id,
                            process_instance_id: instance.instance_id,
                            fiber_id: fiber.fiber_id,
                            service_task_id: gateway_id.clone(),
                            bytecode_addr: fiber.pc,
                            error_class: ErrorClass::ContractViolation,
                            message: "Inclusive gateway: no conditions matched and no default flow"
                                .into(),
                            retry_count: 0,
                            created_at: now_ms(),
                            resolved_at: None,
                            resolution: None,
                        };
                        self.store.save_incident(&incident).await?;
                        pending_events.push(RuntimeEvent::IncidentCreated {
                            incident_id,
                            service_task_id: gateway_id,
                            job_key: None,
                        });
                        fiber.wait = WaitState::Incident { incident_id };
                        self.store.save_fiber(instance.instance_id, fiber).await?;
                        instance.state = ProcessState::Failed { incident_id };
                        return Ok(TickOutcome::Parked(WaitState::Incident { incident_id }));
                    }
                }

                // Set dynamic join expected
                instance
                    .join_expected
                    .insert(*join_id, taken_targets.len() as u16);

                // Spawn fibers
                let mut child_ids = Vec::new();
                for &target in &taken_targets {
                    let child_id = Uuid::now_v7();
                    let child = Fiber::new(child_id, target);
                    self.store.save_fiber(instance.instance_id, &child).await?;
                    pending_events.push(RuntimeEvent::FiberSpawned {
                        fiber_id: child_id,
                        pc: target,
                        parent: Some(fiber.fiber_id),
                    });
                    child_ids.push(child_id);
                }

                // Emit InclusiveForkTaken event
                pending_events.push(RuntimeEvent::InclusiveForkTaken {
                    gateway_id: format!("pc_{}", fiber.pc),
                    branches_taken: taken_targets.clone(),
                    join_id: *join_id,
                    expected: taken_targets.len() as u16,
                });

                // Delete parent fiber (same pattern as Fork)
                self.store
                    .delete_fiber(instance.instance_id, fiber.fiber_id)
                    .await?;
                Ok(TickOutcome::Ended)
            }

            Instr::JoinDynamic { id, next } => {
                let expected =
                    instance.join_expected.get(id).copied().ok_or_else(|| {
                        anyhow!("JoinDynamic: no expected count for join_id {}", id)
                    })?;

                let count = self.store.join_arrive(instance.instance_id, *id).await?;

                pending_events.push(RuntimeEvent::JoinArrived {
                    join_id: *id,
                    fiber_id: fiber.fiber_id,
                });

                if count >= expected {
                    // All branches arrived — release
                    self.store.join_reset(instance.instance_id, *id).await?;
                    instance.join_expected.remove(id); // clean up dynamic expected
                    pending_events.push(RuntimeEvent::JoinReleased {
                        join_id: *id,
                        next_pc: *next,
                        released_fiber_id: fiber.fiber_id,
                    });
                    // Advance pc AFTER event is recorded (PITR determinism)
                    fiber.pc = *next;
                    fiber.wait = WaitState::Running;
                    Ok(TickOutcome::Continue)
                } else {
                    // Wait for more — consume this fiber (do NOT save before delete)
                    self.store
                        .delete_fiber(instance.instance_id, fiber.fiber_id)
                        .await?;
                    Ok(TickOutcome::Ended)
                }
            }

            Instr::End => {
                self.store
                    .delete_fiber(instance.instance_id, fiber.fiber_id)
                    .await?;
                Ok(TickOutcome::Ended)
            }

            Instr::EndTerminate => {
                // Do NOT delete fibers — engine owns all teardown.
                Ok(TickOutcome::Terminated)
            }

            Instr::Fail { code } => {
                let incident_id = Uuid::now_v7();
                let incident = Incident {
                    incident_id,
                    process_instance_id: instance.instance_id,
                    fiber_id: fiber.fiber_id,
                    service_task_id: String::new(),
                    bytecode_addr: fiber.pc,
                    error_class: ErrorClass::BusinessRejection {
                        rejection_code: format!("FAIL_{code}"),
                    },
                    message: format!("Process failed with code {code}"),
                    retry_count: 0,
                    created_at: now_ms(),
                    resolved_at: None,
                    resolution: None,
                };
                self.store.save_incident(&incident).await?;
                fiber.wait = WaitState::Incident { incident_id };
                self.store.save_fiber(instance.instance_id, fiber).await?;
                Ok(TickOutcome::Failed { code: *code })
            }

            // ExecFfi — signal the engine to dispatch in-process.
            //
            // The VM assigns a UUIDv7 invocation id and returns a typed
            // TickOutcome::ExecFfi. PC is NOT advanced here; the engine
            // advances it after successful dispatch (or parks the fiber
            // on incident). Per A2 §2.
            Instr::ExecFfi { template_id, .. } => {
                let invocation_id = Uuid::now_v7();
                Ok(TickOutcome::ExecFfi {
                    template_id: *template_id,
                    pc: fiber.pc,
                    invocation_id,
                })
            }
        }
    }

    /// Run a fiber to completion or parking. Returns the final outcome.
    pub async fn run_fiber(
        &self,
        fiber: &mut Fiber,
        instance: &mut ProcessInstance,
        program: &CompiledProgram,
        max_steps: usize,
    ) -> Result<TickOutcome> {
        let mut pending_events = Vec::new();
        for _ in 0..max_steps {
            match self
                .tick_fiber_inner(fiber, instance, program, &mut pending_events)
                .await?
            {
                TickOutcome::Continue => continue,
                other => {
                    self.flush_events(instance.instance_id, &mut pending_events)
                        .await?;
                    return Ok(other);
                }
            }
        }
        self.flush_events(instance.instance_id, &mut pending_events)
            .await?;
        Ok(TickOutcome::Continue)
    }

    async fn flush_events(
        &self,
        instance_id: Uuid,
        pending_events: &mut Vec<RuntimeEvent>,
    ) -> Result<()> {
        if pending_events.is_empty() {
            return Ok(());
        }
        self.store
            .batch_append_events(instance_id, pending_events)
            .await?;
        pending_events.clear();
        Ok(())
    }

    /// Resume a fiber parked on a job. Fiber-resume ONLY — no mutation of
    /// instance flags/payload, no dedupe, no save_instance, no job ack. The
    /// engine owns all completion mutation.
    ///
    /// Returns `Some(fiber_id)` if a matching fiber was found and resumed,
    /// `None` if no fiber is parked on this job_key (ghost signal).
    pub async fn complete_job(
        &self,
        instance: &mut ProcessInstance,
        completion: &JobCompletion,
        program: &CompiledProgram,
    ) -> Result<Option<Uuid>> {
        let fibers = self.store.load_fibers(instance.instance_id).await?;
        let parked = fibers.iter().find(
            |f| matches!(&f.wait, WaitState::Job { job_key } if job_key == &completion.job_key),
        );

        if let Some(parked_fiber) = parked {
            let mut fiber = parked_fiber.clone();

            let retc = if let Some(Instr::ExecNative { retc, .. }) =
                program.program.get(fiber.pc as usize)
            {
                *retc
            } else {
                0
            };

            for _ in 0..retc {
                fiber.stack.push(Value::Bool(true));
            }

            fiber.pc += 1;
            fiber.wait = WaitState::Running;

            self.store.save_fiber(instance.instance_id, &fiber).await?;

            Ok(Some(fiber.fiber_id))
        } else {
            Ok(None)
        }
    }

    /// Resolve a race — called by the engine when an arm fires.
    /// Returns the resume_at address if this fiber was in a matching race.
    pub async fn resolve_race(
        &self,
        instance: &mut ProcessInstance,
        fiber: &mut Fiber,
        race_id: RaceId,
        winner_index: usize,
        arms: &[WaitArm],
    ) -> Result<Option<Addr>> {
        // Verify fiber is in the expected race
        let in_race =
            matches!(&fiber.wait, WaitState::Race { race_id: rid, .. } if *rid == race_id);
        if !in_race {
            return Ok(None);
        }

        let winner_arm = arms
            .get(winner_index)
            .ok_or_else(|| anyhow!("resolve_race: winner_index {} out of bounds", winner_index))?;
        let resume_at = winner_arm.resume_at();

        // Emit RaceWon
        self.store
            .append_event(
                instance.instance_id,
                &RuntimeEvent::RaceWon {
                    race_id,
                    fiber_id: fiber.fiber_id,
                    winner_index,
                    resume_at,
                },
            )
            .await?;

        // Emit RaceCancelled for loser arms
        let cancelled_indices: Vec<usize> =
            (0..arms.len()).filter(|&i| i != winner_index).collect();
        if !cancelled_indices.is_empty() {
            self.store
                .append_event(
                    instance.instance_id,
                    &RuntimeEvent::RaceCancelled {
                        race_id,
                        cancelled_indices,
                    },
                )
                .await?;
        }

        // Resume fiber at winner's address
        fiber.pc = resume_at;
        fiber.wait = WaitState::Running;
        self.store.save_fiber(instance.instance_id, fiber).await?;

        Ok(Some(resume_at))
    }

    /// Handle a job failure — create incident and park fiber.
    pub async fn fail_job(
        &self,
        instance: &mut ProcessInstance,
        failure: &JobFailure,
    ) -> Result<()> {
        let fibers = self.store.load_fibers(instance.instance_id).await?;
        let parked = fibers
            .iter()
            .find(|f| matches!(&f.wait, WaitState::Job { job_key } if job_key == &failure.job_key));

        if let Some(parked_fiber) = parked {
            let mut fiber = parked_fiber.clone();
            let incident_id = Uuid::now_v7();
            let service_task_id = format!("pc_{}", fiber.pc);

            let incident = Incident {
                incident_id,
                process_instance_id: instance.instance_id,
                fiber_id: fiber.fiber_id,
                service_task_id: service_task_id.clone(),
                bytecode_addr: fiber.pc,
                error_class: failure.error_class.clone(),
                message: failure.message.clone(),
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
                        job_key: Some(failure.job_key.clone()),
                    },
                )
                .await?;

            fiber.wait = WaitState::Incident { incident_id };
            self.store.save_fiber(instance.instance_id, &fiber).await?;

            instance.state = ProcessState::Failed { incident_id };
            self.store.save_instance(instance).await?;
        }

        Ok(())
    }
}

fn is_truthy(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::I64(n) => *n != 0,
        Value::Str(_) => true,
        Value::Ref(_) => true,
    }
}

fn value_key(value: &Value) -> String {
    match value {
        Value::Bool(b) => format!("b:{b}"),
        Value::I64(n) => format!("i:{n}"),
        Value::Str(s) => format!("s:{s}"),
        Value::Ref(r) => format!("r:{r}"),
    }
}

fn message_name_key(program: &CompiledProgram, name: u32) -> String {
    program
        .message_name_map
        .get(&name)
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

pub fn apply_completion(instance: &mut ProcessInstance, completion: &JobCompletion) {
    instance.domain_payload = Arc::<str>::from(completion.domain_payload.as_str());
    instance.domain_payload_hash = compute_hash(&completion.domain_payload);
    // Merge orch_flags: completion flags update instance flags
    // We need to convert string keys back to FlagKey (u32)
    for (key_str, value) in &completion.orch_flags {
        if let Some(stripped) = key_str.strip_prefix("flag_") {
            if let Ok(key) = stripped.parse::<u32>() {
                instance.flags.insert(key, value.clone());
            }
        }
    }
}

pub fn compute_hash(data: &str) -> [u8; 32] {
    blake3::hash(data.as_bytes()).into()
}

fn now_ms() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_store::store_memory::MemoryStore;
    use std::collections::BTreeMap;

    fn make_program(instrs: Vec<Instr>) -> CompiledProgram {
        CompiledProgram {
            bytecode_version: [0u8; 32],
            program: instrs,
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["create_case".to_string(), "request_docs".to_string()],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        }
    }

    fn make_instance() -> ProcessInstance {
        let payload = r#"{"case_id":"test"}"#;
        ProcessInstance {
            instance_id: Uuid::now_v7(),
            process_key: "test".to_string(),
            bytecode_version: [0u8; 32],
            tenant_id: "default".to_string(),
            domain_payload: payload.to_string().into(),
            domain_payload_hash: compute_hash(payload),
            session_stack: ob_poc_types::session_stack::SessionStackState::default(),
            flags: BTreeMap::new(),
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: "test-corr".to_string(),
            entry_id: Uuid::new_v4(),
            runbook_id: Uuid::new_v4(),
            created_at: 0,
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: None,
        }
    }

    /// A3.T1: Linear flow with jobs
    #[tokio::test]
    async fn test_linear_flow_with_jobs() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        // ExecNative(task_a) → ExecNative(task_b) → End
        let program = make_program(vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::ExecNative {
                task_type: 1,
                argc: 0,
                retc: 0,
            },
            Instr::End,
        ]);

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();

        // Tick — should park on first ExecNative
        let outcome = vm
            .tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            TickOutcome::Parked(WaitState::Job { .. })
        ));

        // Dequeue job
        let jobs = store
            .dequeue_jobs(
                &["create_case".to_string()],
                10,
                "default",
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1);

        // Complete first job
        let payload_after_1 = r#"{"case_id":"test","step":"1"}"#;
        let completion1 = JobCompletion {
            job_key: jobs[0].job_key.clone(),
            domain_payload: payload_after_1.to_string(),
            expected_instance_payload_hash: compute_hash(r#"{"case_id":"abc"}"#),
            orch_flags: BTreeMap::new(),
        };
        let resumed = vm
            .complete_job(&mut instance, &completion1, &program)
            .await
            .unwrap();
        assert!(resumed.is_some(), "Fiber should be resumed");
        apply_completion(&mut instance, &completion1);
        store.save_instance(&instance).await.unwrap();
        assert_eq!(instance.domain_payload.as_ref(), payload_after_1);

        // Load resumed fiber
        let fibers = store.load_fibers(instance.instance_id).await.unwrap();
        assert_eq!(fibers.len(), 1);
        let mut fiber = fibers[0].clone();
        assert_eq!(fiber.pc, 1);
        assert_eq!(fiber.wait, WaitState::Running);

        // Tick again — should park on second ExecNative
        let outcome = vm
            .tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            TickOutcome::Parked(WaitState::Job { .. })
        ));

        // Dequeue second job
        let jobs = store
            .dequeue_jobs(
                &["request_docs".to_string()],
                10,
                "default",
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1);

        // Complete second job
        let payload_after_2 = r#"{"case_id":"test","step":"2"}"#;
        let completion2 = JobCompletion {
            job_key: jobs[0].job_key.clone(),
            domain_payload: payload_after_2.to_string(),
            expected_instance_payload_hash: compute_hash(payload_after_1),
            orch_flags: BTreeMap::new(),
        };
        let resumed = vm
            .complete_job(&mut instance, &completion2, &program)
            .await
            .unwrap();
        assert!(resumed.is_some());
        apply_completion(&mut instance, &completion2);
        store.save_instance(&instance).await.unwrap();

        // Load resumed fiber and tick to End
        let fibers = store.load_fibers(instance.instance_id).await.unwrap();
        let mut fiber = fibers[0].clone();
        assert_eq!(fiber.pc, 2);

        let outcome = vm
            .tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();
        assert!(matches!(outcome, TickOutcome::Ended));
    }

    /// A3.T2: Flag round-trip across job boundary
    #[tokio::test]
    async fn test_flag_round_trip_across_job() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        // ExecNative → (completion sets flag_0) → StoreFlag(0) → LoadFlag(0) → BrIf(End) → Fail → End
        let program = make_program(vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            }, // 0
            Instr::LoadFlag { key: 0 }, // 1
            Instr::BrIf { target: 4 },  // 2
            Instr::Fail { code: 1 },    // 3 (should not reach)
            Instr::End,                 // 4
        ]);

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();

        // Tick — parks on ExecNative
        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();

        // Complete job — set flag_0 = true via orch_flags
        let jobs = store
            .dequeue_jobs(
                &["create_case".to_string()],
                1,
                "default",
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        let payload = r#"{"done":true}"#;
        let completion = JobCompletion {
            job_key: jobs[0].job_key.clone(),
            domain_payload: payload.to_string(),
            expected_instance_payload_hash: compute_hash(r#"{"case_id":"abc"}"#),
            orch_flags: BTreeMap::from([("flag_0".to_string(), Value::Bool(true))]),
        };
        let resumed = vm
            .complete_job(&mut instance, &completion, &program)
            .await
            .unwrap();
        assert!(resumed.is_some());
        apply_completion(&mut instance, &completion);

        // Flag should be set
        assert_eq!(instance.flags.get(&0), Some(&Value::Bool(true)));

        // Resume and run to completion
        let fibers = store.load_fibers(instance.instance_id).await.unwrap();
        let mut fiber = fibers[0].clone();
        let outcome = vm
            .run_fiber(&mut fiber, &mut instance, &program, 100)
            .await
            .unwrap();
        assert!(matches!(outcome, TickOutcome::Ended));
    }

    /// A3.T2b: FlagSet event emitted by StoreFlag
    #[tokio::test]
    async fn test_flag_set_event() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let program = make_program(vec![
            Instr::PushBool(true),
            Instr::StoreFlag { key: 5 },
            Instr::End,
        ]);

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        vm.run_fiber(&mut fiber, &mut instance, &program, 100)
            .await
            .unwrap();

        let events = store.read_events(instance.instance_id, 1).await.unwrap();
        let flag_set = events
            .iter()
            .find(|(_, e)| matches!(e, RuntimeEvent::FlagSet { .. }));
        assert!(flag_set.is_some());
    }

    /// A3.T3: Dedupe on re-delivery
    #[tokio::test]
    async fn test_dedupe_on_redelivery() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let program = make_program(vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::End,
        ]);

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();

        // First tick — parks
        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();
        let jobs = store
            .dequeue_jobs(
                &["create_case".to_string()],
                1,
                "default",
                "test-worker",
                300_000,
            )
            .await
            .unwrap();

        // Complete job
        let payload = r#"{"completed":true}"#;
        let completion = JobCompletion {
            job_key: jobs[0].job_key.clone(),
            domain_payload: payload.to_string(),
            expected_instance_payload_hash: compute_hash(r#"{"case_id":"abc"}"#),
            orch_flags: BTreeMap::new(),
        };
        let resumed = vm
            .complete_job(&mut instance, &completion, &program)
            .await
            .unwrap();
        assert!(resumed.is_some());
        apply_completion(&mut instance, &completion);
        store
            .dedupe_put(&completion.job_key, &completion)
            .await
            .unwrap();

        // Simulate re-delivery: create a new fiber at pc=0 (as if restarted)
        let mut fiber2 = Fiber::new(Uuid::now_v7(), 0);
        let outcome = vm
            .tick_fiber(&mut fiber2, &mut instance, &program)
            .await
            .unwrap();

        // Should NOT park — dedupe returns cached completion, advances to End
        assert!(matches!(outcome, TickOutcome::Continue));
        assert_eq!(fiber2.pc, 1);

        // No new jobs enqueued
        let queue = store
            .dequeue_jobs(
                &["create_case".to_string()],
                10,
                "default",
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        assert!(queue.is_empty());
    }

    /// A3.T4: vm.complete_job does fiber-resume only — hash validation is
    /// now the engine's responsibility. VM should succeed regardless of hash.
    #[tokio::test]
    async fn test_payload_hash_validation() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let program = make_program(vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::End,
        ]);

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();
        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();

        let jobs = store
            .dequeue_jobs(
                &["create_case".to_string()],
                1,
                "default",
                "test-worker",
                300_000,
            )
            .await
            .unwrap();

        // Complete with wrong hash — VM no longer validates, engine does
        let completion = JobCompletion {
            job_key: jobs[0].job_key.clone(),
            domain_payload: r#"{"done":true}"#.to_string(),
            expected_instance_payload_hash: [0xFFu8; 32], // Wrong hash
            orch_flags: BTreeMap::new(),
        };

        let result = vm.complete_job(&mut instance, &completion, &program).await;
        assert!(result.is_ok(), "VM should not validate hash — engine does");
        assert!(result.unwrap().is_some(), "Fiber should be resumed");
    }

    /// A3.T5: WaitMsg and End
    #[tokio::test]
    async fn test_wait_msg_and_end() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let program = make_program(vec![
            Instr::WaitMsg {
                wait_id: 0,
                name: 1,
                corr_reg: 0,
            },
            Instr::End,
        ]);

        let mut instance = make_instance();
        let mut fiber = Fiber::new(Uuid::now_v7(), 0);

        let outcome = vm
            .tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            TickOutcome::Parked(WaitState::Msg { .. })
        ));
    }

    #[tokio::test]
    async fn test_wait_for_persists_resume_pc() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let program = make_program(vec![Instr::WaitFor { ms: 1000 }, Instr::End]);
        let mut instance = make_instance();
        let mut fiber = Fiber::new(Uuid::now_v7(), 0);

        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();

        let persisted = store
            .load_fiber(instance.instance_id, fiber.fiber_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.pc, 1);
        assert!(matches!(persisted.wait, WaitState::Timer { .. }));
    }

    #[tokio::test]
    async fn test_wait_until_persists_resume_pc() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let program = make_program(vec![Instr::WaitUntil { deadline_ms: 42 }, Instr::End]);
        let mut instance = make_instance();
        let mut fiber = Fiber::new(Uuid::now_v7(), 0);

        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();

        let persisted = store
            .load_fiber(instance.instance_id, fiber.fiber_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.pc, 1);
        assert_eq!(persisted.wait, WaitState::Timer { deadline_ms: 42 });
    }

    #[tokio::test]
    async fn test_wait_msg_persists_resume_pc() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let program = make_program(vec![
            Instr::WaitMsg {
                wait_id: 7,
                name: 1,
                corr_reg: 0,
            },
            Instr::End,
        ]);
        let mut instance = make_instance();
        let mut fiber = Fiber::new(Uuid::now_v7(), 0);

        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();

        let persisted = store
            .load_fiber(instance.instance_id, fiber.fiber_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.pc, 1);
        assert!(matches!(
            persisted.wait,
            WaitState::Msg {
                wait_id: 7,
                name: 1,
                ..
            }
        ));
    }

    /// A3.T6: Event log completeness for linear flow
    #[tokio::test]
    async fn test_event_log_completeness() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let program = make_program(vec![
            Instr::ExecNative {
                task_type: 0,
                argc: 0,
                retc: 0,
            },
            Instr::End,
        ]);

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();

        // Tick — parks
        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();

        // Complete
        let jobs = store
            .dequeue_jobs(
                &["create_case".to_string()],
                1,
                "default",
                "test-worker",
                300_000,
            )
            .await
            .unwrap();
        let payload = r#"{"done":true}"#;
        let completion = JobCompletion {
            job_key: jobs[0].job_key.clone(),
            domain_payload: payload.to_string(),
            expected_instance_payload_hash: compute_hash(r#"{"case_id":"abc"}"#),
            orch_flags: BTreeMap::new(),
        };
        let resumed = vm
            .complete_job(&mut instance, &completion, &program)
            .await
            .unwrap();
        assert!(resumed.is_some());
        let payload_hash_before = instance.domain_payload_hash;
        let resumed_fiber = store
            .load_fiber(instance.instance_id, resumed.unwrap())
            .await
            .unwrap()
            .unwrap();
        apply_completion(&mut instance, &completion);
        store
            .atomic_complete(
                &instance,
                &completion,
                &[RuntimeEvent::JobCompleted {
                    job_key: completion.job_key.clone(),
                    payload_hash_before,
                    payload_hash_after: instance.domain_payload_hash,
                    orch_flags_out: completion.orch_flags.clone(),
                    pc_next: resumed_fiber.pc,
                }],
            )
            .await
            .unwrap();

        // Check events
        let events = store.read_events(instance.instance_id, 1).await.unwrap();

        let has_activated = events
            .iter()
            .any(|(_, e)| matches!(e, RuntimeEvent::JobActivated { .. }));
        let has_completed = events
            .iter()
            .any(|(_, e)| matches!(e, RuntimeEvent::JobCompleted { .. }));

        assert!(has_activated, "Missing JobActivated event");
        assert!(has_completed, "Missing JobCompleted event");
    }

    // ═══════════════════════════════════════════════════════════
    //  Phase 1: Race semantics tests
    // ═══════════════════════════════════════════════════════════

    /// T-RACE-1: WAIT_ANY(timer, msg) → msg arrives first → winner is msg arm;
    /// timer cancelled; fiber resumes once at msg's resume_at.
    #[tokio::test]
    async fn t_race_1_msg_wins() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        // Program layout:
        //   0: WaitAny(race_id=0, [timer→addr 3, msg→addr 1])
        //   1: End   ← msg winner path
        //   2: End   ← (unused spacer)
        //   3: End   ← timer winner path (escalation)
        let program = CompiledProgram {
            bytecode_version: [1u8; 32],
            program: vec![
                Instr::WaitAny {
                    race_id: 0,
                    arms: Box::new([
                        WaitArm::Deadline {
                            deadline_ms: u64::MAX, // far future — won't fire
                            resume_at: 3,
                        },
                        WaitArm::Msg {
                            name: 1,
                            corr_reg: 0,
                            resume_at: 1,
                        },
                    ]),
                },
                Instr::End, // addr 1: msg path
                Instr::End, // addr 2: spacer
                Instr::End, // addr 3: timer path
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::from([(
                0,
                RacePlanEntry {
                    arms: vec![
                        WaitArm::Deadline {
                            deadline_ms: u64::MAX,
                            resume_at: 3,
                        },
                        WaitArm::Msg {
                            name: 1,
                            corr_reg: 0,
                            resume_at: 1,
                        },
                    ],
                    boundary_element_id: None,
                },
            )]),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();
        store
            .store_program(program.bytecode_version, &program)
            .await
            .unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();

        // Tick — should park in Race
        let outcome = vm
            .tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();
        assert!(
            matches!(
                outcome,
                TickOutcome::Parked(WaitState::Race { race_id: 0, .. })
            ),
            "Expected Parked(Race), got {:?}",
            outcome
        );

        // Simulate message arrival — resolve race with winner_index=1 (msg arm)
        vm.resolve_race(&mut instance, &mut fiber, 0, 1, &program.race_plan[&0].arms)
            .await
            .unwrap();

        // Fiber should be at addr 1 (msg path), Running
        assert_eq!(fiber.pc, 1);
        assert_eq!(fiber.wait, WaitState::Running);

        // Run to completion
        let outcome = vm
            .run_fiber(&mut fiber, &mut instance, &program, 10)
            .await
            .unwrap();
        assert!(matches!(outcome, TickOutcome::Ended));

        // Verify events: should have RaceRegistered, RaceWon, RaceCancelled
        let events = store.read_events(instance.instance_id, 1).await.unwrap();
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, RuntimeEvent::RaceRegistered { .. })),
            "Missing RaceRegistered event"
        );
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                RuntimeEvent::RaceWon {
                    winner_index: 1,
                    ..
                }
            )),
            "Missing RaceWon event with winner_index=1"
        );
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, RuntimeEvent::RaceCancelled { .. })),
            "Missing RaceCancelled event"
        );
    }

    /// T-RACE-2: WAIT_ANY(timer, msg) → timer wins → escalation branch;
    /// later msg arrival is ignored.
    #[tokio::test]
    async fn t_race_2_timer_wins() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let program = CompiledProgram {
            bytecode_version: [2u8; 32],
            program: vec![
                Instr::WaitAny {
                    race_id: 0,
                    arms: Box::new([
                        WaitArm::Deadline {
                            deadline_ms: 0, // already expired
                            resume_at: 3,
                        },
                        WaitArm::Msg {
                            name: 1,
                            corr_reg: 0,
                            resume_at: 1,
                        },
                    ]),
                },
                Instr::End, // addr 1: msg path (should NOT be reached)
                Instr::End, // addr 2: spacer
                Instr::End, // addr 3: timer/escalation path
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::from([(
                0,
                RacePlanEntry {
                    arms: vec![
                        WaitArm::Deadline {
                            deadline_ms: 0,
                            resume_at: 3,
                        },
                        WaitArm::Msg {
                            name: 1,
                            corr_reg: 0,
                            resume_at: 1,
                        },
                    ],
                    boundary_element_id: None,
                },
            )]),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();
        store
            .store_program(program.bytecode_version, &program)
            .await
            .unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();

        // Tick — parks in Race
        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();

        // Resolve with timer winning (index 0)
        vm.resolve_race(&mut instance, &mut fiber, 0, 0, &program.race_plan[&0].arms)
            .await
            .unwrap();

        // Fiber should be at addr 3 (timer/escalation path)
        assert_eq!(fiber.pc, 3);
        assert_eq!(fiber.wait, WaitState::Running);

        // Late message arrival: try to resolve again — should return None (fiber no longer in Race)
        let result = vm
            .resolve_race(&mut instance, &mut fiber, 0, 1, &program.race_plan[&0].arms)
            .await
            .unwrap();
        assert!(
            result.is_none(),
            "Late signal should be ignored (fiber not in Race)"
        );
    }

    /// T-RACE-3: Crash after RaceWon persisted but before fiber resume.
    /// Simulates replay: events contain RaceWon, fiber state must be reconstructable.
    #[tokio::test]
    async fn t_race_3_replay_after_race_won() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let arms = vec![
            WaitArm::Deadline {
                deadline_ms: u64::MAX,
                resume_at: 3,
            },
            WaitArm::Msg {
                name: 1,
                corr_reg: 0,
                resume_at: 1,
            },
        ];

        let program = CompiledProgram {
            bytecode_version: [3u8; 32],
            program: vec![
                Instr::WaitAny {
                    race_id: 0,
                    arms: arms.clone().into_boxed_slice(),
                },
                Instr::End,
                Instr::End,
                Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::from([(
                0,
                RacePlanEntry {
                    arms: arms.clone(),
                    boundary_element_id: None,
                },
            )]),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();

        // Tick to park
        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();

        // Resolve race — this persists RaceWon event
        vm.resolve_race(&mut instance, &mut fiber, 0, 1, &arms)
            .await
            .unwrap();

        // "Crash" — reload fiber from store
        let reloaded_fibers = store.load_fibers(instance.instance_id).await.unwrap();
        let reloaded = reloaded_fibers
            .iter()
            .find(|f| f.fiber_id == fiber.fiber_id)
            .expect("Fiber should still exist in store");

        // After resolve_race, fiber should be persisted as Running at resume_at
        assert_eq!(
            reloaded.pc, 1,
            "Fiber pc should be at msg winner's resume_at"
        );
        assert_eq!(
            reloaded.wait,
            WaitState::Running,
            "Fiber should be Running after race resolution"
        );

        // Verify RaceWon event exists in log
        let events = store.read_events(instance.instance_id, 1).await.unwrap();
        let race_won = events
            .iter()
            .find(|(_, e)| matches!(e, RuntimeEvent::RaceWon { .. }));
        assert!(race_won.is_some(), "RaceWon event must be persisted");
    }

    /// T-RACE-4: Duplicate signal (same msg_id) is a no-op.
    /// After race is resolved, subsequent resolve_race calls return None.
    #[tokio::test]
    async fn t_race_4_duplicate_signal_noop() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let arms = vec![
            WaitArm::Deadline {
                deadline_ms: u64::MAX,
                resume_at: 3,
            },
            WaitArm::Msg {
                name: 1,
                corr_reg: 0,
                resume_at: 1,
            },
        ];

        let program = CompiledProgram {
            bytecode_version: [4u8; 32],
            program: vec![
                Instr::WaitAny {
                    race_id: 0,
                    arms: arms.clone().into_boxed_slice(),
                },
                Instr::End,
                Instr::End,
                Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::from([(
                0,
                RacePlanEntry {
                    arms: arms.clone(),
                    boundary_element_id: None,
                },
            )]),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();

        // Tick to park
        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();
        assert!(matches!(fiber.wait, WaitState::Race { .. }));

        // First resolve — succeeds
        let result1 = vm
            .resolve_race(&mut instance, &mut fiber, 0, 1, &arms)
            .await
            .unwrap();
        assert_eq!(result1, Some(1), "First resolve should succeed");

        // Second resolve (duplicate) — returns None, fiber is no longer in Race
        let result2 = vm
            .resolve_race(&mut instance, &mut fiber, 0, 1, &arms)
            .await
            .unwrap();
        assert!(
            result2.is_none(),
            "Duplicate resolve should return None (idempotent)"
        );

        // Fiber state unchanged from first resolve
        assert_eq!(fiber.pc, 1);
        assert_eq!(fiber.wait, WaitState::Running);
    }

    // ═══════════════════════════════════════════════════════════
    //  Phase 2: Boundary timer tests
    // ═══════════════════════════════════════════════════════════

    /// T-BTIMER-3: Job completes before timer → normal path taken.
    #[tokio::test]
    async fn t_btimer_3_job_completes_before_timer() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let race_id: RaceId = 0;
        let program = CompiledProgram {
            bytecode_version: [10u8; 32],
            program: vec![
                Instr::ExecNative {
                    task_type: 0,
                    argc: 0,
                    retc: 0,
                },
                Instr::Jump { target: 2 },
                Instr::End, // normal end
                Instr::End, // escalation end
            ],
            debug_map: BTreeMap::from([(0, "verify_docs".to_string())]),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::from([(
                race_id,
                RacePlanEntry {
                    arms: vec![
                        WaitArm::Internal {
                            kind: 0,
                            key_reg: 0,
                            resume_at: 1,
                        },
                        WaitArm::Timer {
                            duration_ms: 259_200_000,
                            resume_at: 3,
                            interrupting: true,
                            cycle: None,
                        },
                    ],
                    boundary_element_id: None,
                },
            )]),
            boundary_map: BTreeMap::from([(0, race_id)]),
            write_set: BTreeMap::new(),
            task_manifest: vec!["verify_docs".to_string()],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();
        store
            .store_program(program.bytecode_version, &program)
            .await
            .unwrap();

        // Tick — ExecNative parks on Job
        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();
        let outcome = vm
            .tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            TickOutcome::Parked(WaitState::Job { .. })
        ));

        // Capture actual job_key
        let actual_job_key = match &fiber.wait {
            WaitState::Job { job_key } => job_key.clone(),
            _ => panic!("Expected Job wait state"),
        };

        // Simulate promotion: Job → Race, preserving job_key
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        fiber.wait = WaitState::Race {
            race_id,
            timer_deadline_ms: Some(now + 259_200_000),
            job_key: Some(actual_job_key),
            interrupting: true,
            timer_arm_index: Some(1),
            cycle_remaining: None,
            cycle_fired_count: 0,
        };
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();

        // Job completes — resolve race with Internal arm (index 0)
        let result = vm
            .resolve_race(
                &mut instance,
                &mut fiber,
                race_id,
                0,
                &program.race_plan[&race_id].arms,
            )
            .await
            .unwrap();

        assert_eq!(result, Some(1), "Resume at addr 1 (normal)");
        assert_eq!(fiber.pc, 1);
        assert_eq!(fiber.wait, WaitState::Running);

        let outcome = vm
            .run_fiber(&mut fiber, &mut instance, &program, 10)
            .await
            .unwrap();
        assert!(matches!(outcome, TickOutcome::Ended));
    }

    /// T-BTIMER-4: Timer fires before job → escalation path; late completion ignored.
    #[tokio::test]
    async fn t_btimer_4_timer_fires_before_job() {
        let store = Arc::new(MemoryStore::new());
        let vm = Vm::new(store.clone());

        let race_id: RaceId = 0;
        let program = CompiledProgram {
            bytecode_version: [11u8; 32],
            program: vec![
                Instr::ExecNative {
                    task_type: 0,
                    argc: 0,
                    retc: 0,
                },
                Instr::Jump { target: 2 },
                Instr::End,
                Instr::End,
            ],
            debug_map: BTreeMap::from([(0, "verify_docs".to_string())]),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::from([(
                race_id,
                RacePlanEntry {
                    arms: vec![
                        WaitArm::Internal {
                            kind: 0,
                            key_reg: 0,
                            resume_at: 1,
                        },
                        WaitArm::Timer {
                            duration_ms: 1,
                            resume_at: 3,
                            interrupting: true,
                            cycle: None,
                        },
                    ],
                    boundary_element_id: None,
                },
            )]),
            boundary_map: BTreeMap::from([(0, race_id)]),
            write_set: BTreeMap::new(),
            task_manifest: vec!["verify_docs".to_string()],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };

        let mut instance = make_instance();
        store.save_instance(&instance).await.unwrap();
        store
            .store_program(program.bytecode_version, &program)
            .await
            .unwrap();

        let mut fiber = Fiber::new(Uuid::now_v7(), 0);
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();
        vm.tick_fiber(&mut fiber, &mut instance, &program)
            .await
            .unwrap();

        let actual_job_key = match &fiber.wait {
            WaitState::Job { job_key } => job_key.clone(),
            _ => panic!("Expected Job wait state"),
        };

        // Promote with expired deadline, preserving job_key
        fiber.wait = WaitState::Race {
            race_id,
            timer_deadline_ms: Some(0),
            job_key: Some(actual_job_key.clone()),
            interrupting: true,
            timer_arm_index: Some(1),
            cycle_remaining: None,
            cycle_fired_count: 0,
        };
        store
            .save_fiber(instance.instance_id, &fiber)
            .await
            .unwrap();

        // Timer wins
        let result = vm
            .resolve_race(
                &mut instance,
                &mut fiber,
                race_id,
                1,
                &program.race_plan[&race_id].arms,
            )
            .await
            .unwrap();

        assert_eq!(result, Some(3), "Resume at addr 3 (escalation)");

        // Ack the job using stored key
        store.ack_job(&actual_job_key).await.unwrap();

        let outcome = vm
            .run_fiber(&mut fiber, &mut instance, &program, 10)
            .await
            .unwrap();
        assert!(matches!(outcome, TickOutcome::Ended));

        // Late completion ignored
        let late = vm
            .resolve_race(
                &mut instance,
                &mut fiber,
                race_id,
                0,
                &program.race_plan[&race_id].arms,
            )
            .await
            .unwrap();
        assert!(late.is_none(), "Late completion should be ignored");
    }

    /// T-BTIMER-5: Verifier rejects non-interrupting + multiple timers per task.
    #[test]
    fn t_btimer_5_verifier_rejects_invalid() {
        use bpmn_lite_compiler::parser::parse_bpmn;
        use bpmn_lite_compiler::verifier;

        // 5a: non-interrupting boundary timer is valid (Phase 2A)
        let xml_ni = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="proc1" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:serviceTask id="task1" name="Do Work">
              <bpmn:extensionElements><zeebe:taskDefinition type="do_work" /></bpmn:extensionElements>
            </bpmn:serviceTask>
            <bpmn:boundaryEvent id="reminder" attachedToRef="task1" cancelActivity="false">
              <bpmn:timerEventDefinition><bpmn:timeDuration>P1D</bpmn:timeDuration></bpmn:timerEventDefinition>
            </bpmn:boundaryEvent>
            <bpmn:endEvent id="end" />
            <bpmn:serviceTask id="notify" name="Notify">
              <bpmn:extensionElements><zeebe:taskDefinition type="send_reminder" /></bpmn:extensionElements>
            </bpmn:serviceTask>
            <bpmn:endEvent id="end_r" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
            <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
            <bpmn:sequenceFlow id="f3" sourceRef="reminder" targetRef="notify" />
            <bpmn:sequenceFlow id="f4" sourceRef="notify" targetRef="end_r" />
          </bpmn:process>
        </bpmn:definitions>"#;

        let g1 = parse_bpmn(xml_ni).unwrap();
        let e1 = verifier::verify(&g1);
        assert!(
            e1.is_empty(),
            "Non-interrupting boundary timer should pass verification. Errors: {:?}",
            e1
        );

        // 5b: multiple boundary timers on same task rejected
        let xml_multi = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="proc1" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:serviceTask id="task1" name="Do Work">
              <bpmn:extensionElements><zeebe:taskDefinition type="do_work" /></bpmn:extensionElements>
            </bpmn:serviceTask>
            <bpmn:boundaryEvent id="t_a" attachedToRef="task1" cancelActivity="true">
              <bpmn:timerEventDefinition><bpmn:timeDuration>P1D</bpmn:timeDuration></bpmn:timerEventDefinition>
            </bpmn:boundaryEvent>
            <bpmn:boundaryEvent id="t_b" attachedToRef="task1" cancelActivity="true">
              <bpmn:timerEventDefinition><bpmn:timeDuration>P3D</bpmn:timeDuration></bpmn:timerEventDefinition>
            </bpmn:boundaryEvent>
            <bpmn:endEvent id="end" />
            <bpmn:endEvent id="end_a" />
            <bpmn:endEvent id="end_b" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
            <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
            <bpmn:sequenceFlow id="f3" sourceRef="t_a" targetRef="end_a" />
            <bpmn:sequenceFlow id="f4" sourceRef="t_b" targetRef="end_b" />
          </bpmn:process>
        </bpmn:definitions>"#;

        let g2 = parse_bpmn(xml_multi).unwrap();
        let e2 = verifier::verify(&g2);
        assert!(
            e2.iter()
                .any(|e| e.message.contains("boundary timers") && e.message.contains("max 1")),
            "Should reject multi-timer. Errors: {:?}",
            e2
        );
    }
}
