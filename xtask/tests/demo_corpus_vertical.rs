//! Multi-crate application vertical: the §10 demo workflow's real,
//! checked-in manifests (`bpmn-lite-server-runner::demo::build_demo_plan`)
//! lowered and verified through `bpmn-lite-compiler`'s real v2 pipeline.
//! Moved from `bpmn-lite-engine/src/tests.rs` (EOP-PLAN-CRATE-HYGIENE-001,
//! H2) — `build_demo_plan`/`demo_initial_vars` relocated to
//! `bpmn-lite-server-runner::demo` (rest.rs's real demo-mode consumer),
//! so this corpus-sweep half of the V5.5 regression gate now belongs here,
//! not in the engine crate's own unit-test module.
//!
//! "All demo/test workflows recompiled; full verifier pass over the
//! recompiled corpus is itself a test" — a standing regression gate, not a
//! one-time manual claim. This is the only DSL corpus item compiled
//! against real, checked-in manifests
//! (`manifests/ob-poc-v1.0.0.yaml`/`manifests/dmn-lite-v1.0.0.yaml`), not a
//! hand-built in-test fixture. `build_demo_plan`'s own tests
//! (`bpmn-lite-server-runner/src/demo.rs`) already prove the T1
//! manifest-resolution pipeline; this test adds the missing link —
//! lowering that plan all the way to a verified `ExecutableWorkflow`,
//! which `build_demo_plan`'s own tests never do.

#[test]
fn corpus_sweep_demo_source_lowers_and_verifies() {
    let plan = bpmn_lite_server_runner::demo::build_demo_plan().expect("§10 demo must compile");
    bpmn_lite_compiler::Compiler::lower_dsl(&plan)
        .expect("§10 demo plan must lower and pass the full v2 verifier (V-1..V-11)");
}
