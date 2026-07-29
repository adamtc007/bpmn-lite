#![no_main]

// EOP-FUZZ F8.5 — the DSL S-expression frontend under hostile bytes.
// The pipeline under test is `dsl::compile` (lex → parse → lint →
// validate_dag) followed by `lower_plan` (structured lowering →
// verify_program bytecode admission).
//
// Oracles:
//   D-O1 no-panic    — any byte sequence either compiles or rejects with
//                      a CompileError; a panic in lexer, parser, linter,
//                      DAG validation, lowering, or admission is the
//                      finding.
//   D-O2 gate parity — a plan `dsl::compile` ADMITS must also lower and
//                      pass bytecode admission. The two gates are
//                      supposed to be one pipeline; a plan the frontend
//                      blesses and the lowering rejects is a coherence
//                      finding between them (and exactly the seam the
//                      REST path currently stops halfway across).
//
// The registry is the demo-bindings stub: verbs/decisions the linter
// resolves, so grammar-valid sources can reach the later phases instead
// of dying at symbol resolution.

use std::sync::OnceLock;

use bpmn_lite_compiler::dsl::{lower_plan, StubPlaceholderRegistry};
use libfuzzer_sys::fuzz_target;

fn registry() -> &'static StubPlaceholderRegistry {
    static REGISTRY: OnceLock<StubPlaceholderRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| StubPlaceholderRegistry::new().with_demo_bindings())
}

fuzz_target!(|data: &[u8]| {
    let source = String::from_utf8_lossy(data);
    let Ok(plan) = bpmn_lite_compiler::dsl::compile(&source, registry()) else {
        return; // D-O1: rejection is the legal outcome for hostile bytes
    };
    if let Err(error) = lower_plan(&plan) {
        panic!(
            "D-O2: dsl::compile admitted a plan that lower_plan rejects: {error}\nsource:\n{source}"
        );
    }
});
