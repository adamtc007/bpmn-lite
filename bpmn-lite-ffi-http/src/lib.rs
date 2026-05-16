//! HTTP FFI execution owner for bpmn-lite.
//!
//! Registers HTTP endpoint templates in the FFI catalogue and dispatches
//! `Instr::ExecFfi` calls to real HTTP services. Implements the B6 contract.

#![forbid(unsafe_code)]

mod owner;
mod template;

pub use owner::HttpFfiOwner;
pub use template::{HttpIdempotency, HttpMethod, HttpTemplateConfig};
