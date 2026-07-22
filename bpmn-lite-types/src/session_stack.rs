//! Minimal session-stack bridge types — local to bpmn-lite.
//!
//! These are a projection of the ob-poc `session_stack` DTO family, carrying
//! only the fields bpmn-lite actually reads or writes.  The full ob-poc type
//! includes UI/viewport fields (`ViewLevel`, `SessionStackFrame`) that
//! bpmn-lite never touches; they are deliberately omitted here to keep this
//! crate free of the cross-repo git dep.
//!
//! Serde shape is intentionally compatible with the ob-poc originals so that
//! JSON round-trips remain lossless at the integration boundary.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Execution-relevant session-stack state carried into a bpmn-lite activation.
///
/// This is a value type copied across the integration boundary.  Each system
/// persists and mutates its own copy independently.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SessionStackState {
    pub session_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<SessionScopeState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_workspace: Option<SessionWorkspaceKind>,
    /// Stack frames — bpmn-lite never inspects individual frames; the vec is
    /// preserved opaquely so that round-tripping through ob-poc is lossless.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_stack: Vec<serde_json::Value>,
    #[serde(default)]
    pub trace_sequence: u64,
}

impl SessionStackState {
    /// A18 A3 rollback-set: opaque serialized snapshot for
    /// `ConcurrencyRecord::rollback_session_stack`. Serde JSON, not this
    /// module's canonical-encoding primitives — `workspace_stack`'s
    /// arbitrary `serde_json::Value` frames can only round-trip through
    /// `canonical.rs`'s fallible `encode_canonical_json`, and
    /// `ConcurrencyRecord`'s `CanonicalEncode` impl is infallible by trait
    /// signature (see `ConcurrencyRecord`'s doc comment for the full
    /// rationale). `expect` is sound here: every field of this struct is
    /// already-valid in-memory state (no externally-supplied string to
    /// reject), so `serde_json::to_string` cannot fail for it.
    pub fn to_rollback_snapshot(&self) -> Box<str> {
        serde_json::to_string(self)
            .expect("SessionStackState always serializes")
            .into_boxed_str()
    }

    /// Inverse of `to_rollback_snapshot`. A malformed snapshot is a defect
    /// in whatever wrote it (the kernel is the only writer), not a
    /// tolerable runtime condition — callers should propagate the error as
    /// a hard rollback failure, never silently substitute a default.
    pub fn from_rollback_snapshot(snapshot: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(snapshot)
    }
}

/// Client-group scope snapshot at activation time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionScopeState {
    pub client_group_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_group_name: Option<String>,
}

/// Workspace kind carried in the session stack.
///
/// Variants must stay serde-compatible with `ob_poc_types::session_stack::SessionWorkspaceKind`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionWorkspaceKind {
    ProductMaintenance,
    Catalogue,
    Deal,
    Cbu,
    Kyc,
    InstrumentMatrix,
    #[serde(rename = "onboarding_request")]
    OnBoarding,
    #[serde(rename = "semos_maintenance")]
    SemOsMaintenance,
    LifecycleResources,
    BookingPrincipal,
    Bpmn,
}
