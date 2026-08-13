//! BPMN-Lite PostgreSQL persistence.
//!
//! Owns `PostgresWorkflowStore` (impl of `WorkflowStore`) and the
//! migration set that defines its schema. Separated from
//! `bpmn-lite-store` so a memory-only deployment (CI, integration
//! tests, smoke harnesses) doesn't link sqlx + the full Postgres
//! client.
//!
//! Phase 2.4 (2026-05-14) migrated `store_postgres.rs` and the
//! `migrations/` set here from `bpmn-lite-core/`. The
//! `sqlx::migrate!("./migrations")` invocation path is unchanged
//! — `./migrations` resolves to this crate's `migrations/` dir at
//! macro-expansion time, the same way it resolved to
//! `bpmn-lite-core/migrations/` before the move.

pub(crate) mod ffi_template_store;
mod pending_store;
pub(crate) mod store_postgres;

pub use ffi_template_store::PostgresFfiTemplateStore;
pub use pending_store::PostgresPendingInvocationStore;
pub use store_postgres::*;

#[cfg(test)]
mod test_lock {
    use std::sync::OnceLock;
    use tokio::sync::Mutex;

    static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    pub(crate) fn get_mutex() -> &'static Mutex<()> {
        TEST_MUTEX.get_or_init(|| Mutex::new(()))
    }
}
