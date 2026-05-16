//! `ForeignFunctionInvocationRecord` — the append-only audit record written
//! for every FFI call.
//!
//! Per A2 §9. The record is written with `outcome_kind = Pending` BEFORE
//! dispatch; UPDATEd with the final outcome AFTER the owner returns. Crash
//! recovery detects stale `Pending` rows and applies the idempotency policy.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One row of the FFI audit log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForeignFunctionInvocationRecord {
    /// UUIDv7. Equals `FfiCall.invocation_id`.
    pub invocation_id: Uuid,
    pub caller_process_instance_id: Uuid,
    /// BPMN element ID of the calling ServiceTask.
    pub caller_task_id: String,
    /// Bytecode address of the `ExecFfi` instruction (disambiguates when
    /// the same task_id appears at multiple program points, e.g. loops).
    pub caller_pc: u32,
    pub template_id: [u8; 32],
    pub owner_type: String,
    pub tenant_id: String,
    /// Epoch milliseconds.
    pub invoked_at: i64,
    /// As sent to the owner.
    pub input_payload: Vec<u8>,
    pub outcome_kind: FfiOutcomeKind,
    /// Present iff outcome_kind == Success.
    pub output_payload: Option<Vec<u8>>,
    /// Present for Success; optional for NoMatch / Incident.
    pub trace_payload: Option<Vec<u8>>,
    /// Present iff outcome_kind == Incident.
    pub error_payload: Option<Vec<u8>>,
}

/// The four observable states of an invocation record.
///
/// `Pending` is the sentinel written before dispatch begins. A stale
/// `Pending` row (older than a configurable threshold) indicates a crash
/// mid-dispatch; the engine applies the template's `Idempotency` policy
/// on recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FfiOutcomeKind {
    Pending,
    Success,
    NoMatch,
    Incident,
}

impl FfiOutcomeKind {
    /// The Postgres-side text encoding for `outcome_kind` columns.
    pub fn as_str(&self) -> &'static str {
        match self {
            FfiOutcomeKind::Pending => "pending",
            FfiOutcomeKind::Success => "success",
            FfiOutcomeKind::NoMatch => "no_match",
            FfiOutcomeKind::Incident => "incident",
        }
    }
}
