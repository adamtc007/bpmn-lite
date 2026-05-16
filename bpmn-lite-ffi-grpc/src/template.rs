//! gRPC template configuration — parsed form of `FfiTemplate.owner_metadata`.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Parsed, validated gRPC template config.
#[derive(Debug, Clone)]
pub struct GrpcTemplateConfig {
    pub endpoint: String,
    pub timeout: Duration,
}

#[derive(Debug, Serialize, Deserialize)]
struct OwnerMetadataWire {
    endpoint: String,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    5000
}

impl GrpcTemplateConfig {
    pub fn from_owner_metadata(bytes: &[u8]) -> Result<Self> {
        let wire: OwnerMetadataWire =
            serde_json::from_slice(bytes).context("owner_metadata is not valid JSON")?;
        if wire.endpoint.is_empty() {
            anyhow::bail!("owner_metadata.endpoint must not be empty");
        }
        Ok(Self {
            endpoint: wire.endpoint,
            timeout: Duration::from_millis(wire.timeout_ms.max(1)),
        })
    }

    pub fn to_owner_metadata(endpoint: &str, timeout_ms: u64) -> Result<Vec<u8>> {
        let wire = OwnerMetadataWire {
            endpoint: endpoint.to_string(),
            timeout_ms,
        };
        Ok(serde_json::to_vec(&wire)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid() {
        let meta = br#"{"endpoint":"http://host:50099"}"#;
        let cfg = GrpcTemplateConfig::from_owner_metadata(meta).unwrap();
        assert_eq!(cfg.endpoint, "http://host:50099");
        assert_eq!(cfg.timeout, Duration::from_millis(5000));
    }

    #[test]
    fn parse_custom_timeout() {
        let meta = br#"{"endpoint":"http://host:50099","timeout_ms":3000}"#;
        let cfg = GrpcTemplateConfig::from_owner_metadata(meta).unwrap();
        assert_eq!(cfg.timeout, Duration::from_millis(3000));
    }

    #[test]
    fn reject_empty_endpoint() {
        let meta = br#"{"endpoint":""}"#;
        let err = GrpcTemplateConfig::from_owner_metadata(meta).unwrap_err();
        assert!(err.to_string().contains("endpoint"));
    }
}
