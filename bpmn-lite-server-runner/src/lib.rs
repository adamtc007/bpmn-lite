//! BPMN-Lite gRPC server library.
//!
//! Re-exports the proto-generated types so integration tests and external
//! crates can build gRPC clients without duplicating proto compilation.

// Moved from bpmn-lite-engine (H2, EOP-PLAN-CRATE-HYGIENE-001): rest.rs
// is the sole real (demo-mode) runtime consumer of build_demo_plan/
// demo_initial_vars; xtask/tests/demo_corpus_vertical.rs is the second,
// test-only consumer, verifying the demo DSL corpus still lowers and
// passes the compiler's verifier. pub because both are real cross-file
// (and one cross-crate) callers, not a test-only escape hatch.
pub mod demo;
pub mod event_fanout;
pub mod grpc;
pub mod rest;
// `load_harness` lived here until cleanup Phase 0.3 — it is bin-only
// code and now lives directly under `src/bin/load_harness.rs`. No
// other crate ever imported it through the library re-export.
