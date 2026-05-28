//! `dmn-lite-server` — federated DSL bus host for the dmn-lite
//! decision vocabulary (v0.6 §T2B.9, item 36).
//!
//! Bootstrap responsibilities only — actual work lives in the sibling
//! modules:
//!
//! - [`catalogue`] — load + compile + verify `.dmn-lite` sources.
//! - [`evaluator`] — `DecisionEvaluator` impl over the catalogue.
//! - [`runtime`] — bus runtime wiring (BusClient + sender + BusServer).
//!
//! Configuration is env-driven so a future docker-compose deployment
//! can drive it without touching code.

mod catalogue;
mod evaluator;
mod runtime;

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use sqlx::PgPool;
use tracing_subscriber::EnvFilter;

const DEFAULT_BIND: &str = "0.0.0.0:50062";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let cfg = StartupConfig::from_env()?;
    tracing::info!(
        database_url = mask_url(&cfg.database_url),
        bind_addr = %cfg.bind_addr,
        decisions_dir = %cfg.decisions_dir.display(),
        catalogue_toml = %cfg.catalogue_toml.display(),
        allowlist_yaml = %cfg.allowlist_yaml.display(),
        catalogue_version = %cfg.catalogue_version,
        bpmn_lite_endpoint = ?cfg.bpmn_lite_endpoint,
        "dmn-lite-server starting"
    );

    let allowlist = load_allowlist(&cfg.allowlist_yaml)?;
    let catalogue = Arc::new(catalogue::build(
        &cfg.decisions_dir,
        &cfg.catalogue_toml,
        &allowlist,
    )?);
    tracing::info!(
        loaded = catalogue.len(),
        ids = ?catalogue.ids().collect::<Vec<_>>(),
        "decision catalogue ready"
    );

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .with_context(|| format!("connect to {}", mask_url(&cfg.database_url)))?;

    let mut peers = Vec::new();
    if let Some(endpoint) = cfg.bpmn_lite_endpoint.as_ref() {
        peers.push(("bpmn-lite".to_owned(), endpoint.clone()));
    }

    let runtime = runtime::start(runtime::BusRuntimeConfig {
        pool,
        catalogue,
        bind_addr: cfg.bind_addr,
        catalogue_version: cfg.catalogue_version.clone(),
        peers,
    })
    .await
    .context("start dmn-lite bus runtime")?;

    wait_for_shutdown().await;
    tracing::info!("shutdown signal received — stopping dmn-lite bus runtime");
    runtime.shutdown().await?;
    tracing::info!("dmn-lite-server stopped cleanly");
    Ok(())
}

struct StartupConfig {
    database_url: String,
    bind_addr: SocketAddr,
    decisions_dir: PathBuf,
    catalogue_toml: PathBuf,
    allowlist_yaml: PathBuf,
    /// Catalogue version this server claims to host. Defaults to the
    /// `manifests/dmn-lite-vX.Y.Z.yaml` convention. Override via
    /// `DMN_LITE_CATALOGUE_VERSION` env.
    catalogue_version: String,
    bpmn_lite_endpoint: Option<String>,
}

impl StartupConfig {
    fn from_env() -> Result<Self> {
        let database_url = env::var("DMN_LITE_DATABASE_URL")
            .or_else(|_| env::var("DATABASE_URL"))
            .context(
                "DMN_LITE_DATABASE_URL (or fallback DATABASE_URL) must be set — \
                 dmn-lite-server's bus uses Postgres for outbox/inbox durability",
            )?;

        let bind_addr: SocketAddr = env::var("DMN_LITE_BUS_LISTEN")
            .unwrap_or_else(|_| DEFAULT_BIND.to_owned())
            .parse()
            .context("parse DMN_LITE_BUS_LISTEN as <ip:port>")?;

        let decisions_dir = env::var("DMN_LITE_DECISIONS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_repo_path("dmn-lite-decisions"));

        let catalogue_toml = env::var("DMN_LITE_CATALOGUE_TOML")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_repo_path("test-data/sem-os-stub.toml"));

        let allowlist_yaml = env::var("DMN_LITE_ALLOWLIST")
            .map(PathBuf::from)
            .unwrap_or_else(|_| decisions_dir.join("manifest-allowlist.yaml"));

        let bpmn_lite_endpoint = env::var("BPMN_LITE_BUS_ENDPOINT").ok();
        let catalogue_version =
            env::var("DMN_LITE_CATALOGUE_VERSION").unwrap_or_else(|_| "v1.0.0".to_owned());

        Ok(Self {
            database_url,
            bind_addr,
            decisions_dir,
            catalogue_toml,
            allowlist_yaml,
            catalogue_version,
            bpmn_lite_endpoint,
        })
    }
}

fn default_repo_path(rel: &str) -> PathBuf {
    // CARGO_MANIFEST_DIR points at `dmn-lite-server/` at build time; the
    // catalogue + decisions live at the workspace root.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("..").join(rel)
}

fn load_allowlist(path: &std::path::Path) -> Result<Vec<String>> {
    #[derive(serde::Deserialize, Default)]
    struct Allowlist {
        #[serde(default)]
        public_decisions: Vec<String>,
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read allowlist {}", path.display()))?;
    let parsed: Allowlist = serde_yaml::from_str(&text)
        .with_context(|| format!("parse allowlist YAML {}", path.display()))?;
    Ok(parsed.public_decisions)
}

fn mask_url(url: &str) -> String {
    // Strip the credential portion of a Postgres URL for log output.
    if let Some(at) = url.find('@')
        && let Some(scheme_end) = url.find("://")
    {
        let head = &url[..scheme_end + 3];
        let tail = &url[at + 1..];
        return format!("{head}***:***@{tail}");
    }
    url.to_owned()
}

async fn wait_for_shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("register SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}
