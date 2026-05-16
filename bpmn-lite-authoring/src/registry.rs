use crate::dto::WorkflowGraphDto;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

// ── Template State Machine ──
// Draft → Published → Retired
// (no backward transitions from Retired)

/// State of a workflow template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TemplateState {
    Draft,
    Published,
    Retired,
}

/// How the template was authored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceFormat {
    Yaml,
    BpmnImport,
    Agent,
}

/// A versioned workflow template — the publish artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplate {
    pub template_key: String,
    pub template_version: u32,
    pub process_key: String,
    pub bytecode_version: String,
    pub state: TemplateState,
    pub source_format: SourceFormat,
    pub dto_snapshot: WorkflowGraphDto,
    pub task_manifest: Vec<String>,
    pub bpmn_xml: Option<String>,
    pub summary_md: Option<String>,
    pub verb_registry_hash: Option<String>,
    pub created_at: i64,
    pub published_at: Option<i64>,
}

/// Persistence trait for workflow templates.
#[async_trait]
pub trait TemplateStore: Send + Sync {
    async fn save(&self, tpl: &WorkflowTemplate) -> Result<()>;
    async fn load(&self, key: &str, version: u32) -> Result<Option<WorkflowTemplate>>;
    async fn list(
        &self,
        key: Option<&str>,
        state: Option<TemplateState>,
    ) -> Result<Vec<WorkflowTemplate>>;
    async fn set_state(&self, key: &str, version: u32, new_state: TemplateState) -> Result<()>;
    async fn load_latest_published(&self, key: &str) -> Result<Option<WorkflowTemplate>>;
}

// ── MemoryTemplateStore ──

type StoreKey = (String, u32);

/// In-memory TemplateStore for testing and POC.
///
/// Enforces immutability rules:
/// - Published content cannot be modified (only state → Retired)
/// - Retired cannot transition back to Draft or Published
/// - Valid transitions: Draft→Published, Published→Retired
pub struct MemoryTemplateStore {
    inner: RwLock<HashMap<StoreKey, WorkflowTemplate>>,
}

impl MemoryTemplateStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryTemplateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TemplateStore for MemoryTemplateStore {
    async fn save(&self, tpl: &WorkflowTemplate) -> Result<()> {
        let key = (tpl.template_key.clone(), tpl.template_version);
        let mut store = self.inner.write().map_err(|e| anyhow!("Lock: {}", e))?;

        // Immutability guard: cannot overwrite Published or Retired templates
        if let Some(existing) = store.get(&key) {
            match existing.state {
                TemplateState::Published => {
                    return Err(anyhow!(
                        "Cannot modify published template {}:v{}",
                        tpl.template_key,
                        tpl.template_version
                    ));
                }
                TemplateState::Retired => {
                    return Err(anyhow!(
                        "Cannot modify retired template {}:v{}",
                        tpl.template_key,
                        tpl.template_version
                    ));
                }
                TemplateState::Draft => {
                    // Draft can be overwritten
                }
            }
        }

        store.insert(key, tpl.clone());
        Ok(())
    }

    async fn load(&self, key: &str, version: u32) -> Result<Option<WorkflowTemplate>> {
        let store = self.inner.read().map_err(|e| anyhow!("Lock: {}", e))?;
        Ok(store.get(&(key.to_string(), version)).cloned())
    }

    async fn list(
        &self,
        key: Option<&str>,
        state: Option<TemplateState>,
    ) -> Result<Vec<WorkflowTemplate>> {
        let store = self.inner.read().map_err(|e| anyhow!("Lock: {}", e))?;
        let results: Vec<_> = store
            .values()
            .filter(|tpl| {
                if let Some(k) = key {
                    if tpl.template_key != k {
                        return false;
                    }
                }
                if let Some(ref s) = state {
                    if &tpl.state != s {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        Ok(results)
    }

    async fn set_state(&self, key: &str, version: u32, new_state: TemplateState) -> Result<()> {
        let store_key = (key.to_string(), version);
        let mut store = self.inner.write().map_err(|e| anyhow!("Lock: {}", e))?;

        let tpl = store
            .get_mut(&store_key)
            .ok_or_else(|| anyhow!("Template not found: {}:v{}", key, version))?;

        // State transition validation
        match (&tpl.state, &new_state) {
            (TemplateState::Draft, TemplateState::Published) => {}
            (TemplateState::Published, TemplateState::Retired) => {}
            (from, to) => {
                return Err(anyhow!(
                    "Invalid state transition {:?} → {:?} for {}:v{}",
                    from,
                    to,
                    key,
                    version
                ));
            }
        }

        tpl.state = new_state;
        if tpl.state == TemplateState::Published && tpl.published_at.is_none() {
            tpl.published_at = Some(now_ms());
        }

        Ok(())
    }

    async fn load_latest_published(&self, key: &str) -> Result<Option<WorkflowTemplate>> {
        let store = self.inner.read().map_err(|e| anyhow!("Lock: {}", e))?;
        let latest = store
            .values()
            .filter(|tpl| tpl.template_key == key && tpl.state == TemplateState::Published)
            .max_by_key(|tpl| tpl.template_version)
            .cloned();
        Ok(latest)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{EdgeDto, NodeDto, WorkflowGraphDto};

    fn sample_dto() -> WorkflowGraphDto {
        WorkflowGraphDto {
            id: "test".to_string(),
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
        }
    }

    fn sample_template(key: &str, version: u32, state: TemplateState) -> WorkflowTemplate {
        WorkflowTemplate {
            template_key: key.to_string(),
            template_version: version,
            process_key: format!("{}_process", key),
            bytecode_version: "abc123".to_string(),
            state,
            source_format: SourceFormat::Yaml,
            dto_snapshot: sample_dto(),
            task_manifest: vec!["do_work".to_string()],
            bpmn_xml: None,
            summary_md: None,
            verb_registry_hash: None,
            created_at: 1000,
            published_at: None,
        }
    }

    /// T-PUB-1: MemoryTemplateStore save + load round-trip.
    #[tokio::test]
    async fn t_pub_1_save_load_round_trip() {
        let store = MemoryTemplateStore::new();
        let tpl = sample_template("wf1", 1, TemplateState::Draft);

        store.save(&tpl).await.unwrap();
        let loaded = store.load("wf1", 1).await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.template_key, "wf1");
        assert_eq!(loaded.template_version, 1);
        assert_eq!(loaded.state, TemplateState::Draft);
    }

    /// T-PUB-2: Valid transitions: Draft→Published→Retired; Retired→Draft rejected.
    #[tokio::test]
    async fn t_pub_2_state_transitions() {
        let store = MemoryTemplateStore::new();
        let tpl = sample_template("wf1", 1, TemplateState::Draft);
        store.save(&tpl).await.unwrap();

        // Draft → Published: OK
        store
            .set_state("wf1", 1, TemplateState::Published)
            .await
            .unwrap();
        let loaded = store.load("wf1", 1).await.unwrap().unwrap();
        assert_eq!(loaded.state, TemplateState::Published);
        assert!(loaded.published_at.is_some());

        // Published → Retired: OK
        store
            .set_state("wf1", 1, TemplateState::Retired)
            .await
            .unwrap();
        let loaded = store.load("wf1", 1).await.unwrap().unwrap();
        assert_eq!(loaded.state, TemplateState::Retired);

        // Retired → Draft: REJECTED
        let result = store.set_state("wf1", 1, TemplateState::Draft).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid state"));

        // Retired → Published: REJECTED
        let result = store.set_state("wf1", 1, TemplateState::Published).await;
        assert!(result.is_err());
    }

    /// T-PUB-3: Published content immutable (save rejected).
    #[tokio::test]
    async fn t_pub_3_published_immutable() {
        let store = MemoryTemplateStore::new();
        let tpl = sample_template("wf1", 1, TemplateState::Draft);
        store.save(&tpl).await.unwrap();
        store
            .set_state("wf1", 1, TemplateState::Published)
            .await
            .unwrap();

        // Try to overwrite published template
        let mut tpl2 = sample_template("wf1", 1, TemplateState::Published);
        tpl2.bytecode_version = "new_version".to_string();
        let result = store.save(&tpl2).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot modify"));
    }

    /// T-PUB-4: List filters by state.
    #[tokio::test]
    async fn t_pub_4_list_filters() {
        let store = MemoryTemplateStore::new();

        let tpl1 = sample_template("wf1", 1, TemplateState::Draft);
        let tpl2 = sample_template("wf1", 2, TemplateState::Draft);
        let tpl3 = sample_template("wf2", 1, TemplateState::Draft);
        store.save(&tpl1).await.unwrap();
        store.save(&tpl2).await.unwrap();
        store.save(&tpl3).await.unwrap();

        // Publish wf1:v1
        store
            .set_state("wf1", 1, TemplateState::Published)
            .await
            .unwrap();

        // List all → 3
        let all = store.list(None, None).await.unwrap();
        assert_eq!(all.len(), 3);

        // List by key → 2
        let wf1 = store.list(Some("wf1"), None).await.unwrap();
        assert_eq!(wf1.len(), 2);

        // List published → 1
        let published = store
            .list(None, Some(TemplateState::Published))
            .await
            .unwrap();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].template_key, "wf1");

        // List draft → 2
        let drafts = store.list(None, Some(TemplateState::Draft)).await.unwrap();
        assert_eq!(drafts.len(), 2);
    }

    /// T-PUB-5: load_latest_published returns highest published version.
    #[tokio::test]
    async fn t_pub_5_latest_published() {
        let store = MemoryTemplateStore::new();

        // Create v1 and v2, publish both
        let tpl1 = sample_template("wf1", 1, TemplateState::Draft);
        let tpl2 = sample_template("wf1", 2, TemplateState::Draft);
        store.save(&tpl1).await.unwrap();
        store.save(&tpl2).await.unwrap();
        store
            .set_state("wf1", 1, TemplateState::Published)
            .await
            .unwrap();
        store
            .set_state("wf1", 2, TemplateState::Published)
            .await
            .unwrap();

        let latest = store.load_latest_published("wf1").await.unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().template_version, 2);

        // Retire v2 — latest published should be v1
        store
            .set_state("wf1", 2, TemplateState::Retired)
            .await
            .unwrap();
        let latest = store.load_latest_published("wf1").await.unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().template_version, 1);

        // No published for unknown key
        let none = store.load_latest_published("unknown").await.unwrap();
        assert!(none.is_none());
    }
}
