//! `BusClient` builder + public send-side API.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use dsl_bus_protocol::v1::{InvocationRequest, InvocationResult};
use dsl_bus_storage::{insert_outbox, BusEndpoint, InsertOutcome, OutboxEntry};
use prost::Message;
use sqlx::PgPool;
use thiserror::Error;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tonic::transport::Endpoint;
use uuid::Uuid;

use crate::sender::{self, SenderConfig, SenderStats};
use crate::uuid_convert::from_proto_opt;

/// Safety-net fallback timer for the outbox sender — runs unconditionally
/// every 30 seconds **regardless of whether a notification arrived**.
///
/// This is **not a polling interval**. The primary wake-up mechanism is
/// [`tokio::sync::Notify`]: writers call [`OutboxNotifier::notify`] after
/// committing an outbox row, and the sender wakes within microseconds.
/// The fallback covers the rare cases where the signal is genuinely lost
/// (writer crashes between commit and notify; tokio's wake-list races
/// under extreme load) so a row can't sit indefinitely.
///
/// Per addendum v0.6-A2 §2 No.4 this value is **not configurable** via
/// the public API. A test-only escape hatch lives in `tests.rs` so the
/// fallback test doesn't take 30 s of wall clock; production callers
/// always get exactly 30 s.
pub(crate) const FALLBACK_TIMER_SECS: u64 = 30;

/// Maximum number of outbox rows claimed per drain iteration.
const DEFAULT_SENDER_BATCH: i64 = 10;
/// Backoff ceiling for retries (caps exponential growth).
const DEFAULT_MAX_BACKOFF_SECS: i64 = 60;

#[derive(Debug, Error)]
pub enum BusClientError {
    #[error("bus storage error: {0}")]
    Storage(#[from] dsl_bus_storage::BusStorageError),

    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("invalid uuid payload (expected 16 bytes, got {actual_len})")]
    MalformedUuid { actual_len: usize },

    #[error("unknown target domain '{0}' — no peer endpoint registered")]
    UnknownPeer(String),

    #[error("invocation request must carry an idempotency_key")]
    MissingIdempotencyKey,

    #[error("invocation result must carry an idempotency_key")]
    MissingResultIdempotencyKey,

    #[error("transport error: {0}")]
    Transport(#[from] tonic::transport::Error),
}

/// Tunable knobs for the bus client. Built via [`BusClientBuilder`].
///
/// Per addendum v0.6-A2 §2 No.4 the 30 s fallback timer is **not**
/// exposed here — see [`FALLBACK_TIMER_SECS`].
#[derive(Debug, Clone)]
pub struct BusClientConfig {
    pub sender_batch_size: i64,
    pub max_backoff_secs: i64,
}

impl Default for BusClientConfig {
    fn default() -> Self {
        Self {
            sender_batch_size: DEFAULT_SENDER_BATCH,
            max_backoff_secs: DEFAULT_MAX_BACKOFF_SECS,
        }
    }
}

/// In-process signal handle for "an outbox row was just committed".
///
/// Writers in `dsl-bus-client` (and `dsl-bus-server`, post-A2) call
/// [`notify`](Self::notify) after their `tx.commit().await?` so the
/// sender task wakes from its [`tokio::sync::Notify`] park and drains
/// the new row inside microseconds. Multiple `notify()` calls between
/// drains coalesce into a single wake-up — the database table is the
/// queue, not this signal.
///
/// Construction is `pub(crate)` so the only way to obtain an
/// `OutboxNotifier` is via [`BusClient::outbox_notifier`]: the bus
/// client owns the underlying `Arc<Notify>` and hands clones to writers
/// + its own sender task.
#[derive(Clone)]
pub struct OutboxNotifier {
    inner: Arc<Notify>,
}

impl OutboxNotifier {
    pub(crate) fn new() -> (Self, Arc<Notify>) {
        let inner = Arc::new(Notify::new());
        (Self { inner: inner.clone() }, inner)
    }

    /// Wake the sender task. Cheap, coalescing, never blocks.
    pub fn notify(&self) {
        self.inner.notify_one();
    }
}

/// Registry of peer endpoints, keyed by `target_domain`.
#[derive(Debug, Clone, Default)]
pub(crate) struct PeerRegistry {
    endpoints: HashMap<String, Endpoint>,
}

impl PeerRegistry {
    pub(crate) fn endpoint(&self, domain: &str) -> Result<&Endpoint, BusClientError> {
        self.endpoints
            .get(domain)
            .ok_or_else(|| BusClientError::UnknownPeer(domain.to_owned()))
    }
}

/// Send-side bus client. Cloneable handle backed by `Arc` state.
#[derive(Clone)]
pub struct BusClient {
    pub(crate) pool: PgPool,
    pub(crate) peers: Arc<PeerRegistry>,
    pub(crate) config: Arc<BusClientConfig>,
    /// Domain id of the local participant (carried into outbound
    /// `InvocationRequest.source_domain` when callers do not set it).
    pub(crate) local_domain: Arc<String>,
    /// Handed to writers as `OutboxNotifier` via [`outbox_notifier`].
    pub(crate) notifier: OutboxNotifier,
    /// Sender-side handle on the same `Arc<Notify>`. The sender task
    /// parks on this; writers wake it via the `notifier` clone.
    pub(crate) notify: Arc<Notify>,
}

/// Handle returned by [`BusClient::start_sender`] — call
/// [`shutdown`](SenderHandle::shutdown) to stop the loop and await the
/// background task.
pub struct SenderHandle {
    pub(crate) shutdown: tokio::sync::watch::Sender<bool>,
    pub(crate) join: JoinHandle<()>,
    pub(crate) stats: Arc<SenderStats>,
}

impl SenderHandle {
    /// Snapshot of in-flight sender metrics.
    pub fn stats(&self) -> SenderStats {
        (*self.stats).snapshot()
    }

    /// Signal the sender to stop and await its termination.
    pub async fn shutdown(self) -> Result<(), tokio::task::JoinError> {
        let _ = self.shutdown.send(true);
        self.join.await
    }
}

impl BusClient {
    /// Start a builder; defaults are mostly OK but you must provide a
    /// `pool` + `local_domain` before `build()`.
    pub fn builder() -> BusClientBuilder {
        BusClientBuilder::default()
    }

    /// Cloneable `OutboxNotifier` handle — pass to writers that commit
    /// outbox rows (e.g. the bus server, a bpmn-lite executor) so they
    /// can wake the sender task immediately after `tx.commit()`.
    pub fn outbox_notifier(&self) -> OutboxNotifier {
        self.notifier.clone()
    }

    /// Expose the underlying `PgPool` for callers that need to write
    /// their own outbox rows atomically (e.g. the T3 plan walker which
    /// also inserts a `PendingInvocation` in the same logical unit).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Write an invocation to the outbox. Returns the idempotency key
    /// (also present in `req`) and an [`InsertOutcome`] so callers can
    /// distinguish a fresh enqueue from an idempotent replay.
    pub async fn submit_invocation(
        &self,
        target_domain: &str,
        mut req: InvocationRequest,
    ) -> Result<(Uuid, InsertOutcome), BusClientError> {
        // Confirm the target is registered so we fail fast at submit
        // time rather than from inside the sender loop.
        self.peers.endpoint(target_domain)?;

        let key = from_proto_opt(&req.idempotency_key)?
            .ok_or(BusClientError::MissingIdempotencyKey)?;

        if req.source_domain.is_empty() {
            req.source_domain = self.local_domain.as_str().to_owned();
        }

        let payload = req.encode_to_vec();
        let entry = OutboxEntry::new_pending(
            Uuid::now_v7(),
            target_domain.to_owned(),
            BusEndpoint::Invocation,
            payload,
            key,
        );
        let outcome = insert_outbox(&self.pool, &entry).await?;
        // A2 §2: wake the sender immediately after the outbox write
        // commits. Coalesces with any concurrent writes.
        self.notify.notify_one();
        Ok((key, outcome))
    }

    /// Receiver-side: enqueue a `DeliverResult` payload bound for the
    /// originating domain. Mirrors [`submit_invocation`] but with the
    /// `result` endpoint.
    pub async fn send_result(
        &self,
        target_domain: &str,
        result: InvocationResult,
    ) -> Result<(Uuid, InsertOutcome), BusClientError> {
        self.peers.endpoint(target_domain)?;

        let key = from_proto_opt(&result.idempotency_key)?
            .ok_or(BusClientError::MissingResultIdempotencyKey)?;

        let payload = result.encode_to_vec();
        let entry = OutboxEntry::new_pending(
            Uuid::now_v7(),
            target_domain.to_owned(),
            BusEndpoint::Result,
            payload,
            key,
        );
        let outcome = insert_outbox(&self.pool, &entry).await?;
        self.notify.notify_one();
        Ok((key, outcome))
    }

    /// Spawn the §8.5 sender task with the production 30 s fallback.
    pub fn start_sender(&self) -> SenderHandle {
        self.start_sender_internal(Duration::from_secs(FALLBACK_TIMER_SECS))
    }

    /// Internal entry that constructs the `SenderConfig`. Tests call
    /// this directly to shorten the fallback for the fallback-timer
    /// integration test; production goes through [`start_sender`].
    pub(crate) fn start_sender_internal(&self, fallback: Duration) -> SenderHandle {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let stats = Arc::new(SenderStats::default());
        let config = SenderConfig {
            pool: self.pool.clone(),
            peers: self.peers.clone(),
            fallback,
            batch_size: self.config.sender_batch_size,
            max_backoff_secs: self.config.max_backoff_secs,
            notify: self.notify.clone(),
            stats: stats.clone(),
            shutdown: shutdown_rx,
        };
        let join = tokio::spawn(sender::run(config));
        SenderHandle {
            shutdown: shutdown_tx,
            join,
            stats,
        }
    }
}

/// Builder for [`BusClient`].
#[derive(Default)]
pub struct BusClientBuilder {
    pool: Option<PgPool>,
    peers: HashMap<String, String>,
    config: BusClientConfig,
    local_domain: Option<String>,
}

impl BusClientBuilder {
    pub fn pool(mut self, pool: PgPool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Identify the local participant. Used to fill
    /// `InvocationRequest.source_domain` when the caller leaves it
    /// blank.
    pub fn local_domain(mut self, domain: impl Into<String>) -> Self {
        self.local_domain = Some(domain.into());
        self
    }

    pub fn add_peer(mut self, domain: impl Into<String>, endpoint: impl Into<String>) -> Self {
        self.peers.insert(domain.into(), endpoint.into());
        self
    }

    pub fn sender_batch_size(mut self, batch_size: i64) -> Self {
        self.config.sender_batch_size = batch_size;
        self
    }

    pub fn max_backoff_secs(mut self, secs: i64) -> Self {
        self.config.max_backoff_secs = secs;
        self
    }

    pub async fn build(self) -> Result<BusClient, BusClientError> {
        let pool = self.pool.expect("BusClientBuilder.pool is required");
        let local_domain = self
            .local_domain
            .expect("BusClientBuilder.local_domain is required");
        let mut endpoints = HashMap::with_capacity(self.peers.len());
        for (domain, uri) in self.peers {
            let endpoint = Endpoint::from_shared(uri)?;
            endpoints.insert(domain, endpoint);
        }
        let (notifier, notify) = OutboxNotifier::new();
        Ok(BusClient {
            pool,
            peers: Arc::new(PeerRegistry { endpoints }),
            config: Arc::new(self.config),
            local_domain: Arc::new(local_domain),
            notifier,
            notify,
        })
    }
}
