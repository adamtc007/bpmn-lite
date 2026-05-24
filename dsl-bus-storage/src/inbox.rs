//! Inbox CRUD (v0.6 §8.2 / §8.6).

use sqlx::PgExecutor;
use uuid::Uuid;

use crate::types::{BusEndpoint, InboxEntry, InboxStatus, InsertOutcome, Result};

/// Insert a freshly-received [`InboxEntry`]. Returns
/// [`InsertOutcome::Duplicate`] if the `idempotency_key` is already
/// present — the receiver handler then replays the previously-stored
/// outcome instead of re-running the verb.
pub async fn insert_inbox(
    executor: impl PgExecutor<'_>,
    entry: &InboxEntry,
) -> Result<InsertOutcome> {
    let res = sqlx::query(
        r#"
        INSERT INTO dsl_bus.inbox (
            idempotency_key, source_domain, endpoint, execution_id,
            received_at, processed_at, status, payload
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (idempotency_key) DO NOTHING
        "#,
    )
    .bind(entry.idempotency_key)
    .bind(&entry.source_domain)
    .bind(entry.endpoint.as_str())
    .bind(entry.execution_id)
    .bind(entry.received_at)
    .bind(entry.processed_at)
    .bind(entry.status.as_str())
    .bind(&entry.payload)
    .execute(executor)
    .await?;

    Ok(if res.rows_affected() == 1 {
        InsertOutcome::Inserted
    } else {
        InsertOutcome::Duplicate
    })
}

/// Look up an inbox row by idempotency key.
pub async fn lookup_inbox(
    executor: impl PgExecutor<'_>,
    idempotency_key: Uuid,
) -> Result<Option<InboxEntry>> {
    let row = sqlx::query(
        r#"
        SELECT idempotency_key, source_domain, endpoint, execution_id,
               received_at, processed_at, status, payload
          FROM dsl_bus.inbox
         WHERE idempotency_key = $1
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(executor)
    .await?;

    row.map(row_to_entry).transpose()
}

/// Transition `status → processed` and stamp `processed_at = now()`.
pub async fn mark_inbox_processed(
    executor: impl PgExecutor<'_>,
    idempotency_key: Uuid,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE dsl_bus.inbox
           SET status = 'processed',
               processed_at = now()
         WHERE idempotency_key = $1
        "#,
    )
    .bind(idempotency_key)
    .execute(executor)
    .await?;
    Ok(())
}

fn row_to_entry(row: sqlx::postgres::PgRow) -> Result<InboxEntry> {
    use sqlx::Row as _;
    let endpoint: String = row.try_get("endpoint")?;
    let status: String = row.try_get("status")?;
    Ok(InboxEntry {
        idempotency_key: row.try_get("idempotency_key")?,
        source_domain: row.try_get("source_domain")?,
        endpoint: BusEndpoint::parse(&endpoint)?,
        execution_id: row.try_get("execution_id")?,
        received_at: row.try_get("received_at")?,
        processed_at: row.try_get("processed_at")?,
        status: InboxStatus::parse(&status)?,
        payload: row.try_get("payload")?,
    })
}
