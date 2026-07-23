use crate::{
    Addr, CompiledProgram, FfiTaskDecl, FlagKey, Instr, JoinId, JoinPlanEntry, WaitId,
    WaitPlanEntry,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const CURRENT_ARTIFACT_ABI: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ArtifactHash([u8; 32]);

impl ArtifactHash {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn into_bytes(self) -> [u8; 32] {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedLimits {
    max_stack: u32,
    max_registers: u32,
    max_fibers: u32,
    max_steps: u64,
    /// V-7 (V&S §7): maximum control-stack depth across all fibre-reachable
    /// paths. Typed now; computed by V3's dual-stack abstract interpreter.
    /// Zero until then — a v2-pre-V3 artifact has no D2 words to create
    /// control-stack depth in the first place, so zero is exact, not a
    /// placeholder.
    max_control_depth: u32,
    /// V-7: maximum simultaneously-armed guard/race scopes. Same status as
    /// `max_control_depth`.
    max_barriers: u32,
    /// V-7: maximum simultaneously-live concurrency-table records. Same
    /// status as `max_control_depth`.
    max_records: u32,
}

impl VerifiedLimits {
    pub fn max_stack(&self) -> u32 {
        self.max_stack
    }

    pub fn max_registers(&self) -> u32 {
        self.max_registers
    }

    pub fn max_fibers(&self) -> u32 {
        self.max_fibers
    }

    pub fn max_steps(&self) -> u64 {
        self.max_steps
    }

    pub fn max_control_depth(&self) -> u32 {
        self.max_control_depth
    }

    pub fn max_barriers(&self) -> u32 {
        self.max_barriers
    }

    pub fn max_records(&self) -> u32 {
        self.max_records
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    debug_map: BTreeMap<Addr, String>,
    join_plan: BTreeMap<JoinId, JoinPlanEntry>,
    wait_plan: BTreeMap<WaitId, WaitPlanEntry>,
    message_name_map: BTreeMap<u32, String>,
    write_set: BTreeMap<String, BTreeSet<FlagKey>>,
    task_manifest: Vec<String>,
    flag_symbol_table: BTreeMap<FlagKey, String>,
    data_objects: BTreeMap<String, crate::DataObjectDecl>,
    ffi_task_decls: BTreeMap<Addr, FfiTaskDecl>,
    /// V4 D2 — `V2ArmEffect`/`V2AwaitEffect` binding table. See doc on
    /// `CompiledProgram::v2_ffi_task_decls`.
    v2_ffi_task_decls: BTreeMap<Addr, FfiTaskDecl>,
}

impl ArtifactMetadata {
    pub fn debug_map(&self) -> &BTreeMap<Addr, String> {
        &self.debug_map
    }

    pub fn join_plan(&self) -> &BTreeMap<JoinId, JoinPlanEntry> {
        &self.join_plan
    }

    pub fn wait_plan(&self) -> &BTreeMap<WaitId, WaitPlanEntry> {
        &self.wait_plan
    }

    pub fn message_name_map(&self) -> &BTreeMap<u32, String> {
        &self.message_name_map
    }

    pub fn write_set(&self) -> &BTreeMap<String, BTreeSet<FlagKey>> {
        &self.write_set
    }

    pub fn task_manifest(&self) -> &[String] {
        &self.task_manifest
    }

    pub fn flag_symbol_table(&self) -> &BTreeMap<FlagKey, String> {
        &self.flag_symbol_table
    }

    pub fn data_objects(&self) -> &BTreeMap<String, crate::DataObjectDecl> {
        &self.data_objects
    }

    pub fn ffi_task_decls(&self) -> &BTreeMap<Addr, FfiTaskDecl> {
        &self.ffi_task_decls
    }

    pub fn v2_ffi_task_decls(&self) -> &BTreeMap<Addr, FfiTaskDecl> {
        &self.v2_ffi_task_decls
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactEnvelope {
    abi_version: u32,
    compiler_version: String,
    instructions: Vec<Instr>,
    metadata: ArtifactMetadata,
    limits: VerifiedLimits,
}

impl ArtifactEnvelope {
    pub fn abi_version(&self) -> u32 {
        self.abi_version
    }

    pub fn compiler_version(&self) -> &str {
        &self.compiler_version
    }

    pub fn instructions(&self) -> &[Instr] {
        &self.instructions
    }

    pub fn metadata(&self) -> &ArtifactMetadata {
        &self.metadata
    }

    pub fn limits(&self) -> &VerifiedLimits {
        &self.limits
    }

    #[doc(hidden)]
    pub fn from_legacy_program(
        program: CompiledProgram,
        compiler_version: impl Into<String>,
    ) -> Result<Self, ArtifactError> {
        let write_set = program
            .write_set
            .iter()
            .map(|(name, keys)| (name.clone(), keys.iter().copied().collect()))
            .collect();
        let metadata = ArtifactMetadata {
            debug_map: program.debug_map.clone(),
            join_plan: program.join_plan.clone(),
            wait_plan: program.wait_plan.clone(),
            message_name_map: program.message_name_map.clone(),
            write_set,
            task_manifest: program.task_manifest.clone(),
            flag_symbol_table: program.flag_symbol_table.clone(),
            data_objects: program.data_objects.clone(),
            ffi_task_decls: program.ffi_task_decls.clone(),
            v2_ffi_task_decls: program.v2_ffi_task_decls.clone(),
        };
        let limits = verify_program(&program.program, &metadata)?;
        Ok(Self {
            abi_version: CURRENT_ARTIFACT_ABI,
            compiler_version: compiler_version.into(),
            instructions: program.program,
            metadata,
            limits,
        })
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ArtifactError> {
        serde_json::to_vec(self).map_err(ArtifactError::Serialization)
    }
}

#[derive(Clone, Debug)]
pub struct ExecutableWorkflow {
    envelope: ArtifactEnvelope,
    hash: ArtifactHash,
}

impl ExecutableWorkflow {
    #[doc(hidden)]
    pub fn from_verified_envelope(envelope: ArtifactEnvelope) -> Result<Self, ArtifactError> {
        let computed = verify_program(envelope.instructions(), envelope.metadata())?;
        if computed != envelope.limits {
            return Err(ArtifactError::LimitMismatch);
        }
        let canonical = envelope.canonical_bytes()?;
        let hash = ArtifactHash(blake3::hash(&canonical).into());
        Ok(Self { envelope, hash })
    }

    pub fn verify(bytes: &[u8]) -> Result<Self, ArtifactError> {
        let envelope: ArtifactEnvelope =
            serde_json::from_slice(bytes).map_err(ArtifactError::Serialization)?;
        if envelope.abi_version != CURRENT_ARTIFACT_ABI {
            return Err(ArtifactError::UnsupportedAbi(envelope.abi_version));
        }
        let canonical = envelope.canonical_bytes()?;
        if canonical != bytes {
            return Err(ArtifactError::NonCanonical);
        }
        Self::from_verified_envelope(envelope)
    }

    pub fn envelope(&self) -> &ArtifactEnvelope {
        &self.envelope
    }

    pub fn hash(&self) -> ArtifactHash {
        self.hash
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ArtifactError> {
        self.envelope.canonical_bytes()
    }

    /// Transitional adapter for the pre-T7 VM. New persistence always stores
    /// the envelope; the adapter is deleted when the VM consumes this type.
    pub fn to_legacy_program(&self) -> CompiledProgram {
        let metadata = self.envelope.metadata();
        CompiledProgram {
            bytecode_version: self.hash.into_bytes(),
            program: self.envelope.instructions.clone(),
            debug_map: metadata.debug_map.clone(),
            join_plan: metadata.join_plan.clone(),
            wait_plan: metadata.wait_plan.clone(),
            message_name_map: metadata.message_name_map.clone(),
            write_set: metadata
                .write_set
                .iter()
                .map(|(name, keys)| (name.clone(), keys.iter().copied().collect::<HashSet<_>>()))
                .collect(),
            task_manifest: metadata.task_manifest.clone(),
            flag_symbol_table: metadata.flag_symbol_table.clone(),
            data_objects: metadata.data_objects.clone(),
            ffi_task_decls: metadata.ffi_task_decls.clone(),
            v2_ffi_task_decls: metadata.v2_ffi_task_decls.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("artifact serialization failed: {0}")]
    Serialization(serde_json::Error),
    #[error("unsupported artifact ABI version {0}")]
    UnsupportedAbi(u32),
    #[error("artifact bytes are not in canonical form")]
    NonCanonical,
    #[error("artifact verifier rejected instruction {address}: {reason}")]
    InvalidInstruction { address: Addr, reason: String },
    #[error("artifact verifier rejected side table: {0}")]
    InvalidMetadata(String),
    #[error("artifact declared limits do not match verifier-computed limits")]
    LimitMismatch,
}

fn verify_program(
    instructions: &[Instr],
    metadata: &ArtifactMetadata,
) -> Result<VerifiedLimits, ArtifactError> {
    if instructions.is_empty() {
        return Err(ArtifactError::InvalidMetadata(
            "instruction stream is empty".to_string(),
        ));
    }
    let len = instructions.len();
    for address in metadata.debug_map.keys().copied() {
        require_address(address, len, address, "debug map")?;
    }
    for address in metadata.ffi_task_decls.keys().copied() {
        require_address(address, len, address, "FFI task table")?;
        if !matches!(instructions[address.index()], Instr::ExecFfi { .. }) {
            return Err(ArtifactError::InvalidMetadata(format!(
                "FFI declaration address {address} is not ExecFfi"
            )));
        }
    }
    for address in metadata.v2_ffi_task_decls.keys().copied() {
        require_address(address, len, address, "v2 FFI effect table")?;
        if !matches!(
            instructions[address.index()],
            Instr::V2ArmEffect { .. } | Instr::V2AwaitEffect { .. }
        ) {
            return Err(ArtifactError::InvalidMetadata(format!(
                "v2 FFI effect declaration address {address} is not V2ArmEffect/V2AwaitEffect"
            )));
        }
    }
    let referenced_joins: BTreeSet<JoinId> = BTreeSet::new();
    for (address, instruction) in instructions.iter().enumerate() {
        match instruction {
            Instr::ExecNative { task_type, .. } | Instr::ExecDslTask { task_type, .. }
                if *task_type as usize >= metadata.task_manifest.len() =>
            {
                return Err(ArtifactError::InvalidInstruction {
                    address: Addr::from(address as u32),
                    reason: format!("task type {task_type} has no manifest entry"),
                });
            }
            Instr::ExecFfi { .. } if !metadata.ffi_task_decls.contains_key(&(Addr::from(address as u32))) => {
                return Err(ArtifactError::InvalidMetadata(format!(
                    "ExecFfi at address {address} has no FFI declaration"
                )));
            }
            Instr::V2ArmEffect { .. } | Instr::V2AwaitEffect { .. }
                if !metadata
                    .v2_ffi_task_decls
                    .contains_key(&(Addr::from(address as u32))) =>
            {
                return Err(ArtifactError::InvalidMetadata(format!(
                    "{instruction:?} at address {address} has no v2 FFI effect declaration"
                )));
            }
            _ => {}
        }
    }
    // V5.3 (§18, landed 2026-07-23): v1 `Instr::WaitMsg`/`WaitAny` are
    // deleted, along with `wait_plan`'s only producer and `race_plan`/
    // `boundary_map` entirely — `wait_plan`/`join_plan` remain typed
    // side tables (still populated by static v1 AND-join bookkeeping) but
    // no live construction path populates a wait-plan row any more, so
    // there is nothing left to reconcile a `referenced_waits` set against.
    if metadata.wait_plan.keys().next().is_some() {
        return Err(ArtifactError::InvalidMetadata(
            "wait side table is non-empty but no v1 instruction can populate it any more"
                .to_string(),
        ));
    }
    if referenced_joins != metadata.join_plan.keys().copied().collect() {
        return Err(ArtifactError::InvalidMetadata(
            "static join side table and instruction references are not bijective".to_string(),
        ));
    }
    // V5.3 (§18, landed 2026-07-23): the V-9 "a v2-bearing artifact must
    // carry none of the v1 race-plan/join-plan/boundary-route side
    // tables" active check that used to live here is retired along with
    // `race_plan`/`boundary_map` (deleted entirely) and v1 `Fork`/`Join`
    // (deleted, `join_plan`'s only producer) — there is no longer a v1
    // side table any construction path can populate, on either side of
    // the v1/v2 split, so the mixed-artifact hazard this check guarded
    // against no longer has a way to occur. `join_plan`'s bijection check
    // above already proves it stays empty.

    let mut heights = vec![None; len];
    heights[0] = Some(0u32);
    let mut queue = VecDeque::from([0usize]);
    let mut reachable_end = false;
    let mut max_stack = 0u32;
    let mut max_register = 0u32;
    let mut max_fibers = 1u32;
    let mut loop_multiplier = 1u64;

    while let Some(address) = queue.pop_front() {
        let height = heights[address].ok_or_else(|| ArtifactError::InvalidInstruction {
            address: Addr::from(address as u32),
            reason: "reachable instruction has no abstract stack state".to_string(),
        })?;
        let instruction = &instructions[address];
        let (pops, pushes) = stack_effect(instruction);
        if height < pops {
            return Err(ArtifactError::InvalidInstruction {
                address: Addr::from(address as u32),
                reason: format!("stack underflow: requires {pops}, has {height}"),
            });
        }
        let next_height = height - pops + pushes;
        max_stack = max_stack.max(next_height);
        match instruction {
            Instr::V2WaitMsg { corr_reg, .. } | Instr::PublishMessage { corr_reg, .. } => {
                max_register = max_register.max(u32::from(*corr_reg) + 1);
            }
            Instr::V2Fork { targets, .. } => {
                max_fibers = max_fibers.saturating_add(targets.len() as u32);
            }
            // A guard's handler spawns exactly one fibre when triggered
            // (`apply_v2_trigger_guard`), independent of the main line —
            // same undercounting gap as `V2Fork`'s targets, found the
            // same way (a real trigger test hit `ResourceLimitExceeded`
            // on a legal program).
            Instr::V2Guard { .. } | Instr::V2GuardN { .. } => {
                max_fibers = max_fibers.saturating_add(1);
                // Post-close remediation (V&S §13 amendment v0.5 ruling A,
                // restored): a `GUARD-N>` bounded by an immediately-
                // following `GUARD-TIMER>`/`GUARD-TIMER-CYCLE>` pair can
                // now spawn up to `max_fires` handler fibres from this ONE
                // static site over the scope's lifetime, not just one —
                // same undercounting shape as the comment above already
                // names, extended the same way: the `+1` above already
                // covers the first spawn, so add the remaining
                // `max_fires - 1`. `max_fires` is a static embedded field
                // (verifier-checkable, unlike an unbounded manual
                // re-trigger via repeated `Command::V2TriggerGuard` — that
                // gap is pre-existing, unaffected by this fix, and stays
                // exactly what it already was: an approximate static
                // bound backstopped by `validate_snapshot_limits`'s
                // runtime `ResourceLimitExceeded`, not a hard compile-time
                // guarantee). An UNBOUNDED `GUARD-TIMER>` (no
                // `GUARD-TIMER-CYCLE>` following) gets no extra static
                // credit here either, for the identical reason — the same
                // runtime backstop applies to it that already applies to
                // unbounded manual re-triggering today.
                if let Some(Instr::V2GuardTimerCycle { max_fires }) =
                    instructions.get(address + 2).filter(|_| {
                        matches!(instructions.get(address + 1), Some(Instr::V2GuardArmTimer))
                    })
                {
                    max_fibers = max_fibers.saturating_add(max_fires.saturating_sub(1));
                }
            }
            Instr::RoutePayload { branches, .. } if branches.is_empty() => {
                return Err(ArtifactError::InvalidInstruction {
                    address: Addr::from(address as u32),
                    reason: "payload route has no branches".to_string(),
                });
            }
            Instr::BrCounterLt { limit, target, .. } if *target < Addr::from(address as u32) => {
                if *limit == 0 {
                    return Err(ArtifactError::InvalidInstruction {
                        address: Addr::from(address as u32),
                        reason: "backward counter branch has zero bound".to_string(),
                    });
                }
                loop_multiplier = loop_multiplier.saturating_mul(u64::from(*limit));
            }
            Instr::Jump { target } | Instr::BrIf { target } | Instr::BrIfNot { target }
                if *target <= Addr::from(address as u32) =>
            {
                return Err(ArtifactError::InvalidInstruction {
                    address: Addr::from(address as u32),
                    reason: "unbounded backward control flow".to_string(),
                });
            }
            Instr::End | Instr::EndTerminate => reachable_end = true,
            _ => {}
        }

        for successor in successors(address, instruction, instructions, len)? {
            match heights[successor] {
                Some(existing) if existing != next_height => {
                    return Err(ArtifactError::InvalidInstruction {
                        address: Addr::from(successor as u32),
                        reason: format!(
                            "inconsistent stack height at CFG merge: {existing} versus {next_height}"
                        ),
                    });
                }
                Some(_) => {}
                None => {
                    heights[successor] = Some(next_height);
                    queue.push_back(successor);
                }
            }
        }
    }

    if !reachable_end {
        return Err(ArtifactError::InvalidMetadata(
            "entry point cannot reach an End or EndTerminate instruction".to_string(),
        ));
    }

    // V5.3 (§18, landed 2026-07-23): the v1 non-interrupting-boundary-
    // timer escalation-fiber accounting that used to live here (walking
    // `race_plan`) is deleted along with `race_plan`/`boundary_map`
    // themselves — boundary timers now lower exclusively to `V2Guard`/
    // `V2GuardN` + `GUARD-TIMER>`, whose escalation-fiber accounting is
    // already covered above by the `Instr::V2Guard { .. } | Instr::V2GuardN
    // { .. } => max_fibers.saturating_add(1)` arm (a guard's handler
    // spawns exactly one fibre when triggered — `apply_v2_trigger_guard`).

    // V-7 (V&S §7): the dual-stack abstract interpreter's control-stack
    // half. A v1-only artifact has no D2 words, so this returns zeroed
    // limits for it (not a special case — the walk simply never visits a
    // scope-opening instruction); a v2-bearing artifact gets the real
    // computed maxima. This call is also where V-1..V-5/V-8 are actually
    // enforced — `verify_program` rejects a malformed v2 control-stack
    // shape here, not just the operand-stack/reachability properties
    // already checked above.
    // `loop_multiplier` (computed above, over reachable backward
    // `BrCounterLt` bounds) is threaded through so V-7's record/barrier
    // maxima account for loop re-entry, not just fork-fibre multiplicity
    // — a second static instruction visit inside a bounded loop's body is
    // otherwise invisible to `verify_v2_control_stack`'s worklist, which
    // (correctly, for V-1/V-2 purposes) treats a revisited address as
    // "already checked," not "count it again."
    let control_stack_limits =
        crate::v2_verifier::verify_v2_control_stack(instructions, loop_multiplier)?;

    // V-11 (§7, §18 ruling J): every reachable instruction must have a
    // path to a terminal instruction. Deliberately sequenced AFTER the
    // `verify_v2_control_stack` call above (which enforces V-8, backward
    // v2 edges) and AFTER the height walk above it (which already
    // rejected unbounded backward v1 `Jump`/`BrIf`/`BrIfNot` edges) — by
    // this point the only backward edge that can still exist is a bounded
    // `BrCounterLt` loop's own back-edge, which `verify_terminal_reachability`'s
    // own doc comment explains is handled correctly by construction, not
    // via a special case here.
    crate::v2_verifier::verify_terminal_reachability(instructions)?;

    Ok(VerifiedLimits {
        max_stack,
        max_registers: max_register.max(8),
        max_fibers,
        max_steps: (len as u64).saturating_mul(loop_multiplier),
        max_control_depth: control_stack_limits.max_control_depth,
        max_barriers: control_stack_limits.max_barriers,
        max_records: control_stack_limits.max_records,
    })
}

/// V-9's active check needs to distinguish v2 words from v1 without
/// depending on `v2_verifier`'s internals — a plain tag match, kept next
/// to `Instr`'s own definition-adjacent helpers rather than duplicated.
pub(crate) fn stack_effect(instruction: &Instr) -> (u32, u32) {
    match instruction {
        Instr::PushBool(_) | Instr::PushI64(_) | Instr::LoadFlag { .. } => (0, 1),
        // `V2LoadPlaceholderMatch`'s stack effect mirrors `LoadFlag`'s
        // `(0, 1)` exactly — the DSL-side placeholder-match analogue (see
        // the `Instr` doc comment). `V2RouteZeroMatch` takes no operand at
        // all (unconditional raise, not a conditional assert) — `(0, 0)`,
        // covered by the catch-all below.
        Instr::V2LoadPlaceholderMatch { .. } => (0, 1),
        // `V2MiIndexLive` mirrors `V2LoadPlaceholderMatch`'s `(0, 1)`
        // exactly — the MI-side index/skip analogue (§18 ruling K).
        // `V2MiArityCheck` takes no operand and pushes nothing (a runtime
        // assert, not a value producer) — covered by the catch-all below.
        Instr::V2MiIndexLive { .. } => (0, 1),
        // `V2MiLoadElement` (§18 ruling K Part 2): pushes the resolved MI
        // element's `Value` — `(0, 1)`, the same shape as `LoadFlag`/
        // `V2MiIndexLive`, matching this codebase's convention that a
        // pure operand-stack producer always reports `(0, 1)` regardless
        // of the pushed value's own runtime type/size (`max_stack` bounds
        // slot *count*; `Value::Array`'s own size is bounded separately —
        // see `types::MAX_VALUE_ARRAY_LEN`/`MAX_VALUE_ARRAY_DEPTH`).
        Instr::V2MiLoadElement { .. } => (0, 1),
        Instr::Pop | Instr::StoreFlag { .. } | Instr::BrIf { .. } | Instr::BrIfNot { .. } => (1, 0),
        Instr::ExecNative { argc, retc, .. } => (u32::from(*argc), u32::from(*retc)),
        Instr::ExecDslTask { .. } => (0, 0),
        // FFI values are resolved through the compiled binding side table and
        // written back through binding targets; they never touch the VM stack.
        Instr::ExecFfi { .. } => (0, 0),
        // V2.7 addressing-review BLOCKING #2.4: duration/deadline pop from
        // the operand stack per §5's literal `( duration -- )` notation,
        // not an embedded field — see `V2ArmTimer`'s doc comment.
        Instr::V2WaitFor | Instr::V2WaitUntil | Instr::V2ArmTimer { .. } | Instr::V2GuardArmTimer => {
            (1, 0)
        }
        _ => (0, 0),
    }
}

pub(crate) fn successors(
    address: usize,
    instruction: &Instr,
    instructions: &[Instr],
    len: usize,
) -> Result<Vec<usize>, ArtifactError> {
    let mut result = Vec::new();
    let mut add = |target: Addr, description: &str| -> Result<(), ArtifactError> {
        require_address(target, len, Addr::from(address as u32), description)?;
        result.push(target.index());
        Ok(())
    };
    match instruction {
        Instr::Jump { target } => add(*target, "jump")?,
        Instr::BrIf { target } | Instr::BrIfNot { target } | Instr::BrCounterLt { target, .. } => {
            add(*target, "branch")?;
            if address + 1 < len {
                result.push(address + 1);
            }
        }
        Instr::RoutePayload {
            branches,
            default_target,
        } => {
            for branch in branches {
                add(branch.target, "payload route")?;
            }
            if let Some(target) = default_target {
                add(*target, "payload route default")?;
            }
        }
        Instr::End | Instr::EndTerminate | Instr::Fail { .. } => {}

        // ─── v2 ISA (V2.7 7.3) ──────────────────────────────────
        //
        // The verifier walks a CFG built directly over this same flat v2
        // `Instr` stream, not the compiler's `IRGraph` (`bpmn-lite-compiler`'s
        // `IRNode`/`IREdge`, BPMN-XML-shaped and disconnected from bytecode):
        // V2.7 7.4's hand-assembled fixtures have no compiler-side IR at all
        // (v2 frontends don't exist until V5), so `IRGraph` cannot be the
        // representation V3 verifies. `successors()` — already the verifier's
        // existing home for v1 branch/fork/join edges — is extended here with
        // the v2 words that introduce additional static reachable edges
        // beyond plain fallthrough; V3 builds its abstract control-stack
        // dataflow on top of these edges plus `v2_control_stack_effect`
        // below.
        Instr::V2Guard { handler } | Instr::V2GuardN { handler } => {
            add(*handler, "guard handler")?;
            if address + 1 < len {
                result.push(address + 1);
            }
        }
        // BoundaryError v2 migration: `GUARD-ERROR>` carries its OWN
        // `handler` (the deliberate, ratified break of "arm instructions
        // never carry targets" — see `Instr::V2GuardArmError`'s doc
        // comment). Without an explicit arm here this fell through to the
        // generic `_ if address + 1 < len` case below, which only pushes
        // the fallthrough successor and never validates or registers
        // `handler` as a reachable edge — silently swallowing the exact
        // out-of-bounds-address check `V2Guard`/`V2GuardN` already get.
        // Treated like `V2Guard`/`V2GuardN`'s own handler (a direct,
        // immediately reachable edge), not like `V2ArmTimer`/`V2ArmMsg`/
        // `V2ArmEffect`'s race-arm indirection through `}RACE` — there is
        // no equivalent "resolving" instruction for a guard's error arms;
        // each arm's handler is reachable the moment the arm executes, via
        // `apply_job_failure`'s match, not via a later closing word. Also
        // falls through to `address + 1` — arm words register, they don't
        // transfer control (multiple `V2GuardArmError`s / the guarded host
        // task's own instructions follow contiguously).
        Instr::V2GuardArmError { handler, .. } => {
            add(*handler, "guard error-arm handler")?;
            if address + 1 < len {
                result.push(address + 1);
            }
        }
        // V2.7 addressing-review CONCERN #2.3: an ARM-* word only
        // *registers* an alternative — it does not itself transfer
        // control. `}RACE` is where resolution happens (§5: "A command
        // addressed to an armed alternative resolves it: winner
        // continuation runs") and, not incidentally, where the race's
        // control-stack handle is popped (`[ h -- ]`). Modeling the arm
        // target as a successor of the ARM-* word (the original V2.7 cut)
        // stranded that pop on a dead-end `V2RaceClose` node the CFG walk
        // never threaded through to the winning target's abstract entry
        // state — exactly the gap V-1 balance would have missed. So the
        // arm word only falls through to the next arm/`}RACE`; the arm
        // targets are attributed to `V2RaceClose` below, via
        // `race_arm_targets`, so any future dataflow walk necessarily
        // applies `}RACE`'s pop before it reaches an arm target.
        Instr::V2ArmTimer { .. } | Instr::V2ArmMsg { .. } | Instr::V2ArmEffect { .. }
            if address + 1 < len =>
        {
            result.push(address + 1);
        }
        Instr::V2ArmTimer { .. } | Instr::V2ArmMsg { .. } | Instr::V2ArmEffect { .. } => {}
        Instr::V2Fork { targets, .. } => {
            for target in targets.iter().copied() {
                add(target, "v2 fork")?;
            }
        }
        // `}RACE` parks, pops the race's control-stack handle, then
        // transfers control to whichever arm won — so each arm's static
        // `target` is a successor of `V2RaceClose`, not of the arm word
        // that registered it. `race_arm_targets` scans back to the
        // matching `V2RaceOpen`; V-5 (race shape) requires the n ARM-*
        // words to sit contiguously between `RACE{` and `}RACE` with no
        // other control flow, so the backward scan is exact, and doubles
        // as a structural check of that shape (mismatched/missing
        // `RACE{` or a non-ARM instruction in between is a verifier
        // rejection here, not a silent miscount).
        Instr::V2RaceClose => {
            for target in race_arm_targets(address, instructions)? {
                add(target, "race arm target (via }RACE resolution)")?;
            }
        }
        // `CANCEL-SCOPE` unwinds and does not resume this fiber, matching
        // `End`/`EndTerminate`/`Fail`'s no-fallthrough treatment above.
        Instr::V2CancelScope => {}
        // `V2RouteZeroMatch` unconditionally raises an Incident and never
        // falls through — same no-fallthrough class as `Fail`/
        // `V2CancelScope` (V-11's terminal set, `v2_verifier::is_terminal`,
        // is amended to match). `V2LoadPlaceholderMatch` is deliberately
        // NOT listed here — it is a plain operand-stack producer like
        // `LoadFlag`, and falls through to the generic `address + 1` arm
        // below exactly as `LoadFlag` does.
        Instr::V2RouteZeroMatch => {}

        _ if address + 1 < len => result.push(address + 1),
        _ => {}
    }
    Ok(result)
}

/// Scan backward from a `V2RaceClose` at `close_address` to its matching
/// `V2RaceOpen`, collecting each `V2Arm*` word's `target` in declaration
/// order. V-5 (race shape, V&S §7) requires the arms to sit contiguously
/// between `RACE{` and `}RACE` with no *state mutation* in between — not
/// zero tolerance for any other instruction at all: `V2ArmTimer` pops its
/// own `duration` operand (V2.7 addressing-review BLOCKING #2.4), so a
/// pure operand-stack producer (`PushI64`/`PushBool`/`LoadFlag`/`Pop`)
/// immediately before an arm word, supplying that arm's own operand, is
/// legal region content. Anything else — a branch, a nested guard/race/
/// fork, or reaching the front of the program without a `V2RaceOpen` — is
/// rejected as a malformed race, doubling this function as an early,
/// partial V-5 shape check.
pub(crate) fn race_arm_targets(
    close_address: usize,
    instructions: &[Instr],
) -> Result<Vec<Addr>, ArtifactError> {
    let mut targets = Vec::new();
    let mut cursor = close_address;
    loop {
        if cursor == 0 {
            return Err(ArtifactError::InvalidInstruction {
                address: Addr::from(close_address as u32),
                reason: "}RACE has no matching RACE{ (scanned back to program start)"
                    .to_string(),
            });
        }
        cursor -= 1;
        match &instructions[cursor] {
            Instr::V2ArmTimer { target }
            | Instr::V2ArmMsg { target, .. }
            | Instr::V2ArmEffect { target, .. } => targets.push(*target),
            Instr::V2RaceOpen { arm_count } => {
                targets.reverse();
                if targets.len() != *arm_count as usize {
                    return Err(ArtifactError::InvalidInstruction {
                        address: Addr::from(close_address as u32),
                        reason: format!(
                            "}}RACE arm count mismatch: RACE{{ at {cursor} declared \
                             {arm_count} arms, found {}",
                            targets.len()
                        ),
                    });
                }
                return Ok(targets);
            }
            // An arm word's own operand computation — not a foreign
            // instruction breaking the region's contiguity. V3 review
            // remediation forward note (V-5): this is the READ-vs-MUTATE
            // line, and it is deliberate, not an oversight to "complete
            // later." `PushBool`/`PushI64`/`LoadFlag` read a constant or
            // an existing flag onto the operand stack — none of them can
            // observe or change anything about the enclosing race/guard
            // scope. `Pop` only discards a value already computed inside
            // this same tolerated region. `StoreFlag` is deliberately
            // EXCLUDED and falls through to the rejection arm below: it
            // WRITES `ProcessInstance.flags`, exactly the "unguarded
            // state mutation between arm and park" V-5 (V&S §7) forbids.
            // If a future edit is tempted to add `StoreFlag` here "to let
            // an arm compute something more complex," that tempting edit
            // is the one this comment exists to stop — a mutation inside
            // an armed-but-unresolved race is a real V-5 violation, not
            // an artificial restriction.
            Instr::PushBool(_) | Instr::PushI64(_) | Instr::LoadFlag { .. } | Instr::Pop => {}
            _ => {
                return Err(ArtifactError::InvalidInstruction {
                    address: Addr::from(close_address as u32),
                    reason: format!(
                        "}}RACE's arm region contains a non-arm, non-operand \
                         instruction at {cursor} — race shape is not contiguous \
                         back to RACE{{"
                    ),
                });
            }
        }
    }
}

/// Whether a v2 `Instr` pushes, pops, or leaves untouched the abstract
/// control stack (V&S §5's `[ ... ]` notation) — the dataflow fact V2.7 7.3
/// exposes for V3's abstract interpretation to consume. This is *not* the
/// interpretation itself (V3 owns balance/nesting/arity proofs); it is the
/// minimal per-instruction classification those proofs are built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlStackEffect {
    /// `[ -- h ]` — allocates a record, pushes its handle.
    Push,
    /// `[ h -- ]` — pops and retires a handle.
    Pop,
    /// `[ h ]` — reads the top handle without popping it (armed/park words
    /// that act on the race/guard/barrier they're already inside).
    Peek,
    /// No control-stack interaction (`( ... )` only, or a v1 instruction).
    None,
}

pub fn v2_control_stack_effect(instr: &Instr) -> ControlStackEffect {
    match instr {
        Instr::V2Guard { .. } | Instr::V2GuardN { .. } | Instr::V2GuardR | Instr::V2RaceOpen { .. } => {
            ControlStackEffect::Push
        }
        Instr::V2GuardEnd | Instr::V2GuardNEnd | Instr::V2GuardREnd | Instr::V2RaceClose | Instr::V2Join { .. } => {
            ControlStackEffect::Pop
        }
        Instr::V2ArmTimer { .. }
        | Instr::V2ArmMsg { .. }
        | Instr::V2ArmEffect { .. }
        | Instr::V2CancelScope => ControlStackEffect::Peek,
        // V2Fork pushes the barrier handle onto each *spawned fibre's*
        // inherited control stack (V&S §5: "each inheriting the parent's
        // control stack with the barrier handle pushed on top") — not onto
        // the forking fibre's own stack, which is a cross-fibre effect V3's
        // per-CFG-edge dataflow models separately from this single-fibre
        // classification.
        //
        // **V3 handoff note (addressing-review disposition, recorded not
        // as a defect):** this function is NOT total for `V2Fork` in the
        // sense its `None` return might suggest. `None` here means "no
        // effect on *this* fibre's own control stack" — it does not mean
        // "no control-stack effect anywhere as a result of this
        // instruction." V3's abstract interpreter must model the
        // children-inherit-handle semantics explicitly, as a distinct,
        // cross-fibre dataflow fact (each spawned fibre's *entry* control
        // stack = the forking fibre's stack + the barrier handle), and
        // must not attempt to derive that fact by calling this function
        // and reading `None` as "nothing to model."
        Instr::V2Fork { .. } => ControlStackEffect::None,
        Instr::V2WaitFor
        | Instr::V2WaitUntil
        | Instr::V2WaitMsg { .. }
        | Instr::V2AwaitEffect { .. } => ControlStackEffect::None,
        _ => ControlStackEffect::None,
    }
}

pub(crate) fn require_address(
    target: Addr,
    len: usize,
    address: Addr,
    description: &str,
) -> Result<(), ArtifactError> {
    if target.index() >= len {
        return Err(ArtifactError::InvalidInstruction {
            address,
            reason: format!("{description} target {target} is outside instruction stream {len}"),
        });
    }
    Ok(())
}

/// V2.7 7.4 — hand-assembled v2 `Instr` fixtures. No compiler emits v2 words
/// yet (that's V5); these are built directly, address by address, and
/// proven to decode, round-trip, and pass the same `verify_program`
/// structural checks every v1 artifact passes. Neither fixture exercises
/// `kernel::apply` — v2 word *semantics* are V4's scope
/// (`bpmn-lite-kernel`'s `apply_tick`).
#[cfg(test)]
mod v2_fixtures {
    use super::*;

    fn empty_compiled_program(instructions: Vec<Instr>) -> CompiledProgram {
        CompiledProgram {
            bytecode_version: [0u8; 32],
            program: instructions,
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: Vec::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
            v2_ffi_task_decls: BTreeMap::new(),
        }
    }

    fn assert_decodes_and_round_trips(instructions: Vec<Instr>) -> ArtifactEnvelope {
        let program = empty_compiled_program(instructions);
        let envelope = ArtifactEnvelope::from_legacy_program(program, "v2.7-fixture")
            .expect("hand-assembled v2 fixture must pass verify_program");
        let bytes = envelope.canonical_bytes().expect("canonical_bytes");
        let workflow =
            ExecutableWorkflow::verify(&bytes).expect("decode + verify must round-trip");
        assert_eq!(
            workflow.envelope().canonical_bytes().unwrap(),
            bytes,
            "re-encoding a decoded v2 envelope must reproduce the exact original bytes"
        );
        envelope
    }

    /// EX-oracle-shaped: an interrupting guard (`V2Guard`) wraps a parallel
    /// subprocess (`V2Fork`/`V2Join`); the guard's handler is a race
    /// (`V2RaceOpen`/`V2ArmTimer`/`V2ArmMsg`/`V2RaceClose`) with a timer
    /// alternative and a message alternative. `V2ArmTimer`'s `duration` is
    /// supplied via `PushI64` immediately before it (V2.7 addressing-review
    /// BLOCKING #2.4 — operand stack, not an embedded field).
    #[test]
    fn ex_oracle_shaped_guard_over_fork_join_with_race_handler_decodes_and_round_trips() {
        let instructions = ex_oracle_program();
        let envelope = assert_decodes_and_round_trips(instructions);
        assert_eq!(envelope.instructions().len(), 16);
    }

    /// Addresses, named, for the EX-oracle fixture — shared by the fixture
    /// test above and the race-handle-pop proof below, so the two can't
    /// silently drift out of sync with each other.
    struct ExOracleAddrs {
        race_open: usize,
        timer_win: usize,
        msg_win: usize,
    }

    fn ex_oracle_program() -> Vec<Instr> {
        vec![
            /* 0  */ Instr::V2Guard { handler: Addr::new(8) },
            /* 1  */ Instr::V2Fork {
                targets: Box::new([Addr::new(2), Addr::new(4)]),
                pairing: Addr::new(1),
            },
            /* 2  */ Instr::V2Join { pairing: Addr::new(1) }, // branch A
            /* 3  */ Instr::Jump { target: Addr::new(6) },
            /* 4  */ Instr::V2Join { pairing: Addr::new(1) }, // branch B
            /* 5  */ Instr::Jump { target: Addr::new(6) },
            /* 6  */ Instr::V2GuardEnd,
            /* 7  */ Instr::End,
            /* 8  */ Instr::V2RaceOpen { arm_count: 2 }, // guard handler entry
            /* 9  */ Instr::PushI64(5_000), // V2ArmTimer's own duration operand
            /* 10 */ Instr::V2ArmTimer { target: Addr::new(13) },
            /* 11 */ Instr::V2ArmMsg {
                target: Addr::new(14),
                name: 42,
                corr_reg: 0,
            },
            /* 12 */ Instr::V2RaceClose,
            /* 13 */ Instr::Jump { target: Addr::new(15) }, // timer-win continuation
            /* 14 */ Instr::Jump { target: Addr::new(15) }, // msg-win continuation
            /* 15 */ Instr::End,
        ]
    }

    fn ex_oracle_addrs() -> ExOracleAddrs {
        ExOracleAddrs {
            race_open: 8,
            timer_win: 13,
            msg_win: 14,
        }
    }

    /// V2.7 addressing-review CONCERN #2.3: confirms the race's
    /// control-stack handle (pushed by `V2RaceOpen`, popped by
    /// `V2RaceClose`) is popped in the ABSTRACT ENTRY STATE at each arm
    /// target — not stranded on a dead path at `V2RaceClose` itself. Walks
    /// the same `successors()` graph the (future) verifier will use,
    /// threading `v2_control_stack_effect` forward from `V2RaceOpen` as a
    /// simple depth counter (mirrors `verify_program`'s existing
    /// operand-stack-height BFS, scoped to control-stack depth instead).
    /// If the arm-target edge were still attributed to the `V2Arm*` word
    /// (the original V2.7 cut, before this remediation), this would fail:
    /// the target's entry depth would be computed relative to `V2RaceOpen`
    /// directly, never passing through `V2RaceClose`'s `Pop`.
    #[test]
    fn race_handle_is_popped_in_abstract_entry_state_at_each_arm_target() {
        let instructions = ex_oracle_program();
        let addrs = ex_oracle_addrs();
        let len = instructions.len();

        // entry_depth[addr] = control-stack depth *before* executing the
        // instruction at addr, relative to `race_open`'s own entry depth
        // (taken as the zero baseline — this test is scoped to the race's
        // own handle, not the enclosing guard's).
        let mut entry_depth: BTreeMap<usize, i64> = BTreeMap::new();
        entry_depth.insert(addrs.race_open, 0);
        let mut queue = std::collections::VecDeque::from([addrs.race_open]);
        while let Some(addr) = queue.pop_front() {
            let depth = entry_depth[&addr];
            let instruction = &instructions[addr];
            let delta = match v2_control_stack_effect(instruction) {
                ControlStackEffect::Push => 1,
                ControlStackEffect::Pop => -1,
                ControlStackEffect::Peek | ControlStackEffect::None => 0,
            };
            let exit_depth = depth + delta;
            for successor in successors(addr, instruction, &instructions, len).unwrap() {
                match entry_depth.get(&successor) {
                    Some(existing) => assert_eq!(
                        *existing, exit_depth,
                        "control-stack depth must agree at CFG merge point {successor}"
                    ),
                    None => {
                        entry_depth.insert(successor, exit_depth);
                        queue.push_back(successor);
                    }
                }
            }
        }

        assert_eq!(
            entry_depth[&addrs.timer_win], 0,
            "race handle must already be popped on entry to the timer-win arm target"
        );
        assert_eq!(
            entry_depth[&addrs.msg_win], 0,
            "race handle must already be popped on entry to the msg-win arm target"
        );
    }

    /// §18 v0.10 ruling H hypothesis fixture, `max_fibers` half. Companion
    /// to `bpmn-lite-kernel`'s
    /// `v2fork_mixed_real_work_and_skip_to_join_branches_retires_barrier_via_unmodified_mechanism`,
    /// which proves the runtime side (barrier retirement); this proves
    /// `VerifiedLimits::max_fibers` (computed at ~line 478 above,
    /// `saturating_add(targets.len() as u32)` for `Instr::V2Fork`) already
    /// correctly bounds a fork with a skip-to-join branch, with NO fix
    /// needed — because `targets.len()` is 3 regardless of whether one of
    /// those 3 spawned fibres does "real work" or immediately `BrIf`s to
    /// its `V2Join`. The skip pattern changes what a fibre DOES, never how
    /// many fibres `V2Fork` spawns, so the existing static
    /// `targets.len()`-based accounting was never a quantity this ruling
    /// needed to touch.
    #[test]
    fn max_fibers_already_accounts_for_a_v2fork_with_a_skip_to_join_branch() {
        let instructions = vec![
            /* 0 */ Instr::V2Fork {
                targets: Box::new([Addr::new(1), Addr::new(3), Addr::new(5)]),
                pairing: Addr::new(0),
            },
            /* 1 */ Instr::Jump { target: Addr::new(2) }, // branch A: "real work" stand-in
            /* 2 */ Instr::Jump { target: Addr::new(8) },
            /* 3 */ Instr::Jump { target: Addr::new(4) }, // branch B: "real work" stand-in
            /* 4 */ Instr::Jump { target: Addr::new(8) },
            /* 5 */ Instr::PushBool(true), // branch C: skip condition
            /* 6 */ Instr::BrIf { target: Addr::new(8) }, // skip straight to V2Join
            /* 7 */ Instr::Jump { target: Addr::new(8) }, // branch C's own "real work"
            /* 8 */ Instr::V2Join { pairing: Addr::new(0) },
            /* 9 */ Instr::End,
        ];
        let envelope = assert_decodes_and_round_trips(instructions);
        assert_eq!(
            envelope.limits().max_fibers(),
            4,
            "1 (base/root fibre) + 3 (V2Fork's targets.len(), the static bound — \
             unaffected by which of the 3 spawned fibres skips straight to V2Join)"
        );
    }

    /// Re-entrant `V2Fork`/`V2Join`: a bounded loop (v1's `IncCounter`/
    /// `BrCounterLt`, the only legal backward-branch mechanism) whose body
    /// is a `V2Fork`/`V2Join` pair — proving the same static addresses
    /// admit repeated execution (a fresh activation per V&S §5, a runtime
    /// property V4 discharges; this fixture proves only that the *shape*
    /// is legal, decodes, and round-trips).
    #[test]
    fn reentrant_fork_join_in_bounded_loop_decodes_and_round_trips() {
        let instructions = vec![
            /* 0 */ Instr::IncCounter { counter_id: 0 },
            /* 1 */ Instr::V2Fork {
                targets: Box::new([Addr::new(3), Addr::new(5)]),
                pairing: Addr::new(1),
            },
            /* 2 */ Instr::V2CancelScope, // unreachable filler slot, never targeted
            /* 3 */ Instr::V2Join { pairing: Addr::new(1) }, // branch A
            /* 4 */ Instr::Jump { target: Addr::new(7) },
            /* 5 */ Instr::V2Join { pairing: Addr::new(1) }, // branch B
            /* 6 */ Instr::Jump { target: Addr::new(7) },
            /* 7 */ Instr::BrCounterLt {
                counter_id: 0,
                limit: 3,
                target: Addr::new(0),
            },
            /* 8 */ Instr::End,
        ];
        let envelope = assert_decodes_and_round_trips(instructions);
        assert_eq!(envelope.instructions().len(), 9);
    }
}
