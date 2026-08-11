use crate::manifest::ParameterManifest;
use crate::registry::{SourceFormat, TemplateState, TemplateStore, WorkflowTemplate};
use anyhow::{anyhow, Result};
use async_trait::async_trait;

/// PostgreSQL-backed TemplateStore.
///
/// Relies on migration 013_create_workflow_templates.sql for schema + immutability triggers.
pub struct PostgresTemplateStore {
    pool: sqlx::PgPool,
}

impl PostgresTemplateStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

fn state_to_str(s: &TemplateState) -> &'static str {
    match s {
        TemplateState::Draft => "draft",
        TemplateState::Published => "published",
        TemplateState::Retired => "retired",
    }
}

fn str_to_state(s: &str) -> Result<TemplateState> {
    match s {
        "draft" => Ok(TemplateState::Draft),
        "published" => Ok(TemplateState::Published),
        "retired" => Ok(TemplateState::Retired),
        other => Err(anyhow!("Unknown template state: {}", other)),
    }
}

fn format_to_str(f: &SourceFormat) -> &'static str {
    match f {
        SourceFormat::Yaml => "yaml",
        SourceFormat::BpmnImport => "bpmn_import",
        SourceFormat::Agent => "agent",
        SourceFormat::Graph => "graph",
    }
}

fn str_to_format(s: &str) -> Result<SourceFormat> {
    match s {
        "yaml" => Ok(SourceFormat::Yaml),
        "bpmn_import" => Ok(SourceFormat::BpmnImport),
        "agent" => Ok(SourceFormat::Agent),
        "graph" => Ok(SourceFormat::Graph),
        other => Err(anyhow!("Unknown source format: {}", other)),
    }
}

/// Maps the DB-trigger immutability rejection (migration 013's
/// `enforce_template_immutability`, surfaced as SQLSTATE P0001 —
/// `RAISE EXCEPTION`) onto the same error shape `MemoryTemplateStore`
/// produces ("Cannot modify ..."), so callers see comparable semantics
/// regardless of backend. Matches on the SQLSTATE (the mechanism), not
/// on message text.
fn map_immutability_error(e: sqlx::Error, key: &str, version: u32) -> anyhow::Error {
    if let Some(db) = e.as_database_error() {
        if db.code().as_deref() == Some("P0001") {
            return anyhow!(
                "Cannot modify published or retired template {}:v{}: {}",
                key,
                version,
                db.message()
            );
        }
    }
    e.into()
}

fn epoch_ms_to_datetime(epoch_ms: i64) -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    let secs = epoch_ms / 1000;
    let nanos = ((epoch_ms % 1000) * 1_000_000) as u32;
    chrono::Utc
        .timestamp_opt(secs, nanos)
        .single()
        .unwrap_or_else(chrono::Utc::now)
}

fn datetime_to_epoch_ms(dt: chrono::DateTime<chrono::Utc>) -> i64 {
    dt.timestamp_millis()
}

#[async_trait]
impl TemplateStore for PostgresTemplateStore {
    async fn save(&self, tpl: &WorkflowTemplate) -> Result<()> {
        let dto_json = serde_json::to_value(&tpl.dto_snapshot)?;
        let task_manifest_json = serde_json::to_value(&tpl.task_manifest)?;
        let parameter_manifest_json = serde_json::to_value(&tpl.parameter_manifest)?;
        let created = epoch_ms_to_datetime(tpl.created_at);
        let published = tpl.published_at.map(epoch_ms_to_datetime);

        sqlx::query(
            r#"
            INSERT INTO workflow_templates
                (template_key, template_version, process_key, bytecode_version,
                 state, source_format, dto_snapshot, task_manifest, parameter_manifest,
                 bpmn_xml, summary_md, verb_registry_hash, created_at, published_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            ON CONFLICT (template_key, template_version) DO UPDATE SET
                process_key = EXCLUDED.process_key,
                bytecode_version = EXCLUDED.bytecode_version,
                state = EXCLUDED.state,
                source_format = EXCLUDED.source_format,
                dto_snapshot = EXCLUDED.dto_snapshot,
                task_manifest = EXCLUDED.task_manifest,
                parameter_manifest = EXCLUDED.parameter_manifest,
                bpmn_xml = EXCLUDED.bpmn_xml,
                summary_md = EXCLUDED.summary_md,
                verb_registry_hash = EXCLUDED.verb_registry_hash,
                published_at = EXCLUDED.published_at
            "#,
        )
        .bind(&tpl.template_key)
        .bind(tpl.template_version as i32)
        .bind(&tpl.process_key)
        .bind(&tpl.bytecode_version)
        .bind(state_to_str(&tpl.state))
        .bind(format_to_str(&tpl.source_format))
        .bind(&dto_json)
        .bind(&task_manifest_json)
        .bind(&parameter_manifest_json)
        .bind(&tpl.bpmn_xml)
        .bind(&tpl.summary_md)
        .bind(&tpl.verb_registry_hash)
        .bind(created)
        .bind(published)
        .execute(&self.pool)
        .await
        .map_err(|e| map_immutability_error(e, &tpl.template_key, tpl.template_version))?;

        Ok(())
    }

    async fn load(&self, key: &str, version: u32) -> Result<Option<WorkflowTemplate>> {
        let row = sqlx::query_as::<_, TemplateRow>(
            r#"
            SELECT template_key, template_version, process_key, bytecode_version,
                   state, source_format, dto_snapshot, task_manifest, parameter_manifest,
                   bpmn_xml, summary_md, verb_registry_hash, created_at, published_at
            FROM workflow_templates
            WHERE template_key = $1 AND template_version = $2
            "#,
        )
        .bind(key)
        .bind(version as i32)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| r.into_template()).transpose()
    }

    async fn list(
        &self,
        key: Option<&str>,
        state: Option<TemplateState>,
    ) -> Result<Vec<WorkflowTemplate>> {
        let state_str = state.as_ref().map(state_to_str);

        let rows = sqlx::query_as::<_, TemplateRow>(
            r#"
            SELECT template_key, template_version, process_key, bytecode_version,
                   state, source_format, dto_snapshot, task_manifest, parameter_manifest,
                   bpmn_xml, summary_md, verb_registry_hash, created_at, published_at
            FROM workflow_templates
            WHERE ($1::text IS NULL OR template_key = $1)
              AND ($2::text IS NULL OR state = $2)
            ORDER BY template_key, template_version
            "#,
        )
        .bind(key)
        .bind(state_str)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|r| r.into_template()).collect()
    }

    async fn set_state(&self, key: &str, version: u32, new_state: TemplateState) -> Result<()> {
        let published_at = if new_state == TemplateState::Published {
            Some(chrono::Utc::now())
        } else {
            None
        };

        let result = sqlx::query(
            r#"
            UPDATE workflow_templates
            SET state = $3,
                published_at = COALESCE($4, published_at)
            WHERE template_key = $1 AND template_version = $2
            "#,
        )
        .bind(key)
        .bind(version as i32)
        .bind(state_to_str(&new_state))
        .bind(published_at)
        .execute(&self.pool)
        .await
        .map_err(|e| map_immutability_error(e, key, version))?;

        if result.rows_affected() == 0 {
            return Err(anyhow!("Template not found: {}:v{}", key, version));
        }

        Ok(())
    }

    async fn load_latest_published(&self, key: &str) -> Result<Option<WorkflowTemplate>> {
        let row = sqlx::query_as::<_, TemplateRow>(
            r#"
            SELECT template_key, template_version, process_key, bytecode_version,
                   state, source_format, dto_snapshot, task_manifest, parameter_manifest,
                   bpmn_xml, summary_md, verb_registry_hash, created_at, published_at
            FROM workflow_templates
            WHERE template_key = $1 AND state = 'published'
            ORDER BY template_version DESC
            LIMIT 1
            "#,
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| r.into_template()).transpose()
    }
}

/// Internal row type for sqlx deserialization.
#[derive(sqlx::FromRow)]
struct TemplateRow {
    template_key: String,
    template_version: i32,
    process_key: String,
    bytecode_version: String,
    state: String,
    source_format: String,
    dto_snapshot: serde_json::Value,
    task_manifest: serde_json::Value,
    parameter_manifest: Option<serde_json::Value>,
    bpmn_xml: Option<String>,
    summary_md: Option<String>,
    verb_registry_hash: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    published_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl TemplateRow {
    fn into_template(self) -> Result<WorkflowTemplate> {
        Ok(WorkflowTemplate {
            template_key: self.template_key,
            template_version: self.template_version as u32,
            process_key: self.process_key,
            bytecode_version: self.bytecode_version,
            state: str_to_state(&self.state)?,
            source_format: str_to_format(&self.source_format)?,
            dto_snapshot: serde_json::from_value(self.dto_snapshot)?,
            task_manifest: serde_json::from_value(self.task_manifest)?,
            parameter_manifest: match self.parameter_manifest {
                Some(v) => serde_json::from_value(v)?,
                None => ParameterManifest::default(),
            },
            bpmn_xml: self.bpmn_xml,
            summary_md: self.summary_md,
            verb_registry_hash: self.verb_registry_hash,
            created_at: datetime_to_epoch_ms(self.created_at),
            published_at: self.published_at.map(datetime_to_epoch_ms),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{EdgeDto, NodeDto, WorkflowGraphDto};

    async fn test_store() -> PostgresTemplateStore {
        let db_url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| "postgresql:///bpmn_lite_test".to_string());
        let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
        PostgresTemplateStore::new(pool)
    }

    fn sample_dto() -> WorkflowGraphDto {
        WorkflowGraphDto {
            id: "pg_test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![EdgeDto {
                from: "start".to_string(),
                to: "end".to_string(),
                condition: None,
                is_default: false,
                on_error: None,
            }],
            default_guard_budget: None,
            default_retry_policy: None,
        }
    }

    fn sample_template(key: &str, format: SourceFormat) -> WorkflowTemplate {
        WorkflowTemplate {
            template_key: key.to_string(),
            template_version: 1,
            process_key: "pg_test_proc".to_string(),
            bytecode_version: "abc123".to_string(),
            state: TemplateState::Draft,
            source_format: format,
            dto_snapshot: sample_dto(),
            task_manifest: vec!["do_work".to_string()],
            parameter_manifest: ParameterManifest::default(),
            bpmn_xml: None,
            summary_md: None,
            verb_registry_hash: None,
            created_at: 1000,
            published_at: None,
        }
    }

    /// T-PUB-10: Postgres save/load/set_state round-trip.
    /// Requires a running Postgres instance with the migration applied.
    #[tokio::test]
    async fn t_pub_10_postgres_round_trip() {
        let store = test_store().await;
        let template_key = format!("pg_test_wf_{}", uuid::Uuid::now_v7());

        let dto = WorkflowGraphDto {
            id: "pg_test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![EdgeDto {
                from: "start".to_string(),
                to: "end".to_string(),
                condition: None,
                is_default: false,
                on_error: None,
            }],
        default_guard_budget: None,
        default_retry_policy: None,
        };

        let tpl = WorkflowTemplate {
            template_key: template_key.clone(),
            template_version: 1,
            process_key: "pg_test_proc".to_string(),
            bytecode_version: "abc123".to_string(),
            state: TemplateState::Draft,
            source_format: SourceFormat::Yaml,
            dto_snapshot: dto,
            task_manifest: vec!["do_work".to_string()],
            parameter_manifest: ParameterManifest::default(),
            bpmn_xml: None,
            summary_md: None,
            verb_registry_hash: None,
            created_at: 1000,
            published_at: None,
        };

        // Save
        store.save(&tpl).await.unwrap();

        // Load
        let loaded = store.load(&template_key, 1).await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.state, TemplateState::Draft);

        // Publish
        store
            .set_state(&template_key, 1, TemplateState::Published)
            .await
            .unwrap();
        let published = store.load(&template_key, 1).await.unwrap().unwrap();
        assert_eq!(published.state, TemplateState::Published);

        // Load latest published (was a latent bug: queried the fixed prefix
        // "pg_test_wf" instead of the uuid-derived key)
        let latest = store.load_latest_published(&template_key).await.unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().template_version, 1);
    }

    /// T-PUB-11: Graph-format template round-trip — save published, load,
    /// list, load_latest_published all preserve `SourceFormat::Graph`.
    #[tokio::test]
    async fn t_pub_11_postgres_graph_format_round_trip() {
        let store = test_store().await;
        let template_key = format!("pg_test_graph_{}", uuid::Uuid::now_v7());

        let mut tpl = sample_template(&template_key, SourceFormat::Graph);
        tpl.state = TemplateState::Published;
        tpl.published_at = Some(2000);
        store.save(&tpl).await.unwrap();

        let loaded = store.load(&template_key, 1).await.unwrap().unwrap();
        assert_eq!(loaded.source_format, SourceFormat::Graph);
        assert_eq!(loaded.state, TemplateState::Published);

        let listed = store
            .list(Some(&template_key), Some(TemplateState::Published))
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].source_format, SourceFormat::Graph);

        let latest = store.load_latest_published(&template_key).await.unwrap();
        assert_eq!(latest.unwrap().source_format, SourceFormat::Graph);
    }

    /// T-PUB-12: DB-trigger immutability — re-saving changed content over a
    /// published row is rejected by migration 013's trigger, and the error
    /// is wrapped to match MemoryTemplateStore's "Cannot modify" semantics.
    #[tokio::test]
    async fn t_pub_12_postgres_published_immutable_via_trigger() {
        let store = test_store().await;
        let template_key = format!("pg_test_immut_{}", uuid::Uuid::now_v7());

        let tpl = sample_template(&template_key, SourceFormat::Graph);
        store.save(&tpl).await.unwrap();
        store
            .set_state(&template_key, 1, TemplateState::Published)
            .await
            .unwrap();

        // Re-save with changed content → DB trigger rejects, error is
        // comparable to MemoryTemplateStore's ("Cannot modify ...").
        let mut tpl2 = sample_template(&template_key, SourceFormat::Graph);
        tpl2.bytecode_version = "changed_version".to_string();
        let err = store.save(&tpl2).await.unwrap_err();
        assert!(
            err.to_string().contains("Cannot modify"),
            "expected wrapped immutability error, got: {err}"
        );

        // Published → Draft revert is also trigger-rejected and wrapped.
        let err = store
            .set_state(&template_key, 1, TemplateState::Draft)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Cannot modify"),
            "expected wrapped revert rejection, got: {err}"
        );

        // Content unchanged after the rejected writes.
        let still = store.load(&template_key, 1).await.unwrap().unwrap();
        assert_eq!(still.bytecode_version, "abc123");
        assert_eq!(still.state, TemplateState::Published);
    }
}
