//! PostgreSQL implementation of `PendingInvocationStore` (v0.6 §8.3).
//!
//! SQL strings live here as private items; the public surface is the
//! `PostgresPendingInvocationStore` struct + its trait impl.

use async_trait::async_trait;
use bpmn_lite_store::pending::{InsertOutcome, PendingInvocation, PendingInvocationStore};
use bpmn_lite_store::{StoreError, StoreResult};
use bpmn_lite_types::TenantId;
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
}

#[async_trait]
impl PendingInvocationStore for PostgresPendingInvocationStore {
    async fn insert(&self, record: PendingInvocation) -> StoreResult<InsertOutcome> {
        let mut tx = self.pool.begin().await.map_err(StoreError::unavailable)?;
        sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
            .bind(record.tenant_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(StoreError::unavailable)?;

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
        .bind(record.tenant_id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(StoreError::unavailable)?;

        tx.commit().await.map_err(StoreError::unavailable)?;

        Ok(if res.rows_affected() == 1 {
            InsertOutcome::Inserted
        } else {
            InsertOutcome::Duplicate
        })
    }

    async fn record_ack(
        &self,
        tenant_id: &TenantId,
        callout_id: Uuid,
        execution_id: Uuid,
        ack_received_at: DateTime<Utc>,
    ) -> StoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(StoreError::unavailable)?;
        sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
            .bind(tenant_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(StoreError::unavailable)?;

        let res = sqlx::query(
            r#"
            UPDATE bpmn_pending_invocation
               SET execution_id = $2,
                   ack_received_at = $3
             WHERE tenant_id = $4 AND callout_id = $1
            "#,
        )
        .bind(callout_id)
        .bind(execution_id)
        .bind(ack_received_at)
        .bind(tenant_id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(StoreError::unavailable)?;

        tx.commit().await.map_err(StoreError::unavailable)?;

        if res.rows_affected() != 1 {
            return Err(StoreError::NotFound(format!(
                "pending row for callout_id {callout_id}"
            )));
        }
        Ok(())
    }

    async fn take_by_execution_id(
        &self,
        tenant_id: &TenantId,
        execution_id: Uuid,
    ) -> StoreResult<Option<PendingInvocation>> {
        let mut tx = self.pool.begin().await.map_err(StoreError::unavailable)?;
        set_tenant(&mut tx, tenant_id)
            .await
            .map_err(StoreError::unavailable)?;
        let row = sqlx::query(
            r#"
            DELETE FROM bpmn_pending_invocation
             WHERE tenant_id = $1 AND execution_id = $2
             RETURNING callout_id, process_instance_id, node_id, target_domain, verb_id,
                       idempotency_key, execution_id, submitted_at, ack_received_at, timeout_at,
                       tenant_id
            "#,
        )
        .bind(tenant_id.as_str())
        .bind(execution_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(StoreError::unavailable)?;
        tx.commit().await.map_err(StoreError::unavailable)?;
        row.map(row_to_record).transpose()
    }

    async fn lookup_by_execution_id(
        &self,
        tenant_id: &TenantId,
        execution_id: Uuid,
    ) -> StoreResult<Option<PendingInvocation>> {
        let mut tx = self.pool.begin().await.map_err(StoreError::unavailable)?;
        set_tenant(&mut tx, tenant_id)
            .await
            .map_err(StoreError::unavailable)?;
        let row = sqlx::query(
            r#"
            SELECT callout_id, process_instance_id, node_id, target_domain, verb_id,
                   idempotency_key, execution_id, submitted_at, ack_received_at, timeout_at,
                   tenant_id
              FROM bpmn_pending_invocation
             WHERE tenant_id = $1 AND execution_id = $2
            "#,
        )
        .bind(tenant_id.as_str())
        .bind(execution_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(StoreError::unavailable)?;
        tx.commit().await.map_err(StoreError::unavailable)?;
        row.map(row_to_record).transpose()
    }

    async fn lookup_by_callout_id(
        &self,
        tenant_id: &TenantId,
        callout_id: Uuid,
    ) -> StoreResult<Option<PendingInvocation>> {
        let mut tx = self.pool.begin().await.map_err(StoreError::unavailable)?;
        set_tenant(&mut tx, tenant_id)
            .await
            .map_err(StoreError::unavailable)?;
        let row = sqlx::query(
            r#"
            SELECT callout_id, process_instance_id, node_id, target_domain, verb_id,
                   idempotency_key, execution_id, submitted_at, ack_received_at, timeout_at,
                   tenant_id
              FROM bpmn_pending_invocation
             WHERE tenant_id = $1 AND callout_id = $2
            "#,
        )
        .bind(tenant_id.as_str())
        .bind(callout_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(StoreError::unavailable)?;
        tx.commit().await.map_err(StoreError::unavailable)?;
        row.map(row_to_record).transpose()
    }

    async fn list_for_process(
        &self,
        tenant_id: &TenantId,
        process_instance_id: Uuid,
    ) -> StoreResult<Vec<PendingInvocation>> {
        let mut tx = self.pool.begin().await.map_err(StoreError::unavailable)?;
        set_tenant(&mut tx, tenant_id)
            .await
            .map_err(StoreError::unavailable)?;

        let rows = sqlx::query(
            r#"
            SELECT callout_id, process_instance_id, node_id, target_domain, verb_id,
                   idempotency_key, execution_id, submitted_at, ack_received_at, timeout_at,
                   tenant_id
              FROM bpmn_pending_invocation
             WHERE tenant_id = $1 AND process_instance_id = $2
             ORDER BY submitted_at
            "#,
        )
        .bind(tenant_id.as_str())
        .bind(process_instance_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(StoreError::unavailable)?;

        tx.commit().await.map_err(StoreError::unavailable)?;
        rows.into_iter().map(row_to_record).collect()
    }
}

async fn set_tenant(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &TenantId,
) -> StoreResult<()> {
    sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
        .bind(tenant_id.as_str())
        .execute(&mut **tx)
        .await
        .map_err(StoreError::unavailable)?;
    Ok(())
}

fn row_to_record(row: sqlx::postgres::PgRow) -> StoreResult<PendingInvocation> {
    Ok(PendingInvocation {
        tenant_id: TenantId::new(
            row.try_get::<String, _>("tenant_id")
                .map_err(StoreError::unavailable)?,
        )
        .map_err(StoreError::invalid)?,
        callout_id: row.try_get("callout_id").map_err(StoreError::unavailable)?,
        process_instance_id: row
            .try_get("process_instance_id")
            .map_err(StoreError::unavailable)?,
        node_id: row.try_get("node_id").map_err(StoreError::unavailable)?,
        target_domain: row
            .try_get("target_domain")
            .map_err(StoreError::unavailable)?,
        verb_id: row.try_get("verb_id").map_err(StoreError::unavailable)?,
        idempotency_key: row
            .try_get("idempotency_key")
            .map_err(StoreError::unavailable)?,
        execution_id: row
            .try_get("execution_id")
            .map_err(StoreError::unavailable)?,
        submitted_at: row
            .try_get("submitted_at")
            .map_err(StoreError::unavailable)?,
        ack_received_at: row
            .try_get("ack_received_at")
            .map_err(StoreError::unavailable)?,
        timeout_at: row.try_get("timeout_at").map_err(StoreError::unavailable)?,
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
        migrator.run(&pool).await.expect("run migrations");

        // Perform TRUNCATE as admin before returning app connection
        sqlx::query("TRUNCATE bpmn_pending_invocation, workflow_instances CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        let app_url = if url.contains("@") {
            let parts: Vec<&str> = url.split('@').collect();
            let host_part = parts[1];
            format!(
                "postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@{}",
                host_part
            )
        } else {
            "postgresql://bpmn_lite_app:bpmn_lite_app_dev_password@localhost/bpmn_lite_test"
                .to_string()
        };
        let app_pool = PgPool::connect(&app_url)
            .await
            .expect("Failed to connect as bpmn_lite_app");
        (app_pool, guard)
    }

    async fn setup() -> (
        PostgresPendingInvocationStore,
        tokio::sync::MutexGuard<'static, ()>,
    ) {
        let (pool, guard) = setup_t2b8_pool().await;
        (PostgresPendingInvocationStore::new(pool), guard)
    }

    fn record(callout: Uuid, process: Uuid, idem: Uuid) -> PendingInvocation {
        PendingInvocation::new(
            TenantId::new("default").unwrap(),
            callout,
            process,
            "create-cbu",
            "ob-poc",
            "cbu.create",
            idem,
        )
    }

    async fn insert_instance(store: &PostgresPendingInvocationStore, instance_id: Uuid) {
        let mut tx = store.pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_tenant', 'default', true)")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            r#"
            INSERT INTO workflow_instances
                (instance_id, tenant_id, process_key, bytecode_version,
                 domain_payload, domain_payload_hash, state, correlation_id)
            VALUES ($1, 'default', 'pending-test', $2, '{}', $3, '"Running"'::jsonb, $4)
            "#,
        )
        .bind(instance_id)
        .bind(vec![0u8; 32])
        .bind(blake3::hash(b"{}").as_bytes().as_slice())
        .bind(instance_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn insert_then_lookup_round_trips() {
        let (store, _lock) = setup().await;
        let cid = Uuid::now_v7();
        let pid = Uuid::now_v7();
        let idem = Uuid::now_v7();
        insert_instance(&store, pid).await;
        assert_eq!(
            store.insert(record(cid, pid, idem)).await.unwrap(),
            InsertOutcome::Inserted
        );
        let hit = store
            .lookup_by_callout_id(&TenantId::new("default").unwrap(), cid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hit.process_instance_id, pid);
        assert_eq!(hit.idempotency_key, idem);
        assert!(hit.execution_id.is_none());
        assert!(hit.ack_received_at.is_none());
    }

    #[tokio::test]
    async fn duplicate_callout_id_is_a_no_op_insert() {
        let (store, _lock) = setup().await;
        let cid = Uuid::now_v7();
        let first_pid = Uuid::now_v7();
        let second_pid = Uuid::now_v7();
        insert_instance(&store, first_pid).await;
        insert_instance(&store, second_pid).await;
        store
            .insert(record(cid, first_pid, Uuid::now_v7()))
            .await
            .unwrap();
        let second = store
            .insert(record(cid, second_pid, Uuid::now_v7()))
            .await
            .unwrap();
        assert_eq!(second, InsertOutcome::Duplicate);
    }

    #[tokio::test]
    async fn duplicate_idempotency_key_violates_unique_constraint() {
        let (store, _lock) = setup().await;
        let idem = Uuid::now_v7();
        let first_pid = Uuid::now_v7();
        let second_pid = Uuid::now_v7();
        insert_instance(&store, first_pid).await;
        insert_instance(&store, second_pid).await;
        store
            .insert(record(Uuid::now_v7(), first_pid, idem))
            .await
            .unwrap();
        let second = store
            .insert(record(Uuid::now_v7(), second_pid, idem))
            .await
            .unwrap();
        assert_eq!(second, InsertOutcome::Duplicate);
    }

    #[tokio::test]
    async fn record_ack_then_take_completes_the_lifecycle() {
        let (store, _lock) = setup().await;
        let cid = Uuid::now_v7();
        let pid = Uuid::now_v7();
        insert_instance(&store, pid).await;
        store
            .insert(record(cid, pid, Uuid::now_v7()))
            .await
            .unwrap();

        let exec = Uuid::now_v7();
        let now = Utc::now();
        store
            .record_ack(&TenantId::new("default").unwrap(), cid, exec, now)
            .await
            .unwrap();

        // Stage 3: delete + return.
        let taken = store
            .take_by_execution_id(&TenantId::new("default").unwrap(), exec)
            .await
            .unwrap();
        assert!(taken.is_some());
        assert_eq!(taken.as_ref().unwrap().callout_id, cid);

        // Duplicate take is a clean None.
        assert!(store
            .take_by_execution_id(&TenantId::new("default").unwrap(), exec)
            .await
            .unwrap()
            .is_none());
        // And lookup_by_callout_id confirms the row is gone.
        assert!(store
            .lookup_by_callout_id(&TenantId::new("default").unwrap(), cid)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn record_ack_fails_on_unknown_callout_id() {
        let (store, _lock) = setup().await;
        let err = store
            .record_ack(
                &TenantId::new("default").unwrap(),
                Uuid::now_v7(),
                Uuid::now_v7(),
                Utc::now(),
            )
            .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn list_for_process_returns_only_matching_rows_in_submission_order() {
        let (store, _lock) = setup().await;
        let pid_a = Uuid::now_v7();
        let pid_b = Uuid::now_v7();
        insert_instance(&store, pid_a).await;
        insert_instance(&store, pid_b).await;
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

        let a_rows = store
            .list_for_process(&TenantId::new("default").unwrap(), pid_a)
            .await
            .unwrap();
        assert_eq!(a_rows.len(), 3);
        assert!(a_rows.iter().all(|r| r.process_instance_id == pid_a));
        assert_eq!(
            store
                .list_for_process(&TenantId::new("default").unwrap(), pid_b)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
