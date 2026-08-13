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

/// Resolve a `BindingSource` to a concrete JSON scalar against `instance`'s
/// flags and the already-parsed `domain` payload.
///
/// Shared by FFI input encoding and message correlation so both derive a value
/// from process data through one code path — a second, divergent derivation
/// would make correlation silently mismatch. `label` names the source for
/// diagnostics (a target field for FFI, the correlation-key expression here).
fn resolve_binding_scalar_with_domain(
    instance: &ProcessInstance,
    source: &BindingSource,
    domain: &serde_json::Value,
    label: &str,
) -> Result<serde_json::Value, FfiBindingError> {
    Ok(match source {
        BindingSource::Literal(Literal::Bool(value)) => serde_json::Value::Bool(*value),
        BindingSource::Literal(Literal::I64(value)) => (*value).into(),
        BindingSource::Literal(Literal::F64(value)) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| FfiBindingError::WrongType(label.to_string()))?,
        BindingSource::Literal(Literal::String(value)) => serde_json::Value::String(value.clone()),
        BindingSource::FlagRef(key) => match instance.flags.get(key) {
            Some(Value::Bool(value)) => serde_json::Value::Bool(*value),
            Some(Value::I64(value)) => (*value).into(),
            Some(Value::Str(value)) => (*value).into(),
            Some(Value::Ref(value)) => (*value).into(),
            // §18 ruling K Part 2: `Value::Array` is new and is not a valid
            // scalar — a flag holding an array is a real type mismatch, not
            // something to silently flatten. Fail closed.
            Some(Value::Array(_)) => return Err(FfiBindingError::WrongType(label.to_string())),
            None => return Err(FfiBindingError::MissingValue(label.to_string())),
        },
        BindingSource::DomainPayloadRef(path) => read_json_path(domain, path)
            .cloned()
            .ok_or_else(|| FfiBindingError::MissingValue(label.to_string()))?,
    })
}

/// Resolve a `BindingSource` to a concrete JSON scalar, parsing the domain
/// payload as needed. The correlation-facing entry point onto the same
/// resolution `encode_ffi_inputs` uses.
// H4.2 (EOP-PLAN-CRATE-HYGIENE-001): pub(crate) — zero external callers
// (grep-confirmed); only used by this same file's `correlation key`
// resolution path.
pub(crate) fn resolve_binding_scalar(
    instance: &ProcessInstance,
    source: &BindingSource,
    label: &str,
) -> Result<serde_json::Value, FfiBindingError> {
    // Only DomainPayloadRef needs the parsed payload; parse lazily so a
    // literal/flag correlation key costs no JSON parse.
    let domain = match source {
        BindingSource::DomainPayloadRef(_) => serde_json::from_str::<serde_json::Value>(
            &instance.domain_payload,
        )
        .map_err(|error| FfiBindingError::InvalidDomainPayload(error.to_string()))?,
        _ => serde_json::Value::Null,
    };
    resolve_binding_scalar_with_domain(instance, source, &domain, label)
}

/// Canonicalize a resolved JSON scalar into a message-correlation key string.
///
/// The single derivation waiter, publisher, and the gRPC wire boundary all go
/// through (V&S §28) — content-based, so a dynamic business key (`case_id`
/// from the payload) correlates by its value, not an intern id. Correlation
/// keys are scalar bool/integer/string; floats and composites fail closed.
pub fn correlation_key_string(scalar: &serde_json::Value) -> Result<String, FfiBindingError> {
    match scalar {
        serde_json::Value::Bool(value) => Ok(value.to_string()),
        serde_json::Value::String(value) => Ok(value.clone()),
        serde_json::Value::Number(number) => number
            .as_i64()
            .map(|value| value.to_string())
            .or_else(|| number.as_u64().map(|value| value.to_string()))
            .ok_or_else(|| {
                FfiBindingError::WrongType(
                    "correlation key must be an integer or string, not a float".to_string(),
                )
            }),
        serde_json::Value::Null
        | serde_json::Value::Array(_)
        | serde_json::Value::Object(_) => Err(FfiBindingError::WrongType(
            "correlation key must be a scalar (bool, integer, or string)".to_string(),
        )),
    }
}

/// Resolve a correlation-key `BindingSource` to its content string in one step.
pub fn resolve_correlation_key(
    instance: &ProcessInstance,
    source: &BindingSource,
) -> Result<String, FfiBindingError> {
    correlation_key_string(&resolve_binding_scalar(instance, source, "correlation key")?)
}

pub fn encode_ffi_inputs(
    instance: &ProcessInstance,
    declaration: &FfiTaskDecl,
) -> Result<Vec<u8>, FfiBindingError> {
    let domain = serde_json::from_str::<serde_json::Value>(&instance.domain_payload)
        .map_err(|error| FfiBindingError::InvalidDomainPayload(error.to_string()))?;
    let mut object = serde_json::Map::new();
    for binding in &declaration.inputs {
        let value = resolve_binding_scalar_with_domain(
            instance,
            &binding.source,
            &domain,
            &binding.target_field,
        )?;
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

#[cfg(test)]
mod correlation_tests {
    use super::*;
    use crate::session_stack::SessionStackState;
    use crate::types::ProcessState;
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn instance_with(domain: &str, flags: BTreeMap<FlagKey, Value>) -> ProcessInstance {
        ProcessInstance {
            instance_id: Uuid::nil(),
            tenant_id: "t".to_string(),
            process_key: "p".to_string(),
            bytecode_version: [0u8; 32],
            domain_payload: domain.to_string().into(),
            domain_payload_hash: [0u8; 32],
            session_stack: SessionStackState::default(),
            flags,
            counters: BTreeMap::new(),
            join_expected: BTreeMap::new(),
            state: ProcessState::Running,
            correlation_id: "c".to_string(),
            entry_id: Uuid::nil(),
            runbook_id: Uuid::nil(),
            created_at: 1,
            integrity_hash: None,
            quarantine_state: None,
            plan_hash: None,
            current_node_id: None,
            placeholder_values: None,
        }
    }

    #[test]
    fn correlation_key_string_canonicalizes_scalars_and_rejects_composites() {
        use serde_json::json;
        assert_eq!(correlation_key_string(&json!("ACME-42")).unwrap(), "ACME-42");
        assert_eq!(correlation_key_string(&json!(42)).unwrap(), "42");
        assert_eq!(correlation_key_string(&json!(-7)).unwrap(), "-7");
        assert_eq!(correlation_key_string(&json!(true)).unwrap(), "true");
        assert!(correlation_key_string(&json!(1.5)).is_err(), "float rejected");
        assert!(correlation_key_string(&json!(null)).is_err(), "null rejected");
        assert!(correlation_key_string(&json!([1, 2])).is_err(), "array rejected");
        assert!(correlation_key_string(&json!({"a": 1})).is_err(), "object rejected");
    }

    #[test]
    fn resolve_correlation_key_reads_a_dynamic_string_from_domain_payload() {
        let instance = instance_with(r#"{"case_id":"ACME-42"}"#, BTreeMap::new());
        let source = BindingSource::DomainPayloadRef(vec!["case_id".to_string()]);
        assert_eq!(resolve_correlation_key(&instance, &source).unwrap(), "ACME-42");
    }

    #[test]
    fn resolve_correlation_key_reads_an_i64_flag() {
        let mut flags = BTreeMap::new();
        flags.insert(3u32, Value::I64(99));
        let instance = instance_with("{}", flags);
        assert_eq!(
            resolve_correlation_key(&instance, &BindingSource::FlagRef(3)).unwrap(),
            "99"
        );
    }

    #[test]
    fn resolve_correlation_key_reads_a_string_literal() {
        let instance = instance_with("{}", BTreeMap::new());
        let source = BindingSource::Literal(Literal::String("fixed-key".to_string()));
        assert_eq!(resolve_correlation_key(&instance, &source).unwrap(), "fixed-key");
    }

    #[test]
    fn resolve_correlation_key_rejects_a_missing_payload_path() {
        let instance = instance_with(r#"{"other":1}"#, BTreeMap::new());
        let source = BindingSource::DomainPayloadRef(vec!["case_id".to_string()]);
        assert!(resolve_correlation_key(&instance, &source).is_err());
    }

    #[test]
    fn resolve_correlation_key_rejects_a_composite_payload_value() {
        let instance = instance_with(r#"{"case_id":[1,2,3]}"#, BTreeMap::new());
        let source = BindingSource::DomainPayloadRef(vec!["case_id".to_string()]);
        assert!(resolve_correlation_key(&instance, &source).is_err());
    }
}
