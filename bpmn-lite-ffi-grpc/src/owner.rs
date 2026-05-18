//! `GrpcFfiOwner` — the `FfiExecutionOwner` implementation for gRPC.

use std::collections::HashMap;
use std::sync::RwLock;

use anyhow::Result;
use async_trait::async_trait;
use ffi_types::{
    FfiExecutionOwner, FfiTemplate, FieldSchema, Idempotency, compute_template_id,
    wire::{FfiCall, FfiIncidentClass, FfiResult},
};
use tonic::Code;

use crate::proto::FfiBridgeRequest;
use crate::proto::ffi_bridge_client::FfiBridgeClient;
use crate::template::GrpcTemplateConfig;

pub struct GrpcFfiOwner {
    templates: RwLock<HashMap<[u8; 32], GrpcTemplateConfig>>,
}

impl GrpcFfiOwner {
    pub fn new() -> Self {
        Self {
            templates: RwLock::new(HashMap::new()),
        }
    }

    /// Register a gRPC template. Returns the `FfiTemplate` for publication.
    pub fn register_template(
        &self,
        endpoint: String,
        timeout_ms: u64,
        input_schema: Vec<FieldSchema>,
        output_schema: Vec<FieldSchema>,
        idempotency: Idempotency,
        tenant_id: String,
        publisher: String,
    ) -> Result<FfiTemplate> {
        let owner_metadata = GrpcTemplateConfig::to_owner_metadata(&endpoint, timeout_ms)?;
        let config = GrpcTemplateConfig::from_owner_metadata(&owner_metadata)?;

        let mut template = FfiTemplate {
            template_id: [0u8; 32],
            owner_type: "grpc".to_string(),
            owner_metadata,
            input_schema,
            output_schema,
            idempotency,
            tenant_id,
            published_at: now_ms(),
            publisher,
        };
        template.template_id = compute_template_id(&template);

        self.templates
            .write()
            .expect("GrpcFfiOwner lock poisoned")
            .insert(template.template_id, config);

        Ok(template)
    }
}

impl Default for GrpcFfiOwner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FfiExecutionOwner for GrpcFfiOwner {
    fn owner_type(&self) -> &str {
        "grpc"
    }

    fn supports_template(&self, template_id: &[u8; 32]) -> bool {
        self.templates
            .read()
            .expect("lock")
            .contains_key(template_id)
    }

    async fn invoke(&self, call: FfiCall) -> Result<FfiResult> {
        let config = {
            let guard = self.templates.read().expect("lock");
            guard.get(&call.template_id).cloned()
        };
        let config = match config {
            Some(c) => c,
            None => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::ContractViolation,
                    message: format!(
                        "grpc template {:?} not registered",
                        hex::encode(call.template_id)
                    ),
                    retry_hint_ms: None,
                });
            }
        };

        // Connect and invoke with per-call timeout.
        let mut client = match FfiBridgeClient::connect(config.endpoint.clone()).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::Transient,
                    message: format!("gRPC connect failed to {}: {}", config.endpoint, e),
                    retry_hint_ms: Some(500),
                });
            }
        };

        let request = tonic::Request::new(FfiBridgeRequest {
            inputs_json: call.input_payload.clone(),
            invocation_id: call.invocation_id.to_string(),
        });
        // Apply timeout via tonic request metadata.
        let mut request = request;
        request.set_timeout(config.timeout);

        let response = match client.invoke(request).await {
            Ok(r) => r.into_inner(),
            Err(status) => return Ok(grpc_status_to_incident(status)),
        };

        // Empty outputs_json signals NoMatch.
        if response.outputs_json.is_empty() {
            let trace = trace_json(&config.endpoint, "no_match");
            return Ok(FfiResult::NoMatch {
                trace_payload: Some(trace),
            });
        }

        // Validate output is a JSON object.
        match serde_json::from_slice::<serde_json::Value>(&response.outputs_json) {
            Ok(v) if v.is_object() => {}
            Ok(v) => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::ContractViolation,
                    message: format!(
                        "gRPC outputs_json must be a JSON object, got {}",
                        json_type(&v)
                    ),
                    retry_hint_ms: None,
                });
            }
            Err(e) => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::ContractViolation,
                    message: format!("gRPC outputs_json is not valid JSON: {}", e),
                    retry_hint_ms: None,
                });
            }
        }

        let trace = trace_json(&config.endpoint, "success");
        Ok(FfiResult::Success {
            output_payload: response.outputs_json.to_vec(),
            trace_payload: trace,
            new_domain_payload: None,
        })
    }
}

fn grpc_status_to_incident(status: tonic::Status) -> FfiResult {
    let (error_class, retry_hint_ms) = match status.code() {
        Code::NotFound => (
            FfiIncidentClass::BusinessRejection {
                rejection_code: "GRPC_NOT_FOUND".to_string(),
            },
            None,
        ),
        Code::AlreadyExists | Code::Aborted => (
            FfiIncidentClass::BusinessRejection {
                rejection_code: "GRPC_CONFLICT".to_string(),
            },
            None,
        ),
        Code::InvalidArgument | Code::FailedPrecondition | Code::OutOfRange => {
            (FfiIncidentClass::ContractViolation, None)
        }
        Code::PermissionDenied | Code::Unauthenticated => {
            (FfiIncidentClass::ContractViolation, None)
        }
        Code::Unavailable | Code::DeadlineExceeded | Code::ResourceExhausted => {
            (FfiIncidentClass::Transient, Some(1000u64))
        }
        Code::Internal | Code::Unknown | Code::Unimplemented => {
            (FfiIncidentClass::Transient, Some(500u64))
        }
        _ => (FfiIncidentClass::ContractViolation, None),
    };
    FfiResult::Incident {
        error_class,
        message: format!("gRPC {}: {}", status.code(), status.message()),
        retry_hint_ms,
    }
}

fn trace_json(endpoint: &str, outcome: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "endpoint": endpoint,
        "outcome": outcome,
    }))
    .unwrap_or_default()
}

fn json_type(v: &serde_json::Value) -> &'static str {
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
