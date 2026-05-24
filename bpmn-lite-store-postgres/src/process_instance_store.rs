//! PostgreSQL implementation of `BpmnProcessInstanceStore` (v0.6 §8.4).

use async_trait::async_trait;
use bpmn_lite_store::process_instance::{
    BpmnProcessInstance, BpmnProcessInstanceStore, ProcessStatus,
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub struct PostgresBpmnProcessInstanceStore {
    pool: PgPool,
}

impl PostgresBpmnProcessInstanceStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl BpmnProcessInstanceStore for PostgresBpmnProcessInstanceStore {
    async fn insert(&self, instance: BpmnProcessInstance) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO bpmn_process_instance (
                id, workflow_id, current_node, status, variables,
                waiting_on_callout_id, waiting_on_execution_id,
                started_at, last_advanced_at, completed_at,
                end_status, failure_reason
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
        )
        .bind(instance.id)
        .bind(&instance.workflow_id)
        .bind(&instance.current_node)
        .bind(instance.status.as_str())
        .bind(&instance.variables)
        .bind(instance.waiting_on_callout_id)
        .bind(instance.waiting_on_execution_id)
        .bind(instance.started_at)
        .bind(instance.last_advanced_at)
        .bind(instance.completed_at)
        .bind(&instance.end_status)
        .bind(&instance.failure_reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn load(&self, id: Uuid) -> anyhow::Result<Option<BpmnProcessInstance>> {
        let row = sqlx::query(
            r#"
            SELECT id, workflow_id, current_node, status, variables,
                   waiting_on_callout_id, waiting_on_execution_id,
                   started_at, last_advanced_at, completed_at,
                   end_status, failure_reason
              FROM bpmn_process_instance
             WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_instance).transpose()
    }

    async fn update(&self, instance: BpmnProcessInstance) -> anyhow::Result<()> {
        let res = sqlx::query(
            r#"
            UPDATE bpmn_process_instance
               SET workflow_id = $2,
                   current_node = $3,
                   status = $4,
                   variables = $5,
                   waiting_on_callout_id = $6,
                   waiting_on_execution_id = $7,
                   last_advanced_at = $8,
                   completed_at = $9,
                   end_status = $10,
                   failure_reason = $11
             WHERE id = $1
            "#,
        )
        .bind(instance.id)
        .bind(&instance.workflow_id)
        .bind(&instance.current_node)
        .bind(instance.status.as_str())
        .bind(&instance.variables)
        .bind(instance.waiting_on_callout_id)
        .bind(instance.waiting_on_execution_id)
        .bind(instance.last_advanced_at)
        .bind(instance.completed_at)
        .bind(&instance.end_status)
        .bind(&instance.failure_reason)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() != 1 {
            anyhow::bail!("bpmn_process_instance id {} not found", instance.id);
        }
        Ok(())
    }

    async fn list_by_status(
        &self,
        status: ProcessStatus,
    ) -> anyhow::Result<Vec<BpmnProcessInstance>> {
        let rows = sqlx::query(
            r#"
            SELECT id, workflow_id, current_node, status, variables,
                   waiting_on_callout_id, waiting_on_execution_id,
                   started_at, last_advanced_at, completed_at,
                   end_status, failure_reason
              FROM bpmn_process_instance
             WHERE status = $1
             ORDER BY started_at
            "#,
        )
        .bind(status.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_instance).collect()
    }
}

fn row_to_instance(row: sqlx::postgres::PgRow) -> anyhow::Result<BpmnProcessInstance> {
    let status_str: String = row.try_get("status")?;
    Ok(BpmnProcessInstance {
        id: row.try_get("id")?,
        workflow_id: row.try_get("workflow_id")?,
        current_node: row.try_get("current_node")?,
        status: ProcessStatus::parse(&status_str)?,
        variables: row.try_get("variables")?,
        waiting_on_callout_id: row.try_get("waiting_on_callout_id")?,
        waiting_on_execution_id: row.try_get("waiting_on_execution_id")?,
        started_at: row.try_get("started_at")?,
        last_advanced_at: row.try_get("last_advanced_at")?,
        completed_at: row.try_get("completed_at")?,
        end_status: row.try_get("end_status")?,
        failure_reason: row.try_get("failure_reason")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_TEST_DATABASE_URL: &str = "postgresql://localhost/bpmn_lite_test";

    // Same workaround as `pending_store::tests`: skip `sqlx::migrate!`
    // because the pre-existing migration 026 has a broken
    // `GRANT CONNECT ON DATABASE current_database()` that can't be
    // applied to a fresh DB. Apply only the two T2B.8 migrations.
    const PENDING_MIGRATION: &str =
        include_str!("../migrations/033_bpmn_pending_invocation.sql");
    const PROCESS_MIGRATION: &str =
        include_str!("../migrations/034_bpmn_process_instance.sql");

    async fn setup() -> PostgresBpmnProcessInstanceStore {
        let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_owned());
        let pool = PgPool::connect(&url).await.expect("connect");
        sqlx::query("DROP TABLE IF EXISTS bpmn_pending_invocation CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DROP TABLE IF EXISTS bpmn_process_instance CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(PENDING_MIGRATION).execute(&pool).await.unwrap();
        sqlx::raw_sql(PROCESS_MIGRATION).execute(&pool).await.unwrap();
        sqlx::query("TRUNCATE bpmn_process_instance")
            .execute(&pool)
            .await
            .unwrap();
        PostgresBpmnProcessInstanceStore::new(pool)
    }

    fn fresh(id: Uuid) -> BpmnProcessInstance {
        BpmnProcessInstance::new(id, "custody-cbu-onboarding", "start")
    }

    #[tokio::test]
    #[ignore]
    async fn insert_then_load_round_trips() {
        let store = setup().await;
        let id = Uuid::now_v7();
        store.insert(fresh(id)).await.unwrap();
        let row = store.load(id).await.unwrap().unwrap();
        assert_eq!(row.workflow_id, "custody-cbu-onboarding");
        assert_eq!(row.current_node, "start");
        assert_eq!(row.status, ProcessStatus::Created);
    }

    #[tokio::test]
    #[ignore]
    async fn update_changes_status_and_waiting_pointers() {
        let store = setup().await;
        let id = Uuid::now_v7();
        store.insert(fresh(id)).await.unwrap();

        let mut row = store.load(id).await.unwrap().unwrap();
        row.status = ProcessStatus::WaitingOnSubmission;
        row.current_node = "create-cbu".into();
        let callout_id = Uuid::now_v7();
        row.waiting_on_callout_id = Some(callout_id);
        row.last_advanced_at = chrono::Utc::now();
        store.update(row).await.unwrap();

        let after = store.load(id).await.unwrap().unwrap();
        assert_eq!(after.status, ProcessStatus::WaitingOnSubmission);
        assert_eq!(after.current_node, "create-cbu");
        assert_eq!(after.waiting_on_callout_id, Some(callout_id));
        assert!(after.waiting_on_execution_id.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn update_rejects_unknown_id() {
        let store = setup().await;
        let err = store.update(fresh(Uuid::now_v7())).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    #[ignore]
    async fn status_check_constraint_rejects_bad_value() {
        // The Rust enum guarantees we never write a bad status, so
        // exercise the DB-level CHECK directly via raw SQL.
        let store = setup().await;
        let id = Uuid::now_v7();
        store.insert(fresh(id)).await.unwrap();
        let res = sqlx::query(
            "UPDATE bpmn_process_instance SET status = 'Bogus' WHERE id = $1",
        )
        .bind(id)
        .execute(&store.pool)
        .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    #[ignore]
    async fn list_by_status_groups_correctly() {
        let store = setup().await;
        for status in [
            ProcessStatus::Running,
            ProcessStatus::WaitingOnSubmission,
            ProcessStatus::WaitingOnSubmission,
            ProcessStatus::Completed,
        ] {
            let mut p = fresh(Uuid::now_v7());
            p.status = status;
            store.insert(p).await.unwrap();
        }
        assert_eq!(
            store
                .list_by_status(ProcessStatus::WaitingOnSubmission)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store
                .list_by_status(ProcessStatus::Completed)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .list_by_status(ProcessStatus::Failed)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn two_stage_durability_walks_status_correctly() {
        // Created → Running → WaitingOnSubmission → WaitingOnInvocation
        // → Completed. This is the happy-path the §10 demo walks per
        // BPMN node, verified end-to-end through the store.
        let store = setup().await;
        let id = Uuid::now_v7();
        store.insert(fresh(id)).await.unwrap();

        for next in [
            ProcessStatus::Running,
            ProcessStatus::WaitingOnSubmission,
            ProcessStatus::WaitingOnInvocation,
            ProcessStatus::Completed,
        ] {
            let mut row = store.load(id).await.unwrap().unwrap();
            row.status = next;
            row.last_advanced_at = chrono::Utc::now();
            store.update(row).await.unwrap();
            assert_eq!(store.load(id).await.unwrap().unwrap().status, next);
        }
    }
}
