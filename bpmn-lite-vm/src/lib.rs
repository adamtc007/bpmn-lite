//! BPMN-Lite bytecode interpreter.
//!
//! Single-concern crate that owns the fiber dispatch + instruction
//! execution + wait/race/loop primitives. Kept separate from
//! `bpmn-lite-engine` (the orchestrator) so the interpreter's
//! surface area stays locked once the instruction set is stable.
//!
//! Phase 2.5 (2026-05-14) migrated `vm.rs` (2,010 LOC) here from
//! `bpmn-lite-core/src/`.

pub mod json_path;
pub mod vm;

pub use vm::*;
