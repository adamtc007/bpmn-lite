//! BPMN-Lite DSL/graph authoring server library.
//!
//! Hosts the workflow **designer** half of the former combined
//! `bpmn-lite-server`: DSL/BPMN/DMN compile preview, macro application,
//! diagnostics resolution, template catalogue, and design-session
//! (graph-backed authoring) endpoints. The workflow instance runner half
//! lives in the sibling `bpmn-lite-server-runner` crate.

pub mod rest;
