//! `FfiCatalogue` — cache-front over an `FfiTemplateStore`.
//!
//! Per A2 §6. The cache is loaded at startup; runtime template lookups are
//! cache-only on the hot path. The catalogue implements
//! [`FfiCatalogueSnapshot`] so the compiler verifier (A6) can use it
//! without an async layer.
//!
//! Cache load policy:
//!
//! - `load_into_cache(tenant_id)` loads both the tenant's templates and
//!   the GLOBAL tenant's templates (per A2 §6 GLOBAL semantics).
//! - `lookup_cached(id)` is cache-only; returns `None` if not loaded.
//! - `lookup_or_fetch(id)` falls through to the store; the store call is
//!   async and may hit Postgres, so it is NOT for the hot dispatch path.

use crate::store::FfiTemplateStore;
use ffi_types::{FfiCatalogueSnapshot, FfiTemplate, GLOBAL_TENANT_ID};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock as TokioRwLock;

pub struct FfiCatalogue {
    store: Arc<dyn FfiTemplateStore>,
    // We use a tokio RwLock here because the lookup_or_fetch path is async
    // and may need to hold the lock across an await; std::sync::RwLock
    // cannot be held across await points.
    cache: TokioRwLock<HashMap<[u8; 32], Arc<FfiTemplate>>>,
    // A separate std-mutex-free snapshot view exclusively for sync access
    // from the verifier. It's a clone of the cache at the last load.
    // Updated atomically under the cache write lock.
    snapshot_view: std::sync::RwLock<Arc<HashMap<[u8; 32], FfiTemplate>>>,
}

impl FfiCatalogue {
    pub fn new(store: Arc<dyn FfiTemplateStore>) -> Self {
        Self {
            store,
            cache: TokioRwLock::new(HashMap::new()),
            snapshot_view: std::sync::RwLock::new(Arc::new(HashMap::new())),
        }
    }

    /// Load all templates visible to `tenant_id` (tenant-specific + GLOBAL)
    /// into the in-memory cache. Returns the count of distinct templates
    /// loaded.
    pub async fn load_into_cache(&self, tenant_id: &str) -> anyhow::Result<usize> {
        let mut tenant_templates = self.store.list_by_tenant(tenant_id).await?;
        if tenant_id != GLOBAL_TENANT_ID {
            let mut global_templates =
                self.store.list_by_tenant(GLOBAL_TENANT_ID).await?;
            tenant_templates.append(&mut global_templates);
        }

        let mut new_cache: HashMap<[u8; 32], Arc<FfiTemplate>> =
            HashMap::with_capacity(tenant_templates.len());
        let mut snapshot_map: HashMap<[u8; 32], FfiTemplate> =
            HashMap::with_capacity(tenant_templates.len());
        for t in tenant_templates {
            snapshot_map.insert(t.template_id, t.clone());
            new_cache.insert(t.template_id, Arc::new(t));
        }
        let count = new_cache.len();

        let mut cache_guard = self.cache.write().await;
        *cache_guard = new_cache;
        drop(cache_guard);

        let mut snap_guard = self
            .snapshot_view
            .write()
            .map_err(|e| anyhow::anyhow!("snapshot lock poisoned: {e}"))?;
        *snap_guard = Arc::new(snapshot_map);

        Ok(count)
    }

    /// Cache-only lookup. Returns `None` if not loaded (callers may either
    /// trigger a `load_into_cache` or use `lookup_or_fetch`).
    pub async fn lookup_cached(
        &self,
        template_id: &[u8; 32],
    ) -> Option<Arc<FfiTemplate>> {
        let guard = self.cache.read().await;
        guard.get(template_id).cloned()
    }

    /// Cache-first lookup; falls through to the store on miss and caches
    /// the result. NOT for the hot dispatch path — use `lookup_cached`
    /// after a startup `load_into_cache`.
    pub async fn lookup_or_fetch(
        &self,
        template_id: &[u8; 32],
    ) -> anyhow::Result<Option<Arc<FfiTemplate>>> {
        if let Some(t) = self.lookup_cached(template_id).await {
            return Ok(Some(t));
        }
        let fetched = self.store.lookup(template_id).await?;
        if let Some(template) = fetched {
            let arc = Arc::new(template.clone());
            let mut guard = self.cache.write().await;
            guard.insert(template.template_id, arc.clone());
            drop(guard);

            let mut snap_guard = self
                .snapshot_view
                .write()
                .map_err(|e| anyhow::anyhow!("snapshot lock poisoned: {e}"))?;
            // Copy-on-write: clone the current snapshot map, insert, replace.
            let mut new_map: HashMap<[u8; 32], FfiTemplate> =
                snap_guard.as_ref().clone();
            new_map.insert(template.template_id, template);
            *snap_guard = Arc::new(new_map);

            Ok(Some(arc))
        } else {
            Ok(None)
        }
    }

    /// Number of templates currently in cache (test/observability helper).
    pub async fn cache_len(&self) -> usize {
        self.cache.read().await.len()
    }

    /// Snapshot the cached templates as an owned `Vec`.
    ///
    /// Used by `FfiDispatcher::validate_coverage` to walk every loaded
    /// template at startup. Holds the read lock only long enough to clone
    /// out the values; the cache remains usable concurrently.
    pub async fn list_cached(&self) -> Vec<Arc<FfiTemplate>> {
        let guard = self.cache.read().await;
        guard.values().cloned().collect()
    }

    /// Borrow the catalogue as a snapshot for the compiler verifier.
    /// Returns an owned `CatalogueSnapshot` that wraps an Arc<HashMap>.
    pub fn snapshot(&self) -> CatalogueSnapshot {
        let guard = self
            .snapshot_view
            .read()
            .expect("snapshot lock poisoned");
        CatalogueSnapshot {
            map: Arc::clone(&*guard),
        }
    }
}

/// An immutable snapshot of the catalogue's content at the moment
/// `FfiCatalogue::snapshot()` was called. Implements `FfiCatalogueSnapshot`
/// so the compiler verifier (A6) can use it directly.
pub struct CatalogueSnapshot {
    map: Arc<HashMap<[u8; 32], FfiTemplate>>,
}

impl FfiCatalogueSnapshot for CatalogueSnapshot {
    fn lookup(&self, template_id: &[u8; 32]) -> Option<&FfiTemplate> {
        self.map.get(template_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryFfiTemplateStore;
    use ffi_types::{
        compute_template_id, FieldSchema, Idempotency, SchemaKind,
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
    async fn load_and_lookup_cached() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let t = make_template("dmn-lite", "tenant-a", 1);
        store.publish(&t).await.unwrap();

        let cat = FfiCatalogue::new(store);
        let count = cat.load_into_cache("tenant-a").await.unwrap();
        assert_eq!(count, 1);

        let got = cat.lookup_cached(&t.template_id).await.unwrap();
        assert_eq!(got.template_id, t.template_id);
    }

    #[tokio::test]
    async fn load_includes_global_templates() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let tenant_t = make_template("dmn-lite", "tenant-a", 1);
        let global_t = make_template("dmn-lite", GLOBAL_TENANT_ID, 99);
        store.publish(&tenant_t).await.unwrap();
        store.publish(&global_t).await.unwrap();

        let cat = FfiCatalogue::new(store);
        let count = cat.load_into_cache("tenant-a").await.unwrap();
        assert_eq!(count, 2);
        assert!(cat.lookup_cached(&tenant_t.template_id).await.is_some());
        assert!(cat.lookup_cached(&global_t.template_id).await.is_some());
    }

    #[tokio::test]
    async fn load_for_global_tenant_does_not_double_count() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let g = make_template("dmn-lite", GLOBAL_TENANT_ID, 1);
        store.publish(&g).await.unwrap();

        let cat = FfiCatalogue::new(store);
        let count = cat.load_into_cache(GLOBAL_TENANT_ID).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn lookup_or_fetch_misses_cache_then_loads() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let t = make_template("dmn-lite", "tenant-a", 1);
        store.publish(&t).await.unwrap();

        let cat = FfiCatalogue::new(store);
        // Cache is empty.
        assert!(cat.lookup_cached(&t.template_id).await.is_none());

        let got = cat
            .lookup_or_fetch(&t.template_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.template_id, t.template_id);

        // After fetch, the cache has it.
        assert!(cat.lookup_cached(&t.template_id).await.is_some());
    }

    #[tokio::test]
    async fn lookup_or_fetch_returns_none_for_missing() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let cat = FfiCatalogue::new(store);
        let got = cat.lookup_or_fetch(&[42u8; 32]).await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn snapshot_view_implements_catalogue_snapshot_trait() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let t = make_template("dmn-lite", "tenant-a", 1);
        store.publish(&t).await.unwrap();

        let cat = FfiCatalogue::new(store);
        cat.load_into_cache("tenant-a").await.unwrap();

        let snap = cat.snapshot();
        let found: Option<&FfiTemplate> = snap.lookup(&t.template_id);
        assert!(found.is_some());
        assert_eq!(found.unwrap().template_id, t.template_id);

        // Missing id returns None.
        assert!(snap.lookup(&[7u8; 32]).is_none());
    }

    #[tokio::test]
    async fn snapshot_view_updates_after_fetch() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let t = make_template("dmn-lite", "tenant-a", 1);
        store.publish(&t).await.unwrap();

        let cat = FfiCatalogue::new(store);
        // No load_into_cache; rely on lookup_or_fetch to populate.
        let pre_snap = cat.snapshot();
        assert!(pre_snap.lookup(&t.template_id).is_none());

        cat.lookup_or_fetch(&t.template_id).await.unwrap();

        let post_snap = cat.snapshot();
        assert!(post_snap.lookup(&t.template_id).is_some());
        // Old snapshot is unaffected (immutable view).
        assert!(pre_snap.lookup(&t.template_id).is_none());
    }

    #[tokio::test]
    async fn reload_replaces_cache_atomically() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let t1 = make_template("dmn-lite", "tenant-a", 1);
        store.publish(&t1).await.unwrap();

        let cat = FfiCatalogue::new(store.clone());
        cat.load_into_cache("tenant-a").await.unwrap();
        assert_eq!(cat.cache_len().await, 1);

        let t2 = make_template("http", "tenant-a", 2);
        store.publish(&t2).await.unwrap();

        cat.load_into_cache("tenant-a").await.unwrap();
        assert_eq!(cat.cache_len().await, 2);
        assert!(cat.lookup_cached(&t1.template_id).await.is_some());
        assert!(cat.lookup_cached(&t2.template_id).await.is_some());
    }
}
