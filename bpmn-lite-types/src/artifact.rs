use crate::{
    Addr, CompiledProgram, ErrorRoute, FfiTaskDecl, FlagKey, Instr, JoinId, JoinPlanEntry, RaceId,
    RacePlanEntry, WaitArm, WaitId, WaitPlanEntry,
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
    race_plan: BTreeMap<RaceId, RacePlanEntry>,
    boundary_map: BTreeMap<Addr, RaceId>,
    write_set: BTreeMap<String, BTreeSet<FlagKey>>,
    task_manifest: Vec<String>,
    error_route_map: BTreeMap<Addr, Vec<ErrorRoute>>,
    flag_symbol_table: BTreeMap<FlagKey, String>,
    data_objects: BTreeMap<String, crate::DataObjectDecl>,
    ffi_task_decls: BTreeMap<Addr, FfiTaskDecl>,
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

    pub fn race_plan(&self) -> &BTreeMap<RaceId, RacePlanEntry> {
        &self.race_plan
    }

    pub fn boundary_map(&self) -> &BTreeMap<Addr, RaceId> {
        &self.boundary_map
    }

    pub fn write_set(&self) -> &BTreeMap<String, BTreeSet<FlagKey>> {
        &self.write_set
    }

    pub fn task_manifest(&self) -> &[String] {
        &self.task_manifest
    }

    pub fn error_route_map(&self) -> &BTreeMap<Addr, Vec<ErrorRoute>> {
        &self.error_route_map
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
            race_plan: program.race_plan.clone(),
            boundary_map: program.boundary_map.clone(),
            write_set,
            task_manifest: program.task_manifest.clone(),
            error_route_map: program.error_route_map.clone(),
            flag_symbol_table: program.flag_symbol_table.clone(),
            data_objects: program.data_objects.clone(),
            ffi_task_decls: program.ffi_task_decls.clone(),
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
            race_plan: metadata.race_plan.clone(),
            boundary_map: metadata.boundary_map.clone(),
            write_set: metadata
                .write_set
                .iter()
                .map(|(name, keys)| (name.clone(), keys.iter().copied().collect::<HashSet<_>>()))
                .collect(),
            task_manifest: metadata.task_manifest.clone(),
            error_route_map: metadata.error_route_map.clone(),
            flag_symbol_table: metadata.flag_symbol_table.clone(),
            data_objects: metadata.data_objects.clone(),
            ffi_task_decls: metadata.ffi_task_decls.clone(),
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
    for (address, race_id) in &metadata.boundary_map {
        require_address(*address, len, *address, "boundary map")?;
        if !matches!(instructions[address.index()], Instr::ExecNative { .. }) {
            return Err(ArtifactError::InvalidMetadata(format!(
                "boundary address {address} is not ExecNative"
            )));
        }
        if !metadata.race_plan.contains_key(race_id) {
            return Err(ArtifactError::InvalidMetadata(format!(
                "boundary address {address} references missing race {race_id}"
            )));
        }
    }
    for address in metadata.ffi_task_decls.keys().copied() {
        require_address(address, len, address, "FFI task table")?;
        if !matches!(instructions[address.index()], Instr::ExecFfi { .. }) {
            return Err(ArtifactError::InvalidMetadata(format!(
                "FFI declaration address {address} is not ExecFfi"
            )));
        }
    }
    for address in metadata.error_route_map.keys().copied() {
        require_address(address, len, address, "error route table")?;
    }
    let mut referenced_races = BTreeSet::new();
    let mut referenced_waits = BTreeSet::new();
    let mut referenced_joins = BTreeSet::new();
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
            Instr::WaitMsg { wait_id, name, .. } => {
                referenced_waits.insert(*wait_id);
                if !metadata.message_name_map.contains_key(name) {
                    return Err(ArtifactError::InvalidInstruction {
                        address: Addr::from(address as u32),
                        reason: format!("message name {name} has no side-table entry"),
                    });
                }
            }
            Instr::WaitAny { race_id, .. } => {
                referenced_races.insert(*race_id);
            }
            // Static AND joins require a persisted register template. Inclusive
            // joins derive their expected cardinality at runtime from the fork,
            // so they intentionally have no static join-plan row.
            Instr::Join { id, .. } => {
                referenced_joins.insert(*id);
            }
            _ => {}
        }
    }
    referenced_races.extend(metadata.boundary_map.values().copied());
    if referenced_races != metadata.race_plan.keys().copied().collect() {
        return Err(ArtifactError::InvalidMetadata(
            "race side table and instruction references are not bijective".to_string(),
        ));
    }
    if referenced_waits != metadata.wait_plan.keys().copied().collect() {
        return Err(ArtifactError::InvalidMetadata(
            "wait side table and instruction references are not bijective".to_string(),
        ));
    }
    if referenced_joins != metadata.join_plan.keys().copied().collect() {
        return Err(ArtifactError::InvalidMetadata(
            "static join side table and instruction references are not bijective".to_string(),
        ));
    }

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
            Instr::WaitMsg { corr_reg, .. } | Instr::PublishMessage { corr_reg, .. } => {
                max_register = max_register.max(u32::from(*corr_reg) + 1);
            }
            Instr::Fork { targets } => {
                max_fibers = max_fibers.saturating_add(targets.len() as u32);
            }
            Instr::ForkInclusive { branches, .. } => {
                max_fibers = max_fibers.saturating_add(branches.len() as u32);
            }
            Instr::RoutePayload { branches, .. } | Instr::ForkPayload { branches, .. }
                if branches.is_empty() =>
            {
                return Err(ArtifactError::InvalidInstruction {
                    address: Addr::from(address as u32),
                    reason: "payload route has no branches".to_string(),
                });
            }
            Instr::ForkPayload { branches, .. } => {
                max_fibers = max_fibers.saturating_add(branches.len() as u32);
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

        for successor in successors(address, instruction, len)? {
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

    // A non-interrupting boundary timer preserves its host fiber and creates a
    // new escalation fiber on every firing. These fibers are not represented
    // by a Fork instruction, so account for their verifier-bounded cycle count
    // explicitly. This is intentionally conservative: escalation fibers may
    // overlap if they themselves park before a later cycle fires.
    for race in metadata.race_plan.values() {
        for arm in &race.arms {
            if let WaitArm::Timer {
                interrupting: false,
                cycle,
                ..
            } = arm
            {
                let spawned = cycle.as_ref().map_or(1, |spec| spec.max_fires);
                max_fibers = max_fibers.saturating_add(spawned);
            }
        }
    }

    Ok(VerifiedLimits {
        max_stack,
        max_registers: max_register.max(8),
        max_fibers,
        max_steps: (len as u64).saturating_mul(loop_multiplier),
        // No D2 words exist in a v2-pre-V3 artifact to create control-stack
        // depth, armed scopes, or concurrency-table records — V3's verifier
        // computes real maxima once V4's words exist to bound (V&S V-7).
        max_control_depth: 0,
        max_barriers: 0,
        max_records: 0,
    })
}

fn stack_effect(instruction: &Instr) -> (u32, u32) {
    match instruction {
        Instr::PushBool(_) | Instr::PushI64(_) | Instr::LoadFlag { .. } => (0, 1),
        Instr::Pop | Instr::StoreFlag { .. } | Instr::BrIf { .. } | Instr::BrIfNot { .. } => (1, 0),
        Instr::ExecNative { argc, retc, .. } => (u32::from(*argc), u32::from(*retc)),
        Instr::ExecDslTask { .. } => (0, 0),
        // FFI values are resolved through the compiled binding side table and
        // written back through binding targets; they never touch the VM stack.
        Instr::ExecFfi { .. } => (0, 0),
        _ => (0, 0),
    }
}

fn successors(
    address: usize,
    instruction: &Instr,
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
        Instr::Fork { targets } => {
            for target in targets.iter().copied() {
                add(target, "fork")?;
            }
        }
        Instr::Join { next, .. } | Instr::JoinDynamic { next, .. } => add(*next, "join")?,
        Instr::WaitAny { arms, .. } => {
            for arm in arms {
                add(arm.resume_at(), "race arm")?;
            }
        }
        Instr::ForkInclusive {
            branches,
            default_target,
            ..
        } => {
            for branch in branches {
                add(branch.target, "inclusive branch")?;
            }
            if let Some(target) = default_target {
                add(*target, "inclusive default")?;
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
        Instr::ForkPayload {
            branches,
            default_target,
            ..
        } => {
            for branch in branches {
                add(branch.target, "payload fork")?;
            }
            if let Some(target) = default_target {
                add(*target, "payload fork default")?;
            }
        }
        Instr::End | Instr::EndTerminate | Instr::Fail { .. } => {}
        _ if address + 1 < len => result.push(address + 1),
        _ => {}
    }
    Ok(result)
}

fn require_address(
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
