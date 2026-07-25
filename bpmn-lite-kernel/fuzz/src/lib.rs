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
//!   O2 K-invariants    — `check_k_invariants` after every accepted step
//!   O4 limits          — observed peaks never exceed `VerifiedLimits`
//!   O5 terminate       — `Terminate` succeeds on any reachable
//!                        non-terminal state
//!
//! Known generator gaps (no silent caps): guard/race/wait/MI opcodes are
//! emitted only via the low-weight hostile arm, not correct-by-
//! construction, so their *admitted* forms are underrepresented until the
//! generator grows dedicated block shapes for them; `TimerFired` claims
//! are synthesized with `TimerKind::Wait` only.

use std::collections::BTreeMap;

use bpmn_lite_kernel::DeterministicContext;
use bpmn_lite_types::ffi_bindings::{BindingSource, Literal};
use bpmn_lite_types::session_stack::SessionStackState;
use bpmn_lite_types::{
    Addr, ArtifactEnvelope, ClaimedTimer, ClaimedTimerIdentity, Command, CommandEnvelope,
    CompiledProgram, ConcurrencyTable, EffectId, EffectOutput, ErrorClass, ExecutableWorkflow,
    Fiber, Instr, JournalCommand, JournalRecord, PersistedSnapshotState, ProcessInstance,
    ProcessState, RecordState, SnapshotEnvelope, TenantId, TimerKind,
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
    emit_region(&mut instrs, tape, 2);
    instrs.push(Instr::End);

    let mut program = bpmn_lite_types::legacy_program! {
        bytecode_version: [7u8; 32],
        program: instrs,
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![],
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    // Hostile-metadata probe (R3/V8.3 admission checks): a corr source
    // keyed to a random address is only admissible if that address happens
    // to hold a message word — which this generator never emits, so this
    // arm is a pure reject-path probe today.
    if tape.u8() % 8 == 0 {
        let mut sources = BTreeMap::new();
        sources.insert(
            Addr::new(u32::from(tape.u16()) % ADDR_PROBE_RANGE),
            BindingSource::Literal(Literal::Bool(false)),
        );
        program = program.with_v2_corr_sources(sources);
    }
    program
}

fn emit_region(instrs: &mut Vec<Instr>, tape: &mut Tape, fork_depth: u8) {
    let blocks = 1 + tape.u8() % 3;
    for _ in 0..blocks {
        if tape.exhausted() || instrs.len() >= MAX_PROGRAM_LEN {
            return;
        }
        match tape.u8() % 10 {
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
            5..=7 if fork_depth > 0 => emit_fork_join(instrs, tape, fork_depth - 1),
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
            _ => {}
        }
    }
}

/// Correct-by-construction SESE fork/join block mirroring the kernel
/// fixture's canonical shape: fork targets branch starts; every branch
/// ends `V2Join{pairing: fork addr}` + `Jump{continuation}`.
fn emit_fork_join(instrs: &mut Vec<Instr>, tape: &mut Tape, fork_depth: u8) {
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
            emit_region(instrs, tape, fork_depth);
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

pub fn initial_envelope(workflow: &ExecutableWorkflow) -> SnapshotEnvelope {
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
                    TimerKind::Wait,
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
        let Ok(transition) = bpmn_lite_kernel::apply(workflow, &snapshot, &command, &context)
        else {
            // A clean reject is a legal outcome for a hostile command.
            continue;
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
