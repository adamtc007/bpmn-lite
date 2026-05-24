//! Dispatcher traits + the tonic service shims that adapt them.
//
// `tonic::Status` is ~176 bytes; tonic itself returns
// `Result<_, Status>` throughout its generated code, so the large-Err
// warning is unavoidable at this layer.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use async_trait::async_trait;
use dsl_bus_protocol::v1::entity_service_server::EntityService;
use dsl_bus_protocol::v1::invocation_service_server::InvocationService;
use dsl_bus_protocol::v1::result_service_server::ResultService;
use dsl_bus_protocol::v1::sem_os_service_server::SemOsService;
use dsl_bus_protocol::v1::{
    DagPackOutcome, DagPackRequest, DagPackResponse, EntityResolution, EntityResolutionRequest,
    EntityResolutionResult, ExecutionOutcome, InvocationRequest, InvocationResult, ReceiptStatus,
    ResolutionOutcome, ResolvedBinding, ResultAck, SubmissionAck, SubmissionStatus,
    ValidationIssue, ValidationOutcome, ValidationResult,
};
use dsl_bus_storage::{
    insert_inbox, insert_outbox, lookup_inbox, BusEndpoint, InboxEntry, OutboxEntry,
};
use prost::Message;
use sqlx::PgPool;
use tonic::{Request, Response, Status};
use tracing::warn;
use uuid::Uuid;

use crate::server::BusServerError;
use crate::uuid_convert::{from_proto_opt, to_proto};

// ── Public dispatcher contracts ──────────────────────────────────────

/// Context for one `InvocationService.Submit` call, after structural
/// validation. The dispatcher receives the verb id both as namespaced
/// (`source_domain` carries the prefix) and as `local_verb_id` (prefix
/// stripped) so domain handlers can reach into their own catalogue
/// without re-parsing.
#[derive(Debug, Clone)]
pub struct InvocationContext {
    pub idempotency_key: Uuid,
    pub source_domain: String,
    pub catalogue_version: String,
    pub local_verb_id: String,
    pub result_callback_endpoint: String,
}

/// Successful dispatch result. `execution_id` is the receiver-domain
/// engine's identifier for this run; it's returned to the caller in
/// the `SubmissionAck` and stored on the outbox row.
#[derive(Debug, Clone)]
pub struct InvocationOutcome {
    pub execution_id: Uuid,
    pub outcome: ExecutionOutcome,
}

/// Consumer-supplied verb runner. Implementations live in per-domain
/// bus-handler crates (T2B.5 / T2B.6).
#[async_trait]
pub trait InvocationDispatcher: Send + Sync + 'static {
    async fn dispatch(
        &self,
        ctx: InvocationContext,
        inputs: Vec<ResolvedBinding>,
    ) -> Result<InvocationOutcome, BusServerError>;
}

#[derive(Debug, Clone)]
pub struct ResultContext {
    pub idempotency_key: Uuid,
    pub execution_id: Uuid,
    pub source_domain: String,
    pub audit_reference: String,
}

/// Consumer-supplied callback for results bound for *this* domain.
/// bpmn-lite implements this to advance a process instance; pure
/// "service" domains (ob-poc, dmn-lite) typically register a no-op.
#[async_trait]
pub trait ResultDispatcher: Send + Sync + 'static {
    async fn dispatch(
        &self,
        ctx: ResultContext,
        outcome: ExecutionOutcome,
    ) -> Result<(), BusServerError>;
}

// ── tonic adapters (pub(crate)) ──────────────────────────────────────

pub(crate) struct InvocationServiceImpl {
    pub pool: PgPool,
    pub dispatcher: Arc<dyn InvocationDispatcher>,
    pub local_domain: Arc<String>,
    /// Rung after `tx.commit()` so the local outbox sender drains the
    /// freshly-enqueued result row immediately (A2 §2).
    pub outbox_notifier: dsl_bus_client::OutboxNotifier,
}

#[tonic::async_trait]
impl InvocationService for InvocationServiceImpl {
    async fn submit(
        &self,
        req: Request<InvocationRequest>,
    ) -> Result<Response<SubmissionAck>, Status> {
        let req = req.into_inner();

        let key = match from_proto_opt(&req.idempotency_key) {
            Ok(Some(k)) => k,
            Ok(None) => {
                return reject(SubmissionStatus::RejectedMalformed, "idempotency_key missing");
            }
            Err(err) => {
                return reject(SubmissionStatus::RejectedMalformed, &err.to_string());
            }
        };

        // Idempotent receive — replay the cached execution_id.
        match lookup_inbox(&self.pool, key).await {
            Ok(Some(existing)) => {
                return Ok(Response::new(SubmissionAck {
                    execution_id: existing.execution_id.map(to_proto),
                    status: SubmissionStatus::Duplicate as i32,
                    detail: "idempotency_key already received".into(),
                }));
            }
            Ok(None) => {}
            Err(err) => return internal_status(err),
        }

        let local_verb_id = strip_domain_prefix(&req.verb_id);
        let ctx = InvocationContext {
            idempotency_key: key,
            source_domain: req.source_domain.clone(),
            catalogue_version: req.catalogue_version.clone(),
            local_verb_id: local_verb_id.to_owned(),
            result_callback_endpoint: req.result_callback_endpoint.clone(),
        };

        // `req.encode_to_vec()` is needed later for the inbox payload, so
        // clone the inputs into the dispatcher call instead of consuming.
        let dispatch_result = self.dispatcher.dispatch(ctx, req.inputs.clone()).await;
        let outcome = match dispatch_result {
            Ok(o) => o,
            Err(BusServerError::UnknownVerb(detail)) => {
                return reject(SubmissionStatus::RejectedVerbUnknown, &detail);
            }
            Err(BusServerError::VersionIncompatible(detail)) => {
                return reject(SubmissionStatus::RejectedVersionIncompatible, &detail);
            }
            Err(BusServerError::AuthorityDenied(detail)) => {
                return reject(SubmissionStatus::RejectedAuthority, &detail);
            }
            Err(BusServerError::Malformed(detail)) => {
                return reject(SubmissionStatus::RejectedMalformed, &detail);
            }
            Err(other) => return internal_status(other),
        };

        // Atomically record receipt + enqueue the result delivery.
        let mut tx = match self.pool.begin().await {
            Ok(t) => t,
            Err(err) => return internal_status(err),
        };

        let inbox_entry = InboxEntry::new_received(
            key,
            req.source_domain.clone(),
            BusEndpoint::Invocation,
            Some(outcome.execution_id),
            Some(req.encode_to_vec()),
        );
        if let Err(err) = insert_inbox(&mut *tx, &inbox_entry).await {
            return internal_status(err);
        }

        let result_payload = build_invocation_result(
            key,
            outcome.execution_id,
            &outcome.outcome,
            self.local_domain.as_str(),
        );
        let result_entry = OutboxEntry::new_pending(
            Uuid::now_v7(),
            req.source_domain.clone(),
            BusEndpoint::Result,
            result_payload.encode_to_vec(),
            key,
        );
        if let Err(err) = insert_outbox(&mut *tx, &result_entry).await {
            return internal_status(err);
        }

        if let Err(err) = tx.commit().await {
            return internal_status(err);
        }
        // A2 §2: wake the local sender so the result row drains
        // immediately instead of waiting for the fallback timer.
        self.outbox_notifier.notify();

        Ok(Response::new(SubmissionAck {
            execution_id: Some(to_proto(outcome.execution_id)),
            status: SubmissionStatus::Accepted as i32,
            detail: String::new(),
        }))
    }

    // A3 §2.1 — Validate stub. Returns NOT_IMPLEMENTED unconditionally
    // in v0.6 per the §6 stub discipline ("no conditional dispatching
    // based on 'is this stubbed?'"). The wire shape is the contract;
    // real validation lands in V&S v0.4.
    async fn validate(
        &self,
        _request: Request<InvocationRequest>,
    ) -> Result<Response<ValidationResult>, Status> {
        Ok(Response::new(ValidationResult {
            outcome: ValidationOutcome::NotImplemented as i32,
            issues: vec![ValidationIssue {
                field_name: String::new(),
                issue_kind: "not_implemented".into(),
                detail: "InvocationService.Validate is stubbed in v0.6 — full \
                         implementation lands in engine V&S v0.4 per addendum \
                         v0.6-A3 §4."
                    .into(),
                specific: None,
            }],
            validation_id: Some(to_proto(Uuid::now_v7())),
        }))
    }
}

pub(crate) struct ResultServiceImpl {
    pub pool: PgPool,
    pub dispatcher: Arc<dyn ResultDispatcher>,
}

#[tonic::async_trait]
impl ResultService for ResultServiceImpl {
    async fn deliver_result(
        &self,
        req: Request<InvocationResult>,
    ) -> Result<Response<ResultAck>, Status> {
        let req = req.into_inner();

        let key = match from_proto_opt(&req.idempotency_key) {
            Ok(Some(k)) => k,
            Ok(None) => {
                return Ok(reject_result(
                    ReceiptStatus::RejectedUnknownExecution,
                    "idempotency_key missing",
                ));
            }
            Err(err) => {
                return Ok(reject_result(
                    ReceiptStatus::RejectedUnknownExecution,
                    &err.to_string(),
                ));
            }
        };

        match lookup_inbox(&self.pool, key).await {
            Ok(Some(_)) => {
                return Ok(Response::new(ResultAck {
                    status: ReceiptStatus::DuplicateIgnored as i32,
                    detail: "idempotency_key already processed".into(),
                }));
            }
            Ok(None) => {}
            Err(err) => return internal_status(err),
        }

        let execution_id = match from_proto_opt(&req.execution_id) {
            Ok(Some(id)) => id,
            _ => {
                return Ok(reject_result(
                    ReceiptStatus::RejectedUnknownExecution,
                    "execution_id missing",
                ));
            }
        };

        let outcome = req.outcome.clone().unwrap_or_else(|| ExecutionOutcome {
            kind: dsl_bus_protocol::v1::ExecutionOutcomeKind::OutcomeUnspecified as i32,
            detail: "no outcome provided".into(),
            bindings: vec![],
        });

        let ctx = ResultContext {
            idempotency_key: key,
            execution_id,
            source_domain: req.source_domain.clone(),
            audit_reference: req.audit_reference.clone(),
        };

        if let Err(err) = self.dispatcher.dispatch(ctx, outcome).await {
            warn!(error = %err, "result dispatcher failed");
            return internal_status(err);
        }

        let inbox_entry = InboxEntry::new_received(
            key,
            req.source_domain.clone(),
            BusEndpoint::Result,
            Some(execution_id),
            Some(req.encode_to_vec()),
        );
        if let Err(err) = insert_inbox(&self.pool, &inbox_entry).await {
            return internal_status(err);
        }

        Ok(Response::new(ResultAck {
            status: ReceiptStatus::Received as i32,
            detail: String::new(),
        }))
    }
}

// ── helpers ──────────────────────────────────────────────────────────

pub(crate) fn strip_domain_prefix(verb_id: &str) -> &str {
    verb_id.split_once(':').map(|(_, rest)| rest).unwrap_or(verb_id)
}

fn build_invocation_result(
    idempotency_key: Uuid,
    execution_id: Uuid,
    outcome: &ExecutionOutcome,
    source_domain: &str,
) -> InvocationResult {
    InvocationResult {
        execution_id: Some(to_proto(execution_id)),
        idempotency_key: Some(to_proto(idempotency_key)),
        outcome: Some(outcome.clone()),
        source_domain: source_domain.to_owned(),
        executed_at: None,
        plan_id: None,
        audit_reference: String::new(),
    }
}

fn reject(status: SubmissionStatus, detail: &str) -> Result<Response<SubmissionAck>, Status> {
    Ok(Response::new(SubmissionAck {
        execution_id: None,
        status: status as i32,
        detail: detail.to_owned(),
    }))
}

fn reject_result(status: ReceiptStatus, detail: &str) -> Response<ResultAck> {
    Response::new(ResultAck {
        status: status as i32,
        detail: detail.to_owned(),
    })
}

fn internal_status<R, E: std::fmt::Display>(err: E) -> Result<R, Status> {
    Err(Status::internal(err.to_string()))
}


// ── A3 §2.2 — EntityService stub ─────────────────────────────────────
//
// Wire-only implementation: always returns `RESOLUTION_NOT_IMPLEMENTED`.
// Per A3 §6 discipline #1 + #6, this is the *only* impl in v0.6 — no
// conditional "real vs stub" pathways. Bus-handler crates re-export
// this type under domain-specific names (e.g.
// `ob_poc_bus_handler::ObPocEntityServiceImpl`) so the manifest's
// declared service kind matches the registered impl.
pub struct StubEntityService;

#[tonic::async_trait]
impl EntityService for StubEntityService {
    async fn resolve(
        &self,
        _request: Request<EntityResolutionRequest>,
    ) -> Result<Response<EntityResolutionResult>, Status> {
        Ok(Response::new(EntityResolutionResult {
            resolutions: vec![EntityResolution {
                outcome: ResolutionOutcome::ResolutionNotImplemented as i32,
                resolved_uuid: Vec::new(),
                entity_type: String::new(),
                current_state: String::new(),
                audit_reference: String::new(),
                detail: "EntityService.Resolve is stubbed in v0.6 — full \
                         implementation lands in engine V&S v0.4 per addendum \
                         v0.6-A3 §4."
                    .into(),
            }],
            resolution_id: Some(to_proto(Uuid::now_v7())),
        }))
    }
}

// ── A3 §2.3 — SemOsService stub ──────────────────────────────────────
pub struct StubSemOsService;

#[tonic::async_trait]
impl SemOsService for StubSemOsService {
    async fn fetch_dag_packs(
        &self,
        _request: Request<DagPackRequest>,
    ) -> Result<Response<DagPackResponse>, Status> {
        Ok(Response::new(DagPackResponse {
            packs: vec![dsl_bus_protocol::v1::DagPack {
                pack_id: String::new(),
                domain: String::new(),
                version: String::new(),
                semantic_graph: Vec::new(),
                constellation_maps: Vec::new(),
                derivation_chains: Vec::new(),
                fsm_applicability: None,
                detail: "SemOsService.FetchDagPacks is stubbed in v0.6 — full \
                         implementation lands in engine V&S v0.4 per addendum \
                         v0.6-A3 §4."
                    .into(),
                outcome: DagPackOutcome::DagPackNotImplemented as i32,
            }],
            response_id: Some(to_proto(Uuid::now_v7())),
        }))
    }
}
