//! FFI protocol types for the bpmn-lite Foreign Function Interface.
//!
//! This crate is the vocabulary-neutral contract surface specified by A2.
//! It carries no dependency on `bpmn-lite-types` or `dmn-lite-types`; every
//! type here describes the FFI boundary, not any specific vocabulary.
//!
//! Module map:
//!
//! - [`schema`] — `FieldSchema`, `SchemaKind` (vocabulary-neutral schema language)
//! - [`idempotency`] — `Idempotency` enum (replay/recovery discipline)
//! - [`template`] — `FfiTemplate`, `GLOBAL_TENANT_ID`
//! - [`wire`] — `FfiCall`, `FfiResult`, `FfiIncidentClass` (the call boundary)
//! - [`record`] — `ForeignFunctionInvocationRecord`, `FfiOutcomeKind` (audit)
//! - [`owner`] — `FfiExecutionOwner` trait (registered owners implement this)
//! - [`snapshot`] — `FfiCatalogueSnapshot` trait (read-only view for the verifier)
//! - [`canonical`] — `compute_template_id`: deterministic BLAKE3 hash per A2 §3
//!
//! All public surface re-exports through the crate root.

#![forbid(unsafe_code)]

pub mod canonical;
pub mod idempotency;
pub mod owner;
pub mod record;
pub mod schema;
pub mod snapshot;
pub mod template;
pub mod wire;

pub use canonical::compute_template_id;
pub use idempotency::Idempotency;
pub use owner::FfiExecutionOwner;
pub use record::{FfiOutcomeKind, ForeignFunctionInvocationRecord};
pub use schema::{FieldSchema, SchemaKind};
pub use snapshot::FfiCatalogueSnapshot;
pub use template::{FfiTemplate, GLOBAL_TENANT_ID};
pub use wire::{FfiCall, FfiIncidentClass, FfiResult};
