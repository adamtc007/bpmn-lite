//! gRPC FFI execution owner for bpmn-lite.

#![forbid(unsafe_code)]

mod owner;
mod template;

/// Generated FfiBridge proto types (client + server).
pub mod proto {
    tonic::include_proto!("ffi_bridge.v1");
}

pub use owner::GrpcFfiOwner;
pub use template::GrpcTemplateConfig;
