//! dmn-lite receiver-side bus handler.
//!
//! Implements [`dsl_bus_server::InvocationDispatcher`] for the dmn-lite
//! domain. The actual decision evaluation is delegated to a
//! caller-supplied [`DecisionEvaluator`] port; T2B.9 wires the real
//! dmn-lite engine + FFI catalogue against this trait at app startup.
//!
//! dmn-lite is a pure-decision domain — it never receives results, so
//! [`NoopResultDispatcher`] returns `BusServerError::UnknownVerb` for
//! every DeliverResult call.

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

/// Successful evaluation of a dmn-lite decision. `bindings` carries
/// the decision output rows as `name → value` pairs.
#[derive(Debug, Clone)]
pub struct DecisionOutcome {
    pub execution_id: Uuid,
    pub kind: ExecutionOutcomeKind,
    pub detail: String,
    pub bindings: Vec<ResolvedBinding>,
}

#[derive(Debug, Error)]
pub enum DecisionEvaluatorError {
    #[error("unknown decision: {0}")]
    UnknownDecision(String),

    #[error("catalogue version incompatible: {0}")]
    VersionIncompatible(String),

    #[error("authority denied: {0}")]
    AuthorityDenied(String),

    #[error("malformed input: {0}")]
    Malformed(String),

    #[error("internal evaluator error: {0}")]
    Internal(String),
}

impl From<DecisionEvaluatorError> for BusServerError {
    fn from(e: DecisionEvaluatorError) -> Self {
        match e {
            DecisionEvaluatorError::UnknownDecision(s) => BusServerError::UnknownVerb(s),
            DecisionEvaluatorError::VersionIncompatible(s) => {
                BusServerError::VersionIncompatible(s)
            }
            DecisionEvaluatorError::AuthorityDenied(s) => BusServerError::AuthorityDenied(s),
            DecisionEvaluatorError::Malformed(s) => BusServerError::Malformed(s),
            DecisionEvaluatorError::Internal(s) => BusServerError::Internal(s),
        }
    }
}

/// Port the app supplies — T2B.9 wires this to the real dmn-lite
/// `VerifiedDecision`-driven evaluator.
#[async_trait]
pub trait DecisionEvaluator: Send + Sync + 'static {
    async fn evaluate(
        &self,
        local_decision_id: &str,
        catalogue_version: &str,
        inputs: Vec<ResolvedBinding>,
    ) -> Result<DecisionOutcome, DecisionEvaluatorError>;
}

/// `InvocationDispatcher` implementation for dmn-lite. Holds an `Arc`
/// over the caller's [`DecisionEvaluator`] so the same instance can be
/// shared across the tonic server's worker threads.
pub struct DmnLiteBusHandler {
    evaluator: Arc<dyn DecisionEvaluator>,
    /// T2B master DoD #46 — when set, mismatched
    /// `catalogue_version` rejects with `VersionIncompatible`.
    expected_catalogue_version: Option<String>,
}

impl DmnLiteBusHandler {
    pub fn new<E: DecisionEvaluator>(evaluator: E) -> Self {
        Self {
            evaluator: Arc::new(evaluator),
            expected_catalogue_version: None,
        }
    }

    pub fn from_arc(evaluator: Arc<dyn DecisionEvaluator>) -> Self {
        Self {
            evaluator,
            expected_catalogue_version: None,
        }
    }

    pub fn with_catalogue_version(mut self, version: impl Into<String>) -> Self {
        self.expected_catalogue_version = Some(version.into());
        self
    }
}

#[async_trait]
impl InvocationDispatcher for DmnLiteBusHandler {
    async fn dispatch(
        &self,
        ctx: InvocationContext,
        inputs: Vec<ResolvedBinding>,
    ) -> Result<InvocationOutcome, BusServerError> {
        if let Some(ref expected) = self.expected_catalogue_version
            && &ctx.catalogue_version != expected
        {
            return Err(BusServerError::VersionIncompatible(format!(
                "dmn-lite expects catalogue_version {expected}, got {}",
                ctx.catalogue_version
            )));
        }
        let outcome = self
            .evaluator
            .evaluate(&ctx.local_verb_id, &ctx.catalogue_version, inputs)
            .await?;
        Ok(InvocationOutcome {
            execution_id: outcome.execution_id,
            outcome: ExecutionOutcome {
                kind: outcome.kind as i32,
                detail: outcome.detail,
                bindings: outcome.bindings,
            },
        })
    }
}

/// `ResultDispatcher` for dmn-lite — never invoked in the demo flow,
/// but the bus server requires both dispatcher slots to be populated.
pub struct NoopResultDispatcher;

#[async_trait]
impl ResultDispatcher for NoopResultDispatcher {
    async fn dispatch(
        &self,
        _ctx: ResultContext,
        _outcome: ExecutionOutcome,
    ) -> Result<(), BusServerError> {
        Err(BusServerError::UnknownVerb(
            "dmn-lite does not receive bus results".into(),
        ))
    }
}

#[cfg(test)]
mod tests;
