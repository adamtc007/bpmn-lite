//! Public `BusServer` + builder.

use std::net::SocketAddr;
use std::sync::Arc;

use dsl_bus_client::OutboxNotifier;
use dsl_bus_protocol::v1::entity_service_server::EntityServiceServer;
use dsl_bus_protocol::v1::invocation_service_server::InvocationServiceServer;
use dsl_bus_protocol::v1::result_service_server::ResultServiceServer;
use dsl_bus_protocol::v1::sem_os_service_server::SemOsServiceServer;
use sqlx::PgPool;
use thiserror::Error;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tonic::transport::Server;

use crate::services::{
    InvocationDispatcher, InvocationServiceImpl, ResultDispatcher, ResultServiceImpl,
    StubEntityService, StubSemOsService,
};

#[derive(Debug, Error)]
pub enum BusServerError {
    #[error("bus storage error: {0}")]
    Storage(#[from] dsl_bus_storage::BusStorageError),

    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("invalid uuid payload (expected 16 bytes, got {actual_len})")]
    MalformedUuid { actual_len: usize },

    #[error("verb unknown: {0}")]
    UnknownVerb(String),

    #[error("catalogue version incompatible: {0}")]
    VersionIncompatible(String),

    #[error("authority denied: {0}")]
    AuthorityDenied(String),

    #[error("malformed request: {0}")]
    Malformed(String),

    #[error("internal dispatcher error: {0}")]
    Internal(String),
}

/// Receiver-side bus server. Configure via the builder, then call
/// `serve()` to bind + run until shutdown.
pub struct BusServer {
    pool: PgPool,
    local_domain: Arc<String>,
    invocation: Arc<dyn InvocationDispatcher>,
    result: Arc<dyn ResultDispatcher>,
    /// Rung after committing the inbox + result-outbox transaction in
    /// `InvocationService::submit` so the local `dsl-bus-client`
    /// sender task drains the new row immediately (A2 §2).
    outbox_notifier: OutboxNotifier,
    bind_addr: SocketAddr,
    /// A3 §2.2 — registers `EntityService` (stubbed in v0.6) when set.
    /// Per A3 §6 discipline #4 a domain that does *not* hold entity
    /// state (e.g. dmn-lite) must leave this `false`, so gRPC returns
    /// `UNIMPLEMENTED` natively — distinct from a registered stub
    /// returning `NOT_IMPLEMENTED`.
    enable_entity_service: bool,
    /// A3 §2.3 — registers `SemOsService` (stubbed in v0.6) when set.
    enable_sem_os_service: bool,
}

impl BusServer {
    pub fn builder() -> BusServerBuilder {
        BusServerBuilder::default()
    }

    /// Bind + serve on the configured address until the returned
    /// [`ServerHandle`] receives a shutdown signal.
    pub async fn serve(self) -> Result<ServerHandle, BusServerError> {
        let invocation = InvocationServiceImpl {
            pool: self.pool.clone(),
            dispatcher: self.invocation,
            local_domain: self.local_domain,
            outbox_notifier: self.outbox_notifier,
        };
        let result = ResultServiceImpl {
            pool: self.pool,
            dispatcher: self.result,
        };

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let bind = self.bind_addr;
        let mut server = Server::builder()
            .add_service(InvocationServiceServer::new(invocation))
            .add_service(ResultServiceServer::new(result));
        // A3 §6 discipline #3: register only what the domain declares.
        // Domains that opt in get a stub returning NOT_IMPLEMENTED;
        // domains that don't opt in leave the gRPC route absent so
        // callers see UNIMPLEMENTED natively.
        if self.enable_entity_service {
            server = server.add_service(EntityServiceServer::new(StubEntityService));
        }
        if self.enable_sem_os_service {
            server = server.add_service(SemOsServiceServer::new(StubSemOsService));
        }
        let server_fut = server.serve_with_shutdown(bind, async {
            let _ = shutdown_rx.await;
        });
        let join = tokio::spawn(async move {
            if let Err(err) = server_fut.await {
                tracing::warn!(error = %err, "dsl-bus-server exited with error");
            }
        });

        Ok(ServerHandle {
            shutdown: shutdown_tx,
            join,
            bound_addr: bind,
        })
    }
}

/// Handle to a running bus server. Drop or call `shutdown()` to stop.
pub struct ServerHandle {
    shutdown: oneshot::Sender<()>,
    join: JoinHandle<()>,
    bound_addr: SocketAddr,
}

impl ServerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.bound_addr
    }

    /// Trigger shutdown and wait for the server to drain.
    pub async fn shutdown(self) -> Result<(), tokio::task::JoinError> {
        let _ = self.shutdown.send(());
        self.join.await
    }
}

#[derive(Default)]
pub struct BusServerBuilder {
    pool: Option<PgPool>,
    local_domain: Option<String>,
    invocation: Option<Arc<dyn InvocationDispatcher>>,
    result: Option<Arc<dyn ResultDispatcher>>,
    outbox_notifier: Option<OutboxNotifier>,
    bind_addr: Option<SocketAddr>,
    enable_entity_service: bool,
    enable_sem_os_service: bool,
}

impl BusServerBuilder {
    pub fn pool(mut self, pool: PgPool) -> Self {
        self.pool = Some(pool);
        self
    }

    pub fn local_domain(mut self, domain: impl Into<String>) -> Self {
        self.local_domain = Some(domain.into());
        self
    }

    pub fn invocation_dispatcher<D: InvocationDispatcher>(mut self, d: D) -> Self {
        self.invocation = Some(Arc::new(d));
        self
    }

    pub fn result_dispatcher<D: ResultDispatcher>(mut self, d: D) -> Self {
        self.result = Some(Arc::new(d));
        self
    }

    /// Required (A2 §2): the bus server enqueues a result row after
    /// every accepted invocation. The notifier is owned by the local
    /// `dsl-bus-client::BusClient` — call
    /// [`BusClient::outbox_notifier`](dsl_bus_client::BusClient::outbox_notifier)
    /// to obtain a clone.
    pub fn outbox_notifier(mut self, n: OutboxNotifier) -> Self {
        self.outbox_notifier = Some(n);
        self
    }

    pub fn bind(mut self, addr: SocketAddr) -> Self {
        self.bind_addr = Some(addr);
        self
    }

    /// A3 §2.2 — opt in to `EntityService`. Registers a stub that
    /// returns `RESOLUTION_NOT_IMPLEMENTED` for every query. Capability
    /// discipline: declare this in the manifest's `services:` block so
    /// consumers know it's available (even though stubbed).
    pub fn enable_entity_service(mut self) -> Self {
        self.enable_entity_service = true;
        self
    }

    /// A3 §2.3 — opt in to `SemOsService`. Registers a stub that
    /// returns `DAG_PACK_NOT_IMPLEMENTED` for every query.
    pub fn enable_sem_os_service(mut self) -> Self {
        self.enable_sem_os_service = true;
        self
    }

    pub fn build(self) -> BusServer {
        BusServer {
            pool: self.pool.expect("BusServerBuilder.pool is required"),
            local_domain: Arc::new(
                self.local_domain
                    .expect("BusServerBuilder.local_domain is required"),
            ),
            invocation: self
                .invocation
                .expect("BusServerBuilder.invocation_dispatcher is required"),
            result: self
                .result
                .expect("BusServerBuilder.result_dispatcher is required"),
            outbox_notifier: self
                .outbox_notifier
                .expect("BusServerBuilder.outbox_notifier is required"),
            bind_addr: self
                .bind_addr
                .expect("BusServerBuilder.bind is required"),
            enable_entity_service: self.enable_entity_service,
            enable_sem_os_service: self.enable_sem_os_service,
        }
    }
}
