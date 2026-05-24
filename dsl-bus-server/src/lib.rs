//! Federated DSL bus — receiver-side server.
//!
//! [`BusServer`] wires the tonic-generated `InvocationServiceServer`
//! and `ResultServiceServer` (from [`dsl-bus-protocol`]) into a single
//! pipeline: idempotent inbox short-circuit → consumer-supplied
//! dispatcher → atomic inbox + outbox-result write. The actual verb /
//! result handling is consumer-provided via the
//! [`InvocationDispatcher`] / [`ResultDispatcher`] traits — the server
//! crate stays domain-neutral.
//!
//! Usage (per-domain app wiring, T2B.9):
//!
//! ```text
//! BusServer::builder()
//!     .pool(pg_pool)
//!     .local_domain("ob-poc")
//!     .invocation_dispatcher(MyDispatcher::new(...))
//!     .result_dispatcher(NoopResultDispatcher)
//!     .bind("0.0.0.0:50061".parse().unwrap())
//!     .serve()
//!     .await
//! ```
//!
//! Receive flow (v0.6 §8.6):
//! 1. Decode `idempotency_key`; structural validation.
//! 2. `lookup_inbox` — if found, reply with `SubmissionStatus::Duplicate`
//!    (or `ReceiptStatus::DuplicateIgnored`) using the stored
//!    `execution_id`.
//! 3. Otherwise call the consumer dispatcher (outside any tx).
//! 4. In one tx: `insert_inbox` + (for invocations) `insert_outbox`
//!    enqueuing the result reply.
//! 5. Return `SubmissionStatus::Accepted` / `ReceiptStatus::Received`.

#![forbid(unsafe_code)]

mod server;
mod services;
mod uuid_convert;

pub use server::{BusServer, BusServerBuilder, BusServerError, ServerHandle};
pub use services::{
    InvocationContext, InvocationDispatcher, InvocationOutcome, ResultContext, ResultDispatcher,
    StubEntityService, StubSemOsService,
};

#[cfg(test)]
mod tests;
