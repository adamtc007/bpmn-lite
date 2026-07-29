#[cfg(feature = "postgres")]
mod bus_runtime;

use std::sync::Arc;
use uuid::Uuid;

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_ffi_grpc::GrpcFfiOwner;
use bpmn_lite_ffi_http::HttpFfiOwner;
use bpmn_lite_server_runner::event_fanout::EventFanout;
use bpmn_lite_server_runner::grpc::proto::bpmn_lite_server::BpmnLiteServer;
use bpmn_lite_server_runner::grpc::{BpmnLiteService, RequestLimits, ServerMetrics};
use bpmn_lite_store::store::WorkflowStore;
use bpmn_lite_store::store_memory::MemoryStore;
use dmn_lite_bridge::DmnLiteOwner;
use ffi_catalogue::{FfiCatalogue, MemoryFfiTemplateStore};
use ffi_dispatcher::FfiDispatcher;
use futures::{stream, StreamExt};
use tokio::sync::Semaphore;
use tonic::transport::Server;
use tonic_health::server::health_reporter;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    validate_production_configuration()?;
    let addr = parse_bind_addr().parse()?;

    let database_url = parse_database_url();
    #[cfg(feature = "postgres")]
    let database_admin_url = std::env::var("DATABASE_ADMIN_URL").ok();
    #[cfg(feature = "postgres")]
    let postgres_listener_url = database_url.clone();

    let store_mode = std::env::var("BPMN_LITE_STORE").unwrap_or_else(|_| "postgres".to_string());
    let allow_memory = store_mode.eq_ignore_ascii_case("memory");

    // T2B.9 — also retain the runtime pool for the federated DSL bus
    // runtime so the bus reuses the same connection bookkeeping the
    // BPMN store uses. `None` means the bus is disabled (memory store
    // selected, or no postgres feature).
    #[cfg(feature = "postgres")]
    let mut bus_pool: Option<sqlx::PgPool> = None;
    let store: Arc<dyn WorkflowStore> = match database_url {
        #[cfg(feature = "postgres")]
        Some(url) => {
            // A18 — split admin / runtime connections.
            //
            // When DATABASE_ADMIN_URL is set, migrations run through it
            // (typically a Postgres superuser) and the runtime pool then
            // connects through DATABASE_URL (typically the unprivileged
            // bpmn_lite_app role). The admin pool is dropped once
            // migrations complete so the application owns only the
            // RLS-bounded runtime connection.
            //
            // When DATABASE_ADMIN_URL is unset, the runtime URL is used
            // for both — preserves backwards compatibility for dev
            // workflows that still run as superuser.
            if let Some(admin_url) = database_admin_url.as_deref() {
                tracing::info!("Running migrations via DATABASE_ADMIN_URL...");
                let admin_pool = sqlx::PgPool::connect(admin_url).await?;
                let admin_store =
                    bpmn_lite_store_postgres::PostgresWorkflowStore::new(admin_pool.clone());
                admin_store.migrate().await?;

                // Run bus migrations under admin role with search_path=dsl_bus
                let mut bus_admin_url = admin_url.to_string();
                if bus_admin_url.contains('?') {
                    bus_admin_url.push_str("&options=-csearch_path%3Ddsl_bus");
                } else {
                    bus_admin_url.push_str("?options=-csearch_path%3Ddsl_bus");
                }
                let bus_admin_pool = sqlx::PgPool::connect(&bus_admin_url).await?;
                dsl_bus_storage::migrate(&bus_admin_pool).await?;
                bus_admin_pool.close().await;

                admin_pool.close().await;
                tracing::info!("Migrations applied; admin pool closed");
            }

            tracing::info!("Connecting to PostgreSQL for runtime...");
            let pool = sqlx::PgPool::connect(&url).await?;

            // A18-Session-1: warn (not yet error) if connected as superuser.
            verify_not_superuser(&pool).await?;

            bus_pool = Some(pool.clone());
            let pg = bpmn_lite_store_postgres::PostgresWorkflowStore::new(pool);
            if database_admin_url.is_none() {
                // No admin URL — run migrations through the runtime pool
                // (legacy / dev path).
                pg.migrate().await?;

                // Also run bus migrations
                let mut bus_url = url.clone();
                if bus_url.contains('?') {
                    bus_url.push_str("&options=-csearch_path%3Ddsl_bus");
                } else {
                    bus_url.push_str("?options=-csearch_path%3Ddsl_bus");
                }
                let bus_pool_migrator = sqlx::PgPool::connect(&bus_url).await?;
                dsl_bus_storage::migrate(&bus_pool_migrator).await?;
                bus_pool_migrator.close().await;

                tracing::info!("Using PostgresWorkflowStore (migrations applied via runtime pool)");
            } else {
                tracing::info!(
                    "Using PostgresWorkflowStore (migrations already applied via admin pool)"
                );
            }
            Arc::new(pg)
        }
        #[cfg(not(feature = "postgres"))]
        Some(_) => {
            return Err(config_error(
                "--database-url / DATABASE_URL set but postgres feature not enabled",
            ));
        }
        None => {
            if allow_memory {
                tracing::warn!("Using MemoryStore because BPMN_LITE_STORE=memory");
                Arc::new(MemoryStore::new())
            } else {
                return Err(config_error(
                    "DATABASE_URL is required unless BPMN_LITE_STORE=memory is set",
                ));
            }
        }
    };

    // FFI infrastructure — dmn-lite decision vocabulary wired in-process.
    let ffi_store = Arc::new(MemoryFfiTemplateStore::new());
    let ffi_cat = Arc::new(FfiCatalogue::new(ffi_store.clone()));
    let ffi_owner = Arc::new(DmnLiteOwner::new());
    let http_ffi_owner = Arc::new(HttpFfiOwner::new());
    let grpc_ffi_owner = Arc::new(GrpcFfiOwner::new());
    let mut ffi_dispatcher = FfiDispatcher::new(ffi_cat.clone());
    ffi_dispatcher.register_owner(ffi_owner.clone())?;
    ffi_dispatcher.register_owner(http_ffi_owner.clone())?;
    ffi_dispatcher.register_owner(grpc_ffi_owner.clone())?;
    let ffi_dispatcher = Arc::new(ffi_dispatcher);
    tracing::info!("FFI dispatcher initialised with dmn-lite + http + grpc execution owners");

    // Build the transport adapter independently of the deterministic engine.
    #[cfg(feature = "postgres")]
    let early_bus_client: Option<Arc<dsl_bus_client::BusClient>> = match &bus_pool {
        Some(pool) if std::env::var("BPMN_LITE_BUS_LISTEN").is_ok() => {
            let pending = Arc::new(
                bpmn_lite_store_postgres::PostgresPendingInvocationStore::new(pool.clone()),
            );
            let mut builder = dsl_bus_client::BusClient::builder()
                .pool(pool.clone())
                .local_domain("bpmn-lite")
                .submission_ack_handler(Arc::new(bus_runtime::StoreSubmissionAckHandler::new(
                    pending.clone(),
                    store.clone(),
                )));
            if let Ok(uri) = std::env::var("OB_POC_BUS_ENDPOINT") {
                builder = builder.add_peer("ob-poc", uri);
            }
            if let Ok(uri) = std::env::var("DMN_LITE_BUS_ENDPOINT") {
                builder = builder.add_peer("dmn-lite", uri);
            }
            let client = builder.build().await?;
            Some(Arc::new(client))
        }
        _ => None,
    };

    let engine_builder =
        BpmnLiteEngine::new(store.clone()).with_ffi_dispatcher(ffi_dispatcher.clone());
    let engine = Arc::new(engine_builder);
    let event_fanout = Arc::new(EventFanout::new(
        engine.clone(),
        std::time::Duration::from_millis(parse_u64_env("BPMN_LITE_EVENT_FANOUT_FALLBACK_MS", 500)),
    ));
    #[cfg(feature = "postgres")]
    if let Some(url) = postgres_listener_url {
        event_fanout.start_postgres_listener(url).await?;
        tracing::info!("Postgres LISTEN/NOTIFY event fanout enabled");
    }

    // ────────────────────────────────────────────────────────────────
    // T2B.9 — Federated DSL bus runtime
    //
    // Co-located with the 50051 BPMN gRPC server: the bus binds on its
    // own port (default 50063) and shares the runtime pool. Disabled
    // when BPMN_LITE_BUS_LISTEN is unset, so the existing deployment
    // story keeps working unchanged.
    //
    // Splitting the bus into a separate bin (`bpmn-lite-bus-server`)
    // is tech debt logged for after T3 — the ProcessAdvancer needs
    // an inter-process channel to the BPMN executor, and T3 owns
    // that contract.
    // ────────────────────────────────────────────────────────────────
    #[cfg(feature = "postgres")]
    let bus_runtime = match (
        bus_pool.take(),
        early_bus_client,
        std::env::var("BPMN_LITE_BUS_LISTEN"),
    ) {
        (Some(pool), Some(bc), Ok(listen)) => {
            let bind_addr: std::net::SocketAddr = listen
                .parse()
                .map_err(|e| config_error(&format!("invalid BPMN_LITE_BUS_LISTEN: {e}")))?;
            tracing::info!(bind_addr = %bind_addr, "starting bpmn-lite federated bus runtime");
            Some(
                bus_runtime::start(bus_runtime::BusRuntimeConfig {
                    pool,
                    bind_addr,
                    client: bc,
                    store: store.clone(),
                    engine: engine.clone(),
                })
                .await?,
            )
        }
        _ => {
            tracing::info!("federated bus runtime disabled (set BPMN_LITE_BUS_LISTEN to enable)");
            None
        }
    };

    let scheduler_owner = generate_scheduler_owner();
    let tick_batch_size = parse_usize_env("BPMN_LITE_TICK_BATCH_SIZE", 128);
    let tick_lease_ms = parse_u64_env("BPMN_LITE_TICK_LEASE_MS", 5_000);
    let tick_interval_ms = parse_u64_env("BPMN_LITE_TICK_INTERVAL_MS", 500);
    let scheduler_tenant_concurrency = parse_usize_env(
        "BPMN_LITE_SCHEDULER_TENANT_CONCURRENCY",
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
    );
    let dedupe_retention_ms =
        parse_u64_env("BPMN_LITE_DEDUPE_RETENTION_MS", 7 * 24 * 60 * 60 * 1_000);

    let recovery = engine
        .recover_all_tenants(&scheduler_owner, tick_batch_size, tick_lease_ms)
        .await?;
    tracing::info!(
        tenants = recovery.tenants_scanned,
        instances = recovery.instances_verified,
        timers = recovery.timers_applied,
        effects = recovery.interrupted_effects,
        stale_jobs = recovery.stale_jobs_reclaimed,
        "startup recovery gate passed"
    );

    // Background: reclaim stale claimed jobs (every 60s, 5min timeout)
    let reclaim_store = store.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            match reclaim_store.reclaim_stale_jobs(5 * 60 * 1000).await {
                Ok(n) if n > 0 => tracing::warn!(reclaimed = n, "Reclaimed stale jobs"),
                Err(e) => tracing::error!(error = %e, "Job reclaim failed"),
                _ => {}
            }
        }
    });

    // Background: claim and tick a bounded batch of running instances per tenant.
    // The scheduler enumerates all known tenants from the tenants table (no RLS
    // on that table), then ticks each tenant's instances independently.
    let tick_engine = engine.clone();
    let tick_store = store.clone();
    let tick_owner = scheduler_owner.clone();
    tokio::spawn(async move {
        let mut tenant_cursor = 0usize;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(tick_interval_ms)).await;
            let mut tenants = match tick_store.list_tenants().await {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!(error = %e, "scheduler: failed to list tenants");
                    continue;
                }
            };
            if tenants.is_empty() {
                continue;
            }
            tenant_cursor %= tenants.len();
            tenants.rotate_left(tenant_cursor);
            tenant_cursor = tenant_cursor.saturating_add(1);
            stream::iter(tenants)
                .for_each_concurrent(scheduler_tenant_concurrency, |tenant_id| {
                    let tick_owner = tick_owner.clone();
                    let tick_engine = tick_engine.clone();
                    async move {
                        let tenant_id = match bpmn_lite_types::TenantId::new(tenant_id) {
                            Ok(tenant_id) => tenant_id,
                            Err(error) => {
                                tracing::error!(%error, "scheduler received invalid tenant id");
                                return;
                            }
                        };
                        let engine = tick_engine.for_tenant(tenant_id.clone());
                        let logical_time = match std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                        {
                            Ok(duration) => duration.as_millis() as u64,
                            Err(error) => {
                                tracing::error!(tenant_id = %tenant_id, error = %error, "scheduler clock is before Unix epoch");
                                return;
                            }
                        };
                        if let Err(error) = engine
                            .tick_due_timers(
                                &tick_owner,
                                logical_time,
                                tick_batch_size,
                                tick_lease_ms,
                            )
                            .await
                        {
                            tracing::error!(tenant_id = %tenant_id, %error, "durable timer batch failed");
                        }
                        if let Err(error) = engine
                            .dispatch_pending_effects(
                                &tick_owner,
                                tick_batch_size,
                                tick_lease_ms,
                            )
                            .await
                        {
                            tracing::error!(tenant_id = %tenant_id, %error, "durable effect batch failed");
                        }
                        if let Err(error) = engine
                            .tick_claimed_batch(&tick_owner, tick_batch_size, tick_lease_ms)
                            .await
                        {
                            tracing::error!(tenant_id = %tenant_id, %error, "scheduler tick batch failed");
                        }
                    }
                })
                .await;
        }
    });

    // Background: prune dedupe cache using the configured late-delivery window.
    let prune_store = store.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            match prune_store.prune_dedupe_cache(dedupe_retention_ms).await {
                Ok(n) if n > 0 => tracing::info!(pruned = n, "Pruned dedupe cache"),
                Err(e) => tracing::error!(error = %e, "Dedupe prune failed"),
                _ => {}
            }
        }
    });

    // Validate that every ExecFfi instruction in stored programs has a registered owner.
    let coverage_gaps = ffi_dispatcher.validate_coverage().await;
    if coverage_gaps.is_empty() {
        tracing::info!("FFI coverage validated: all stored programs have registered owners");
    } else {
        for gap in &coverage_gaps {
            let template_id_hex: String =
                gap.template_id.iter().map(|b| format!("{b:02x}")).collect();
            let reason = format!("{:?}", gap.reason);
            tracing::warn!(
                template_id = %template_id_hex,
                reason = %reason,
                "FFI coverage gap: stored program references unregistered template"
            );
        }
        if is_production_environment() {
            return Err(config_error(
                "production readiness denied: stored FFI operations lack dispatch owners",
            ));
        }
        tracing::warn!(
            gaps = coverage_gaps.len(),
            "FFI coverage gaps detected at startup"
        );
    }

    tracing::info!(
        bind_addr = %addr,
        store_mode = %store_mode,
        scheduler_owner = %scheduler_owner,
        "BPMN-Lite gRPC server starting"
    );

    let service = BpmnLiteService {
        engine: engine.clone(),
        event_fanout,
        limits: RequestLimits::from_env(),
        metrics: Arc::new(ServerMetrics::default()),
        subscription_limiter: Arc::new(Semaphore::new(parse_usize_env(
            "BPMN_LITE_MAX_EVENT_SUBSCRIPTIONS",
            256,
        ))),
        ffi_owner,
        http_ffi_owner,
        grpc_ffi_owner,
        ffi_catalogue: ffi_cat,
        ffi_store,
    };
    let max_message_bytes = parse_usize_env("BPMN_LITE_GRPC_MAX_MESSAGE_BYTES", 4 * 1024 * 1024);

    // B3 — Standard gRPC health endpoint (grpc.health.v1.Health).
    // grpc_health_probe and compatible clients use this protocol.
    // The custom rpc Health(HealthRequest) in bpmn_lite.proto remains
    // as the platform-specific deep health check.
    let (mut health_reporter, health_service) = health_reporter();
    health_reporter
        .set_serving::<BpmnLiteServer<BpmnLiteService>>()
        .await;
    tracing::info!("gRPC standard health service registered (grpc.health.v1.Health)");

    tracing::info!("BPMN-Lite gRPC server listening on {}", addr);

    #[cfg(unix)]
    let mut sigterm = {
        use tokio::signal::unix::{signal, SignalKind};
        signal(SignalKind::terminate())?
    };
    let shutdown_signal = async move {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        {
            tokio::select! {
                _ = ctrl_c => {}
                _ = sigterm.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = ctrl_c.await;
        }

        tracing::info!("shutdown signal received — draining in-flight requests");
    };

    Server::builder()
        .timeout(std::time::Duration::from_secs(parse_u64_env(
            "BPMN_LITE_GRPC_TIMEOUT_SECS",
            30,
        )))
        .concurrency_limit_per_connection(parse_usize_env(
            "BPMN_LITE_GRPC_CONCURRENCY_PER_CONNECTION",
            256,
        ))
        .add_service(health_service)
        .add_service(
            BpmnLiteServer::new(service)
                .max_decoding_message_size(max_message_bytes)
                .max_encoding_message_size(max_message_bytes),
        )
        .serve_with_shutdown(addr, shutdown_signal)
        .await?;

    tracing::info!("BPMN-Lite gRPC server stopped");

    #[cfg(feature = "postgres")]
    if let Some(runtime) = bus_runtime {
        if let Err(e) = runtime.shutdown().await {
            tracing::warn!(error = %e, "federated bus runtime shutdown reported error");
        } else {
            tracing::info!("federated bus runtime stopped cleanly");
        }
    }

    Ok(())
}

fn generate_scheduler_owner() -> String {
    match std::env::var("BPMN_LITE_SCHEDULER_OWNER") {
        Ok(label) => format!("{}-{}", label, Uuid::now_v7()),
        Err(_) => format!("bpmn-lite-{}", Uuid::now_v7()),
    }
}

fn parse_usize_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_u64_env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn config_error(message: &str) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.to_string(),
    ))
}

fn validate_production_configuration() -> Result<(), Box<dyn std::error::Error>> {
    if !is_production_environment() {
        return Ok(());
    }
    #[cfg(not(feature = "postgres"))]
    return Err(config_error(
        "production binary was compiled without the postgres feature",
    ));
    #[cfg(feature = "postgres")]
    {
        if !std::env::var("BPMN_LITE_STORE")
            .unwrap_or_else(|_| "postgres".to_string())
            .eq_ignore_ascii_case("postgres")
        {
            return Err(config_error("production requires BPMN_LITE_STORE=postgres"));
        }
        if std::env::var("DATABASE_URL").is_err() {
            return Err(config_error("production requires DATABASE_URL"));
        }
        let auth_mode = std::env::var("BPMN_LITE_AUTH_MODE")
            .map_err(|_| config_error("production requires BPMN_LITE_AUTH_MODE"))?;
        if auth_mode.eq_ignore_ascii_case("disabled") {
            return Err(config_error("production authentication cannot be disabled"));
        }
        let tls_mode = std::env::var("BPMN_LITE_TLS_MODE")
            .map_err(|_| config_error("production requires BPMN_LITE_TLS_MODE"))?;
        if !matches!(tls_mode.as_str(), "native" | "terminated") {
            return Err(config_error(
                "BPMN_LITE_TLS_MODE must be 'native' or 'terminated' in production",
            ));
        }
        let late_delivery_ms = parse_required_u64_env("BPMN_LITE_MAX_LATE_DELIVERY_MS")?;
        let dedupe_retention_ms = parse_required_u64_env("BPMN_LITE_DEDUPE_RETENTION_MS")?;
        if dedupe_retention_ms < late_delivery_ms {
            return Err(config_error(
                "dedupe retention must be at least the maximum late-delivery window",
            ));
        }
        let required_dispatchers = std::env::var("BPMN_LITE_REQUIRED_DISPATCHERS")
            .map_err(|_| config_error("production requires BPMN_LITE_REQUIRED_DISPATCHERS"))?;
        if required_dispatchers.trim().is_empty() {
            return Err(config_error(
                "production requires BPMN_LITE_REQUIRED_DISPATCHERS",
            ));
        }
        for dispatcher in required_dispatchers.split(',').map(str::trim) {
            match dispatcher {
                "ffi" => {}
                "bus" if std::env::var("BPMN_LITE_BUS_LISTEN").is_ok() => {}
                "bus" => {
                    return Err(config_error(
                        "bus is required but BPMN_LITE_BUS_LISTEN is not configured",
                    ));
                }
                unknown => {
                    return Err(config_error(&format!(
                        "unknown required dispatcher '{unknown}'"
                    )));
                }
            }
        }
        Ok(())
    }
}

fn is_production_environment() -> bool {
    std::env::var("BPMN_LITE_ENV")
        .map(|value| value.eq_ignore_ascii_case("production"))
        .unwrap_or(false)
}

fn parse_required_u64_env(name: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let value =
        std::env::var(name).map_err(|_| config_error(&format!("production requires {name}")))?;
    let parsed = value
        .parse::<u64>()
        .map_err(|_| config_error(&format!("{name} must be a positive integer")))?;
    if parsed == 0 {
        return Err(config_error(&format!("{name} must be greater than zero")));
    }
    Ok(parsed)
}

/// Parse database URL from `--database-url <url>` CLI arg or `DATABASE_URL` env var.
fn parse_database_url() -> Option<String> {
    // CLI arg takes precedence
    let args: Vec<String> = std::env::args().collect();
    if let Some(url) = args
        .windows(2)
        .find(|w| w[0] == "--database-url")
        .map(|w| w[1].clone())
    {
        return Some(url);
    }
    // Fall back to env var
    std::env::var("DATABASE_URL").ok()
}

/// A18-Session-1 — Warn (or, in A18-Session-3, refuse to start) if the
/// application is connected to PostgreSQL as a superuser role.
///
/// Postgres superusers automatically bypass row-level security (BYPASSRLS
/// is implicit), which means RLS policies have no effect even when
/// enabled. The deployment must use a non-superuser role (typically
/// `bpmn_lite_app`, created in migration 026) for runtime work.
///
/// This session emits a loud WARN. A18-Session-3 will tighten this to a
/// B11 — Refuse to start if the runtime connection is a Postgres superuser.
///
/// Superusers implicitly BYPASSRLS, defeating migration 025's RLS policies
/// and migration 029's immutable-field trigger (triggers can be disabled by
/// superusers). The deployment must connect as `bpmn_lite_app` or another
/// non-superuser role.
///
/// Set `BPMN_LITE_ALLOW_SUPERUSER=1` to downgrade to a WARN for local
/// development workflows that legitimately use the postgres role (e.g.
/// running tests directly against the dev database without docker-compose).
#[cfg(feature = "postgres")]
async fn verify_not_superuser(pool: &sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
    let is_superuser: bool =
        sqlx::query_scalar("SELECT rolsuper FROM pg_roles WHERE rolname = current_user")
            .fetch_one(pool)
            .await?;

    if is_superuser {
        if std::env::var("BPMN_LITE_ALLOW_SUPERUSER").unwrap_or_default() == "1" {
            tracing::warn!(
                "Connected as superuser. RLS BYPASSED. Proceeding because BPMN_LITE_ALLOW_SUPERUSER=1"
            );
            return Ok(());
        }
        return Err("Application connected to Postgres as a superuser role. \
             This BYPASSES row-level security and the immutable-field trigger \
             (migration 029). Connect using a non-superuser role (typically \
             bpmn_lite_app, created by migration 026)."
            .into());
    } else {
        tracing::info!(
            "Application connected as non-superuser role; \
             RLS and immutable-field trigger are active."
        );
    }

    Ok(())
}

/// Parse bind address from `--bind <addr>` CLI arg or `BPMN_LITE_BIND` env var.
fn parse_bind_addr() -> String {
    let args: Vec<String> = std::env::args().collect();
    if let Some(addr) = args
        .windows(2)
        .find(|w| w[0] == "--bind")
        .map(|w| w[1].clone())
    {
        return addr;
    }

    std::env::var("BPMN_LITE_BIND").unwrap_or_else(|_| "0.0.0.0:50051".to_string())
}

#[cfg(test)]
mod tests_owner {
    use super::*;

    #[test]
    fn test_generate_scheduler_owner() {
        std::env::set_var("BPMN_LITE_SCHEDULER_OWNER", "test-label");
        let owner = generate_scheduler_owner();
        assert!(owner.starts_with("test-label-"));
        let suffix = &owner["test-label-".len()..];
        assert!(uuid::Uuid::parse_str(suffix).is_ok());

        std::env::remove_var("BPMN_LITE_SCHEDULER_OWNER");
        let owner_default = generate_scheduler_owner();
        assert!(owner_default.starts_with("bpmn-lite-"));
        let suffix_default = &owner_default["bpmn-lite-".len()..];
        assert!(uuid::Uuid::parse_str(suffix_default).is_ok());
    }
}

#[cfg(test)]
#[cfg(feature = "postgres")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_verify_not_superuser_rejects_superuser() {
        let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://postgres@localhost:5432/bpmn_lite_test".to_string());

        let pool = match sqlx::PgPool::connect(&url).await {
            Ok(p) => p,
            Err(_) => return,
        };

        let res = verify_not_superuser(&pool).await;
        assert!(
            res.is_err(),
            "verify_not_superuser must reject superuser role"
        );

        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("superuser role"),
            "Error message should mention superuser role"
        );
    }
}
