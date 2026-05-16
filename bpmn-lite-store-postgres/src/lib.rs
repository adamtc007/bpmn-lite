//! BPMN-Lite PostgreSQL persistence.
//!
//! Owns `PostgresProcessStore` (impl of `ProcessStore`) and the
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

pub mod ffi_template_store;
pub mod store_postgres;

pub use ffi_template_store::PostgresFfiTemplateStore;
pub use store_postgres::*;
