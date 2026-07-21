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

pub mod dsl;
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

use anyhow::{anyhow, Result};
use bpmn_lite_types::{ArtifactEnvelope, ExecutableWorkflow};
use dsl::frontend::{FrontendError, WorkflowFrontend};

/// Marker name used at compiler/runtime boundaries: only verifier-admitted
/// artifacts can be returned by the compiler.
pub type VerifiedWorkflow = ExecutableWorkflow;

pub struct Compiler;

pub struct XmlFrontend;

impl WorkflowFrontend for XmlFrontend {
    type Source = str;

    fn lower(source: &str) -> Result<VerifiedWorkflow, FrontendError> {
        let graph =
            parse_bpmn(source).map_err(|error| FrontendError::Artifact(error.to_string()))?;
        Compiler::lower(&graph).map_err(|error| FrontendError::Artifact(error.to_string()))
    }
}

impl Compiler {
    pub fn lower(graph: &IRGraph) -> Result<VerifiedWorkflow> {
        verify_or_err(graph)?;
        let legacy = lowering::lower(graph)?;
        let bytecode_errors = verify_bytecode(&legacy);
        if !bytecode_errors.is_empty() {
            let messages = bytecode_errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            return Err(anyhow!("Bytecode verification failed:\n{messages}"));
        }
        let envelope = ArtifactEnvelope::from_legacy_program(legacy, env!("CARGO_PKG_VERSION"))?;
        ExecutableWorkflow::from_verified_envelope(envelope).map_err(Into::into)
    }

    pub fn lower_dsl(
        plan: &dsl::plan::WorkflowExecutionPlan,
    ) -> std::result::Result<VerifiedWorkflow, FrontendError> {
        dsl::frontend::lower_plan(plan)
    }
}

#[cfg(test)]
mod artifact_tests {
    use super::*;

    const SIMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
      <bpmn:process id="verified" isExecutable="true">
        <bpmn:startEvent id="start" />
        <bpmn:endEvent id="end" />
        <bpmn:sequenceFlow id="flow" sourceRef="start" targetRef="end" />
      </bpmn:process>
    </bpmn:definitions>"#;

    fn verified_bytes() -> Vec<u8> {
        let graph = parse_bpmn(SIMPLE).unwrap();
        Compiler::lower(&graph).unwrap().canonical_bytes().unwrap()
    }

    #[test]
    fn complete_envelope_hash_is_stable_and_round_trips() {
        let bytes = verified_bytes();
        let first = ExecutableWorkflow::verify(&bytes).unwrap();
        let second = ExecutableWorkflow::verify(&bytes).unwrap();
        assert_eq!(first.hash(), second.hash());
        assert_eq!(first.canonical_bytes().unwrap(), bytes);
        assert!(first.envelope().limits().max_steps() > 0);
    }

    #[test]
    fn mutated_envelopes_are_rejected_without_panicking() {
        let bytes = verified_bytes();

        let truncated = &bytes[..bytes.len() - 1];
        assert!(ExecutableWorkflow::verify(truncated).is_err());

        let mut underflow: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        underflow["instructions"][0] = serde_json::json!("Pop");
        let underflow = serde_json::to_vec(&underflow).unwrap();
        assert!(ExecutableWorkflow::verify(&underflow).is_err());

        let mut looped: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        looped["instructions"][0] = serde_json::json!({ "Jump": { "target": 0 } });
        let looped = serde_json::to_vec(&looped).unwrap();
        assert!(ExecutableWorkflow::verify(&looped).is_err());

        let mut dangling: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        dangling["metadata"]["debug_map"] = serde_json::json!({ "999": "missing" });
        let dangling = serde_json::to_vec(&dangling).unwrap();
        assert!(ExecutableWorkflow::verify(&dangling).is_err());

        let mut false_limits: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        false_limits["limits"]["max_stack"] = serde_json::json!(999);
        let false_limits = serde_json::to_vec(&false_limits).unwrap();
        assert!(ExecutableWorkflow::verify(&false_limits).is_err());
    }
}
