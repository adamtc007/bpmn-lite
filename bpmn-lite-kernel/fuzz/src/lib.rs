//! Shared byte-tape generator + oracle stepper for the kernel fuzz targets
//! (EOP-FUZZ-BPMN-ISA-002 F2, fork F-A: generators live here, production
//! crates carry no `arbitrary` impls).
//!
//! The tape is the libFuzzer input interpreted as a decision stream:
//! first to build a structurally plausible (not always valid!) program via
//! the same public constructors the compiler uses, then — for programs the
//! real admission path (`ArtifactEnvelope::from_legacy_program` →
//! `ExecutableWorkflow::from_verified_envelope`, fork F-B) accepts — to
//! drive `kernel::apply` with an adversarial command sequence under the
//! plan's oracles:
//!
//!   O1 no-panic        — any panic below is a finding (libFuzzer oracle)
//!   O2 K-invariants    — `check_k_invariants` after every accepted step;
//!                        a `TransitionError::Integrity` reject is ALSO a
//!                        finding, not a legal reject — Ring 3 fires only
//!                        on a frame the kernel itself computed, and every
//!                        snapshot in this harness is kernel-produced
//!   O4 limits          — observed peaks never exceed `VerifiedLimits`
//!   O5 terminate       — `Terminate` succeeds on any reachable
//!                        non-terminal state
//!   O7 quiescence      — a non-terminal end state retains an external
//!                        unblock channel (`check_progressable`); an
//!                        all-fibres-barrier-parked frame is a deadlock
//!                        sinkhole the verifier's SESE proofs claim to
//!                        exclude — verifier-soundness class, same as O4
//!
//! Known generator gaps (no silent caps): MI opcodes still lack a
//! correct-by-construction block shape (they need collection setup; the
//! path is covered end-to-end by F5's multi-instance fixture);
//! guard/race/wait blocks ARE correct-by-construction since the F5
//! ruling, and `TimerFired` claims are synthesized with both
//! `TimerKind::Wait` and `TimerKind::Race`.

use std::collections::BTreeMap;

use bpmn_lite_kernel::{DeterministicContext, TransitionError};
use bpmn_lite_types::{BindingSource, Literal};
use bpmn_lite_types::session_stack::SessionStackState;
use bpmn_lite_types::{
    Addr, ArtifactEnvelope, ClaimedTimer, ClaimedTimerIdentity, Command, CommandEnvelope,
    CompiledProgram, ConcurrencyTable, EffectId, EffectOutput, ErrorClass, ExecutableWorkflow,
    Fiber, Instr, JournalCommand, JournalRecord, PersistedSnapshotState, ProcessInstance,
    ProcessState, RecordState, SnapshotEnvelope, TenantId, TimerKind, WaitState,
};
use uuid::Uuid;

/// Hard cap on generated program length; hostile jump targets are drawn
/// modulo a slightly larger range so out-of-range addresses stay reachable
/// as verifier-reject probes.
const MAX_PROGRAM_LEN: usize = 80;
const ADDR_PROBE_RANGE: u32 = 96;
const MAX_STEPS: usize = 48;

pub struct Tape<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Tape<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn u8(&mut self) -> u8 {
        let byte = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        byte
    }

    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes([self.u8(), self.u8()])
    }

    pub fn bool(&mut self) -> bool {
        self.u8() & 1 == 1
    }

    pub fn exhausted(&self) -> bool {
        self.pos >= self.data.len()
    }
}

/// Build a structurally plausible program from the tape. Deliberately
/// emits boundary garbage (dangling jumps, unpaired joins, stack probes,
/// corr-source metadata keyed off message words) at a tuned rate so both
/// the admission-reject and admit paths stay hot.
pub fn gen_program(tape: &mut Tape) -> CompiledProgram {
    let mut instrs = Vec::new();
    let mut msg_words = Vec::new();
    emit_region(&mut instrs, tape, 2, true, &mut msg_words);
    instrs.push(Instr::End);

    let mut program = bpmn_lite_types::legacy_program! {
        bytecode_version: [7u8; 32],
        program: instrs,
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::from([
            (0u32, "msg-0".to_string()),
            (1u32, "msg-1".to_string()),
            (2u32, "msg-2".to_string()),
            (3u32, "msg-3".to_string()),
        ]),
        write_set: BTreeMap::new(),
        task_manifest: vec![],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    // Corr-source metadata probe (R3/V8.3 admission checks): keyed either
    // to a REAL message word emitted above (must ADMIT — exercises the
    // accept branch of the R3 check) or to a random address (reject probe).
    if tape.u8() % 4 == 0 {
        let mut sources = BTreeMap::new();
        let address = if !msg_words.is_empty() && tape.bool() {
            msg_words[usize::from(tape.u8()) % msg_words.len()]
        } else {
            Addr::new(u32::from(tape.u16()) % ADDR_PROBE_RANGE)
        };
        sources.insert(address, BindingSource::Literal(Literal::Bool(false)));
        program = program.with_v2_corr_sources(sources);
    }
    program
}

pub fn emit_region(
    instrs: &mut Vec<Instr>,
    tape: &mut Tape,
    fork_depth: u8,
    top_level: bool,
    msg_words: &mut Vec<Addr>,
) {
    let blocks = 1 + tape.u8() % 3;
    for _ in 0..blocks {
        if tape.exhausted() || instrs.len() >= MAX_PROGRAM_LEN {
            return;
        }
        match tape.u8() % 16 {
            0..=4 => match tape.u8() % 3 {
                0 => {
                    instrs.push(Instr::PushBool(tape.bool()));
                    instrs.push(Instr::Pop);
                }
                1 => instrs.push(Instr::IncCounter {
                    counter_id: u32::from(tape.u8() % 4),
                }),
                _ => {
                    instrs.push(Instr::PushI64(i64::from(tape.u8())));
                    instrs.push(Instr::Pop);
                }
            },
            5..=7 if fork_depth > 0 => emit_fork_join(instrs, tape, fork_depth - 1, msg_words),
            8 => match tape.u8() % 4 {
                // Hostile arm: tape-driven control flow that the verifier
                // must either prove sound or reject — never admit-and-crash.
                0 => instrs.push(Instr::Jump {
                    target: Addr::new(u32::from(tape.u16()) % ADDR_PROBE_RANGE),
                }),
                1 => instrs.push(Instr::V2Join {
                    pairing: Addr::new(u32::from(tape.u16()) % ADDR_PROBE_RANGE),
                }),
                2 => instrs.push(Instr::BrIf {
                    target: Addr::new(u32::from(tape.u16()) % ADDR_PROBE_RANGE),
                }),
                _ => instrs.push(Instr::Fail {
                    code: u32::from(tape.u8()),
                }),
            },
            // Wait words (r.1: duration/deadline on the operand stack).
            9 => {
                instrs.push(Instr::PushI64(1 + i64::from(tape.u8())));
                instrs.push(Instr::V2WaitFor);
            }
            10 => {
                let name = u32::from(tape.u8() % 4);
                msg_words.push(Addr::new(instrs.len() as u32));
                instrs.push(Instr::V2WaitMsg { name });
            }
            11 => emit_race(instrs, tape, msg_words),
            // Guard family: handler-entry semantics differ per kind (V-4),
            // and a plain-guard handler is entered PRE-push, so its `End`
            // only admits when the surrounding stack is empty — hence
            // top-level only.
            12 | 13 if top_level => {
                let variant = tape.u8() % 3;
                emit_guard(instrs, tape, fork_depth, variant, msg_words)
            }
            _ => {}
        }
    }
}

/// Guard block in the verifier-canonical shape (EX-oracle fixture /
/// v4_guard_handler_entry_state tests):
///
///   [PushI64 dur]?          (only for the timer-armed variant)
///   V2Guard{handler: H}     (or V2GuardN)
///   [V2GuardArmTimer]?      (must IMMEDIATELY follow its guard open)
///   <body region>
///   V2GuardEnd / V2GuardNEnd
///   Jump cont               (skip the handler)
///   H: handler — plain guard enters PRE-push (End on empty stack);
///      GuardN enters POST-push (V2GuardNEnd pops own token, then End)
///   cont:
fn emit_guard(
    instrs: &mut Vec<Instr>,
    tape: &mut Tape,
    fork_depth: u8,
    variant: u8,
    msg_words: &mut Vec<Addr>,
) {
    let non_interrupting = variant == 1;
    let timer_armed = variant == 2;
    if timer_armed {
        instrs.push(Instr::PushI64(1 + i64::from(tape.u8())));
    }
    let guard_at = instrs.len();
    instrs.push(if non_interrupting {
        Instr::V2GuardN { handler: Addr::new(0) }
    } else {
        Instr::V2Guard { handler: Addr::new(0) }
    });
    if timer_armed {
        instrs.push(Instr::V2GuardArmTimer);
    }
    emit_region(instrs, tape, fork_depth, false, msg_words);
    instrs.push(if non_interrupting {
        Instr::V2GuardNEnd
    } else {
        Instr::V2GuardEnd
    });
    let skip_at = instrs.len();
    instrs.push(Instr::Jump { target: Addr::new(0) });
    let handler = Addr::new(instrs.len() as u32);
    if non_interrupting {
        instrs.push(Instr::V2GuardNEnd);
    }
    instrs.push(Instr::End);
    let continuation = Addr::new(instrs.len() as u32);
    instrs[guard_at] = if non_interrupting {
        Instr::V2GuardN { handler }
    } else {
        Instr::V2Guard { handler }
    };
    instrs[skip_at] = Instr::Jump {
        target: continuation,
    };
}

/// Race block in the EX-oracle fixture's canonical shape: open, timer arm
/// (duration pushed first, r.1), message arm, close; arm resume targets
/// jump to the continuation past the close.
fn emit_race(instrs: &mut Vec<Instr>, tape: &mut Tape, msg_words: &mut Vec<Addr>) {
    let name = u32::from(tape.u8() % 4);
    instrs.push(Instr::V2RaceOpen { arm_count: 2 });
    instrs.push(Instr::PushI64(1 + i64::from(tape.u8())));
    let arm_timer_at = instrs.len();
    instrs.push(Instr::V2ArmTimer { target: Addr::new(0) });
    let arm_msg_at = instrs.len();
    msg_words.push(Addr::new(arm_msg_at as u32));
    instrs.push(Instr::V2ArmMsg { target: Addr::new(0), name });
    instrs.push(Instr::V2RaceClose);
    let timer_resume = instrs.len();
    instrs.push(Instr::Jump { target: Addr::new(0) });
    let msg_resume = instrs.len();
    instrs.push(Instr::Jump { target: Addr::new(0) });
    let continuation = Addr::new(instrs.len() as u32);
    instrs[arm_timer_at] = Instr::V2ArmTimer {
        target: Addr::new(timer_resume as u32),
    };
    instrs[arm_msg_at] = Instr::V2ArmMsg {
        target: Addr::new(msg_resume as u32),
        name,
    };
    instrs[timer_resume] = Instr::Jump {
        target: continuation,
    };
    instrs[msg_resume] = Instr::Jump {
        target: continuation,
    };
}

/// Correct-by-construction SESE fork/join block mirroring the kernel
/// fixture's canonical shape: fork targets branch starts; every branch
/// ends `V2Join{pairing: fork addr}` + `Jump{continuation}`.
fn emit_fork_join(
    instrs: &mut Vec<Instr>,
    tape: &mut Tape,
    fork_depth: u8,
    msg_words: &mut Vec<Addr>,
) {
    let branch_count = 2 + usize::from(tape.u8() % 2);
    let fork_at = instrs.len();
    instrs.push(Instr::V2Fork {
        targets: Vec::new().into(),
        pairing: Addr::new(fork_at as u32),
    });
    let mut branch_starts = Vec::new();
    let mut jump_slots = Vec::new();
    for _ in 0..branch_count {
        branch_starts.push(Addr::new(instrs.len() as u32));
        if fork_depth > 0 && tape.bool() {
            emit_region(instrs, tape, fork_depth, false, msg_words);
        }
        instrs.push(Instr::V2Join {
            pairing: Addr::new(fork_at as u32),
        });
        jump_slots.push(instrs.len());
        instrs.push(Instr::Jump { target: Addr::new(0) });
    }
    let continuation = Addr::new(instrs.len() as u32);
    instrs[fork_at] = Instr::V2Fork {
        targets: branch_starts.into(),
        pairing: Addr::new(fork_at as u32),
    };
    for slot in jump_slots {
        instrs[slot] = Instr::Jump {
            target: continuation,
        };
    }
}

/// The real public admission path (fork F-B): no fuzz-only entry.
pub fn admit(program: CompiledProgram) -> Option<ExecutableWorkflow> {
    let envelope = ArtifactEnvelope::from_legacy_program(program, "kernel-fuzz").ok()?;
    ExecutableWorkflow::from_verified_envelope(envelope).ok()
}

fn initial_envelope(workflow: &ExecutableWorkflow) -> SnapshotEnvelope {
    let instance_id = Uuid::from_u128(0xF0);
    let fiber_id = Uuid::from_u128(0xF1);
    let instance = ProcessInstance {
        instance_id,
        tenant_id: "tenant-a".to_string(),
        process_key: "kernel-fuzz".to_string(),
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
    let state = PersistedSnapshotState::new(
        instance,
        [Fiber::new(fiber_id, 0)],
        BTreeMap::new(),
        [],
        ConcurrencyTable::default(),
        [],
    );
    SnapshotEnvelope::new(
        workflow.envelope().abi_version(),
        workflow.hash().into_bytes(),
        0,
        state,
    )
}

fn gen_command(tape: &mut Tape, state: &PersistedSnapshotState) -> Command {
    let fibers: Vec<Uuid> = state.fibers().keys().copied().collect();
    let pick_fiber = |tape: &mut Tape| -> Uuid {
        if fibers.is_empty() {
            Uuid::from_u128(0xDEAD)
        } else {
            fibers[usize::from(tape.u8()) % fibers.len()]
        }
    };
    let instance_id = state.instance().instance_id;
    match tape.u8() % 12 {
        0..=4 => Command::Tick {
            fiber_id: if tape.bool() {
                None
            } else {
                Some(pick_fiber(tape))
            },
        },
        5 => {
            let fiber_id = pick_fiber(tape);
            let fiber_pc = state
                .fibers()
                .get(&fiber_id)
                .map_or(0, |fiber| fiber.pc.index() as u32);
            // Half targeted (an EffectId the kernel could actually be
            // waiting on), half wild.
            let effect_id = if tape.bool() {
                EffectId::for_instruction(instance_id, fiber_id, fiber_pc)
            } else {
                EffectId::from_uuid(Uuid::from_u128(u128::from(tape.u16())))
            };
            Command::EffectCompleted {
                effect_id,
                output: EffectOutput::Json(serde_json::json!({ "k": tape.u8() })),
            }
        }
        6 => Command::EffectFailed {
            effect_id: EffectId::from_uuid(Uuid::from_u128(u128::from(tape.u16()))),
            job_key: format!("job-{}", tape.u8()),
            error_class: match tape.u8() % 3 {
                0 => ErrorClass::Transient,
                1 => ErrorClass::ContractViolation,
                _ => ErrorClass::BusinessRejection {
                    rejection_code: format!("R{}", tape.u8()),
                },
            },
            message: "fuzz failure".to_string(),
            retry: None,
            attempt: u32::from(tape.u8()),
        },
        7 => Command::MessageDelivered {
            message_id: format!("m-{}", tape.u8()),
            name: format!("msg-{}", tape.u8() % 4),
            correlation_key: "corr".to_string(),
            payload: b"{}".to_vec(),
            payload_hash: None,
            expires_at: i64::from(tape.u16()),
        },
        8 => {
            let fiber_id = pick_fiber(tape);
            Command::TimerFired {
                timer: ClaimedTimer::new(
                    ClaimedTimerIdentity::new(
                        TenantId::new("tenant-a").expect("static tenant id is non-empty"),
                        EffectId::from_uuid(Uuid::from_u128(u128::from(tape.u16()))),
                        instance_id,
                        fiber_id,
                    ),
                    u64::from(tape.u16()),
                    if tape.bool() {
                        TimerKind::Wait
                    } else {
                        TimerKind::Race {
                            race_id: u32::from(tape.u8()),
                            arm_index: usize::from(tape.u8() % 2),
                            resume_at: u32::from(tape.u8()),
                            interrupting: tape.bool(),
                            job_key: None,
                            boundary_element_id: None,
                            arm_count: 2,
                        }
                    },
                    None,
                    Uuid::from_u128(0xC1A1),
                ),
                fired_at: u64::from(tape.u16()),
            }
        }
        9 => Command::Cancel {
            reason: "fuzz cancel".to_string(),
        },
        10 => Command::ResolveIncident {
            incident_id: state
                .incidents()
                .keys()
                .next()
                .copied()
                .unwrap_or_else(|| Uuid::from_u128(u128::from(tape.u16()))),
            resolution: "fuzz resolution".to_string(),
        },
        _ => Command::Terminate,
    }
}

/// O7 — structural quiescence check: a non-terminal state is progressable
/// iff some EXTERNAL channel can still advance it. Internal-only waits
/// (`V2Barrier`, v1 `Join`) are advanced solely by sibling fibres
/// arriving, so a frame where every live fibre is barrier-parked — and no
/// incident is open (`ResolveIncident` is a channel) — can never move
/// again. Soundness (no false positives, given K-invariants): every armed
/// barrier has count ≥ 1 (K-3), so ≥ 1 member has not arrived and is
/// parked at a DIFFERENT barrier; finitely many fibres forces that
/// dependency chain into a cycle — a genuine deadlock. Structural on
/// purpose: it derives progressability from `WaitState` alone rather than
/// probing `apply` with synthesized commands, so command-validation
/// strictness cannot fake a sinkhole. Scope (no silent gaps): it does NOT
/// catch semantically-dead external waits (a message name nothing will
/// publish, a timer no scheduler tracks) — those need the P2 differential
/// oracle.
fn check_progressable(
    fibers: &BTreeMap<Uuid, Fiber>,
    has_open_incident: bool,
) -> Result<(), String> {
    if fibers.is_empty() {
        return Err("zero live fibres in a non-terminal state".to_string());
    }
    if has_open_incident {
        return Ok(());
    }
    let externally_unblockable = fibers
        .values()
        .any(|fiber| !matches!(fiber.wait, WaitState::V2Barrier { .. } | WaitState::Join { .. }));
    if externally_unblockable {
        Ok(())
    } else {
        Err(format!(
            "all {} live fibres are parked on internal barriers — no external unblock channel",
            fibers.len()
        ))
    }
}

pub struct StepOutcome {
    pub genesis: SnapshotEnvelope,
    pub final_envelope: SnapshotEnvelope,
    /// One record per accepted transition, in order — a valid journal tail
    /// from `genesis`, built with the same context values `replay` will
    /// re-derive. Empty-and-incomplete if any `state_hash` was uncomputable
    /// (hostile float payloads); `journal_complete` says which.
    pub journal: Vec<JournalRecord>,
    pub journal_complete: bool,
}

/// Drive an admitted workflow with a tape-derived command sequence,
/// asserting O2/O4 after every accepted transition and O5 at the end.
pub fn step_workflow(workflow: &ExecutableWorkflow, tape: &mut Tape) -> StepOutcome {
    let limits = workflow.envelope().limits();
    let abi = workflow.envelope().abi_version();
    let genesis = initial_envelope(workflow);
    let mut current = genesis.clone();
    let mut revision = 0u64;
    let mut journal = Vec::new();
    let mut journal_complete = true;

    for step in 0..MAX_STEPS {
        if current.state().instance().state.is_terminal() {
            break;
        }
        let command = gen_command(tape, current.state());
        let context = DeterministicContext::new(
            100 + step as u64,
            Uuid::from_u128(0x1000 + step as u128),
            revision + 1,
        );
        let snapshot = current.state().to_runtime_snapshot();
        let transition = match bpmn_lite_kernel::apply(workflow, &snapshot, &command, &context) {
            Ok(transition) => transition,
            // `Integrity` is emitted ONLY by the Ring 3 shadow check over
            // the frame the kernel itself computed (entry validation uses
            // `ResourceLimitExceeded`), and every snapshot here is
            // kernel-produced — so this reject is production fail-closing
            // on a kernel defect, and letting it `continue` would mask a
            // finding as a clean reject.
            Err(TransitionError::Integrity(violation)) => {
                panic!("O2: kernel computed a Ring 3-invalid frame (masked as a reject): {violation}")
            }
            // Any other reject is a legal outcome for a hostile command.
            Err(_) => continue,
        };
        let prior_state_hash = current.state_hash();
        let prior_revision = revision as i64;
        revision += 1;
        current = bpmn_lite_kernel::materialize_snapshot(current.state(), &transition, abi, revision);
        match (prior_state_hash, current.state_hash()) {
            (Ok(prior_hash), Ok(new_hash)) if journal_complete => {
                journal.push(JournalRecord::new(
                    CommandEnvelope::new(
                        context.command_id(),
                        context.logical_time() as i64,
                        JournalCommand::Kernel(command.clone()),
                    ),
                    prior_revision,
                    revision,
                    workflow.hash().into_bytes(),
                    prior_hash,
                    new_hash,
                    transition.events(),
                    transition.effects(),
                ));
            }
            _ => {
                journal_complete = false;
                journal.clear();
            }
        }

        // O2 — the K-1..K-3 theorems hold after every accepted transition.
        if let Err(message) = bpmn_lite_kernel::check_k_invariants(
            current.state().fibers(),
            current.state().concurrency_table(),
        ) {
            panic!("O2: K-invariant violated after an admitted transition: {message}");
        }

        // O4 — verifier soundness: observed peaks never exceed the
        // envelope's VerifiedLimits. A runtime excursion above a verified
        // bound is a verifier bug, not a runtime one.
        let fibers = current.state().fibers();
        assert!(
            fibers.len() as u32 <= limits.max_fibers(),
            "O4: {} live fibres exceeds verified max_fibers {}",
            fibers.len(),
            limits.max_fibers()
        );
        for fiber in fibers.values() {
            let depth = bpmn_lite_kernel::effective_control_stack(fiber).len() as u32;
            assert!(
                depth <= limits.max_control_depth(),
                "O4: control depth {depth} exceeds verified max_control_depth {}",
                limits.max_control_depth()
            );
            assert!(
                fiber.stack.len() as u32 <= limits.max_stack(),
                "O4: operand stack {} exceeds verified max_stack {}",
                fiber.stack.len(),
                limits.max_stack()
            );
        }
        let armed = current
            .state()
            .concurrency_table()
            .iter()
            .filter(|(_, record)| record.state == RecordState::Armed)
            .count() as u32;
        assert!(
            armed <= limits.max_records(),
            "O4: {armed} armed records exceeds verified max_records {}",
            limits.max_records()
        );
    }

    // O7 — quiescence: a non-terminal end state must retain an external
    // unblock channel or a runnable fibre. All-fibres-parked-on-internal-
    // barriers (a cross-barrier wait cycle) can never move again; the
    // verifier's SESE proofs claim to exclude that shape at admission, so
    // reaching it is a verifier-soundness defect, same class as O4.
    if !current.state().instance().state.is_terminal() {
        if let Err(message) = check_progressable(
            current.state().fibers(),
            !current.state().incidents().is_empty(),
        ) {
            panic!("O7: deadlock sinkhole in a non-terminal end state: {message}");
        }
    }

    // O5 — Terminate must succeed on ANY reachable non-terminal state
    // (documented poisoned-instance discipline; #93's terminal-command
    // escape hatch).
    if !current.state().instance().state.is_terminal() {
        let context =
            DeterministicContext::new(9_999, Uuid::from_u128(0x9999), revision + 1);
        let snapshot = current.state().to_runtime_snapshot();
        bpmn_lite_kernel::apply(workflow, &snapshot, &Command::Terminate, &context)
            .expect("O5: Terminate rejected on a reachable non-terminal state");
    }

    StepOutcome {
        genesis,
        final_envelope: current,
        journal,
        journal_complete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F2 receipt — generator tuning gate from the ratified plan: over a
    /// deterministic pseudo-random tape population, at least 30% of
    /// generated programs must pass the real admission path, else the
    /// fuzzer wastes its budget on rejects and `apply` coverage stays
    /// cold. Deterministic LCG, so this is a cement test, not a flake.
    #[test]
    fn generator_admission_rate_is_at_least_30_percent() {
        let mut admitted = 0u32;
        const RUNS: u32 = 1_000;
        let mut lcg: u64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..RUNS {
            let bytes: Vec<u8> = (0..256)
                .map(|_| {
                    lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    (lcg >> 33) as u8
                })
                .collect();
            let mut tape = Tape::new(&bytes);
            if admit(gen_program(&mut tape)).is_some() {
                admitted += 1;
            }
        }
        let rate = admitted * 100 / RUNS;
        assert!(
            rate >= 30,
            "admission rate {rate}% < 30% ({admitted}/{RUNS}) — generator needs retuning"
        );
    }

    /// Each correct-by-construction block shape must be admitted STANDALONE
    /// — a per-construct receipt, so a verifier-canonical-shape drift shows
    /// up as a named failure here, not as a silent admission-rate collapse.
    #[test]
    fn every_generator_block_shape_is_admitted_standalone() {
        // (label, tape bytes that force one specific block then padding)
        // emit_region reads: blocks=(b0%3)+1 → 1 block; selector b1%16.
        let cases: Vec<(&str, Vec<u8>)> = vec![
            // selector 9: PushI64+V2WaitFor
            ("wait_for", vec![0, 9, 5]),
            // selector 10: V2WaitMsg
            ("wait_msg", vec![0, 10, 1]),
            // selector 11: race block
            ("race", vec![0, 11, 2, 7]),
            // selector 12, variant 0: plain guard, empty body
            ("guard", vec![0, 12, 0, 0, 15]),
            // selector 12, variant 1: GuardN, empty body
            ("guard_n", vec![0, 12, 1, 0, 15]),
            // selector 12, variant 2: timer-armed guard, empty body
            ("guard_arm_timer", vec![0, 12, 2, 9, 0, 15]),
            // selector 5 (fork), branch-body bools read the 2-padding (false)
            ("fork_join", vec![0, 5, 0]),
        ];
        for (label, bytes) in cases {
            // Trailing 2s: an exhausted tape reads 0, and gen_program's
            // final corr-source gate (u8 % 4 == 0) would then fire with a
            // zero address — a deliberate reject probe under fuzzing, but
            // noise here. 2-padding keeps that gate closed (2 % 4 != 0),
            // reads as bool=false for optional branch bodies, and as a
            // benign PushI64/Pop block wherever a body consumes it.
            let bytes: Vec<u8> = bytes.into_iter().chain([2u8; 16]).collect();
            let mut tape = Tape::new(&bytes);
            let program = gen_program(&mut tape);
            assert!(
                admit(program).is_some(),
                "{label}: canonical block shape was REJECTED by admission"
            );
        }
    }

    /// O7 red→green receipt: an all-barrier-parked frame must be flagged
    /// as a sinkhole (red); an open incident, a runnable fibre, or any
    /// externally-unblockable wait clears the same frame (green). Zero
    /// fibres non-terminal is red too (the Ring 3 guard's own shape,
    /// defense-in-depth here).
    #[test]
    fn quiescence_check_flags_all_barrier_parked_frames_only() {
        let record_id = bpmn_lite_types::RecordId::new(Uuid::from_u128(0xB0));
        let barrier_parked = |id: u128| {
            let mut fiber = Fiber::new(Uuid::from_u128(id), 0);
            fiber.wait = WaitState::V2Barrier { record_id };
            (fiber.fiber_id, fiber)
        };
        let all_parked: BTreeMap<_, _> = [barrier_parked(1), barrier_parked(2)].into();
        assert!(
            check_progressable(&all_parked, false).is_err(),
            "red: all-barrier-parked frame not flagged as a sinkhole"
        );
        assert!(
            check_progressable(&all_parked, true).is_ok(),
            "green: an open incident is an external channel"
        );

        let mut with_runner = all_parked.clone();
        let runner = Fiber::new(Uuid::from_u128(3), 0);
        with_runner.insert(runner.fiber_id, runner);
        assert!(
            check_progressable(&with_runner, false).is_ok(),
            "green: a runnable fibre is an external channel"
        );

        assert!(
            check_progressable(&BTreeMap::new(), false).is_err(),
            "red: zero live fibres in a non-terminal state"
        );
    }

    /// Admitted programs must step without tripping any oracle on benign
    /// tapes — the green half of the harness's own receipt.
    #[test]
    fn admitted_programs_step_clean_under_oracles() {
        let mut stepped = 0u32;
        let mut lcg: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..200 {
            let bytes: Vec<u8> = (0..512)
                .map(|_| {
                    lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    (lcg >> 33) as u8
                })
                .collect();
            let mut tape = Tape::new(&bytes);
            if let Some(workflow) = admit(gen_program(&mut tape)) {
                let _ = step_workflow(&workflow, &mut tape);
                stepped += 1;
            }
        }
        assert!(stepped > 0, "no program admitted — stepper untested");
    }
}
