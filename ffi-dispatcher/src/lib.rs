//! FFI dispatcher — routes `FfiCall` to the registered owner.
//!
//! Per A2 §2 / §5. The dispatcher holds:
//! - An `Arc<FfiCatalogue>` for template lookups.
//! - A registry of `owner_type` → `Arc<dyn FfiExecutionOwner>`.
//!
//! `dispatch(call)` looks up the template, finds the registered owner for
//! the template's `owner_type`, and forwards the call. Missing template
//! or missing owner is a hard error returned to the caller (the bpmn-lite
//! engine maps these to runtime incidents).
//!
//! Startup validation (`validate_coverage`) ensures every template in the
//! catalogue cache has a registered owner that claims to support its id.
//! Misconfiguration surfaces at startup, not at first call.

#![forbid(unsafe_code)]

use ffi_catalogue::FfiCatalogue;
use ffi_types::{FfiCall, FfiExecutionOwner, FfiResult};
use std::collections::HashMap;
use std::sync::Arc;

/// Routes FFI calls from the bpmn-lite engine to registered execution owners.
pub struct FfiDispatcher {
    catalogue: Arc<FfiCatalogue>,
    owners: HashMap<String, Arc<dyn FfiExecutionOwner>>,
}

impl FfiDispatcher {
    pub fn new(catalogue: Arc<FfiCatalogue>) -> Self {
        Self {
            catalogue,
            owners: HashMap::new(),
        }
    }

    /// Register an owner. Fails if the same `owner_type` is already
    /// registered (no overrides — registration is one-shot per startup).
    pub fn register_owner(
        &mut self,
        owner: Arc<dyn FfiExecutionOwner>,
    ) -> anyhow::Result<()> {
        let ot = owner.owner_type().to_string();
        if self.owners.contains_key(&ot) {
            anyhow::bail!("FFI owner already registered: {}", ot);
        }
        self.owners.insert(ot, owner);
        Ok(())
    }

    /// Return the registered owner for an `owner_type`, or `None`.
    pub fn lookup_owner(&self, owner_type: &str) -> Option<Arc<dyn FfiExecutionOwner>> {
        self.owners.get(owner_type).cloned()
    }

    /// True iff every template in the catalogue cache is supported by a
    /// registered owner with matching `owner_type`. Call after registering
    /// owners and loading the catalogue cache. Returns a list of
    /// `(template_id, owner_type, reason)` for every coverage gap; an empty
    /// list means startup is good.
    pub async fn validate_coverage(&self) -> Vec<CoverageGap> {
        let mut gaps = Vec::new();
        for t in self.catalogue.list_cached().await {
            match self.owners.get(&t.owner_type) {
                None => gaps.push(CoverageGap {
                    template_id: t.template_id,
                    owner_type: t.owner_type.clone(),
                    reason: GapReason::NoOwnerRegistered,
                }),
                Some(owner) => {
                    if !owner.supports_template(&t.template_id) {
                        gaps.push(CoverageGap {
                            template_id: t.template_id,
                            owner_type: t.owner_type.clone(),
                            reason: GapReason::OwnerRejectsTemplate,
                        });
                    }
                }
            }
        }
        gaps
    }

    /// Dispatch one call. Looks up the template, finds the owner, invokes.
    ///
    /// Errors:
    /// - template not in catalogue → `DispatchError::TemplateNotFound`
    /// - no owner registered for the template's owner_type → `DispatchError::OwnerNotRegistered`
    /// - owner returned a Rust-level error (panic, transport failure) → propagated up
    ///
    /// Business outcomes (`NoMatch`, `Incident`) are NOT dispatcher errors;
    /// they are returned inside the `FfiResult` as the owner produced them.
    pub async fn dispatch(&self, call: FfiCall) -> anyhow::Result<FfiResult> {
        let template = self
            .catalogue
            .lookup_cached(&call.template_id)
            .await
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "FFI template not found in catalogue cache: {}",
                    hex_short(&call.template_id)
                )
            })?;

        let owner = self.owners.get(&template.owner_type).ok_or_else(|| {
            anyhow::anyhow!(
                "no FFI owner registered for owner_type '{}' (template {})",
                template.owner_type,
                hex_short(&call.template_id)
            )
        })?;

        owner.invoke(call).await
    }

    /// Number of registered owners (observability).
    pub fn owner_count(&self) -> usize {
        self.owners.len()
    }

    /// Return the `owner_type` for a template cached in the catalogue, or
    /// `"unknown"` if the template is not in the cache.
    pub async fn owner_type_for(&self, template_id: &[u8; 32]) -> String {
        self.catalogue
            .lookup_cached(template_id)
            .await
            .map(|t| t.owner_type.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverageGap {
    pub template_id: [u8; 32],
    pub owner_type: String,
    pub reason: GapReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapReason {
    NoOwnerRegistered,
    OwnerRejectsTemplate,
}

fn hex_short(bytes: &[u8; 32]) -> String {
    // First 8 bytes as hex — enough for human-readable error messages.
    let mut s = String::with_capacity(16);
    for b in &bytes[..8] {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ffi_catalogue::{FfiTemplateStore, MemoryFfiTemplateStore};
    use ffi_types::{
        compute_template_id, FfiTemplate, FieldSchema, Idempotency, SchemaKind,
    };
    use uuid::Uuid;

    fn make_template(owner_type: &str) -> FfiTemplate {
        let mut t = FfiTemplate {
            template_id: [0u8; 32],
            owner_type: owner_type.to_string(),
            owner_metadata: vec![],
            input_schema: vec![FieldSchema {
                name: "x".to_string(),
                kind: SchemaKind::Bool,
                required: true,
            }],
            output_schema: vec![],
            idempotency: Idempotency::Idempotent,
            tenant_id: "tenant-a".to_string(),
            published_at: 0,
            publisher: "test".to_string(),
        };
        t.template_id = compute_template_id(&t);
        t
    }

    fn make_call(template_id: [u8; 32]) -> FfiCall {
        FfiCall {
            invocation_id: Uuid::now_v7(),
            template_id,
            tenant_id: "tenant-a".to_string(),
            process_instance_id: Uuid::now_v7(),
            caller_task_id: "T1".to_string(),
            input_payload: b"{}".to_vec(),
        }
    }

    /// A mock owner that always returns Success with a fixed output payload.
    struct MockOwner {
        owner_type: String,
    }

    #[async_trait]
    impl FfiExecutionOwner for MockOwner {
        fn owner_type(&self) -> &str {
            &self.owner_type
        }

        async fn invoke(&self, call: FfiCall) -> anyhow::Result<FfiResult> {
            // Echo the input back as output for easy assertion.
            Ok(FfiResult::Success {
                output_payload: call.input_payload.clone(),
                trace_payload: b"trace".to_vec(),
                new_domain_payload: None,
            })
        }

        fn supports_template(&self, _: &[u8; 32]) -> bool {
            true
        }
    }

    async fn setup(owner_types: &[&str]) -> (FfiDispatcher, Arc<FfiCatalogue>) {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let cat = Arc::new(FfiCatalogue::new(store));
        let mut disp = FfiDispatcher::new(cat.clone());
        for ot in owner_types {
            disp.register_owner(Arc::new(MockOwner {
                owner_type: (*ot).to_string(),
            }))
            .unwrap();
        }
        (disp, cat)
    }

    #[tokio::test]
    async fn register_owner_succeeds() {
        let (disp, _) = setup(&["dmn-lite"]).await;
        assert_eq!(disp.owner_count(), 1);
        assert!(disp.lookup_owner("dmn-lite").is_some());
    }

    #[tokio::test]
    async fn register_owner_rejects_duplicate() {
        let (mut disp, _) = setup(&["dmn-lite"]).await;
        let result = disp.register_owner(Arc::new(MockOwner {
            owner_type: "dmn-lite".to_string(),
        }));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already"));
    }

    #[tokio::test]
    async fn dispatch_routes_to_registered_owner() {
        // Build catalogue with one template; register matching owner.
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let t = make_template("dmn-lite");
        store.publish(&t).await.unwrap();
        let cat = Arc::new(FfiCatalogue::new(store));
        cat.load_into_cache("tenant-a").await.unwrap();

        let mut disp = FfiDispatcher::new(cat);
        disp.register_owner(Arc::new(MockOwner {
            owner_type: "dmn-lite".to_string(),
        }))
        .unwrap();

        let call = make_call(t.template_id);
        let result = disp.dispatch(call).await.unwrap();
        match result {
            FfiResult::Success { output_payload, .. } => {
                assert_eq!(output_payload, b"{}".to_vec());
            }
            _ => panic!("expected Success"),
        }
    }

    #[tokio::test]
    async fn dispatch_errors_when_template_not_in_cache() {
        let (disp, _) = setup(&["dmn-lite"]).await;
        let call = make_call([42u8; 32]);
        let err = disp.dispatch(call).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn validate_coverage_returns_empty_when_all_templates_covered() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let t1 = make_template("dmn-lite");
        let t2 = make_template("http");
        store.publish(&t1).await.unwrap();
        store.publish(&t2).await.unwrap();
        let cat = Arc::new(FfiCatalogue::new(store));
        cat.load_into_cache("tenant-a").await.unwrap();

        let mut disp = FfiDispatcher::new(cat);
        disp.register_owner(Arc::new(MockOwner {
            owner_type: "dmn-lite".to_string(),
        }))
        .unwrap();
        disp.register_owner(Arc::new(MockOwner {
            owner_type: "http".to_string(),
        }))
        .unwrap();

        let gaps = disp.validate_coverage().await;
        assert!(gaps.is_empty(), "expected no gaps, got: {:?}", gaps);
    }

    #[tokio::test]
    async fn validate_coverage_reports_missing_owner() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let t_dmn = make_template("dmn-lite");
        let t_http = make_template("http");
        store.publish(&t_dmn).await.unwrap();
        store.publish(&t_http).await.unwrap();
        let cat = Arc::new(FfiCatalogue::new(store));
        cat.load_into_cache("tenant-a").await.unwrap();

        let mut disp = FfiDispatcher::new(cat);
        // Register only dmn-lite; http is uncovered.
        disp.register_owner(Arc::new(MockOwner {
            owner_type: "dmn-lite".to_string(),
        }))
        .unwrap();

        let gaps = disp.validate_coverage().await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].owner_type, "http");
        assert_eq!(gaps[0].reason, GapReason::NoOwnerRegistered);
        assert_eq!(gaps[0].template_id, t_http.template_id);
    }

    /// An owner that rejects every template (`supports_template` returns false).
    struct StrictOwner;
    #[async_trait::async_trait]
    impl FfiExecutionOwner for StrictOwner {
        fn owner_type(&self) -> &str {
            "strict"
        }
        async fn invoke(&self, _: FfiCall) -> anyhow::Result<FfiResult> {
            anyhow::bail!("strict owner refuses to invoke")
        }
        fn supports_template(&self, _: &[u8; 32]) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn validate_coverage_reports_owner_rejection() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let t = make_template("strict");
        store.publish(&t).await.unwrap();
        let cat = Arc::new(FfiCatalogue::new(store));
        cat.load_into_cache("tenant-a").await.unwrap();

        let mut disp = FfiDispatcher::new(cat);
        disp.register_owner(Arc::new(StrictOwner)).unwrap();

        let gaps = disp.validate_coverage().await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].reason, GapReason::OwnerRejectsTemplate);
    }

    #[tokio::test]
    async fn dispatch_errors_when_no_owner_registered() {
        let store = Arc::new(MemoryFfiTemplateStore::new());
        let t = make_template("http"); // owner_type that we won't register
        store.publish(&t).await.unwrap();
        let cat = Arc::new(FfiCatalogue::new(store));
        cat.load_into_cache("tenant-a").await.unwrap();

        let mut disp = FfiDispatcher::new(cat);
        disp.register_owner(Arc::new(MockOwner {
            owner_type: "dmn-lite".to_string(),
        }))
        .unwrap();

        let call = make_call(t.template_id);
        let err = disp.dispatch(call).await.unwrap_err();
        assert!(err.to_string().contains("no FFI owner registered"));
        assert!(err.to_string().contains("http"));
    }
}
