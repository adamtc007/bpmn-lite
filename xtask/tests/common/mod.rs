//! Shared fixture for the `bpmn-lite-store-postgres` multi-crate application
//! vertical tests moved into `xtask/tests/` under
//! EOP-PLAN-CRATE-HYGIENE-001 H1 (work item 3).
//!
//! `bpmn-lite-store-postgres/src/store_postgres.rs`'s own `mod tests`
//! carries a private `setup()`/`make_instance()`/`save_instance()` fixture
//! trio (reached via `use super::*` from inside the crate) that ~90 SQL/
//! persistence-contract tests still use and that stays put. Those helpers
//! are not `pub` — `#[cfg(test)]`-only items do not survive the crate
//! boundary, and their SQL-role internals (e.g. `self.pool`,
//! `set_tenant_context`) are private implementation detail, not a
//! supported capability contract (plan R2). Duplicating the ~150-line
//! fixture here — once, shared by every moved file via `mod common;` —
//! is the smaller footprint than either exposing test-only production
//! surface or copy-pasting it three times; `save_instance` below is
//! reimplemented purely against `bpmn-lite-store`'s public
//! `RuntimeStore`/`AdminProjectionStore` trait API (the same one the
//! in-crate version composes), not against private fields.
//!
//! `#[allow(dead_code)]`: each of the three vertical-test files uses a
//! different subset of this fixture; the trait check for "unused" is
//! per-binary (`tests/*.rs` are each their own crate), so a subset used by
//! only one of the three files must not be flagged in the other two.

#![allow(dead_code)]

use bpmn_lite_store::{
    store::{AdminProjectionStore, RuntimeStore},
    StoreError, StoreResult,
};
use bpmn_lite_store_postgres::PostgresWorkflowStore;
use bpmn_lite_types::{Claim, ProcessInstance, ProcessState, TenantId, TransitionBuilder, Value};
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use uuid::Uuid;

const DEFAULT_TEST_DATABASE_URL: &str = "postgresql://localhost/bpmn_lite_test";

static TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn get_mutex() -> &'static tokio::sync::Mutex<()> {
    TEST_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Connects, runs both the bpmn-lite and dsl-bus migration sets, grants the
/// constrained `bpmn_lite_app` role, and truncates every table a prior run
/// of these tests could have left dirty — mirroring
/// `bpmn-lite-store-postgres/src/store_postgres.rs`'s in-crate `setup()`
/// verbatim, since that helper is private and not reachable from here.
pub(crate) async fn setup() -> (
    PgPool,
    PostgresWorkflowStore,
    tokio::sync::MutexGuard<'static, ()>,
) {
    let guard = get_mutex().lock().await;
    let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
    let pool = PgPool::connect(&url).await.expect("connect to db");

    // Run migrations
    let migrator = sqlx::migrate!("../bpmn-lite-store-postgres/migrations");
    migrator.run(&pool).await.expect("run migrations");

    let db_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let grant_sql = format!(
        "GRANT CONNECT, TEMPORARY ON DATABASE \"{}\" TO bpmn_lite_app",
        db_name
    );
    sqlx::query(&grant_sql).execute(&pool).await.unwrap();
    sqlx::query("GRANT USAGE ON SCHEMA public TO bpmn_lite_app")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO bpmn_lite_app",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO bpmn_lite_app")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("DROP SCHEMA IF EXISTS dsl_bus CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE SCHEMA dsl_bus")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("GRANT USAGE ON SCHEMA dsl_bus TO bpmn_lite_app")
        .execute(&pool)
        .await
        .unwrap();

    // Run bus migrations under admin role with search_path=dsl_bus
    let mut bus_admin_url = url.clone();
    if bus_admin_url.contains('?') {
        bus_admin_url.push_str("&options=-csearch_path%3Ddsl_bus");
    } else {
        bus_admin_url.push_str("?options=-csearch_path%3Ddsl_bus");
    }
    let bus_admin_pool = PgPool::connect(&bus_admin_url)
        .await
        .expect("connect to db for bus migrations");
    dsl_bus_storage::migrate(&bus_admin_pool)
        .await
        .expect("run bus migrations");
    bus_admin_pool.close().await;

    // Grant bpmn_lite_app DML-only privileges on dsl_bus tables and sequences
    sqlx::query(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dsl_bus TO bpmn_lite_app",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA dsl_bus TO bpmn_lite_app")
        .execute(&pool)
        .await
        .unwrap();

    use std::str::FromStr;
    let mut options = sqlx::postgres::PgConnectOptions::from_str(&url).expect("parse db url");
    options = options
        .username("bpmn_lite_app")
        .password("bpmn_lite_app_dev_password");
    let app_pool = PgPool::connect_with(options)
        .await
        .expect("connect as bpmn_lite_app");

    // Truncate all tables
    sqlx::query("TRUNCATE dsl_bus.outbox CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    // FK-orphaned live tables — not reachable by any CASCADE, so a reused
    // test DB would leak them across tests. Named here so the harness
    // exercises the same completeness the cutover wipe script must have.
    sqlx::query("TRUNCATE dsl_bus.inbox CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE message_buffer")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("TRUNCATE workflow_instances CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE compiled_programs")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE job_queue")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE dedupe_cache")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE message_dedupe")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE dead_letter_queue")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE event_sequences")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE event_log")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE payload_history")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE incidents")
        .execute(&pool)
        .await
        .unwrap();
    // Artifact migration lineage persists a new_hash per old_hash; without
    // this truncation a re-run of the migration tests collides on a hash
    // stored by the prior run (the artifact hash is content-deterministic).
    sqlx::query("TRUNCATE artifact_lineage")
        .execute(&pool)
        .await
        .unwrap();

    let store = PostgresWorkflowStore::new(app_pool);
    (pool, store, guard)
}

pub(crate) fn test_hash(data: &str) -> [u8; 32] {
    blake3::hash(data.as_bytes()).into()
}

pub(crate) fn make_instance(id: Uuid) -> ProcessInstance {
    let payload = r#"{"case_id":"abc"}"#;
    let hash = test_hash(payload);
    ProcessInstance {
        instance_id: id,
        tenant_id: "default".to_string(),
        process_key: "test-process".to_string(),
        bytecode_version: [0u8; 32],
        domain_payload: payload.to_string().into(),
        domain_payload_hash: hash,
        session_stack: bpmn_lite_types::session_stack::SessionStackState::default(),
        flags: BTreeMap::from([(0, Value::Bool(true)), (1, Value::I64(42))]),
        counters: BTreeMap::from([(0, 5), (1, 10)]),
        join_expected: BTreeMap::from([(0, 3)]),
        state: ProcessState::Running,
        correlation_id: "runbook-entry-1".to_string(),
        entry_id: Uuid::nil(),
        runbook_id: Uuid::nil(),
        created_at: 1700000000000,
        integrity_hash: None,
        quarantine_state: None,
        plan_hash: None,
        current_node_id: None,
        placeholder_values: None,
    }
}

/// Reimplementation of the in-crate test-only `PostgresWorkflowStore::
/// save_instance` inherent method, against the store's public
/// `RuntimeStore`/`AdminProjectionStore` trait contract only (no private
/// field or method access) — see the module doc comment.
pub(crate) async fn save_instance(
    store: &PostgresWorkflowStore,
    lease_owner: &str,
    instance: &ProcessInstance,
) -> StoreResult<()> {
    store
        .ensure_tenant(&TenantId::new(instance.tenant_id.clone()).map_err(StoreError::invalid)?)
        .await?;
    if store
        .load_instance(&TenantId::new("default").unwrap(), instance.instance_id)
        .await?
        .is_none()
    {
        let tenant = TenantId::new(instance.tenant_id.clone()).map_err(StoreError::invalid)?;
        store
            .commit_transition(
                &Claim::new(tenant, instance.instance_id, 0, 0, ""),
                &TransitionBuilder::new(instance.clone()).build(),
            )
            .await
            .map(|_| ())
            .map_err(StoreError::integrity)
    } else {
        let claim = store
            .claim_instance_for_transition(
                &TenantId::new(instance.tenant_id.clone()).map_err(StoreError::invalid)?,
                instance.instance_id,
                lease_owner,
                30_000,
            )
            .await
            .map_err(StoreError::integrity)?
            .ok_or_else(|| StoreError::Integrity("fixture instance is leased".to_string()))?;
        let result = store
            .commit_transition(&claim, &TransitionBuilder::new(instance.clone()).build())
            .await
            .map(|_| ())
            .map_err(StoreError::integrity);
        store
            .release_instance_transition(
                &TenantId::new(instance.tenant_id.clone()).map_err(StoreError::invalid)?,
                instance.instance_id,
                claim.lease_token(),
            )
            .await?;
        result
    }
}
