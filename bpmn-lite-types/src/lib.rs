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

pub mod events;
pub mod ffi_bindings;
pub mod integrity;
pub mod session_stack;
pub mod types;

// Crate-prelude re-exports — every external consumer can `use
// bpmn_lite_types::*` and get the full vocabulary, mirroring the
// way `bpmn-lite-core` used to expose these via `pub mod`.
pub use events::*;
pub use ffi_bindings::*;
pub use types::*;
