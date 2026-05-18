//! `HttpFfiOwner` — the `FfiExecutionOwner` implementation for HTTP.

// TODO(refactor): replace reqwest with hyper when streaming/HTTP2 needed.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Instant;

use anyhow::Result;
use async_trait::async_trait;
use ffi_types::{
    FfiExecutionOwner, FfiTemplate, FieldSchema, Idempotency, compute_template_id,
    wire::{FfiCall, FfiIncidentClass, FfiResult},
};

use crate::template::{HttpIdempotency, HttpMethod, HttpTemplateConfig};

pub struct HttpFfiOwner {
    // TODO(refactor): replace reqwest::Client with hyper when streaming/HTTP2 needed.
    client: reqwest::Client,
    templates: RwLock<HashMap<[u8; 32], HttpTemplateConfig>>,
}

impl HttpFfiOwner {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(64)
            .connect_timeout(std::time::Duration::from_secs(3))
            .user_agent("bpmn-lite-http-ffi/0.1")
            .build()
            .expect("reqwest client build failed");
        Self {
            client,
            templates: RwLock::new(HashMap::new()),
        }
    }

    /// Register an HTTP template. Returns the `FfiTemplate` for publication.
    pub fn register_template(
        &self,
        url: String,
        method: HttpMethod,
        static_headers: HashMap<String, String>,
        timeout_ms: u64,
        path_params: Vec<String>,
        success_status_codes: Vec<u16>,
        idempotency: HttpIdempotency,
        input_schema: Vec<FieldSchema>,
        output_schema: Vec<FieldSchema>,
        tenant_id: String,
        publisher: String,
    ) -> Result<FfiTemplate> {
        let owner_metadata = HttpTemplateConfig::to_owner_metadata(
            &url,
            &method,
            &static_headers,
            timeout_ms,
            &path_params,
            &success_status_codes,
        )?;

        let ffi_idempotency = match &idempotency {
            HttpIdempotency::Idempotent => Idempotency::Idempotent,
            HttpIdempotency::NonIdempotent => Idempotency::NonIdempotent,
            HttpIdempotency::IdempotentWithKey { selector } => Idempotency::IdempotentWithKey {
                selector: selector.clone(),
            },
        };

        let mut template = FfiTemplate {
            template_id: [0u8; 32],
            owner_type: "http".to_string(),
            owner_metadata,
            input_schema,
            output_schema,
            idempotency: ffi_idempotency,
            tenant_id,
            published_at: now_ms(),
            publisher,
        };
        template.template_id = compute_template_id(&template);

        let effective_idempotency = if matches!(
            idempotency,
            HttpIdempotency::Idempotent
                | HttpIdempotency::NonIdempotent
                | HttpIdempotency::IdempotentWithKey { .. }
        ) {
            idempotency
        } else {
            method.default_idempotency()
        };

        let config = HttpTemplateConfig::from_owner_metadata(
            &template.owner_metadata,
            effective_idempotency,
        )?;

        self.templates
            .write()
            .expect("HttpFfiOwner lock poisoned")
            .insert(template.template_id, config);

        Ok(template)
    }
}

impl Default for HttpFfiOwner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FfiExecutionOwner for HttpFfiOwner {
    fn owner_type(&self) -> &str {
        "http"
    }

    fn supports_template(&self, template_id: &[u8; 32]) -> bool {
        self.templates
            .read()
            .expect("lock poisoned")
            .contains_key(template_id)
    }

    async fn invoke(&self, call: FfiCall) -> Result<FfiResult> {
        let config = {
            let guard = self.templates.read().expect("lock poisoned");
            guard.get(&call.template_id).cloned()
        };

        let config = match config {
            Some(c) => c,
            None => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::ContractViolation,
                    message: format!(
                        "http template {:?} not registered in HttpFfiOwner",
                        hex::encode(call.template_id)
                    ),
                    retry_hint_ms: None,
                });
            }
        };

        let input: serde_json::Value = match serde_json::from_slice(&call.input_payload) {
            Ok(v) => v,
            Err(e) => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::ContractViolation,
                    message: format!("input_payload is not valid JSON: {}", e),
                    retry_hint_ms: None,
                });
            }
        };

        let input_obj = match input.as_object() {
            Some(o) => o.clone(),
            None => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::ContractViolation,
                    message: "input_payload must be a JSON object".to_string(),
                    retry_hint_ms: None,
                });
            }
        };

        // Build URL: substitute path params.
        let mut url = config.url.clone();
        let mut remaining_fields: serde_json::Map<String, serde_json::Value> = input_obj.clone();
        for param in &config.path_params {
            let placeholder = format!("{{{}}}", param);
            let value = match remaining_fields.remove(param) {
                Some(v) => url_encode_value(&v),
                None => {
                    return Ok(FfiResult::Incident {
                        error_class: FfiIncidentClass::ContractViolation,
                        message: format!("path_param '{}' not found in input_payload", param),
                        retry_hint_ms: None,
                    });
                }
            };
            url = url.replace(&placeholder, &value);
        }

        let started_at = Instant::now();

        // Build request.
        let mut request = self.client.request(config.method.as_reqwest(), &url);

        // Add static headers + Content-Type / Accept defaults.
        request = request
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
        for (k, v) in &config.static_headers {
            request = request.header(k.as_str(), v.as_str());
        }

        // Idempotency-Key header for IdempotentWithKey.
        if let HttpIdempotency::IdempotentWithKey { selector } = &config.idempotency {
            if let Some(key_value) = input_obj.get(selector.as_str()) {
                request = request.header(
                    "Idempotency-Key",
                    key_value.to_string().trim_matches('"').to_string(),
                );
            }
        }

        // Remaining fields: query string (GET) or JSON body (POST).
        request = match config.method {
            HttpMethod::Get => {
                let mut req = request;
                for (k, v) in &remaining_fields {
                    req = req.query(&[(k, url_encode_value(v))]);
                }
                req
            }
            HttpMethod::Post => request.json(&remaining_fields),
        };

        request = request.timeout(config.timeout);

        // Execute.
        let response = match request.send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::Transient,
                    message: format!(
                        "request timed out after {}ms: {}",
                        config.timeout.as_millis(),
                        e
                    ),
                    retry_hint_ms: Some(1000),
                });
            }
            Err(e) if e.is_connect() => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::Transient,
                    message: format!("connection refused: {}", e),
                    retry_hint_ms: Some(500),
                });
            }
            Err(e) => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::Transient,
                    message: format!("HTTP request failed: {}", e),
                    retry_hint_ms: Some(500),
                });
            }
        };

        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        let status = response.status().as_u16();

        // Read body.
        let body_bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::Transient,
                    message: format!("failed to read response body: {}", e),
                    retry_hint_ms: Some(500),
                });
            }
        };

        let trace = trace_json(&url, status, elapsed_ms);

        // Error mapping per B6 §6.
        if !config.is_success_status(status) {
            let excerpt = body_excerpt(&body_bytes);
            let error_class = match status {
                400 | 401 | 403 | 422 => FfiIncidentClass::ContractViolation,
                404 => FfiIncidentClass::BusinessRejection {
                    rejection_code: "HTTP_NOT_FOUND".to_string(),
                },
                409 => FfiIncidentClass::BusinessRejection {
                    rejection_code: "HTTP_CONFLICT".to_string(),
                },
                s if s >= 400 && s < 500 => FfiIncidentClass::ContractViolation,
                s if s >= 500 => FfiIncidentClass::Transient,
                _ => FfiIncidentClass::ContractViolation,
            };
            let retry_hint_ms = if matches!(error_class, FfiIncidentClass::Transient) {
                Some(1000u64)
            } else {
                None
            };
            return Ok(FfiResult::Incident {
                error_class,
                message: format!("HTTP {}: {}", status, excerpt),
                retry_hint_ms,
            });
        }

        // Parse response body as JSON.
        let body_ref: &[u8] = body_bytes.as_ref();
        if body_ref.is_empty() || body_ref == b"null" || body_ref == b"{}" {
            return Ok(FfiResult::NoMatch {
                trace_payload: Some(trace),
            });
        }

        let body: serde_json::Value = match serde_json::from_slice(body_ref) {
            Ok(v) => v,
            Err(e) => {
                return Ok(FfiResult::Incident {
                    error_class: FfiIncidentClass::ContractViolation,
                    message: format!("response body is not valid JSON: {}", e),
                    retry_hint_ms: None,
                });
            }
        };

        if body.is_null() || matches!(&body, serde_json::Value::Object(m) if m.is_empty()) {
            return Ok(FfiResult::NoMatch {
                trace_payload: Some(trace),
            });
        }

        if !body.is_object() {
            return Ok(FfiResult::Incident {
                error_class: FfiIncidentClass::ContractViolation,
                message: format!(
                    "response body must be a JSON object, got {}",
                    json_type_name(&body)
                ),
                retry_hint_ms: None,
            });
        }

        let output_payload = serde_json::to_vec(&body).unwrap_or_default();
        Ok(FfiResult::Success {
            output_payload,
            trace_payload: trace,
            new_domain_payload: None,
        })
    }
}

fn url_encode_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => {
            percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
        }
        other => other.to_string(),
    }
}

fn body_excerpt(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() > 256 {
        format!("{}...", &s[..256])
    } else {
        s.into_owned()
    }
}

fn trace_json(url: &str, status: u16, elapsed_ms: u64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "status": status,
        "url": url,
        "response_ms": elapsed_ms,
    }))
    .unwrap_or_default()
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
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
