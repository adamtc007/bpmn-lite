//! FFI template idempotency declaration.
//!
//! Per A2 §6: declares how the engine recovers from a crash mid-dispatch.

use serde::{Deserialize, Serialize};

/// Replay/recovery discipline for a published template.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Idempotency {
    /// Same input always produces same output. After a crash mid-dispatch
    /// the engine may re-invoke the owner with the original FfiCall and
    /// trust the result will match the (lost) original.
    Idempotent,

    /// Not safe to re-invoke. After a crash mid-dispatch the engine
    /// escalates the unfinished invocation as an incident.
    NonIdempotent,

    /// Idempotent when a specific input field serves as a deduplication key.
    /// `selector` is a dotted JSON path (A7 `json_path` notation) into the
    /// serialised `FfiCall.input_payload`. On retry the owner is expected
    /// to detect the duplicate key and return the same result.
    IdempotentWithKey { selector: String },
}
