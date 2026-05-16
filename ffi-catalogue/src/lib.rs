//! FFI template catalogue.
//!
//! Two layers:
//!
//! - [`store`] — `FfiTemplateStore` trait + `MemoryFfiTemplateStore`
//!   (in-memory backend; Postgres backend lives in
//!   `bpmn-lite-store-postgres`).
//! - [`catalogue`] — `FfiCatalogue`: cache-front over the store.
//!   Templates are loaded at startup; hot-path lookups are cache-only.
//!   Implements `FfiCatalogueSnapshot` so the compiler verifier can use it
//!   directly.

#![forbid(unsafe_code)]

pub mod catalogue;
pub mod store;

pub use catalogue::FfiCatalogue;
pub use store::{FfiTemplateStore, MemoryFfiTemplateStore};
