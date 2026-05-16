use std::sync::Arc;
use uuid::Uuid;

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_server::event_fanout::EventFanout;
use bpmn_lite_server::grpc::proto::bpmn_lite_server::BpmnLiteServer;
use bpmn_lite_server::grpc::{BpmnLiteService, RequestLimits, ServerMetrics};
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_store::store_memory::MemoryStore;
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

    let engine = Arc::new(BpmnLiteEngine::new(store.clone()));
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

    tracing::info!("BPMN-Lite gRPC server listening on {}", addr);

    let service = BpmnLiteService {
        engine: engine.clone(),
        event_fanout,
        limits: RequestLimits::from_env(),
        metrics: Arc::new(ServerMetrics::default()),
        subscription_limiter: Arc::new(Semaphore::new(parse_usize_env(
            "BPMN_LITE_MAX_EVENT_SUBSCRIPTIONS",
            256,
        ))),
    };
    let max_message_bytes = parse_usize_env("BPMN_LITE_GRPC_MAX_MESSAGE_BYTES", 4 * 1024 * 1024);

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
        .serve(addr)
        .await?;

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
