//! bpmn-lite receiver-side bus handler.
//!
//! bpmn-lite is the *workflow* domain: it submits invocations to
//! service domains (ob-poc, dmn-lite) and receives results back via
//! `ResultService.DeliverResult`. This crate implements
//! [`dsl_bus_server::ResultDispatcher`] so the bus server, on result
//! receipt, can hand the outcome to the bpmn-lite runtime via a
//! caller-supplied [`ProcessAdvancer`] port. T2B.9 / T3 wires the real
//! runtime against this port.
//!
//! The [`RejectInvocationDispatcher`] fills the bus server's other
//! dispatcher slot: bpmn-lite doesn't accept invocations from peers in
//! the v0.6 demo flow, so every Submit gets rejected as `UnknownVerb`.

#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use dsl_bus_protocol::v1::{ExecutionOutcome, ExecutionOutcomeKind, ResolvedBinding};
use dsl_bus_server::{
    BusServerError, InvocationContext, InvocationDispatcher, InvocationOutcome, ResultContext,
    ResultDispatcher,
};
use thiserror::Error;
use uuid::Uuid;

/// Information the bpmn-lite runtime needs to advance one process
/// instance: the receiver-supplied `execution_id` (which the caller
/// recorded in `bpmn_pending_invocation` per v0.6 §8.3), the outcome
/// taxonomy + detail, and the resolved bindings the verb produced.
#[derive(Debug, Clone)]
pub struct ProcessAdvanceInput {
    pub idempotency_key: Uuid,
    pub execution_id: Uuid,
    pub source_domain: String,
    pub outcome_kind: ExecutionOutcomeKind,
    pub outcome_detail: String,
    pub bindings: Vec<ResolvedBinding>,
    pub audit_reference: String,
}

#[derive(Debug, Error)]
pub enum ProcessAdvancerError {
    /// No pending-call row matched the `execution_id`. This is the
    /// `ReceiptStatus::RejectedUnknownExecution` path.
    #[error("no pending invocation matches execution_id {0}")]
    UnknownExecution(Uuid),

    #[error("malformed result payload: {0}")]
    Malformed(String),

    #[error("internal runtime error: {0}")]
    Internal(String),
}

impl From<ProcessAdvancerError> for BusServerError {
    fn from(e: ProcessAdvancerError) -> Self {
        match e {
            // No dedicated BusServerError variant for unknown-execution
            // — surface it as `Internal` because the bus server only
            // translates UnknownVerb / Version / Authority / Malformed
            // to first-class SubmissionStatus codes. Unknown-execution
            // does have a `ReceiptStatus::RejectedUnknownExecution`
            // but that's emitted earlier (before this dispatcher
            // runs) when idempotency_key is missing.
            ProcessAdvancerError::UnknownExecution(id) => {
                BusServerError::Internal(format!("unknown execution_id {id}"))
            }
            ProcessAdvancerError::Malformed(s) => BusServerError::Malformed(s),
            ProcessAdvancerError::Internal(s) => BusServerError::Internal(s),
        }
    }
}

/// Port the app supplies — T3 wires this to the bpmn-lite runtime's
/// `advance(process_instance_id, outcome)` once the pending-call row
/// is looked up by `execution_id`.
#[async_trait]
pub trait ProcessAdvancer: Send + Sync + 'static {
    async fn advance(&self, input: ProcessAdvanceInput) -> Result<(), ProcessAdvancerError>;
}

/// `ResultDispatcher` implementation for bpmn-lite. Holds an `Arc` over
/// the caller's [`ProcessAdvancer`] so the bus server's worker threads
/// share a single advancer.
pub struct BpmnLiteBusHandler {
    advancer: Arc<dyn ProcessAdvancer>,
}

impl BpmnLiteBusHandler {
    pub fn new<A: ProcessAdvancer>(advancer: A) -> Self {
        Self {
            advancer: Arc::new(advancer),
        }
    }

    pub fn from_arc(advancer: Arc<dyn ProcessAdvancer>) -> Self {
        Self { advancer }
    }
}

#[async_trait]
impl ResultDispatcher for BpmnLiteBusHandler {
    async fn dispatch(
        &self,
        ctx: ResultContext,
        outcome: ExecutionOutcome,
    ) -> Result<(), BusServerError> {
        let kind = ExecutionOutcomeKind::try_from(outcome.kind).unwrap_or(
            ExecutionOutcomeKind::OutcomeUnspecified,
        );
        let input = ProcessAdvanceInput {
            idempotency_key: ctx.idempotency_key,
            execution_id: ctx.execution_id,
            source_domain: ctx.source_domain,
            outcome_kind: kind,
            outcome_detail: outcome.detail,
            bindings: outcome.bindings,
            audit_reference: ctx.audit_reference,
        };
        self.advancer.advance(input).await?;
        Ok(())
    }
}

/// `InvocationDispatcher` impl that rejects everything — bpmn-lite is
/// a caller-only domain in the v0.6 demo and doesn't accept Submits.
pub struct RejectInvocationDispatcher;

#[async_trait]
impl InvocationDispatcher for RejectInvocationDispatcher {
    async fn dispatch(
        &self,
        ctx: InvocationContext,
        _inputs: Vec<ResolvedBinding>,
    ) -> Result<InvocationOutcome, BusServerError> {
        Err(BusServerError::UnknownVerb(format!(
            "bpmn-lite does not accept invocations (rejected '{}')",
            ctx.local_verb_id
        )))
    }
}

#[cfg(test)]
mod tests;
