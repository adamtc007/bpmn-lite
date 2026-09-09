//! Minimal session-stack bridge types — local to bpmn-lite.
//!
//! These carry only the fields bpmn-lite actually reads or writes. The
//! workspace kind is an **opaque, caller-supplied taxonomy value**
//! (reuse-programme tranche BL-T2 cut-over): bpmn-lite assigns it no
//! semantics, validates only shape (see [`WorkspaceKind`]), and
//! round-trips it verbatim. Any host system's workspace vocabulary is
//! that system's business, never this crate's.

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
    pub active_workspace: Option<WorkspaceKind>,
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

/// Opaque workspace-kind taxonomy value carried in the session stack.
///
/// Caller-supplied at instance start; bpmn-lite validates shape only and
/// never interprets the value. Shape rule (fail-closed): 1–64 bytes,
/// ASCII, each character alphanumeric or one of `- _ . :`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct WorkspaceKind(String);

/// Rejection reasons for a malformed workspace-kind value. Localized and
/// typed — a bad value is never silently substituted or defaulted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceKindError {
    #[error("workspace kind is empty")]
    Empty,
    #[error("workspace kind length {len} exceeds {max} bytes")]
    TooLong { len: usize, max: usize },
    #[error("workspace kind contains illegal byte {byte:#04x} at offset {offset}; allowed: ASCII alphanumeric and '-' '_' '.' ':'")]
    IllegalByte { byte: u8, offset: usize },
}

impl WorkspaceKind {
    pub const MAX_LEN: usize = 64;

    pub fn new(value: impl Into<String>) -> Result<Self, WorkspaceKindError> {
        let value = value.into();
        if value.is_empty() {
            return Err(WorkspaceKindError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(WorkspaceKindError::TooLong {
                len: value.len(),
                max: Self::MAX_LEN,
            });
        }
        if let Some((offset, byte)) = value
            .bytes()
            .enumerate()
            .find(|(_, b)| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':')))
        {
            return Err(WorkspaceKindError::IllegalByte { byte, offset });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for WorkspaceKind {
    type Error = WorkspaceKindError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<WorkspaceKind> for String {
    fn from(value: WorkspaceKind) -> Self {
        value.0
    }
}

impl std::fmt::Display for WorkspaceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
