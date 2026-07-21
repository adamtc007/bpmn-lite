use crate::{
    Command, DurableEffect, EffectId, Fiber, Incident, JoinId, ProcessInstance, RuntimeEvent,
    Snapshot, StartCommand, Timestamp, Uuid,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SNAPSHOT_SCHEMA_VERSION: u16 = 1;
pub const COMMAND_SCHEMA_VERSION: u16 = 1;
pub const EVENT_SCHEMA_VERSION: u16 = 1;
pub const JOURNAL_SCHEMA_VERSION: u16 = 1;

/// Canonical durable state covered by the snapshot hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedSnapshotState {
    instance: ProcessInstance,
    fibers: BTreeMap<Uuid, Fiber>,
    join_counts: BTreeMap<JoinId, u16>,
    incidents: BTreeMap<Uuid, Incident>,
}

impl PersistedSnapshotState {
    pub fn new(
        mut instance: ProcessInstance,
        fibers: impl IntoIterator<Item = Fiber>,
        join_counts: BTreeMap<JoinId, u16>,
        incidents: impl IntoIterator<Item = Incident>,
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

    pub fn to_runtime_snapshot(&self) -> Snapshot {
        Snapshot::new(self.instance.clone(), self.fibers.values().cloned())
            .with_join_counts(self.join_counts.clone())
            .with_incidents(self.incidents.values().cloned())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotEnvelope {
    schema_version: u16,
    artifact_abi: u32,
    revision: u64,
    state: PersistedSnapshotState,
}

impl SnapshotEnvelope {
    pub fn new(artifact_abi: u32, revision: u64, state: PersistedSnapshotState) -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            artifact_abi,
            revision,
            state,
        }
    }

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

    pub fn state_hash(&self) -> Result<[u8; 32], PersistenceEnvelopeError> {
        Ok(blake3::hash(&self.canonical_bytes()?).into())
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub fn artifact_abi(&self) -> u32 {
        self.artifact_abi
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
    state_hash: [u8; 32],
    event_envelopes: Vec<EventEnvelope>,
    effect_ids: Vec<EffectId>,
}

impl JournalRecord {
    pub fn new(
        command: CommandEnvelope,
        prior_revision: i64,
        new_revision: u64,
        artifact_hash: [u8; 32],
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
}
