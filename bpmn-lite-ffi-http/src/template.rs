//! HTTP template configuration — the parsed form of `FfiTemplate.owner_metadata`.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// HTTP method supported for v0.1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    pub fn as_reqwest(&self) -> reqwest::Method {
        match self {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
        }
    }

    pub fn default_idempotency(&self) -> HttpIdempotency {
        match self {
            // GET is safe and idempotent by HTTP convention.
            HttpMethod::Get => HttpIdempotency::Idempotent,
            // POST is not idempotent by convention.
            HttpMethod::Post => HttpIdempotency::NonIdempotent,
        }
    }
}

/// Idempotency declaration for an HTTP template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpIdempotency {
    Idempotent,
    NonIdempotent,
    /// Advisory: passes the selector field value as `Idempotency-Key` header.
    IdempotentWithKey {
        selector: String,
    },
}

/// The parsed, validated form of `FfiTemplate.owner_metadata` for an HTTP template.
#[derive(Debug, Clone)]
pub struct HttpTemplateConfig {
    pub url: String,
    pub method: HttpMethod,
    pub static_headers: HashMap<String, String>,
    pub timeout: Duration,
    pub path_params: Vec<String>,
    pub success_status_codes: Vec<u16>,
    pub idempotency: HttpIdempotency,
}

/// Wire-format shape of the owner_metadata JSON (before validation).
#[derive(Debug, Serialize, Deserialize)]
struct OwnerMetadataWire {
    url: String,
    method: HttpMethod,
    #[serde(default)]
    static_headers: HashMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default)]
    path_params: Vec<String>,
    #[serde(default = "default_success_codes")]
    success_status_codes: Vec<u16>,
}

fn default_timeout_ms() -> u64 {
    5000
}

fn default_success_codes() -> Vec<u16> {
    vec![200]
}

impl HttpTemplateConfig {
    /// Parse and validate `owner_metadata` bytes (UTF-8 JSON).
    pub fn from_owner_metadata(bytes: &[u8], idempotency: HttpIdempotency) -> Result<Self> {
        let wire: OwnerMetadataWire =
            serde_json::from_slice(bytes).context("owner_metadata is not valid JSON")?;

        // Validate URL parses.
        let _ = reqwest::Url::parse(&wire.url)
            .with_context(|| format!("owner_metadata.url '{}' is not a valid URL", wire.url))?;

        // Validate method.
        match wire.method {
            HttpMethod::Get | HttpMethod::Post => {}
        }

        // Validate every path_param has a matching placeholder in the URL.
        for param in &wire.path_params {
            let placeholder = format!("{{{}}}", param);
            if !wire.url.contains(&placeholder) {
                bail!(
                    "path_param '{}' has no '{{{}}}' placeholder in url '{}'",
                    param,
                    param,
                    wire.url
                );
            }
        }

        if wire.success_status_codes.is_empty() {
            bail!("success_status_codes must not be empty");
        }

        Ok(Self {
            url: wire.url,
            method: wire.method,
            static_headers: wire.static_headers,
            timeout: Duration::from_millis(wire.timeout_ms.max(1)),
            path_params: wire.path_params,
            success_status_codes: wire.success_status_codes,
            idempotency,
        })
    }

    /// Serialise to canonical owner_metadata bytes (keys sorted, deterministic).
    pub fn to_owner_metadata(
        url: &str,
        method: &HttpMethod,
        static_headers: &HashMap<String, String>,
        timeout_ms: u64,
        path_params: &[String],
        success_status_codes: &[u16],
    ) -> Result<Vec<u8>> {
        let wire = OwnerMetadataWire {
            url: url.to_string(),
            method: method.clone(),
            static_headers: static_headers.clone(),
            timeout_ms,
            path_params: path_params.to_vec(),
            success_status_codes: success_status_codes.to_vec(),
        };
        // serde_json preserves insertion order in maps, not alphabetical.
        // Serialize to Value first, then use a BTreeMap to get sorted keys.
        let value = serde_json::to_value(wire)?;
        let sorted = sort_json_keys(value);
        Ok(serde_json::to_vec(&sorted)?)
    }

    pub fn is_success_status(&self, status: u16) -> bool {
        self.success_status_codes.contains(&status)
    }
}

fn sort_json_keys(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let sorted: serde_json::Map<String, Value> = map
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .map(|(k, v)| (k, sort_json_keys(v)))
                .collect();
            Value::Object(sorted)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_metadata(method: &str) -> Vec<u8> {
        format!(r#"{{"url":"http://host/api","method":"{}"}}"#, method).into_bytes()
    }

    #[test]
    fn parse_get_defaults_idempotent() {
        let cfg = HttpTemplateConfig::from_owner_metadata(
            &minimal_metadata("GET"),
            HttpIdempotency::Idempotent,
        )
        .unwrap();
        assert_eq!(cfg.method, HttpMethod::Get);
        assert_eq!(cfg.timeout, Duration::from_millis(5000));
        assert_eq!(cfg.success_status_codes, vec![200]);
        assert!(cfg.path_params.is_empty());
    }

    #[test]
    fn parse_post_with_path_param() {
        let meta =
            br#"{"url":"http://host/credit/{client_id}","method":"POST","path_params":["client_id"]}"#;
        let cfg =
            HttpTemplateConfig::from_owner_metadata(meta, HttpIdempotency::NonIdempotent).unwrap();
        assert_eq!(cfg.path_params, vec!["client_id"]);
    }

    #[test]
    fn reject_path_param_not_in_url() {
        let meta = br#"{"url":"http://host/api","method":"GET","path_params":["missing"]}"#;
        let err =
            HttpTemplateConfig::from_owner_metadata(meta, HttpIdempotency::Idempotent).unwrap_err();
        assert!(
            err.to_string().contains("has no"),
            "expected 'has no' in: {}",
            err
        );
    }

    #[test]
    fn reject_invalid_url() {
        let meta = br#"{"url":"not-a-url","method":"GET"}"#;
        let err =
            HttpTemplateConfig::from_owner_metadata(meta, HttpIdempotency::Idempotent).unwrap_err();
        assert!(err.to_string().contains("valid URL"));
    }

    #[test]
    fn canonical_owner_metadata_is_sorted() {
        let bytes = HttpTemplateConfig::to_owner_metadata(
            "http://host/api",
            &HttpMethod::Post,
            &HashMap::new(),
            3000,
            &[],
            &[200],
        )
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        // Keys should appear in alphabetical order: method, path_params, static_headers, ...
        let method_pos = text.find("method").unwrap();
        let url_pos = text.find("url").unwrap();
        // 'method' < 'url' alphabetically
        assert!(method_pos < url_pos, "keys not sorted: {}", text);
    }
}
