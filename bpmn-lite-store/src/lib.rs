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
pub use pending::{
    InsertOutcome as PendingInsertOutcome, MemoryPendingInvocationStore, PendingInvocation,
    PendingInvocationStore,
};
pub use store::*;
pub use store_memory::*;
