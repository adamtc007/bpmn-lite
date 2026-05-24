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
}
