//! BPMN-Lite gRPC server library.
//!
//! Re-exports the proto-generated types so integration tests and external
//! crates can build gRPC clients without duplicating proto compilation.

pub mod event_fanout;
pub mod grpc;
// `load_harness` lived here until cleanup Phase 0.3 — it is bin-only
// code and now lives directly under `src/bin/load_harness.rs`. No
// other crate ever imported it through the library re-export.
