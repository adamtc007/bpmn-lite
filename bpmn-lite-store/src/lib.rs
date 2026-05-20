//! BPMN-Lite persistence boundary.
//!
//! Owns the `ProcessStore` async trait that every storage backend
//! implements, plus the `MemoryProcessStore` in-process default. The
//! PostgreSQL backend is a separate crate (`bpmn-lite-store-postgres`)
//! so binaries that don't need Postgres don't link sqlx.
//!
//! Phase 2.3 (2026-05-14) migrated `store.rs` and `store_memory.rs`
//! here from `bpmn-lite-core/src/`. Submodules are `pub mod` for
//! module-qualified access; the prelude re-exports the user-facing
//! types flat.

pub mod pending;
pub mod process_instance;
pub mod store;
pub mod store_memory;

pub use pending::{
    InsertOutcome as PendingInsertOutcome, MemoryPendingInvocationStore, PendingInvocation,
    PendingInvocationStore,
};
pub use process_instance::{
    BpmnProcessInstance, BpmnProcessInstanceStore, MemoryBpmnProcessInstanceStore, ProcessStatus,
};
pub use store::*;
pub use store_memory::*;
