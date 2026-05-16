use std::sync::Arc;
use uuid::Uuid;

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_server::event_fanout::EventFanout;
use bpmn_lite_server::grpc::proto::bpmn_lite_server::BpmnLiteServer;
use bpmn_lite_server::grpc::{BpmnLiteService, RequestLimits, ServerMetrics};
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_ffi_grpc::GrpcFfiOwner;
use bpmn_lite_ffi_http::HttpFfiOwner;
use dmn_lite_bridge::DmnLiteOwner;
use ffi_catalogue::{FfiCatalogue, MemoryFfiTemplateStore};
use ffi_dispatcher::FfiDispatcher;
use tokio::sync::Semaphore;
use tonic::transport::Server;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let addr = parse_bind_addr().parse()?;

    let database_url = parse_database_url();
    #[cfg(feature = "postgres")]
    let postgres_listener_url = database_url.clone();

    let store_mode = std::env::var("BPMN_LITE_STORE").unwrap_or_else(|_| "postgres".to_string());
    let allow_memory = store_mode.eq_ignore_ascii_case("memory");

    let store: Arc<dyn ProcessStore> = match database_url {
        #[cfg(feature = "postgres")]
        Some(url) => {
            tracing::info!("Connecting to PostgreSQL...");
            let pool = sqlx::PgPool::connect(&url).await?;
            let pg = bpmn_lite_store_postgres::PostgresProcessStore::new(pool);
            pg.migrate().await?;
            tracing::info!("Using PostgresProcessStore (migrations applied)");
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

    let engine = Arc::new(BpmnLiteEngine::new(store.clone()).with_ffi_dispatcher(ffi_dispatcher.clone()));
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

    // Background: claim and tick a bounded batch of running instances.
    let tick_engine = engine.clone();
    let tick_owner = scheduler_owner.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(tick_interval_ms)).await;
            if let Err(e) = tick_engine
                .tick_claimed_batch(&tick_owner, tick_batch_size, tick_lease_ms)
                .await
            {
                tracing::error!(error = %e, "scheduler tick batch failed");
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
        Ok(n) => tracing::warn!(count = n, "A17: {} interrupted FFI call(s) detected; see above for details", n),
        Err(e) => tracing::warn!(error = %e, "A17: interrupted FFI call scan failed (non-fatal)"),
    }

    // Validate that every ExecFfi instruction in stored programs has a registered owner.
    let coverage_gaps = ffi_dispatcher.validate_coverage().await;
    if coverage_gaps.is_empty() {
        tracing::info!("FFI coverage validated: all stored programs have registered owners");
    } else {
        for gap in &coverage_gaps {
            let template_id_hex: String = gap.template_id.iter().map(|b| format!("{b:02x}")).collect();
            let reason = format!("{:?}", gap.reason);
            tracing::warn!(
                template_id = %template_id_hex,
                reason = %reason,
                "FFI coverage gap: stored program references unregistered template"
            );
        }
        tracing::warn!(gaps = coverage_gaps.len(), "FFI coverage gaps detected at startup");
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

    tracing::info!("BPMN-Lite gRPC server listening on {}", addr);

    let shutdown_signal = async {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate())
                .expect("failed to register SIGTERM handler");
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
