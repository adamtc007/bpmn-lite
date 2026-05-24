//! Bus runtime wiring for `dmn-lite-server`.
//!
//! Assembles the canonical T2B.9 wiring (v0.6 §T2B.9, items 36–37):
//! BusClient + outbox sender + BusServer with dmn-lite's
//! `DecisionEvaluator`. Caller owns the `Postgres` pool and the
//! catalogue; this module owns the bus surface and the background
//! sender task lifecycle.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use dmn_lite_bus_handler::{DmnLiteBusHandler, NoopResultDispatcher};
use dsl_bus_client::{BusClient, SenderHandle};
use dsl_bus_server::{BusServer, ServerHandle};
use sqlx::PgPool;

use crate::evaluator::CatalogueDecisionEvaluator;
use crate::catalogue::DecisionCatalogue;

/// Owned bus runtime — keep alive for the lifetime of the process; drop
/// (or call [`shutdown`](Self::shutdown)) to stop both the server and
/// the outbox sender cleanly.
pub(crate) struct BusRuntime {
    pub(crate) server: ServerHandle,
    pub(crate) sender: SenderHandle,
}

impl BusRuntime {
    pub(crate) async fn shutdown(self) -> Result<()> {
        // Stop accepting new submissions first so no further outbox
        // rows arrive while we drain.
        let _ = self.server.shutdown().await;
        let _ = self.sender.shutdown().await;
        Ok(())
    }
}

/// Configuration plumbed in by `main`.
pub(crate) struct BusRuntimeConfig {
    pub(crate) pool: PgPool,
    pub(crate) catalogue: Arc<DecisionCatalogue>,
    pub(crate) bind_addr: SocketAddr,
    /// Catalogue version this server claims to host. Incoming
    /// `InvocationRequest.catalogue_version` is compared against
    /// this — mismatches reject with `VersionIncompatible` per
    /// T2B master DoD #46.
    pub(crate) catalogue_version: String,
    /// `(target_domain, endpoint_uri)` pairs. The caller registers
    /// peers it might need to talk to — in v0.6 §10 dmn-lite only
    /// sends result rows back to bpmn-lite, so this is typically a
    /// single entry.
    pub(crate) peers: Vec<(String, String)>,
}

/// Stand up the bus runtime: apply migrations, build a `BusClient` +
/// `BusServer`, spawn the sender, and bind the server. Returns a handle
/// that owns the background lifecycle.
pub(crate) async fn start(config: BusRuntimeConfig) -> Result<BusRuntime> {
    dsl_bus_storage::migrate(&config.pool)
        .await
        .context("apply dsl-bus-storage migrations")?;

    let mut builder = BusClient::builder()
        .pool(config.pool.clone())
        .local_domain("dmn-lite");
    for (domain, uri) in &config.peers {
        builder = builder.add_peer(domain.clone(), uri.clone());
    }
    let client = builder
        .build()
        .await
        .context("build BusClient for dmn-lite")?;
    let notifier = client.outbox_notifier();
    let sender = client.start_sender();

    let evaluator = CatalogueDecisionEvaluator::new(config.catalogue.clone());
    // A3 §3.4 — dmn-lite declares ONLY InvocationService. It is
    // stateless (no entities) and self-contained (no DAG packs). We
    // explicitly do NOT call `.enable_entity_service()` or
    // `.enable_sem_os_service()` so the gRPC server returns
    // `UNIMPLEMENTED` natively for those routes — distinct from a
    // registered stub returning `NOT_IMPLEMENTED` per A3 §6
    // discipline #4. (`InvocationService.Validate` is on the same
    // service as Submit and ships as a stub automatically.)
    let handler = DmnLiteBusHandler::new(evaluator)
        .with_catalogue_version(config.catalogue_version.clone());
    let server = BusServer::builder()
        .pool(config.pool.clone())
        .local_domain("dmn-lite")
        .invocation_dispatcher(handler)
        .result_dispatcher(NoopResultDispatcher)
        .outbox_notifier(notifier)
        .bind(config.bind_addr)
        .build()
        .serve()
        .await
        .context("bind BusServer for dmn-lite")?;

    tracing::info!(
        bind_addr = %server.local_addr(),
        decisions = config.catalogue.len(),
        "dmn-lite bus server listening"
    );

    Ok(BusRuntime { server, sender })
}
