//! Wire types crossing the FFI dispatch boundary.
//!
//! Per A2 §7. `FfiCall` is what the bpmn-lite engine sends to a registered
//! `FfiExecutionOwner`. `FfiResult` is what the owner returns. Owners
//! never see bpmn-lite internals (flags, session_stack, domain_payload);
//! everything they need is pre-extracted into `FfiCall.input_payload`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One foreign function invocation, as the owner sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FfiCall {
    /// Pre-assigned UUIDv7. Recorded in `ForeignFunctionInvocationRecord`.
    pub invocation_id: Uuid,

    /// The published template being invoked.
    pub template_id: [u8; 32],

    pub tenant_id: String,
    pub process_instance_id: Uuid,

    /// BPMN element ID of the calling ServiceTask (from `debug_map`).
    pub caller_task_id: String,

    /// Serialised input fields as a JSON object. Keys are input field
    /// names from the template's `input_schema`. Values are JSON-typed
    /// per `SchemaKind`:
    ///
    /// - `Bool` → JSON bool
    /// - `I64` → JSON number (integer)
    /// - `F64` → JSON number (float)
    /// - `String` / `SemOsDomain` → JSON string
    /// - `Opaque` → owner-defined; the engine forwards the JSON value
    ///   that the binding source produced
    pub input_payload: Vec<u8>,
}

/// The owner's response.
///
/// Per A2 §8 outcome model: three variants for three distinct semantics.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FfiResult {
    /// The call produced a result.
    Success {
        /// Serialised output fields as a JSON object.
        output_payload: Vec<u8>,
        /// Evaluation trace for audit. Required for Success.
        trace_payload: Vec<u8>,
        /// If the owner replaces the canonical business payload wholesale
        /// (HTTP/gRPC owners doing JSON-in/JSON-out), the new payload goes
        /// here. The engine replaces `instance.domain_payload` with this
        /// value and recomputes `domain_payload_hash`. `None` = unchanged.
        new_domain_payload: Option<String>,
    },

    /// The call completed mechanically but produced no business result.
    /// dmn-lite `EvalError::NoMatch` maps here. The engine does NOT apply
    /// output bindings; the process advances `fiber.pc`.
    NoMatch {
        /// Present when evaluation began before no-match was determined.
        /// May be `None` for owners that fail-fast before evaluation.
        trace_payload: Option<Vec<u8>>,
    },

    /// Technical failure.
    Incident {
        error_class: FfiIncidentClass,
        message: String,
        /// Retry hint in milliseconds. Meaningful only for `Transient`.
        retry_hint_ms: Option<u64>,
    },
}

/// Mirror of `bpmn_lite_types::ErrorClass` defined here so ffi-types has
/// no bpmn-lite dependency. The engine converts at the boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum FfiIncidentClass {
    Transient,
    ContractViolation,
    BusinessRejection { rejection_code: String },
}
