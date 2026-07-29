//! Sender task — drains the outbox and dispatches payloads to peers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dsl_bus_protocol::v1::invocation_service_client::InvocationServiceClient;
use dsl_bus_protocol::v1::result_service_client::ResultServiceClient;
use dsl_bus_protocol::v1::{InvocationRequest, InvocationResult};
use dsl_bus_storage::{
    BusEndpoint, OutboxEntry, claim_pending_outbox, mark_outbox_retry, mark_outbox_submitted,
};
use futures::{StreamExt, stream};
use prost::Message;
use sqlx::PgPool;
use tokio::sync::{Notify, watch};
use tonic::transport::Channel;
use tracing::{debug, warn};

use crate::client::PeerRegistry;
use crate::uuid_convert::from_proto_opt;

const MAX_IN_FLIGHT: usize = 32;

/// Shape of the §8.5 sender loop, post-A2.
///
/// Primary wake-up is via the shared `notify` handle the writers ring
/// after each successful `tx.commit()`. The `fallback` `Duration` is a
/// safety net — production builds always pass
/// `Duration::from_secs(client::FALLBACK_TIMER_SECS)`; the only reason
/// it lives on the config struct rather than as a hard-coded sleep is
/// so the in-crate fallback-timer test can swap it for a shorter value
/// without forcing real wall-clock waits.
pub(crate) struct SenderConfig {
    pub pool: PgPool,
    pub peers: Arc<PeerRegistry>,
    pub fallback: Duration,
    pub batch_size: i64,
    pub max_backoff_secs: i64,
    pub notify: Arc<Notify>,
    pub stats: Arc<SenderStats>,
    pub shutdown: watch::Receiver<bool>,
    pub submission_ack_handler: Option<Arc<dyn crate::SubmissionAckHandler>>,
    pub channels: Arc<tokio::sync::RwLock<std::collections::HashMap<String, Channel>>>,
}

/// Atomic counters covering the sender's behaviour. Cheap to read; the
/// public surface is the `snapshot()` reflection.
#[derive(Default)]
pub struct SenderStats {
    submitted: AtomicU64,
    retried: AtomicU64,
    rows_seen: AtomicU64,
}

impl SenderStats {
    pub fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Relaxed)
    }
    pub fn retried(&self) -> u64 {
        self.retried.load(Ordering::Relaxed)
    }
    fn rows_seen(&self) -> u64 {
        self.rows_seen.load(Ordering::Relaxed)
    }

    /// Cloneable snapshot — useful for assertions that expect a frozen
    /// view of the counters.
    pub fn snapshot(&self) -> Self {
        Self {
            submitted: AtomicU64::new(self.submitted()),
            retried: AtomicU64::new(self.retried()),
            rows_seen: AtomicU64::new(self.rows_seen()),
        }
    }
}

pub(crate) async fn run(mut cfg: SenderConfig) {
    // Drain on startup — outbox may carry rows committed before this
    // task spawned (post-crash recovery, deferred-retry rows whose
    // `next_attempt_at` is already in the past).
    if let Err(err) = drain_until_empty(&cfg).await {
        warn!(error = %err, "startup drain failed; continuing");
    }

    let mut fallback = tokio::time::interval(cfg.fallback);
    // The first `tick()` fires immediately — consume it so the first
    // real iteration waits the full fallback period (the startup
    // drain already covered the "rows from before we started" case).
    fallback.tick().await;

    loop {
        if *cfg.shutdown.borrow() {
            break;
        }

        tokio::select! {
            // Primary path: writer rang the bell.
            _ = cfg.notify.notified() => {
                if let Err(err) = drain_until_empty(&cfg).await {
                    warn!(error = %err, "post-notify drain failed; continuing");
                }
            }
            // Safety net: in case a notification was missed.
            _ = fallback.tick() => {
                if let Err(err) = drain_until_empty(&cfg).await {
                    warn!(error = %err, "fallback drain failed; continuing");
                }
            }
            // Shutdown.
            _ = cfg.shutdown.changed() => {
                if *cfg.shutdown.borrow() {
                    break;
                }
            }
        }
    }
    debug!("dsl-bus-client sender shutting down");
}

/// Repeatedly call `drain_once` until a batch comes back empty.
///
/// Bursty writers (e.g. a BPMN process that emits several callouts in
/// quick succession) coalesce their notifications into a single
/// wake-up; this loop ensures the wake-up drains all the rows they
/// committed, not just the first batch.
async fn drain_until_empty(cfg: &SenderConfig) -> Result<(), sqlx::Error> {
    let tenants: Vec<String> =
        sqlx::query_scalar("SELECT tenant_id FROM dsl_bus.list_pending_outbox_tenants()")
            .fetch_all(&cfg.pool)
            .await?;

    for tenant_id in tenants {
        loop {
            let claimed = drain_once_for_tenant(cfg, &tenant_id).await?;
            cfg.stats
                .rows_seen
                .fetch_add(claimed as u64, Ordering::Relaxed);
            if claimed == 0 {
                break;
            }
        }
    }
    Ok(())
}

async fn drain_once_for_tenant(cfg: &SenderConfig, tenant_id: &str) -> Result<usize, sqlx::Error> {
    let mut tx = cfg.pool.begin().await?;
    sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;

    let claim_token = uuid::Uuid::now_v7();
    let claim_until = chrono::Utc::now() + chrono::Duration::seconds(30);
    let entries = claim_pending_outbox(
        &mut tx,
        cfg.batch_size,
        "dsl-bus-sender",
        claim_token,
        claim_until,
    )
    .await
    .map_err(|e| match e {
        dsl_bus_storage::BusStorageError::Sqlx(err) => err,
        other => sqlx::Error::Configuration(other.to_string().into()),
    })?;
    let claimed = entries.len();
    tx.commit().await?;

    let results = stream::iter(entries.into_iter().map(|entry| dispatch_entry(cfg, entry)))
        .buffer_unordered(MAX_IN_FLIGHT)
        .collect::<Vec<_>>()
        .await;
    for result in results {
        result?;
    }
    Ok(claimed)
}

async fn dispatch_entry(cfg: &SenderConfig, entry: OutboxEntry) -> Result<(), sqlx::Error> {
    match entry.target_endpoint {
        BusEndpoint::Invocation => dispatch_invocation(cfg, entry).await,
        BusEndpoint::Result => dispatch_result(cfg, entry).await,
    }
}

async fn dispatch_invocation(cfg: &SenderConfig, entry: OutboxEntry) -> Result<(), sqlx::Error> {
    let channel = match peer_channel(cfg, &entry.target_domain).await {
        Ok(c) => c,
        Err(err) => {
            return record_retry(cfg, &entry, &err).await;
        }
    };

    let req = match InvocationRequest::decode(&entry.payload[..]) {
        Ok(r) => r,
        Err(err) => {
            return record_retry(cfg, &entry, &format!("decode: {err}")).await;
        }
    };

    let mut client = InvocationServiceClient::new(channel);
    match client.submit(req).await {
        Ok(resp) => {
            let ack = resp.into_inner();
            match from_proto_opt(&ack.execution_id) {
                Ok(Some(exec_id)) => {
                    if let Some(handler) = &cfg.submission_ack_handler {
                        handler
                            .accepted(&entry, exec_id)
                            .await
                            .map_err(|error| sqlx::Error::Configuration(error.into()))?;
                    } else {
                        record_submitted(cfg, &entry, exec_id).await?;
                    }
                    cfg.stats.submitted.fetch_add(1, Ordering::Relaxed);
                }
                Ok(None) => {
                    // Receiver responded with no execution_id — the
                    // SubmissionStatus carries the reason. Record it
                    // verbatim so non-retryable rejections (version
                    // skew, malformed, authority, verb unknown)
                    // surface in `last_error`.
                    let label = submission_status_label(ack.status);
                    let detail = if ack.detail.is_empty() {
                        label.to_owned()
                    } else {
                        format!("{label}: {}", ack.detail)
                    };
                    record_retry(cfg, &entry, &detail).await?;
                }
                Err(err) => {
                    record_retry(cfg, &entry, &err.to_string()).await?;
                }
            }
        }
        Err(status) => {
            record_retry(cfg, &entry, &format!("status: {}", status.message())).await?;
        }
    }
    Ok(())
}

fn submission_status_label(status: i32) -> &'static str {
    use dsl_bus_protocol::v1::SubmissionStatus;
    match SubmissionStatus::try_from(status).unwrap_or(SubmissionStatus::SubmissionUnspecified) {
        SubmissionStatus::SubmissionUnspecified => "rejected (unspecified)",
        SubmissionStatus::Accepted => "accepted but no execution_id",
        SubmissionStatus::Duplicate => "duplicate (no execution_id)",
        SubmissionStatus::RejectedVerbUnknown => "rejected: verb unknown",
        SubmissionStatus::RejectedVersionIncompatible => "rejected: catalogue version incompatible",
        SubmissionStatus::RejectedAuthority => "rejected: authority denied",
        SubmissionStatus::RejectedMalformed => "rejected: malformed request",
    }
}

async fn dispatch_result(cfg: &SenderConfig, entry: OutboxEntry) -> Result<(), sqlx::Error> {
    let channel = match peer_channel(cfg, &entry.target_domain).await {
        Ok(c) => c,
        Err(err) => {
            return record_retry(cfg, &entry, &err).await;
        }
    };

    let msg = match InvocationResult::decode(&entry.payload[..]) {
        Ok(r) => r,
        Err(err) => {
            return record_retry(cfg, &entry, &format!("decode: {err}")).await;
        }
    };

    let mut client = ResultServiceClient::new(channel);
    let exec_id = from_proto_opt(&msg.execution_id)
        .ok()
        .flatten()
        .unwrap_or_else(uuid::Uuid::nil);

    match client.deliver_result(msg).await {
        Ok(_resp) => {
            // Result deliveries don't return a fresh execution_id — re-use
            // the one we sent so the outbox row carries something useful.
            record_submitted(cfg, &entry, exec_id).await?;
            cfg.stats.submitted.fetch_add(1, Ordering::Relaxed);
        }
        Err(status) => {
            record_retry(cfg, &entry, &format!("status: {}", status.message())).await?;
        }
    }
    Ok(())
}

async fn peer_channel(cfg: &SenderConfig, domain: &str) -> Result<Channel, String> {
    if let Some(channel) = cfg.channels.read().await.get(domain).cloned() {
        return Ok(channel);
    }
    let endpoint = cfg
        .peers
        .endpoint(domain)
        .map_err(|error| error.to_string())?
        .clone();
    let channel = endpoint
        .connect()
        .await
        .map_err(|error| format!("connect: {error}"))?;
    cfg.channels
        .write()
        .await
        .insert(domain.to_string(), channel.clone());
    Ok(channel)
}

async fn record_retry(
    cfg: &SenderConfig,
    entry: &OutboxEntry,
    message: &str,
) -> Result<(), sqlx::Error> {
    let backoff = exp_backoff_secs(entry.attempt_count, cfg.max_backoff_secs);
    let claim_token = entry.claim_token.ok_or_else(|| {
        sqlx::Error::Protocol("claimed outbox row has no claim token".to_string())
    })?;
    let mut tx = cfg.pool.begin().await?;
    sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
        .bind(&entry.tenant_id)
        .execute(&mut *tx)
        .await?;
    mark_outbox_retry(&mut *tx, entry.id, claim_token, backoff, message)
        .await
        .map_err(map_storage_err)?;
    tx.commit().await?;
    cfg.stats.retried.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

async fn record_submitted(
    cfg: &SenderConfig,
    entry: &OutboxEntry,
    execution_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    let claim_token = entry.claim_token.ok_or_else(|| {
        sqlx::Error::Protocol("claimed outbox row has no claim token".to_string())
    })?;
    let mut tx = cfg.pool.begin().await?;
    sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
        .bind(&entry.tenant_id)
        .execute(&mut *tx)
        .await?;
    mark_outbox_submitted(&mut *tx, entry.id, claim_token, execution_id)
        .await
        .map_err(map_storage_err)?;
    tx.commit().await?;
    Ok(())
}

fn map_storage_err(e: dsl_bus_storage::BusStorageError) -> sqlx::Error {
    match e {
        dsl_bus_storage::BusStorageError::Sqlx(err) => err,
        other => sqlx::Error::Configuration(other.to_string().into()),
    }
}

/// 1s, 2s, 4s, 8s, … capped at `max_secs` (v0.6 §6.4).
pub(crate) fn exp_backoff_secs(attempt_count: i32, max_secs: i64) -> i64 {
    let attempts = attempt_count.max(0) as u32;
    // 2^attempt — saturate before overflow.
    let raw: i64 = 1i64.checked_shl(attempts).unwrap_or(i64::MAX);
    raw.clamp(1, max_secs)
}
