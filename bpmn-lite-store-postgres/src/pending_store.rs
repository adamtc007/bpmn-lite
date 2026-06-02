//! PostgreSQL implementation of `PendingInvocationStore` (v0.6 §8.3).
//!
//! SQL strings live here as private items; the public surface is the
//! `PostgresPendingInvocationStore` struct + its trait impl.

use async_trait::async_trait;
use bpmn_lite_store::pending::{InsertOutcome, PendingInvocation, PendingInvocationStore};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub struct PostgresPendingInvocationStore {
    pool: PgPool,
}

impl PostgresPendingInvocationStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn list_tenants(&self) -> anyhow::Result<Vec<String>> {
        let rows_res = sqlx::query("SELECT tenant_id FROM tenants ORDER BY first_seen_at")
            .fetch_all(&self.pool)
            .await;
        match rows_res {
            Ok(rows) => {
                let tenants: Vec<String> = rows.iter().map(|r| r.get::<String, _>("tenant_id")).collect();
                if tenants.is_empty() {
                    Ok(vec!["default".to_string()])
                } else {
                    Ok(tenants)
                }
            }
            Err(_) => Ok(vec!["default".to_string()]),
        }
    }

    async fn resolve_instance_tenant(&self, instance_id: Uuid) -> anyhow::Result<String> {
        let row: (Option<String>,) = sqlx::query_as("SELECT resolve_instance_tenant($1)")
            .bind(instance_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0.unwrap_or_else(|| "default".to_string()))
    }

    async fn resolve_callout_tenant(&self, callout_id: Uuid) -> anyhow::Result<(String, Uuid)> {
        let tenants = self.list_tenants().await?;
        for tenant_id in tenants {
            let mut tx = self.pool.begin().await?;
            if let Err(_) = sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
                .bind(&tenant_id)
                .execute(&mut *tx)
                .await
            {
                continue;
            }
            let res: Option<(Uuid,)> = sqlx::query_as("SELECT process_instance_id FROM bpmn_pending_invocation WHERE callout_id = $1")
                .bind(callout_id)
                .fetch_optional(&mut *tx)
                .await
                .ok()
                .flatten();
            let _ = tx.commit().await;
            if let Some((pi_id,)) = res {
                return Ok((tenant_id, pi_id));
            }
        }
        anyhow::bail!("no pending row for callout_id {callout_id}")
    }
}

#[async_trait]
impl PendingInvocationStore for PostgresPendingInvocationStore {
    async fn insert(&self, record: PendingInvocation) -> anyhow::Result<InsertOutcome> {
        let tenant_id = self.resolve_instance_tenant(record.process_instance_id).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
            .bind(&tenant_id)
            .execute(&mut *tx)
            .await?;

        // `(callout_id)` is PK and `(idempotency_key)` is UNIQUE.
        // ON CONFLICT on either deduplicates, so a re-submit of the
        // same logical row is a no-op rather than an error.
        let res = sqlx::query(
            r#"
            INSERT INTO bpmn_pending_invocation (
                callout_id, process_instance_id, node_id, target_domain, verb_id,
                idempotency_key, execution_id, submitted_at, ack_received_at, timeout_at, tenant_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(record.callout_id)
        .bind(record.process_instance_id)
        .bind(&record.node_id)
        .bind(&record.target_domain)
        .bind(&record.verb_id)
        .bind(record.idempotency_key)
        .bind(record.execution_id)
        .bind(record.submitted_at)
        .bind(record.ack_received_at)
        .bind(record.timeout_at)
        .bind(&tenant_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(if res.rows_affected() == 1 {
            InsertOutcome::Inserted
        } else {
            InsertOutcome::Duplicate
        })
    }

    async fn record_ack(
        &self,
        callout_id: Uuid,
        execution_id: Uuid,
        ack_received_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let (tenant_id, _) = self.resolve_callout_tenant(callout_id).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
            .bind(&tenant_id)
            .execute(&mut *tx)
            .await?;

        let res = sqlx::query(
            r#"
            UPDATE bpmn_pending_invocation
               SET execution_id = $2,
                   ack_received_at = $3
             WHERE callout_id = $1
            "#,
        )
        .bind(callout_id)
        .bind(execution_id)
        .bind(ack_received_at)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        if res.rows_affected() != 1 {
            anyhow::bail!("no pending row for callout_id {callout_id}");
        }
        Ok(())
    }

    async fn take_by_execution_id(
        &self,
        execution_id: Uuid,
    ) -> anyhow::Result<Option<PendingInvocation>> {
        let tenants = self.list_tenants().await?;
        for tenant_id in tenants {
            let mut tx = self.pool.begin().await?;
            if let Err(_) = sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
                .bind(&tenant_id)
                .execute(&mut *tx)
                .await
            {
                continue;
            }
            let row = sqlx::query(
                r#"
                DELETE FROM bpmn_pending_invocation
                 WHERE execution_id = $1
                 RETURNING callout_id, process_instance_id, node_id, target_domain, verb_id,
                           idempotency_key, execution_id, submitted_at, ack_received_at, timeout_at
                "#,
            )
            .bind(execution_id)
            .fetch_optional(&mut *tx)
            .await?;
            tx.commit().await?;
            if let Some(r) = row {
                return Ok(Some(row_to_record(r)?));
            }
        }
        Ok(None)
    }

    async fn lookup_by_execution_id(
        &self,
        execution_id: Uuid,
    ) -> anyhow::Result<Option<PendingInvocation>> {
        let tenants = self.list_tenants().await?;
        for tenant_id in tenants {
            let mut tx = self.pool.begin().await?;
            if let Err(_) = sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
                .bind(&tenant_id)
                .execute(&mut *tx)
                .await
            {
                continue;
            }
            let row = sqlx::query(
                r#"
                SELECT callout_id, process_instance_id, node_id, target_domain, verb_id,
                       idempotency_key, execution_id, submitted_at, ack_received_at, timeout_at
                  FROM bpmn_pending_invocation
                 WHERE execution_id = $1
                "#,
            )
            .bind(execution_id)
            .fetch_optional(&mut *tx)
            .await?;
            tx.commit().await?;
            if let Some(r) = row {
                return Ok(Some(row_to_record(r)?));
            }
        }
        Ok(None)
    }

    async fn lookup_by_callout_id(
        &self,
        callout_id: Uuid,
    ) -> anyhow::Result<Option<PendingInvocation>> {
        let tenants = self.list_tenants().await?;
        for tenant_id in tenants {
            let mut tx = self.pool.begin().await?;
            if let Err(_) = sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
                .bind(&tenant_id)
                .execute(&mut *tx)
                .await
            {
                continue;
            }
            let row = sqlx::query(
                r#"
                SELECT callout_id, process_instance_id, node_id, target_domain, verb_id,
                       idempotency_key, execution_id, submitted_at, ack_received_at, timeout_at
                  FROM bpmn_pending_invocation
                 WHERE callout_id = $1
                "#,
            )
            .bind(callout_id)
            .fetch_optional(&mut *tx)
            .await?;
            tx.commit().await?;
            if let Some(r) = row {
                return Ok(Some(row_to_record(r)?));
            }
        }
        Ok(None)
    }

    async fn list_for_process(
        &self,
        process_instance_id: Uuid,
    ) -> anyhow::Result<Vec<PendingInvocation>> {
        let tenant_id = self.resolve_instance_tenant(process_instance_id).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
            .bind(&tenant_id)
            .execute(&mut *tx)
            .await?;

        let rows = sqlx::query(
            r#"
            SELECT callout_id, process_instance_id, node_id, target_domain, verb_id,
                   idempotency_key, execution_id, submitted_at, ack_received_at, timeout_at
              FROM bpmn_pending_invocation
             WHERE process_instance_id = $1
             ORDER BY submitted_at
            "#,
        )
        .bind(process_instance_id)
        .fetch_all(&mut *tx)
        .await?;

        tx.commit().await?;
        rows.into_iter().map(row_to_record).collect()
    }
}

fn row_to_record(row: sqlx::postgres::PgRow) -> anyhow::Result<PendingInvocation> {
    Ok(PendingInvocation {
        callout_id: row.try_get("callout_id")?,
        process_instance_id: row.try_get("process_instance_id")?,
        node_id: row.try_get("node_id")?,
        target_domain: row.try_get("target_domain")?,
        verb_id: row.try_get("verb_id")?,
        idempotency_key: row.try_get("idempotency_key")?,
        execution_id: row.try_get("execution_id")?,
        submitted_at: row.try_get("submitted_at")?,
        ack_received_at: row.try_get("ack_received_at")?,
        timeout_at: row.try_get("timeout_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_TEST_DATABASE_URL: &str = "postgresql://localhost/bpmn_lite_test";

    pub(crate) async fn setup_t2b8_pool() -> (PgPool, tokio::sync::MutexGuard<'static, ()>) {
        let guard = crate::test_lock::get_mutex().lock().await;
        let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_owned());
        let pool = PgPool::connect(&url).await.expect("connect");

        // Run migrations
        let migrator = sqlx::migrate!("./migrations");
        migrator.run(&pool)
            .await
            .expect("run migrations");

        // Perform TRUNCATE as admin before returning app connection
        sqlx::query("TRUNCATE bpmn_pending_invocation, bpmn_process_instance CASCADE")
            .execute(&pool)
            .await
            .unwrap();

        let app_url = if url.contains("@") {
            let parts: Vec<&str> = url.split('@').collect();
            let host_part = parts[1];
            format!("postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@{}", host_part)
        } else {
            "postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@localhost/bpmn_lite_test".to_string()
        };
        let app_pool = PgPool::connect(&app_url).await.expect("Failed to connect as bpmn_lite_app");
        (app_pool, guard)
    }

    async fn setup() -> (PostgresPendingInvocationStore, tokio::sync::MutexGuard<'static, ()>) {
        let (pool, guard) = setup_t2b8_pool().await;
        (PostgresPendingInvocationStore::new(pool), guard)
    }

    fn record(callout: Uuid, process: Uuid, idem: Uuid) -> PendingInvocation {
        PendingInvocation::new(callout, process, "create-cbu", "ob-poc", "cbu.create", idem)
    }

    #[tokio::test]
    async fn insert_then_lookup_round_trips() {
        let (store, _lock) = setup().await;
        let cid = Uuid::now_v7();
        let pid = Uuid::now_v7();
        let idem = Uuid::now_v7();
        assert_eq!(
            store.insert(record(cid, pid, idem)).await.unwrap(),
            InsertOutcome::Inserted
        );
        let hit = store.lookup_by_callout_id(cid).await.unwrap().unwrap();
        assert_eq!(hit.process_instance_id, pid);
        assert_eq!(hit.idempotency_key, idem);
        assert!(hit.execution_id.is_none());
        assert!(hit.ack_received_at.is_none());
    }

    #[tokio::test]
    async fn duplicate_callout_id_is_a_no_op_insert() {
        let (store, _lock) = setup().await;
        let cid = Uuid::now_v7();
        store
            .insert(record(cid, Uuid::now_v7(), Uuid::now_v7()))
            .await
            .unwrap();
        let second = store
            .insert(record(cid, Uuid::now_v7(), Uuid::now_v7()))
            .await
            .unwrap();
        assert_eq!(second, InsertOutcome::Duplicate);
    }

    #[tokio::test]
    async fn duplicate_idempotency_key_violates_unique_constraint() {
        let (store, _lock) = setup().await;
        let idem = Uuid::now_v7();
        store
            .insert(record(Uuid::now_v7(), Uuid::now_v7(), idem))
            .await
            .unwrap();
        let second = store
            .insert(record(Uuid::now_v7(), Uuid::now_v7(), idem))
            .await
            .unwrap();
        assert_eq!(second, InsertOutcome::Duplicate);
    }

    #[tokio::test]
    async fn record_ack_then_take_completes_the_lifecycle() {
        let (store, _lock) = setup().await;
        let cid = Uuid::now_v7();
        let pid = Uuid::now_v7();
        store
            .insert(record(cid, pid, Uuid::now_v7()))
            .await
            .unwrap();

        let exec = Uuid::now_v7();
        let now = Utc::now();
        store.record_ack(cid, exec, now).await.unwrap();

        // Stage 3: delete + return.
        let taken = store.take_by_execution_id(exec).await.unwrap();
        assert!(taken.is_some());
        assert_eq!(taken.as_ref().unwrap().callout_id, cid);

        // Duplicate take is a clean None.
        assert!(store.take_by_execution_id(exec).await.unwrap().is_none());
        // And lookup_by_callout_id confirms the row is gone.
        assert!(store.lookup_by_callout_id(cid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn record_ack_fails_on_unknown_callout_id() {
        let (store, _lock) = setup().await;
        let err = store
            .record_ack(Uuid::now_v7(), Uuid::now_v7(), Utc::now())
            .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn list_for_process_returns_only_matching_rows_in_submission_order() {
        let (store, _lock) = setup().await;
        let pid_a = Uuid::now_v7();
        let pid_b = Uuid::now_v7();
        for _ in 0..3 {
            store
                .insert(record(Uuid::now_v7(), pid_a, Uuid::now_v7()))
                .await
                .unwrap();
        }
        store
            .insert(record(Uuid::now_v7(), pid_b, Uuid::now_v7()))
            .await
            .unwrap();

        let a_rows = store.list_for_process(pid_a).await.unwrap();
        assert_eq!(a_rows.len(), 3);
        assert!(a_rows.iter().all(|r| r.process_instance_id == pid_a));
        assert_eq!(store.list_for_process(pid_b).await.unwrap().len(), 1);
    }
}
