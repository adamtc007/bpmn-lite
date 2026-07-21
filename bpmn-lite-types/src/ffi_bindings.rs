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

use crate::types::{FlagKey, ProcessInstance, Value};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
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
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
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
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
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
#[serde(tag = "source", content = "value", rename_all = "snake_case")]
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
#[serde(tag = "target", content = "value", rename_all = "snake_case")]
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

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FfiBindingError {
    #[error("domain payload is not valid JSON: {0}")]
    InvalidDomainPayload(String),
    #[error("FFI output is not valid JSON: {0}")]
    InvalidOutput(String),
    #[error("missing binding value: {0}")]
    MissingValue(String),
    #[error("binding value has the wrong type: {0}")]
    WrongType(String),
}

pub fn encode_ffi_inputs(
    instance: &ProcessInstance,
    declaration: &FfiTaskDecl,
) -> Result<Vec<u8>, FfiBindingError> {
    let domain = serde_json::from_str::<serde_json::Value>(&instance.domain_payload)
        .map_err(|error| FfiBindingError::InvalidDomainPayload(error.to_string()))?;
    let mut object = serde_json::Map::new();
    for binding in &declaration.inputs {
        let value = match &binding.source {
            BindingSource::Literal(Literal::Bool(value)) => serde_json::Value::Bool(*value),
            BindingSource::Literal(Literal::I64(value)) => (*value).into(),
            BindingSource::Literal(Literal::F64(value)) => serde_json::Number::from_f64(*value)
                .map(serde_json::Value::Number)
                .ok_or_else(|| FfiBindingError::WrongType(binding.target_field.clone()))?,
            BindingSource::Literal(Literal::String(value)) => {
                serde_json::Value::String(value.clone())
            }
            BindingSource::FlagRef(key) => match instance.flags.get(key) {
                Some(Value::Bool(value)) => serde_json::Value::Bool(*value),
                Some(Value::I64(value)) => (*value).into(),
                Some(Value::Str(value)) => (*value).into(),
                Some(Value::Ref(value)) => (*value).into(),
                None => return Err(FfiBindingError::MissingValue(binding.target_field.clone())),
            },
            BindingSource::DomainPayloadRef(path) => read_json_path(&domain, path)
                .cloned()
                .ok_or_else(|| FfiBindingError::MissingValue(binding.target_field.clone()))?,
        };
        object.insert(binding.target_field.clone(), value);
    }
    serde_json::to_vec(&serde_json::Value::Object(object))
        .map_err(|error| FfiBindingError::InvalidDomainPayload(error.to_string()))
}

pub fn apply_ffi_outputs(
    instance: &mut ProcessInstance,
    declaration: &FfiTaskDecl,
    bytes: &[u8],
) -> Result<(), FfiBindingError> {
    let output: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| FfiBindingError::InvalidOutput(error.to_string()))?;
    let mut domain: serde_json::Value = serde_json::from_str(&instance.domain_payload)
        .map_err(|error| FfiBindingError::InvalidDomainPayload(error.to_string()))?;
    for binding in &declaration.outputs {
        let value = output
            .get(&binding.source_field)
            .cloned()
            .ok_or_else(|| FfiBindingError::MissingValue(binding.source_field.clone()))?;
        match &binding.target {
            BindingTarget::FlagWrite(key) => {
                let value = match value {
                    serde_json::Value::Bool(value) => Value::Bool(value),
                    serde_json::Value::Number(value) => {
                        Value::I64(value.as_i64().ok_or_else(|| {
                            FfiBindingError::WrongType(binding.source_field.clone())
                        })?)
                    }
                    _ => return Err(FfiBindingError::WrongType(binding.source_field.clone())),
                };
                instance.flags.insert(*key, value);
            }
            BindingTarget::DomainPayloadWrite(path) => write_json_path(&mut domain, path, value)?,
        }
    }
    let canonical = serde_json::to_string(&domain)
        .map_err(|error| FfiBindingError::InvalidDomainPayload(error.to_string()))?;
    instance.domain_payload = Arc::from(canonical.as_str());
    instance.domain_payload_hash = crate::EffectId::content_hash(canonical.as_bytes());
    Ok(())
}

fn read_json_path<'a>(
    root: &'a serde_json::Value,
    path: &[String],
) -> Option<&'a serde_json::Value> {
    path.iter()
        .try_fold(root, |value, segment| value.get(segment))
}

fn write_json_path(
    root: &mut serde_json::Value,
    path: &[String],
    value: serde_json::Value,
) -> Result<(), FfiBindingError> {
    let Some((last, parents)) = path.split_last() else {
        return Err(FfiBindingError::MissingValue(
            "empty output path".to_string(),
        ));
    };
    let mut current = root;
    for segment in parents {
        current = current
            .as_object_mut()
            .and_then(|object| object.get_mut(segment))
            .ok_or_else(|| FfiBindingError::MissingValue(segment.clone()))?;
    }
    current
        .as_object_mut()
        .ok_or_else(|| FfiBindingError::WrongType(last.clone()))?
        .insert(last.clone(), value);
    Ok(())
}
