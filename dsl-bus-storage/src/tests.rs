//! Integration tests for outbox / inbox CRUD.
//!
//! All tests are `#[ignore]` because they touch a real Postgres. Run
//! them via:
//!
//! ```text
//! BPMN_LITE_TEST_DATABASE_URL=postgresql://localhost/bpmn_lite_test \
//!     cargo test -p dsl-bus-storage -- --ignored --test-threads=1
//! ```
//!
//! Each test takes a fresh pool, runs the migrations, truncates the
//! two tables, and exercises one slice of the CRUD surface.

use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    insert_inbox, insert_outbox, lookup_inbox, mark_inbox_processed, mark_outbox_retry,
    mark_outbox_submitted, select_pending_outbox, BusEndpoint, InboxEntry, InboxStatus,
    InsertOutcome, OutboxEntry, OutboxStatus,
};

const DEFAULT_TEST_DATABASE_URL: &str = "postgresql://localhost/bpmn_lite_test";

async fn setup() -> PgPool {
    let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_owned());
    let pool = PgPool::connect(&url).await.expect("connect to db");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    sqlx::query("TRUNCATE dsl_bus.outbox")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE dsl_bus.inbox")
        .execute(&pool)
        .await
        .unwrap();
    pool
}

fn sample_outbox(idempotency_key: Uuid) -> OutboxEntry {
    OutboxEntry::new_pending(
        Uuid::now_v7(),
        "ob-poc",
        BusEndpoint::Invocation,
        b"protobuf-bytes-here".to_vec(),
        idempotency_key,
    )
}

fn sample_inbox(idempotency_key: Uuid) -> InboxEntry {
    InboxEntry::new_received(
        idempotency_key,
        "bpmn-lite",
        BusEndpoint::Invocation,
        Some(Uuid::now_v7()),
        Some(b"raw-request-protobuf".to_vec()),
    )
}

// ── Outbox ───────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn outbox_insert_then_select_pending_returns_row() {
    let pool = setup().await;
    let entry = sample_outbox(Uuid::now_v7());

    let outcome = insert_outbox(&pool, &entry).await.unwrap();
    assert_eq!(outcome, InsertOutcome::Inserted);

    let mut tx = pool.begin().await.unwrap();
    let rows = select_pending_outbox(&mut tx, 10).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, entry.id);
    assert_eq!(rows[0].target_domain, "ob-poc");
    assert_eq!(rows[0].target_endpoint, BusEndpoint::Invocation);
    assert_eq!(rows[0].status, OutboxStatus::Pending);
    assert_eq!(rows[0].attempt_count, 0);
}

#[tokio::test]
#[ignore]
async fn outbox_insert_is_idempotent_on_key_plus_endpoint() {
    let pool = setup().await;
    let key = Uuid::now_v7();

    let first = insert_outbox(&pool, &sample_outbox(key)).await.unwrap();
    let second = insert_outbox(&pool, &sample_outbox(key)).await.unwrap();

    assert_eq!(first, InsertOutcome::Inserted);
    assert_eq!(second, InsertOutcome::Duplicate);

    let mut tx = pool.begin().await.unwrap();
    let rows = select_pending_outbox(&mut tx, 10).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(rows.len(), 1, "duplicate insert must not multiply rows");
}

#[tokio::test]
#[ignore]
async fn outbox_same_key_different_endpoint_is_a_distinct_row() {
    let pool = setup().await;
    let key = Uuid::now_v7();

    let invocation = sample_outbox(key);
    let mut result = sample_outbox(key);
    result.target_endpoint = BusEndpoint::Result;
    result.id = Uuid::now_v7();

    assert_eq!(
        insert_outbox(&pool, &invocation).await.unwrap(),
        InsertOutcome::Inserted
    );
    assert_eq!(
        insert_outbox(&pool, &result).await.unwrap(),
        InsertOutcome::Inserted
    );

    let mut tx = pool.begin().await.unwrap();
    let rows = select_pending_outbox(&mut tx, 10).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
#[ignore]
async fn outbox_mark_submitted_records_execution_id_and_transitions_status() {
    let pool = setup().await;
    let entry = sample_outbox(Uuid::now_v7());
    insert_outbox(&pool, &entry).await.unwrap();

    let exec_id = Uuid::now_v7();
    mark_outbox_submitted(&pool, entry.id, exec_id)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let rows = select_pending_outbox(&mut tx, 10).await.unwrap();
    tx.commit().await.unwrap();
    assert!(rows.is_empty(), "submitted row must not appear on pending sweep");

    let row = fetch_row_by_id(&pool, entry.id).await;
    assert_eq!(row.status, OutboxStatus::Submitted);
    assert_eq!(row.execution_id, Some(exec_id));
    assert!(row.submitted_at.is_some());
}

#[tokio::test]
#[ignore]
async fn outbox_mark_retry_bumps_attempt_count_and_defers_next_attempt() {
    let pool = setup().await;
    let entry = sample_outbox(Uuid::now_v7());
    insert_outbox(&pool, &entry).await.unwrap();

    mark_outbox_retry(&pool, entry.id, 60, "connection refused")
        .await
        .unwrap();

    // Immediate sweep must not see the row: next_attempt_at is in the future.
    let mut tx = pool.begin().await.unwrap();
    let immediate = select_pending_outbox(&mut tx, 10).await.unwrap();
    tx.commit().await.unwrap();
    assert!(immediate.is_empty(), "deferred row must be invisible to sweep");

    let row = fetch_row_by_id(&pool, entry.id).await;
    assert_eq!(row.status, OutboxStatus::Pending);
    assert_eq!(row.attempt_count, 1);
    assert_eq!(row.last_error.as_deref(), Some("connection refused"));
    assert!(row.next_attempt_at > entry.next_attempt_at);
}

#[tokio::test]
#[ignore]
async fn outbox_concurrent_select_uses_skip_locked() {
    let pool = setup().await;
    let mut keys: Vec<Uuid> = Vec::new();
    for _ in 0..4 {
        let e = sample_outbox(Uuid::now_v7());
        insert_outbox(&pool, &e).await.unwrap();
        keys.push(e.id);
    }

    let mut tx_a = pool.begin().await.unwrap();
    let rows_a = select_pending_outbox(&mut tx_a, 4).await.unwrap();
    // Sibling tx claims any rows tx_a hasn't locked.
    let mut tx_b = pool.begin().await.unwrap();
    let rows_b = select_pending_outbox(&mut tx_b, 4).await.unwrap();

    tx_a.commit().await.unwrap();
    tx_b.commit().await.unwrap();

    assert_eq!(rows_a.len(), 4);
    assert_eq!(rows_b.len(), 0, "second tx must observe FOR UPDATE SKIP LOCKED");
}

// ── Inbox ────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn inbox_insert_reports_inserted_then_duplicate_for_same_key() {
    let pool = setup().await;
    let key = Uuid::now_v7();

    let first = insert_inbox(&pool, &sample_inbox(key)).await.unwrap();
    assert_eq!(first, InsertOutcome::Inserted);

    let second = insert_inbox(&pool, &sample_inbox(key)).await.unwrap();
    assert_eq!(second, InsertOutcome::Duplicate);
}

#[tokio::test]
#[ignore]
async fn inbox_lookup_returns_some_for_existing_none_for_missing() {
    let pool = setup().await;
    let key = Uuid::now_v7();
    let entry = sample_inbox(key);
    insert_inbox(&pool, &entry).await.unwrap();

    let hit = lookup_inbox(&pool, key).await.unwrap();
    assert!(hit.is_some());
    let hit = hit.unwrap();
    assert_eq!(hit.source_domain, "bpmn-lite");
    assert_eq!(hit.endpoint, BusEndpoint::Invocation);
    assert_eq!(hit.status, InboxStatus::Received);
    assert_eq!(hit.execution_id, entry.execution_id);

    let miss = lookup_inbox(&pool, Uuid::now_v7()).await.unwrap();
    assert!(miss.is_none());
}

#[tokio::test]
#[ignore]
async fn inbox_mark_processed_sets_processed_at_and_status() {
    let pool = setup().await;
    let key = Uuid::now_v7();
    insert_inbox(&pool, &sample_inbox(key)).await.unwrap();

    mark_inbox_processed(&pool, key).await.unwrap();

    let entry = lookup_inbox(&pool, key).await.unwrap().unwrap();
    assert_eq!(entry.status, InboxStatus::Processed);
    assert!(entry.processed_at.is_some());
}

// ── helpers ──────────────────────────────────────────────────────────

async fn fetch_row_by_id(pool: &PgPool, id: Uuid) -> OutboxEntry {
    let mut conn = pool.acquire().await.unwrap();
    // Use the public select to read back the row — limit on rows pending
    // would skip submitted rows, so do a direct fetch via a transient
    // helper that re-reads everything regardless of status.
    sqlx::query(
        r#"
        SELECT id, target_domain, target_endpoint, payload, idempotency_key,
               execution_id, callout_id, status, attempt_count, next_attempt_at,
               last_error, created_at, submitted_at
          FROM outbox
         WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await
    .map(|row| {
        use sqlx::Row as _;
        let endpoint: String = row.get("target_endpoint");
        let status: String = row.get("status");
        OutboxEntry {
            id: row.get("id"),
            target_domain: row.get("target_domain"),
            target_endpoint: BusEndpoint::parse_for_test(&endpoint),
            payload: row.get("payload"),
            idempotency_key: row.get("idempotency_key"),
            execution_id: row.get("execution_id"),
            callout_id: row.get("callout_id"),
            status: OutboxStatus::parse_for_test(&status),
            attempt_count: row.get("attempt_count"),
            next_attempt_at: row.get("next_attempt_at"),
            last_error: row.get("last_error"),
            created_at: row.get("created_at"),
            submitted_at: row.get("submitted_at"),
        }
    })
    .unwrap()
}

// Test-only parse helpers that panic on invalid values. The real
// parsers return typed errors, but the tests assert against schema
// constraints, so a malformed value is a test infrastructure bug.
impl BusEndpoint {
    fn parse_for_test(s: &str) -> Self {
        Self::parse(s).expect("malformed target_endpoint in test fixture")
    }
}
impl OutboxStatus {
    fn parse_for_test(s: &str) -> Self {
        Self::parse(s).expect("malformed status in test fixture")
    }
}
