//! BPMN process instance state (v0.6 §8.4).
//!
//! One row per long-lived BPMN workflow instance. `status` drives the
//! executor's blocking states and is the source of truth on restart —
//! a process that survived a crash mid-callout is identified by
//! `WaitingOnSubmission` / `WaitingOnInvocation` plus the matching
//! `waiting_on_callout_id` / `waiting_on_execution_id`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::PoisonError;
use uuid::Uuid;

/// Lifecycle states a `bpmn_process_instance` row can occupy.
/// Mirrors the `CHECK` constraint in the migration verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessStatus {
    Created,
    Running,
    WaitingOnSubmission,
    WaitingOnInvocation,
    Completed,
    Failed,
}

impl ProcessStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "Created",
            Self::Running => "Running",
            Self::WaitingOnSubmission => "WaitingOnSubmission",
            Self::WaitingOnInvocation => "WaitingOnInvocation",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }

    pub fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "Created" => Self::Created,
            "Running" => Self::Running,
            "WaitingOnSubmission" => Self::WaitingOnSubmission,
            "WaitingOnInvocation" => Self::WaitingOnInvocation,
            "Completed" => Self::Completed,
            "Failed" => Self::Failed,
            other => {
                return Err(anyhow::anyhow!(
                    "invalid ProcessStatus '{other}' — schema CHECK constraint violated"
                ));
            }
        })
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

/// One row in `bpmn_process_instance`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BpmnProcessInstance {
    pub id: Uuid,
    pub workflow_id: String,
    pub current_node: String,
    pub status: ProcessStatus,
    pub variables: JsonValue,
    pub waiting_on_callout_id: Option<Uuid>,
    pub waiting_on_execution_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub last_advanced_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub end_status: Option<String>,
    pub failure_reason: Option<String>,
}

impl BpmnProcessInstance {
    /// Fresh instance at the start node, in `Created` status.
    pub fn new(
        id: Uuid,
        workflow_id: impl Into<String>,
        start_node: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            workflow_id: workflow_id.into(),
            current_node: start_node.into(),
            status: ProcessStatus::Created,
            variables: JsonValue::Object(serde_json::Map::new()),
            waiting_on_callout_id: None,
            waiting_on_execution_id: None,
            started_at: now,
            last_advanced_at: now,
            completed_at: None,
            end_status: None,
            failure_reason: None,
        }
    }

    /// Seed initial variables into the instance.
    pub fn with_variables(mut self, vars: JsonValue) -> Self {
        self.variables = vars;
        self
    }
}

/// Persistence boundary for `bpmn_process_instance`.
///
/// The trait deliberately exposes a coarse `update` rather than
/// fine-grained transitions — T3 will rebuild the executor and decide
/// the right transition vocabulary then. For now the executor can do
/// `let mut p = load(id); p.transition_to(...); update(&p)` inside a
/// transaction.
#[async_trait]
pub trait BpmnProcessInstanceStore: Send + Sync {
    /// Insert a fresh instance row. Errors if the id collides.
    async fn insert(&self, instance: BpmnProcessInstance) -> anyhow::Result<()>;

    /// Load by id. Returns `None` if no row matches.
    async fn load(&self, id: Uuid) -> anyhow::Result<Option<BpmnProcessInstance>>;

    /// Overwrite all mutable fields of the row identified by
    /// `instance.id`. Errors if no row matches.
    async fn update(&self, instance: BpmnProcessInstance) -> anyhow::Result<()>;

    /// List every instance in `status`. Used at startup to recover
    /// `WaitingOnSubmission` / `WaitingOnInvocation` rows whose
    /// process crashed before completion.
    async fn list_by_status(
        &self,
        status: ProcessStatus,
    ) -> anyhow::Result<Vec<BpmnProcessInstance>>;
}

/// In-memory `BpmnProcessInstanceStore`. Thread-safe; cloneable
/// through `Arc<dyn BpmnProcessInstanceStore>`.
#[derive(Default)]
pub struct MemoryBpmnProcessInstanceStore {
    by_id: Mutex<HashMap<Uuid, BpmnProcessInstance>>,
}

impl MemoryBpmnProcessInstanceStore {
    pub fn new() -> Self {
        Self::default()
    }
}

fn unpoison<G>(r: Result<G, PoisonError<G>>) -> G {
    r.unwrap_or_else(|p| p.into_inner())
}

#[async_trait]
impl BpmnProcessInstanceStore for MemoryBpmnProcessInstanceStore {
    async fn insert(&self, instance: BpmnProcessInstance) -> anyhow::Result<()> {
        let mut guard = unpoison(self.by_id.lock());
        if guard.contains_key(&instance.id) {
            anyhow::bail!("bpmn_process_instance id {} already exists", instance.id);
        }
        guard.insert(instance.id, instance);
        Ok(())
    }

    async fn load(&self, id: Uuid) -> anyhow::Result<Option<BpmnProcessInstance>> {
        let guard = unpoison(self.by_id.lock());
        Ok(guard.get(&id).cloned())
    }

    async fn update(&self, instance: BpmnProcessInstance) -> anyhow::Result<()> {
        let mut guard = unpoison(self.by_id.lock());
        if !guard.contains_key(&instance.id) {
            anyhow::bail!("bpmn_process_instance id {} not found", instance.id);
        }
        guard.insert(instance.id, instance);
        Ok(())
    }

    async fn list_by_status(
        &self,
        status: ProcessStatus,
    ) -> anyhow::Result<Vec<BpmnProcessInstance>> {
        let guard = unpoison(self.by_id.lock());
        Ok(guard.values().filter(|p| p.status == status).cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(id: Uuid) -> BpmnProcessInstance {
        BpmnProcessInstance::new(id, "custody-cbu-onboarding", "start")
    }

    #[tokio::test]
    async fn insert_then_load_returns_row() {
        let store = MemoryBpmnProcessInstanceStore::new();
        let id = Uuid::now_v7();
        store.insert(fresh(id)).await.unwrap();
        let row = store.load(id).await.unwrap().unwrap();
        assert_eq!(row.workflow_id, "custody-cbu-onboarding");
        assert_eq!(row.current_node, "start");
        assert_eq!(row.status, ProcessStatus::Created);
    }

    #[tokio::test]
    async fn insert_rejects_duplicate_id() {
        let store = MemoryBpmnProcessInstanceStore::new();
        let id = Uuid::now_v7();
        store.insert(fresh(id)).await.unwrap();
        let dup = store.insert(fresh(id)).await;
        assert!(dup.is_err());
    }

    #[tokio::test]
    async fn update_overwrites_mutable_fields() {
        let store = MemoryBpmnProcessInstanceStore::new();
        let id = Uuid::now_v7();
        store.insert(fresh(id)).await.unwrap();

        let mut row = store.load(id).await.unwrap().unwrap();
        row.status = ProcessStatus::WaitingOnSubmission;
        row.current_node = "create-cbu".into();
        row.waiting_on_callout_id = Some(Uuid::now_v7());
        store.update(row.clone()).await.unwrap();

        let after = store.load(id).await.unwrap().unwrap();
        assert_eq!(after.status, ProcessStatus::WaitingOnSubmission);
        assert_eq!(after.current_node, "create-cbu");
        assert!(after.waiting_on_callout_id.is_some());
    }

    #[tokio::test]
    async fn update_fails_on_missing_row() {
        let store = MemoryBpmnProcessInstanceStore::new();
        let err = store.update(fresh(Uuid::now_v7())).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn list_by_status_groups_correctly() {
        let store = MemoryBpmnProcessInstanceStore::new();
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

    #[test]
    fn process_status_parse_round_trip() {
        for s in [
            ProcessStatus::Created,
            ProcessStatus::Running,
            ProcessStatus::WaitingOnSubmission,
            ProcessStatus::WaitingOnInvocation,
            ProcessStatus::Completed,
            ProcessStatus::Failed,
        ] {
            assert_eq!(ProcessStatus::parse(s.as_str()).unwrap(), s);
        }
        assert!(ProcessStatus::parse("Bogus").is_err());
    }

    #[test]
    fn terminal_status_predicate() {
        assert!(ProcessStatus::Completed.is_terminal());
        assert!(ProcessStatus::Failed.is_terminal());
        assert!(!ProcessStatus::Running.is_terminal());
        assert!(!ProcessStatus::WaitingOnSubmission.is_terminal());
    }
}
