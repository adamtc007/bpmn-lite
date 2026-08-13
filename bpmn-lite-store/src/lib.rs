//! BPMN-Lite persistence boundary.
//!
//! Owns the four persistence capability traits that every storage backend
//! implements, plus the `MemoryStore` in-process default. The
//! PostgreSQL backend is a separate crate (`bpmn-lite-store-postgres`)
//! so binaries that don't need Postgres don't link sqlx.
//!
//! Phase 2.3 (2026-05-14) migrated `store.rs` and `store_memory.rs`
//! here from `bpmn-lite-core/src/`. Submodules are `pub mod` for
//! module-qualified access; the prelude re-exports the user-facing
//! types flat.

mod error;
pub mod pending;
pub mod store;
pub mod store_memory;

pub use error::{
    ArtifactStoreError, ClaimError, CommitError, CommitOutcome, StoreError, StoreResult,
};
// H3 (EOP-PLAN-CRATE-HYGIENE-001): the flat root re-export of `pending`'s
// items (`PendingInvocation`, `MemoryPendingInvocationStore`,
// `InsertOutcome`, `PendingInvocationStore`) was dead — every real
// cross-crate caller already used the module-qualified
// `bpmn_lite_store::pending::*` path exclusively (grep-confirmed). Removed;
// `pub mod pending;` above is the one canonical access path.
pub use store::*;
pub use store_memory::*;
