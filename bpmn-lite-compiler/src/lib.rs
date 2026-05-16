//! BPMN-Lite compiler.
//!
//! Owns the pipeline that turns BPMN 2.0 XML (or a fed-in IR graph
//! from `bpmn-lite-authoring`'s YAML/DTO path) into a verified
//! `CompiledProgram` of bytecode the VM can execute.
//!
//! Phase 2.2 (2026-05-14) migrated `ir.rs`, `parser.rs`,
//! `lowering.rs`, and `verifier.rs` here as a cohesive unit from
//! `bpmn-lite-core/src/compiler/`. Submodules are `pub(crate)` —
//! consumers reach the surface through the prelude re-exports
//! below.

pub mod ir;
pub mod lowering;
pub mod parser;
pub mod verifier;

// Crate-prelude re-exports — flat access to the IR types + the
// parser / lowerer / verifier entry points. Downstream crates can
// either `use bpmn_lite_compiler::IRGraph` (flat) or
// `use bpmn_lite_compiler::ir::IRGraph` (module-qualified).
pub use ir::*;
pub use lowering::lower;
pub use parser::parse_bpmn;
pub use verifier::{verify, verify_bytecode, verify_or_err, VerifyError};
