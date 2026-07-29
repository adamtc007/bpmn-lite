//! Federated DSL bus runtime for `bpmn-lite-server` (v0.6 §T2B.9 + T3.4).
//!
//! T3.4 update: `StoreBackedAdvancer` now operates on `WorkflowStore`
//! (the bytecode engine store with plan_hash support) instead of the
//! separate `BpmnProcessInstanceStore`. When a result arrives for a
//! plan-based instance it:
//!
//! 1. Takes the pending invocation row (establishes which node fired).
//! 2. Loads the `ProcessInstance` via `WorkflowStore`.
//! 3. If the instance has a `plan_hash`, loads the plan and advances
//!    `current_node_id` to the completed node's `next` neighbour.
//! 4. Populates `placeholder_values` from the result bindings.
//! 5. Sets `instance.state = ProcessState::Running` so the tick loop
//!    picks it up and walks the next plan node.

#![cfg(feature = "postgres")]

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bpmn_lite_bus_handler::{
    BpmnLiteBusHandler, ProcessAdvanceInput, ProcessAdvancer, ProcessAdvancerError,
};
use bpmn_lite_store::pending::PendingInvocationStore;
use bpmn_lite_store::store::{transition_from_tick_ops, TickOperation, WorkflowStore};
use bpmn_lite_store_postgres::PostgresPendingInvocationStore;
use bpmn_lite_types::ProcessState;
use bpmn_lite_types::TenantId;
use dsl_bus_client::BusClient;
use dsl_bus_client::{SubmissionAckFuture, SubmissionAckHandler};
use dsl_bus_protocol::v1::ExecutionOutcomeKind;
use dsl_bus_server::{BusServer, ServerHandle};
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) struct StoreSubmissionAckHandler {
    pending: Arc<PostgresPendingInvocationStore>,
    store: Arc<dyn WorkflowStore>,
}

impl StoreSubmissionAckHandler {
    pub(crate) fn new(
        pending: Arc<PostgresPendingInvocationStore>,
        store: Arc<dyn WorkflowStore>,
    ) -> Self {
        Self { pending, store }
    }
}

impl SubmissionAckHandler for StoreSubmissionAckHandler {
    fn accepted<'a>(
        &'a self,
        entry: &'a dsl_bus_storage::OutboxEntry,
        execution_id: Uuid,
    ) -> SubmissionAckFuture<'a> {
        Box::pin(async move {
            let callout_id = entry
                .callout_id
                .ok_or_else(|| "invocation outbox row has no callout_id".to_string())?;
            let dispatch_claim_token = entry
                .claim_token()
                .ok_or_else(|| "claimed outbox row has no claim token".to_string())?;
            let pending = self
                .pending
                .lookup_by_callout_id(
                    &TenantId::new(entry.tenant_id.clone()).map_err(|error| error.to_string())?,
                    callout_id,
                )
                .await
                .map_err(|error| format!("lookup pending invocation: {error}"))?
                .ok_or_else(|| format!("no pending invocation for callout {callout_id}"))?;
            let mut instance = self
                .store
                .load_instance(&pending.tenant_id, pending.process_instance_id)
                .await
                .map_err(|error| format!("load ack instance: {error}"))?
                .ok_or_else(|| "pending invocation references missing instance".to_string())?;
            if matches!(
                instance.state,
                ProcessState::WaitingOnInvocation { execution_id: current, .. }
                    if current == execution_id
            ) {
                return Ok(());
            }
            let ProcessState::WaitingOnSubmission {
                callout_id: waiting_callout,
                node_id,
            } = &instance.state
            else {
                return Err("submission ack does not match instance state".to_string());
            };
            if *waiting_callout != callout_id {
                return Err("submission ack callout identity mismatch".to_string());
            }
            let node_id = node_id.clone();
            let owner = format!("bus-ack-{dispatch_claim_token}");
            let claim = self
                .store
                .claim_instance_for_transition(
                    &pending.tenant_id,
                    instance.instance_id,
                    &owner,
                    30_000,
                )
                .await
                .map_err(|error| format!("claim ack instance: {error}"))?
                .ok_or_else(|| "submission ack lost instance claim race".to_string())?;
            instance.state = ProcessState::WaitingOnInvocation {
                execution_id,
                node_id,
            };
            let transition = bpmn_lite_types::TransitionBuilder::new(instance.clone())
                .bus_submission_ack(bpmn_lite_types::BusSubmissionAckMutation::new(
                    entry.id,
                    callout_id,
                    execution_id,
                    dispatch_claim_token,
                ))
                .build();
            let commit = self.store.commit_transition(&claim, &transition).await;
            let release = self
                .store
                .release_instance_transition(&pending.tenant_id, instance.instance_id, &owner)
                .await;
            commit.map_err(|error| format!("commit submission ack: {error}"))?;
            release.map_err(|error| format!("release ack instance: {error}"))?;
            Ok(())
        })
    }
}

/// Owned bus runtime.
pub(crate) struct BusRuntime {
    server: ServerHandle,
    sender: dsl_bus_client::SenderHandle,
}

impl BusRuntime {
    pub(crate) async fn shutdown(self) -> anyhow::Result<()> {
        let _ = self.server.shutdown().await;
        let _ = self.sender.shutdown().await;
        Ok(())
    }
}

/// Configuration plumbed in by `main`.
pub(crate) struct BusRuntimeConfig {
    pub(crate) pool: PgPool,
    pub(crate) bind_addr: SocketAddr,
    /// Pre-built bus client (T3.3 — built before engine so it can be wired in).
    pub(crate) client: Arc<BusClient>,
    /// T3.4 — engine's WorkflowStore for loading/saving plan-based instances.
    pub(crate) store: Arc<dyn WorkflowStore>,
    pub(crate) engine: Arc<bpmn_lite_engine::BpmnLiteEngine>,
}

pub(crate) async fn start(config: BusRuntimeConfig) -> anyhow::Result<BusRuntime> {
    // Startup as bpmn_lite_app role assumes database schema is already migrated by admin role.
    let client = config.client;
    let notifier = client.outbox_notifier();
    let sender = client.start_sender();

    let advancer = StoreBackedAdvancer {
        pending: Arc::new(PostgresPendingInvocationStore::new(config.pool.clone())),
        store: config.store,
    };

    let server = BusServer::builder()
        .pool(config.pool.clone())
        .local_domain("bpmn-lite")
        .invocation_dispatcher(BpmnLiteBusHandler::new_with_engine(
            advancer.clone(),
            config.engine.clone(),
            config.pool.clone(),
        ))
        .result_dispatcher(BpmnLiteBusHandler::new(advancer))
        .outbox_notifier(notifier)
        .bind(config.bind_addr)
        .build()
        .serve()
        .await?;

    tracing::info!(
        bind_addr = %server.local_addr(),
        "bpmn-lite bus server listening (result receiver)"
    );

    Ok(BusRuntime { server, sender })
}

// ── StoreBackedAdvancer ──────────────────────────────────────────────

/// T3.4 — advances plan-based process instances on result arrival.
///
/// Flow:
/// 1. Take pending invocation row (establishes node_id + process_instance_id).
/// 2. Load ProcessInstance from WorkflowStore.
/// 3. For plan-based instances, load the plan, advance past the completed node,
///    and bind validated result values.
/// 4. Set state = Running (tick loop will call PlanWalker.advance() on next cycle).
/// 5. For terminal outcomes (VerbFailed etc.) set state = Failed.
#[derive(Clone)]
struct StoreBackedAdvancer {
    pending: Arc<PostgresPendingInvocationStore>,
    store: Arc<dyn WorkflowStore>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutcomeDisposition {
    Commit,
    Retry,
    Terminal,
    Malformed,
}

fn classify_outcome(outcome: ExecutionOutcomeKind) -> OutcomeDisposition {
    match outcome {
        ExecutionOutcomeKind::Committed | ExecutionOutcomeKind::IdempotentReplayReturned => {
            OutcomeDisposition::Commit
        }
        ExecutionOutcomeKind::OptimisticConflict | ExecutionOutcomeKind::LockTimeout => {
            OutcomeDisposition::Retry
        }
        ExecutionOutcomeKind::OutcomeUnspecified => OutcomeDisposition::Malformed,
        ExecutionOutcomeKind::VerbFailed
        | ExecutionOutcomeKind::AuthorityDenied
        | ExecutionOutcomeKind::Cancelled
        | ExecutionOutcomeKind::TimedOut
        | ExecutionOutcomeKind::PanicRecovered
        | ExecutionOutcomeKind::RejectedByAdmission
        | ExecutionOutcomeKind::VersionMismatch => OutcomeDisposition::Terminal,
    }
}

#[async_trait]
impl ProcessAdvancer for StoreBackedAdvancer {
    async fn advance(&self, input: ProcessAdvanceInput) -> Result<(), ProcessAdvancerError> {
        let tenant_id = TenantId::new(input.tenant_id.clone())
            .map_err(|error| ProcessAdvancerError::Malformed(error.to_string()))?;
        let row = self
            .pending
            .lookup_by_execution_id(&tenant_id, input.execution_id)
            .await
            .map_err(|e| ProcessAdvancerError::Internal(format!("lookup pending: {e}")))?;

        let Some(row) = row else {
            return Ok(());
        };

        let owner = format!("bus-resumer-{}", Uuid::now_v7());

        let mut instance = match self
            .store
            .load_instance(&row.tenant_id, row.process_instance_id)
            .await
        {
            Ok(Some(inst)) => inst,
            Ok(None) => {
                return Err(ProcessAdvancerError::Internal(format!(
                    "pending row referenced unknown instance {}",
                    row.process_instance_id
                )));
            }
            Err(e) => {
                return Err(ProcessAdvancerError::Internal(format!(
                    "load instance: {e}"
                )));
            }
        };

        let claim = self
            .store
            .claim_instance_for_transition(&row.tenant_id, instance.instance_id, &owner, 30_000)
            .await
            .map_err(|e| ProcessAdvancerError::Internal(format!("claim instance: {e}")))?;
        let claim = claim.ok_or_else(|| {
            ProcessAdvancerError::Internal(
                "failed to claim instance lease for bus resume".to_owned(),
            )
        })?;

        match classify_outcome(input.outcome_kind) {
            OutcomeDisposition::Commit => {
                let incident_id = bpmn_lite_types::EffectId::for_transition(
                    instance.instance_id,
                    claim.expected_revision().saturating_add(1),
                    0,
                )
                .as_uuid();
                instance.quarantine_state = Some(
                    "legacy pending invocation has no canonical effect command mapping".to_string(),
                );
                instance.state = ProcessState::Failed { incident_id };
            }
            OutcomeDisposition::Retry => {
                self.store
                    .release_instance_transition(&row.tenant_id, instance.instance_id, &owner)
                    .await
                    .map_err(|error| {
                        ProcessAdvancerError::Internal(format!("release instance: {error}"))
                    })?;
                return Ok(());
            }
            OutcomeDisposition::Malformed => {
                let _ = self
                    .store
                    .release_instance_transition(&row.tenant_id, instance.instance_id, &owner)
                    .await;
                return Err(ProcessAdvancerError::Malformed(
                    "ExecutionOutcomeKind::OutcomeUnspecified — peer must populate kind".to_owned(),
                ));
            }
            OutcomeDisposition::Terminal => {
                instance.state = ProcessState::Failed {
                    incident_id: bpmn_lite_types::EffectId::for_transition(
                        instance.instance_id,
                        claim.expected_revision().saturating_add(1),
                        0,
                    )
                    .as_uuid(),
                };
            }
        }

        let ops = vec![
            TickOperation::TakePendingInvocation {
                execution_id: input.execution_id,
            },
            TickOperation::SaveInstance {
                instance: instance.clone(),
            },
        ];

        let transition = transition_from_tick_ops(&instance, &ops);
        let commit_res = self.store.commit_transition(&claim, &transition).await;

        let release_res = self
            .store
            .release_instance_transition(&row.tenant_id, instance.instance_id, &owner)
            .await;

        match commit_res {
            Err(e) => {
                return Err(ProcessAdvancerError::Internal(format!(
                    "commit transition: {e}"
                )));
            }
            Ok(_) => {
                if let Err(e) = release_res {
                    return Err(ProcessAdvancerError::Internal(format!(
                        "release instance: {e}"
                    )));
                }
            }
        }

        tracing::info!(
            execution_id = %input.execution_id,
            callout_id = %row.callout_id,
            process_instance_id = %row.process_instance_id,
            node_id = %row.node_id,
            source_domain = %input.source_domain,
            outcome = ?input.outcome_kind,
            "legacy bus result quarantined; canonical DSL execution uses kernel jobs/effects"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_and_timeout_are_retry_only() {
        assert_eq!(
            classify_outcome(ExecutionOutcomeKind::OptimisticConflict),
            OutcomeDisposition::Retry
        );
        assert_eq!(
            classify_outcome(ExecutionOutcomeKind::LockTimeout),
            OutcomeDisposition::Retry
        );
    }

    #[test]
    fn only_committed_outcomes_bind_outputs() {
        assert_eq!(
            classify_outcome(ExecutionOutcomeKind::Committed),
            OutcomeDisposition::Commit
        );
        assert_eq!(
            classify_outcome(ExecutionOutcomeKind::IdempotentReplayReturned),
            OutcomeDisposition::Commit
        );
        assert_eq!(
            classify_outcome(ExecutionOutcomeKind::AuthorityDenied),
            OutcomeDisposition::Terminal
        );
        assert_eq!(
            classify_outcome(ExecutionOutcomeKind::OutcomeUnspecified),
            OutcomeDisposition::Malformed
        );
    }
}
