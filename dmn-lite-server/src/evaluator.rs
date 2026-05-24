//! `DecisionEvaluator` implementation for the dmn-lite bus server.
//!
//! Translates bus-protocol bindings into the engine's `TypedInputContext`,
//! evaluates against a pre-verified decision via the stack VM, and
//! translates the resulting `TypedOutputContext` back into bus
//! `ResolvedBinding` rows.

use std::sync::Arc;

use async_trait::async_trait;
use dmn_lite_bus_handler::{
    DecisionEvaluator, DecisionEvaluatorError, DecisionOutcome,
};
use dmn_lite_engine::evaluate as engine_evaluate;
use dmn_lite_types::ir::{FieldSchema, ResolvedType, TypedValue};
use dmn_lite_types::values::TypedInputContextBuilder;
use dsl_bus_protocol::v1::{
    typed_value::Value as ProtoTypedValueKind, ExecutionOutcomeKind, ResolvedBinding,
    TypedValue as ProtoTypedValue,
};
use uuid::Uuid;

use crate::catalogue::{CatalogueEntry, DecisionCatalogue};

/// Real evaluator backed by a pre-built [`DecisionCatalogue`]. Cheap to
/// clone (`Arc<DecisionCatalogue>` under the hood).
pub(crate) struct CatalogueDecisionEvaluator {
    catalogue: Arc<DecisionCatalogue>,
}

impl CatalogueDecisionEvaluator {
    pub(crate) fn new(catalogue: Arc<DecisionCatalogue>) -> Self {
        Self { catalogue }
    }
}

#[async_trait]
impl DecisionEvaluator for CatalogueDecisionEvaluator {
    async fn evaluate(
        &self,
        local_decision_id: &str,
        _catalogue_version: &str,
        inputs: Vec<ResolvedBinding>,
    ) -> Result<DecisionOutcome, DecisionEvaluatorError> {
        let entry = self.catalogue.get(local_decision_id).ok_or_else(|| {
            DecisionEvaluatorError::UnknownDecision(local_decision_id.to_owned())
        })?;

        evaluate_entry(entry, inputs)
    }
}

fn evaluate_entry(
    entry: &CatalogueEntry,
    inputs: Vec<ResolvedBinding>,
) -> Result<DecisionOutcome, DecisionEvaluatorError> {
    let compiled = entry.verified.as_compiled();
    let input_ctx =
        build_input_context(&compiled.input_schema, &inputs).map_err(|msg| {
            DecisionEvaluatorError::Malformed(format!("{}: {msg}", compiled.name))
        })?;

    let output = engine_evaluate(&entry.verified, &input_ctx, &entry.source_text)
        .map_err(|e| {
            DecisionEvaluatorError::Internal(format!("{}: {e:?}", compiled.name))
        })?;

    let bindings = output_to_bindings(&compiled.output_schema, &output.output)?;

    Ok(DecisionOutcome {
        execution_id: Uuid::now_v7(),
        kind: ExecutionOutcomeKind::Committed,
        detail: format!("evaluated decision '{}' via dmn-lite stack VM", compiled.name),
        bindings,
    })
}

fn build_input_context(
    schema: &[FieldSchema],
    inputs: &[ResolvedBinding],
) -> Result<dmn_lite_types::TypedInputContext, String> {
    let mut builder = TypedInputContextBuilder::new(schema);
    for binding in inputs {
        let Some(value_msg) = binding.value.as_ref() else {
            return Err(format!("binding '{}' missing value", binding.name));
        };

        let field = schema
            .iter()
            .find(|f| f.name == binding.name)
            .ok_or_else(|| format!("unknown input field '{}'", binding.name))?;

        let typed = match (&field.field_type, value_msg.value.as_ref()) {
            (ResolvedType::Bool, Some(ProtoTypedValueKind::BoolValue(b))) => {
                TypedValue::Bool(*b)
            }
            (ResolvedType::Integer, Some(ProtoTypedValueKind::IntValue(n))) => {
                TypedValue::Integer(*n)
            }
            (ResolvedType::Decimal, Some(ProtoTypedValueKind::DoubleValue(d))) => {
                TypedValue::Decimal(*d)
            }
            (ResolvedType::Str, Some(ProtoTypedValueKind::StringValue(s))) => {
                TypedValue::Str(s.clone())
            }
            // Enum inputs arrive on the wire as strings (the symbol).
            // The dmn-lite-bridge crate has a richer resolver pathway
            // for in-process FFI; the bus contract is simpler — the
            // sender must already have resolved to a string symbol
                // that the engine then matches via const-pool lookup.
            (ResolvedType::Enum { .. }, Some(ProtoTypedValueKind::StringValue(s))) => {
                TypedValue::Str(s.clone())
            }
            (_, Some(ProtoTypedValueKind::NullValue(_))) => TypedValue::Null,
            (expected, Some(_)) => {
                return Err(format!(
                    "binding '{}' type mismatch — expected {}, got {}",
                    binding.name,
                    expected.type_name(),
                    proto_typed_value_kind(value_msg)
                ));
            }
            (_, None) => {
                return Err(format!("binding '{}' has empty value oneof", binding.name));
            }
        };

        builder
            .set_by_name(&binding.name, typed)
            .map_err(|e| format!("{e:?}"))?;
    }

    Ok(builder.build())
}

fn output_to_bindings(
    schema: &[FieldSchema],
    output: &dmn_lite_types::values::TypedOutputContext,
) -> Result<Vec<ResolvedBinding>, DecisionEvaluatorError> {
    let mut out = Vec::with_capacity(schema.len());
    for field in schema {
        let value = output.get(field.field_id);
        let proto = typed_to_proto(value, &field.field_type)?;
        out.push(ResolvedBinding {
            name: field.name.clone(),
            value: Some(proto),
        });
    }
    Ok(out)
}

fn typed_to_proto(
    value: &TypedValue,
    field_type: &ResolvedType,
) -> Result<ProtoTypedValue, DecisionEvaluatorError> {
    let (kind, type_name): (ProtoTypedValueKind, &'static str) = match value {
        TypedValue::Bool(b) => (ProtoTypedValueKind::BoolValue(*b), "bool"),
        TypedValue::Integer(n) => (ProtoTypedValueKind::IntValue(*n), "i64"),
        TypedValue::Decimal(d) => (ProtoTypedValueKind::DoubleValue(*d), "f64"),
        TypedValue::Str(s) => (ProtoTypedValueKind::StringValue(s.clone()), "string"),
        // Enum outputs surface as the resolved symbol (the engine
        // already mapped value_id → symbol via the decision's
        // const-pool). We don't have the symbol here, only the
        // identity; for v0.6 §6.3 this is acceptable because the
        // BPMN executor compares by string symbol set produced from
        // the dmn-lite manifest.
        TypedValue::Enum { value_id, .. } => (
            ProtoTypedValueKind::StringValue(format!("{value_id}")),
            "enum",
        ),
        TypedValue::Null => (ProtoTypedValueKind::NullValue(true), field_type.type_name()),
    };
    Ok(ProtoTypedValue {
        value: Some(kind),
        type_name: type_name.to_owned(),
    })
}

fn proto_typed_value_kind(value: &ProtoTypedValue) -> &'static str {
    match value.value.as_ref() {
        Some(ProtoTypedValueKind::StringValue(_)) => "string",
        Some(ProtoTypedValueKind::IntValue(_)) => "int",
        Some(ProtoTypedValueKind::DoubleValue(_)) => "double",
        Some(ProtoTypedValueKind::BoolValue(_)) => "bool",
        Some(ProtoTypedValueKind::UuidValue(_)) => "uuid",
        Some(ProtoTypedValueKind::BlobValue(_)) => "blob",
        Some(ProtoTypedValueKind::NullValue(_)) => "null",
        None => "<unset>",
    }
}
