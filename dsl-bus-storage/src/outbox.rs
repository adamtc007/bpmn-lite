//! Outbox CRUD (v0.6 §8.1 / §8.5).
//!
//! All SQL lives here; nothing leaks to consumers.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

use crate::types::{BusEndpoint, InsertOutcome, OutboxEntry, OutboxStatus, Result};

/// Insert a freshly-built [`OutboxEntry`]. Idempotent on
/// `(idempotency_key, target_endpoint)` per §8.1 — re-inserts of the
/// same logical row become [`InsertOutcome::Duplicate`].
pub async fn insert_outbox(
    executor: impl PgExecutor<'_>,
    entry: &OutboxEntry,
) -> Result<InsertOutcome> {
    let res = sqlx::query(
        r#"
        INSERT INTO dsl_bus.outbox (
            id, target_domain, target_endpoint, payload, idempotency_key,
            execution_id, callout_id, status, attempt_count, next_attempt_at,
            last_error, created_at, submitted_at, tenant_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        ON CONFLICT (idempotency_key, target_endpoint) DO NOTHING
        "#,
    )
    .bind(entry.id)
    .bind(&entry.target_domain)
    .bind(entry.target_endpoint.as_str())
    .bind(&entry.payload)
    .bind(entry.idempotency_key)
    .bind(entry.execution_id)
    .bind(entry.callout_id)
    .bind(entry.status.as_str())
    .bind(entry.attempt_count)
    .bind(entry.next_attempt_at)
    .bind(&entry.last_error)
    .bind(entry.created_at)
    .bind(entry.submitted_at)
    .bind(&entry.tenant_id)
    .execute(executor)
    .await?;

    Ok(if res.rows_affected() == 1 {
        InsertOutcome::Inserted
    } else {
        InsertOutcome::Duplicate
    })
}

/// Claim up to `limit` due rows in a short transaction.
///
/// **Must run inside a transaction.** The query uses `FOR UPDATE SKIP
/// LOCKED` so concurrent senders see disjoint claim sets; the lock is
/// only meaningful for the lifetime of the surrounding transaction.
///
/// Callers should commit (after marking each row submitted or retrying)
/// to release the locks. Aborting the transaction returns the rows to
/// the queue.
pub async fn claim_pending_outbox(
    conn: &mut sqlx::PgConnection,
    limit: i64,
    owner: &str,
    claim_token: Uuid,
    claim_until: DateTime<Utc>,
) -> Result<Vec<OutboxEntry>> {
    let rows = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT id
            FROM dsl_bus.outbox
            WHERE status IN ('pending', 'dispatching')
              AND next_attempt_at <= now()
              AND (claim_until IS NULL OR claim_until <= now())
            ORDER BY next_attempt_at, id
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE dsl_bus.outbox AS outbox
        SET status = 'dispatching', claim_owner = $2, claim_token = $3,
            claim_until = $4
        FROM candidates
        WHERE outbox.id = candidates.id
        RETURNING outbox.id, outbox.target_domain, outbox.target_endpoint,
                  outbox.payload, outbox.idempotency_key, outbox.execution_id,
                  outbox.callout_id, outbox.status, outbox.attempt_count,
                  outbox.next_attempt_at, outbox.last_error, outbox.created_at,
                  outbox.submitted_at, outbox.tenant_id, outbox.claim_token
        "#,
    )
    .bind(limit)
    .bind(owner)
    .bind(claim_token)
    .bind(claim_until)
    .fetch_all(&mut *conn)
    .await?;

    rows.into_iter().map(row_to_entry).collect()
}

/// Transition a row to `submitted` and record the receiver-assigned
/// `execution_id`.
pub async fn mark_outbox_submitted(
    executor: impl PgExecutor<'_>,
    id: Uuid,
    claim_token: Uuid,
    execution_id: Uuid,
) -> Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE dsl_bus.outbox
           SET status = 'submitted',
               execution_id = $2,
               submitted_at = now(), claim_owner = NULL, claim_token = NULL,
               claim_until = NULL
         WHERE id = $1 AND claim_token = $3 AND status = 'dispatching'
        "#,
    )
    .bind(id)
    .bind(execution_id)
    .bind(claim_token)
    .execute(executor)
    .await?;
    if result.rows_affected() != 1 {
        return Err(crate::BusStorageError::LostClaim(id));
    }
    Ok(())
}

/// Schedule a retry with exponential backoff. `backoff_secs` controls
/// when `next_attempt_at` should fire; status returns to `pending` so
/// the row appears on the next sender sweep.
pub async fn mark_outbox_retry(
    executor: impl PgExecutor<'_>,
    id: Uuid,
    claim_token: Uuid,
    backoff_secs: i64,
    error: &str,
) -> Result<()> {
    let next_attempt = Utc::now() + Duration::seconds(backoff_secs);
    let result = sqlx::query(
        r#"
        UPDATE dsl_bus.outbox
           SET status = 'pending',
               attempt_count = attempt_count + 1,
               next_attempt_at = $2,
               last_error = $3,
               claim_owner = NULL, claim_token = NULL, claim_until = NULL
         WHERE id = $1 AND claim_token = $4 AND status = 'dispatching'
        "#,
    )
    .bind(id)
    .bind(next_attempt)
    .bind(error)
    .bind(claim_token)
    .execute(executor)
    .await?;
    if result.rows_affected() != 1 {
        return Err(crate::BusStorageError::LostClaim(id));
    }
    Ok(())
}

fn row_to_entry(row: sqlx::postgres::PgRow) -> Result<OutboxEntry> {
    use sqlx::Row as _;
    let endpoint: String = row.try_get("target_endpoint")?;
    let status: String = row.try_get("status")?;
    Ok(OutboxEntry {
        id: row.try_get("id")?,
        target_domain: row.try_get("target_domain")?,
        target_endpoint: BusEndpoint::parse(&endpoint)?,
        payload: row.try_get("payload")?,
        idempotency_key: row.try_get("idempotency_key")?,
        execution_id: row.try_get("execution_id")?,
        callout_id: row.try_get("callout_id")?,
        status: OutboxStatus::parse(&status)?,
        attempt_count: row.try_get("attempt_count")?,
        next_attempt_at: row.try_get("next_attempt_at")?,
        last_error: row.try_get("last_error")?,
        created_at: row.try_get("created_at")?,
        submitted_at: row.try_get("submitted_at")?,
        tenant_id: row.try_get("tenant_id")?,
        claim_token: row.try_get("claim_token")?,
    })
}
