//! `DmnLiteOwner` — the `FfiExecutionOwner` implementation for dmn-lite.

use crate::resolver::ValueResolver;
use async_trait::async_trait;
use dmn_lite_engine::evaluate;
use dmn_lite_types::EvalError;
use dmn_lite_types::compiled::VerifiedDecision;
use dmn_lite_types::ir::{FieldSchema, ResolvedType, TypedValue};
use dmn_lite_types::values::TypedInputContextBuilder;
use ffi_types::wire::{FfiCall, FfiIncidentClass, FfiResult};
use ffi_types::{
    FfiExecutionOwner, FfiTemplate, FieldSchema as FfiFieldSchema, Idempotency, compute_template_id,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// In-process FFI execution owner for dmn-lite decisions.
///
/// Thread-safe. Decisions are registered at startup and evaluated on every
/// `invoke` call with no re-compilation.
pub struct DmnLiteOwner {
    decisions: RwLock<HashMap<[u8; 32], Arc<VerifiedDecision>>>,
    resolver: Option<Arc<dyn ValueResolver>>,
}

impl DmnLiteOwner {
    pub fn new() -> Self {
        Self {
            decisions: RwLock::new(HashMap::new()),
            resolver: None,
        }
    }

    /// Attach a Sem OS symbol resolver for enum-domain input fields.
    pub fn with_resolver(mut self, resolver: Arc<dyn ValueResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Register a compiled decision and return the `FfiTemplate` for
    /// publication in the FFI catalogue.
    ///
    /// `owner_metadata` = the decision's 32-byte BLAKE3 `artifact_hash`.
    /// The `template_id` is therefore unique per compiled artifact.
    pub fn register_decision(
        &self,
        decision: VerifiedDecision,
        input_schema: Vec<FfiFieldSchema>,
        output_schema: Vec<FfiFieldSchema>,
        idempotency: Idempotency,
        tenant_id: String,
        publisher: String,
    ) -> FfiTemplate {
        let artifact_hash = decision.as_compiled().artifact_hash;
        let owner_metadata = artifact_hash.as_bytes().to_vec();

        let mut template = FfiTemplate {
            template_id: [0u8; 32],
            owner_type: "dmn-lite".to_string(),
            owner_metadata,
            input_schema,
            output_schema,
            idempotency,
            tenant_id,
            published_at: now_ms(),
            publisher,
        };
        template.template_id = compute_template_id(&template);

        self.decisions
            .write()
            .expect("DmnLiteOwner lock poisoned")
            .insert(template.template_id, Arc::new(decision));

        template
    }
}

impl Default for DmnLiteOwner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FfiExecutionOwner for DmnLiteOwner {
    fn owner_type(&self) -> &str {
        "dmn-lite"
    }

    fn supports_template(&self, template_id: &[u8; 32]) -> bool {
        self.decisions
            .read()
            .expect("DmnLiteOwner lock poisoned")
            .contains_key(template_id)
    }

    async fn invoke(&self, call: FfiCall) -> anyhow::Result<FfiResult> {
        let decision = {
            let guard = self.decisions.read().expect("DmnLiteOwner lock poisoned");
            match guard.get(&call.template_id) {
                Some(d) => Arc::clone(d),
                None => {
                    return Ok(FfiResult::Incident {
                        error_class: FfiIncidentClass::ContractViolation,
                        message: format!(
                            "dmn-lite-bridge: template {:02x}{:02x}... not registered",
                            call.template_id[0], call.template_id[1]
                        ),
                        retry_hint_ms: None,
                    });
                }
            }
        };

        let input_json: serde_json::Value = serde_json::from_slice(&call.input_payload)
            .map_err(|e| anyhow::anyhow!("invalid input_payload JSON: {}", e))?;

        let compiled = decision.as_compiled();
        let input_ctx = match build_input_context(
            &input_json,
            &compiled.typed_ir.input_schema,
            self.resolver.as_deref(),
        ) {
            Ok(ctx) => ctx,
            Err(e) => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::ContractViolation,
                    message: format!("input binding error: {}", e),
                    retry_hint_ms: None,
                });
            }
        };

        match evaluate(&decision, &input_ctx, "") {
            Ok(output) => {
                // Serialise output as JSON object keyed by output field name.
                let mut obj = serde_json::Map::new();
                for field in &compiled.typed_ir.output_schema {
                    let tv = output.output.get(field.field_id);
                    obj.insert(field.name.clone(), typed_value_to_json(tv));
                }
                let output_payload = serde_json::to_vec(&serde_json::Value::Object(obj))
                    .expect("output serialisation cannot fail");
                // EvaluationTrace doesn't derive Serialize; serialise as debug string.
                let trace_payload =
                    serde_json::to_vec(&format!("{:?}", output.trace)).unwrap_or_default();
                Ok(FfiResult::Success {
                    output_payload,
                    trace_payload,
                    new_domain_payload: None,
                })
            }
            Err(EvalError::NoMatch) => Ok(FfiResult::NoMatch {
                trace_payload: None,
            }),
            Err(e) => Ok(FfiResult::Incident {
                error_class: FfiIncidentClass::ContractViolation,
                message: format!("dmn-lite evaluation error: {}", e),
                retry_hint_ms: None,
            }),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_input_context(
    json: &serde_json::Value,
    schema: &[FieldSchema],
    resolver: Option<&dyn ValueResolver>,
) -> anyhow::Result<dmn_lite_types::values::TypedInputContext> {
    let mut builder = TypedInputContextBuilder::new(schema);
    for field in schema {
        let field_json = match json.get(&field.name) {
            Some(v) => v,
            None => continue, // absent → slot stays None (missing)
        };
        let typed_value = json_to_typed_value(field_json, field, resolver)
            .map_err(|e| anyhow::anyhow!("field '{}': {}", field.name, e))?;
        builder
            .set_by_name(&field.name, typed_value)
            .map_err(|e| anyhow::anyhow!("set field '{}': {}", field.name, e))?;
    }
    Ok(builder.build())
}

fn json_to_typed_value(
    v: &serde_json::Value,
    field: &FieldSchema,
    resolver: Option<&dyn ValueResolver>,
) -> anyhow::Result<TypedValue> {
    match &field.field_type {
        ResolvedType::Bool => match v {
            serde_json::Value::Bool(b) => Ok(TypedValue::Bool(*b)),
            other => anyhow::bail!("expected bool, got {}", type_name_json(other)),
        },
        ResolvedType::Integer => match v {
            serde_json::Value::Number(n) if n.is_i64() => {
                Ok(TypedValue::Integer(n.as_i64().unwrap()))
            }
            other => anyhow::bail!("expected integer, got {}", type_name_json(other)),
        },
        ResolvedType::Decimal => match v {
            serde_json::Value::Number(n) => {
                let f = n
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("cannot convert to f64"))?;
                Ok(TypedValue::Decimal(f))
            }
            other => anyhow::bail!("expected number, got {}", type_name_json(other)),
        },
        ResolvedType::Str => match v {
            serde_json::Value::String(s) => Ok(TypedValue::Str(s.clone())),
            other => anyhow::bail!("expected string, got {}", type_name_json(other)),
        },
        ResolvedType::Enum { domain_id } => {
            let symbol = match v {
                serde_json::Value::String(s) => s.as_str(),
                other => anyhow::bail!(
                    "expected string symbol for enum, got {}",
                    type_name_json(other)
                ),
            };
            // Try the ValueResolver; fall back to Str (which causes InputTypeMismatch
            // in dmn-lite — see module-level doc on SemOsDomain resolution).
            if let Some(tv) = resolver.and_then(|r| r.resolve(domain_id, symbol)) {
                return Ok(tv);
            }
            Ok(TypedValue::Str(symbol.to_string()))
        }
    }
}

fn typed_value_to_json(tv: &TypedValue) -> serde_json::Value {
    match tv {
        TypedValue::Bool(b) => serde_json::Value::Bool(*b),
        TypedValue::Integer(n) => serde_json::Value::Number((*n).into()),
        TypedValue::Decimal(f) => serde_json::json!(f),
        TypedValue::Str(s) => serde_json::Value::String(s.clone()),
        TypedValue::Enum { value_id, .. } => {
            // Emit the ValueId as a string; consumers resolve via Sem OS if needed.
            serde_json::Value::String(value_id.to_string())
        }
        TypedValue::Null => serde_json::Value::Null,
    }
}

fn type_name_json(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use dmn_lite_compiler::{Catalogue, compile_and_verify, load_catalogue_from_str};
    use dmn_lite_parser::parse;
    use ffi_types::{FieldSchema as FfiField, Idempotency, SchemaKind};

    const INT_CAT: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"
[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "integers"
"#;

    fn cat(s: &str) -> Catalogue {
        load_catalogue_from_str(s).expect("catalogue must load")
    }

    fn make_bool_decision(c: &Catalogue) -> VerifiedDecision {
        let src = r#"(define-decision eligible :hit-policy first
            :inputs  ((active :type bool :domain N))
            :outputs ((result :type bool :domain N))
            :rules   ((rule r001 :when ((active = true))  :then ((result = true)))
                      (rule r999 :when (*) :then ((result = false)))))"#;
        compile_and_verify(parse(src).expect("parse"), c, src).expect("compile_and_verify")
    }

    fn bool_schema(name: &str) -> FfiField {
        FfiField {
            name: name.to_string(),
            kind: SchemaKind::Bool,
            required: true,
        }
    }

    #[tokio::test]
    async fn register_and_invoke_true_input() {
        let c = cat(INT_CAT);
        let owner = DmnLiteOwner::new();
        let template = owner.register_decision(
            make_bool_decision(&c),
            vec![bool_schema("active")],
            vec![FfiField {
                name: "result".to_string(),
                kind: SchemaKind::Bool,
                required: false,
            }],
            Idempotency::Idempotent,
            "tenant-a".to_string(),
            "test".to_string(),
        );
        assert_eq!(template.owner_type, "dmn-lite");
        assert!(owner.supports_template(&template.template_id));

        let result = owner
            .invoke(FfiCall {
                invocation_id: uuid::Uuid::now_v7(),
                template_id: template.template_id,
                tenant_id: "tenant-a".to_string(),
                process_instance_id: uuid::Uuid::now_v7(),
                caller_task_id: "T1".to_string(),
                input_payload: b"{\"active\":true}".to_vec(),
            })
            .await
            .unwrap();

        match result {
            FfiResult::Success { output_payload, .. } => {
                let obj: serde_json::Value = serde_json::from_slice(&output_payload).unwrap();
                assert_eq!(obj["result"], serde_json::Value::Bool(true));
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn invoke_false_input_returns_false_via_catch_all() {
        let c = cat(INT_CAT);
        let owner = DmnLiteOwner::new();
        let template = owner.register_decision(
            make_bool_decision(&c),
            vec![bool_schema("active")],
            vec![FfiField {
                name: "result".to_string(),
                kind: SchemaKind::Bool,
                required: false,
            }],
            Idempotency::Idempotent,
            "tenant-a".to_string(),
            "test".to_string(),
        );
        let result = owner
            .invoke(FfiCall {
                invocation_id: uuid::Uuid::now_v7(),
                template_id: template.template_id,
                tenant_id: "tenant-a".to_string(),
                process_instance_id: uuid::Uuid::now_v7(),
                caller_task_id: "T1".to_string(),
                input_payload: b"{\"active\":false}".to_vec(),
            })
            .await
            .unwrap();

        match result {
            FfiResult::Success { output_payload, .. } => {
                let obj: serde_json::Value = serde_json::from_slice(&output_payload).unwrap();
                assert_eq!(obj["result"], serde_json::Value::Bool(false));
            }
            other => panic!("expected Success(false), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn no_match_decision_returns_no_match() {
        let c = cat(INT_CAT);
        // UNIQUE, no catch-all → false input → NoMatch.
        let src = r#"(define-decision t :hit-policy unique
            :inputs  ((active :type bool :domain N))
            :outputs ((result :type bool :domain N))
            :rules   ((rule r001 :when ((active = true)) :then ((result = true)))))"#;
        let decision = compile_and_verify(parse(src).expect("parse"), &c, src).expect("cv");

        let owner = DmnLiteOwner::new();
        let template = owner.register_decision(
            decision,
            vec![bool_schema("active")],
            vec![FfiField {
                name: "result".to_string(),
                kind: SchemaKind::Bool,
                required: false,
            }],
            Idempotency::Idempotent,
            "tenant-a".to_string(),
            "test".to_string(),
        );
        let result = owner
            .invoke(FfiCall {
                invocation_id: uuid::Uuid::now_v7(),
                template_id: template.template_id,
                tenant_id: "tenant-a".to_string(),
                process_instance_id: uuid::Uuid::now_v7(),
                caller_task_id: "T1".to_string(),
                input_payload: b"{\"active\":false}".to_vec(),
            })
            .await
            .unwrap();

        assert!(
            matches!(result, FfiResult::NoMatch { .. }),
            "got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn unknown_template_returns_incident() {
        let owner = DmnLiteOwner::new();
        let result = owner
            .invoke(FfiCall {
                invocation_id: uuid::Uuid::now_v7(),
                template_id: [7u8; 32],
                tenant_id: "t".to_string(),
                process_instance_id: uuid::Uuid::now_v7(),
                caller_task_id: "T1".to_string(),
                input_payload: b"{}".to_vec(),
            })
            .await
            .unwrap();

        assert!(
            matches!(
                result,
                FfiResult::Incident {
                    error_class: FfiIncidentClass::ContractViolation,
                    ..
                }
            ),
            "got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn same_source_same_catalogue_produces_same_template_id() {
        let c = cat(INT_CAT);
        let owner = DmnLiteOwner::new();
        let schema = vec![bool_schema("active")];
        let t1 = owner.register_decision(
            make_bool_decision(&c),
            schema.clone(),
            vec![],
            Idempotency::Idempotent,
            "t".to_string(),
            "t".to_string(),
        );
        let t2 = owner.register_decision(
            make_bool_decision(&c),
            schema,
            vec![],
            Idempotency::Idempotent,
            "t".to_string(),
            "t".to_string(),
        );
        assert_eq!(t1.template_id, t2.template_id);
    }

    #[tokio::test]
    async fn invalid_input_payload_returns_incident() {
        let c = cat(INT_CAT);
        let owner = DmnLiteOwner::new();
        let template = owner.register_decision(
            make_bool_decision(&c),
            vec![bool_schema("active")],
            vec![],
            Idempotency::Idempotent,
            "t".to_string(),
            "t".to_string(),
        );
        let result = owner
            .invoke(FfiCall {
                invocation_id: uuid::Uuid::now_v7(),
                template_id: template.template_id,
                tenant_id: "t".to_string(),
                process_instance_id: uuid::Uuid::now_v7(),
                caller_task_id: "T1".to_string(),
                input_payload: b"not-json{{".to_vec(),
            })
            .await
            .unwrap_err();

        assert!(result.to_string().contains("JSON"));
    }
}
