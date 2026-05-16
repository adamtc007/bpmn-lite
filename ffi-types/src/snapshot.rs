//! `FfiCatalogueSnapshot` — read-only catalogue view for the compile-time
//! verifier.
//!
//! Per A2 §11. The bpmn-lite compiler's verifier needs to look up templates
//! by id to validate input/output bindings against schemas. It does not
//! need the full `FfiCatalogue` machinery (no caching, no async). This
//! trait is the minimal surface.

use crate::template::FfiTemplate;

/// A snapshot of the FFI catalogue presented to the compiler.
///
/// Implementations:
/// - `FfiCatalogue` (in ffi-catalogue) implements this against its
///   in-memory cache.
/// - `MockFfiCatalogueSnapshot` (in test code) implements this against
///   a hand-constructed `HashMap`.
pub trait FfiCatalogueSnapshot: Send + Sync {
    fn lookup(&self, template_id: &[u8; 32]) -> Option<&FfiTemplate>;
}
