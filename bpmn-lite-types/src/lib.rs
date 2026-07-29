//! BPMN-Lite domain model — leaf crate.
//!
//! Holds the value types every other bpmn-lite crate operates on:
//! scalar id aliases (`Addr`, `JoinId`, `WaitId`, `RaceId`, `FlagKey`,
//! `Timestamp`), the bytecode instruction set, `ProcessInstance`,
//! `Fiber`, `CompiledProgram`, `RuntimeEvent`, and the various wait /
//! activation / completion DTOs that flow between the engine and the
//! gRPC + persistence boundaries.
//!
//! Phase 2.1 (2026-05-14) migrated `types.rs` and `events.rs` here
//! from `bpmn-lite-core/src/{types,events}.rs`. Downstream crates
//! reach them as either `bpmn_lite_types::types::Foo` /
//! `bpmn_lite_types::events::Bar` (the modules) or
//! `bpmn_lite_types::Foo` / `Bar` (via the prelude `pub use`s
//! below).

pub mod artifact;
pub mod canonical;
pub mod concurrency;
pub mod events;
pub mod ffi_bindings;
pub mod integrity;
pub(crate) mod integrity_rings;
pub mod persistence;
pub mod session_stack;
pub mod transition;
pub mod types;
pub(crate) mod v2_verifier;

// Crate-prelude re-exports — every external consumer can `use
// bpmn_lite_types::*` and get the full vocabulary, mirroring the
// way `bpmn-lite-core` used to expose these via `pub mod`.
pub use artifact::*;
pub use canonical::{CanonicalDecodeError, CanonicalEncode, CanonicalReader, CanonicalWriter};
pub use concurrency::*;
pub use events::*;
pub use ffi_bindings::*;
pub use integrity::{compute_instance_integrity_hash, IntegrityViolation};
pub use integrity_rings::{IntegrityError, TripwireSurface};
pub use persistence::*;
pub use session_stack::*;
pub use transition::*;
pub use types::*;
pub use uuid::Uuid;
