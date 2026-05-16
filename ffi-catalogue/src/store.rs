//! Backend-agnostic FFI template store.
//!
//! Per A2 §6. Templates are content-addressed (by `template_id` BLAKE3
//! digest) and **immutable after publication**. The in-memory backend is
//! authoritative for tests; the Postgres backend lives in
//! `bpmn-lite-store-postgres`.

use async_trait::async_trait;
use ffi_types::FfiTemplate;
use std::collections::HashMap;
use std::sync::RwLock;

/// Persistence trait for FFI templates.
///
/// Implementations: [`MemoryFfiTemplateStore`] (this crate),
/// `PostgresFfiTemplateStore` (in `bpmn-lite-store-postgres`).
#[async_trait]
pub trait FfiTemplateStore: Send + Sync {
    /// Publish a new template.
    ///
    /// Fails if `template.template_id` already exists with **different**
    /// content (immutability guard). Identical content is idempotent
    /// (no-op success).
    async fn publish(&self, template: &FfiTemplate) -> anyhow::Result<()>;

    async fn lookup(
        &self,
        template_id: &[u8; 32],
    ) -> anyhow::Result<Option<FfiTemplate>>;

    /// List all templates for the given tenant. Includes the GLOBAL tenant
    /// implicitly only if the caller passes [`ffi_types::GLOBAL_TENANT_ID`].
    /// Callers wanting tenant+GLOBAL union must merge two calls.
    async fn list_by_tenant(&self, tenant_id: &str) -> anyhow::Result<Vec<FfiTemplate>>;

    /// List all templates for the given (owner_type, tenant) pair.
    async fn list_by_owner(
        &self,
        owner_type: &str,
        tenant_id: &str,
    ) -> anyhow::Result<Vec<FfiTemplate>>;
}

/// In-memory `FfiTemplateStore`. Suitable for tests and bootstrapping;
/// production deployments use the Postgres backend.
pub struct MemoryFfiTemplateStore {
    inner: RwLock<HashMap<[u8; 32], FfiTemplate>>,
}

impl MemoryFfiTemplateStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Number of stored templates (test helper).
    pub fn len(&self) -> usize {
        self.inner.read().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for MemoryFfiTemplateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FfiTemplateStore for MemoryFfiTemplateStore {
    async fn publish(&self, template: &FfiTemplate) -> anyhow::Result<()> {
        let mut guard = self
            .inner
            .write()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?;
        if let Some(existing) = guard.get(&template.template_id) {
            if existing != template {
                anyhow::bail!(
                    "FFI template {} already published with different content (immutability guard)",
                    hex(&template.template_id)
                );
            }
            // Identical content → idempotent no-op.
            return Ok(());
        }
        guard.insert(template.template_id, template.clone());
        Ok(())
    }

    async fn lookup(
        &self,
        template_id: &[u8; 32],
    ) -> anyhow::Result<Option<FfiTemplate>> {
        let guard = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?;
        Ok(guard.get(template_id).cloned())
    }

    async fn list_by_tenant(
        &self,
        tenant_id: &str,
    ) -> anyhow::Result<Vec<FfiTemplate>> {
        let guard = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?;
        Ok(guard
            .values()
            .filter(|t| t.tenant_id == tenant_id)
            .cloned()
            .collect())
    }

    async fn list_by_owner(
        &self,
        owner_type: &str,
        tenant_id: &str,
    ) -> anyhow::Result<Vec<FfiTemplate>> {
        let guard = self
            .inner
            .read()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?;
        Ok(guard
            .values()
            .filter(|t| t.owner_type == owner_type && t.tenant_id == tenant_id)
            .cloned()
            .collect())
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffi_types::{
        compute_template_id, FfiTemplate, FieldSchema, Idempotency, SchemaKind,
        GLOBAL_TENANT_ID,
    };

    fn make_template(owner_type: &str, tenant_id: &str, marker: u8) -> FfiTemplate {
        let mut t = FfiTemplate {
            template_id: [0u8; 32],
            owner_type: owner_type.to_string(),
            owner_metadata: vec![marker],
            input_schema: vec![FieldSchema {
                name: "x".to_string(),
                kind: SchemaKind::Bool,
                required: true,
            }],
            output_schema: vec![],
            idempotency: Idempotency::Idempotent,
            tenant_id: tenant_id.to_string(),
            published_at: 0,
            publisher: "test".to_string(),
        };
        t.template_id = compute_template_id(&t);
        t
    }

    #[tokio::test]
    async fn publish_then_lookup_roundtrip() {
        let store = MemoryFfiTemplateStore::new();
        let t = make_template("dmn-lite", "tenant-a", 1);
        store.publish(&t).await.unwrap();
        let got = store.lookup(&t.template_id).await.unwrap().unwrap();
        assert_eq!(got, t);
    }

    #[tokio::test]
    async fn lookup_missing_returns_none() {
        let store = MemoryFfiTemplateStore::new();
        let got = store.lookup(&[42u8; 32]).await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn publish_idempotent_for_identical_content() {
        let store = MemoryFfiTemplateStore::new();
        let t = make_template("dmn-lite", "tenant-a", 1);
        store.publish(&t).await.unwrap();
        // Second publish with identical content succeeds.
        store.publish(&t).await.unwrap();
        assert_eq!(store.len(), 1);
    }

    #[tokio::test]
    async fn publish_rejects_different_content_at_same_id() {
        let store = MemoryFfiTemplateStore::new();
        let mut t = make_template("dmn-lite", "tenant-a", 1);
        store.publish(&t).await.unwrap();
        // Forge a template with the same id but different content (simulates
        // a misbehaving caller — production code always recomputes the id).
        t.publisher = "different".to_string();
        // tenant/published_at/publisher don't change template_id, but the
        // store compares FULL content for the immutability guard, not just
        // the canonical-encoding identity. So this DOES trip the guard.
        let result = store.publish(&t).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("immutability"));
    }

    #[tokio::test]
    async fn list_by_tenant_isolates() {
        let store = MemoryFfiTemplateStore::new();
        let a1 = make_template("dmn-lite", "tenant-a", 1);
        let a2 = make_template("http", "tenant-a", 2);
        let b1 = make_template("dmn-lite", "tenant-b", 3);
        store.publish(&a1).await.unwrap();
        store.publish(&a2).await.unwrap();
        store.publish(&b1).await.unwrap();

        let a_set = store.list_by_tenant("tenant-a").await.unwrap();
        assert_eq!(a_set.len(), 2);
        let b_set = store.list_by_tenant("tenant-b").await.unwrap();
        assert_eq!(b_set.len(), 1);
    }

    #[tokio::test]
    async fn list_by_owner_filters_correctly() {
        let store = MemoryFfiTemplateStore::new();
        store
            .publish(&make_template("dmn-lite", "tenant-a", 1))
            .await
            .unwrap();
        store
            .publish(&make_template("http", "tenant-a", 2))
            .await
            .unwrap();
        store
            .publish(&make_template("dmn-lite", "tenant-b", 3))
            .await
            .unwrap();

        let dmn = store.list_by_owner("dmn-lite", "tenant-a").await.unwrap();
        assert_eq!(dmn.len(), 1);
        assert_eq!(dmn[0].owner_type, "dmn-lite");
        assert_eq!(dmn[0].tenant_id, "tenant-a");
    }

    #[tokio::test]
    async fn list_includes_only_explicit_tenant_not_global() {
        // Per the trait contract: callers wanting tenant+GLOBAL union must
        // call twice. Verify the store does NOT silently fold GLOBAL in.
        let store = MemoryFfiTemplateStore::new();
        store
            .publish(&make_template("dmn-lite", "tenant-a", 1))
            .await
            .unwrap();
        store
            .publish(&make_template("dmn-lite", GLOBAL_TENANT_ID, 2))
            .await
            .unwrap();
        let a = store.list_by_tenant("tenant-a").await.unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].tenant_id, "tenant-a");
    }
}
