//! Federated DSL bus — sender-side client.
//!
//! [`BusClient`] is the send-side surface for any federated DSL
//! participant. It writes outbox rows on demand
//! ([`submit_invocation`](BusClient::submit_invocation),
//! [`send_result`](BusClient::send_result)) and runs the §8.5 sender
//! task that drains the outbox into peer gRPC endpoints.
//!
//! Lifecycle:
//!
//! ```text
//! BusClient::builder()
//!     .pool(pg_pool)
//!     .add_peer("ob-poc", "http://localhost:50061")
//!     .add_peer("dmn-lite", "http://localhost:50062")
//!     .build().await?
//!     .start_sender();   // → JoinHandle; loop until shutdown
//! ```
//!
//! Once started, every `submit_invocation` / `send_result` call:
//!
//! 1. encodes the protobuf message,
//! 2. writes a pending outbox row keyed by `idempotency_key`,
//! 3. returns immediately — the in-process sender loop dispatches.
//!
//! Successful dispatch transitions the row to `submitted` with the
//! receiver-assigned `execution_id`. Transport errors transition the
//! row to `pending` with exponential backoff (1s, 2s, 4s, …, capped at
//! `BusClientConfig::max_backoff_secs`).

#![forbid(unsafe_code)]

mod client;
mod sender;
mod uuid_convert;

pub use client::{
    BusClient, BusClientBuilder, BusClientConfig, BusClientError, OutboxNotifier, SenderHandle,
};
pub use sender::SenderStats;

#[cfg(test)]
mod tests;
