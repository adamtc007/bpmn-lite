#![forbid(unsafe_code)]

use bpmn_lite_types::ffi_bindings::{apply_ffi_outputs, encode_ffi_inputs};
use bpmn_lite_types::{
    Addr, BufferedMessageMutation, Command, CommandEnvelope, ConcurrencyMutation,
    ConcurrencyRecord, ControlStackDelta, DedupeWrite, DurableEffect, EffectId, EffectMutation,
    EffectOutput, EffectTerminalState, ErrorClass, ExecutableWorkflow, Fiber, FlagKey, Incident,
    Instr, JobActivation, JobMutation, JoinId, JoinMutation, JournalCommand, JournalRecord,
    PersistedSnapshotState, ProcessInstance, ProcessState, RecordCounters, RecordId, RecordKind,
    RecordState, RuntimeEvent, Snapshot, SnapshotEnvelope, TerminalCleanup, TimerKind,
    TimerMutation, TimerRepeatSpec, Transition, TransitionBuilder, Uuid, Value, WaitState,
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
    fn next_revision(&self) -> u64 {
        self.next_revision
    }

    fn derived_id(&self, ordinal: u32) -> Uuid {
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
    /// V4.3 — Ring 3 shadow assert failure over the frame this transition
    /// would produce. See `bpmn_lite_types::IntegrityError::Ring3Runtime`.
    Integrity(bpmn_lite_types::IntegrityError),
    /// §18 ruling J (v0.10): superseded as `RoutePayload`/`ForkPayload`'s
    /// zero-match behaviour — a gateway matching none of its declared
    /// routes now raises an `Incident` via `fail_contract` (same
    /// mechanism `ForkInclusive`'s own zero-match arm already used) and
    /// stays resumable, rather than aborting the whole transition with a
    /// hard `Err`. Kept as a variant (not deleted — an API/serialization
    /// surface, not dead weight) since no call site constructs it
    /// anymore; retained for any external caller still matching on it.
    RouteNotMatched(String),
    /// §18 ruling K Part 2. `Instr::V2MiLoadElement` runs only after
    /// `Instr::V2MiIndexLive` has already confirmed `index < length`
    /// against the same `collection_flag` — reaching this error means the
    /// flag's value disagreed between the two checks despite no
    /// flag-mutating instruction running in between, or the flag was
    /// never a `Value::Array` at all. A genuine invariant violation, not a
    /// normal "empty/short collection" outcome — fail closed rather than
    /// substituting a default value.
    MultiInstanceElementUnavailable { flag: FlagKey, index: u32 },
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
            Self::Integrity(error) => write!(formatter, "{error}"),
            Self::RouteNotMatched(node) => {
                write!(formatter, "no deterministic route matched at {node}")
            }
            Self::MultiInstanceElementUnavailable { flag, index } => write!(
                formatter,
                "multi-instance element {index} unavailable in flag {flag} \
                 (collection changed between arity check and load, or is \
                 not a Value::Array)"
            ),
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

/// Fold one `Transition` into the next persisted snapshot. `pub` for the
/// same reason as `check_k_invariants` (V4.2): `replay` above and the
/// kernel fuzz harness (EOP-FUZZ-BPMN-ISA-002 F2) must consult the ONE
/// fold implementation — a harness-local reimplementation that drifted
/// from this would make every downstream oracle (K-invariants, limits
/// conformance, replay determinism) meaningless. Pure function, no
/// fuzz-only semantics.
pub fn materialize_snapshot(
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
            ConcurrencyMutation::Insert(record) => concurrency_table.insert((**record).clone()),
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

/// V4.2 — the K-1..K-3 kernel-preservation theorems (EOP-VS-BPMN-ISA-002
/// §7), checked directly against a materialized frame rather than argued
/// only in doc comments. Exposed (not `#[cfg(test)]`-gated) because V4.3's
/// Ring 3 shadow asserts are the same facts checked unconditionally in
/// production, at every park/resume, over live snapshot state instead of a
/// property test's synthetic one — this is the one implementation both
/// consult.
///
/// Scope: every check below is over `RecordState::Armed` records only.
/// §7's "live concurrency record" is the *currently open* structure being
/// tracked; a `Retired` record is a closed historical fact (kept, per
/// `RecordKind::Compensation`'s doc comment, because some kinds demand
/// history) and its membership is deliberately allowed to dangle — nothing
/// un-registers a fibre from a record's `members` set on retirement, only
/// the record's own `state` flips and (for barriers) `Retire`/`Remove`
/// mutations delete losing members' fibres outright. K-2's "for live F" is
/// scoped to fibres for the same reason: a retired record does not
/// obligate any surviving fibre's control stack to still reference it.
///
/// K-2 is checked against each fibre's *effective* stack, not the literal
/// `control_stack` field: `V2Join` (non-last-arrival) and `V2RaceClose`
/// both pop their handle before parking, moving it into
/// `WaitState::V2Barrier`/`WaitState::V2Race` instead — the handle is still
/// logically held (the record still lists the fibre as a member, correctly
/// — it may yet be the one that resolves the barrier/race), just relocated
/// off the vector while parked. `v2_cancel_guard_scope`'s tree walk already
/// treats this construction as canonical (`chain = control_stack.clone()`
/// + wait-derived handle); this function reuses the same one.
pub fn effective_control_stack(fiber: &Fiber) -> Vec<RecordId> {
    let mut chain = fiber.control_stack.clone();
    match &fiber.wait {
        WaitState::V2Barrier { record_id } | WaitState::V2Race { record_id, .. } => {
            chain.push(*record_id);
        }
        _ => {}
    }
    chain
}

pub fn check_k_invariants(
    fibers: &std::collections::BTreeMap<Uuid, Fiber>,
    table: &bpmn_lite_types::concurrency::ConcurrencyTable,
) -> Result<(), String> {
    for (record_id, record) in table.iter() {
        if record.state != RecordState::Armed {
            continue;
        }
        // K-1: every member of a live record references a live fibre.
        for member in &record.members {
            if !fibers.contains_key(member) {
                return Err(format!(
                    "K-1 violated: record {record_id} (armed) has member {member}, no live fibre"
                ));
            }
        }
        // K-3: for live barriers, 0 < count <= arity — a barrier that hit
        // zero must have retired in the same transition it did, so an
        // *armed* barrier observed with count == 0 is itself a violation.
        if record.kind == RecordKind::Barrier {
            let RecordCounters { arity, count } = record.counters;
            if count == 0 || count > arity {
                return Err(format!(
                    "K-3 violated: barrier {record_id} armed with count={count}, arity={arity}"
                ));
            }
        }
    }

    // K-2: for every live fibre F, h on F's effective stack iff F is a
    // member of armed record h. Checked both directions.
    for (fiber_id, fiber) in fibers {
        for handle in &effective_control_stack(fiber) {
            match table.get(*handle) {
                Some(record) if record.state == RecordState::Armed => {
                    if !record.members.contains(fiber_id) {
                        return Err(format!(
                            "K-2 violated: fibre {fiber_id} has handle {handle} on its \
                             control stack, but is not a member of record {handle}"
                        ));
                    }
                }
                Some(record) => {
                    return Err(format!(
                        "K-2 violated: fibre {fiber_id} has handle {handle} on its control \
                         stack, but record {handle} is {:?}, not Armed",
                        record.state
                    ));
                }
                None => {
                    return Err(format!(
                        "K-2 violated: fibre {fiber_id} has handle {handle} on its control \
                         stack, but no such record exists"
                    ));
                }
            }
        }
    }
    for (record_id, record) in table.iter() {
        if record.state != RecordState::Armed {
            continue;
        }
        for member in &record.members {
            let Some(fiber) = fibers.get(member) else {
                continue; // already reported as a K-1 violation above
            };
            if !effective_control_stack(fiber).contains(record_id) {
                return Err(format!(
                    "K-2 violated: fibre {member} is a member of armed record {record_id}, \
                     but does not have it on its control stack"
                ));
            }
        }
    }
    Ok(())
}

/// V4.3 — folds a `Transition`'s deltas onto `snapshot`'s fibre map and
/// concurrency table, the minimum needed for Ring 3's shadow asserts (no
/// joins/incidents/pending-effects bookkeeping — those aren't part of
/// what Ring 3 checks). Mirrors `materialize_snapshot`'s fiber/table
/// folding exactly; kept separate because that function also threads
/// join counts and incidents through `PersistedSnapshotState`, which
/// `apply` itself has no need of and no access to (only the store layer
/// carries that forward).
/// Shared set-arithmetic core of `derive_post_transition_frame`: the live
/// fibre map after folding a transition's fibre deltas onto `pre` — insert/
/// overwrite every upserted fibre, then remove every deleted id (idempotent,
/// so duplicates in `deletes` are harmless), then clear entirely if this is
/// a full-cleanup terminal transition. Factored out (not just inlined in
/// `derive_post_transition_frame`) so callers that don't yet have a built
/// `Transition` — e.g. `Instr::End` deciding, mid-instruction, whether *this*
/// deletion is the one that empties the fibre set — can ask the identical
/// question against the in-progress `Changes` instead of hand-rolling new
/// arithmetic. See `Instr::End`'s doc comment for why the pre-transition
/// fibre count alone is not a safe way to answer "is this the last fibre?".
fn apply_fiber_deltas<'a>(
    pre: &std::collections::BTreeMap<Uuid, Fiber>,
    upserts: impl IntoIterator<Item = &'a Fiber>,
    deletes: impl IntoIterator<Item = &'a Uuid>,
    delete_all_fibers: bool,
) -> std::collections::BTreeMap<Uuid, Fiber> {
    if delete_all_fibers {
        return std::collections::BTreeMap::new();
    }
    let mut fibers = pre.clone();
    for fiber in upserts {
        fibers.insert(fiber.fiber_id, fiber.clone());
    }
    for fiber_id in deletes {
        fibers.remove(fiber_id);
    }
    fibers
}

fn derive_post_transition_frame(
    snapshot: &Snapshot,
    transition: &Transition,
) -> (
    std::collections::BTreeMap<Uuid, Fiber>,
    bpmn_lite_types::concurrency::ConcurrencyTable,
) {
    let fibers = apply_fiber_deltas(
        snapshot.fibers(),
        transition.fibers_upsert(),
        transition.fibers_delete(),
        transition.terminal_cleanup().delete_all_fibers(),
    );
    let mut table = snapshot.concurrency_table().clone();
    for mutation in transition.concurrency_mutations() {
        match mutation {
            ConcurrencyMutation::Insert(record) => table.insert((**record).clone()),
            ConcurrencyMutation::Retire(id) => {
                if let Some(record) = table.get_mut(*id) {
                    record.state = RecordState::Retired;
                }
            }
            ConcurrencyMutation::Remove(id) => {
                table.remove(*id);
            }
        }
    }
    (fibers, table)
}

/// V4.3 — Ring 3 (V&S §6): unconditional structural asserts over the
/// frame a transition would produce, run on *every* `apply` call (every
/// call either parks a fibre or resumes/advances one — there is no
/// separate "park" vs "resume" code path to distinguish, so checking the
/// result unconditionally realizes "at every park/resume" exactly).
/// Fail-closed: a verified program under a proven kernel yielding a
/// structurally invalid frame has one explanation — corruption.
///
/// Checks, each O(fibres + records):
/// - PC within program, for every touched (`fibers_upsert`) fibre.
/// - Operand/control stack heights within `VerifiedLimits`, same scope.
/// - K-1/K-2/K-3 shadows (`check_k_invariants`) — subsumes "every handle
///   resolves in the concurrency table" (K-2 already rejects a dangling
///   handle) and "barrier counts <= static arity" (K-3).
/// - Every pending effect owned by exactly one waiting fibre: no two
///   fibres in the resulting frame reference the same `EffectId`, whether
///   parked directly (`WaitState::Effect`) or as a race alternative
///   (`WaitState::V2Race`'s `Effect` arms).
/// - A non-terminal instance has ≥1 live fibre in the post-transition
///   frame. Zero live fibres with a still-`Running` (or otherwise
///   non-terminal) `ProcessState` is stuck by construction — nothing is
///   left to drive the instance forward. This is the general catch for the
///   `Instr::End` pre-transition-count bug class (see its doc comment):
///   any transition, from any instruction, that lands here with an empty
///   fibre set and a non-terminal state is the same defect shape, whatever
///   produced it.
fn ring3_shadow_check(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    transition: &Transition,
) -> Result<(), bpmn_lite_types::IntegrityError> {
    let (fibers, table) = derive_post_transition_frame(snapshot, transition);
    let limits = workflow.envelope().limits();
    let program_len = workflow.envelope().instructions().len();

    // Effective post-transition state: `state_override`, when present,
    // wins over `next_snapshot().state` — mirrors `materialize_snapshot`'s
    // own resolution of the two, so this check judges the same "final
    // state" the store layer will actually persist, not an intermediate
    // value the kernel itself never treats as authoritative.
    let effective_state = transition
        .state_override()
        .unwrap_or(&transition.next_snapshot().state);
    if fibers.is_empty() && !effective_state.is_terminal() {
        return Err(bpmn_lite_types::IntegrityError::Ring3Runtime(format!(
            "instance {} has zero live fibres post-transition but state {:?} is non-terminal \
             — stuck by construction",
            snapshot.instance().instance_id,
            effective_state
        )));
    }

    for fiber in transition.fibers_upsert() {
        if fiber.pc.index() >= program_len {
            return Err(bpmn_lite_types::IntegrityError::Ring3Runtime(format!(
                "fibre {} pc {} out of bounds (program length {program_len})",
                fiber.fiber_id, fiber.pc
            )));
        }
        if fiber.stack.len() > limits.max_stack() as usize {
            return Err(bpmn_lite_types::IntegrityError::Ring3Runtime(format!(
                "fibre {} operand stack height {} exceeds verified limit {}",
                fiber.fiber_id,
                fiber.stack.len(),
                limits.max_stack()
            )));
        }
        if fiber.control_stack.len() > limits.max_control_depth() as usize {
            return Err(bpmn_lite_types::IntegrityError::Ring3Runtime(format!(
                "fibre {} control-stack depth {} exceeds verified limit {}",
                fiber.fiber_id,
                fiber.control_stack.len(),
                limits.max_control_depth()
            )));
        }
    }

    check_k_invariants(&fibers, &table).map_err(bpmn_lite_types::IntegrityError::Ring3Runtime)?;

    // Part 2 investigation (Adam's ruling, 2026-07-22, barrier-starvation
    // hypothesis — confirmed real, see
    // `fork_join_barrier_starves_when_a_member_dies_without_arriving_via_guard_rollback`):
    // a live (Armed) `Barrier` record's outstanding arrival count
    // (`counters.count`) must be satisfiable by its own live, not-yet-
    // arrived membership. A member fibre that dies without ever arriving
    // (e.g. cancelled by an enclosing guard's rollback, or by an
    // interrupting guard trigger, or any other same-transition fibre
    // deletion reachable while it's still a barrier member) is correctly
    // deregistered from `record.members` by
    // `v2_reconcile_ancestor_membership` — but nothing anywhere
    // decrements `record.counters.count` to match: `Instr::V2Join` is the
    // *only* code path that ever decrements it, and only on a genuine
    // arrival. Detection, not remediation (open fork, not decided here —
    // see the plan doc): every live member of an Armed barrier is,
    // individually, either already arrived (parked in
    // `WaitState::V2Barrier` for this exact record — it can never arrive
    // a second time) or not yet arrived (anything else — still capable of
    // producing exactly one future arrival). If fewer live members remain
    // capable of a future arrival than the record still demands, the
    // barrier is structurally dead: no sequence of further commands can
    // ever retire it.
    for (record_id, record) in table.iter() {
        if record.state != RecordState::Armed || record.kind != RecordKind::Barrier {
            continue;
        }
        let not_yet_arrived = record
            .members
            .iter()
            .filter(|member| {
                fibers.get(*member).is_some_and(|fiber| {
                    fiber.wait
                        != (WaitState::V2Barrier {
                            record_id: *record_id,
                        })
                })
            })
            .count() as u32;
        if not_yet_arrived < record.counters.count {
            return Err(bpmn_lite_types::IntegrityError::Ring3Runtime(format!(
                "barrier {record_id} armed with outstanding count={} but only \
                 {not_yet_arrived} live not-yet-arrived member(s) remain among {} live \
                 member(s) — structurally unsatisfiable, stuck by construction",
                record.counters.count,
                record.members.len(),
            )));
        }
    }

    let mut effect_owners: std::collections::BTreeMap<EffectId, Uuid> =
        std::collections::BTreeMap::new();
    for fiber in fibers.values() {
        let referenced: Vec<EffectId> = match &fiber.wait {
            WaitState::Effect { effect_id } => vec![*effect_id],
            WaitState::V2Race { arms, .. } => arms
                .iter()
                .filter_map(|arm| match arm {
                    bpmn_lite_types::V2RaceArm::Effect { effect_id, .. } => Some(*effect_id),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        for effect_id in referenced {
            if let Some(other) = effect_owners.insert(effect_id, fiber.fiber_id)
                && other != fiber.fiber_id
            {
                return Err(bpmn_lite_types::IntegrityError::Ring3Runtime(format!(
                    "effect {effect_id:?} owned by both fibre {other} and fibre {}",
                    fiber.fiber_id
                )));
            }
        }
    }
    Ok(())
}

/// §18 ruling K Part 2. Shared by `V2MiIndexLive`/`V2MiArityCheck`: the
/// runtime length of an MI collection, read as `instance.flags[flag]`'s
/// `Value::Array` length. A missing key or any non-`Array` value defaults
/// to length 0 — the "legal empty collection" default ruling K item (c)
/// already ratifies, applied uniformly now that both words read the same
/// flag the same way instead of one reading an `I64` and one an `Array`.
fn mi_collection_len(instance: &ProcessInstance, flag: &FlagKey) -> i64 {
    match instance.flags.get(flag) {
        Some(Value::Array(items)) => items.len() as i64,
        _ => 0,
    }
}

/// Pure transition function. It performs no I/O and obtains time and identity only
/// from `DeterministicContext` or the durable command.
pub fn apply(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    command: &Command,
    context: &DeterministicContext,
) -> Result<Transition, TransitionError> {
    // §18 ruling K Part 2 defense-in-depth (blind-review finding, see
    // docs/todo/EOP-PLAN-BPMN-ISA-002.md "poisoned-instance" writeup):
    // `Command::Cancel`/`Command::Terminate` both unconditionally overwrite
    // `instance.state` (to `Cancelled`/`Terminated`) and emit a
    // `TerminalCleanup` without ever reading `instance.flags` or any
    // fiber's `stack`/`regs` contents (see their arms below — `Cancel`
    // only reads `fiber.wait` to describe cancelled waits; `Terminate`
    // only reads a fiber id from `snapshot.fibers().keys()`). Neither
    // depends on `Value::Array` size/depth being within bounds, so an
    // instance already poisoned by an oversized/deep `Value::Array` in a
    // flag (via a path that predates or otherwise evades the gRPC-boundary
    // check in `bpmn-lite-server/src/grpc.rs::check_orch_flags`) must
    // still be able to reach `Cancelled`/`Terminated` — otherwise it is a
    // permanent zombie with no exposed remedy (not `Incidented`, so
    // `ResolveIncident` does not apply either). The exemption is scoped
    // to exactly the array-size/depth portion of `validate_snapshot_limits`
    // — fiber-count and per-fiber stack-length/register-count checks still
    // run unconditionally for every command, `Cancel`/`Terminate` included,
    // since those are unrelated to the poisoning and remain load-bearing.
    let check_arrays = !matches!(command, Command::Cancel { .. } | Command::Terminate);
    validate_snapshot_limits(workflow, snapshot, check_arrays)?;
    let transition = match command {
        Command::TimerFired { timer, fired_at } => {
            apply_timer(workflow, snapshot, timer, *fired_at, context)
        }
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
            builder = builder
                .event(RuntimeEvent::Cancelled {
                    reason: reason.clone(),
                })
                .terminal_cleanup(TerminalCleanup::new(true, true, true));
            Ok(retire_all_armed_records(snapshot, builder).build())
        }
        Command::Tick { .. } => apply_tick(workflow, snapshot, command, context),
        Command::EffectCompleted {
            output: EffectOutput::Job(completion),
            ..
        } => apply_job_completion(snapshot, completion),
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
            let builder = TransitionBuilder::new(next)
                .event(RuntimeEvent::Terminated { at, fiber_id })
                .terminal_cleanup(TerminalCleanup::new(true, true, true));
            Ok(retire_all_armed_records(snapshot, builder).build())
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
            // Fail closed: only an instance actually parked on THIS incident
            // (ProcessState::Incidented { incident_id }) may be revived to
            // Running. Without this, an instance that moved on to some other
            // state (e.g. Cancelled, while an Incident record was still
            // unresolved) would be silently yanked back to Running by a
            // late-arriving ResolveIncident — reviving a state the instance
            // never asked to leave.
            let parked_on_this_incident = matches!(
                &snapshot.instance().state,
                ProcessState::Incidented { incident_id: parked_id } if *parked_id == *incident_id
            );
            if !parked_on_this_incident {
                return Err(TransitionError::InvalidCommand(
                    "ResolveIncident requires the instance to be Incidented on this incident_id",
                ));
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
    ring3_shadow_check(workflow, snapshot, &transition).map_err(TransitionError::Integrity)?;
    let logical_time = i64::try_from(context.logical_time())
        .map_err(|_| TransitionError::NumericOverflow("logical time"))?;
    Ok(transition.with_command_envelope(CommandEnvelope::new(
        context.command_id(),
        logical_time,
        JournalCommand::Kernel(command.clone()),
    )))
}

/// `check_arrays` scopes the `Value::Array` size/depth walk (both the
/// per-fiber stack/register scan and the `instance.flags` scan below); it
/// is `false` only for `Command::Cancel`/`Command::Terminate` (see the
/// call site in `apply` for why that exemption is safe). Fiber-count and
/// per-fiber stack-length/register-count checks are NOT gated by this flag
/// — they run for every command unconditionally, `Cancel`/`Terminate`
/// included, since they are unrelated to `Value::Array` poisoning.
fn validate_snapshot_limits(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    check_arrays: bool,
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
        // §18 ruling K Part 2 finding: `max_stack` bounds *slot count*, which
        // was sufficient while every `Value` variant was fixed-width.
        // `Value::Array` is not — a single stack slot can now hold an
        // arbitrarily large/deep tree, so slot count alone no longer bounds
        // this fibre's actual frame size. Walk every `Value` this fibre
        // carries and reject any that exceeds
        // `types::MAX_VALUE_ARRAY_LEN`/`MAX_VALUE_ARRAY_DEPTH`. Skipped for
        // `Cancel`/`Terminate` (`check_arrays == false`) — see doc comment
        // above.
        if check_arrays {
            for value in fiber.stack.iter() {
                if let Err(limit_error) = value.check_array_limits() {
                    return Err(TransitionError::ResourceLimitExceeded {
                        resource: "Value::Array size/depth",
                        actual: match limit_error {
                            bpmn_lite_types::types::ValueLimitError::TooLong { actual, .. } => {
                                actual
                            }
                            bpmn_lite_types::types::ValueLimitError::TooDeep { max } => {
                                max as usize
                            }
                        },
                        limit: match limit_error {
                            bpmn_lite_types::types::ValueLimitError::TooLong { max, .. } => {
                                max as u64
                            }
                            bpmn_lite_types::types::ValueLimitError::TooDeep { max } => {
                                u64::from(max)
                            }
                        },
                    });
                }
            }
        }
    }
    // Same bound, applied to `instance.flags` — this is the boundary that
    // (on the instance's first `apply` call after spawn/resume) also
    // catches an oversized/deep `Value::Array` supplied externally via
    // `orch_flags`. The gRPC boundary (`bpmn-lite-server/src/grpc.rs`,
    // `check_orch_flags`) now also rejects an oversized/deep array before
    // it ever reaches `orch_flags`/`instance.flags` in the first place;
    // this scan remains as the runtime backstop for any instance already
    // poisoned before that fix landed, or via any other path. Skipped for
    // `Cancel`/`Terminate` (`check_arrays == false`) — see doc comment
    // above.
    if check_arrays {
        for value in snapshot.instance().flags.values() {
            if let Err(limit_error) = value.check_array_limits() {
                return Err(TransitionError::ResourceLimitExceeded {
                    resource: "Value::Array size/depth (flag)",
                    actual: match limit_error {
                        bpmn_lite_types::types::ValueLimitError::TooLong { actual, .. } => actual,
                        bpmn_lite_types::types::ValueLimitError::TooDeep { max } => max as usize,
                    },
                    limit: match limit_error {
                        bpmn_lite_types::types::ValueLimitError::TooLong { max, .. } => max as u64,
                        bpmn_lite_types::types::ValueLimitError::TooDeep { max } => u64::from(max),
                    },
                });
            }
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
    // Parked on an open Incident, resumable via Command::ResolveIncident —
    // Incidented, not Failed (the latter means genuinely dead forever).
    next.state = ProcessState::Incidented { incident_id };
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
            // V5 dynamic-arity gateway lowering glue (§18 rulings H/J, not
            // a ratified D2 word — see the `Instr` doc comment). DSL-side
            // analogue of `LoadFlag` above: pushes the placeholder-match
            // boolean `Instr::ForkPayload`'s kernel handler already
            // computes internally, exposing it as a straight-line
            // operand-stack producer so a V2Fork-based DSL inclusive-split
            // lowering can build its own `BrIf`/`BrIfNot` checks the same
            // way XML's `LoadFlag`-based lowering does.
            Instr::V2LoadPlaceholderMatch {
                placeholder,
                expected_value,
            } => {
                let matched = instance.placeholder_matches(placeholder, expected_value);
                fiber.stack.push(Value::Bool(matched));
                fiber.pc = fiber.pc.saturating_add(1);
            }
            // Unconditional zero-match incident (§18 ruling J), reached
            // only when a chain of `LoadFlag`/`V2LoadPlaceholderMatch` +
            // `BrIf`-to-`V2Fork` checks ahead of it all fell through —
            // i.e. no branch's condition was true and there is no
            // always-live (unconditional/default) branch. Same helper,
            // same message shape, as `RoutePayload`/`ForkPayload`/
            // `ForkInclusive`'s own zero-match arms — not a new mechanism.
            Instr::V2RouteZeroMatch => {
                let node = metadata
                    .debug_map()
                    .get(&fiber.pc)
                    .cloned()
                    .unwrap_or_else(|| format!("pc_{}", fiber.pc));
                return fail_contract(
                    instance,
                    fiber,
                    changes,
                    context,
                    &format!(
                        "inclusive gateway at {node} matched none of its declared \
                         branches and has no default target"
                    ),
                );
            }
            // V5 dynamic-arity MI lowering glue (§18 ruling K, not a
            // ratified D2 word — see the `Instr` doc comment). MI's
            // `inputCollection` is represented, for this step, by its
            // runtime LENGTH only (an `I64` flag) — no array/collection
            // value type exists anywhere in this ISA (a real, recorded
            // finding, not an oversight). A missing key or a non-`I64`
            // value defaults to length 0 — the same default `LoadFlag`
            // already uses for a missing bool flag, and exactly ruling K
            // item (c)'s "empty collection is legal" default, not a
            // special case bolted on here.
            // §18 ruling K Part 2: `collection_flag` now names a flag
            // holding the collection's actual `Value::Array`, not a
            // separate `I64` length — length is derived from `.len()`, a
            // single source of truth (see the `Instr` doc comment for why
            // the prior two-flag design was rejected).
            Instr::V2MiIndexLive {
                collection_flag,
                index,
            } => {
                let length = mi_collection_len(&instance, collection_flag);
                let live = i64::from(*index) < length;
                fiber.stack.push(Value::Bool(live));
                fiber.pc = fiber.pc.saturating_add(1);
            }
            // Only reached on the live path (`V2MiIndexLive` + `BrIfNot`
            // already confirmed `index < length` against the same flag
            // immediately before this runs, with no flag-mutating
            // instruction in between within one straight-line branch).
            Instr::V2MiLoadElement {
                collection_flag,
                index,
            } => {
                let element = match instance.flags.get(collection_flag) {
                    Some(Value::Array(items)) => items.get(*index as usize).cloned(),
                    _ => None,
                };
                let element = element.ok_or(TransitionError::MultiInstanceElementUnavailable {
                    flag: *collection_flag,
                    index: *index,
                })?;
                fiber.stack.push(element);
                fiber.pc = fiber.pc.saturating_add(1);
            }
            // Runs once, straight-line, immediately before the MI
            // `V2Fork` — see the `Instr` doc comment for why this must
            // precede `V2Fork` rather than run inside or after it, and
            // for why exceeding the declared max is a hard
            // `ResourceLimitExceeded` reject, not an `Incident` (ruling
            // K's own "not unified with zero-match" text, applied to a
            // different runtime-zero case: this one is over-capacity, not
            // under). Bounds element *count* only — total encoded size is
            // bounded separately, see the `Instr` doc comment.
            Instr::V2MiArityCheck {
                collection_flag,
                max,
            } => {
                let length = mi_collection_len(&instance, collection_flag);
                if length > i64::from(*max) {
                    return Err(TransitionError::ResourceLimitExceeded {
                        resource: "multi-instance collection length",
                        actual: length.max(0) as usize,
                        limit: u64::from(*max),
                    });
                }
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
                    apply_completion(&mut instance, completion)?;
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
                // V5.3 (§18, landed 2026-07-23): the v1 `race_plan`/
                // `boundary_map` boundary-timer-promotion mechanism this
                // arm used to consult here — "structure consulted as
                // state," D1's forbidden pattern — is deleted. Boundary
                // timers on service tasks now lower exclusively to
                // `V2Guard`/`V2GuardN` + `GUARD-TIMER>` wrapping this same
                // `ExecNative` (§18 ruling I), which needs no cooperation
                // from the task-dispatch arm itself — the guard scope races
                // independently around it. This arm always parks on the
                // job alone.
                fiber.wait = WaitState::Job { job_key };
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
                    apply_completion(&mut instance, completion)?;
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
                match target.or(*default_target) {
                    Some(next_pc) => fiber.pc = next_pc,
                    None => {
                        // §18 ruling J: zero-match raises an Incident, not
                        // a hard transition error — the workflow COULDN'T
                        // decide, which is what an incident means (not a
                        // decision to stop, `END-TERMINATE`'s meaning).
                        // Supersedes the old `TransitionError::RouteNotMatched`
                        // hard-abort here; reuses `fail_contract` verbatim,
                        // the same helper `Instr::ForkInclusive`'s own
                        // zero-match arm already calls for the identical
                        // shape ("gateway matched none of its declared
                        // routes") — not a new mechanism, the existing one
                        // applied where it was previously missing.
                        let node = metadata
                            .debug_map()
                            .get(&fiber.pc)
                            .cloned()
                            .unwrap_or_else(|| format!("pc_{}", fiber.pc));
                        return fail_contract(
                            instance,
                            fiber,
                            changes,
                            context,
                            &format!(
                                "exclusive route at {node} matched none of its declared \
                                 branches and has no default target"
                            ),
                        );
                    }
                }
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
            // K-1/K-2/K-3 (V4.2): touches no concurrency record and pushes
            // nothing onto the control stack — vacuously preserves all
            // three (the fibre's own set of held handles is unchanged).
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
            Instr::PublishMessage { name } => {
                let correlation_key =
                    resolve_correlation_key_at(workflow, &instance, fiber.pc)?;
                let message_name = workflow
                    .envelope()
                    .metadata()
                    .message_name_map()
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| name.to_string());
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
            Instr::End => {
                // Was: `snapshot.fibers().len() == 1` — the *pre*-transition
                // fibre count. That's only a safe proxy for "this is the
                // last fibre" when deletion and `End` never coincide with
                // an in-flight `V2Join` retirement in the same transition.
                // Ruling B (last arrival survives and falls through into
                // whatever follows the join, in the same step) makes that
                // coincidence routine: a `V2Join` immediately followed by
                // `End` (fork → two branches → join → end, the single most
                // common BPMN shape) retires the barrier — pushing its
                // cancelled siblings onto `fibers_delete` — then falls
                // through to this arm in the same transition, which pushes
                // this fibre's own id too. The pre-transition count never
                // reflects any of that, so the instance never observed
                // itself reaching zero live fibres: `fibers_delete` still
                // gets processed (the fibres really are deleted), but
                // `instance.state` stays `Running` forever with no fibre
                // left to advance it. Permanently stuck, silently.
                //
                // Fixed per the set formulation: live-after = (pre-transition
                // ∪ newly-upserted) − deleted, checked for emptiness against
                // this instruction's own net effect so far — not a stale
                // snapshot count.
                changes.fibers_delete.push(fiber.fiber_id);
                let live_after = apply_fiber_deltas(
                    snapshot.fibers(),
                    &changes.fibers_upsert,
                    &changes.fibers_delete,
                    false,
                );
                if live_after.is_empty() {
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
                // V-1's fix (v2_verifier.rs) exempts `EndTerminate` from the
                // empty-control-stack requirement — unlike `End`, it kills
                // the WHOLE instance, so no fibre's own open scope can be
                // orphaned (every fibre dies with it). That exemption's
                // rider, the K-invariant side: `TerminalCleanup`'s
                // `delete_all_fibers`/`delete_all_joins` above clear
                // `fibers`/`joins` but never touch `concurrency_table` — an
                // enclosing `V2Fork` barrier or an open guard anywhere else
                // in the instance would otherwise survive forever as a
                // zero-member `Armed` record, never retired (a real K-1/K-2
                // orphaned-record hazard, unguarded by the verifier once it
                // stopped requiring the pop for this instruction
                // specifically). `Instr::End` needs no equivalent: its own
                // V-1 check still requires this fibre's stack to be empty,
                // and `End` only fires once every fibre is dead (the
                // pre-condition below), so by construction every other
                // fibre's own scopes already closed (and retired) through
                // their own normal scope-close instructions before this
                // point — there is nothing left open to sweep. Bulk
                // retirement, not a subtree walk (`v2_cancel_guard_scope`'s
                // shape doesn't apply — there's no single root; every
                // still-`Armed` record anywhere in the table is in scope,
                // since the whole instance is ending). Folds `changes`
                // staged earlier this tick, not just `snapshot`: a record
                // opened immediately before this instruction (no
                // intervening park) exists only in `changes`.
                for record_id in armed_record_ids_in_transition(snapshot, &changes) {
                    changes
                        .concurrency_mutations
                        .push(ConcurrencyMutation::Retire(record_id));
                }
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
            // open/close and the fork/barrier/join pair.
            //
            // K-1/K-2 (V4.2): allocates a fresh `Armed` record and pushes
            // its handle onto the opening fibre's own stack, registering
            // that same fibre as the record's sole initial member in the
            // same step — the two canonical facts are established
            // together, by construction. K-3: n/a (not a barrier).
            Instr::V2Guard { handler } => {
                let record_id = RecordId::new(context.derived_id(ordinal));
                ordinal = ordinal.saturating_add(1);
                let mut record = ConcurrencyRecord {
                    handler: Some(*handler),
                    // A18 (supersedes V4.1's original ruling): `V2Guard`
                    // is control-only — unwind members, spawn the handler
                    // — no rollback snapshot is captured here any more.
                    // That is now `V2GuardR`'s exclusive data disposition
                    // (see `ConcurrencyRecord`'s doc comment). Leaving
                    // every `rollback_*` field at `ConcurrencyRecord::new`'s
                    // `None` default is exactly the point: a plain
                    // `V2Guard` handle is rollback-INcapable by
                    // construction, so `V2CancelScope`/automatic
                    // rollback-on-failure both reject it rather than
                    // silently restoring nothing.
                    // V&S §15 (v0.7) ruling F: the guard's own static
                    // address, since `record_id` doesn't survive a re-open
                    // — `fiber.pc` here IS this V2Guard instruction's own
                    // address (not yet advanced past it).
                    opened_at: Some(fiber.pc),
                    ..ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: true })
                };
                // K-1/K-2 (V4.2): the opening fibre is this record's first
                // member — omitting this left every guard record's
                // `members` permanently empty (V2RaceOpen/V2Fork already
                // register their own opener/children; guards did not).
                record.members.insert(fiber.fiber_id);
                changes
                    .concurrency_mutations
                    .push(ConcurrencyMutation::Insert(Box::new(record)));
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
            // K-1/K-2 (V4.2): the closing fibre pops its own handle and
            // the record retires (moot membership henceforth, per
            // `check_k_invariants`'s doc comment); no other live fibre can
            // hold this handle (SESE nesting — a guard's only members are
            // fibres inside it, and none survive its own close without
            // also popping). K-3: n/a.
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
            // K-1/K-2/K-3 (V4.2): allocates a fresh barrier with `arity =
            // count = |targets|` (K-3's bound holds at birth: 0 <
            // |targets| <= |targets|, verifier admits only non-empty
            // fork target lists), registers every child as a member as it
            // spawns them (K-1/K-2 for the barrier itself), and — since
            // the forking fibre dies in this same step — reconciles every
            // *ancestor* handle already on its stack via
            // `v2_reconcile_ancestor_membership`: the dying fibre is
            // removed and every child added, in one batched update per
            // ancestor (K-1/K-2 for enclosing scopes).
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
                    child_fibers: ids.clone(),
                    targets: targets.to_vec(),
                });
                changes
                    .concurrency_mutations
                    .push(ConcurrencyMutation::Insert(Box::new(record)));
                // K-1/K-2 (V4.2): the children inherit every ancestor
                // handle already on the forking fibre's own stack (e.g. an
                // enclosing `V2Guard`), and the forking fibre itself is
                // about to die — both sides of those ancestor records'
                // `members` need to move with it, or the ancestor is left
                // either missing the new members (K-2) or still listing a
                // now-dead one (K-1).
                let mut ancestor_ops = Vec::new();
                for ancestor in &fiber.control_stack {
                    ancestor_ops.push((*ancestor, MembershipOp::Remove(fiber.fiber_id)));
                    for child_id in &ids {
                        ancestor_ops.push((*ancestor, MembershipOp::Add(*child_id)));
                    }
                }
                v2_reconcile_ancestor_membership(
                    snapshot,
                    &ancestor_ops,
                    &std::collections::BTreeSet::new(),
                    &mut changes,
                );
                changes.fibers_delete.push(fiber.fiber_id);
                return Ok(changes.finish(instance));
            }
            // K-3 (V4.2): decrements `count` by exactly one per arrival,
            // never below zero (`saturating_sub`, and the verifier's
            // static arity check bounds how many arrivals a barrier can
            // ever see) — retirement fires exactly at zero, in the same
            // step. K-1/K-2, non-last arrival: the arriving fibre pops the
            // handle into `WaitState::V2Barrier` instead of the literal
            // stack — `effective_control_stack`/`v2_cancel_guard_scope`
            // treat that as equivalent, so membership stays intact. K-1/
            // K-2, last arrival: every other member is deleted outright
            // (K-1 — no live fibre left to reference), and
            // `v2_reconcile_ancestor_membership` drops each from any
            // *ancestor* record above the retiring barrier that isn't
            // itself retiring in this same step.
            Instr::V2Join { pairing: _ } => {
                let handle = fiber
                    .control_stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2Join"))?;
                changes.control_stack_deltas.push(ControlStackDelta::Pop {
                    fiber_id: fiber.fiber_id,
                    handle,
                });
                // `fetch_record_in_transition`, not a raw `snapshot` read:
                // a nested barrier's survivor can pop an inner V2Join
                // (whose cancellation reconciles the OUTER barrier's own
                // membership, staged into `changes`) and then, without
                // blocking, immediately execute the OUTER V2Join in the
                // SAME transition — a raw `snapshot` read here would see
                // the outer record as it stood before that reconciliation
                // and silently clobber it on this arm's own re-`Insert`.
                let mut record = fetch_record_in_transition(snapshot, &changes, handle).ok_or(
                    TransitionError::InvalidCommand("V2Join: unknown barrier handle"),
                )?;
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
                    // K-1 (V4.2): a cancelled member may still carry
                    // ancestor handles beyond `handle` (e.g. the enclosing
                    // `V2Guard`) — `handle` itself is retiring in this same
                    // transition so its own membership is moot, but any
                    // outer ancestor still `Armed` must drop the now-dead
                    // fibre or it dangles.
                    let mut ancestor_ops = Vec::new();
                    for member in &cancelled {
                        if let Some(member_fiber) = snapshot.fibers().get(member) {
                            for ancestor in &member_fiber.control_stack {
                                ancestor_ops.push((*ancestor, MembershipOp::Remove(*member)));
                            }
                        }
                    }
                    v2_reconcile_ancestor_membership(
                        snapshot,
                        &ancestor_ops,
                        &std::collections::BTreeSet::from([handle]),
                        &mut changes,
                    );
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
                        .push(ConcurrencyMutation::Insert(Box::new(record)));
                    changes.events.push(RuntimeEvent::V2JoinArrived {
                        record_id: handle,
                        fiber_id: fiber.fiber_id,
                    });
                    fiber.wait = WaitState::V2Barrier { record_id: handle };
                    changes.fibers_upsert.push(fiber);
                    return Ok(changes.finish(instance));
                }
            }
            // K-1/K-2/K-3 (V4.2): allocates a fresh race with the opening
            // fibre as its sole member and pushes the handle onto that
            // same fibre's stack in the same step (as `V2Guard`). K-3:
            // `arity = count = arm_count`; a race isn't a `Barrier`-kind
            // record so K-3's literal bound doesn't apply to it, but the
            // same 0-at-birth-would-be-a-bug shape holds by construction
            // (the verifier admits only `arm_count >= 1`).
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
                    .push(ConcurrencyMutation::Insert(Box::new(record)));
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
            // K-1/K-2/K-3 (V4.2): arms an alternative on the already-open
            // race (opened by `V2RaceOpen` in this same tick, per the
            // module's `race_arms` accumulator) — schedules a timer effect
            // but touches no concurrency record and pushes/pops nothing;
            // the fibre's held-handle set is unchanged.
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
            // K-1/K-2/K-3 (V4.2): as `V2ArmTimer` — records the
            // alternative for `V2RaceClose` to park on; no concurrency
            // record or control-stack change.
            Instr::V2ArmMsg { target, name } => {
                let corr_key = resolve_correlation_key_at(workflow, &instance, fiber.pc)?;
                race_arms.push(bpmn_lite_types::V2RaceArm::Msg {
                    target: *target,
                    name: *name,
                    corr_key,
                });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            // K-1/K-2/K-3 (V4.2): as `V2ArmTimer`/`V2ArmMsg` — the effect
            // invocation dispatches immediately but, like its siblings,
            // touches no concurrency record or control stack; the arm's
            // `effect_id` is the resolution key `apply_ffi_completion`
            // matches later, not a handle.
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
            // K-1/K-2 (V4.2): pops the race handle off the literal control
            // stack and moves it into `WaitState::V2Race` instead — the
            // record's membership (set at `V2RaceOpen`, unchanged since)
            // stays correct because `effective_control_stack` treats a
            // parked `V2Race`'s `record_id` as still "held". K-3: n/a (not
            // a barrier).
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
            // K-1/K-2 (V4.2): as `V2Guard` — opening fibre registered as
            // sole member in the same step the handle is pushed. K-3: n/a.
            Instr::V2GuardN { handler } => {
                // As V2Guard, `RecordKind::Guard { interrupting: false }`.
                // Q2 (EOP-VS-BPMN-ISA-002 §10, Adam-ratified): a
                // non-interrupting guard re-arms after its trigger — the
                // trigger path itself (not yet implemented for GuardN)
                // must re-`Insert` an `Armed` record rather than retiring
                // it, once built.
                let record_id = RecordId::new(context.derived_id(ordinal));
                ordinal = ordinal.saturating_add(1);
                let mut record = ConcurrencyRecord {
                    handler: Some(*handler),
                    // A18: as `V2Guard`, no rollback snapshot — GuardN was
                    // already never rollback-eligible in practice (ruling D
                    // excludes non-interrupting guards from automatic
                    // rollback-on-failure), and A18 makes that structural
                    // rather than merely policy: the field is `None` here
                    // by the same `ConcurrencyRecord::new` default `V2Guard`
                    // now uses, not populated-but-unconsulted.
                    // V&S §15 (v0.7) ruling F: see V2Guard's identical field
                    // for the rationale.
                    opened_at: Some(fiber.pc),
                    ..ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: false })
                };
                record.members.insert(fiber.fiber_id);
                changes
                    .concurrency_mutations
                    .push(ConcurrencyMutation::Insert(Box::new(record)));
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
            // K-1/K-2 (V4.2): as `V2GuardEnd` — this is the record's
            // normal (non-triggered) close path; retires the record in the
            // same step the fibre pops its handle. K-3: n/a.
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
            // A18: `GUARD-R>` — as `V2Guard`, opening fibre registered as
            // sole member in the same step the handle is pushed (K-1/K-2).
            // K-3: n/a. Unlike `V2Guard`/`V2GuardN`, this is the ONLY
            // opcode that captures the A3 rollback-set snapshot —
            // `domain_payload`/`flags`/`join_expected`/session stack — and
            // carries no `handler` (nothing to trigger via
            // `V2TriggerGuard`; its only unwind paths are `V2CancelScope`
            // and automatic rollback-on-definitive-failure).
            Instr::V2GuardR => {
                let record_id = RecordId::new(context.derived_id(ordinal));
                ordinal = ordinal.saturating_add(1);
                let mut record = ConcurrencyRecord {
                    handler: None,
                    rollback_domain_payload: Some(instance.domain_payload.to_string().into_boxed_str()),
                    rollback_domain_payload_hash: Some(instance.domain_payload_hash),
                    rollback_flags: Some(instance.flags.clone()),
                    rollback_join_expected: Some(instance.join_expected.clone()),
                    rollback_session_stack: Some(instance.session_stack.to_rollback_snapshot()),
                    // V&S §15 (v0.7) ruling F: see V2Guard's identical field
                    // for the rationale.
                    opened_at: Some(fiber.pc),
                    ..ConcurrencyRecord::new(record_id, RecordKind::Guard { interrupting: true })
                };
                record.members.insert(fiber.fiber_id);
                changes
                    .concurrency_mutations
                    .push(ConcurrencyMutation::Insert(Box::new(record)));
                fiber.control_stack.push(record_id);
                changes.control_stack_deltas.push(ControlStackDelta::Push {
                    fiber_id: fiber.fiber_id,
                    handle: record_id,
                });
                changes.events.push(RuntimeEvent::V2GuardROpened {
                    record_id,
                    fiber_id: fiber.fiber_id,
                });
                fiber.pc = fiber.pc.saturating_add(1);
            }
            // A18: `<GUARD-R` — as `V2GuardEnd`, the record's normal
            // (non-rollback) close path. K-1/K-2/K-3 as `V2GuardEnd`.
            Instr::V2GuardREnd => {
                let handle = fiber
                    .control_stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2GuardREnd"))?;
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
            // §18 v0.10 ruling I: `GUARD-TIMER>` — arms the guard this
            // fibre just opened (verifier-enforced adjacency: this
            // instruction's own address is always exactly the opening
            // guard's address + 1, so `fiber.control_stack.last()` is
            // unambiguously that same record). K-1/K-2/K-3: touches no
            // concurrency record and no control stack, exactly like
            // `V2ArmTimer` arming an open `RACE{` — the fibre's held-
            // handle set and every record's membership are unchanged by
            // arming; only a durable timer effect is scheduled, bound to
            // `TimerKind::V2GuardTimer` rather than `TimerKind::V2Race`.
            // Fire-time dispatch (both the `V2Guard`/`V2GuardN` case,
            // which behaves exactly as a manually-issued
            // `Command::V2TriggerGuard`, and the `V2GuardR` case, which
            // has no `handler` to trigger and instead runs the same
            // automatic-rollback path as a definitive job failure) lives
            // in `apply_timer`'s `TimerKind::V2GuardTimer` arm below, not
            // here — arming and firing are deliberately separate steps,
            // same separation `V2ArmTimer`/`Command::TimerFired` already
            // have for races.
            Instr::V2GuardArmTimer => {
                let value = fiber
                    .stack
                    .pop()
                    .ok_or(TransitionError::StackUnderflow("V2GuardArmTimer"))?;
                let Value::I64(duration) = value else {
                    return Err(TransitionError::InvalidCommand(
                        "V2GuardArmTimer: duration must be I64",
                    ));
                };
                let record_id = *fiber.control_stack.last().ok_or(
                    TransitionError::InvalidCommand("V2GuardArmTimer: no open guard"),
                )?;
                let due_at = context.logical_time().saturating_add(duration.max(0) as u64);
                let effect_id =
                    EffectId::for_instruction(instance.instance_id, fiber.fiber_id, fiber.pc.into());
                // Bounded to 1 fire by default (a plain boundary timer
                // fires once); GUARD-TIMER-CYCLE>, if present, overwrites
                // `remaining` outright rather than narrowing it.
                let record_kind = fetch_record_in_transition(snapshot, &changes, record_id)
                    .map(|record| record.kind);
                let repeat_spec = match record_kind {
                    Some(RecordKind::Guard { interrupting: false }) => {
                        Some(TimerRepeatSpec::new(duration.max(0) as u64, 1, 0))
                    }
                    _ => None,
                };
                changes.effects.push(DurableEffect::schedule_timer(
                    effect_id,
                    fiber.fiber_id,
                    due_at,
                    TimerKind::V2GuardTimer { record_id },
                    repeat_spec,
                ));
                fiber.pc = fiber.pc.saturating_add(1);
            }
            // §18 v0.10 ruling I, second arming-trigger kind (BoundaryError
            // v2 migration): arms the guard this fibre just opened, exactly
            // as `V2GuardArmTimer` resolves its own target — `record_id` is
            // unambiguously `fiber.control_stack.last()` because the
            // verifier enforces this instruction (and any run of sibling
            // `V2GuardArmError`/`V2GuardArmTimer` arms) sits immediately
            // after the guard-open it arms. Unlike `V2GuardArmTimer`, this
            // instruction DOES mutate the concurrency record — it pushes
            // `(error_code, handler)` onto the record's own `error_routes`
            // — because error-match arming is data (N routes stored on the
            // record), not a durable effect keyed by record id the way a
            // timer is. The target record was opened earlier THIS SAME tick
            // (verifier-enforced adjacency), so it exists only as an
            // in-flight `ConcurrencyMutation::Insert` in `changes`, not yet
            // materialized into `snapshot` — mutated in place there, same
            // "check in-flight mutations first" reasoning `V2GuardArmTimer`
            // already documents, except here it's the ONLY case (no
            // already-materialized-in-snapshot fallback): `error_routes` is
            // static per-guard configuration set once at open time, never
            // re-armed on a `V2GuardN` re-open the way a timer effect is.
            Instr::V2GuardArmError { error_code, handler } => {
                let record_id = *fiber.control_stack.last().ok_or(
                    TransitionError::InvalidCommand("V2GuardArmError: no open guard"),
                )?;
                let mut armed = false;
                for mutation in changes.concurrency_mutations.iter_mut().rev() {
                    if let ConcurrencyMutation::Insert(record) = mutation {
                        if record.id != record_id {
                            continue;
                        }
                        // Defensive, matching this codebase's existing style
                        // elsewhere in `apply()` — verifier adjacency
                        // guarantees this holds for any verified program;
                        // this is the fail-closed backstop for a
                        // hand-assembled/unverified one.
                        if record.state != RecordState::Armed
                            || !matches!(record.kind, RecordKind::Guard { .. })
                        {
                            return Err(TransitionError::InvalidCommand(
                                "V2GuardArmError: target record is not an armed guard",
                            ));
                        }
                        let route = (error_code.clone(), *handler);
                        if route.0.is_none() {
                            // Catch-all: always last (verifier enforces at
                            // most one).
                            record.error_routes.push(route);
                        } else {
                            // Specific code: insert before any existing
                            // trailing catch-all, preserving
                            // specific-first/catch-all-last order among
                            // multiple `V2GuardArmError` arms stacked on one
                            // guard-open.
                            let insert_pos = record
                                .error_routes
                                .iter()
                                .position(|(code, _)| code.is_none())
                                .unwrap_or(record.error_routes.len());
                            record.error_routes.insert(insert_pos, route);
                        }
                        armed = true;
                        break;
                    }
                }
                if !armed {
                    return Err(TransitionError::InvalidCommand(
                        "V2GuardArmError: no open guard record found to arm",
                    ));
                }
                fiber.pc = fiber.pc.saturating_add(1);
            }
            // Post-close remediation (V&S §13 amendment v0.5 ruling A,
            // restored — see `Instr::V2GuardTimerCycle`'s own doc comment
            // for the full rationale). Verifier-enforced to sit at exactly
            // `guard_arm_timer_address + 1`, so the effect it patches is
            // guaranteed to be the very last element `changes.effects`
            // holds right now — pushed by the `V2GuardArmTimer` this
            // instruction is the immediate successor of, earlier in this
            // SAME tick's instruction loop. No control-stack or
            // concurrency-record change; touches no VM operand stack.
            Instr::V2GuardTimerCycle { max_fires } => {
                match changes.effects.last_mut() {
                    Some(DurableEffect::ScheduleTimer {
                        kind: TimerKind::V2GuardTimer { .. },
                        repeat_spec: repeat_spec @ Some(_),
                        ..
                    }) => {
                        let current = repeat_spec.as_ref().expect("just matched Some");
                        *repeat_spec = Some(TimerRepeatSpec::new(
                            current.interval_ms(),
                            *max_fires,
                            current.fired_count(),
                        ));
                    }
                    _ => {
                        // Unreachable past a verified program (V-1's new
                        // GUARD-TIMER-CYCLE> arm rejects anything but
                        // immediately-after-V2GuardArmTimer-on-a-GuardN
                        // placement, and V2GuardArmTimer always attaches
                        // `Some(repeat_spec)` for a GuardN target) — kept
                        // as a typed, fail-closed error rather than a
                        // panic/silent no-op for a malformed or
                        // hand-assembled-and-unverified program.
                        return Err(TransitionError::InvalidCommand(
                            "V2GuardTimerCycle: no GUARD-TIMER> repeat spec to set",
                        ));
                    }
                }
                fiber.pc = fiber.pc.saturating_add(1);
            }
            // K-1/K-2/K-3 (V4.2): a bare park — no concurrency record or
            // control-stack change, so all three are vacuously preserved.
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
            // K-1/K-2/K-3 (V4.2): as `V2WaitFor`.
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
            // K-1/K-2/K-3 (V4.2): as `V2WaitFor`.
            Instr::V2WaitMsg { name } => {
                let corr_key = resolve_correlation_key_at(workflow, &instance, fiber.pc)?;
                // Signal-before-wait: consume a message already buffered for
                // THIS fibre's subscription. The match is on name AND the
                // resolved content key — never `.first()`. `buffered_messages`
                // is a per-instance list that may hold entries for sibling
                // fibres parked on other names/keys (a parallel split with
                // several concurrent message waits); taking the first would
                // route a sibling's message here and strand that sibling.
                let message_name_map =
                    workflow.envelope().metadata().message_name_map();
                if let Some(buffered) = snapshot.buffered_messages().iter().find(|buffered| {
                    message_name_matches(
                        message_name_map,
                        &buffered.message.message_name,
                        *name,
                    ) && buffered.message.correlation_key == corr_key
                }) {
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
                changes.events.push(RuntimeEvent::WaitMsgSubscribed {
                    fiber_id: fiber.fiber_id,
                    name: *name,
                    corr_key: corr_key.clone(),
                });
                // V4 remediation (2026-07-22, found during V5 scoping):
                // this word never advanced `fiber.pc` before parking,
                // unlike every other parking word (`V2WaitFor`/
                // `V2WaitUntil` above, v1 `WaitMsg`'s own parking branch).
                // `apply_message`'s plain (non-race) `WaitState::Msg` match
                // doesn't set a resume `pc` either — only the race-arm case
                // has an explicit `resume_at`. Together, a fibre parked
                // here resumed at the *same* `pc` and re-executed
                // `V2WaitMsg` rather than continuing past it. Zero test
                // coverage anywhere caught this — genuinely untested since
                // V4.1 landed the word. Fixed to match `V2WaitFor`'s and
                // v1 `WaitMsg`'s own pattern.
                fiber.pc = fiber.pc.saturating_add(1);
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
            // K-1/K-2 (V4.2): the popped handle and everything nested
            // under it retires/deletes via `v2_rollback_guard_scope`
            // (itself K-1/K-2-preserving — see its own doc comment); the
            // calling fibre `Continues`, so its own remaining handles are
            // untouched. K-3: any barrier nested under the cancelled scope
            // retires along with it (moot membership thereafter), not
            // left mid-count.
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
                let restored = v2_rollback_guard_scope(
                    snapshot,
                    handle,
                    RollbackCaller::Continues(fiber.fiber_id),
                    &mut changes,
                )?;
                // A3 rollback-set (A18): domain_payload, flags,
                // join_expected, and the session stack are all restored
                // together — `ProcessInstance::counters` is deliberately
                // untouched (see `ConcurrencyRecord`'s doc comment).
                instance.domain_payload = restored.domain_payload.to_string().into();
                instance.domain_payload_hash = restored.domain_payload_hash;
                instance.flags = restored.flags;
                instance.join_expected = restored.join_expected;
                instance.session_stack = restored.session_stack;
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
        attempt,
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

    // BoundaryError v2 migration: replaces the deleted `error_route_map`
    // side-table lookup with a search over the fibre's OWN guard scopes —
    // "structure consulted as state" was exactly the D1-class defect
    // `error_route_map` shared with the already-deleted v1 `race_plan`/
    // `boundary_map`; this is the fix. Modeled on the `innermost_guard`
    // search a few dozen lines below (read that one's doc comment for the
    // full "innermost guard, don't search past it" rationale — identical
    // reasoning applies here: a boundary error only catches failures of its
    // own attached task's guard scope, not an ancestor's). Walks
    // `fiber.control_stack` innermost-first, stops at the first `Armed`
    // `RecordKind::Guard{..}` record regardless of whether IT has a
    // matching route (an outer guard's error route must never catch an
    // inner guard's unmatched failure), and inside that one record's own
    // `error_routes` (populated by `V2GuardArmError`, already sorted
    // specific-code-first/catch-all-last) takes the first match.
    let matched_error_route: Option<(String, Addr, RecordId)> = if let ErrorClass::BusinessRejection {
        rejection_code,
    } = error_class
    {
        // Two-step, not a single `find_map`: the search must stop at the
        // innermost armed Guard-kind record unconditionally (a `None` from
        // "this guard has no matching route" must NOT fall through to an
        // outer guard's routes — that would let an outer boundary error
        // catch an inner scope's unmatched failure, which is exactly the
        // per-scope-only catching this migration's doc comment above
        // promises and v1's per-host-task `error_route_map` granularity
        // never violated). A single `find_map` over the whole walk conflates
        // "not a guard, keep looking" with "is the guard, but no match" —
        // both return `None` from the closure — which was the actual bug
        // (independent blind review, 2026-07-23, live-repro'd against a
        // nested-guard fixture: an outer catch-all wrongly caught an inner
        // guard's unrelated-code rejection instead of falling through to an
        // Incident).
        fiber
            .control_stack
            .iter()
            .rev()
            .find_map(|id| {
                let record = snapshot.concurrency_table().get(*id)?;
                if record.state != RecordState::Armed {
                    return None;
                }
                if !matches!(record.kind, RecordKind::Guard { .. }) {
                    return None;
                }
                Some((*id, record))
            })
            .and_then(|(id, record)| {
                record
                    .error_routes
                    .iter()
                    .find(|(code, _)| {
                        code.as_deref()
                            .map(|c| c == rejection_code.as_str())
                            .unwrap_or(true)
                    })
                    .map(|(_, handler)| (rejection_code.clone(), *handler, id))
            })
    } else {
        None
    };
    if let Some((matched_code, matched_handler, matched_record_id)) = matched_error_route {
        // `changes` is still `Changes::default()` here (every earlier
        // branch in this function returns before reaching this point), so
        // — matching `apply_timer`'s own `TimerKind::V2GuardTimer` fold
        // pattern, which takes the helper's returned `Changes` as its base
        // rather than merging into a separate pre-existing one — the fired
        // guard's `Changes` simply becomes this transition's `changes`.
        let mut changes = v2_trigger_guard_changes_with_target(
            snapshot,
            matched_record_id,
            context,
            matched_handler,
        )?;
        if !job_key.is_empty() {
            changes.jobs_ack.push(job_key.to_string());
        }
        // `boundary_id` no longer names a BPMN element (the side table that
        // carried `boundary_element_id` is deleted) — nothing downstream
        // consumes this field's exact content beyond logging (checked: only
        // the kernel's own producer and `verifier.rs`'s unrelated
        // `host_boundary_count`/`boundary_ids` naming touch this string;
        // no test asserts on it), so a placeholder derived from the firing
        // guard's own `RecordId` is used rather than threading a BPMN
        // element id through `error_routes`/`V2GuardArmError` for no
        // consumer.
        changes.events.push(RuntimeEvent::ErrorRouted {
            job_key: job_key.to_string(),
            error_code: matched_code,
            boundary_id: format!("guard:{matched_record_id}"),
            resume_at: matched_handler,
        });
        return Ok(changes.finish(snapshot.instance().clone()));
    }

    // Adam-ratified (V&S §13 amendment v0.5 ruling C, revised by §14
    // amendment v0.6 ruling D): a *definitive* job failure (reached here —
    // no retry token left, no matching armed error route) for a fibre
    // sitting inside an armed *interrupting* V2Guard scope bypasses the v1
    // error-route/incident path — but only for `ErrorClass::ContractViolation`
    // (a technical fault). §13's original text said "no distinction between
    // ContractViolation/BusinessRejection/exhausted-Transient"; §14 supersedes
    // that clause: an unmatched `BusinessRejection` is a gap in the workflow's
    // own route map, not a machine fault, and rolling back would destroy the
    // evidence that a business outcome occurred; an exhausted `Transient` is
    // the retry budget's own terminal state and belongs in quarantine with
    // its attempt history, not silently erased. Both always surface as an
    // `Incident`, exactly like today's v1 path, regardless of guard nesting.
    //
    // `rollback_eligible` is an EXHAUSTIVE match over `ErrorClass`, no
    // wildcard arm — this is §14's meta-rule ("no failure class reaches
    // rollback by falling through") enforced by the compiler: a future
    // fourth `ErrorClass` variant is a compile error here until it is
    // deliberately classified, not a silent inheritor of whatever a
    // wildcard would have done.
    let rollback_eligible = match error_class {
        ErrorClass::ContractViolation => true,
        ErrorClass::BusinessRejection { .. } => false,
        ErrorClass::Transient => false,
    };
    // A transient failure that's still retriable is handled above and
    // never reaches here; a timer/wait resolving normally is ordinary
    // forward progress, not reachable through this function at all
    // (`TimerFired` has its own handler).
    //
    // Uses the same shared `v2_rollback_guard_scope` as `V2CancelScope` —
    // "all roads lead to Rome" — but the triggering fibre is killed (not
    // continued in place): there's no "next instruction" for an
    // externally-surfaced job failure to fall through to, and per Adam:
    // "kill the fibre... can simply be re-run" — the instance is left
    // exactly as it was at scope-open, ready for an external actor to
    // retry the whole scope, not auto-respawned.
    //
    // Innermost-guard selection (fixed in independent V4.6 blind review,
    // 2026-07-21): stops at the first Guard-kind record the stack search
    // meets, innermost-first, regardless of its interrupting flag — a
    // non-interrupting innermost guard means this rule does not fire even
    // if an outer interrupting guard exists further out on the stack,
    // matching §13's explicit carve-out ("today's v1 incident/routing
    // path is unchanged for fibres whose innermost armed guard is
    // non-interrupting"). `find_map` skips non-guard handles
    // (V2Barrier/V2Race) — they aren't guards, so they don't count as
    // "the innermost guard".
    //
    // A18: additionally requires `rollback_domain_payload.is_some()` —
    // only a `V2GuardR`-opened record carries a rollback snapshot at all.
    // A plain interrupting `V2Guard` is now control-only (no automatic
    // rollback-on-failure data disposition — see `Instr::V2Guard`'s doc
    // comment); a definitive failure whose innermost armed guard is a
    // plain `V2Guard` no longer matches here and falls through to the
    // ordinary v1 incident path below, exactly as if no guard were
    // present. This is a deliberate behavior change from V4.1 (retargeting
    // fixtures that exercised guard-rollback onto `V2GuardR` is the A18
    // migration, not a regression — see the plan doc's A18 Part 2 entry).
    let innermost_guard = rollback_eligible
        .then(|| {
            fiber.control_stack.iter().rev().find_map(|id| {
                let record = snapshot.concurrency_table().get(*id)?;
                if record.state != RecordState::Armed {
                    return None;
                }
                match record.kind {
                    // The search STOPS at the first armed Guard-kind
                    // record regardless of rollback-capability — a plain
                    // `V2Guard`/`V2GuardN` sitting innermost must not be
                    // skipped past in search of a rollback-capable
                    // `V2GuardR` further out (that would roll back the
                    // wrong, outer scope). `rollback_domain_payload.is_some()`
                    // is carried alongside `interrupting` so the caller can
                    // tell "innermost guard found, but not rollback-capable"
                    // apart from "no guard at all" without re-querying.
                    RecordKind::Guard { interrupting } => {
                        Some((*id, interrupting, record.rollback_domain_payload.is_some()))
                    }
                    _ => None,
                }
            })
        })
        .flatten();
    if let Some((guard_handle, true, true)) = innermost_guard {
        let guard_record = snapshot
            .concurrency_table()
            .get(guard_handle)
            .cloned()
            .ok_or(TransitionError::InvalidCommand(
                "rollback: unknown scope handle",
            ))?;
        let restored = v2_rollback_guard_scope(
            snapshot,
            guard_handle,
            RollbackCaller::Dies(fiber.fiber_id),
            &mut changes,
        )?;
        let mut instance = snapshot.instance().clone();
        // A3 rollback-set (A18): see the `V2CancelScope` handler's
        // identical restoration for the full field-by-field rationale.
        instance.domain_payload = restored.domain_payload.to_string().into();
        instance.domain_payload_hash = restored.domain_payload_hash;
        instance.flags = restored.flags;
        instance.join_expected = restored.join_expected;
        instance.session_stack = restored.session_stack;
        if !job_key.is_empty() {
            changes.jobs_ack.push(job_key.to_string());
        }

        // §13 amendment (Adam's ruling, 2026-07-22): §13's kill-and-no-
        // incident disposition only holds when the guard scope is a
        // proper subset of the instance's live fibres — `v2_rollback_guard_scope`
        // has already pushed the triggering fibre (a genuine member of
        // the cancelled subtree, `RollbackCaller::Dies` does not exempt
        // it) onto `changes.fibers_delete`. Compute whether anything else
        // in the instance survives this transition; if not, killing the
        // trigger too would leave the instance with zero live fibres and
        // a non-terminal state — permanently stuck, and Ring 3's own
        // zero-live-fibre assert would reject the frame outright. Restore
        // the fibre instead: pop the retiring guard (and anything nested
        // above it on this fibre's own stack — also retiring, same
        // cascade) off its control stack, and park it on an incident at
        // the guard's own `opened_at` address, so resolving the incident
        // re-executes `GUARD>` and opens a fresh scope activation over the
        // now-restored payload — exactly §13's own stated intent ("the
        // instance is left as it was at scope-open so it can simply be
        // re-run"), now with a mechanism. The kernel still never initiates
        // the retry itself; it parks and waits for `Command::ResolveIncident`.
        let live_after_without_trigger: std::collections::BTreeSet<Uuid> = apply_fiber_deltas(
            snapshot.fibers(),
            &changes.fibers_upsert,
            &changes.fibers_delete,
            false,
        )
        .into_keys()
        .filter(|id| *id != fiber.fiber_id)
        .collect();
        if live_after_without_trigger.is_empty() {
            let opened_at = guard_record.opened_at.ok_or(TransitionError::InvalidCommand(
                "rollback: spanning guard scope carries no opened_at address",
            ))?;
            // The trigger fibre survives after all — undo its deletion.
            changes.fibers_delete.retain(|id| *id != fiber.fiber_id);
            // Pop the retiring guard (and anything nested above it on this
            // fibre's own chain — also part of the cancelled subtree) off
            // the surviving fibre's control stack, mirroring
            // `V2CancelScope`'s own explicit pop at its call site: re-
            // executing `GUARD>` on resume must open a fresh record, not
            // layer under the stale retired one still sitting on the stack
            // (K-2 would also reject a live fibre referencing a Retired
            // handle).
            if let Some(pos) = fiber.control_stack.iter().position(|id| *id == guard_handle) {
                let popped: Vec<_> = fiber.control_stack.split_off(pos);
                for handle in popped.into_iter().rev() {
                    changes.control_stack_deltas.push(ControlStackDelta::Pop {
                        fiber_id: fiber.fiber_id,
                        handle,
                    });
                }
            }
            // `v2_rollback_guard_scope`'s own ancestor-membership sweep
            // (run above, as part of the initial call) treated this fibre
            // as fully dead and removed it from every ancestor record on
            // its pre-rollback control stack, including any still-`Armed`
            // scope enclosing this guard. That's wrong here: the fibre is
            // not leaving those outer scopes, only the retiring guard (and
            // whatever nested under it) — re-add it to whatever remains on
            // its (now-truncated) control stack.
            let readd_ops: Vec<_> = fiber
                .control_stack
                .iter()
                .map(|ancestor| (*ancestor, MembershipOp::Add(fiber.fiber_id)))
                .collect();
            v2_reconcile_ancestor_membership(
                snapshot,
                &readd_ops,
                &std::collections::BTreeSet::new(),
                &mut changes,
            );

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
                process_instance_id: instance.instance_id,
                fiber_id: fiber.fiber_id,
                service_task_id: service_task_id.clone(),
                // Diagnostic pointer: where the underlying technical fault
                // actually occurred, NOT the resume address — those are
                // deliberately different fields now (see `fiber.pc` below).
                bytecode_addr: fiber.pc,
                error_class: error_class.clone(),
                message: message.to_string(),
                retry_count: *attempt,
                created_at: logical_timestamp(context)?,
                resolved_at: None,
                resolution: None,
            };
            // Resume address: the guard's own opening word, not the failed
            // task — `Command::ResolveIncident` does not itself touch
            // `fiber.pc`, so whatever is set here is exactly where
            // execution resumes, opening a fresh guard activation.
            fiber.pc = opened_at;
            fiber.wait = WaitState::Incident { incident_id };
            // Parked on an open Incident, resumable via
            // Command::ResolveIncident — Incidented, not Failed. Inherits
            // the ordinary incident path's classification below (A18: "A18's
            // spanning case sets the same state as the ordinary incident
            // path, whatever that is").
            instance.state = ProcessState::Incidented { incident_id };
            changes.incidents.push(incident);
            changes.fibers_upsert.push(fiber);
            changes.events.push(RuntimeEvent::IncidentCreated {
                incident_id,
                service_task_id,
                job_key: (!job_key.is_empty()).then(|| job_key.to_string()),
            });
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
        // V&S §15 (v0.7) ruling E: real attempt history, not a hardcoded
        // lie. `0` at call sites with no RetryPolicy bookkeeping to
        // report (an honest absence — see `Command::EffectFailed::attempt`'s
        // doc comment).
        retry_count: *attempt,
        created_at: logical_timestamp(context)?,
        resolved_at: None,
        resolution: None,
    };
    fiber.wait = WaitState::Incident { incident_id };
    let mut instance = snapshot.instance().clone();
    // Parked on an open Incident, resumable via Command::ResolveIncident —
    // Incidented, not Failed.
    instance.state = ProcessState::Incidented { incident_id };
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
                && corr_key.as_str() == correlation_key.as_str() =>
            {
                let mut resumed = fiber.clone();
                resumed.wait = WaitState::Running;
                matched = Some(resumed);
                break;
            }
            WaitState::V2Race { record_id, arms } => {
                for arm in arms {
                    let bpmn_lite_types::V2RaceArm::Msg {
                        target,
                        name: waiting_name,
                        corr_key,
                    } = arm
                    else {
                        continue;
                    };
                    if message_name_matches(metadata.message_name_map(), name, *waiting_name)
                        && corr_key.as_str() == correlation_key.as_str()
                    {
                        let mut resumed = fiber.clone();
                        resumed.pc = *target;
                        resumed.wait = WaitState::Running;
                        matched = Some(resumed);
                        v2_race_record = Some(*record_id);
                        break;
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
    let Some(fiber) = matched else {
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
        corr_key: correlation_key.to_string(),
        msg_ref: None,
    });
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

/// Resolve the content correlation key for a message word at `pc` from the
/// program's `v2_corr_sources` side table (§28). Fails closed if the
/// instruction has no source entry (a correctly compiled program always emits
/// one) or the source does not resolve to a scalar.
fn resolve_correlation_key_at(
    workflow: &ExecutableWorkflow,
    instance: &ProcessInstance,
    pc: Addr,
) -> Result<String, TransitionError> {
    let source = workflow
        .envelope()
        .metadata()
        .v2_corr_sources()
        .get(&pc)
        .ok_or(TransitionError::InvalidCommand(
            "message word has no correlation-key source",
        ))?;
    bpmn_lite_types::ffi_bindings::resolve_correlation_key(instance, source).map_err(|_| {
        TransitionError::InvalidCommand("correlation key did not resolve to a scalar")
    })
}

fn error_class_label(error_class: &ErrorClass) -> &'static str {
    match error_class {
        ErrorClass::Transient => "transient",
        ErrorClass::ContractViolation => "contract_violation",
        ErrorClass::BusinessRejection { .. } => "business_rejection",
    }
}

fn apply_job_completion(
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
        .find(|fiber| matches!(&fiber.wait, WaitState::Job { job_key } if job_key == &completion.job_key))
        .cloned()
        .ok_or(TransitionError::InvalidCommand("completion has no parked fiber"))?;
    let before = current_hash;
    let mut instance = snapshot.instance().clone();
    apply_completion(&mut instance, completion)?;
    let mut changes = Changes::default();
    // V5.3 (§18, landed 2026-07-23): the v1 `race_plan`-consulting branch
    // that used to live here (a job racing a boundary timer via
    // `WaitState::Race`) is deleted along with `race_plan`/`boundary_map`
    // — boundary timers on service tasks now race independently via
    // `V2Guard`/`V2GuardN` + `GUARD-TIMER>` wrapping the task (§18 ruling
    // I), which needs no cooperation from job completion at all: an
    // ordinary job completion always just advances PC+1.
    fiber.pc = fiber.pc.saturating_add(1);
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
/// One membership change against an *existing* (already-`Insert`ed in a
/// prior transition) ancestor record: `Add` when a new fibre inherits that
/// record's handle onto its own control stack (e.g. `V2Fork`'s children,
/// a triggered guard's handler fibre), `Remove` when a fibre carrying that
/// handle dies (e.g. `V2Fork`'s own forking fibre, a race's/barrier's
/// cancelled members) without the record itself being retired in the same
/// transition (a retired record's membership is moot — K-2/K-1 are scoped
/// to `Armed` records, see `check_k_invariants`'s doc comment).
#[derive(Debug)]
enum MembershipOp {
    Add(Uuid),
    Remove(Uuid),
}

/// Fetches a concurrency record via `changes` first, `snapshot` only as
/// fallback. `apply_tick` can execute several instructions against one
/// fibre before a transition commits, so `snapshot` — fixed at the
/// transition's start — may already be stale for a record an earlier
/// instruction this same tick touched; a `Retire`/`Remove` staged for
/// `handle` means the record is gone this transition even if `snapshot`
/// still shows it `Armed`. Every mid-transition concurrency-record read
/// in this file must go through this helper (or its bulk sibling,
/// `armed_record_ids_in_transition`), never `snapshot.concurrency_
/// table()` directly.
fn fetch_record_in_transition(
    snapshot: &Snapshot,
    changes: &Changes,
    handle: RecordId,
) -> Option<ConcurrencyRecord> {
    for mutation in changes.concurrency_mutations.iter().rev() {
        // Exhaustive, no wildcard: a new `ConcurrencyMutation` variant MUST
        // fail compilation here, forcing a decision about its pending-fold
        // semantics — a guarded-arm + `_ => {}` shape would silently ignore
        // it, which is exactly the partial-pending-awareness hazard this
        // helper exists to prevent.
        match mutation {
            ConcurrencyMutation::Insert(record) => {
                if record.id == handle {
                    return Some((**record).clone());
                }
            }
            ConcurrencyMutation::Retire(id) | ConcurrencyMutation::Remove(id) => {
                if *id == handle {
                    return None;
                }
            }
        }
    }
    snapshot.concurrency_table().get(handle).cloned()
}

/// The bulk sibling of `fetch_record_in_transition`: returns every record
/// `Armed` after folding `changes.concurrency_mutations` onto `snapshot`,
/// for callers that need the full set rather than one record by handle.
/// Same rationale as `fetch_record_in_transition` — a record opened
/// earlier this transition may exist only in `changes`.
/// #103e's sibling for the COMMAND path (found by the EOP-FUZZ F2 O5
/// oracle, 2026-07-25): `Command::Cancel`/`Command::Terminate` emit
/// `TerminalCleanup` that deletes every fibre but — before this helper —
/// left every armed concurrency record in place, so the Ring 3
/// post-transition frame check rejected the transition with a K-1
/// violation ("armed record has member, no live fibre") on ANY instance
/// holding an armed barrier/guard/race (i.e. any in-flight fork). That
/// made such instances un-cancellable and un-terminatable — a direct
/// violation of the poisoned-instance discipline documented on `apply`.
/// Bulk retirement over the snapshot table, same rationale as
/// `Instr::EndTerminate`'s sweep. Routed through the allowlisted
/// `armed_record_ids_in_transition` (read-safety lint) with an empty
/// `Changes`: command arms stage no prior mutations this tick, so folding
/// the default `Changes` is exact, not an approximation.
fn retire_all_armed_records(
    snapshot: &Snapshot,
    mut builder: TransitionBuilder,
) -> TransitionBuilder {
    for record_id in armed_record_ids_in_transition(snapshot, &Changes::default()) {
        builder = builder.concurrency_mutation(ConcurrencyMutation::Retire(record_id));
    }
    builder
}

fn armed_record_ids_in_transition(snapshot: &Snapshot, changes: &Changes) -> Vec<RecordId> {
    let mut table = snapshot.concurrency_table().clone();
    for mutation in &changes.concurrency_mutations {
        match mutation {
            ConcurrencyMutation::Insert(record) => table.insert((**record).clone()),
            ConcurrencyMutation::Retire(id) => {
                if let Some(record) = table.get_mut(*id) {
                    record.state = RecordState::Retired;
                }
            }
            ConcurrencyMutation::Remove(id) => {
                table.remove(*id);
            }
        }
    }
    table
        .iter()
        .filter(|(_, record)| record.state == RecordState::Armed)
        .map(|(id, _)| *id)
        .collect()
}

/// K-1/K-2 discharge helper (V&S §7): every word that creates a fibre
/// inheriting a prefix of another fibre's control stack, or deletes a
/// fibre that still carries ancestor handles beyond whatever record it is
/// directly being removed from, must keep those ancestor records'
/// `members` sets in agreement with "who has my handle on their stack" —
/// otherwise K-1 (member liveness) or K-2 (stack↔membership consistency)
/// breaks for the *enclosing* scope, not just the one the word directly
/// touches. Batches all ops per record before emitting `Insert` mutations
/// (multiple ops against the same ancestor in one call must fold into a
/// single mutation — emitting one stale `Insert` per op would make later
/// ones silently undo earlier ones, since `Insert` overwrites by key).
/// `exclude` skips records already being `Insert`ed/`Retire`d/`Remove`d
/// elsewhere in the same transition (their membership is moot or already
/// current).
fn v2_reconcile_ancestor_membership(
    snapshot: &Snapshot,
    ops: &[(RecordId, MembershipOp)],
    exclude: &std::collections::BTreeSet<RecordId>,
    changes: &mut Changes,
) {
    let mut touched: std::collections::BTreeMap<RecordId, ConcurrencyRecord> =
        std::collections::BTreeMap::new();
    for (handle, op) in ops {
        if exclude.contains(handle) {
            continue;
        }
        let Some(mut record) = touched
            .get(handle)
            .cloned()
            .or_else(|| fetch_record_in_transition(snapshot, changes, *handle))
        else {
            continue;
        };
        match op {
            MembershipOp::Add(fiber_id) => {
                record.members.insert(*fiber_id);
            }
            MembershipOp::Remove(fiber_id) => {
                record.members.remove(fiber_id);
            }
        }
        touched.insert(*handle, record);
    }
    for record in touched.into_values() {
        changes.concurrency_mutations.push(ConcurrencyMutation::Insert(Box::new(record)));
    }
}

/// K-1/K-2 (V4.2 discharge for `Command::V2TriggerGuard`, both branches):
/// spawns the handler with `handler_stack` (a prefix of some existing
/// live fibre's own effective stack — established invariant, hence a
/// valid prefix by construction) and immediately registers it as a member
/// of every record on that prefix via `v2_reconcile_ancestor_membership`
/// before anything else runs. Interrupting branch: `v2_cancel_guard_scope`
/// discovers the *entire* live subtree under `record_id` by walking
/// control-stack/wait-state membership directly (not `.members`), so
/// `retire_order`/`cancelled_fibers` are exhaustive; every cancelled fibre
/// is then swept from any *ancestor* record above `record_id` that isn't
/// itself retiring this same step. K-3: any barrier in the cancelled
/// subtree retires with it (moot thereafter). Non-interrupting branch: the
/// record re-arms unchanged (still `Armed`, same identity) — only the
/// handler's own ancestor registration applies.
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
    let changes = v2_trigger_guard_changes(snapshot, *record_id, context)?;
    Ok(changes.finish(snapshot.instance().clone()))
}

/// The actual K-1/K-2 discharge (see the doc comment above), factored out
/// of `apply_v2_trigger_guard` so `apply_timer`'s `TimerKind::V2GuardTimer`
/// arm (§18 v0.10 ruling I) can reuse it verbatim for a timer-armed
/// `V2Guard`/`V2GuardN` firing — "the same effect as issuing
/// `Command::V2TriggerGuard`," not a re-derived parallel mechanism.
/// Returns `Changes` rather than a finished `Transition` so the timer-fire
/// caller can fold in its own `RuntimeEvent::TimerFired`/timer-consume
/// mutation before finishing — `apply_v2_trigger_guard` itself finishes
/// against `snapshot.instance().clone()` unchanged, exactly as before this
/// refactor (this function never mutates `ProcessInstance` fields; only
/// `V2GuardR`'s rollback path does that, see `apply_v2_guard_timer_rollback`).
fn v2_trigger_guard_changes(
    snapshot: &Snapshot,
    record_id: RecordId,
    context: &DeterministicContext,
) -> Result<Changes, TransitionError> {
    let record = snapshot
        .concurrency_table()
        .get(record_id)
        .cloned()
        .ok_or(TransitionError::InvalidCommand(
            "V2TriggerGuard: unknown guard handle",
        ))?;
    if !matches!(record.kind, RecordKind::Guard { .. }) {
        return Err(TransitionError::InvalidCommand(
            "V2TriggerGuard: handle is not a guard",
        ));
    }
    if record.state != RecordState::Armed {
        return Err(TransitionError::InvalidCommand(
            "V2TriggerGuard: guard is not armed",
        ));
    }
    let handler_target = record.handler.ok_or(TransitionError::InvalidCommand(
        "V2TriggerGuard: guard has no handler address",
    ))?;
    v2_trigger_guard_changes_with_target(snapshot, record_id, context, handler_target)
}

/// The actual K-1/K-2 discharge, factored further out of
/// `v2_trigger_guard_changes` so `apply_job_failure`'s `GUARD-ERROR>` match
/// path (BoundaryError v2 migration) can fire a guard's handler at an
/// explicit target — one of the record's own `error_routes` entries —
/// instead of unconditionally reading `record.handler`. This is the
/// deliberate, ratified, contained break of the "arm instructions never
/// carry targets" convention documented on `Instr::V2GuardArmError`: every
/// OTHER caller of this mechanism (`Command::V2TriggerGuard` via
/// `v2_trigger_guard_changes`, `apply_timer`'s `TimerKind::V2GuardTimer`
/// arm) still resolves its target from `record.handler`/the timer's own
/// bookkeeping and reaches this function only through that thin wrapper —
/// `record.handler` itself is untouched by this refactor.
fn v2_trigger_guard_changes_with_target(
    snapshot: &Snapshot,
    record_id: RecordId,
    context: &DeterministicContext,
    handler_target: Addr,
) -> Result<Changes, TransitionError> {
    let record = snapshot
        .concurrency_table()
        .get(record_id)
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
                .position(|id| *id == record_id)
                .map(|pos| {
                    let end = if interrupting { pos } else { pos + 1 };
                    fiber.control_stack[..end].to_vec()
                })
        })
        .unwrap_or_default();

    let mut changes = Changes::default();
    let handler_fiber_id = context.derived_id(0);
    let mut handler_fiber = Fiber::new(handler_fiber_id, handler_target);
    handler_fiber.control_stack = handler_stack.clone();
    changes.fibers_upsert.push(handler_fiber);
    changes.events.push(RuntimeEvent::FiberSpawned {
        fiber_id: handler_fiber_id,
        pc: handler_target,
        parent: None,
    });
    // K-1/K-2 (V4.2): the handler fibre inherits every handle in
    // `handler_stack` (ancestors above this guard for the interrupting
    // case; those plus the guard's own re-armed token for `V2GuardN`) —
    // each such still-`Armed` record must count it as a member.
    let handler_ops: Vec<_> = handler_stack
        .iter()
        .map(|ancestor| (*ancestor, MembershipOp::Add(handler_fiber_id)))
        .collect();
    v2_reconcile_ancestor_membership(
        snapshot,
        &handler_ops,
        &std::collections::BTreeSet::new(),
        &mut changes,
    );

    if interrupting {
        let mut retire_order = Vec::new();
        let mut cancelled_fibers = Vec::new();
        v2_cancel_guard_scope(snapshot, record_id, &mut retire_order, &mut cancelled_fibers);
        for id in &retire_order {
            changes
                .concurrency_mutations
                .push(ConcurrencyMutation::Retire(*id));
        }
        for fiber_id in &cancelled_fibers {
            changes.fibers_delete.push(*fiber_id);
        }
        // K-1 (V4.2): cancelled fibres may carry ancestor handles above
        // `record_id` (this guard nested inside another scope) — those
        // records aren't in `retire_order` (only `record_id` and its own
        // descendants are), so a still-`Armed` outer ancestor must drop
        // the now-dead fibre explicitly.
        let retiring: std::collections::BTreeSet<RecordId> = retire_order.iter().copied().collect();
        let mut ancestor_ops = Vec::new();
        for fiber_id in &cancelled_fibers {
            if let Some(dead_fiber) = snapshot.fibers().get(fiber_id) {
                for ancestor in &dead_fiber.control_stack {
                    ancestor_ops.push((*ancestor, MembershipOp::Remove(*fiber_id)));
                }
            }
        }
        v2_reconcile_ancestor_membership(snapshot, &ancestor_ops, &retiring, &mut changes);
        changes.events.push(RuntimeEvent::V2GuardTriggered {
            record_id,
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
            record_id,
            handler_fiber_id,
        });
    }
    Ok(changes)
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
///
/// K-1/K-2 (V4.2): identical discharge argument to the interrupting branch
/// of `apply_v2_trigger_guard` — `v2_cancel_guard_scope`'s walk is
/// exhaustive over the live subtree, and every cancelled fibre is swept
/// from ancestor records above `guard_handle` not themselves retiring this
/// step. K-3: as above, subtree barriers retire with the scope.
/// A18 A3 rollback-set: everything `v2_rollback_guard_scope` restores onto
/// the caller's own `ProcessInstance`. Deliberately excludes
/// `ProcessInstance::counters` (loop/retry bounds) — see
/// `ConcurrencyRecord`'s doc comment for why restoring them would be
/// unsound, not merely out of scope.
struct RollbackSnapshot {
    domain_payload: Box<str>,
    domain_payload_hash: [u8; 32],
    flags: std::collections::BTreeMap<bpmn_lite_types::FlagKey, Value>,
    join_expected: std::collections::BTreeMap<JoinId, u16>,
    session_stack: bpmn_lite_types::session_stack::SessionStackState,
}

fn v2_rollback_guard_scope(
    snapshot: &Snapshot,
    guard_handle: RecordId,
    caller: RollbackCaller,
    changes: &mut Changes,
) -> Result<RollbackSnapshot, TransitionError> {
    // `V2CancelScope` runs inside `apply_tick`'s per-instruction loop, so
    // `guard_handle`'s record may exist only in `changes`, not yet
    // `snapshot` — fetch_record_in_transition, not a raw table read.
    let record = fetch_record_in_transition(snapshot, changes, guard_handle).ok_or(
        TransitionError::InvalidCommand("rollback: unknown scope handle"),
    )?;
    // A18/V-10: only a `V2GuardR`-opened record carries a rollback
    // snapshot at all — a plain `V2Guard`/`V2GuardN` handle reaching here
    // (via `V2CancelScope` or automatic rollback-on-failure) is rejected,
    // not silently treated as a no-op restore. This is the runtime half of
    // the "V2CancelScope must target GUARD-R>" rule; V-10's static
    // companion check in `v2_verifier.rs` rejects the same mistargeting
    // at verify time, before this branch could ever be reached from a
    // verified program.
    let (rollback_payload, rollback_hash, rollback_flags, rollback_join_expected, rollback_session_stack) =
        match (
            record.rollback_domain_payload,
            record.rollback_domain_payload_hash,
            record.rollback_flags,
            record.rollback_join_expected,
            record.rollback_session_stack,
        ) {
            (Some(payload), Some(hash), Some(flags), Some(join_expected), Some(session_stack)) => {
                (payload, hash, flags, join_expected, session_stack)
            }
            _ => {
                return Err(TransitionError::InvalidCommand(
                    "rollback: handle carries no rollback snapshot (not a GUARD-R> scope)",
                ));
            }
        };
    let session_stack = bpmn_lite_types::session_stack::SessionStackState::from_rollback_snapshot(
        &rollback_session_stack,
    )
    .map_err(|_| {
        TransitionError::InvalidCommand("rollback: malformed session-stack snapshot")
    })?;
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
    // K-1 (V4.2): as in the interrupting-trigger path — cancelled fibres
    // may carry ancestor handles above `guard_handle` that aren't in
    // `retire_order`.
    let retiring: std::collections::BTreeSet<RecordId> = retire_order.iter().copied().collect();
    let mut ancestor_ops = Vec::new();
    for dead_id in &cancelled_fibers {
        if let Some(dead_fiber) = snapshot.fibers().get(dead_id) {
            for ancestor in &dead_fiber.control_stack {
                ancestor_ops.push((*ancestor, MembershipOp::Remove(*dead_id)));
            }
        }
    }
    v2_reconcile_ancestor_membership(snapshot, &ancestor_ops, &retiring, changes);
    changes.events.push(RuntimeEvent::V2ScopeCancelled {
        record_id: guard_handle,
        fiber_id,
        cancelled_records: retire_order,
        cancelled_fibers,
    });
    Ok(RollbackSnapshot {
        domain_payload: rollback_payload,
        domain_payload_hash: rollback_hash,
        flags: rollback_flags,
        join_expected: rollback_join_expected,
        session_stack,
    })
}

/// §18 v0.10 ruling I: a `GUARD-TIMER>`-armed `V2GuardR` firing. `GUARD-R>`
/// carries no `handler` (by construction — see `Instr::V2GuardR`'s doc
/// comment: "its only unwind paths are `V2CancelScope` and automatic
/// rollback-on-definitive-failure"), so a timer-fired trigger against it
/// cannot go through `v2_trigger_guard_changes` at all — there is nothing
/// to spawn. Ruling I's own text is unqualified about which kind of
/// guarded work a timer trigger applies to ("indifferent to whether the
/// work inside is synchronous, effect-based, or a nested FORK region"),
/// and by the same reasoning it does not exempt the guard's *own kind*
/// either: a `GUARD-R>` wrapped in a boundary timer is exactly "this
/// scope, and everything nested inside it, times out — restore the A3
/// rollback-set" with no handler step, i.e. the automatic-rollback branch
/// `apply_job_failure` already takes for a definitive `ContractViolation`
/// inside an interrupting rollback-capable guard, minus the "which job
/// failed" starting point (§13 ruling C originally, §14 ruling D
/// narrowing it to `GUARD-R>` specifically). "All roads lead to Rome"
/// (§13 amendment v0.5) gains a third caller of the same
/// `v2_rollback_guard_scope` primitive here — `V2CancelScope` (in-line,
/// continues), `apply_job_failure` (external job failure, dies), and now
/// this (external deadline, dies).
///
/// The one genuine adaptation: `apply_job_failure` always has a specific
/// failing fibre (the one parked on the job that failed) to hand
/// `RollbackCaller::Dies`. A guard's own deadline has no equivalent — the
/// scope times out as a whole, not because any one member failed — so the
/// representative fibre for `RollbackCaller::Dies`'s audit id and the
/// §13-amendment spanning-case survivor-selection anchor (below) is the
/// lowest-UUID live member of the record (deterministic and replay-
/// stable; `v2_cancel_guard_scope`'s walk is exhaustive over the whole
/// live subtree regardless of which live member is named — see its own
/// doc comment — so this choice is bookkeeping, not a semantic one).
///
/// Scope finding (V-10/A18 interaction, reported per the task's own ask
/// rather than silently assumed benign): V-10 already forbids `GUARD-R>`
/// from being opened while nested inside a `FORK`'s child fibre, but it
/// does NOT forbid a `FORK`/`JOIN` region from existing *inside* a
/// `GUARD-R>`'s own extent (dominance, not exclusion — see `Instr::V2GuardR`'s
/// doc comment). So `guard_record.members` can legitimately contain
/// several live fibres spanning multiple fork branches when this fires,
/// not just one — `v2_cancel_guard_scope`'s walk already handles that
/// (it discovers the entire live subtree, not just direct members), and
/// the spanning-case check below (computed over the FULL post-rollback
/// fibre set, not just the chosen representative) is unaffected by how
/// many members there were. No V-10 amendment needed; recorded because
/// the interaction was worth checking, not because it changed anything.
///
/// Returns the `Changes` accumulated so far and the `ProcessInstance` with
/// the A3 rollback-set already restored (and, in the spanning case, its
/// `state` set to `Incidented`) — mirroring `apply_job_failure`'s own
/// local `instance` variable, just handed back instead of finished
/// in-place, so the caller (`apply_timer`) can still attach its own
/// `TimerFired` event and `Consume` timer mutation before finishing.
fn apply_v2_guard_timer_rollback(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    guard_handle: RecordId,
    guard_record: &ConcurrencyRecord,
    context: &DeterministicContext,
) -> Result<(Changes, ProcessInstance), TransitionError> {
    let Some(trigger_fiber_id) = guard_record.members.iter().min().copied() else {
        // No live member left inside the scope at all — the record is
        // still Armed (checked by the caller) but has already been
        // vacated some other way (e.g. every member already left via a
        // path that doesn't retire the record itself). Nothing to roll
        // back or kill; leave the record exactly as-is, the caller still
        // consumes the timer.
        return Ok((Changes::default(), snapshot.instance().clone()));
    };
    let mut changes = Changes::default();
    let restored = v2_rollback_guard_scope(
        snapshot,
        guard_handle,
        RollbackCaller::Dies(trigger_fiber_id),
        &mut changes,
    )?;
    let mut instance = snapshot.instance().clone();
    // A3 rollback-set (A18): see `V2CancelScope`'s handler for the full
    // field-by-field rationale — not repeated here.
    instance.domain_payload = restored.domain_payload.to_string().into();
    instance.domain_payload_hash = restored.domain_payload_hash;
    instance.flags = restored.flags;
    instance.join_expected = restored.join_expected;
    instance.session_stack = restored.session_stack;

    // §13 amendment (spanning case): identical computation to
    // `apply_job_failure`'s own branch of the same shape — see its
    // comment for the full rationale, not repeated here. If killing the
    // representative trigger fibre (and everything else
    // `v2_rollback_guard_scope` just cancelled) would leave the instance
    // with zero live fibres, restore the trigger fibre instead and park
    // it on an incident at the guard's own `opened_at` address.
    let live_after_without_trigger: std::collections::BTreeSet<Uuid> = apply_fiber_deltas(
        snapshot.fibers(),
        &changes.fibers_upsert,
        &changes.fibers_delete,
        false,
    )
    .into_keys()
    .filter(|id| *id != trigger_fiber_id)
    .collect();
    if live_after_without_trigger.is_empty() {
        let Some(mut trigger_fiber) = snapshot.fibers().get(&trigger_fiber_id).cloned() else {
            return Err(TransitionError::InvalidCommand(
                "guard timer rollback: trigger fibre vanished mid-transition",
            ));
        };
        let opened_at = guard_record.opened_at.ok_or(TransitionError::InvalidCommand(
            "guard timer rollback: spanning guard scope carries no opened_at address",
        ))?;
        // The trigger fibre survives after all — undo its deletion.
        changes.fibers_delete.retain(|id| *id != trigger_fiber_id);
        // Pop the retiring guard (and anything nested above it on this
        // fibre's own chain) off the surviving fibre's control stack —
        // mirrors `apply_job_failure`'s identical truncation, see its
        // comment for why.
        if let Some(pos) = trigger_fiber
            .control_stack
            .iter()
            .position(|id| *id == guard_handle)
        {
            let popped: Vec<_> = trigger_fiber.control_stack.split_off(pos);
            for handle in popped.into_iter().rev() {
                changes.control_stack_deltas.push(ControlStackDelta::Pop {
                    fiber_id: trigger_fiber.fiber_id,
                    handle,
                });
            }
        }
        // `v2_rollback_guard_scope`'s ancestor-membership sweep (run
        // above) treated this fibre as fully dead and removed it from
        // every ancestor record on its pre-rollback control stack — wrong
        // here, since it is not leaving those outer scopes, only the
        // retiring guard. Re-add it to whatever remains on its
        // now-truncated control stack.
        let readd_ops: Vec<_> = trigger_fiber
            .control_stack
            .iter()
            .map(|ancestor| (*ancestor, MembershipOp::Add(trigger_fiber.fiber_id)))
            .collect();
        v2_reconcile_ancestor_membership(
            snapshot,
            &readd_ops,
            &std::collections::BTreeSet::new(),
            &mut changes,
        );

        let incident_id = context.derived_id(0);
        let service_task_id = workflow
            .envelope()
            .metadata()
            .debug_map()
            .get(&trigger_fiber.pc)
            .cloned()
            .unwrap_or_else(|| format!("pc_{}", trigger_fiber.pc));
        let incident = Incident {
            incident_id,
            process_instance_id: instance.instance_id,
            fiber_id: trigger_fiber.fiber_id,
            service_task_id: service_task_id.clone(),
            bytecode_addr: trigger_fiber.pc,
            error_class: ErrorClass::ContractViolation,
            message: "guard-arming timer deadline elapsed".to_string(),
            retry_count: 0,
            created_at: logical_timestamp(context)?,
            resolved_at: None,
            resolution: None,
        };
        // Resume address: the guard's own opening word, not wherever the
        // deadline caught the scope mid-execution — matches
        // `apply_job_failure`'s identical resume-address choice, for the
        // identical reason (`Command::ResolveIncident` re-executes
        // `GUARD-R>`, opening a fresh scope activation over the restored
        // payload).
        trigger_fiber.pc = opened_at;
        trigger_fiber.wait = WaitState::Incident { incident_id };
        instance.state = ProcessState::Incidented { incident_id };
        changes.incidents.push(incident);
        changes.fibers_upsert.push(trigger_fiber);
        changes.events.push(RuntimeEvent::IncidentCreated {
            incident_id,
            service_task_id,
            job_key: None,
        });
    }
    Ok((changes, instance))
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
    // Parked on an open Incident, resumable via Command::ResolveIncident —
    // Incidented, not Failed.
    instance.state = ProcessState::Incidented { incident_id };
    changes.incidents.push(incident);
    changes.events.push(RuntimeEvent::IncidentCreated {
        incident_id,
        service_task_id: format!("pc_{}", fiber.pc),
        job_key: None,
    });
    changes.fibers_upsert.push(fiber);
    Ok(changes.finish(instance))
}

fn apply_completion(
    instance: &mut ProcessInstance,
    completion: &bpmn_lite_types::JobCompletion,
) -> Result<(), TransitionError> {
    instance.domain_payload = completion.domain_payload.clone().into();
    instance.domain_payload_hash = blake3_hash(completion.domain_payload.as_bytes());
    for (name, value) in &completion.orch_flags {
        let key = name
            .strip_prefix("flag_")
            .and_then(|raw| raw.parse::<u32>().ok())
            .ok_or(TransitionError::InvalidCommand(
                "orch_flags key must be flag_<u32>",
            ))?;
        instance.flags.insert(key, value.clone());
    }
    Ok(())
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
        // §18 ruling K Part 2: non-empty-is-truthy, the same convention
        // every other collection-bearing language construct in common use
        // applies (and consistent with this codebase's own `*value != 0`
        // "non-default is truthy" pattern for the scalar variants above).
        Value::Array(items) => !items.is_empty(),
    }
}

fn describe_wait(wait: &WaitState) -> String {
    match wait {
        WaitState::Running => String::new(),
        WaitState::Timer { deadline_ms } => format!("Timer({deadline_ms})"),
        WaitState::Msg { name, corr_key, .. } => format!("Msg({name}, {corr_key:?})"),
        WaitState::Job { job_key } => format!("Job({job_key})"),
        WaitState::Effect { effect_id } => format!("Effect({})", effect_id.as_uuid()),
        WaitState::Join { join_id, .. } => format!("Join({join_id})"),
        WaitState::V2Barrier { record_id } => format!("V2Barrier({record_id})"),
        WaitState::V2Race { record_id, arms } => {
            format!("V2Race({record_id}, {} arms)", arms.len())
        }
        WaitState::Incident { incident_id } => format!("Incident({incident_id})"),
    }
}

fn apply_timer(
    workflow: &ExecutableWorkflow,
    snapshot: &Snapshot,
    timer: &bpmn_lite_types::ClaimedTimer,
    fired_at: u64,
    context: &DeterministicContext,
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
        // §18 v0.10 ruling I: a `GUARD-TIMER>`-armed guard's deadline.
        // Deliberately matched on `TimerKind` alone (the wildcard `_` in
        // the tuple's second position, unlike every arm above) — arming
        // does not park the fibre in a matching `WaitState` the way a race
        // does (the guarded body keeps executing normally after arming),
        // so there is no fibre-side wait-state to gate staleness against.
        // Staleness is instead gated on the record itself: `RecordState`
        // (already `Retired` if the guard closed normally via
        // `<GUARD`/`<GUARD-N`/`<GUARD-R`, or was already triggered, before
        // this deadline arrived) is the sole check — exactly the same
        // shape as `TimerKind::V2Race`'s `WaitState::V2Race` match guard,
        // just keyed on the concurrency table instead of a fibre.
        //
        // Builds its own complete `Changes`-based transition and returns
        // early, bypassing `builder`/`rearmed` entirely — the shared
        // `builder`/`rearmed` machinery below is v1's own general-purpose
        // rearm path (dead since V5.3's v1 deletion; nothing sets
        // `rearmed` any more). This arm's own rearm decision (below,
        // post-close remediation restoring V&S §13 amendment v0.5 ruling A
        // to `GUARD-TIMER>` specifically — a `GUARD-N>`-kind record's
        // timer DOES rearm, in the same transition, via
        // `TimerMutation::Rearm`; `GUARD>`/`GUARD-R>` still never do,
        // their record having just retired) is self-contained (reusing
        // `v2_trigger_guard_changes`/`apply_v2_guard_timer_rollback`, both
        // already `Changes`-shaped), so merging into the shared `builder`
        // above would just be needless field-by-field duplication for no
        // arm that could ever run alongside it (each `Command::TimerFired`
        // addresses exactly one `ClaimedTimer`, hence exactly one
        // `TimerKind`).
        (TimerKind::V2GuardTimer { record_id }, _) => {
            let Some(record) = snapshot.concurrency_table().get(*record_id).cloned() else {
                // Record no longer exists at all (fully retired and swept
                // already) — stale fire, no-op besides consuming below.
                return Ok(TransitionBuilder::new(snapshot.instance().clone())
                    .timer_mutation(TimerMutation::Consume {
                        timer_id: timer.timer_id(),
                        claim_token: timer.claim_token(),
                    })
                    .build());
            };
            if record.state != RecordState::Armed || !matches!(record.kind, RecordKind::Guard { .. })
            {
                // Stale: already triggered/closed normally (Retired), or
                // — should be unreachable past a verified program, since
                // V2GuardArmTimer only ever binds a Guard-kind record —
                // not actually a guard. Either way, not this deadline's
                // job to act; no-op besides consuming.
                return Ok(TransitionBuilder::new(snapshot.instance().clone())
                    .timer_mutation(TimerMutation::Consume {
                        timer_id: timer.timer_id(),
                        claim_token: timer.claim_token(),
                    })
                    .build());
            }
            let interrupting = matches!(record.kind, RecordKind::Guard { interrupting: true });
            let (mut changes, instance) = if record.rollback_domain_payload.is_some() {
                // `V2GuardR`-opened: no `handler` to trigger (by
                // construction — see `Instr::V2GuardR`'s doc comment), so
                // firing runs the same automatic-rollback path a
                // definitive job failure inside an interrupting
                // rollback-capable guard already takes
                // (`apply_job_failure`) — "all roads lead to Rome" for a
                // third caller of `v2_rollback_guard_scope`.
                apply_v2_guard_timer_rollback(workflow, snapshot, *record_id, &record, context)?
            } else {
                // `V2Guard`/`V2GuardN`: identical effect to a manually
                // issued `Command::V2TriggerGuard` — same helper, same
                // interrupting-unwind-and-spawn or non-interrupting
                // re-arm-and-spawn behaviour, per ruling I's own text
                // ("the arming trigger is what issues it").
                (
                    v2_trigger_guard_changes(snapshot, *record_id, context)?,
                    snapshot.instance().clone(),
                )
            };
            changes.events.push(RuntimeEvent::TimerFired {
                timer_id: timer.timer_id(),
                fiber_id: timer.fiber_id(),
                fired_at,
            });
            // Post-close remediation (V&S §13 amendment v0.5, ruling A,
            // restored to GUARD-TIMER>'s own timer-fire path — see
            // `Instr::V2GuardTimerCycle`'s doc comment for the full
            // rationale). `GUARD>`/`GUARD-R>` (interrupting) always
            // `Consume`, unchanged from ruling I's original fire-once
            // behaviour — their record retired above (via
            // `v2_trigger_guard_changes`'s interrupting branch or the
            // rollback path), so there is nothing left to re-arm against.
            // `GUARD-N>` (non-interrupting) re-arms in the SAME transition
            // per ruling A's default, reusing this timer's own `timer_id`
            // (`TimerMutation::Rearm`, pre-existing generic timer-schedule
            // infrastructure this is the first `V2*` word to populate) —
            // bounded by `timer.repeat_spec()` (set at arm time by
            // `V2GuardArmTimer`/narrowed by `V2GuardTimerCycle`) when
            // present, decrementing `remaining` each fire; exhaustion
            // (`remaining` reaching its last permitted fire) falls through
            // to `Consume` instead — no further rearm, exactly
            // `t_ni_3_cycle_exhausted_reverts_to_job`'s "reverts to job"
            // shape: the guard scope continues with no further timer
            // protection from that point on, behaving as if opened without
            // a trigger at all.
            let rearm = (!interrupting)
                .then(|| timer.repeat_spec())
                .flatten()
                .filter(|spec| spec.remaining() > 1)
                .map(|spec| TimerMutation::Rearm {
                    timer_id: timer.timer_id(),
                    claim_token: timer.claim_token(),
                    due_at: fired_at.saturating_add(spec.interval_ms()),
                    repeat_spec: TimerRepeatSpec::new(
                        spec.interval_ms(),
                        spec.remaining() - 1,
                        spec.fired_count().saturating_add(1),
                    ),
                });
            changes.timer_mutations.push(rearm.unwrap_or(TimerMutation::Consume {
                timer_id: timer.timer_id(),
                claim_token: timer.claim_token(),
            }));
            return Ok(changes.finish(instance));
        }
        _ => {}
    }
    // V5.3 (§18, landed 2026-07-23): `rearmed` used to be set by v1's
    // `TimerKind::Race` non-interrupting-cycle rearm branch, deleted this
    // step along with `race_plan`/`boundary_map`. `TimerKind::V2Race`
    // still never rearms (fire-once, per its own doc comment) and
    // `TimerKind::V2GuardTimer` always returns early above (its own rearm
    // decision — post-close remediation, `GUARD-N>` DOES rearm now, see
    // that arm's doc comment — is self-contained and never falls through
    // to here), so every path that actually reaches this point always
    // consumes the fired timer — unconditionally now, not gated on a flag
    // nothing sets any more.
    builder = builder.timer_mutation(TimerMutation::Consume {
        timer_id: timer.timer_id(),
        claim_token: timer.claim_token(),
    });
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_types::{ArtifactEnvelope, session_stack::SessionStackState};
    use std::collections::BTreeMap;


    fn minimal_instance() -> ProcessInstance {
        ProcessInstance {
            instance_id: Uuid::from_u128(1),
            tenant_id: "tenant-a".to_string(),
            process_key: "p".to_string(),
            bytecode_version: [0u8; 32],
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
        }
    }

    #[test]
    fn apply_completion_accepts_a_well_formed_flag_key() {
        let mut instance = minimal_instance();
        let mut orch_flags = BTreeMap::new();
        orch_flags.insert("flag_5".to_string(), Value::Bool(true));
        let completion = bpmn_lite_types::JobCompletion {
            job_key: "job-1".to_string(),
            domain_payload: "{}".to_string(),
            expected_instance_payload_hash: [0u8; 32],
            orch_flags,
        };
        apply_completion(&mut instance, &completion).unwrap();
        assert_eq!(instance.flags.get(&5), Some(&Value::Bool(true)));
    }

    #[test]
    fn apply_completion_rejects_a_malformed_flag_key() {
        let mut instance = minimal_instance();
        let mut orch_flags = BTreeMap::new();
        orch_flags.insert("not_a_flag_key".to_string(), Value::Bool(true));
        let completion = bpmn_lite_types::JobCompletion {
            job_key: "job-1".to_string(),
            domain_payload: "{}".to_string(),
            expected_instance_payload_hash: [0u8; 32],
            orch_flags,
        };
        assert!(apply_completion(&mut instance, &completion).is_err());
    }

    /// A `v2_corr_sources` table mapping each message-word address to a
    /// `Bool(false)` literal, whose content correlation key is `"false"` —
    /// the deterministic default these hand-assembled fixtures correlate on.
    fn corr_false(addrs: &[u32]) -> BTreeMap<Addr, bpmn_lite_types::ffi_bindings::BindingSource> {
        use bpmn_lite_types::ffi_bindings::{BindingSource, Literal};
        addrs
            .iter()
            .map(|&addr| (Addr::new(addr), BindingSource::Literal(Literal::Bool(false))))
            .collect()
    }

    fn fixture() -> (ExecutableWorkflow, Snapshot, DeterministicContext) {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [7u8; 32],
            // V5.3 (§18, landed 2026-07-23): migrated from v1 `Instr::Fork`
            // to `Instr::V2Fork` — v1 `Fork`/`Join` are deleted entirely
            // this step. This fixture's own base program is filler
            // bytecode most of the 37 tests sharing `fixture()` never
            // actually execute (they construct their own fibers/wait
            // states and drive `apply` directly); the handful that DO run
            // it via `Command::Tick` at pc 0 (e.g.
            // `same_inputs_produce_byte_identical_transition`,
            // `verified_fiber_limit_is_enforced_before_interpretation`)
            // exercise generic determinism/limits properties equally well
            // under `V2Fork`'s barrier-based fork as they did under v1
            // `Fork`'s arrival-counted one — nothing about those
            // properties is v1/v2-specific. Unlike v1 `Fork` (fire-and-
            // forget, no control-stack obligation), `V2Fork` pushes a
            // barrier handle onto each spawned child's control stack that
            // V-1 requires a matching `V2Join` to pop on every path — a
            // bare `[V2Fork, End, End]` is verifier-rejected (control
            // stack non-empty at program end), so this uses the same
            // minimal balanced shape `v2_fork_join_end_completes_
            // instance_not_stuck_running`'s own fixture below establishes.
            program: vec![
                /* 0 */ Instr::V2Fork { targets: vec![Addr::new(1), Addr::new(3)].into(), pairing: Addr::new(0) },
                /* 1 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 2 */ Instr::Jump { target: Addr::new(5) },
                /* 3 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 4 */ Instr::Jump { target: Addr::new(5) },
                /* 5 */ Instr::End,
            ],
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
            first.parity_bytes().unwrap(),
            second.parity_bytes().unwrap()
        );
    }

    #[test]
    fn apply_without_commit_replays_identically() {
        let (workflow, snapshot, context) = fixture();
        let command = Command::Tick { fiber_id: None };
        let before_crash = apply(&workflow, &snapshot, &command, &context).unwrap();
        let after_restart = apply(&workflow, &snapshot, &command, &context).unwrap();
        assert_eq!(
            before_crash.parity_bytes().unwrap(),
            after_restart.parity_bytes().unwrap()
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

    /// §18 ruling K Part 2 finding: `max_stack`/`max_registers` bound slot
    /// *count*, which stopped being sufficient the moment `Value::Array`
    /// existed (a single slot/flag can now be arbitrarily large/deep).
    /// This proves `validate_snapshot_limits`'s new walk actually rejects
    /// an over-large `Value::Array` living in `instance.flags` — by TOTAL
    /// ENCODED SIZE (element count here; see the sibling test below for
    /// depth), not merely "some limit exists somewhere." A flag holding
    /// `MAX_VALUE_ARRAY_LEN + 1` elements is a hard, typed reject, the
    /// same `ResourceLimitExceeded` shape `verified_fiber_limit_is_enforced_before_interpretation`
    /// above already proves for fiber count.
    #[test]
    fn oversized_value_array_in_a_flag_is_rejected_before_interpretation() {
        let (workflow, snapshot, context) = fixture();
        let mut instance = snapshot.instance().clone();
        let oversized_array = Value::Array(
            (0..=bpmn_lite_types::types::MAX_VALUE_ARRAY_LEN as i64)
                .map(Value::I64)
                .collect(),
        );
        instance.flags.insert(0, oversized_array);
        let oversized = Snapshot::new(instance, snapshot.fibers().values().cloned());
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
                resource: "Value::Array size/depth (flag)",
                ..
            }
        ));
    }

    /// Same finding, the depth axis: a `Value::Array` nested deeper than
    /// `MAX_VALUE_ARRAY_DEPTH` in a flag is rejected the same way, even
    /// though its element COUNT at every level is tiny (1) — proving the
    /// bound is genuinely on depth, not merely re-deriving the length
    /// bound in a different shape.
    #[test]
    fn overly_deep_value_array_in_a_flag_is_rejected_before_interpretation() {
        let (workflow, snapshot, context) = fixture();
        let mut instance = snapshot.instance().clone();
        let mut deep = Value::I64(0);
        for _ in 0..=bpmn_lite_types::types::MAX_VALUE_ARRAY_DEPTH {
            deep = Value::Array(vec![deep]);
        }
        instance.flags.insert(0, deep);
        let oversized = Snapshot::new(instance, snapshot.fibers().values().cloned());
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
                resource: "Value::Array size/depth (flag)",
                ..
            }
        ));
    }

    /// §18 ruling K Part 2 defense-in-depth: an instance already poisoned
    /// by an oversized `Value::Array` flag (simulating "poisoned before
    /// the gRPC-boundary fix landed, or via any other path") must still
    /// be reachable by `Command::Cancel` — this is the exact zombie
    /// scenario the blind review found: `apply` used to reject EVERY
    /// command, `Cancel` included, once `validate_snapshot_limits` saw
    /// the poisoned flag, leaving no way out (not `Incidented`, so
    /// `ResolveIncident` did not apply either). `Cancel` succeeds here,
    /// proving the array-limit exemption (`check_arrays == false` for
    /// `Cancel`/`Terminate`) actually closes that gap.
    #[test]
    fn cancel_succeeds_against_an_already_poisoned_instance() {
        let (workflow, snapshot, context) = fixture();
        let mut instance = snapshot.instance().clone();
        let oversized_array = Value::Array(
            (0..=bpmn_lite_types::types::MAX_VALUE_ARRAY_LEN as i64)
                .map(Value::I64)
                .collect(),
        );
        instance.flags.insert(0, oversized_array);
        let poisoned = Snapshot::new(instance, snapshot.fibers().values().cloned());

        // Sanity: a non-exempt command still rejects against this snapshot
        // (proves the poison is real, not a fixture mistake).
        let tick_error = apply(
            &workflow,
            &poisoned,
            &Command::Tick { fiber_id: None },
            &context,
        )
        .unwrap_err();
        assert!(matches!(
            tick_error,
            TransitionError::ResourceLimitExceeded {
                resource: "Value::Array size/depth (flag)",
                ..
            }
        ));

        let transition = apply(
            &workflow,
            &poisoned,
            &Command::Cancel {
                reason: "operator cancel".to_string(),
            },
            &context,
        )
        .expect("Cancel must succeed against an already-poisoned instance");
        assert!(matches!(
            transition.next_snapshot().state,
            ProcessState::Cancelled { .. }
        ));
    }

    /// Same finding, `Command::Terminate` — the other command a stuck
    /// instance needs to be reachable by.
    #[test]
    fn terminate_succeeds_against_an_already_poisoned_instance() {
        let (workflow, snapshot, context) = fixture();
        let mut instance = snapshot.instance().clone();
        let mut deep = Value::I64(0);
        for _ in 0..=bpmn_lite_types::types::MAX_VALUE_ARRAY_DEPTH {
            deep = Value::Array(vec![deep]);
        }
        instance.flags.insert(0, deep);
        let poisoned = Snapshot::new(instance, snapshot.fibers().values().cloned());

        let transition = apply(&workflow, &poisoned, &Command::Terminate, &context)
            .expect("Terminate must succeed against an already-poisoned instance");
        assert!(matches!(
            transition.next_snapshot().state,
            ProcessState::Terminated { .. }
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
            write_set: BTreeMap::new(),
            task_manifest: vec![],
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

        // §18 ruling J: zero-match now raises an Incident (via the same
        // `fail_contract` helper `Instr::ForkInclusive`'s own zero-match
        // arm already used, applied here for consistency) rather than
        // aborting the whole transition with `TransitionError::RouteNotMatched`
        // — the workflow COULDN'T decide which route to take, which is
        // exactly what an Incident means; it is NOT the workflow deciding
        // to stop (that's `END-TERMINATE`'s meaning, not this one).
        let mut instance = base_snapshot.instance().clone();
        instance.domain_payload = r#"{"kind":"unknown"}"#.into();
        instance.bind_placeholder_from_payload("@kind").unwrap();
        let snapshot = Snapshot::new(instance, base_snapshot.fibers().values().cloned());
        let t2 = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: None },
            &context,
        )
        .unwrap();
        let incident_id = match t2.next_snapshot().state {
            ProcessState::Incidented { incident_id } => incident_id,
            ref other => panic!("expected Incidented (not Failed, not a new termination state), got {other:?}"),
        };
        assert_eq!(t2.incidents().len(), 1, "exactly one incident raised");
        assert_eq!(t2.incidents()[0].incident_id, incident_id);
        assert_eq!(
            t2.fibers_upsert().len(),
            1,
            "the fibre survives, parked on the incident — not deleted"
        );
        let surviving = &t2.fibers_upsert()[0];
        assert_eq!(
            surviving.wait,
            WaitState::Incident { incident_id },
            "parked on exactly the incident just raised"
        );
        assert_eq!(
            surviving.pc,
            Addr::new(0),
            "resumes by re-evaluating the SAME RoutePayload — pc was never advanced past it"
        );

        // Drive it further, don't just assert the parked shape: the
        // operator amends the underlying data (ruling J's own text —
        // "an operator amends the payload ... calls ResolveIncident, and
        // the gateway re-evaluates"), resolves the incident, and the
        // instance must actually resume and re-evaluate the route rather
        // than staying stuck.
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
        let after_t2 =
            materialize_snapshot(genesis.state(), &t2, workflow.envelope().abi_version(), 1);
        let mut resolved_instance = after_t2.state().instance().clone();
        resolved_instance.domain_payload = r#"{"kind":"fund"}"#.into();
        resolved_instance.bind_placeholder_from_payload("@kind").unwrap();
        let resolved_snapshot = Snapshot::new(
            resolved_instance,
            after_t2.state().fibers().values().cloned(),
        )
        .with_incidents(after_t2.state().incidents().values().cloned());
        let context3 = DeterministicContext::new(301, Uuid::from_u128(302), 3);
        let t3 = apply(
            &workflow,
            &resolved_snapshot,
            &Command::ResolveIncident {
                incident_id,
                resolution: "operator corrected @kind".to_string(),
            },
            &context3,
        )
        .unwrap();
        assert_eq!(
            t3.next_snapshot().state,
            ProcessState::Running,
            "ResolveIncident restores Running"
        );

        let after_t3 =
            materialize_snapshot(after_t2.state(), &t3, workflow.envelope().abi_version(), 2);
        let re_run_snapshot = Snapshot::new(
            after_t3.state().instance().clone(),
            after_t3.state().fibers().values().cloned(),
        );
        let context4 = DeterministicContext::new(302, Uuid::from_u128(303), 4);
        let t4 = apply(
            &workflow,
            &re_run_snapshot,
            &Command::Tick { fiber_id: None },
            &context4,
        )
        .unwrap();
        assert!(
            matches!(t4.next_snapshot().state, ProcessState::Completed { .. }),
            "resolving the incident and correcting the payload must let the gateway \
             actually re-evaluate and complete, not stay stuck: {:?}",
            t4.next_snapshot().state
        );
    }

    // V5.3 (§18, landed 2026-07-23): `fork_payload_zero_match_raises_
    // incident_not_hard_error` (proving `Instr::ForkPayload`'s zero-match
    // arm raised an Incident, the §18 ruling J shape) is deleted along
    // with `Instr::ForkPayload` itself. Live-emission check, done before
    // deleting: grepped both frontends — `bpmn-lite-compiler/src/dsl/
    // frontend.rs` (the only would-be constructor of a DSL inclusive
    // split) has emitted zero `ForkPayload`/`JoinDynamic` since the §18
    // ruling I/item (e) landing (2026-07-23) switched DSL inclusive
    // splits over to the `V2Fork`/`V2LoadPlaceholderMatch`/
    // `V2RouteZeroMatch` shape unconditionally; `bpmn-lite-compiler/src/
    // dsl/frontend.rs`'s own negative test (`dsl_inclusive_split_..._
    // omits_zero_match_precheck`-family) already asserted this. The
    // ratified plan doc's "deferred to v3 FORK-DYN" disposition for
    // `ForkPayload` (5.2a, 2026-07-22) predates that landing and is
    // superseded by it, not contradicted — item (e) delivered the v2
    // inclusive-split lowering the deferral was waiting on, using the
    // combination-enumeration `V2Fork` shape 5.2a's own text anticipated
    // ("a combination-enumeration V2Fork/V2Join lowering would preserve
    // V-3"). `ForkPayload`'s zero-match Incident behavior this test
    // proved is superseded by `V2RouteZeroMatch`'s own coverage
    // (`bpmn-lite-engine::tests::t_ig_v2_zero_match_no_default_raises_
    // incident`, both frontends share one design per item (e)'s own
    // writeup) — not a silent loss of coverage, the same property proven
    // against the mechanism that actually runs today.

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
            write_set: BTreeMap::new(),
            task_manifest: vec![],
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
        assert_eq!(
            t1.concurrency_mutations().len(),
            3,
            "V2Guard's own Insert, V2Fork's barrier Insert, and V4.2's K-2 ancestor-membership \
             fix-up (guard re-Insert transferring membership from the dying forker to its children)"
        );
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
        assert!(matches!(
            &t1.concurrency_mutations()[2],
            ConcurrencyMutation::Insert(record) if record.id == guard_handle
                && record.members.contains(&child_a.fiber_id)
                && record.members.contains(&child_b.fiber_id)
                && !record.members.contains(&root_fiber_id)
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
            3,
            "K-1 ancestor fix-up (child_a still lists guard_handle on its own control stack, \
             independent of BAR — V4.2) then Retire(BAR) then Retire(G). The fix-up is moot \
             here since G retires later in this same transition anyway (straight-through to \
             V2GuardEnd), but the word can't know that in general, so it always corrects."
        );
        assert!(matches!(
            &t3.concurrency_mutations()[0],
            ConcurrencyMutation::Insert(record) if record.id == guard_handle
                && !record.members.contains(&child_a.fiber_id)
                && record.members.contains(&child_b.fiber_id)
        ));
        assert!(matches!(
            &t3.concurrency_mutations()[1],
            ConcurrencyMutation::Retire(id) if *id == barrier_handle
        ));
        assert!(matches!(
            &t3.concurrency_mutations()[2],
            ConcurrencyMutation::Retire(id) if *id == guard_handle
        ));
        assert_eq!(
            t3.control_stack_deltas().len(),
            2,
            "child_b's own Pop(BAR) then Pop(G) — child_a's stack was already fully popped in tick 2"
        );
    }

    /// V4 remediation (found during V5 scoping, 2026-07-22): plain
    /// (non-race) `V2WaitMsg` never advanced `fiber.pc` before parking, so
    /// a resumed fibre re-executed `V2WaitMsg` instead of continuing past
    /// it — genuinely untested since V4.1 landed the word (grepped: zero
    /// prior construction of `Instr::V2WaitMsg` anywhere in this file).
    /// Red before the fix: `resumed.pc` was `Addr::new(0)` (still pointing
    /// at the `V2WaitMsg` instruction itself); green after: `Addr::new(1)`.
    #[test]
    fn v2_wait_msg_resumes_past_itself_not_at_itself() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [11u8; 32],
            program: vec![
                /* 0 */ Instr::V2WaitMsg { name: 100 },
                /* 1 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program.with_v2_corr_sources(corr_false(&[0])), "v4-waitmsg-remediation").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
        let parked = t1.fibers_upsert()[0].clone();
        assert!(matches!(&parked.wait, WaitState::Msg { name: 100, .. }));

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

        // The V2WaitMsg's v2_corr_sources entry is a Bool(false) literal -> key "false".
        let message_command = Command::MessageDelivered {
            message_id: "m1".to_string(),
            name: "100".to_string(),
            correlation_key: "false".to_string(),
            payload: b"{}".to_vec(),
            payload_hash: None,
            expires_at: 0,
        };
        let t2 = apply(&workflow, &snapshot2, &message_command, &context).unwrap();
        assert_eq!(t2.fibers_upsert().len(), 1);
        let resumed = &t2.fibers_upsert()[0];
        assert_eq!(
            resumed.pc,
            Addr::new(1),
            "must continue past V2WaitMsg (addr 0), not re-park at it"
        );
        assert_eq!(resumed.wait, WaitState::Running);
    }

    /// V7 blind-review Finding 1: the signal-before-wait branch of
    /// `V2WaitMsg` must consume a buffered message only when its name AND
    /// resolved content key match this fibre — never `.first()`. A buffered
    /// message for a sibling subscription (parallel split, distinct keys) must
    /// NOT be consumed by an unrelated fibre. Red before the fix: the fibre
    /// consumed the mismatched message and advanced to `Addr::new(1)`.
    #[test]
    fn v2_wait_msg_ignores_a_buffered_message_with_a_mismatched_key() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [12u8; 32],
            program: vec![
                /* 0 */ Instr::V2WaitMsg { name: 100 },
                /* 1 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        }
        // This wait resolves its correlation key to the content "false".
        .with_v2_corr_sources(corr_false(&[0]));
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v7-buffered-key-mismatch").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        // A buffered message with the right NAME ("100") but a mismatched
        // correlation key ("other") — belongs to some other subscription.
        let buffered = bpmn_lite_types::ClaimedBufferedMessage {
            message: bpmn_lite_types::BufferedMessage {
                tenant_id: base_snapshot.instance().tenant_id.clone(),
                message_name: "100".to_string(),
                correlation_key: "other".to_string(),
                msg_id: "m-other".to_string(),
                payload: b"{}".to_vec(),
                payload_hash: None,
                process_instance_id: Some(base_snapshot.instance().instance_id),
                received_at: 0,
                expires_at: i64::MAX,
            },
            claim_token: "tok".to_string(),
            claim_until: i64::MAX,
        };
        let snapshot = Snapshot::new(
            base_snapshot.instance().clone(),
            [Fiber::new(root_fiber_id, 0)],
        )
        .with_buffered_messages(vec![buffered]);

        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
        let fiber = t1.fibers_upsert()[0].clone();
        assert!(
            matches!(&fiber.wait, WaitState::Msg { .. }),
            "must park — the buffered message's key does not match this wait's key"
        );
        assert!(
            t1.buffered_messages().is_empty(),
            "the mismatched message must NOT be consumed"
        );
    }

    /// V4 word-coverage audit (2026-07-22, following the `V2WaitMsg`
    /// remediation above): `V2WaitUntil` had zero test coverage anywhere
    /// in the kernel — the only other hole the audit found. Unlike
    /// `V2WaitMsg`, direct code inspection showed it already advances
    /// `fiber.pc` correctly (mirroring `V2WaitFor`); this test confirms
    /// that by evidence rather than trusting the read.
    #[test]
    fn v2_wait_until_resumes_past_itself_on_timer_fire() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [12u8; 32],
            program: vec![
                /* 0 */ Instr::PushI64(5_000),
                /* 1 */ Instr::V2WaitUntil,
                /* 2 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-waituntil-audit").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
        let parked = t1.fibers_upsert()[0].clone();
        assert!(matches!(&parked.wait, WaitState::Timer { deadline_ms: 5_000 }));

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

        let claimed_timer = bpmn_lite_types::ClaimedTimer::new(
            bpmn_lite_types::ClaimedTimerIdentity::new(
                bpmn_lite_types::TenantId::new("tenant-a").unwrap(),
                EffectId::for_instruction(root_fiber_id, root_fiber_id, 1),
                after_t1.state().instance().instance_id,
                root_fiber_id,
            ),
            5_000,
            TimerKind::Wait,
            None,
            Uuid::nil(),
        );
        let t2 = apply(
            &workflow,
            &snapshot2,
            &Command::TimerFired { timer: claimed_timer, fired_at: 5_000 },
            &context,
        )
        .unwrap();
        assert_eq!(t2.fibers_upsert().len(), 1);
        let resumed = &t2.fibers_upsert()[0];
        assert_eq!(
            resumed.pc,
            Addr::new(2),
            "must continue past V2WaitUntil (addr 1), not re-park at it"
        );
        assert_eq!(resumed.wait, WaitState::Running);
    }

    /// T11 perf claim — a program of `waits` sequential `V2WaitUntil`s, each
    /// preceded by `filler_pairs` stack-neutral `PushI64;Pop` pairs. Returns
    /// the number of durable commits (one per `apply`, since each apply runs
    /// a maximal deterministic burst and stops only at a park).
    fn count_commits_for_wait_program(waits: usize, filler_pairs: usize) -> usize {
        // Strictly-increasing deadlines so each successive V2WaitUntil parks:
        // a WaitUntil whose deadline is already reached passes through.
        let mut instrs = Vec::new();
        let mut deadline = 5_000i64;
        for _ in 0..waits {
            for _ in 0..filler_pairs {
                instrs.push(Instr::PushI64(0));
                instrs.push(Instr::Pop);
            }
            instrs.push(Instr::PushI64(deadline));
            instrs.push(Instr::V2WaitUntil);
            deadline += 5_000;
        }
        for _ in 0..filler_pairs {
            instrs.push(Instr::PushI64(0));
            instrs.push(Instr::Pop);
        }
        instrs.push(Instr::End);

        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [12u8; 32],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v2-perf-commits").unwrap(),
        )
        .unwrap();
        let (_, base, context) = fixture();
        let instance = base.instance().clone();
        let tenant = bpmn_lite_types::TenantId::new(&instance.tenant_id).unwrap();
        let instance_id = instance.instance_id;
        let root = base.fibers().values().next().unwrap().fiber_id;
        let abi = workflow.envelope().abi_version();
        let bytecode_version = instance.bytecode_version;

        let mut env = SnapshotEnvelope::new(
            abi,
            bytecode_version,
            0,
            PersistedSnapshotState::new(
                instance,
                [Fiber::new(root, 0)],
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
        );
        let mut revision = 1u64;
        let mut commits = 0usize;

        // One apply == one durable commit. Each iteration either runs a
        // Running fibre's burst to its next park/terminal (Tick), or wakes a
        // timer-parked fibre (TimerFired); the instance is done when no fibre
        // is Running or timer-parked.
        loop {
            let timer_park = env
                .state()
                .fibers()
                .values()
                .find(|fiber| matches!(fiber.wait, WaitState::Timer { .. }))
                .cloned();
            let command = if let Some(parked) = timer_park {
                let deadline_ms = match parked.wait {
                    WaitState::Timer { deadline_ms } => deadline_ms,
                    _ => unreachable!("filtered to Timer waits above"),
                };
                // The fibre parks at WaitUntil_addr+1 but the scheduled
                // timer's effect id keys on the WaitUntil's own address.
                let wait_addr = u32::from(parked.pc).saturating_sub(1);
                let timer = bpmn_lite_types::ClaimedTimer::new(
                    bpmn_lite_types::ClaimedTimerIdentity::new(
                        tenant.clone(),
                        EffectId::for_instruction(instance_id, parked.fiber_id, wait_addr),
                        instance_id,
                        parked.fiber_id,
                    ),
                    deadline_ms,
                    TimerKind::Wait,
                    None,
                    Uuid::nil(),
                );
                Command::TimerFired { timer, fired_at: deadline_ms }
            } else if env
                .state()
                .fibers()
                .values()
                .any(|fiber| matches!(fiber.wait, WaitState::Running))
            {
                Command::Tick { fiber_id: None }
            } else {
                break;
            };
            let snapshot = env.state().to_runtime_snapshot();
            let transition = apply(&workflow, &snapshot, &command, &context).unwrap();
            commits += 1;
            env = materialize_snapshot(env.state(), &transition, abi, revision);
            revision += 1;
            assert!(commits <= 10_000, "commit drive failed to terminate");
        }
        commits
    }

    /// V&S §1: "Commit frequency is proportional to *waits*, not to
    /// instruction count." Proven directly: driving each wait to completion
    /// takes a fixed wake+run commit pair, so commits(W, F) == 2·W + 1 with
    /// no instruction-count term — inflating the non-wait instruction count
    /// between waits by 20× (filler 5 → 100) does not change the total.
    #[test]
    fn v2_commits_scale_with_waits_not_instruction_count() {
        for &waits in &[0usize, 1, 2, 4, 8] {
            let baseline = count_commits_for_wait_program(waits, 0);
            assert_eq!(
                baseline,
                2 * waits + 1,
                "commit count must be linear in waits (waits={waits})"
            );
            for &filler in &[5usize, 25, 100] {
                let commits = count_commits_for_wait_program(waits, filler);
                assert_eq!(
                    commits, baseline,
                    "commit count must be independent of instruction count \
                     (waits={waits}, filler_pairs={filler})"
                );
            }
        }
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
                /* 3 */ Instr::V2ArmMsg { target: Addr::new(6), name: 100 },
                /* 4 */ Instr::V2RaceClose,
                /* 5 */ Instr::Jump { target: Addr::new(7) },
                /* 6 */ Instr::Jump { target: Addr::new(7) },
                /* 7 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program.with_v2_corr_sources(corr_false(&[3])), "v4-race-msg-wins").unwrap(),
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
            correlation_key: "false".to_string(),
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
            write_set: BTreeMap::new(),
            task_manifest: vec![],
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
            write_set: BTreeMap::new(),
            task_manifest: vec![],
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
                /* 9  */ Instr::V2ArmMsg { target: Addr::new(12), name: 100 },
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
            write_set: BTreeMap::new(),
            task_manifest: vec!["NotifyCancelled".to_string()],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program.with_v2_corr_sources(corr_false(&[9])), "v4-trigger-guard-cascade").unwrap(),
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

    /// V4.5 — `EOP-EX-BPMN-ISA-002.md` §2, Scenario 1 (happy path, message
    /// wins the race), reproduced end-to-end for real: F0 forks into F1
    /// (60s wait) and F2 (race between a 30s timer and an approval
    /// message); the message wins, F2 parks at its own `JOIN` (non-last
    /// arrival); F1's timer later fires, F1 reaches `JOIN` last — sole
    /// survivor, continues straight through `V2GuardEnd` to `End` in the
    /// same transition, F2 is cancelled at that moment.
    ///
    /// **Documented deviation from the oracle's literal bullet list, not
    /// silently reconciled**: §2 step 5 states
    /// `concurrency_mutations: [Retire(BAR)]` — but the same paragraph's
    /// prose says F1 "continues to 14 (`V2GuardEnd`, pops `G`) → 15
    /// (`End`)" in the *same* transition, which necessarily also retires
    /// `G`. Read literally, the bullet list just under-enumerates what its
    /// own prose describes (drafted by hand, before V4 existed to catch
    /// it) — omitting `Retire(G)` would leave `G` `Armed` forever with no
    /// live fibre able to reach it again, a K-1 violation the property
    /// test in `k_invariant_properties` would catch on its own terms. This
    /// test asserts the K-1-compliant reading (`Retire(BAR)` then
    /// `Retire(G)`, plus V4.2's K-1 ancestor-membership fix-up for the
    /// cancelled F2, which the oracle — drafted before that defect was
    /// found — could not have anticipated either), not the literal bullet.
    #[test]
    fn v2_ex_oracle_scenario_1_message_wins_reproduces_last_arrival_survivor_transition() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [24u8; 32],
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
                /* 9  */ Instr::V2ArmMsg { target: Addr::new(12), name: 100 },
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
            write_set: BTreeMap::new(),
            task_manifest: vec!["NotifyCancelled".to_string()],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program.with_v2_corr_sources(corr_false(&[9])), "v4.5-ex-oracle-scenario-1").unwrap(),
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

        // Tick 1: F0 -> V2Guard + V2Fork -> F1 (branch A), F2 (branch B).
        let context1 = DeterministicContext::new(200, Uuid::from_u128(201), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let (f1, f2) = (t1.fibers_upsert()[0].clone(), t1.fibers_upsert()[1].clone());
        let guard_handle = f1.control_stack[0];
        let barrier_handle = f1.control_stack[1];
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);

        // F1 runs 2 -> 3, parks on WaitState::Timer (60s).
        let context1b = DeterministicContext::new(200, Uuid::from_u128(2015), 1);
        let t1b = apply(
            &workflow,
            &Snapshot::new(after_t1.state().instance().clone(), after_t1.state().fibers().values().cloned())
                .with_concurrency_table(after_t1.state().concurrency_table().clone()),
            &Command::Tick { fiber_id: Some(f1.fiber_id) },
            &context1b,
        )
        .unwrap();
        let after_t1b = materialize_snapshot(after_t1.state(), &t1b, workflow.envelope().abi_version(), 1);

        // F2 runs 6 -> 10, parks on WaitState::V2Race (30s timer + msg arms).
        let context2 = DeterministicContext::new(201, Uuid::from_u128(202), 2);
        let t2 = apply(
            &workflow,
            &Snapshot::new(after_t1b.state().instance().clone(), after_t1b.state().fibers().values().cloned())
                .with_concurrency_table(after_t1b.state().concurrency_table().clone()),
            &Command::Tick { fiber_id: Some(f2.fiber_id) },
            &context2,
        )
        .unwrap();
        let after_t2 = materialize_snapshot(after_t1b.state(), &t2, workflow.envelope().abi_version(), 2);

        // ApprovalReceived arrives, correlation matches register 0's
        // default (Value::Bool(false) -> "b:false") — resolves the race,
        // F2 resumes at 12 -> 13 (V2Join), non-last arrival, parks.
        let context3 = DeterministicContext::new(202, Uuid::from_u128(203), 3);
        let t3 = apply(
            &workflow,
            &Snapshot::new(after_t2.state().instance().clone(), after_t2.state().fibers().values().cloned())
                .with_concurrency_table(after_t2.state().concurrency_table().clone()),
            &Command::MessageDelivered {
                message_id: "m1".to_string(),
                name: "100".to_string(),
                correlation_key: "false".to_string(),
                payload: b"{}".to_vec(),
                payload_hash: None,
                expires_at: 0,
            },
            &context3,
        )
        .unwrap();
        assert_eq!(t3.fibers_upsert()[0].pc, Addr::new(12), "message arm's own target");
        assert_eq!(t3.fibers_upsert()[0].wait, WaitState::Running);
        let after_t3 = materialize_snapshot(after_t2.state(), &t3, workflow.envelope().abi_version(), 3);

        // F2 resumes: 12 (Jump) -> 13 (V2Join), non-last arrival, parks.
        let context3b = DeterministicContext::new(202, Uuid::from_u128(2025), 3);
        let t3b = apply(
            &workflow,
            &Snapshot::new(after_t3.state().instance().clone(), after_t3.state().fibers().values().cloned())
                .with_concurrency_table(after_t3.state().concurrency_table().clone()),
            &Command::Tick { fiber_id: Some(f2.fiber_id) },
            &context3b,
        )
        .unwrap();
        assert!(matches!(
            t3b.fibers_upsert()[0].wait,
            WaitState::V2Barrier { record_id } if record_id == barrier_handle
        ));
        let after_t3 = materialize_snapshot(after_t3.state(), &t3b, workflow.envelope().abi_version(), 3);
        assert_eq!(after_t3.state().fibers().len(), 2, "F1 (parked on its 60s timer) and F2 (parked on BAR) both still live");

        // F1's 60s timer fires — resumes to WaitState::Running (does not
        // yet re-enter the instruction stream; that's the next Tick).
        let context4 = DeterministicContext::new(203, Uuid::from_u128(204), 4);
        let claimed_timer = bpmn_lite_types::ClaimedTimer::new(
            bpmn_lite_types::ClaimedTimerIdentity::new(
                bpmn_lite_types::TenantId::new("tenant-a").unwrap(),
                EffectId::for_instruction(f1.fiber_id, f1.fiber_id, 3),
                after_t3.state().instance().instance_id,
                f1.fiber_id,
            ),
            60_200,
            TimerKind::Wait,
            None,
            Uuid::nil(),
        );
        let t4 = apply(
            &workflow,
            &Snapshot::new(after_t3.state().instance().clone(), after_t3.state().fibers().values().cloned())
                .with_concurrency_table(after_t3.state().concurrency_table().clone()),
            &Command::TimerFired { timer: claimed_timer, fired_at: 60_200 },
            &context4,
        )
        .unwrap();
        assert_eq!(t4.fibers_upsert()[0].wait, WaitState::Running);
        let after_t4 = materialize_snapshot(after_t3.state(), &t4, workflow.envelope().abi_version(), 4);

        // F1 resumes: 4 (V2Join, LAST arrival — BAR retires) -> 14
        // (V2GuardEnd, pops+retires G) -> 15 (End), all in one transition.
        // This is the oracle's §2 step 5 golden transition.
        let context5 = DeterministicContext::new(204, Uuid::from_u128(205), 5);
        let t5 = apply(
            &workflow,
            &Snapshot::new(after_t4.state().instance().clone(), after_t4.state().fibers().values().cloned())
                .with_concurrency_table(after_t4.state().concurrency_table().clone()),
            &Command::Tick { fiber_id: Some(f1.fiber_id) },
            &context5,
        )
        .unwrap();

        assert_eq!(
            t5.fibers_delete(),
            &[f2.fiber_id, f1.fiber_id],
            "F2 cancelled when BAR retires, then F1 itself deletes on reaching End"
        );
        assert!(
            t5.fibers_upsert().is_empty(),
            "F1 (the survivor) runs straight through GuardEnd to End in the same transition — not parked, not re-upserted"
        );
        assert_eq!(
            t5.concurrency_mutations().len(),
            3,
            "K-1 ancestor fix-up for F2 (still lists G via effective_control_stack, V4.2) + Retire(BAR) + Retire(G)"
        );
        assert!(matches!(
            &t5.concurrency_mutations()[0],
            ConcurrencyMutation::Insert(record) if record.id == guard_handle
        ));
        assert!(matches!(
            &t5.concurrency_mutations()[1],
            ConcurrencyMutation::Retire(id) if *id == barrier_handle
        ));
        assert!(matches!(
            &t5.concurrency_mutations()[2],
            ConcurrencyMutation::Retire(id) if *id == guard_handle
        ));
        assert_eq!(
            t5.control_stack_deltas().len(),
            2,
            "F1's own Pop(BAR) then Pop(G) — F2's stack was already fully accounted for by its deletion (v0.4 ruling C)"
        );

        // Byte-identical reproducibility (plan 4.5): the same (artifact,
        // frame, command, context) tuple must produce byte-identical
        // canonical bytes across runs.
        let t5_replay = apply(
            &workflow,
            &Snapshot::new(after_t4.state().instance().clone(), after_t4.state().fibers().values().cloned())
                .with_concurrency_table(after_t4.state().concurrency_table().clone()),
            &Command::Tick { fiber_id: Some(f1.fiber_id) },
            &context5,
        )
        .unwrap();
        assert_eq!(t5.parity_bytes().unwrap(), t5_replay.parity_bytes().unwrap());
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
        // A18 retarget: `V2CancelScope` only ever restores a rollback
        // snapshot for a `GUARD-R>`-opened handle now (`V2Guard` is
        // control-only, no snapshot to restore — V-10's companion check
        // rejects `V2CancelScope` against a plain `V2Guard` at verify
        // time). This fixture originally used `V2Guard`; retargeted to
        // `V2GuardR` to keep testing what it always intended to prove.
        // `V2GuardR` carries no `handler` field (A18), so unlike the
        // original `V2Guard` fixture this program has no second static
        // edge to an `End` for `verify_program`'s global
        // entry-reaches-an-End check to find (`V2CancelScope` is
        // deliberately a dead end in the static CFG — see its
        // `successors()` doc comment — even though the kernel does
        // continue the calling fibre past it dynamically). Addresses 0-1
        // (`PushBool(false); BrIf { target: 7 }`) supply that second,
        // never-dynamically-taken static edge: a legal, forward, stack-
        // effect-neutral branch whose only purpose is to make address 7
        // statically reachable, exactly the role the old handler edge
        // played.
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [12u8; 32],
            program: vec![
                /* 0 */ Instr::PushBool(false),
                /* 1 */ Instr::BrIf { target: Addr::new(7) },
                /* 2 */ Instr::V2GuardR,
                /* 3 */ Instr::PushI64(1_000),
                /* 4 */ Instr::V2WaitFor,
                /* 5 */ Instr::V2CancelScope,
                /* 6 */ Instr::End,
                /* 7 */ Instr::End,
            ],
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
                EffectId::for_instruction(root_fiber_id, root_fiber_id, 4),
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

    /// A18 A3 rollback-set: `GUARD-R>` restores `domain_payload`, business
    /// `flags`, and `join_expected` on rollback — but deliberately NOT
    /// `ProcessInstance::counters` (loop/retry bounds), since restoring
    /// those would let a failing scope retry unboundedly. Constructed so
    /// the counter case actually distinguishes correct from incorrect
    /// behaviour (not merely asserting a value that happens to match
    /// either way): the counter is mutated from 0 to 5 while parked
    /// inside the scope, and the assertion requires it to STAY 5 after
    /// rollback — a wrongly-restoring implementation would leave it 0,
    /// failing this test.
    #[test]
    fn guard_r_rollback_restores_a3_set_but_not_loop_counters() {
        // Same reachable-end padding shape as
        // `v2_cancel_scope_restores_rollback_snapshot_and_continues_in_place`
        // (`V2GuardR` has no handler edge, and `V2CancelScope` is a static
        // dead end — see that test's own comment for the full rationale).
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [20u8; 32],
            program: vec![
                /* 0 */ Instr::PushBool(false),
                /* 1 */ Instr::BrIf { target: Addr::new(7) },
                /* 2 */ Instr::V2GuardR,
                /* 3 */ Instr::PushI64(1_000),
                /* 4 */ Instr::V2WaitFor,
                /* 5 */ Instr::V2CancelScope,
                /* 6 */ Instr::End,
                /* 7 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "a18-a3-rollback-set").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;

        // Seed pre-scope state on the base instance: one flag, one dynamic
        // join-expected entry, zero on the loop counter. This is the
        // snapshot `V2GuardR` must capture and later restore (minus the
        // counter).
        let flag_key: bpmn_lite_types::FlagKey = 7;
        let join_id: JoinId = 42;
        let counter_id: u32 = 1;
        let mut base_instance = base_snapshot.instance().clone();
        base_instance.flags.insert(flag_key, Value::Bool(false));
        base_instance.join_expected.insert(join_id, 3);
        base_instance.counters.insert(counter_id, 0);
        let original_payload = base_instance.domain_payload.to_string();
        let original_flags = base_instance.flags.clone();
        let original_join_expected = base_instance.join_expected.clone();
        let snapshot = Snapshot::new(base_instance, [Fiber::new(root_fiber_id, 0)]);

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

        // Tick 1: PushBool/BrIf (dead branch not taken) -> V2GuardR
        // (captures the A3 snapshot: payload, flags, join_expected,
        // session stack — NOT counters) -> PushI64 -> V2WaitFor parks.
        let context1 = DeterministicContext::new(200, Uuid::from_u128(401), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let guard_handle = t1.fibers_upsert()[0].control_stack[0];
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        let captured = after_t1.state().concurrency_table().get(guard_handle).unwrap();
        assert_eq!(captured.rollback_flags.as_ref().unwrap(), &original_flags);
        assert_eq!(
            captured.rollback_join_expected.as_ref().unwrap(),
            &original_join_expected
        );

        // Resolve the parked timer.
        let context1b = DeterministicContext::new(400, Uuid::from_u128(4015), 1);
        let claimed_timer = bpmn_lite_types::ClaimedTimer::new(
            bpmn_lite_types::ClaimedTimerIdentity::new(
                bpmn_lite_types::TenantId::new("tenant-a").unwrap(),
                EffectId::for_instruction(root_fiber_id, root_fiber_id, 4),
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
        let after_t1c = materialize_snapshot(after_t1.state(), &t1c, workflow.envelope().abi_version(), 1);

        // Mutate everything while the fibre is parked inside the scope —
        // domain_payload, flags, join_expected (all A3-restorable), and
        // the loop counter (NOT A3-restorable, must survive rollback).
        let mut resumed_instance = after_t1c.state().instance().clone();
        resumed_instance.domain_payload = "mutated-while-parked".to_string().into();
        resumed_instance.domain_payload_hash = EffectId::content_hash(b"mutated-while-parked");
        resumed_instance.flags.insert(flag_key, Value::Bool(true));
        resumed_instance.join_expected.insert(join_id, 1);
        resumed_instance.counters.insert(counter_id, 5);
        let snapshot_running = Snapshot::new(resumed_instance, after_t1c.state().fibers().values().cloned())
            .with_concurrency_table(after_t1c.state().concurrency_table().clone());

        // Tick 2: V2CancelScope restores the A3 set, leaves the counter
        // alone, continues to End in the same transition.
        let context2 = DeterministicContext::new(401, Uuid::from_u128(402), 2);
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
            "A3: domain_payload must be restored"
        );
        assert_eq!(
            t2.next_snapshot().flags, original_flags,
            "A3: business flags must be restored to their pre-scope values"
        );
        assert_eq!(
            t2.next_snapshot().join_expected, original_join_expected,
            "A3: join_expected must be restored to its pre-scope values"
        );
        assert_eq!(
            t2.next_snapshot().counters.get(&counter_id).copied(),
            Some(5),
            "A3 deliberately excludes loop/retry counters — restoring them would let a \
             failing scope retry unboundedly. A wrongly-restoring implementation would \
             leave this at the pre-scope value (0), not the mutated one (5)."
        );
    }

    /// A18/V-10: `V2CancelScope` targeting a plain (non-rollback-capable)
    /// `V2Guard`/`V2GuardN` handle must be rejected at VERIFY time, not
    /// merely fail at runtime against a `None` snapshot.
    #[test]
    fn v2_verifier_rejects_cancel_scope_against_plain_guard() {
        let program = vec![
            /* 0 */ Instr::V2Guard { handler: Addr::new(4) },
            /* 1 */ Instr::V2CancelScope,
            /* 2 */ Instr::End,
            /* 3 */ Instr::End, // dead fallthrough of V2Guard's own close, unused
            /* 4 */ Instr::End, // handler target
        ];
        let legacy = bpmn_lite_types::legacy_program! {
            bytecode_version: [21u8; 32],
            program: program,
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
        let result = ArtifactEnvelope::from_legacy_program(legacy, "v10-cancel-scope-wrong-target");
        let err = result.expect_err("V2CancelScope against a plain V2Guard handle must be rejected");
        let message = format!("{err:?}");
        assert!(
            message.contains("V-10"),
            "expected a V-10 mistargeting violation, got {message}"
        );
    }

    /// A18 V-10, positive admission: `GUARD-R>` MAY contain a complete
    /// `FORK`/`JOIN` region (only being CONTAINED BY one is forbidden —
    /// see `v2_verifier_v10_rejects_guard_r_nested_inside_a_fork_branch`).
    #[test]
    fn v2_verifier_v10_admits_guard_r_containing_a_complete_fork_join() {
        let program = vec![
            /* 0 */ Instr::V2GuardR,
            /* 1 */ Instr::V2Fork {
                targets: Box::new([Addr::new(3), Addr::new(6)]),
                pairing: Addr::new(1),
            },
            /* 2 */ Instr::End, // unreachable filler (V2Fork has no fallthrough)
            /* 3 */ Instr::V2Join { pairing: Addr::new(1) },
            /* 4 */ Instr::V2GuardREnd,
            /* 5 */ Instr::End,
            /* 6 */ Instr::V2Join { pairing: Addr::new(1) },
            /* 7 */ Instr::V2GuardREnd,
            /* 8 */ Instr::End,
        ];
        let legacy = bpmn_lite_types::legacy_program! {
            bytecode_version: [22u8; 32],
            program: program,
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
        ArtifactEnvelope::from_legacy_program(legacy, "v10-guard-r-contains-fork-join")
            .expect("GUARD-R> containing a complete FORK/JOIN region must be admitted");
    }

    /// V4.1 automatic rollback-on-fail (Adam-ratified), **rescoped
    /// 2026-07-22 for the §13 amendment** (see
    /// `definitive_job_failure_inside_interrupting_guard_and_spanning_the_whole_instance_parks_on_incident`
    /// below for the amendment itself and its full rationale). This test's
    /// original program shape — a single fibre, guard wrapping the
    /// instance's entire live fibre set — is exactly the case the
    /// amendment changes: killing the trigger fibre there would leave zero
    /// live fibres with a non-terminal state, which Ring 3's own
    /// zero-live-fibre assert (landed alongside the `Instr::End`
    /// remediation, same file) now rejects. That is the *correct* new
    /// behaviour for that shape, not a regression in this test — so this
    /// test is rescoped to what it actually still proves: §13's original
    /// kill-and-no-incident disposition, for the case the amendment leaves
    /// untouched — a guard scope that is a **proper subset** of the
    /// instance's live fibres. The instance now starts with a second,
    /// independent fibre parked on its own job, entered directly at a
    /// second static address rather than spawned via `V2Fork` (a real
    /// `V2Fork`/`V2Join` pairing would introduce barrier-arrival semantics
    /// that are orthogonal to what this test proves — see Part 2's
    /// barrier-starvation investigation for that shape instead), so the
    /// instance survives the rollback independently of the guarded fibre;
    /// the guard-branch assertions below are otherwise unchanged from the
    /// original test.
    ///
    /// A *definitive* job failure (no retry token, no matching armed
    /// error route — the exact point v1 would otherwise create an `Incident`)
    /// for a fibre inside an armed interrupting `V2Guard` scope bypasses
    /// the incident path entirely, restores the scope's rollback snapshot,
    /// and kills the fibre rather than continuing or auto-respawning.
    /// Outside any guard scope, the existing v1 incident path is unchanged
    /// — proven by running the identical failure both inside and outside a
    /// guard.
    #[test]
    fn definitive_job_failure_inside_interrupting_guard_rolls_back_instead_of_incident() {
        // A18 retarget: automatic rollback-on-definitive-failure now only
        // fires for a `GUARD-R>`-opened scope (`V2Guard` is control-only —
        // see `apply_job_failure`'s `innermost_guard` doc comment).
        // Retargeted from `V2Guard`/`V2GuardEnd` to `V2GuardR`/
        // `V2GuardREnd` to keep testing what it always intended to prove.
        //
        // Addresses 0-1 (`PushBool(false); BrIf { target: 8 }`) and 8-10
        // (a never-triggered `V2Guard`/`V2GuardEnd` pair behind that
        // never-taken branch) are test-scaffolding padding, not part of
        // the scenario under test: `sibling_fiber_id` below simulates a
        // second, already-live fibre outside the guard (proving the
        // guard scope is a proper *subset* of the instance's fibres) by
        // being seeded directly into the initial `Snapshot` rather than
        // spawned by any real fork/guard-trigger — with no static
        // fork/guard-with-handler on `root_fiber_id`'s own path any more
        // (`V2GuardR` has none — A18), `verify_program`'s statically
        // computed `max_fibers` ceiling would otherwise be 1, rejecting
        // this legitimately-2-live-fibre initial snapshot. The padding
        // branch supplies a second statically-reachable (never
        // dynamically-taken) fibre-spawn source, exactly restoring the
        // ceiling the original `V2Guard`-with-a-real-handler fixture
        // provided incidentally.
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [13u8; 32],
            program: vec![
                /* 0 */ Instr::PushBool(false),
                /* 1 */ Instr::BrIf { target: Addr::new(8) },
                /* 2 */ Instr::V2GuardR,
                /* 3 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                /* 4 */ Instr::V2GuardREnd,
                /* 5 */ Instr::End,
                /* 6 */ Instr::ExecNative { task_type: 1, argc: 0, retc: 0 },
                /* 7 */ Instr::End,
                /* 8 */ Instr::V2Guard { handler: Addr::new(10) },
                /* 9 */ Instr::V2GuardEnd,
                /* 10 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["SomeTask".to_string(), "OtherTask".to_string()],
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
        let sibling_fiber_id = Uuid::from_u128(424_242);
        let original_payload = base_snapshot.instance().domain_payload.to_string();
        let snapshot = Snapshot::new(
            base_snapshot.instance().clone(),
            [Fiber::new(root_fiber_id, 0), Fiber::new(sibling_fiber_id, 6)],
        );

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

        // Tick 1: root_fiber_id runs V2Guard (captures snapshot) ->
        // ExecNative parks on WaitState::Job.
        let context1 = DeterministicContext::new(300, Uuid::from_u128(301), 1);
        let t1 = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: Some(root_fiber_id) },
            &context1,
        )
        .unwrap();
        let guard_handle = t1.fibers_upsert()[0].control_stack[0];
        let job_key = t1.jobs_enqueue()[0].job_key.clone();
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);

        // Tick 2: sibling_fiber_id — independent of the guard entirely —
        // parks on its own job. This is what keeps the instance alive
        // independent of whatever happens to root_fiber_id.
        let context1b = DeterministicContext::new(300, Uuid::from_u128(3011), 1);
        let t1b = apply(
            &workflow,
            &Snapshot::new(after_t1.state().instance().clone(), after_t1.state().fibers().values().cloned())
                .with_concurrency_table(after_t1.state().concurrency_table().clone()),
            &Command::Tick { fiber_id: Some(sibling_fiber_id) },
            &context1b,
        )
        .unwrap();
        let after_t1b = materialize_snapshot(after_t1.state(), &t1b, workflow.envelope().abi_version(), 2);
        let snapshot_running = Snapshot::new(
            after_t1b.state().instance().clone(),
            after_t1b.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1b.state().concurrency_table().clone());

        // A definitive (non-retriable) failure on root_fiber_id — mutating
        // domain_payload beforehand is not needed to prove the point, but
        // a mutated instance makes the rollback observable rather than a
        // no-op.
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
            attempt: 3,
        };
        let t2 = apply(&workflow, &mutated_snapshot, &fail_command, &context2).unwrap();

        assert_eq!(
            t2.next_snapshot().domain_payload.to_string(),
            original_payload,
            "rollback must restore the pre-scope snapshot, not the mutated value"
        );
        assert!(
            t2.incidents().is_empty(),
            "the v1 incident path must not fire — the guard scope is a proper subset of the \
             instance's live fibres (sibling_fiber_id survives outside it), so §13's original \
             kill-and-no-incident disposition still applies"
        );
        assert_eq!(t2.fibers_delete(), &[root_fiber_id], "the failing fibre is killed, not continued");
        assert!(t2.fibers_upsert().is_empty(), "no auto-respawn");
        assert_eq!(t2.concurrency_mutations().len(), 1);
        assert!(matches!(
            &t2.concurrency_mutations()[0],
            ConcurrencyMutation::Retire(id) if *id == guard_handle
        ));
        assert_eq!(
            t2.next_snapshot().state,
            ProcessState::Running,
            "instance state is untouched by this rollback — sibling_fiber_id keeps it Running"
        );

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
            write_set: BTreeMap::new(),
            task_manifest: vec!["SomeTask".to_string()],
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
                attempt: 7,
            },
            &context4,
        )
        .unwrap();
        assert_eq!(ut2.incidents().len(), 1, "unchanged outside a guard scope: definitive failure still creates an Incident");
        assert_eq!(
            ut2.incidents()[0].retry_count,
            7,
            "V&S §15 ruling E: Incident.retry_count carries the real attempt count, not a hardcoded 0"
        );
    }

    /// §13 amendment (Adam's ruling, 2026-07-22): the spanning case. §13
    /// enumerated three dispositions for the fibre that triggers an
    /// automatic rollback — killed, not continued, not auto-respawned —
    /// and never considered a fourth: parked. When the guard scope being
    /// rolled back is a proper *subset* of the instance's live fibres
    /// (other fibres survive elsewhere), killing the trigger is fine —
    /// that's the case
    /// `definitive_job_failure_inside_interrupting_guard_rolls_back_instead_of_incident`
    /// (rescoped above) still covers. This test is the other case: the
    /// guard scope spans the instance's *entire* live fibre set (this
    /// program's own shape — a single root-level `V2Guard` wrapping the
    /// whole program body, exactly the original pre-rescope fixture) —
    /// killing the trigger too would leave zero live fibres with a
    /// non-terminal state, permanently stuck and silent (§13's original
    /// text also explicitly rules out the v1 incident path firing here,
    /// so there would be no signal of any kind).
    ///
    /// Adam's ruling: don't kill the trigger fibre in this case. Restore
    /// the rollback payload (unchanged from the subset case), pop the
    /// retiring guard off the fibre's own control stack, and park it on an
    /// `Incident` at the guard's `opened_at` address — the guard's own
    /// opening word. This is not auto-respawn (no new fibre) and not
    /// continuation (it doesn't fall through past the guard) — §13's two
    /// prohibitions hold. Resuming via `Command::ResolveIncident`
    /// re-executes `GUARD>`, opening a fresh guard activation with a fresh
    /// rollback snapshot over the now-restored payload — exactly §13's own
    /// stated intent ("the instance is left as it was at scope-open so it
    /// can simply be re-run"), now with a mechanism. The kernel still does
    /// not initiate the retry itself — it parks and waits for an operator
    /// via `ResolveIncident`.
    #[test]
    fn definitive_job_failure_inside_interrupting_guard_and_spanning_the_whole_instance_parks_on_incident() {
        // A18 retarget: as the sibling `..._rolls_back_instead_of_incident`
        // test above — automatic rollback-on-definitive-failure (and thus
        // the §13-amendment park-on-incident path this test proves) now
        // only fires for a `GUARD-R>`-opened scope.
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [15u8; 32],
            program: vec![
                /* 0 */ Instr::V2GuardR,
                /* 1 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                /* 2 */ Instr::V2GuardREnd,
                /* 3 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["SomeTask".to_string()],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-spanning-rollback-parks-on-incident").unwrap(),
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

        // Tick 1: V2Guard (captures snapshot, opened_at = its own address,
        // Addr(0)) -> ExecNative parks the fibre on WaitState::Job.
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

        let mut mutated_instance = snapshot_running.instance().clone();
        mutated_instance.domain_payload = "mutated-before-fail".to_string().into();
        mutated_instance.domain_payload_hash = EffectId::content_hash(b"mutated-before-fail");
        let mutated_snapshot = Snapshot::new(mutated_instance, snapshot_running.fibers().values().cloned())
            .with_concurrency_table(snapshot_running.concurrency_table().clone());

        let context2 = DeterministicContext::new(301, Uuid::from_u128(302), 2);
        let fail_command = Command::EffectFailed {
            effect_id: EffectId::for_instruction(Uuid::nil(), Uuid::nil(), 0),
            job_key: job_key.clone(),
            error_class: ErrorClass::ContractViolation,
            message: "boom".to_string(),
            retry: None,
            attempt: 3,
        };
        let t2 = apply(&workflow, &mutated_snapshot, &fail_command, &context2).unwrap();

        assert_eq!(
            t2.next_snapshot().domain_payload.to_string(),
            original_payload,
            "rollback must restore the pre-scope snapshot even when the fibre survives"
        );
        assert!(
            t2.fibers_delete().is_empty(),
            "the trigger fibre must NOT be deleted — it's the instance's only live fibre"
        );
        assert_eq!(t2.fibers_upsert().len(), 1, "the fibre survives, parked on the incident");
        let surviving = &t2.fibers_upsert()[0];
        assert_eq!(surviving.fiber_id, root_fiber_id);
        assert_eq!(t2.incidents().len(), 1, "no signal at all would otherwise leave this stuck and silent");
        let incident_id = t2.incidents()[0].incident_id;
        assert_eq!(
            surviving.wait,
            WaitState::Incident { incident_id },
            "the surviving fibre must be parked on exactly the incident just raised"
        );
        assert_eq!(
            surviving.pc,
            Addr::new(0),
            "resume address is the guard's own opened_at (GUARD> at Addr(0)), not the failed task"
        );
        assert!(
            surviving.control_stack.is_empty(),
            "the retiring guard (and anything nested above it) must be popped off the \
             surviving fibre's own control stack — re-executing GUARD> must open a fresh \
             record, not layer under the stale retired one"
        );
        assert_eq!(
            t2.next_snapshot().state,
            ProcessState::Incidented { incident_id },
            "instance state follows the same Incidented{incident_id} shape as the unguarded incident path"
        );
        assert_eq!(t2.concurrency_mutations().len(), 1);
        assert!(matches!(
            &t2.concurrency_mutations()[0],
            ConcurrencyMutation::Retire(id) if *id == guard_handle
        ));

        // Drive it further: resolving the incident must resume execution
        // at GUARD> (Addr(0)) and re-arm the record from scratch — not
        // just assert the parked shape, demonstrate the "fresh guard
        // activation" claim.
        let after_t2 = materialize_snapshot(after_t1.state(), &t2, workflow.envelope().abi_version(), 2);
        let resolved_snapshot = Snapshot::new(
            after_t2.state().instance().clone(),
            after_t2.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t2.state().concurrency_table().clone())
        .with_incidents(after_t2.state().incidents().values().cloned());
        assert_eq!(
            resolved_snapshot.instance().state,
            ProcessState::Incidented { incident_id },
            "sanity: materialized instance really is Incidented pre-resolve"
        );

        let context3 = DeterministicContext::new(302, Uuid::from_u128(303), 3);
        let t3 = apply(
            &workflow,
            &resolved_snapshot,
            &Command::ResolveIncident {
                incident_id,
                resolution: "retry".to_string(),
            },
            &context3,
        )
        .unwrap();
        assert_eq!(
            t3.next_snapshot().state,
            ProcessState::Running,
            "ResolveIncident restores Running"
        );
        let resumed_fiber = t3
            .fibers_upsert()
            .iter()
            .find(|fiber| fiber.fiber_id == root_fiber_id)
            .expect("ResolveIncident must upsert the parked fibre back to Running");
        assert_eq!(resumed_fiber.wait, WaitState::Running);
        assert_eq!(
            resumed_fiber.pc,
            Addr::new(0),
            "ResolveIncident does not touch fiber.pc — it resumes exactly where parking left it, GUARD>"
        );

        let after_t3 = materialize_snapshot(after_t2.state(), &t3, workflow.envelope().abi_version(), 3);
        let post_resolve_snapshot = Snapshot::new(
            after_t3.state().instance().clone(),
            after_t3.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t3.state().concurrency_table().clone());

        let context4 = DeterministicContext::new(303, Uuid::from_u128(304), 4);
        let t4 = apply(
            &workflow,
            &post_resolve_snapshot,
            &Command::Tick { fiber_id: Some(root_fiber_id) },
            &context4,
        )
        .unwrap();
        assert_eq!(
            t4.concurrency_mutations().len(),
            1,
            "re-executing GUARD> opens a fresh concurrency record"
        );
        assert!(
            matches!(&t4.concurrency_mutations()[0], ConcurrencyMutation::Insert(record) if record.id != guard_handle),
            "the fresh record must be a genuinely new activation, not a re-arm of the retired one"
        );
        assert!(
            t4.fibers_upsert()[0].control_stack.len() == 1,
            "the resumed fibre picks up the fresh guard handle on its control stack"
        );
    }

    /// V&S §14 amendment v0.6, ruling D: an unmatched `BusinessRejection`
    /// inside an interrupting guard must NOT roll back — §13's original
    /// "no distinction between error classes" clause is superseded. The
    /// workflow's own route map being incomplete is information about the
    /// workflow, not a machine fault, and rolling back would destroy the
    /// evidence that a business outcome occurred. Same program shape as
    /// `definitive_job_failure_inside_interrupting_guard_rolls_back_instead_of_incident`,
    /// same guard, only the error class differs — proving the routing now
    /// depends on `ErrorClass`, not merely on being inside an interrupting
    /// guard.
    #[test]
    fn unmatched_business_rejection_inside_interrupting_guard_incidents_not_rollback() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [17u8; 32],
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
            write_set: BTreeMap::new(),
            task_manifest: vec!["SomeTask".to_string()],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v0.6-unmatched-business-rejection").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let original_payload = base_snapshot.instance().domain_payload.to_string();
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let context1 = DeterministicContext::new(300, Uuid::from_u128(321), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let job_key = t1.jobs_enqueue()[0].job_key.clone();
        let after_t1 = materialize_snapshot(
            &PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
            &t1,
            workflow.envelope().abi_version(),
            1,
        );
        let snapshot_running = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        let context2 = DeterministicContext::new(301, Uuid::from_u128(322), 2);
        let fail_command = Command::EffectFailed {
            effect_id: EffectId::for_instruction(Uuid::nil(), Uuid::nil(), 0),
            job_key: job_key.clone(),
            error_class: ErrorClass::BusinessRejection {
                rejection_code: "NO_ROUTE_FOR_THIS_CODE".to_string(),
            },
            message: "rejected".to_string(),
            retry: None,
            attempt: 1,
        };
        let t2 = apply(&workflow, &snapshot_running, &fail_command, &context2).unwrap();

        assert_eq!(
            t2.incidents().len(),
            1,
            "an unmatched BusinessRejection must surface as an Incident, never roll back, even inside an interrupting guard"
        );
        assert_eq!(t2.incidents()[0].retry_count, 1, "attempt history threads through to the Incident");
        assert!(
            t2.fibers_delete().is_empty(),
            "the incident path parks the fibre, it does not kill it"
        );
        assert_eq!(
            t2.next_snapshot().domain_payload.to_string(),
            original_payload,
            "unmutated in this test, but confirms no rollback-restore path ran"
        );
        assert!(
            t2.concurrency_mutations().is_empty(),
            "no guard scope was retired: rollback must not have run"
        );
    }

    /// V&S §14 amendment v0.6, ruling D: an exhausted-retry `Transient`
    /// failure (reached `apply_job_failure`'s definitive-failure boundary
    /// with `retry: None`) inside an interrupting guard must NOT roll
    /// back — it is the retry budget's own terminal state and belongs in
    /// quarantine (today's Incident path), not silently erased.
    #[test]
    fn exhausted_transient_inside_interrupting_guard_incidents_not_rollback() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [18u8; 32],
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
            write_set: BTreeMap::new(),
            task_manifest: vec!["SomeTask".to_string()],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v0.6-exhausted-transient").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let original_payload = base_snapshot.instance().domain_payload.to_string();
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let context1 = DeterministicContext::new(300, Uuid::from_u128(331), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let job_key = t1.jobs_enqueue()[0].job_key.clone();
        let after_t1 = materialize_snapshot(
            &PersistedSnapshotState::new(
                snapshot.instance().clone(),
                snapshot.fibers().values().cloned(),
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            ),
            &t1,
            workflow.envelope().abi_version(),
            1,
        );
        let snapshot_running = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        let context2 = DeterministicContext::new(301, Uuid::from_u128(332), 2);
        let fail_command = Command::EffectFailed {
            effect_id: EffectId::for_instruction(Uuid::nil(), Uuid::nil(), 0),
            job_key: job_key.clone(),
            error_class: ErrorClass::Transient,
            message: "retries exhausted".to_string(),
            retry: None,
            attempt: 5,
        };
        let t2 = apply(&workflow, &snapshot_running, &fail_command, &context2).unwrap();

        assert_eq!(
            t2.incidents().len(),
            1,
            "an exhausted-retry Transient must surface as an Incident, never roll back, even inside an interrupting guard"
        );
        assert_eq!(
            t2.incidents()[0].retry_count,
            5,
            "V&S §15 ruling E: the attempt history that led to exhaustion is preserved on the Incident, not erased"
        );
        assert!(
            t2.fibers_delete().is_empty(),
            "the incident path parks the fibre, it does not kill it"
        );
        assert_eq!(t2.next_snapshot().domain_payload.to_string(), original_payload);
        assert!(
            t2.concurrency_mutations().is_empty(),
            "no guard scope was retired: rollback must not have run"
        );
    }

    /// V4.6 blind-review regression: ruling C's rollback-on-fail carve-out
    /// is keyed to the *innermost* armed guard, not "any interrupting
    /// guard anywhere on the stack." A fibre nested
    /// `V2Guard(interrupting) > V2GuardN(non-interrupting) > [failing task]`
    /// must take the v1 incident path — the innermost armed guard is the
    /// GuardN, and non-interrupting guards are explicitly carved out —
    /// even though an outer interrupting V2Guard also sits on the stack.
    #[test]
    fn definitive_job_failure_under_non_interrupting_guard_nested_inside_interrupting_guard_still_incidents() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [16u8; 32],
            program: vec![
                /* 0 */ Instr::V2Guard { handler: Addr::new(6) },
                /* 1 */ Instr::V2GuardN { handler: Addr::new(7) },
                /* 2 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                /* 3 */ Instr::V2GuardNEnd,
                /* 4 */ Instr::V2GuardEnd,
                /* 5 */ Instr::End,
                /* 6 */ Instr::End,
                /* 7 */ Instr::V2GuardNEnd,
                /* 8 */ Instr::V2GuardEnd,
                /* 9 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["SomeTask".to_string()],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-nested-guardn-under-guard").unwrap(),
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

        // Tick 1: V2Guard -> V2GuardN -> ExecNative parks the fibre on
        // WaitState::Job, with both guard handles on the control stack,
        // outer (interrupting) first, inner (non-interrupting) last.
        let context1 = DeterministicContext::new(300, Uuid::from_u128(311), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        assert_eq!(t1.fibers_upsert()[0].control_stack.len(), 2);
        let job_key = t1.jobs_enqueue()[0].job_key.clone();
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        let snapshot_running = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        let mut mutated_snapshot = snapshot_running.clone();
        let mut mutated_instance = mutated_snapshot.instance().clone();
        mutated_instance.domain_payload = "mutated-before-fail-nested".to_string().into();
        mutated_instance.domain_payload_hash = EffectId::content_hash(b"mutated-before-fail-nested");
        mutated_snapshot = Snapshot::new(mutated_instance, mutated_snapshot.fibers().values().cloned())
            .with_concurrency_table(mutated_snapshot.concurrency_table().clone());

        let context2 = DeterministicContext::new(301, Uuid::from_u128(312), 2);
        let fail_command = Command::EffectFailed {
            effect_id: EffectId::for_instruction(Uuid::nil(), Uuid::nil(), 0),
            job_key: job_key.clone(),
            error_class: ErrorClass::ContractViolation,
            message: "boom".to_string(),
            retry: None,
            attempt: 4,
        };
        let t2 = apply(&workflow, &mutated_snapshot, &fail_command, &context2).unwrap();

        assert_eq!(
            t2.incidents().len(),
            1,
            "innermost armed guard is non-interrupting: the v1 incident path must fire, \
             not automatic rollback via the outer interrupting guard"
        );
        assert!(
            t2.fibers_delete().is_empty(),
            "the incident path parks the fibre, it does not kill it"
        );
        assert_ne!(
            t2.next_snapshot().domain_payload.to_string(),
            original_payload,
            "no rollback occurred: the mutated payload must survive, not be restored"
        );
        assert!(
            t2.concurrency_mutations().is_empty(),
            "no guard scope was retired: rollback must not have run"
        );
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
            write_set: BTreeMap::new(),
            task_manifest: vec![],
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
        assert_eq!(
            t2.concurrency_mutations().len(),
            1,
            "record re-arms with the same identity/state — the one mutation is K-2's \
             ancestor-membership fix-up (V4.2): the handler now shares guard_handle on its \
             own control stack, so it must be added to guard_handle's members too"
        );
        assert!(matches!(
            &t2.concurrency_mutations()[0],
            ConcurrencyMutation::Insert(record)
                if record.id == guard_handle
                    && record.members.contains(&t2.fibers_upsert()[0].fiber_id)
                    && record.members.contains(&root_fiber_id)
        ));
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
            write_set: BTreeMap::new(),
            task_manifest: vec![],
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

    /// V4.3 — Ring 3 must actually reject a corrupted frame, not just stay
    /// silent because everything happens to be fine (a gate with only a
    /// passing test is not a gate). Hand-constructs a fibre whose control
    /// stack already carries a handle that resolves to no concurrency
    /// record at all — real corruption (or a hand-authored test snapshot,
    /// same thing from the kernel's point of view) — and confirms `apply`
    /// rejects it via `TransitionError::Integrity` even though the word
    /// actually executed (`V2WaitFor`) never touches `control_stack`.
    #[test]
    fn ring3_rejects_a_dangling_control_stack_handle() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [20u8; 32],
            program: vec![
                /* 0 */ Instr::PushI64(60_000),
                /* 1 */ Instr::V2WaitFor,
                /* 2 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4.3-ring3-dangling-handle").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let bogus_handle = RecordId::new(Uuid::from_u128(0xDEAD));
        let mut fiber = Fiber::new(root_fiber_id, 0);
        fiber.control_stack = vec![bogus_handle];
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [fiber]);

        let result = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context);
        assert!(
            matches!(&result, Err(TransitionError::Integrity(_))),
            "expected a Ring 3 rejection, got {result:?}"
        );
    }

    /// V4.3 — isolates the K-2 shadow specifically (as opposed to the
    /// depth-limit check `ring3_rejects_a_dangling_control_stack_handle`
    /// happens to hit first for a program with no scope words at all):
    /// runs a real `V2Guard` to get a legitimately-`Armed` record and a
    /// control-stack depth within the verified limit, then feeds `apply`
    /// a snapshot where that same record has been dropped from the
    /// concurrency table entirely — depth is fine, the handle just
    /// doesn't resolve.
    #[test]
    fn ring3_rejects_a_handle_whose_record_was_dropped() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [23u8; 32],
            program: vec![
                /* 0 */ Instr::V2Guard { handler: Addr::new(4) },
                /* 1 */ Instr::PushI64(60_000),
                /* 2 */ Instr::V2WaitFor,
                /* 3 */ Instr::V2GuardEnd,
                /* 4 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4.3-ring3-dropped-record").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context1) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let parked = t1.fibers_upsert()[0].clone();
        assert_eq!(parked.control_stack.len(), 1);

        // Corruption: the fibre still carries the handle, but the record
        // it points to was never inserted.
        let corrupted_snapshot = Snapshot::new(base_snapshot.instance().clone(), [parked]);
        let context2 = DeterministicContext::new(1_100, Uuid::from_u128(1_101), 2);
        let claimed_timer = bpmn_lite_types::ClaimedTimer::new(
            bpmn_lite_types::ClaimedTimerIdentity::new(
                bpmn_lite_types::TenantId::new("tenant-a").unwrap(),
                EffectId::for_instruction(root_fiber_id, root_fiber_id, 2),
                base_snapshot.instance().instance_id,
                root_fiber_id,
            ),
            60_000,
            TimerKind::Wait,
            None,
            Uuid::nil(),
        );
        let result = apply(
            &workflow,
            &corrupted_snapshot,
            &Command::TimerFired { timer: claimed_timer, fired_at: 60_000 },
            &context2,
        );
        assert!(
            matches!(
                &result,
                Err(TransitionError::Integrity(bpmn_lite_types::IntegrityError::Ring3Runtime(msg)))
                    if msg.contains("no such record exists")
            ),
            "expected a K-2 Ring 3 rejection, got {result:?}"
        );
    }

    /// V4 remediation (found during V5 scoping, 2026-07-22, via the
    /// differential harness's end-to-end drive, not via `apply` unit
    /// tests): `Instr::End` used to decide "am I the last fibre?" from
    /// `snapshot.fibers().len()` — the *pre*-transition count. Ruling B's
    /// last-arrival-survives-and-continues `V2Join` semantics make deletion
    /// and `End` land in the *same* transition for the single most common
    /// BPMN shape (fork → two branches → join → end, no trailing task):
    /// the last-arriving branch retires the barrier (cancelling its
    /// sibling, pushed onto `fibers_delete`), falls through `Jump` into
    /// `End` in the same step, which pushes its own id onto
    /// `fibers_delete` too — but the *pre*-transition count never reflected
    /// either deletion, so `instance.state` never became `Completed` even
    /// though both fibres genuinely got deleted. Net effect: instance stuck
    /// `Running` forever with zero live fibres.
    ///
    /// Red before the fix (confirmed by temporarily reverting `Instr::End`
    /// to the old `snapshot.fibers().len() == 1` check, with the new Ring 3
    /// assert below also disabled so the raw pre-fix behaviour was
    /// observable rather than intercepted a second way): tick 3's `apply`
    /// returned `Ok`, and `t3.next_snapshot().state` was `ProcessState::Running`
    /// — `expected Completed, got Running` — with `t3.fibers_delete()`
    /// still containing both fibre ids. Genuinely stuck, not hypothetically:
    /// both fibres really deleted, state never advanced. (With only the
    /// Ring 3 assert re-enabled and `Instr::End` still reverted, `apply`
    /// instead returns `Err(Integrity(Ring3Runtime(..)))` on tick 3 — the
    /// two fixes independently catch the same defect via different paths,
    /// as intended.) Green after restoring both: `Completed`.
    #[test]
    fn terminal_commands_succeed_mid_fork_and_leave_a_k_clean_frame() {
        // EOP-FUZZ F2 finding (O5 oracle, 2026-07-25), cement-locked:
        // before `retire_all_armed_records`, `Command::Terminate` and
        // `Command::Cancel` against a mid-fork snapshot (armed barrier,
        // live children) were REJECTED by the Ring 3 frame check — the
        // terminal cleanup deleted every fibre but left the armed barrier
        // with members, a K-1 violation — making any in-flight fork
        // un-cancellable and un-terminatable. Red under the pre-fix
        // kernel; green now.
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [25u8; 32],
            program: vec![
                /* 0 */ Instr::V2Fork {
                    targets: Box::new([Addr::new(1), Addr::new(3)]),
                    pairing: Addr::new(0),
                },
                /* 1 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 2 */ Instr::Jump { target: Addr::new(5) },
                /* 3 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 4 */ Instr::Jump { target: Addr::new(5) },
                /* 5 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "fuzz-f2-terminal-sweep").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot =
            Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        // Tick: V2Fork arms the barrier and spawns both children.
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
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
        let mid_fork =
            materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        // Precondition: the mid-fork frame is K-clean and holds an armed record.
        check_k_invariants(mid_fork.state().fibers(), mid_fork.state().concurrency_table())
            .expect("mid-fork frame must be K-clean before the terminal command");
        assert!(
            mid_fork
                .state()
                .concurrency_table()
                .iter()
                .any(|(_, record)| record.state == RecordState::Armed),
            "test precondition: an armed barrier record must exist mid-fork"
        );
        let mid_fork_snapshot = Snapshot::new(
            mid_fork.state().instance().clone(),
            mid_fork.state().fibers().values().cloned(),
        )
        .with_concurrency_table(mid_fork.state().concurrency_table().clone());

        for (label, command) in [
            ("Terminate", Command::Terminate),
            (
                "Cancel",
                Command::Cancel {
                    reason: "operator cancel mid-fork".to_string(),
                },
            ),
        ] {
            let transition = apply(&workflow, &mid_fork_snapshot, &command, &context)
                .unwrap_or_else(|error| {
                    panic!("{label} must succeed against a mid-fork snapshot: {error:?}")
                });
            let after = materialize_snapshot(
                mid_fork.state(),
                &transition,
                workflow.envelope().abi_version(),
                2,
            );
            assert!(after.state().fibers().is_empty(), "{label}: all fibres deleted");
            assert!(
                after
                    .state()
                    .concurrency_table()
                    .iter()
                    .all(|(_, record)| record.state != RecordState::Armed),
                "{label}: no armed record may survive terminal cleanup"
            );
            check_k_invariants(after.state().fibers(), after.state().concurrency_table())
                .unwrap_or_else(|message| {
                    panic!("{label}: post-terminal frame violates K-invariants: {message}")
                });
        }
    }

    #[test]
    fn v2_fork_join_end_completes_instance_not_stuck_running() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [24u8; 32],
            program: vec![
                /* 0 */ Instr::V2Fork {
                    targets: Box::new([Addr::new(1), Addr::new(3)]),
                    pairing: Addr::new(0),
                },
                /* 1 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 2 */ Instr::Jump { target: Addr::new(5) },
                /* 3 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 4 */ Instr::Jump { target: Addr::new(5) },
                /* 5 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-fork-join-end-remediation").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        // Tick 1: V2Fork spawns the two branches, root fibre dies.
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
        assert_eq!(t1.fibers_delete(), &[root_fiber_id]);
        assert_eq!(t1.fibers_upsert().len(), 2);
        let (child_a, child_b) = (t1.fibers_upsert()[0].clone(), t1.fibers_upsert()[1].clone());

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
        let snapshot = Snapshot::new(
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

        let after_t2 = materialize_snapshot(after_t1.state(), &t2, workflow.envelope().abi_version(), 2);
        let snapshot = Snapshot::new(
            after_t2.state().instance().clone(),
            after_t2.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t2.state().concurrency_table().clone());

        // Tick 3: child_b is the last arrival — sole survivor, falls
        // straight through Jump into End in the same transition. Both
        // fibres end up in `fibers_delete` (child_a cancelled by the join
        // retiring, child_b deleted by End itself) with zero upserts — the
        // exact shape the pre-transition-count check couldn't see.
        let t3 = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: Some(child_b.fiber_id) },
            &context,
        )
        .unwrap();
        assert_eq!(
            std::collections::BTreeSet::from_iter(t3.fibers_delete().iter().copied()),
            std::collections::BTreeSet::from([child_a.fiber_id, child_b.fiber_id])
        );
        assert!(t3.fibers_upsert().is_empty());
        assert!(
            matches!(t3.next_snapshot().state, ProcessState::Completed { .. }),
            "expected Completed, got {:?} — instance stuck Running with zero live fibres \
             is exactly the pre-fix defect",
            t3.next_snapshot().state
        );
    }

    /// V-1's `EndTerminate` exemption (mirrors `Fail`'s pre-existing
    /// treatment: neither is matched in `v2_verifier.rs`'s V-1 walk, both
    /// fall through to the no-op `_` arm) — the K-invariant side of the
    /// rider Adam's ruling attached to it: "the kernel must actually
    /// retire all records and delete all fibres on `EndTerminate`." One
    /// branch of a `V2Fork` reaches `EndTerminate` directly (no `V2Join`)
    /// while its sibling branch is still live, mid-flight, with a SECOND
    /// open record of its own (a `V2GuardN` scope) nested inside it — two
    /// distinct `Armed` records must both retire in the same transition
    /// `EndTerminate` fires in, not just the fork barrier. Without the
    /// kernel-side fix, both records would survive forever as zero-member
    /// `Armed` entries: a real K-1/K-2 orphaned-record defect, unguarded
    /// by the verifier once V-1 stopped requiring the pop for
    /// `EndTerminate` specifically.
    #[test]
    fn v2_endterminate_retires_every_live_sibling_record_not_just_the_terminating_branch() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [26u8; 32],
            program: vec![
                /* 0 */ Instr::V2Fork {
                    targets: Box::new([Addr::new(1), Addr::new(2)]),
                    pairing: Addr::new(0),
                },
                /* 1 */ Instr::EndTerminate, // branch A: terminates the whole instance
                /* 2 */ Instr::V2GuardN { handler: Addr::new(6) }, // branch B: opens a 2nd record
                /* 3 */ Instr::PushI64(1_000),
                /* 4 */ Instr::V2WaitFor, // parks — branch B is mid-flight when A fires
                /* 5 */ Instr::EndTerminate, // branch B's own reachable terminal (never
                //       actually reached in this test — branch A fires first — but
                //       must be statically present for V-11; exempt from popping
                //       either the guard or fork handle, same as branch A's)
                /* 6 */ Instr::V2GuardNEnd, // handler entry (post-push, unreached here) —
                //       pops the guard's own re-armed token; the fork barrier from the
                //       enclosing V2Fork is still on this path's inherited stack (handler
                //       entry state inherits ancestors above the guard, per V4.1), so this
                //       path terminates via EndTerminate (exempt) too, not End.
                /* 7 */ Instr::EndTerminate,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v-1-endterminate-exemption-k-invariant")
                .unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        // Distinct contexts per tick (distinct `next_revision`), not one
        // shared context — `derived_id` is `EffectId::for_command(command_id,
        // next_revision, ordinal)`, so reusing the same context across ticks
        // would derive the SAME record id for tick 2's `V2GuardN` open as
        // tick 1's `V2Fork` open (both start their own local `ordinal` at 0).
        let context1 = DeterministicContext::new(500, Uuid::from_u128(501), 1);
        let context2 = DeterministicContext::new(501, Uuid::from_u128(502), 2);
        let context3 = DeterministicContext::new(502, Uuid::from_u128(503), 3);

        // Tick 1: V2Fork spawns both branches; root fibre dies.
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        assert_eq!(t1.fibers_delete(), &[root_fiber_id]);
        assert_eq!(t1.fibers_upsert().len(), 2);
        let barrier_id = t1
            .concurrency_mutations()
            .iter()
            .find_map(|m| match m {
                ConcurrencyMutation::Insert(record) => Some(record.id),
                _ => None,
            })
            .expect("V2Fork must insert exactly one barrier record");
        let (child_a, child_b) = (t1.fibers_upsert()[0].clone(), t1.fibers_upsert()[1].clone());
        assert!(check_k_invariants(
            &t1.fibers_upsert().iter().map(|f| (f.fiber_id, f.clone())).collect(),
            &{
                let mut table = bpmn_lite_types::concurrency::ConcurrencyTable::new();
                for m in t1.concurrency_mutations() {
                    if let ConcurrencyMutation::Insert(record) = m {
                        table.insert((**record).clone());
                    }
                }
                table
            }
        )
        .is_ok());

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
        let snapshot = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        // Tick 2: branch B runs ahead — opens its own V2GuardN (a SECOND
        // live record, distinct from the fork barrier) and parks on
        // V2WaitFor, mid-flight.
        let t2 = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: Some(child_b.fiber_id) },
            &context2,
        )
        .unwrap();
        let guard_id = t2
            .concurrency_mutations()
            .iter()
            .find_map(|m| match m {
                ConcurrencyMutation::Insert(record) => Some(record.id),
                _ => None,
            })
            .expect("V2GuardN must insert its own record");
        assert_ne!(guard_id, barrier_id, "two distinct live records must now be open");
        let after_t2 = materialize_snapshot(after_t1.state(), &t2, workflow.envelope().abi_version(), 2);
        assert_eq!(
            after_t2.state().concurrency_table().get(barrier_id).unwrap().state,
            RecordState::Armed
        );
        assert_eq!(
            after_t2.state().concurrency_table().get(guard_id).unwrap().state,
            RecordState::Armed
        );
        let snapshot = Snapshot::new(
            after_t2.state().instance().clone(),
            after_t2.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t2.state().concurrency_table().clone());

        // Tick 3: branch A fires EndTerminate — accepted despite never
        // reaching V2Join (V-1's new exemption) — and must retire BOTH
        // still-`Armed` records (the fork barrier AND branch B's guard),
        // not just clean up its own fibre.
        let t3 = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: Some(child_a.fiber_id) },
            &context3,
        )
        .unwrap();
        assert!(matches!(
            t3.next_snapshot().state,
            ProcessState::Terminated { .. }
        ));
        let retired: std::collections::BTreeSet<RecordId> = t3
            .concurrency_mutations()
            .iter()
            .filter_map(|m| match m {
                ConcurrencyMutation::Retire(id) => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(
            retired,
            std::collections::BTreeSet::from([barrier_id, guard_id]),
            "every Armed record must retire in the SAME transition EndTerminate fires in"
        );

        let after_t3 = materialize_snapshot(after_t2.state(), &t3, workflow.envelope().abi_version(), 3);
        assert!(
            after_t3.state().fibers().is_empty(),
            "zero live fibres after EndTerminate"
        );
        assert_eq!(
            after_t3.state().concurrency_table().get(barrier_id).unwrap().state,
            RecordState::Retired,
            "fork barrier must not survive as a zero-member Armed record"
        );
        assert_eq!(
            after_t3.state().concurrency_table().get(guard_id).unwrap().state,
            RecordState::Retired,
            "sibling branch's open guard must not survive as an orphaned Armed record"
        );
        assert!(
            check_k_invariants(after_t3.state().fibers(), after_t3.state().concurrency_table()).is_ok(),
            "no K-1/K-2 violation: zero fibres, and no live fibre references a retired record"
        );
    }

    /// §18 v0.10 ruling H, hypothesis fixture (this step's own deliverable,
    /// per Adam's brief — proven by construction, not by reasoning about
    /// the abstract word table). Claim under test: `V2Fork`'s `targets`
    /// array does NOT need to become an operand-stack-driven runtime count
    /// to give "dynamic arity" — a branch that doesn't do real work can
    /// simply jump straight to its own `V2Join` using ONLY existing
    /// opcodes (`PushBool`/`BrIf`/`Jump`), and the barrier still retires
    /// correctly because that branch still calls the SAME `V2Join`
    /// instruction every other branch calls. `V2Fork` here has 3 STATIC
    /// targets (unchanged shape, `targets.len() == 3`, exactly as every
    /// other `V2Fork` fixture in this file) — two do trivial "real work"
    /// (a `Jump` chain standing in for it), the third (`child_c`, spawned
    /// at address 5) evaluates a condition and `BrIf`s straight past its
    /// own nominal work at address 7 to the shared `V2Join` at address 8.
    /// All three still physically execute a `V2Join`; K-3's `count/arity`
    /// bookkeeping (`Instr::V2Fork`'s kernel handler, unmodified — see
    /// this test's own diff, which touches no non-test code) never sees a
    /// count other than 3, because 3 fibres really were spawned and all 3
    /// really do arrive. `check_k_invariants`'s K-3 assertion (wired into
    /// every `apply`/materialize path already, not specially invoked here)
    /// is exercised implicitly at every tick below — a K-3 violation would
    /// surface as `Err(Integrity(Ring3Runtime(..)))`, not as a silent pass.
    #[test]
    fn v2fork_mixed_real_work_and_skip_to_join_branches_retires_barrier_via_unmodified_mechanism() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [25u8; 32],
            program: vec![
                /* 0 */ Instr::V2Fork {
                    targets: Box::new([Addr::new(1), Addr::new(3), Addr::new(5)]),
                    pairing: Addr::new(0),
                },
                /* 1 */ Instr::Jump { target: Addr::new(2) }, // branch A: "real work" stand-in
                /* 2 */ Instr::Jump { target: Addr::new(8) }, // -> shared V2Join
                /* 3 */ Instr::Jump { target: Addr::new(4) }, // branch B: "real work" stand-in
                /* 4 */ Instr::Jump { target: Addr::new(8) },
                /* 5 */ Instr::PushBool(true), // branch C: the skip condition
                /* 6 */ Instr::BrIf { target: Addr::new(8) }, // skip STRAIGHT to V2Join —
                //       no new opcode, this IS "dynamic arity" as a lowering
                //       pattern, not a kernel mechanism change.
                /* 7 */ Instr::Jump { target: Addr::new(8) }, // branch C's own "real work"
                //       path — statically reachable (V-11 must still admit
                //       it) but never taken at THIS runtime, since address 6
                //       always evaluates true here.
                /* 8 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 9 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v18-ruling-h-fork-skip-to-join").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);

        // Tick 1: V2Fork spawns all 3 branches (arity = count = targets.len()
        // = 3, K-3's bound holds at birth exactly as it always has — no
        // change to the kernel handler was made or needed), root fibre dies.
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
        assert_eq!(t1.fibers_delete(), &[root_fiber_id]);
        assert_eq!(t1.fibers_upsert().len(), 3);
        let record = t1
            .concurrency_mutations()
            .iter()
            .find_map(|mutation| match mutation {
                bpmn_lite_types::concurrency::ConcurrencyMutation::Insert(record) => Some(record.clone()),
                _ => None,
            })
            .expect("V2Fork must insert exactly one barrier record");
        assert_eq!(
            record.counters,
            RecordCounters { arity: 3, count: 3 },
            "arity/count must equal targets.len() (3), unchanged from the pre-existing \
             fixed-arity mechanism — no operand-stack-driven runtime count exists or was added"
        );
        let (child_a, child_b, child_c) = (
            t1.fibers_upsert()[0].clone(),
            t1.fibers_upsert()[1].clone(),
            t1.fibers_upsert()[2].clone(),
        );

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
        let snapshot = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());

        // Tick 2: child_a ("real work" branch) arrives at the shared V2Join
        // via a Jump chain — 1st arrival, not last, parks.
        let t2 = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: Some(child_a.fiber_id) },
            &context,
        )
        .unwrap();
        assert!(t2.fibers_delete().is_empty());
        let after_t2 = materialize_snapshot(after_t1.state(), &t2, workflow.envelope().abi_version(), 2);
        let snapshot = Snapshot::new(
            after_t2.state().instance().clone(),
            after_t2.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t2.state().concurrency_table().clone());

        // Tick 3: child_b ("real work" branch) arrives — 2nd of 3, still not
        // last, parks too. Barrier count now 2/3 live members, K-3 (0 <
        // count <= arity) holds throughout — checked implicitly by every
        // `apply` call above via `check_k_invariants`.
        let t3 = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: Some(child_b.fiber_id) },
            &context,
        )
        .unwrap();
        assert!(t3.fibers_delete().is_empty());
        let after_t3 = materialize_snapshot(after_t2.state(), &t3, workflow.envelope().abi_version(), 3);
        let snapshot = Snapshot::new(
            after_t3.state().instance().clone(),
            after_t3.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t3.state().concurrency_table().clone());

        // Tick 4: child_c — the SKIP branch — evaluates PushBool(true) then
        // BrIf straight to the SAME V2Join, never touching address 7's
        // "real work". It is the 3rd and last arrival: sole survivor,
        // continues past the join and falls straight into End in the same
        // transition. All three fibres end up deleted (the two parked
        // non-last-arrivals cancelled by the join retiring, child_c itself
        // deleted by End) — the barrier retires via the UNMODIFIED V2Join
        // mechanism, proving the skip-to-join branch is indistinguishable,
        // from the barrier's point of view, from a "real work" branch.
        let t4 = apply(
            &workflow,
            &snapshot,
            &Command::Tick { fiber_id: Some(child_c.fiber_id) },
            &context,
        )
        .unwrap();
        assert_eq!(
            std::collections::BTreeSet::from_iter(t4.fibers_delete().iter().copied()),
            std::collections::BTreeSet::from([child_a.fiber_id, child_b.fiber_id, child_c.fiber_id]),
            "the skip branch's arrival must retire the barrier exactly like a real-work \
             arrival would — all 3 members deleted at retirement, none left dangling"
        );
        assert!(t4.fibers_upsert().is_empty());
        assert!(
            matches!(t4.next_snapshot().state, ProcessState::Completed { .. }),
            "expected Completed, got {:?} — the skip-to-join branch must complete the \
             barrier and the instance exactly like an all-real-work fork would",
            t4.next_snapshot().state
        );
    }

    // V5.3 (§18, landed 2026-07-23): `ring3_rejects_zero_live_fibres_with_
    // non_terminal_state` is deleted along with v1 `Instr::Join` — checked
    // before deleting, not assumed harmless: this test's pathological
    // vehicle depended specifically on v1 Join's own design flaw (its
    // non-last-arrival branch unconditionally deletes the arriving fibre
    // with NOTHING parked in its place — no fibre at all represents
    // "waiting at the join," only the static `join_plan` counter does).
    // `Instr::V2Join`'s non-last-arrival branch (`bpmn-lite-kernel/src/
    // lib.rs`, `Instr::V2Join` match arm) cannot produce this shape by
    // construction: on every non-last arrival it explicitly parks the
    // real fibre via `fiber.wait = WaitState::V2Barrier { record_id:
    // handle }` and pushes it to `fibers_upsert` before returning — there
    // is no v1-Join-shaped "arrival that deletes without parking" path
    // through V2Join's own honest code to hand-assemble a v2 fixture
    // from. The Ring 3 hazard class this test targeted (zero live fibres
    // post-transition, instance state stuck non-terminal) is not
    // v1-specific and remains tested elsewhere without needing this
    // vehicle: the `Instr::End` stuck-instance fix this test's own doc
    // comment calls a "companion" to (V4's `apply_fiber_deltas`
    // live-after-set fix, still in place and exercised by its own
    // fixtures) and the `v2fork_mixed_real_work_and_skip_to_join_
    // branches_retires_barrier_via_unmodified_mechanism` family, which
    // drives `check_k_invariants` (Ring 3's own enforcement point) on
    // every tick of a real `V2Fork`/`V2Join` program.

    /// Part 2 investigation (Adam's barrier-starvation hypothesis,
    /// 2026-07-22), **rescoped for A18 (2026-07-22)**: the original form of
    /// this test constructed a `FORK` into 3 branches, one of which sat
    /// inside an interrupting `V2Guard` that failed and automatically
    /// rolled back — silently dropping a fork member without decrementing
    /// the barrier's `counters.count`, permanently starving it (the defect
    /// Ring 3's barrier-satisfiability check below exists to reject).
    ///
    /// A18 makes that exact fixture unreconstructible as a *rollback*
    /// repro: automatic rollback-on-definitive-failure now only fires for
    /// a `GUARD-R>`-opened scope, and V-10 statically FORBIDS `GUARD-R>`
    /// from ever being opened while nested inside a `FORK`'s child fibre
    /// (`v2_verifier_v10_rejects_guard_r_nested_inside_a_fork_branch`,
    /// below, proves the identical program shape — `V2GuardR` in place of
    /// `V2Guard` here — is rejected at verify time). The historical
    /// starvation hazard is therefore now unreachable via a rollback-
    /// capable guard **by construction**, not merely caught downstream —
    /// a strictly stronger guarantee than what this test originally
    /// proved.
    ///
    /// This test is retargeted, not deleted, to prove the corresponding
    /// new fact: with the guard left as plain `V2Guard` (still legal
    /// nested inside a fork branch — V-10 only restricts `GUARD-R>`), the
    /// same definitive job failure no longer triggers ANY rollback at
    /// all — `V2Guard` is control-only (A18) and the innermost-guard
    /// search in `apply_job_failure` requires a rollback snapshot to
    /// engage its special-case path. The failure falls straight through
    /// to the ordinary v1 incident path: branch_a parks on an `Incident`
    /// (not deleted), the barrier is completely untouched
    /// (`counters.count` still 1, membership still all three branches),
    /// and no starvation is possible because no member ever silently
    /// disappears.
    #[test]
    fn fork_join_barrier_is_untouched_when_a_member_incidents_because_plain_guard_no_longer_auto_rolls_back(
    ) {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [16u8; 32],
            program: vec![
                /*  0 */ Instr::V2Fork {
                    targets: Box::new([Addr::new(1), Addr::new(6), Addr::new(8)]),
                    pairing: Addr::new(0),
                },
                /*  1 */ Instr::V2Guard { handler: Addr::new(10) },
                /*  2 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                /*  3 */ Instr::V2GuardEnd,
                /*  4 */ Instr::V2Join { pairing: Addr::new(0) },
                /*  5 */ Instr::End,
                /*  6 */ Instr::V2Join { pairing: Addr::new(0) },
                /*  7 */ Instr::End,
                /*  8 */ Instr::V2Join { pairing: Addr::new(0) },
                /*  9 */ Instr::End,
                // Guard handler (unused at runtime — this test never fires
                // `V2TriggerGuard`) — still runs nested inside the fork's
                // barrier scope per the interrupting-guard edge's abstract
                // state, so it must close that scope structurally (V-1)
                // rather than `End` directly.
                /* 10 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 11 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["SomeTask".to_string()],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v4-barrier-starvation-repro").unwrap(),
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

        // Tick 1: V2Fork spawns branch_a (pc=1, guarded), branch_b (pc=6),
        // branch_c (pc=8). Root dies.
        let context1 = DeterministicContext::new(300, Uuid::from_u128(301), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let branch_a = t1.fibers_upsert()[0].clone();
        let branch_b = t1.fibers_upsert()[1].clone();
        let branch_c = t1.fibers_upsert()[2].clone();
        let barrier_handle = branch_b.control_stack[0];
        let mut state = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);
        let mut tick_no = 1u64;

        let mut apply_and_materialize = |fiber_id: Uuid, state: &PersistedSnapshotState| {
            tick_no += 1;
            let snap = Snapshot::new(state.instance().clone(), state.fibers().values().cloned())
                .with_concurrency_table(state.concurrency_table().clone());
            let ctx = DeterministicContext::new(300 + tick_no, Uuid::from_u128((300 + tick_no).into()), 1);
            let t = apply(&workflow, &snap, &Command::Tick { fiber_id: Some(fiber_id) }, &ctx).unwrap();
            let materialized = materialize_snapshot(state, &t, workflow.envelope().abi_version(), tick_no);
            (t, materialized)
        };

        // Tick 2: branch_a enters V2Guard, parks its ExecNative on a job.
        let (t2, after_t2) = apply_and_materialize(branch_a.fiber_id, state.state());
        let job_key = t2.jobs_enqueue()[0].job_key.clone();
        state = after_t2;

        // Tick 3: branch_b arrives at V2Join (count 3 -> 2), non-last, parks.
        let (_, after_t3) = apply_and_materialize(branch_b.fiber_id, state.state());
        state = after_t3;

        // Tick 4: branch_c arrives at V2Join (count 2 -> 1), non-last, parks.
        let (_, after_t4) = apply_and_materialize(branch_c.fiber_id, state.state());
        state = after_t4;

        assert_eq!(
            state.state().concurrency_table().get(barrier_handle).unwrap().counters.count,
            1,
            "sanity: two of three arrivals in, one outstanding"
        );

        // branch_a's job fails definitively, inside a plain (non-rollback)
        // interrupting `V2Guard`. A18: this no longer matches
        // `apply_job_failure`'s innermost-guard rollback special-case at
        // all (no rollback snapshot on the record) — it falls straight
        // through to the ordinary v1 incident path.
        let snapshot_for_fail = Snapshot::new(state.state().instance().clone(), state.state().fibers().values().cloned())
            .with_concurrency_table(state.state().concurrency_table().clone());
        let context5 = DeterministicContext::new(999, Uuid::from_u128(999), 1);
        let fail_command = Command::EffectFailed {
            effect_id: EffectId::for_instruction(Uuid::nil(), Uuid::nil(), 0),
            job_key,
            error_class: ErrorClass::ContractViolation,
            message: "boom".to_string(),
            retry: None,
            attempt: 1,
        };
        let result = apply(&workflow, &snapshot_for_fail, &fail_command, &context5).unwrap();
        assert_eq!(
            result.incidents().len(),
            1,
            "no rollback special-case matches a plain V2Guard any more — this is the ordinary v1 incident path"
        );
        assert!(
            result.fibers_delete().is_empty(),
            "the incident path parks the failing fibre, it does not kill it — branch_a is not silently dropped"
        );
        assert!(
            result.concurrency_mutations().is_empty(),
            "no guard scope was retired and no rollback ran: the barrier record is completely untouched"
        );
        let after_t5 = materialize_snapshot(state.state(), &result, workflow.envelope().abi_version(), tick_no + 1);
        let barrier = after_t5.state().concurrency_table().get(barrier_handle).unwrap();
        assert_eq!(
            barrier.counters.count, 1,
            "barrier arrival count is exactly as it was before the failure — no member silently disappeared"
        );
        assert_eq!(
            barrier.members.len(),
            3,
            "all three branches are still counted as members — branch_a parked on an Incident, not removed"
        );
    }

    /// A18 V-10 positive proof: the exact program shape the historical
    /// barrier-starvation reproduction above used to construct —
    /// `GUARD-R>` nested inside a `FORK`'s child branch — is now rejected
    /// at VERIFY time, not merely detected at runtime by Ring 3. This is
    /// the receipt that the starvation hazard is unreachable via a
    /// rollback-capable guard by construction.
    #[test]
    fn v2_verifier_v10_rejects_guard_r_nested_inside_a_fork_branch() {
        let program = vec![
            /*  0 */ Instr::V2Fork {
                targets: Box::new([Addr::new(1), Addr::new(6), Addr::new(8)]),
                pairing: Addr::new(0),
            },
            /*  1 */ Instr::V2GuardR,
            /*  2 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
            /*  3 */ Instr::V2GuardREnd,
            /*  4 */ Instr::V2Join { pairing: Addr::new(0) },
            /*  5 */ Instr::End,
            /*  6 */ Instr::V2Join { pairing: Addr::new(0) },
            /*  7 */ Instr::End,
            /*  8 */ Instr::V2Join { pairing: Addr::new(0) },
            /*  9 */ Instr::End,
        ];
        let legacy = bpmn_lite_types::legacy_program! {
            bytecode_version: [19u8; 32],
            program: program,
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["SomeTask".to_string()],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let result = ArtifactEnvelope::from_legacy_program(legacy, "v10-guard-r-nested-in-fork");
        let err = result.expect_err("GUARD-R> nested inside a FORK branch must be rejected");
        let message = format!("{err:?}");
        assert!(
            message.contains("V-10"),
            "expected a V-10 dominance violation, got {message}"
        );
    }

    /// V4.2 — K-1..K-3 property tests. Random sequences of `Tick`/
    /// `V2TriggerGuard` commands are fired at the oracle-shaped program
    /// (the same 18-instruction Guard+Fork+Join+Race+handler fixture as
    /// `v2_trigger_guard_reproduces_oracle_cancellation_cascade`), and
    /// `check_k_invariants` is asserted after every command that the
    /// kernel accepts. A K-violation surfacing here is a kernel defect by
    /// definition (V&S §7's discharge protocol).
    ///
    /// Scope, stated rather than silently capped: this generator only
    /// drives `Tick`/`V2TriggerGuard` — it never resolves the fork's
    /// `V2WaitFor` timer or the race's timer/message arms externally
    /// (`TimerFired`/`MessageDelivered` are not in the action set). Fibres
    /// that park on those effects simply stop progressing for the rest of
    /// that run; `V2TriggerGuard` still reaches and cancels them (it does
    /// not require its members to be `Running`), so the interrupting-guard
    /// unwind path — the one most likely to desynchronize control stack
    /// from membership — is still exercised under random timing. Full
    /// external-event coverage is V4.5's golden-transition fixtures.
    /// §18 v0.10 ruling I: a `GUARD-TIMER>`-armed `V2Guard` firing via
    /// `Command::TimerFired` reproduces the exact same cascade a manual
    /// `Command::V2TriggerGuard` produces — this is the whole point of the
    /// ruling ("the arming trigger is what issues it"), so this is the
    /// single fixture that must hold for the ruling to actually be true,
    /// not just documented. Deliberately simpler than
    /// `v2_trigger_guard_reproduces_oracle_cancellation_cascade` (single
    /// fibre, no fork/race nesting) — that test already proves the
    /// cascade mechanics themselves against `Command::V2TriggerGuard`
    /// directly; this one's only job is proving the NEW wiring (arm ->
    /// schedule -> `TimerFired` -> the same mechanism) is correct, not
    /// re-proving the cascade itself.
    #[test]
    fn v2_guard_timer_trigger_fires_the_same_cascade_as_manual_v2_trigger_guard() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [21u8; 32],
            program: vec![
                /* 0 */ Instr::PushI64(5_000),
                /* 1 */ Instr::V2Guard { handler: Addr::new(7) },
                /* 2 */ Instr::V2GuardArmTimer,
                /* 3 */ Instr::PushI64(999_000),
                /* 4 */ Instr::V2WaitFor,
                /* 5 */ Instr::V2GuardEnd,
                /* 6 */ Instr::End,
                /* 7 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                /* 8 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["NotifyGuardTimedOut".to_string()],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "ruling-i-guard-timer-cascade").unwrap(),
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

        // Tick 1: PushI64 -> V2Guard (opens the record) -> V2GuardArmTimer
        // (pops 5_000, schedules a ScheduleTimer effect bound to the new
        // record, TimerKind::V2GuardTimer — the actual new behaviour under
        // test) -> PushI64 -> V2WaitFor parks (its own, unrelated, much
        // longer timer — never fires in this test).
        let context1 = DeterministicContext::new(500, Uuid::from_u128(501), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let guard_handle = t1.fibers_upsert()[0].control_stack[0];
        // Two effects: GUARD-TIMER>'s own (the new behaviour under test)
        // and V2WaitFor's unrelated, much-longer one — both words emit a
        // durable timer effect, and GUARD-TIMER> does not suppress
        // V2WaitFor's (arming is not parking).
        assert_eq!(t1.effects().len(), 2);
        let guard_timer_due_at = t1
            .effects()
            .iter()
            .find_map(|effect| match effect {
                bpmn_lite_types::DurableEffect::ScheduleTimer {
                    due_at,
                    kind: TimerKind::V2GuardTimer { record_id },
                    ..
                } if *record_id == guard_handle => Some(*due_at),
                _ => None,
            })
            .expect("GUARD-TIMER> must schedule a ScheduleTimer effect bound to the guard record");
        // due_at = context1's logical_time (500) + the popped duration (5_000).
        assert_eq!(guard_timer_due_at, 5_500);
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);

        // Fire the guard's own timer (NOT a hand-crafted V2TriggerGuard) —
        // this is the mechanism under test.
        let context2 = DeterministicContext::new(502, Uuid::from_u128(503), 2);
        let claimed_timer = bpmn_lite_types::ClaimedTimer::new(
            bpmn_lite_types::ClaimedTimerIdentity::new(
                bpmn_lite_types::TenantId::new("tenant-a").unwrap(),
                EffectId::for_instruction(root_fiber_id, root_fiber_id, 2),
                after_t1.state().instance().instance_id,
                root_fiber_id,
            ),
            5_000,
            TimerKind::V2GuardTimer { record_id: guard_handle },
            None,
            Uuid::nil(),
        );
        let snapshot2 = Snapshot::new(
            after_t1.state().instance().clone(),
            after_t1.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t1.state().concurrency_table().clone());
        let t2 = apply(
            &workflow,
            &snapshot2,
            &Command::TimerFired { timer: claimed_timer, fired_at: 5_000 },
            &context2,
        )
        .unwrap();

        // Exactly the interrupting-`V2Guard` cascade
        // `v2_trigger_guard_reproduces_oracle_cancellation_cascade` proves
        // against a manual `Command::V2TriggerGuard`: the sole member is
        // cancelled, the guard record retires, the handler fibre spawns
        // (pre-push entry state — empty control stack, since this guard
        // had no enclosing scope).
        assert_eq!(t2.fibers_delete(), &[root_fiber_id]);
        assert_eq!(t2.fibers_upsert().len(), 1);
        let handler_fiber = &t2.fibers_upsert()[0];
        assert_eq!(handler_fiber.pc, Addr::new(7));
        assert!(handler_fiber.control_stack.is_empty());
        assert_eq!(t2.concurrency_mutations().len(), 1);
        assert!(matches!(
            &t2.concurrency_mutations()[0],
            ConcurrencyMutation::Retire(id) if *id == guard_handle
        ));
        assert!(t2.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::TimerFired { fiber_id, fired_at: 5_000, .. } if *fiber_id == root_fiber_id
        )));
        assert!(t2.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::V2GuardTriggered { record_id, .. } if *record_id == guard_handle
        )));
        assert_eq!(
            t2.timer_mutations(),
            &[TimerMutation::Consume {
                timer_id: EffectId::for_instruction(root_fiber_id, root_fiber_id, 2),
                claim_token: Uuid::nil(),
            }]
        );
    }

    /// A guard's trigger is OPTIONAL, not mandatory — a `V2Guard` opened
    /// without a following `V2GuardArmTimer` must schedule no durable
    /// effect at all and behave exactly as every pre-ruling-I guard
    /// fixture already proves (cement-locked, unchanged): this is the
    /// explicit no-regression receipt the task asks for, not merely an
    /// inference from "the existing tests still pass."
    #[test]
    fn v2_guard_opened_without_a_trigger_schedules_no_timer_effect() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [22u8; 32],
            program: vec![
                /* 0 */ Instr::V2Guard { handler: Addr::new(4) },
                /* 1 */ Instr::PushI64(999_000),
                /* 2 */ Instr::V2WaitFor,
                /* 3 */ Instr::V2GuardEnd,
                /* 4 */ Instr::End,
                /* 5 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "ruling-i-guard-no-trigger").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        let snapshot = Snapshot::new(base_snapshot.instance().clone(), [Fiber::new(root_fiber_id, 0)]);
        let context = DeterministicContext::new(510, Uuid::from_u128(511), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context).unwrap();
        // Exactly one effect (V2WaitFor's own) — none bound to the guard
        // record, since no GUARD-TIMER> ever ran.
        assert_eq!(t1.effects().len(), 1);
        assert!(
            !t1.effects().iter().any(|effect| matches!(
                effect,
                bpmn_lite_types::DurableEffect::ScheduleTimer {
                    kind: TimerKind::V2GuardTimer { .. },
                    ..
                }
            )),
            "an unarmed guard must schedule no V2GuardTimer effect at all"
        );
        assert!(matches!(
            t1.fibers_upsert()[0].wait,
            // deadline_ms = context's logical_time (510) + the popped duration (999_000).
            WaitState::Timer { deadline_ms: 999_510 }
        ));
        assert_eq!(t1.fibers_upsert()[0].control_stack.len(), 1, "the guard still opened normally");
    }

    /// §18 v0.10 ruling I / V-10 interaction (reported, not silently
    /// assumed benign — see `apply_v2_guard_timer_rollback`'s own doc
    /// comment for the full reasoning): `V2GuardR` carries no `handler`
    /// (A18), so a timer-armed `V2GuardR` firing cannot go through the
    /// same `Command::V2TriggerGuard`-equivalent path the plain-guard test
    /// above does — it must run the automatic-rollback path instead, with
    /// no single failing job to anchor `RollbackCaller::Dies` against.
    /// This fixture exercises the case worth a dedicated test: a
    /// `V2GuardR` whose scope spans the instance's entire live fibre set
    /// (a single-fibre workflow, the routine shape per §13's amendment) —
    /// so firing the deadline must NOT simply kill the fibre and leave the
    /// instance with zero live fibres; it must restore the A3 rollback set
    /// AND park the fibre on an incident, exactly as
    /// `definitive_job_failure_inside_interrupting_guard_and_spanning_the_whole_instance_parks_on_incident`
    /// already proves for a job-failure-triggered rollback.
    #[test]
    fn v2_guard_r_timer_trigger_rolls_back_and_parks_on_incident_when_spanning_the_whole_instance() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [23u8; 32],
            program: vec![
                /* 0 */ Instr::PushI64(9_000),
                /* 1 */ Instr::V2GuardR,
                /* 2 */ Instr::V2GuardArmTimer,
                /* 3 */ Instr::PushI64(999_000),
                /* 4 */ Instr::V2WaitFor,
                /* 5 */ Instr::V2GuardREnd,
                /* 6 */ Instr::End,
            ],
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
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "ruling-i-guard-r-timer-spanning").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;

        let flag_key: bpmn_lite_types::FlagKey = 9;
        let mut base_instance = base_snapshot.instance().clone();
        base_instance.flags.insert(flag_key, Value::Bool(false));
        let original_payload = base_instance.domain_payload.to_string();
        let original_flags = base_instance.flags.clone();
        let snapshot = Snapshot::new(base_instance, [Fiber::new(root_fiber_id, 0)]);

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

        // Tick 1: PushI64 -> V2GuardR (captures the A3 snapshot) ->
        // V2GuardArmTimer (schedules the deadline) -> PushI64 -> V2WaitFor
        // parks on its own, unrelated, much longer timer.
        let context1 = DeterministicContext::new(520, Uuid::from_u128(521), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        let guard_handle = t1.fibers_upsert()[0].control_stack[0];
        // GUARD-TIMER>'s own effect plus V2WaitFor's unrelated one.
        assert_eq!(t1.effects().len(), 2);
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);

        // Mutate business state while the fibre sits parked inside the
        // scope — this is what rollback must undo.
        let mut resumed_instance = after_t1.state().instance().clone();
        resumed_instance.domain_payload = "mutated-while-parked".to_string().into();
        resumed_instance.domain_payload_hash = EffectId::content_hash(b"mutated-while-parked");
        resumed_instance.flags.insert(flag_key, Value::Bool(true));
        let snapshot2 = Snapshot::new(resumed_instance, after_t1.state().fibers().values().cloned())
            .with_concurrency_table(after_t1.state().concurrency_table().clone());

        // Fire the GUARD-R>'s own deadline.
        let context2 = DeterministicContext::new(522, Uuid::from_u128(523), 2);
        let claimed_timer = bpmn_lite_types::ClaimedTimer::new(
            bpmn_lite_types::ClaimedTimerIdentity::new(
                bpmn_lite_types::TenantId::new("tenant-a").unwrap(),
                EffectId::for_instruction(root_fiber_id, root_fiber_id, 2),
                snapshot2.instance().instance_id,
                root_fiber_id,
            ),
            9_000,
            TimerKind::V2GuardTimer { record_id: guard_handle },
            None,
            Uuid::nil(),
        );
        let t2 = apply(
            &workflow,
            &snapshot2,
            &Command::TimerFired { timer: claimed_timer, fired_at: 9_000 },
            &context2,
        )
        .unwrap();

        assert_eq!(
            t2.next_snapshot().domain_payload.to_string(),
            original_payload,
            "A3: domain_payload must be restored by the timer-triggered rollback"
        );
        assert_eq!(
            t2.next_snapshot().flags, original_flags,
            "A3: business flags must be restored by the timer-triggered rollback"
        );
        assert!(
            t2.fibers_delete().is_empty(),
            "spanning case: the sole live fibre must be restored, not killed \
             (killing it would leave zero live fibres and no way to resume)"
        );
        assert_eq!(t2.fibers_upsert().len(), 1);
        let parked = &t2.fibers_upsert()[0];
        assert_eq!(
            parked.pc,
            Addr::new(1),
            "resume address is GUARD-R>'s own opening word (opened_at), not wherever \
             the deadline caught the scope mid-execution"
        );
        let incident_id = match parked.wait {
            WaitState::Incident { incident_id } => incident_id,
            ref other => panic!("expected WaitState::Incident, got {other:?}"),
        };
        assert_eq!(
            t2.next_snapshot().state,
            ProcessState::Incidented { incident_id },
            "spanning case sets the same state the ordinary incident path does"
        );
        assert_eq!(t2.incidents().len(), 1);
        assert_eq!(t2.incidents()[0].incident_id, incident_id);
        assert_eq!(t2.incidents()[0].error_class, ErrorClass::ContractViolation);
        assert_eq!(t2.concurrency_mutations().len(), 1);
        assert!(matches!(
            &t2.concurrency_mutations()[0],
            ConcurrencyMutation::Retire(id) if *id == guard_handle
        ));
        assert!(t2.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::IncidentCreated { incident_id: id, .. } if *id == incident_id
        )));
    }

    mod k_invariant_properties {
        use super::*;
        use proptest::prelude::*;

        fn build_workflow(bytecode_byte: u8, label: &str, program: Vec<Instr>) -> ExecutableWorkflow {
            // Every message word needs a correlation source; a Bool(false)
            // literal (content key "false") is the deterministic default for
            // these hand-assembled property fixtures.
            let corr_sources = corr_false(
                &program
                    .iter()
                    .enumerate()
                    .filter(|(_, instr)| {
                        matches!(
                            instr,
                            Instr::V2WaitMsg { .. }
                                | Instr::V2ArmMsg { .. }
                                | Instr::PublishMessage { .. }
                        )
                    })
                    .map(|(addr, _)| addr as u32)
                    .collect::<Vec<_>>(),
            );
            let program = bpmn_lite_types::legacy_program! {
                bytecode_version: [bytecode_byte; 32],
                program: program,
                debug_map: BTreeMap::new(),
                join_plan: BTreeMap::new(),
                wait_plan: BTreeMap::new(),
                message_name_map: BTreeMap::new(),
                write_set: BTreeMap::new(),
                task_manifest: vec!["NotifyCancelled".to_string()],
                flag_symbol_table: BTreeMap::new(),
                data_objects: BTreeMap::new(),
                ffi_task_decls: BTreeMap::new(),
            }
            .with_v2_corr_sources(corr_sources);
            ExecutableWorkflow::from_verified_envelope(
                ArtifactEnvelope::from_legacy_program(program, label).unwrap(),
            )
            .unwrap()
        }

        /// Topology 0 — the original V4.2 oracle-shaped fixture: one
        /// interrupting guard wrapping a barrier whose two branches are a
        /// plain wait and a race. Kept as the regression baseline.
        fn topology_guard_fork_race() -> ExecutableWorkflow {
            build_workflow(19, "v4.2-k-invariant-property", vec![
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
                /* 9  */ Instr::V2ArmMsg { target: Addr::new(12), name: 100 },
                /* 10 */ Instr::V2RaceClose,
                /* 11 */ Instr::Jump { target: Addr::new(13) },
                /* 12 */ Instr::Jump { target: Addr::new(13) },
                /* 13 */ Instr::V2Join { pairing: Addr::new(1) },
                /* 14 */ Instr::V2GuardEnd,
                /* 15 */ Instr::End,
                /* 16 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                /* 17 */ Instr::End,
            ])
        }

        /// Topology 1 — nested guards, interrupting OUTSIDE non-interrupting
        /// (`V4.6` blind-review remediation: this is the exact nesting order
        /// that exposed the `apply_job_failure` innermost-guard bug). Outer
        /// handler entry is pre-push (empty); inner `V2GuardN` handler entry
        /// is post-push and must close both its own token and the outer
        /// scope's to legally reach `End` — verified admissible the same
        /// way as `definitive_job_failure_under_non_interrupting_guard_nested_inside_interrupting_guard_still_incidents`.
        fn topology_nested_guard_interrupting_outer() -> ExecutableWorkflow {
            build_workflow(20, "v4.6-nested-guard-interrupting-outer", vec![
                /* 0  */ Instr::V2Guard { handler: Addr::new(7) },
                /* 1  */ Instr::V2GuardN { handler: Addr::new(8) },
                /* 2  */ Instr::PushI64(60_000),
                /* 3  */ Instr::V2WaitFor,
                /* 4  */ Instr::V2GuardNEnd,
                /* 5  */ Instr::V2GuardEnd,
                /* 6  */ Instr::End,
                /* 7  */ Instr::End,
                /* 8  */ Instr::V2GuardNEnd,
                /* 9  */ Instr::V2GuardEnd,
                /* 10 */ Instr::End,
            ])
        }

        /// Topology 2 — nested guards, the REVERSE order: non-interrupting
        /// outside interrupting. Outer `V2GuardN`'s handler entry is
        /// post-push (just its own token) and closes cleanly with one
        /// `V2GuardNEnd`. Inner `V2Guard`'s handler entry is pre-push (the
        /// outer's still-armed `GuardN` token) and closes it with one
        /// `V2GuardNEnd` (kind-matched) before `End`.
        fn topology_nested_guard_noninterrupting_outer() -> ExecutableWorkflow {
            build_workflow(21, "v4.6-nested-guard-noninterrupting-outer", vec![
                /* 0  */ Instr::V2GuardN { handler: Addr::new(7) },
                /* 1  */ Instr::V2Guard { handler: Addr::new(9) },
                /* 2  */ Instr::PushI64(60_000),
                /* 3  */ Instr::V2WaitFor,
                /* 4  */ Instr::V2GuardEnd,
                /* 5  */ Instr::V2GuardNEnd,
                /* 6  */ Instr::End,
                /* 7  */ Instr::V2GuardNEnd,
                /* 8  */ Instr::End,
                /* 9  */ Instr::V2GuardNEnd,
                /* 10 */ Instr::End,
            ])
        }

        /// Topology 3 — a race inside a guard inside a barrier (the reverse
        /// nesting from topology 0, which has the guard outermost). Fork
        /// opens the barrier first; one branch opens an interrupting guard
        /// wrapping a race, the other is a plain wait. The guard's handler
        /// inherits the ambient barrier handle (V-4 pre-push, `apply_v2_trigger_guard`'s
        /// `handler_stack` construction) and legally closes it with its own
        /// `V2Join` — the handler fibre is registered as a member of that
        /// barrier via V4.1's ancestor-membership reconciliation, so this is
        /// real, not just verifier-satisfying dead code.
        fn topology_race_inside_guard_inside_barrier() -> ExecutableWorkflow {
            build_workflow(22, "v4.6-race-inside-guard-inside-barrier", vec![
                /* 0  */ Instr::V2Fork {
                    targets: Box::new([Addr::new(1), Addr::new(12)]),
                    pairing: Addr::new(0),
                },
                /* 1  */ Instr::V2Guard { handler: Addr::new(16) },
                /* 2  */ Instr::V2RaceOpen { arm_count: 2 },
                /* 3  */ Instr::PushI64(30_000),
                /* 4  */ Instr::V2ArmTimer { target: Addr::new(7) },
                /* 5  */ Instr::V2ArmMsg { target: Addr::new(8), name: 100 },
                /* 6  */ Instr::V2RaceClose,
                /* 7  */ Instr::Jump { target: Addr::new(9) },
                /* 8  */ Instr::Jump { target: Addr::new(9) },
                /* 9  */ Instr::V2GuardEnd,
                /* 10 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 11 */ Instr::Jump { target: Addr::new(18) },
                /* 12 */ Instr::PushI64(60_000),
                /* 13 */ Instr::V2WaitFor,
                /* 14 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 15 */ Instr::Jump { target: Addr::new(18) },
                /* 16 */ Instr::V2Join { pairing: Addr::new(0) },
                /* 17 */ Instr::End,
                /* 18 */ Instr::End,
            ])
        }

        /// Topology 4 — re-entrant `FORK`/`JOIN` in a bounded loop (same
        /// legal shape as `v2_verifier::tests::reentrant_fork_join_in_bounded_loop_is_admitted`):
        /// the same static Fork/Join addresses are revisited up to 3 times,
        /// stressing whether K-1/K-2/K-3 survive re-entry, not just a single
        /// pass. No guards — pure barrier re-entry under random Tick
        /// interleaving.
        fn topology_reentrant_fork_join() -> ExecutableWorkflow {
            build_workflow(23, "v4.6-reentrant-fork-join", vec![
                /* 0 */ Instr::IncCounter { counter_id: 0 },
                /* 1 */ Instr::V2Fork {
                    targets: Box::new([Addr::new(3), Addr::new(5)]),
                    pairing: Addr::new(1),
                },
                /* 2 */ Instr::V2CancelScope, // dead filler, matches V2.7's fixture shape
                /* 3 */ Instr::V2Join { pairing: Addr::new(1) },
                /* 4 */ Instr::Jump { target: Addr::new(7) },
                /* 5 */ Instr::V2Join { pairing: Addr::new(1) },
                /* 6 */ Instr::Jump { target: Addr::new(7) },
                /* 7 */ Instr::BrCounterLt { counter_id: 0, limit: 3, target: Addr::new(0) },
                /* 8 */ Instr::End,
            ])
        }

        fn topology_for(selector: u8) -> ExecutableWorkflow {
            match selector % 5 {
                0 => topology_guard_fork_race(),
                1 => topology_nested_guard_interrupting_outer(),
                2 => topology_nested_guard_noninterrupting_outer(),
                3 => topology_race_inside_guard_inside_barrier(),
                _ => topology_reentrant_fork_join(),
            }
        }

        /// Shared driver: fires a random `Tick`/`V2TriggerGuard` sequence
        /// against `workflow` from a single root fibre, asserting
        /// `check_k_invariants` after every accepted `apply`. Widened past
        /// V4.2's original scope in two ways (V4.6 blind-review remediation
        /// item 3): the guard-trigger filter now includes non-interrupting
        /// `V2GuardN` records too (not just interrupting `V2Guard`), and
        /// it's called once per topology in `topology_for` rather than
        /// against one fixed program — a single-topology corpus proves
        /// nothing about a bug that's invisible except under a specific
        /// nesting shape, which is exactly how the `apply_job_failure`
        /// finding above escaped detection.
        fn run_k_invariant_fuzz(workflow: &ExecutableWorkflow, steps: Vec<(u8, u8)>) {
            let (_, base_snapshot, _) = fixture();
            let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
            let mut state = PersistedSnapshotState::new(
                base_snapshot.instance().clone(),
                [Fiber::new(root_fiber_id, 0)],
                BTreeMap::new(),
                [],
                bpmn_lite_types::concurrency::ConcurrencyTable::new(),
                [],
            );
            let mut revision = 0u64;

            for (step_index, (action_sel, index_sel)) in steps.into_iter().enumerate() {
                let snapshot = Snapshot::new(
                    state.instance().clone(),
                    state.fibers().values().cloned(),
                )
                .with_concurrency_table(state.concurrency_table().clone());

                let running_fibers: Vec<Uuid> = state
                    .fibers()
                    .values()
                    .filter(|f| f.wait == WaitState::Running)
                    .map(|f| f.fiber_id)
                    .collect();
                let armed_guards: Vec<RecordId> = state
                    .concurrency_table()
                    .iter()
                    .filter(|(_, record)| {
                        record.state == RecordState::Armed
                            && matches!(record.kind, RecordKind::Guard { .. })
                    })
                    .map(|(id, _)| *id)
                    .collect();

                let command = if action_sel % 2 == 0 && !running_fibers.is_empty() {
                    let idx = index_sel as usize % running_fibers.len();
                    Command::Tick { fiber_id: Some(running_fibers[idx]) }
                } else if !armed_guards.is_empty() {
                    let idx = index_sel as usize % armed_guards.len();
                    Command::V2TriggerGuard { record_id: armed_guards[idx] }
                } else if !running_fibers.is_empty() {
                    Command::Tick { fiber_id: Some(running_fibers[0]) }
                } else {
                    continue;
                };

                let context = DeterministicContext::new(
                    1_000,
                    Uuid::from_u128(9_000_000 + step_index as u128),
                    revision + 1,
                );
                let Ok(transition) = apply(workflow, &snapshot, &command, &context) else {
                    continue;
                };
                let next = materialize_snapshot(
                    &state,
                    &transition,
                    workflow.envelope().abi_version(),
                    revision + 1,
                );
                revision += 1;
                state = next.state().clone();
                assert!(
                    check_k_invariants(state.fibers(), state.concurrency_table()).is_ok(),
                    "{:?}",
                    check_k_invariants(state.fibers(), state.concurrency_table())
                );
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(200))]
            #[test]
            fn k_invariants_hold_after_random_tick_and_trigger_sequences(
                topology_sel in 0u8..5,
                steps in proptest::collection::vec((0u8..3, 0u8..8), 0..15)
            ) {
                let workflow = topology_for(topology_sel);
                run_k_invariant_fuzz(&workflow, steps);
            }
        }

        /// Deterministic companion to the property test above: proptest's
        /// `topology_sel in 0u8..5` picks topologies randomly, so a run
        /// could in principle exercise fewer than all 5 shapes. This test
        /// pins a long, fixed step sequence against every topology
        /// constructor directly — proving each of the 5 (including the two
        /// new nested-guard orderings, the barrier-wrapped race, and the
        /// re-entrant fork/join) is independently admissible and
        /// K-invariant-clean, not just "probably got hit" by the fuzzer.
        #[test]
        fn k_invariants_hold_across_every_topology_deterministically() {
            let steps: Vec<(u8, u8)> = (0u8..30).map(|i| (i % 3, i.wrapping_mul(7) % 8)).collect();
            for selector in 0u8..5 {
                let workflow = topology_for(selector);
                run_k_invariant_fuzz(&workflow, steps.clone());
            }
        }
    }

    /// §18 ruling K: "an interrupting boundary event over the whole MI
    /// construct is simply `V2Guard`/`V2GuardR` wrapping the fork
    /// region — already fully covered by V-10 and the existing guard
    /// machinery, no new mechanism needed for that case; do not build
    /// anything extra for it, just confirm it composes correctly with a
    /// test." This is that test — zero new guard-side code, a `V2Guard`
    /// wrapping a 2-branch MI-shaped `V2Fork`/`V2Join` region built from
    /// this ruling's own new opcodes (`V2MiArityCheck`/`V2MiIndexLive`),
    /// with `Command::V2TriggerGuard` fired while both MI fibres are still
    /// live (parked on their own jobs, neither having reached `V2Join`):
    /// the SAME cascade `v2_guard_fork_join_reproduces_oracle_survivor_shape`
    /// and `v2_trigger_guard_reproduces_oracle_cancellation_cascade` already
    /// prove for a `V2Fork` built from plain `Jump`/`WaitFor`/`Race`
    /// content — both MI fibres cancelled, the barrier and guard both
    /// retire, the handler spawns. Nothing about the per-branch instruction
    /// content (index/skip checks vs. plain jumps) is visible to the
    /// guard/barrier machinery at all, which is the actual point being
    /// proven: composition requires no MI-specific guard code because the
    /// guard never inspects branch content in the first place.
    #[test]
    fn v2_guard_over_multi_instance_fork_composes_via_unmodified_guard_mechanism() {
        let program = bpmn_lite_types::legacy_program! {
            bytecode_version: [26u8; 32],
            program: vec![
                /*  0 */ Instr::V2Guard { handler: Addr::new(14) },
                /*  1 */ Instr::V2MiArityCheck { collection_flag: 0, max: 2 },
                /*  2 */ Instr::V2Fork {
                    targets: Box::new([Addr::new(3), Addr::new(7)]),
                    pairing: Addr::new(2),
                },
                /*  3 */ Instr::V2MiIndexLive { collection_flag: 0, index: 0 },
                /*  4 */ Instr::BrIfNot { target: Addr::new(11) },
                /*  5 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                /*  6 */ Instr::Jump { target: Addr::new(11) },
                /*  7 */ Instr::V2MiIndexLive { collection_flag: 0, index: 1 },
                /*  8 */ Instr::BrIfNot { target: Addr::new(11) },
                /*  9 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
                /* 10 */ Instr::Jump { target: Addr::new(11) },
                /* 11 */ Instr::V2Join { pairing: Addr::new(2) },
                /* 12 */ Instr::V2GuardEnd,
                /* 13 */ Instr::End,
                // Guard handler: V2Guard is opened BEFORE V2Fork (guard
                // wraps the whole MI region), so the handler's inherited
                // (pre-push) control stack is empty — no scope to close,
                // matching `v2_trigger_guard_reproduces_oracle_cancellation_cascade`'s
                // identically-nested handler shape (plain ExecNative+End,
                // no V2Join).
                /* 14 */ Instr::ExecNative { task_type: 1, argc: 0, retc: 0 },
                /* 15 */ Instr::End,
            ],
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec!["VerifyDoc".to_string(), "Escalate".to_string()],
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        };
        let workflow = ExecutableWorkflow::from_verified_envelope(
            ArtifactEnvelope::from_legacy_program(program, "v18-ruling-k-guard-over-mi").unwrap(),
        )
        .unwrap();
        let (_, base_snapshot, _context) = fixture();
        let root_fiber_id = base_snapshot.fibers().values().next().unwrap().fiber_id;
        // Both MI indices (0, 1) live: `collection_flag` (key 0) holds a
        // 2-element `Value::Array` (§18 ruling K Part 2 — length is derived
        // from the array itself, no separate `I64` length flag).
        let mut instance = base_snapshot.instance().clone();
        instance.flags.insert(0, Value::Array(vec![Value::I64(100), Value::I64(200)]));
        let snapshot = Snapshot::new(instance, [Fiber::new(root_fiber_id, 0)]);

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

        // Tick 1: V2Guard + V2MiArityCheck (0 <= 2, passes) + V2Fork run in
        // the same tick (none park) — spawns both MI fibres.
        let context1 = DeterministicContext::new(500, Uuid::from_u128(501), 1);
        let t1 = apply(&workflow, &snapshot, &Command::Tick { fiber_id: None }, &context1).unwrap();
        assert_eq!(t1.fibers_delete(), &[root_fiber_id]);
        assert_eq!(t1.fibers_upsert().len(), 2, "both declared_max=2 fibres spawn regardless of skip/live");
        let (fiber_0, fiber_1) = (t1.fibers_upsert()[0].clone(), t1.fibers_upsert()[1].clone());
        let guard_handle = fiber_0.control_stack[0];
        let barrier_handle = fiber_0.control_stack[1];
        let after_t1 = materialize_snapshot(genesis.state(), &t1, workflow.envelope().abi_version(), 1);

        // Tick 2: fiber_0 runs V2MiIndexLive (0 < 2, live) -> real ExecNative,
        // parks on a job. Neither branch reaches V2Join yet.
        let context2 = DeterministicContext::new(500, Uuid::from_u128(502), 2);
        let t2 = apply(
            &workflow,
            &Snapshot::new(after_t1.state().instance().clone(), after_t1.state().fibers().values().cloned())
                .with_concurrency_table(after_t1.state().concurrency_table().clone()),
            &Command::Tick { fiber_id: Some(fiber_0.fiber_id) },
            &context2,
        )
        .unwrap();
        assert!(t2.jobs_enqueue().iter().any(|j| j.job_key.contains(':')));
        let after_t2 = materialize_snapshot(after_t1.state(), &t2, workflow.envelope().abi_version(), 2);

        // Tick 3: fiber_1 runs V2MiIndexLive (1 < 2, live) -> real
        // ExecNative, parks on its own job too.
        let context3 = DeterministicContext::new(500, Uuid::from_u128(503), 3);
        let t3 = apply(
            &workflow,
            &Snapshot::new(after_t2.state().instance().clone(), after_t2.state().fibers().values().cloned())
                .with_concurrency_table(after_t2.state().concurrency_table().clone()),
            &Command::Tick { fiber_id: Some(fiber_1.fiber_id) },
            &context3,
        )
        .unwrap();
        let after_t3 = materialize_snapshot(after_t2.state(), &t3, workflow.envelope().abi_version(), 3);
        let snapshot_before_trigger = Snapshot::new(
            after_t3.state().instance().clone(),
            after_t3.state().fibers().values().cloned(),
        )
        .with_concurrency_table(after_t3.state().concurrency_table().clone());
        assert_eq!(
            snapshot_before_trigger.fibers().len(),
            2,
            "both MI fibres still live, parked on their own jobs, neither at V2Join"
        );

        // Trigger: neither MI fibre has reached V2Join — both are cancelled
        // by the SAME interrupting-guard cascade the oracle-shaped fixtures
        // above already prove, no MI-specific guard code involved.
        let context4 = DeterministicContext::new(500, Uuid::from_u128(504), 4);
        let trigger = apply(
            &workflow,
            &snapshot_before_trigger,
            &Command::V2TriggerGuard { record_id: guard_handle },
            &context4,
        )
        .unwrap();
        assert_eq!(
            trigger.fibers_delete().len(),
            2,
            "both MI fibres cancelled by the interrupting guard"
        );
        assert!(
            trigger.fibers_delete().contains(&fiber_0.fiber_id)
                && trigger.fibers_delete().contains(&fiber_1.fiber_id)
        );
        assert_eq!(trigger.fibers_upsert().len(), 1, "the handler fibre spawns");
        assert!(
            trigger.concurrency_mutations().iter().any(
                |m| matches!(m, bpmn_lite_types::concurrency::ConcurrencyMutation::Retire(id) if *id == barrier_handle)
            ),
            "the MI barrier retires as part of the cascade"
        );
        assert!(
            trigger.concurrency_mutations().iter().any(
                |m| matches!(m, bpmn_lite_types::concurrency::ConcurrencyMutation::Retire(id) if *id == guard_handle)
            ),
            "the guard itself retires"
        );

        let after_trigger = materialize_snapshot(after_t3.state(), &trigger, workflow.envelope().abi_version(), 4);
        assert_eq!(
            after_trigger.state().concurrency_table().get(barrier_handle).unwrap().state,
            RecordState::Retired,
            "the MI barrier record is retired — no starvation, no live-but-abandoned record"
        );
    }
}
