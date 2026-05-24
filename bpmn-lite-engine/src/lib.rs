//! BPMN-Lite engine.
//!
//! The orchestration crate — wires `bpmn-lite-compiler` (compile),
//! `bpmn-lite-vm` (execute), and `bpmn-lite-store` (persist)
//! together behind a single `BpmnLiteEngine` facade. Owns the
//! process lifecycle API (`start`, `signal`, `cancel`, `inspect`,
//! `complete_job`, `fail_job`) and the background scheduler
//! (`tick_*`).
//!
//! Phase 2.7 (2026-05-14) migrated the engine here from
//! `bpmn-lite-core/src/engine/`. The locked decision from the
//! cleanup plan landed in this slice: **the engine→authoring edge
//! inverts**. `BpmnLiteEngine` no longer exposes `compile_from_dto`
//! / `compile_from_yaml` / `compile_and_publish` — those moved out
//! to `bpmn-lite-authoring::compile_from_dto` /
//! `compile_from_yaml` / `compile_and_publish` (free functions
//! that take a `ProcessStore` handle so they can still persist the
//! compiled program after lowering). The engine keeps only:
//!
//!   - `compile(bpmn_xml: &str)` — BPMN-XML path; depends only on
//!     `bpmn-lite-compiler`.
//!
//! …and the runtime API plus scheduler.

pub mod demo;
pub mod engine;
pub mod plan_walker;

pub use engine::*;

#[cfg(test)]
mod plan_walker_tests;
#[cfg(test)]
mod demo_integration_tests;

#[cfg(test)]
mod tests;
