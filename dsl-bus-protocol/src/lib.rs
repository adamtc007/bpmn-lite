//! Federated DSL bus protocol — generated tonic stubs for the v0.6 §6
//! invocation and result services.
//!
//! Only the tonic-generated module is exported. Handcrafted helpers do
//! not belong here; durable storage lives in [`dsl-bus-storage`] and
//! manifest types live in [`dsl-manifest`].
//!
//! Wire conventions (§6.4):
//! - protobuf binary encoding
//! - HTTP/2 framing via tonic
//! - gzip compression (caller-configured)
//! - UUIDv7 16-byte payloads in [`v1::Uuid`]
//! - all wall-clock fields use [`prost_types::Timestamp`]
//!
//! The generated `InvocationServiceServer` / `ResultServiceServer`
//! traits are the receive-side handler contracts; the matching
//! `*Client` types are the send-side stubs.
//!
//! [`dsl-bus-storage`]: ../dsl_bus_storage/index.html
//! [`dsl-manifest`]: ../dsl_manifest/index.html

#![forbid(unsafe_code)]

/// Generated tonic + prost code for `dsl.bus.v1` — declared in
/// `proto/dsl_bus.proto`.
pub mod v1 {
    tonic::include_proto!("dsl.bus.v1");
}

#[cfg(test)]
mod tests;
