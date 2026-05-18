use std::sync::Arc;
use uuid::Uuid;

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_ffi_grpc::GrpcFfiOwner;
use bpmn_lite_ffi_http::HttpFfiOwner;
use bpmn_lite_server::event_fanout::EventFanout;
use bpmn_lite_server::grpc::proto::bpmn_lite_server::BpmnLiteServer;
use bpmn_lite_server::grpc::{BpmnLiteService, RequestLimits, ServerMetrics};
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_store::store_memory::MemoryStore;
use dmn_lite_bridge::DmnLiteOwner;
use ffi_catalogue::{FfiCatalogue, MemoryFfiTemplateStore};
use ffi_dispatcher::FfiDispatcher;
use tokio::sync::Semaphore;
use tonic::transport::Server;
use tonic_health::server::health_reporter;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let addr = parse_bind_addr().parse()?;

    let database_url = parse_database_url();
    #[cfg(feature = "postgres")]
    let database_admin_url = std::env::var("DATABASE_ADMIN_URL").ok();
    #[cfg(feature = "postgres")]
    let postgres_listener_url = database_url.clone();

    let store_mode = std::env::var("BPMN_LITE_STORE").unwrap_or_else(|_| "postgres".to_string());
    let allow_memory = store_mode.eq_ignore_ascii_case("memory");

    let store: Arc<dyn ProcessStore> = match database_url {
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
                    bpmn_lite_store_postgres::PostgresProcessStore::new(admin_pool.clone());
                admin_store.migrate().await?;
                admin_pool.close().await;
                tracing::info!("Migrations applied; admin pool closed");
            }

            tracing::info!("Connecting to PostgreSQL for runtime...");
            let pool = sqlx::PgPool::connect(&url).await?;

            // A18-Session-1: warn (not yet error) if connected as superuser.
            verify_not_superuser(&pool).await?;

            let pg = bpmn_lite_store_postgres::PostgresProcessStore::new(pool);
            if database_admin_url.is_none() {
                // No admin URL — run migrations through the runtime pool
                // (legacy / dev path).
                pg.migrate().await?;
                tracing::info!("Using PostgresProcessStore (migrations applied via runtime pool)");
            } else {
                tracing::info!(
                    "Using PostgresProcessStore (migrations already applied via admin pool)"
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
    ffi_dispatcher
        .register_owner(ffi_owner.clone())
        .expect("register DmnLiteOwner");
    ffi_dispatcher
        .register_owner(http_ffi_owner.clone())
        .expect("register HttpFfiOwner");
    ffi_dispatcher
        .register_owner(grpc_ffi_owner.clone())
        .expect("register GrpcFfiOwner");
    let ffi_dispatcher = Arc::new(ffi_dispatcher);
    tracing::info!("FFI dispatcher initialised with dmn-lite + http + grpc execution owners");

    let engine =
        Arc::new(BpmnLiteEngine::new(store.clone()).with_ffi_dispatcher(ffi_dispatcher.clone()));
    let event_fanout = Arc::new(EventFanout::new(
        engine.clone(),
        std::time::Duration::from_millis(parse_u64_env("BPMN_LITE_EVENT_FANOUT_FALLBACK_MS", 500)),
    ));
    #[cfg(feature = "postgres")]
    if let Some(url) = postgres_listener_url {
        event_fanout.start_postgres_listener(url).await?;
        tracing::info!("Postgres LISTEN/NOTIFY event fanout enabled");
    }

    let scheduler_owner = std::env::var("BPMN_LITE_SCHEDULER_OWNER")
        .unwrap_or_else(|_| format!("bpmn-lite-{}", Uuid::now_v7()));
    let tick_batch_size = parse_usize_env("BPMN_LITE_TICK_BATCH_SIZE", 128);
    let tick_lease_ms = parse_u64_env("BPMN_LITE_TICK_LEASE_MS", 5_000);
    let tick_interval_ms = parse_u64_env("BPMN_LITE_TICK_INTERVAL_MS", 500);

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
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(tick_interval_ms)).await;
            let tenants = match tick_store.list_tenants().await {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!(error = %e, "scheduler: failed to list tenants");
                    continue;
                }
            };
            for tenant_id in tenants {
                let engine = tick_engine.for_tenant(&tenant_id);
                if let Err(e) = engine
                    .tick_claimed_batch(&tick_owner, tick_batch_size, tick_lease_ms)
                    .await
                {
                    tracing::error!(tenant_id = %tenant_id, error = %e, "scheduler tick batch failed");
                }
            }
        }
    });

    // Background: prune dedupe cache (hourly, 24h TTL)
    let prune_store = store.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            match prune_store.prune_dedupe_cache(24 * 3600 * 1000).await {
                Ok(n) if n > 0 => tracing::info!(pruned = n, "Pruned dedupe cache"),
                Err(e) => tracing::error!(error = %e, "Dedupe prune failed"),
                _ => {}
            }
        }
    });

    // A17 — Detect interrupted FFI calls from a previous crash.
    match engine.detect_interrupted_ffi_calls("default").await {
        Ok(0) => tracing::info!("A17: no interrupted FFI calls detected"),
        Ok(n) => tracing::warn!(
            count = n,
            "A17: {} interrupted FFI call(s) detected; see above for details",
            n
        ),
        Err(e) => tracing::warn!(error = %e, "A17: interrupted FFI call scan failed (non-fatal)"),
    }

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

    let shutdown_signal = async {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm =
                signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
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
    Ok(())
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
        let allow = std::env::var("BPMN_LITE_ALLOW_SUPERUSER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        if allow {
            tracing::warn!(
                "Application connected to Postgres as a superuser role. \
                 BPMN_LITE_ALLOW_SUPERUSER=1 is set — continuing with reduced \
                 security guarantees. Do not use this in production."
            );
        } else {
            return Err("Application connected to Postgres as a superuser role. \
                 This BYPASSES row-level security and the immutable-field trigger \
                 (migration 029). Connect using a non-superuser role (typically \
                 bpmn_lite_app, created by migration 026). \
                 Set BPMN_LITE_ALLOW_SUPERUSER=1 to override for local development."
                .into());
        }
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
