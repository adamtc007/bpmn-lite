//! BPMN-Lite DSL/graph authoring server library.
//!
//! Hosts the workflow **designer** half of the former combined
//! `bpmn-lite-server`: DSL/BPMN/DMN compile preview, macro application,
//! diagnostics resolution, template catalogue, and design-session
//! (graph-backed authoring) endpoints. The workflow instance runner half
//! lives in the sibling `bpmn-lite-server-runner` crate.

mod proposal;
pub mod rest;
// H5 (EOP-PLAN-CRATE-HYGIENE-001): moved from designer-graph — this crate
// (specifically rest.rs's session-runbook endpoint) was always the sole
// consumer; designer-graph's own module-boundary doc comment never listed
// it among its 5 deliberate pub submodules.
mod runbook;
