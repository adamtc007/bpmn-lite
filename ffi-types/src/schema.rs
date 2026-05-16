//! Vocabulary-neutral schema language for FFI template inputs and outputs.
//!
//! Per A2 §4: a small fixed type language with Sem OS domain references as
//! a first-class kind. `Opaque` covers any owner-specific schema not
//! expressible in the closed set.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One field in a template's input or output schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSchema {
    /// Field name. Bindings reference fields by name.
    pub name: String,
    pub kind: SchemaKind,
    /// True if the field MUST be present in every call.
    /// False fields may be omitted; the owner provides a default or
    /// treats the absence as "not set".
    pub required: bool,
}

/// The set of declarable field kinds.
///
/// `Bool` / `I64` are representable in `bpmn_lite_types::Value` (the orch
/// stack/flag value type) and may target `BindingTarget::FlagWrite` outputs.
/// `F64`, `String`, `SemOsDomain`, `Opaque` are NOT representable in `Value`
/// and must target `BindingTarget::DomainPayloadWrite` (i.e. the canonical
/// JSON business payload). The verifier (A6) enforces this discipline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemaKind {
    Bool,
    I64,
    F64,
    /// UTF-8 string of arbitrary length.
    String,
    /// A Sem OS-governed enumerated domain. `domain_id` identifies the
    /// domain in the Sem OS catalogue; `version_hash` is the BLAKE3 of the
    /// domain definition at publication time. Owners validate symbol-to-id
    /// resolution at call time.
    SemOsDomain {
        domain_id: Uuid,
        /// 32 bytes. Stored hex-encoded in JSONB; round-trips losslessly via serde.
        version_hash: [u8; 32],
    },
    /// Owner-specific binary schema. The bpmn-lite verifier treats Opaque
    /// fields as flow-through (no structural type-check beyond presence);
    /// owner-side bridges validate the contained schema themselves.
    Opaque {
        /// Free-text format identifier (no central registry in v1.1).
        owner_format: String,
        /// Verbatim owner-defined schema bytes.
        owner_schema: Vec<u8>,
    },
}

impl SchemaKind {
    /// Returns true if a value of this kind fits inside `bpmn_lite_types::Value`.
    /// Used by the verifier (A6) to reject FlagWrite targets for incompatible kinds.
    pub fn fits_in_flag(&self) -> bool {
        matches!(self, SchemaKind::Bool | SchemaKind::I64)
    }
}
