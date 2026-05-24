//! Pending-invocation registry (v0.6 §8.3).
//!
//! One row per in-flight cross-domain callout the bpmn-lite executor
//! has submitted but not yet received a result for. The lifecycle has
//! three stages, each with a distinct trait method:
//!
//! ```text
//! Stage 1 (insert)          row inserted with callout_id + idempotency_key;
//!                           execution_id = None, ack_received_at = None.
//!                           Tx-coupled with the matching outbox row.
//!
//! Stage 2 (record_ack)      sender's SubmissionAck arrived; row updated
//!                           with execution_id + ack_received_at.
//!                           Tx-coupled with the process_instance
//!                           transition WaitingOnSubmission → WaitingOnInvocation.
//!
//! Stage 3 (take_by_exec)    result arrived via ResultService.DeliverResult;
//!                           row deleted and returned in one query.
//!                           Tx-coupled with the process_instance advance.
//! ```

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

/// One row in `bpmn_pending_invocation`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingInvocation {
    pub callout_id: Uuid,
    pub process_instance_id: Uuid,
    pub node_id: String,
    pub target_domain: String,
    pub verb_id: String,
    pub idempotency_key: Uuid,
    pub execution_id: Option<Uuid>,
    pub submitted_at: DateTime<Utc>,
    pub ack_received_at: Option<DateTime<Utc>>,
    pub timeout_at: Option<DateTime<Utc>>,
}

impl PendingInvocation {
    /// Build a stage-1 record. `submitted_at` is set to `now()`;
    /// `execution_id` / `ack_received_at` start unset.
    pub fn new(
        callout_id: Uuid,
        process_instance_id: Uuid,
        node_id: impl Into<String>,
        target_domain: impl Into<String>,
        verb_id: impl Into<String>,
        idempotency_key: Uuid,
    ) -> Self {
        Self {
            callout_id,
            process_instance_id,
            node_id: node_id.into(),
            target_domain: target_domain.into(),
            verb_id: verb_id.into(),
            idempotency_key,
            execution_id: None,
            submitted_at: Utc::now(),
            ack_received_at: None,
            timeout_at: None,
        }
    }

    pub fn with_timeout(mut self, deadline: DateTime<Utc>) -> Self {
        self.timeout_at = Some(deadline);
        self
    }
}

/// Persistence boundary for `bpmn_pending_invocation`. The Postgres
/// implementation lives in `bpmn-lite-store-postgres`; the in-memory
/// implementation below is the default for tests and the
/// memory-only deployment.
///
/// Implementations need to be safe to call from many tokio tasks
/// concurrently — `bpmn-lite-bus-handler::ProcessAdvancer` calls
/// [`take_by_execution_id`] from a tonic request handler in parallel
/// with the outbox sender calling [`record_ack`].
#[async_trait]
pub trait PendingInvocationStore: Send + Sync {
    /// Stage 1 — caller has committed intent. Insert returns the
    /// duplicate-detection outcome so re-submits of the same
    /// `callout_id` don't multiply rows.
    async fn insert(&self, record: PendingInvocation) -> anyhow::Result<InsertOutcome>;

    /// Stage 2 — record the receiver's `execution_id` and the time
    /// of the SubmissionAck. Returns `Err` if no row matches.
    async fn record_ack(
        &self,
        callout_id: Uuid,
        execution_id: Uuid,
        ack_received_at: DateTime<Utc>,
    ) -> anyhow::Result<()>;

    /// Stage 3 — atomic look-up + delete by execution_id. Returns
    /// `None` if no row matches (e.g. duplicate result delivery).
    async fn take_by_execution_id(
        &self,
        execution_id: Uuid,
    ) -> anyhow::Result<Option<PendingInvocation>>;

    /// Diagnostic / sender-side helper. Returns `None` if no row
    /// matches.
    async fn lookup_by_callout_id(
        &self,
        callout_id: Uuid,
    ) -> anyhow::Result<Option<PendingInvocation>>;

    /// Diagnostic / recovery sweep — list every pending row for a
    /// given process instance (e.g. on startup to inventory what's
    /// in flight).
    async fn list_for_process(
        &self,
        process_instance_id: Uuid,
    ) -> anyhow::Result<Vec<PendingInvocation>>;
}

/// Did the insert land or did the `(callout_id)` PK already hold the row?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Inserted,
    Duplicate,
}

impl InsertOutcome {
    pub const fn was_inserted(self) -> bool {
        matches!(self, Self::Inserted)
    }
}

/// In-memory `PendingInvocationStore`. Thread-safe; cloneable through
/// `Arc<dyn PendingInvocationStore>`.
#[derive(Default)]
pub struct MemoryPendingInvocationStore {
    by_callout: Mutex<HashMap<Uuid, PendingInvocation>>,
}

impl MemoryPendingInvocationStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PendingInvocationStore for MemoryPendingInvocationStore {
    async fn insert(&self, record: PendingInvocation) -> anyhow::Result<InsertOutcome> {
        let mut guard = self.by_callout.lock().expect("poisoned");
        if guard.contains_key(&record.callout_id) {
            return Ok(InsertOutcome::Duplicate);
        }
        // Also check the idempotency_key UNIQUE constraint that Postgres enforces.
        for existing in guard.values() {
            if existing.idempotency_key == record.idempotency_key {
                return Ok(InsertOutcome::Duplicate);
            }
        }
        guard.insert(record.callout_id, record);
        Ok(InsertOutcome::Inserted)
    }

    async fn record_ack(
        &self,
        callout_id: Uuid,
        execution_id: Uuid,
        ack_received_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let mut guard = self.by_callout.lock().expect("poisoned");
        let row = guard
            .get_mut(&callout_id)
            .ok_or_else(|| anyhow::anyhow!("no pending row for callout_id {callout_id}"))?;
        row.execution_id = Some(execution_id);
        row.ack_received_at = Some(ack_received_at);
        Ok(())
    }

    async fn take_by_execution_id(
        &self,
        execution_id: Uuid,
    ) -> anyhow::Result<Option<PendingInvocation>> {
        let mut guard = self.by_callout.lock().expect("poisoned");
        let key = guard
            .iter()
            .find(|(_, v)| v.execution_id == Some(execution_id))
            .map(|(k, _)| *k);
        Ok(key.and_then(|k| guard.remove(&k)))
    }

    async fn lookup_by_callout_id(
        &self,
        callout_id: Uuid,
    ) -> anyhow::Result<Option<PendingInvocation>> {
        let guard = self.by_callout.lock().expect("poisoned");
        Ok(guard.get(&callout_id).cloned())
    }

    async fn list_for_process(
        &self,
        process_instance_id: Uuid,
    ) -> anyhow::Result<Vec<PendingInvocation>> {
        let guard = self.by_callout.lock().expect("poisoned");
        Ok(guard
            .values()
            .filter(|r| r.process_instance_id == process_instance_id)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(callout: Uuid, process: Uuid, idem: Uuid) -> PendingInvocation {
        PendingInvocation::new(callout, process, "node-x", "ob-poc", "cbu.create", idem)
    }

    #[tokio::test]
    async fn insert_then_lookup_returns_row() {
        let store = MemoryPendingInvocationStore::new();
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
    }

    #[tokio::test]
    async fn duplicate_callout_id_returns_duplicate() {
        let store = MemoryPendingInvocationStore::new();
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
    async fn duplicate_idempotency_key_returns_duplicate() {
        let store = MemoryPendingInvocationStore::new();
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
    async fn record_ack_fills_execution_id_and_ack_time() {
        let store = MemoryPendingInvocationStore::new();
        let cid = Uuid::now_v7();
        store
            .insert(record(cid, Uuid::now_v7(), Uuid::now_v7()))
            .await
            .unwrap();

        let exec = Uuid::now_v7();
        let now = Utc::now();
        store.record_ack(cid, exec, now).await.unwrap();

        let row = store.lookup_by_callout_id(cid).await.unwrap().unwrap();
        assert_eq!(row.execution_id, Some(exec));
        assert_eq!(row.ack_received_at, Some(now));
    }

    #[tokio::test]
    async fn take_by_execution_id_removes_and_returns_row() {
        let store = MemoryPendingInvocationStore::new();
        let cid = Uuid::now_v7();
        store
            .insert(record(cid, Uuid::now_v7(), Uuid::now_v7()))
            .await
            .unwrap();
        let exec = Uuid::now_v7();
        store.record_ack(cid, exec, Utc::now()).await.unwrap();

        let taken = store.take_by_execution_id(exec).await.unwrap();
        assert!(taken.is_some());
        // Second take returns None — duplicate result delivery is a no-op.
        let taken_again = store.take_by_execution_id(exec).await.unwrap();
        assert!(taken_again.is_none());
        // lookup_by_callout_id also returns None now.
        assert!(store.lookup_by_callout_id(cid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_for_process_returns_only_matching_rows() {
        let store = MemoryPendingInvocationStore::new();
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

        let b_rows = store.list_for_process(pid_b).await.unwrap();
        assert_eq!(b_rows.len(), 1);
    }
}
