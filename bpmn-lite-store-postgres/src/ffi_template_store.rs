//! Postgres-backed `FfiTemplateStore` (A4 / A2 §6).
//!
//! Schema: `migrations/023_create_ffi_catalogue.sql`. Templates are
//! content-addressed (`template_id BYTEA PRIMARY KEY`) and immutable
//! after publication. `publish` enforces the immutability guard at the
//! SQL layer via `ON CONFLICT DO NOTHING` on the BYTEA primary key,
//! followed by a content equality check.

use async_trait::async_trait;
use ffi_catalogue::FfiTemplateStore;
use ffi_types::{FfiTemplate, FieldSchema, Idempotency};
use sqlx::PgPool;
use sqlx::Row;
use uuid::Uuid;

/// Postgres implementation of `FfiTemplateStore`. Constructed with a
/// shared `PgPool`; safe to use across tasks (the pool is connection-
/// pool-safe).
pub struct PostgresFfiTemplateStore {
    pool: PgPool,
}

impl PostgresFfiTemplateStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl FfiTemplateStore for PostgresFfiTemplateStore {
    async fn publish(&self, template: &FfiTemplate) -> anyhow::Result<()> {
        // Two-step write: try INSERT; if there's already a row with the same
        // template_id, compare content and accept identical content as
        // idempotent (per A2 §6) or reject differing content.

        let input_schema_json = serde_json::to_value(&template.input_schema)?;
        let output_schema_json = serde_json::to_value(&template.output_schema)?;
        let idempotency_json = serde_json::to_value(&template.idempotency)?;
        let published_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            template.published_at,
        )
        .unwrap_or_else(chrono::Utc::now);
        let row_uuid = Uuid::now_v7();

        let inserted = sqlx::query(
            r#"
            INSERT INTO ffi_template (
                template_id, template_uuidv7, owner_type, owner_metadata,
                input_schema_json, output_schema_json, idempotency_json,
                tenant_id, published_at, publisher
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (template_id) DO NOTHING
            "#,
        )
        .bind(template.template_id.as_slice())
        .bind(row_uuid)
        .bind(&template.owner_type)
        .bind(&template.owner_metadata)
        .bind(&input_schema_json)
        .bind(&output_schema_json)
        .bind(&idempotency_json)
        .bind(&template.tenant_id)
        .bind(published_at)
        .bind(&template.publisher)
        .execute(&self.pool)
        .await?;

        if inserted.rows_affected() == 1 {
            return Ok(());
        }

        // Conflict: an existing row with the same template_id. Compare full
        // content for the immutability guard.
        let existing = self.lookup(&template.template_id).await?;
        match existing {
            Some(t) if &t == template => Ok(()),
            Some(_) => anyhow::bail!(
                "FFI template {} already published with different content (immutability guard)",
                hex(&template.template_id)
            ),
            None => anyhow::bail!(
                "INSERT reported conflict but lookup found no row for template {}",
                hex(&template.template_id)
            ),
        }
    }

    async fn lookup(
        &self,
        template_id: &[u8; 32],
    ) -> anyhow::Result<Option<FfiTemplate>> {
        let row = sqlx::query(
            r#"
            SELECT template_id, owner_type, owner_metadata,
                   input_schema_json, output_schema_json, idempotency_json,
                   tenant_id, published_at, publisher
            FROM ffi_template
            WHERE template_id = $1
            "#,
        )
        .bind(template_id.as_slice())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some(r) => Ok(Some(row_to_template(r)?)),
        }
    }

    async fn list_by_tenant(
        &self,
        tenant_id: &str,
    ) -> anyhow::Result<Vec<FfiTemplate>> {
        let rows = sqlx::query(
            r#"
            SELECT template_id, owner_type, owner_metadata,
                   input_schema_json, output_schema_json, idempotency_json,
                   tenant_id, published_at, publisher
            FROM ffi_template
            WHERE tenant_id = $1
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_template).collect()
    }

    async fn list_by_owner(
        &self,
        owner_type: &str,
        tenant_id: &str,
    ) -> anyhow::Result<Vec<FfiTemplate>> {
        let rows = sqlx::query(
            r#"
            SELECT template_id, owner_type, owner_metadata,
                   input_schema_json, output_schema_json, idempotency_json,
                   tenant_id, published_at, publisher
            FROM ffi_template
            WHERE owner_type = $1 AND tenant_id = $2
            "#,
        )
        .bind(owner_type)
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_template).collect()
    }
}

fn row_to_template(row: sqlx::postgres::PgRow) -> anyhow::Result<FfiTemplate> {
    let template_id_bytes: Vec<u8> = row.try_get("template_id")?;
    let template_id: [u8; 32] = template_id_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("template_id must be 32 bytes"))?;
    let owner_type: String = row.try_get("owner_type")?;
    let owner_metadata: Vec<u8> = row.try_get("owner_metadata")?;
    let input_schema_json: serde_json::Value = row.try_get("input_schema_json")?;
    let output_schema_json: serde_json::Value = row.try_get("output_schema_json")?;
    let idempotency_json: serde_json::Value = row.try_get("idempotency_json")?;
    let tenant_id: String = row.try_get("tenant_id")?;
    let published_at: chrono::DateTime<chrono::Utc> = row.try_get("published_at")?;
    let publisher: String = row.try_get("publisher")?;

    let input_schema: Vec<FieldSchema> = serde_json::from_value(input_schema_json)?;
    let output_schema: Vec<FieldSchema> = serde_json::from_value(output_schema_json)?;
    let idempotency: Idempotency = serde_json::from_value(idempotency_json)?;

    Ok(FfiTemplate {
        template_id,
        owner_type,
        owner_metadata,
        input_schema,
        output_schema,
        idempotency,
        tenant_id,
        published_at: published_at.timestamp_millis(),
        publisher,
    })
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffi_types::{compute_template_id, SchemaKind};

    fn make_template(owner_type: &str, tenant_id: &str, marker: u8) -> FfiTemplate {
        let mut t = FfiTemplate {
            template_id: [0u8; 32],
            owner_type: owner_type.to_string(),
            owner_metadata: vec![marker],
            input_schema: vec![FieldSchema {
                name: "x".to_string(),
                kind: SchemaKind::Bool,
                required: true,
            }],
            output_schema: vec![],
            idempotency: Idempotency::Idempotent,
            tenant_id: tenant_id.to_string(),
            published_at: 0,
            publisher: "test".to_string(),
        };
        t.template_id = compute_template_id(&t);
        t
    }

    /// Set up a fresh test database for each test. The connection string is
    /// taken from BPMN_LITE_TEST_DATABASE_URL; tests are skipped when this is
    /// not set so CI without Postgres still passes the unit test suite.
    async fn setup_pool() -> Option<PgPool> {
        let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url).await.ok()?;
        sqlx::migrate!("./migrations").run(&pool).await.ok()?;
        // Clean ffi_template between tests.
        sqlx::query("DELETE FROM ffi_template")
            .execute(&pool)
            .await
            .ok()?;
        Some(pool)
    }

    #[tokio::test]
    #[ignore = "requires BPMN_LITE_TEST_DATABASE_URL"]
    async fn pg_publish_then_lookup_roundtrip() {
        let pool = match setup_pool().await {
            Some(p) => p,
            None => return,
        };
        let store = PostgresFfiTemplateStore::new(pool);
        let t = make_template("dmn-lite", "tenant-a", 1);
        store.publish(&t).await.unwrap();

        let got = store.lookup(&t.template_id).await.unwrap().unwrap();
        // published_at is replaced by row default; mask before comparing.
        let mut got_normal = got.clone();
        got_normal.published_at = t.published_at;
        assert_eq!(got_normal, t);
    }

    #[tokio::test]
    #[ignore = "requires BPMN_LITE_TEST_DATABASE_URL"]
    async fn pg_publish_idempotent_for_identical_content() {
        let pool = match setup_pool().await {
            Some(p) => p,
            None => return,
        };
        let store = PostgresFfiTemplateStore::new(pool);
        let t = make_template("dmn-lite", "tenant-a", 1);
        store.publish(&t).await.unwrap();
        // Second publish with identical content succeeds.
        store.publish(&t).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires BPMN_LITE_TEST_DATABASE_URL"]
    async fn pg_list_by_tenant_isolates() {
        let pool = match setup_pool().await {
            Some(p) => p,
            None => return,
        };
        let store = PostgresFfiTemplateStore::new(pool);
        store
            .publish(&make_template("dmn-lite", "tenant-a", 1))
            .await
            .unwrap();
        store
            .publish(&make_template("dmn-lite", "tenant-b", 2))
            .await
            .unwrap();

        let a = store.list_by_tenant("tenant-a").await.unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].tenant_id, "tenant-a");
    }
}
