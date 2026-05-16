//! Compiled artifact types for FFI task bindings and data-object declarations.
//!
//! These are the output of the A5 lowering pass — the compiler lowers IR-level
//! unresolved `Expression` trees into these resolved forms stored in
//! `CompiledProgram`. The FFI dispatch path (A8, `Instr::ExecFfi`) reads them
//! at runtime to extract input values and write output values.
//!
//! Separation: these types live in `bpmn-lite-types` (the compiled artifact
//! layer) rather than in `bpmn-lite-compiler` (the transient IR layer) because
//! they are part of the durable `CompiledProgram` artifact that the engine,
//! VM, and store all consume.

use crate::types::FlagKey;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Data object declarations ──────────────────────────────────────────────────

/// Primitive base types for process variables declared via BPMN data objects.
///
/// Per A2 §10. `Bool` and `I64` map to `DataObjectStorage::Flag` (they fit in
/// `bpmn_lite_types::Value`). `F64` and `String` map to
/// `DataObjectStorage::DomainPayload` (the canonical JSON business payload).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimitiveType {
    Bool,
    I64,
    F64,
    String,
}

/// Type declaration for a process variable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DataObjectType {
    Primitive(PrimitiveType),
    /// A Sem OS-governed enumerated domain. Maps to `DomainPayload` storage.
    SemOsDomain {
        domain_id: Uuid,
        version_hash: [u8; 32],
    },
}

/// How a data object's value is stored in the process instance at runtime.
///
/// Per A2 §10 storage-assignment rule:
/// - `Bool`, `I64` → `Flag` (fits in `bpmn_lite_types::Value`)
/// - `F64`, `String`, `SemOsDomain` → `DomainPayload`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "storage", rename_all = "snake_case")]
pub enum DataObjectStorage {
    /// Stored in `ProcessInstance.flags` under this key.
    Flag(FlagKey),
    /// Stored in `ProcessInstance.domain_payload` at the given dotted JSON
    /// path (evaluated by `bpmn_lite_vm::json_path`).
    DomainPayload(Vec<String>),
}

/// Role declaration for a data object when the BPMN process is published as
/// an FFI template (Δ7, A12). Has no effect on runtime execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataObjectRole {
    /// Part of the process's FFI input schema.
    Input,
    /// Part of the process's FFI output schema.
    Output,
    /// Process-internal; not exposed in the FFI surface (default).
    Internal,
}

/// A resolved data-object declaration stored in `CompiledProgram.data_objects`.
///
/// Keyed by the data object's `id` attribute. The compiler's lowering pass
/// (A5) produces one entry per declared `<bpmn:dataObject>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataObjectDecl {
    pub id: String,
    pub type_decl: DataObjectType,
    pub storage: DataObjectStorage,
    pub role: DataObjectRole,
}

// ── FFI task binding types ────────────────────────────────────────────────────

/// A literal value in a compiled binding.
///
/// Produced from the C-minimal expression language (A2 §5):
/// `bool` / integer / float / string / symbol literals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Literal {
    Bool(bool),
    I64(i64),
    F64(f64),
    String(String),
}

/// A resolved binding source for one `<bpmn:input>` entry.
///
/// Produced by lowering `Expression::VarRef` / `Expression::Literal` against
/// the `data_objects` map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum BindingSource {
    /// A compile-time constant.
    Literal(Literal),
    /// Read from `ProcessInstance.flags[key]` at dispatch time.
    FlagRef(FlagKey),
    /// Read from `ProcessInstance.domain_payload` at the given path
    /// using `bpmn_lite_vm::json_path::read`.
    DomainPayloadRef(Vec<String>),
}

/// A resolved binding target for one `<bpmn:output>` entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum BindingTarget {
    /// Write into `ProcessInstance.flags[key]`.
    /// Only valid for `SchemaKind::Bool` and `SchemaKind::I64` outputs
    /// (verifier enforces this — A6).
    FlagWrite(FlagKey),
    /// Write into `ProcessInstance.domain_payload` at the given path
    /// using `bpmn_lite_vm::json_path::write_at_path`.
    DomainPayloadWrite(Vec<String>),
}

/// One compiled `<bpmn:input>` binding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledFfiInputBinding {
    /// Name of the FFI template input field (from `target=` attribute).
    pub target_field: String,
    pub source: BindingSource,
}

/// One compiled `<bpmn:output>` binding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledFfiOutputBinding {
    /// Name of the FFI template output field (from `source=` attribute).
    pub source_field: String,
    pub target: BindingTarget,
}

/// The complete compiled declaration for one `Instr::ExecFfi` instruction.
///
/// Indexed by bytecode address in `CompiledProgram.ffi_task_decls`.
/// The A8 VM handler reads this to serialise inputs and deserialise outputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FfiTaskDecl {
    /// Matches `Instr::ExecFfi.template_id`.
    pub template_id: [u8; 32],
    pub inputs: Vec<CompiledFfiInputBinding>,
    pub outputs: Vec<CompiledFfiOutputBinding>,
}
