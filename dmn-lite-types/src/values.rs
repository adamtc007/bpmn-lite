//! Runtime input/output context types for decision evaluation.
//!
//! [`TypedInputContext`] is constructed by the caller (e.g., bpmn-lite at the
//! invocation boundary in V&S §11.4) and passed to the evaluator.
//! [`TypedOutputContext`] is produced by the evaluator after a rule matches.
//!
//! The distinction between *missing* (`None`) and *null* (`Some(Null)`) is
//! preserved in the API for future profiles (Phase v0.3+). In Profile v0.1
//! both are treated identically at evaluation time: any non-null-test
//! predicate returns `false`, and `is-null` returns `true`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use thiserror::Error;

use crate::ids::{FieldId, SchemaHash};
use crate::ir::{FieldSchema, TypedValue};

// ── Schema hash computation ───────────────────────────────────────────────────

/// Compute a schema hash from a slice of field schemas.
///
/// Used by both the input context builder and the reference evaluator to
/// detect schema mismatches at evaluation time.
pub fn compute_schema_hash(schema: &[FieldSchema]) -> SchemaHash {
    let mut h = DefaultHasher::new();
    schema.len().hash(&mut h);
    for f in schema {
        f.name.hash(&mut h);
        f.field_type.hash(&mut h);
        f.field_id.hash(&mut h);
    }
    SchemaHash(h.finish())
}

// ── InputContextError ─────────────────────────────────────────────────────────

/// Errors produced when constructing a [`TypedInputContext`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InputContextError {
    /// The provided slot count does not match the schema arity.
    #[error("slot count {actual} does not match schema arity {expected}")]
    SlotCountMismatch {
        /// Expected number of slots (= schema length).
        expected: usize,
        /// Actual number of slots provided.
        actual: usize,
    },

    /// `set_by_name` was called with a name not present in the schema.
    #[error("no field named '{name}' in schema")]
    UnknownFieldName {
        /// The unrecognised field name.
        name: String,
    },

    /// A `FieldId` exceeds the schema arity.
    #[error("FieldId {field_id} out of bounds for schema arity {arity}")]
    FieldIdOutOfBounds {
        /// The out-of-bounds `FieldId`.
        field_id: FieldId,
        /// The schema arity.
        arity: usize,
    },
}

// ── TypedInputContext ─────────────────────────────────────────────────────────

/// Read-only, schema-keyed input to a single decision evaluation.
///
/// Constructed by the caller via [`TypedInputContextBuilder`] and passed to
/// the reference evaluator or the bytecode VM.
///
/// **Missing vs null:** `slots[i] = None` means the caller did not supply a
/// value for field `i`. `slots[i] = Some(TypedValue::Null)` means the caller
/// explicitly provided null. In Profile v0.1 both are treated identically at
/// evaluation time: any value-dependent predicate returns `false`, and
/// `is-null` returns `true`. The distinction is preserved for future profiles.
#[derive(Debug, Clone)]
pub struct TypedInputContext {
    /// Slot vector indexed by `FieldId`. `None` = missing; `Some(Null)` = explicit null.
    slots: Vec<Option<TypedValue>>,
    /// Hash of the schema this context was built against.
    pub schema_hash: SchemaHash,
}

impl TypedInputContext {
    /// Construct from a schema and a pre-built slot vector.
    ///
    /// Validates that `slots.len()` matches `schema.len()`.
    /// Use [`TypedInputContextBuilder`] for the ergonomic name-based API.
    pub fn from_slots(
        schema: &[FieldSchema],
        slots: Vec<Option<TypedValue>>,
    ) -> Result<Self, InputContextError> {
        if slots.len() != schema.len() {
            return Err(InputContextError::SlotCountMismatch {
                expected: schema.len(),
                actual: slots.len(),
            });
        }
        Ok(Self {
            slots,
            schema_hash: compute_schema_hash(schema),
        })
    }

    /// Read the value at `field_id`.
    ///
    /// Returns `None` for a missing field; `Some(&TypedValue::Null)` for an
    /// explicitly null field; `Some(&value)` for a present value.
    pub fn get(&self, field_id: FieldId) -> Option<&TypedValue> {
        self.slots.get(field_id.0)?.as_ref()
    }

    /// Number of input slots (equals the schema arity).
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// True when no input slots are present (empty schema; unusual in practice).
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// True if slot `i` is missing (was not provided by the caller).
    pub fn is_missing(&self, field_id: FieldId) -> bool {
        self.slots.get(field_id.0).is_none_or(|s| s.is_none())
    }
}

// ── TypedInputContextBuilder ──────────────────────────────────────────────────

/// Ergonomic builder for [`TypedInputContext`].
///
/// Construct with `new(schema)`, call `set`/`set_by_name`/`set_null` for each
/// field, then call `build()`. Fields not explicitly set remain missing (`None`).
#[derive(Debug, Clone)]
pub struct TypedInputContextBuilder {
    slots: Vec<Option<TypedValue>>,
    schema_hash: SchemaHash,
    name_to_id: Vec<(String, FieldId)>,
}

impl TypedInputContextBuilder {
    /// Create a builder for the given schema. All slots start as missing.
    pub fn new(schema: &[FieldSchema]) -> Self {
        let slots = vec![None; schema.len()];
        let schema_hash = compute_schema_hash(schema);
        let name_to_id = schema
            .iter()
            .map(|f| (f.name.clone(), f.field_id))
            .collect();
        Self {
            slots,
            schema_hash,
            name_to_id,
        }
    }

    /// Set a field by `FieldId`. Panics if `field_id` is out of bounds.
    pub fn set(&mut self, field_id: FieldId, value: TypedValue) -> &mut Self {
        assert!(
            field_id.0 < self.slots.len(),
            "FieldId {} out of bounds for schema arity {}",
            field_id.0,
            self.slots.len()
        );
        self.slots[field_id.0] = Some(value);
        self
    }

    /// Set a field by name. Returns `Err` if the name is not in the schema.
    pub fn set_by_name(
        &mut self,
        name: &str,
        value: TypedValue,
    ) -> Result<&mut Self, InputContextError> {
        let field_id = self
            .name_to_id
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, id)| *id)
            .ok_or_else(|| InputContextError::UnknownFieldName {
                name: name.to_owned(),
            })?;
        self.slots[field_id.0] = Some(value);
        Ok(self)
    }

    /// Explicitly mark a field as null (distinct from missing at API level).
    ///
    /// In Profile v0.1 this is evaluated identically to missing.
    pub fn set_null(&mut self, field_id: FieldId) -> &mut Self {
        assert!(field_id.0 < self.slots.len(), "FieldId out of bounds");
        self.slots[field_id.0] = Some(TypedValue::Null);
        self
    }

    /// Consume the builder and produce a [`TypedInputContext`].
    ///
    /// Fields not set remain `None` (missing). No completeness validation is
    /// performed here; the evaluator validates at call time.
    pub fn build(self) -> TypedInputContext {
        TypedInputContext {
            slots: self.slots,
            schema_hash: self.schema_hash,
        }
    }
}

// ── TypedOutputContext ────────────────────────────────────────────────────────

/// Read-only output bindings produced by the evaluator after a rule matches.
///
/// All declared output slots are populated; the evaluator never produces a
/// `TypedOutputContext` with missing slots (the compiler enforces completeness).
#[derive(Debug, Clone)]
pub struct TypedOutputContext {
    /// Slot vector indexed by `FieldId`. Always fully populated (no `None`).
    slots: Vec<TypedValue>,
    /// Hash of the output schema this context was produced against.
    pub schema_hash: SchemaHash,
}

impl TypedOutputContext {
    /// Construct from a schema and a fully-populated slot vector.
    ///
    /// Used by the evaluator. Slot count must equal schema length.
    pub fn from_slots(schema: &[FieldSchema], slots: Vec<TypedValue>) -> Self {
        debug_assert_eq!(
            slots.len(),
            schema.len(),
            "output slot count must match schema"
        );
        Self {
            slots,
            schema_hash: compute_schema_hash(schema),
        }
    }

    /// Read the value at `field_id`.
    pub fn get(&self, field_id: FieldId) -> &TypedValue {
        &self.slots[field_id.0]
    }

    /// Look up an output value by field name.
    pub fn get_by_name<'a>(&'a self, schema: &[FieldSchema], name: &str) -> Option<&'a TypedValue> {
        let id = schema.iter().find(|f| f.name == name)?.field_id;
        Some(self.get(id))
    }

    /// Number of output slots (equals the output schema arity).
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// True when no output slots are present (unusual in practice).
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Iterate over output values in `FieldId` order.
    pub fn iter(&self) -> impl Iterator<Item = &TypedValue> {
        self.slots.iter()
    }
}
