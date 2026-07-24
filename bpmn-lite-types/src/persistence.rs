use crate::canonical::CanonicalWriter;
use crate::concurrency::ConcurrencyTable;
use crate::{
    CanonicalEncode, Command, DurableEffect, EffectId, Fiber, Incident, JoinId, ProcessInstance,
    RuntimeEvent, Snapshot, StartCommand, Timestamp, Uuid,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SNAPSHOT_SCHEMA_VERSION: u16 = 1;
pub const COMMAND_SCHEMA_VERSION: u16 = 1;
pub const EVENT_SCHEMA_VERSION: u16 = 1;
pub const JOURNAL_SCHEMA_VERSION: u16 = 1;

/// Canonical durable state covered by the snapshot hash (D3 Ring 2, V&S §6:
/// `snapshot ‖ fibres by fibre-ID ‖ concurrency table ‖ pending-effect set`).
///
/// V2.1 (full supersession, 2026-07-21): every field here contributes to
/// `state_hash()` through the single tagged-binary `CanonicalEncode` module
/// (`bpmn_lite_types::canonical`) — `instance` via
/// `ProcessInstance::try_canonical_hash_bytes` (fallible: `domain_payload`/
/// `placeholder_values` are opaque externally-supplied JSON), everything
/// else via infallible `CanonicalEncode` impls. `serde_json` still backs
/// this struct's `Serialize`/`Deserialize` derive for the envelope's on-disk
/// storage format (`SnapshotEnvelope::canonical_bytes`/`decode`) — that is a
/// distinct, intentionally out-of-scope concern from the Ring 2 hash domain
/// (V&S §6 names the hash domain, not the storage envelope's wire format).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedSnapshotState {
    instance: ProcessInstance,
    fibers: BTreeMap<Uuid, Fiber>,
    join_counts: BTreeMap<JoinId, u16>,
    incidents: BTreeMap<Uuid, Incident>,
    concurrency_table: ConcurrencyTable,
    pending_effects: BTreeSet<EffectId>,
}

impl PersistedSnapshotState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mut instance: ProcessInstance,
        fibers: impl IntoIterator<Item = Fiber>,
        join_counts: BTreeMap<JoinId, u16>,
        incidents: impl IntoIterator<Item = Incident>,
        concurrency_table: ConcurrencyTable,
        pending_effects: impl IntoIterator<Item = EffectId>,
    ) -> Self {
        // The integrity hash is derived from state and therefore cannot be part
        // of the state it authenticates.
        instance.integrity_hash = None;
        Self {
            instance,
            fibers: fibers
                .into_iter()
                .map(|fiber| (fiber.fiber_id, fiber))
                .collect(),
            join_counts,
            incidents: incidents
                .into_iter()
                .map(|incident| (incident.incident_id, incident))
                .collect(),
            concurrency_table,
            pending_effects: pending_effects.into_iter().collect(),
        }
    }

    pub fn instance(&self) -> &ProcessInstance {
        &self.instance
    }

    pub fn fibers(&self) -> &BTreeMap<Uuid, Fiber> {
        &self.fibers
    }

    pub fn join_counts(&self) -> &BTreeMap<JoinId, u16> {
        &self.join_counts
    }

    pub fn incidents(&self) -> &BTreeMap<Uuid, Incident> {
        &self.incidents
    }

    pub fn concurrency_table(&self) -> &ConcurrencyTable {
        &self.concurrency_table
    }

    pub fn pending_effects(&self) -> &BTreeSet<EffectId> {
        &self.pending_effects
    }

    pub fn to_runtime_snapshot(&self) -> Snapshot {
        Snapshot::new(self.instance.clone(), self.fibers.values().cloned())
            .with_join_counts(self.join_counts.clone())
            .with_incidents(self.incidents.values().cloned())
            .with_concurrency_table(self.concurrency_table.clone())
            .with_pending_effects(self.pending_effects.iter().copied())
    }

    /// V2.1g: the `instance ‖ join_counts ‖ incidents ‖ fibers` portion of
    /// the Ring 2 hash domain, through the single canonical encoder.
    /// `instance` is length-prefixed (`write_bytes`) since its own encoding
    /// is fallible and independently composed — this keeps the byte offset
    /// of everything after it stable regardless of `instance`'s internal
    /// shape, the same reasoning `write_bytes` applies to every other
    /// variable-length field in this module.
    pub fn try_canonical_hash_bytes(&self) -> Result<Vec<u8>, PersistenceEnvelopeError> {
        let instance_bytes = self.instance.try_canonical_hash_bytes().map_err(|error| {
            PersistenceEnvelopeError::Serialization(error.to_string())
        })?;
        let mut w = CanonicalWriter::new();
        w.write_bytes(&instance_bytes);
        w.write_seq(self.join_counts.iter(), |w, (join_id, count)| {
            w.write_u32(*join_id);
            w.write_u16(*count);
        });
        w.write_seq(self.incidents.iter(), |w, (incident_id, incident)| {
            incident_id.canonical_encode(w);
            incident.canonical_encode(w);
        });
        w.write_seq(self.fibers.iter(), |w, (fiber_id, fiber)| {
            fiber_id.canonical_encode(w);
            fiber.canonical_encode(w);
        });
        Ok(w.into_bytes())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotEnvelope {
    schema_version: u16,
    artifact_abi: u32,
    /// Full artifact content hash (distinct from `artifact_abi`, the ABI
    /// tripwire version). Folded into the Ring 2 `state_hash` so
    /// wrong-artifact execution is unrepresentable (V&S §6).
    artifact_hash: [u8; 32],
    revision: u64,
    state: PersistedSnapshotState,
}

impl SnapshotEnvelope {
    pub fn new(
        artifact_abi: u32,
        artifact_hash: [u8; 32],
        revision: u64,
        state: PersistedSnapshotState,
    ) -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            artifact_abi,
            artifact_hash,
            revision,
            state,
        }
    }

    /// Ring 1: verify the tripwire on the raw bytes' decoded envelope.
    /// Full pre-decode (hash-before-parse) verification lives at the store
    /// layer, which holds the separately-persisted physical-integrity hash
    /// (V2.2) — this method still fails closed on schema drift for any
    /// caller that only has bytes in hand.
    pub fn decode(bytes: &[u8]) -> Result<Self, PersistenceEnvelopeError> {
        let envelope: Self = serde_json::from_slice(bytes)
            .map_err(|error| PersistenceEnvelopeError::Serialization(error.to_string()))?;
        if envelope.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err(PersistenceEnvelopeError::UnsupportedVersion {
                envelope: "snapshot",
                version: envelope.schema_version,
            });
        }
        Ok(envelope)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PersistenceEnvelopeError> {
        serde_json::to_vec(self)
            .map_err(|error| PersistenceEnvelopeError::Serialization(error.to_string()))
    }

    /// D3 Ring 2 `state_hash` (V&S §6): `BLAKE3(canonical(snapshot ‖
    /// fibres by fibre-ID ‖ concurrency table ‖ pending-effect set ‖
    /// revision ‖ artifact_hash))`. V2.1 (full supersession, 2026-07-21):
    /// every segment is now the single tagged-binary `CanonicalEncode`
    /// domain — zero `serde_json` in this method's dependency chain.
    pub fn state_hash(&self) -> Result<[u8; 32], PersistenceEnvelopeError> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.state.try_canonical_hash_bytes()?);
        hasher.update(&self.state.concurrency_table.to_canonical_bytes());
        hasher.update(&self.state.pending_effects.to_canonical_bytes());
        hasher.update(&self.revision.to_le_bytes());
        hasher.update(&self.artifact_hash);
        Ok(*hasher.finalize().as_bytes())
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub fn artifact_abi(&self) -> u32 {
        self.artifact_abi
    }

    pub fn artifact_hash(&self) -> [u8; 32] {
        self.artifact_hash
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn state(&self) -> &PersistedSnapshotState {
        &self.state
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JournalCommand {
    Kernel(Command),
    Start(StartCommand),
    Administrative { kind: String },
}

impl JournalCommand {
    pub fn command_type(&self) -> &'static str {
        match self {
            Self::Kernel(Command::Tick { .. }) => "tick",
            Self::Kernel(Command::EffectCompleted { .. }) => "effect_completed",
            Self::Kernel(Command::EffectFailed { .. }) => "effect_failed",
            Self::Kernel(Command::TimerFired { .. }) => "timer_fired",
            Self::Kernel(Command::MessageDelivered { .. }) => "message_delivered",
            Self::Kernel(Command::Cancel { .. }) => "cancel",
            Self::Kernel(Command::Terminate) => "terminate",
            Self::Kernel(Command::ResolveIncident { .. }) => "resolve_incident",
            Self::Kernel(Command::StartChildResult { .. }) => "start_child_result",
            Self::Kernel(Command::JobClaimed { .. }) => "job_claimed",
            Self::Kernel(Command::V2TriggerGuard { .. }) => "v2_trigger_guard",
            Self::Start(_) => "start",
            Self::Administrative { .. } => "administrative",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandEnvelope {
    schema_version: u16,
    command_id: Uuid,
    logical_time: Timestamp,
    command: JournalCommand,
}

impl CommandEnvelope {
    pub fn new(command_id: Uuid, logical_time: Timestamp, command: JournalCommand) -> Self {
        Self {
            schema_version: COMMAND_SCHEMA_VERSION,
            command_id,
            logical_time,
            command,
        }
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub fn command_id(&self) -> Uuid {
        self.command_id
    }

    pub fn logical_time(&self) -> Timestamp {
        self.logical_time
    }

    pub fn command(&self) -> &JournalCommand {
        &self.command
    }

    pub fn command_type(&self) -> &'static str {
        self.command.command_type()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventEnvelope {
    schema_version: u16,
    event: RuntimeEvent,
}

impl EventEnvelope {
    pub fn new(event: RuntimeEvent) -> Self {
        Self {
            schema_version: EVENT_SCHEMA_VERSION,
            event,
        }
    }

    pub fn event(&self) -> &RuntimeEvent {
        &self.event
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalRecord {
    schema_version: u16,
    command: CommandEnvelope,
    prior_revision: i64,
    new_revision: u64,
    artifact_hash: [u8; 32],
    /// D3 Ring 2 chain (V&S §6): this transition's state_hash BEFORE
    /// applying it. `[0; 32]` for the genesis record (no prior state).
    /// The chain invariant: record N's `prior_state_hash` must equal
    /// record N-1's `new_state_hash` (checked at resume — V2.3).
    prior_state_hash: [u8; 32],
    /// This transition's state_hash AFTER applying it. Historically named
    /// `state_hash`; `new_state_hash()` is the chain-aware name, `state_hash()`
    /// stays as an alias so existing call sites are unaffected.
    state_hash: [u8; 32],
    event_envelopes: Vec<EventEnvelope>,
    effect_ids: Vec<EffectId>,
}

impl JournalRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        command: CommandEnvelope,
        prior_revision: i64,
        new_revision: u64,
        artifact_hash: [u8; 32],
        prior_state_hash: [u8; 32],
        state_hash: [u8; 32],
        events: &[RuntimeEvent],
        effects: &[DurableEffect],
    ) -> Self {
        Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            command,
            prior_revision,
            new_revision,
            artifact_hash,
            prior_state_hash,
            state_hash,
            event_envelopes: events.iter().cloned().map(EventEnvelope::new).collect(),
            effect_ids: effects.iter().map(DurableEffect::effect_id).collect(),
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PersistenceEnvelopeError> {
        let record: Self = serde_json::from_slice(bytes)
            .map_err(|error| PersistenceEnvelopeError::Serialization(error.to_string()))?;
        if record.schema_version != JOURNAL_SCHEMA_VERSION {
            return Err(PersistenceEnvelopeError::UnsupportedVersion {
                envelope: "journal",
                version: record.schema_version,
            });
        }
        if record.command.schema_version != COMMAND_SCHEMA_VERSION {
            return Err(PersistenceEnvelopeError::UnsupportedVersion {
                envelope: "command",
                version: record.command.schema_version,
            });
        }
        if let Some(event) = record
            .event_envelopes
            .iter()
            .find(|event| event.schema_version != EVENT_SCHEMA_VERSION)
        {
            return Err(PersistenceEnvelopeError::UnsupportedVersion {
                envelope: "event",
                version: event.schema_version,
            });
        }
        Ok(record)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PersistenceEnvelopeError> {
        serde_json::to_vec(self)
            .map_err(|error| PersistenceEnvelopeError::Serialization(error.to_string()))
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub fn command(&self) -> &CommandEnvelope {
        &self.command
    }

    pub fn prior_revision(&self) -> i64 {
        self.prior_revision
    }

    pub fn new_revision(&self) -> u64 {
        self.new_revision
    }

    pub fn artifact_hash(&self) -> [u8; 32] {
        self.artifact_hash
    }

    pub fn state_hash(&self) -> [u8; 32] {
        self.state_hash
    }

    /// D3 Ring 2 chain-aware alias for `state_hash()` — this record's
    /// state_hash AFTER applying the transition.
    pub fn new_state_hash(&self) -> [u8; 32] {
        self.state_hash
    }

    /// D3 Ring 2 chain: this record's state_hash BEFORE applying the
    /// transition. `[0; 32]` for the genesis record.
    pub fn prior_state_hash(&self) -> [u8; 32] {
        self.prior_state_hash
    }

    pub fn event_envelopes(&self) -> &[EventEnvelope] {
        &self.event_envelopes
    }

    pub fn effect_ids(&self) -> &[EffectId] {
        &self.effect_ids
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PersistenceEnvelopeError {
    #[error("unsupported {envelope} envelope version {version}")]
    UnsupportedVersion {
        envelope: &'static str,
        version: u16,
    },
    #[error("persistence envelope serialization failed: {0}")]
    Serialization(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{session_stack::SessionStackState, ProcessState};
    use std::sync::Arc;

    fn snapshot() -> SnapshotEnvelope {
        SnapshotEnvelope::new(
            1,
            [7u8; 32],
            0,
            PersistedSnapshotState::new(
                ProcessInstance {
                    instance_id: Uuid::from_u128(1),
                    tenant_id: "tenant".to_string(),
                    process_key: "process".to_string(),
                    bytecode_version: [1; 32],
                    domain_payload: Arc::from("{}"),
                    domain_payload_hash: EffectId::content_hash(b"{}"),
                    session_stack: SessionStackState::default(),
                    flags: BTreeMap::new(),
                    counters: BTreeMap::new(),
                    join_expected: BTreeMap::new(),
                    state: ProcessState::Running,
                    correlation_id: "correlation".to_string(),
                    entry_id: Uuid::nil(),
                    runbook_id: Uuid::nil(),
                    created_at: 1,
                    integrity_hash: None,
                    quarantine_state: None,
                    plan_hash: None,
                    current_node_id: None,
                    placeholder_values: None,
                },
                [],
                BTreeMap::new(),
                [],
                ConcurrencyTable::new(),
                [],
            ),
        )
    }

    #[test]
    fn unknown_snapshot_version_is_refused() {
        let mut value: serde_json::Value =
            serde_json::from_slice(&snapshot().canonical_bytes().unwrap()).unwrap();
        value["schema_version"] = serde_json::json!(99);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            SnapshotEnvelope::decode(&bytes),
            Err(PersistenceEnvelopeError::UnsupportedVersion {
                envelope: "snapshot",
                version: 99
            })
        ));
    }

    /// V2.1e gate: golden-bytes fixture for the full `PersistedSnapshotState`
    /// contribution to the Ring 2 hash domain, through the single
    /// tagged-binary `CanonicalEncode` path (`try_canonical_hash_bytes`).
    /// Supersedes the pre-2026-07-21 interim hybrid's `serde_json`-encoded
    /// golden-bytes fixture (`ProcessInstance`/`Fiber` field-reordering
    /// drift is now structurally impossible — the binary tag scheme is
    /// order-independent by construction, not merely guarded against).
    #[test]
    fn golden_bytes_process_instance_and_fiber_canonical_hash_domain() {
        let mut fiber = Fiber::new(Uuid::from_u128(200), crate::types::Addr::new(3));
        fiber.stack.push(crate::types::Value::I64(42));
        fiber
            .control_stack
            .push(crate::concurrency::RecordId::new(Uuid::from_u128(9)));
        let instance = ProcessInstance {
            instance_id: Uuid::from_u128(1),
            tenant_id: "tenant".to_string(),
            process_key: "process".to_string(),
            bytecode_version: [1; 32],
            domain_payload: Arc::from("{}"),
            domain_payload_hash: EffectId::content_hash(b"{}"),
            session_stack: SessionStackState::default(),
            flags: BTreeMap::new(),
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: "correlation".to_string(),
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
            [fiber],
            BTreeMap::new(),
            [],
            ConcurrencyTable::new(),
            [],
        );
        let bytes = state.try_canonical_hash_bytes().unwrap();
        let golden = include_bytes!(
            "../tests/fixtures/golden_process_instance_and_fiber_canonical.bin"
        );
        assert_eq!(
            bytes,
            golden.to_vec(),
            "instance/fiber canonical binary encoding drifted from the committed golden bytes \
             — if this is a deliberate encoding change, regenerate the fixture and have a \
             reviewer ratify the diff; it is exactly the R1 risk this test exists to catch"
        );
    }

    fn perf_instance() -> ProcessInstance {
        ProcessInstance {
            instance_id: Uuid::from_u128(1),
            tenant_id: "tenant".to_string(),
            process_key: "process".to_string(),
            bytecode_version: [1; 32],
            domain_payload: Arc::from("{}"),
            domain_payload_hash: EffectId::content_hash(b"{}"),
            session_stack: SessionStackState::default(),
            flags: BTreeMap::new(),
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: "correlation".to_string(),
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

    fn perf_frame_bytes(live_fibers: usize) -> usize {
        let fibers = (0..live_fibers).map(|index| {
            Fiber::new(Uuid::from_u128(1_000 + index as u128), crate::types::Addr::new(0))
        });
        PersistedSnapshotState::new(
            perf_instance(),
            fibers,
            BTreeMap::new(),
            [],
            ConcurrencyTable::new(),
            [],
        )
        .try_canonical_hash_bytes()
        .unwrap()
        .len()
    }

    /// T11 perf claim — the persisted frame size is proportional to the
    /// number of live tokens (fibres), never to program size: `Instr` lives
    /// in the artifact envelope, not the STATE frame, so the STATE encoding
    /// takes no program input. Identical fibres carry a fixed per-fibre byte
    /// cost, so the frame is *exactly* `base + N·per_fibre` — a superlinear
    /// term (history, program, or anything else creeping into the frame)
    /// breaks the equality.
    #[test]
    fn v2_frame_size_is_linear_in_live_fiber_count() {
        let base = perf_frame_bytes(0);
        let per_fiber = perf_frame_bytes(1) - base;
        assert!(per_fiber > 0, "one live fibre must add bytes");
        for live in [2usize, 4, 8, 16, 32, 64] {
            assert_eq!(
                perf_frame_bytes(live),
                base + live * per_fiber,
                "frame size must be exactly base + N·per_fibre (live={live})"
            );
        }
    }
}
