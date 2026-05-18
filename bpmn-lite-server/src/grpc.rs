use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::Semaphore;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use bpmn_lite_analysis;
use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_ffi_grpc::GrpcFfiOwner;
use bpmn_lite_ffi_http::{HttpFfiOwner, HttpIdempotency, HttpMethod};
use bpmn_lite_types::{ErrorClass, Value};
use dmn_lite_bridge::DmnLiteOwner;
use dmn_lite_compiler::{compile_and_verify, load_catalogue_from_str};
use dmn_lite_parser::parse;
use ffi_catalogue::{FfiCatalogue, FfiTemplateStore};
use ffi_types::{FieldSchema, Idempotency, SchemaKind};

use crate::event_fanout::EventFanout;

#[allow(clippy::enum_variant_names)]
pub mod proto {
    tonic::include_proto!("bpmn_lite.v1");
}

use proto::bpmn_lite_server::BpmnLite;
use proto::*;

#[derive(Clone, Debug)]
pub struct RequestLimits {
    pub max_bpmn_xml_bytes: usize,
    pub max_payload_bytes: usize,
    pub max_session_stack_bytes: usize,
    pub max_orch_flags: usize,
    pub max_string_bytes: usize,
    pub max_task_types: usize,
    pub max_activate_jobs: usize,
    pub max_event_subscriptions: usize,
    pub max_subscription_secs: u64,
}

impl Default for RequestLimits {
    fn default() -> Self {
        Self {
            max_bpmn_xml_bytes: 1_000_000,
            max_payload_bytes: 1_000_000,
            max_session_stack_bytes: 256_000,
            max_orch_flags: 128,
            max_string_bytes: 512,
            max_task_types: 128,
            max_activate_jobs: 100,
            max_event_subscriptions: 256,
            max_subscription_secs: 300,
        }
    }
}

impl RequestLimits {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            max_bpmn_xml_bytes: read_usize_env(
                "BPMN_LITE_MAX_BPMN_XML_BYTES",
                defaults.max_bpmn_xml_bytes,
            ),
            max_payload_bytes: read_usize_env(
                "BPMN_LITE_MAX_PAYLOAD_BYTES",
                defaults.max_payload_bytes,
            ),
            max_session_stack_bytes: read_usize_env(
                "BPMN_LITE_MAX_SESSION_STACK_BYTES",
                defaults.max_session_stack_bytes,
            ),
            max_orch_flags: read_usize_env("BPMN_LITE_MAX_ORCH_FLAGS", defaults.max_orch_flags),
            max_string_bytes: read_usize_env(
                "BPMN_LITE_MAX_STRING_BYTES",
                defaults.max_string_bytes,
            ),
            max_task_types: read_usize_env("BPMN_LITE_MAX_TASK_TYPES", defaults.max_task_types),
            max_activate_jobs: read_usize_env(
                "BPMN_LITE_MAX_ACTIVATE_JOBS",
                defaults.max_activate_jobs,
            ),
            max_event_subscriptions: read_usize_env(
                "BPMN_LITE_MAX_EVENT_SUBSCRIPTIONS",
                defaults.max_event_subscriptions,
            ),
            max_subscription_secs: read_u64_env(
                "BPMN_LITE_MAX_SUBSCRIPTION_SECS",
                defaults.max_subscription_secs,
            ),
        }
    }

    #[allow(clippy::result_large_err)]
    fn check_bytes(&self, field: &str, len: usize, max: usize) -> Result<(), Status> {
        if len > max {
            return Err(Status::resource_exhausted(format!(
                "{} is {} bytes; max is {}",
                field, len, max
            )));
        }
        Ok(())
    }

    #[allow(clippy::result_large_err)]
    fn check_string(&self, field: &str, value: &str) -> Result<(), Status> {
        self.check_bytes(field, value.len(), self.max_string_bytes)
    }

    #[allow(clippy::result_large_err)]
    fn check_orch_flags(&self, flags: &HashMap<String, ProtoValue>) -> Result<(), Status> {
        if flags.len() > self.max_orch_flags {
            return Err(Status::resource_exhausted(format!(
                "orch_flags has {} entries; max is {}",
                flags.len(),
                self.max_orch_flags
            )));
        }
        for (key, value) in flags {
            self.check_string("orch_flags key", key)?;
            if let Some(proto_value::Kind::StrValue(s)) = &value.kind {
                self.check_string("orch_flags string value", s)?;
            }
        }
        Ok(())
    }
}

fn read_usize_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn read_u64_env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

#[derive(Default)]
pub struct ServerMetrics {
    requests_total: AtomicU64,
    request_rejections_total: AtomicU64,
    job_activations_total: AtomicU64,
    job_completions_total: AtomicU64,
    job_failures_total: AtomicU64,
    active_subscriptions: AtomicU64,
    subscription_rejections_total: AtomicU64,
}

impl ServerMetrics {
    fn request_started(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    fn request_rejected(&self) {
        self.request_rejections_total
            .fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> MetricsResponse {
        MetricsResponse {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            request_rejections_total: self.request_rejections_total.load(Ordering::Relaxed),
            job_activations_total: self.job_activations_total.load(Ordering::Relaxed),
            job_completions_total: self.job_completions_total.load(Ordering::Relaxed),
            job_failures_total: self.job_failures_total.load(Ordering::Relaxed),
            active_subscriptions: self.active_subscriptions.load(Ordering::Relaxed),
            subscription_rejections_total: self
                .subscription_rejections_total
                .load(Ordering::Relaxed),
        }
    }
}

struct ActiveSubscriptionGuard {
    metrics: Arc<ServerMetrics>,
}

impl ActiveSubscriptionGuard {
    fn new(metrics: Arc<ServerMetrics>) -> Self {
        metrics.active_subscriptions.fetch_add(1, Ordering::Relaxed);
        Self { metrics }
    }
}

impl Drop for ActiveSubscriptionGuard {
    fn drop(&mut self) {
        self.metrics
            .active_subscriptions
            .fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct BpmnLiteService {
    pub engine: Arc<BpmnLiteEngine>,
    pub event_fanout: Arc<EventFanout>,
    pub limits: RequestLimits,
    pub metrics: Arc<ServerMetrics>,
    pub subscription_limiter: Arc<Semaphore>,
    /// In-process dmn-lite FFI execution owner (registers decisions, evaluates them).
    pub ffi_owner: Arc<DmnLiteOwner>,
    /// In-process HTTP FFI execution owner.
    pub http_ffi_owner: Arc<HttpFfiOwner>,
    /// In-process gRPC FFI execution owner.
    pub grpc_ffi_owner: Arc<GrpcFfiOwner>,
    /// FFI template catalogue (shared with FfiDispatcher via Arc).
    pub ffi_catalogue: Arc<FfiCatalogue>,
    /// Backing store for the FFI catalogue; used to publish new templates.
    pub ffi_store: Arc<dyn FfiTemplateStore>,
}

// --- Proto ↔ Core conversions ---

fn value_to_proto(v: &Value) -> ProtoValue {
    match v {
        Value::Bool(b) => ProtoValue {
            kind: Some(proto_value::Kind::BoolValue(*b)),
        },
        Value::I64(n) => ProtoValue {
            kind: Some(proto_value::Kind::I64Value(*n)),
        },
        Value::Str(idx) => ProtoValue {
            kind: Some(proto_value::Kind::StrValue(format!("str_{}", idx))),
        },
        Value::Ref(idx) => ProtoValue {
            kind: Some(proto_value::Kind::StrValue(format!("ref_{}", idx))),
        },
    }
}

fn proto_to_value(pv: &ProtoValue) -> Value {
    match &pv.kind {
        Some(proto_value::Kind::BoolValue(b)) => Value::Bool(*b),
        Some(proto_value::Kind::I64Value(n)) => Value::I64(*n),
        Some(proto_value::Kind::StrValue(_)) => Value::Str(0),
        None => Value::Bool(false),
    }
}

#[allow(clippy::result_large_err)]
fn proto_to_correlation_value(pv: &Option<ProtoValue>) -> Result<Value, Status> {
    match pv.as_ref().and_then(|value| value.kind.as_ref()) {
        Some(proto_value::Kind::BoolValue(b)) => Ok(Value::Bool(*b)),
        Some(proto_value::Kind::I64Value(n)) => Ok(Value::I64(*n)),
        Some(proto_value::Kind::StrValue(s)) => {
            if let Some(rest) = s.strip_prefix("str_") {
                return rest
                    .parse::<u32>()
                    .map(Value::Str)
                    .map_err(|_| Status::invalid_argument("invalid str_ correlation key"));
            }
            if let Some(rest) = s.strip_prefix("ref_") {
                return rest
                    .parse::<u32>()
                    .map(Value::Ref)
                    .map_err(|_| Status::invalid_argument("invalid ref_ correlation key"));
            }
            Err(Status::invalid_argument(
                "string correlation keys must use str_<id> or ref_<id>",
            ))
        }
        None => Ok(Value::Bool(false)),
    }
}

fn proto_to_orch_flags(
    map: &std::collections::HashMap<String, ProtoValue>,
) -> BTreeMap<String, Value> {
    map.iter()
        .map(|(k, v)| (k.clone(), proto_to_value(v)))
        .collect()
}

#[allow(clippy::result_large_err)]
fn parse_uuid(s: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(s).map_err(|e| Status::invalid_argument(format!("Invalid UUID: {}", e)))
}

#[allow(clippy::result_large_err)]
fn parse_bytecode_version(bytes: &[u8]) -> Result<[u8; 32], Status> {
    bytes
        .try_into()
        .map_err(|_| Status::invalid_argument("bytecode_version must be exactly 32 bytes"))
}

#[allow(clippy::result_large_err)]
fn parse_hash(bytes: &[u8]) -> Result<[u8; 32], Status> {
    bytes
        .try_into()
        .map_err(|_| Status::invalid_argument("domain_payload_hash must be exactly 32 bytes"))
}

fn engine_err(e: anyhow::Error) -> Status {
    Status::internal(format!("{:#}", e))
}

#[allow(clippy::result_large_err)]
fn request_tenant_id(limits: &RequestLimits, tenant_id: &str) -> Result<String, Status> {
    limits.check_string("tenant_id", tenant_id)?;
    if tenant_id.is_empty() {
        Ok("default".to_string())
    } else {
        Ok(tenant_id.to_string())
    }
}

/// Extract the instance_id (UUID) from a job_key formatted as "instance_id:service_task_id:pc".
#[allow(clippy::result_large_err)]
fn extract_instance_id_from_job_key(job_key: &str) -> Result<Uuid, Status> {
    let uuid_str = job_key
        .split(':')
        .next()
        .ok_or_else(|| Status::invalid_argument(format!("Invalid job_key: {}", job_key)))?;
    parse_uuid(uuid_str)
}

#[allow(clippy::result_large_err)]
fn proto_fields_to_schema(fields: &[FfiFieldSchemaProto]) -> Result<Vec<FieldSchema>, Status> {
    fields
        .iter()
        .map(|f| {
            let kind = match f.kind.as_str() {
                "bool" => SchemaKind::Bool,
                "i64" => SchemaKind::I64,
                "f64" => SchemaKind::F64,
                "string" => SchemaKind::String,
                other => {
                    return Err(Status::invalid_argument(format!(
                        "unknown field kind '{}'; use bool, i64, f64, or string",
                        other
                    )))
                }
            };
            Ok(FieldSchema {
                name: f.name.clone(),
                kind,
                required: f.required,
            })
        })
        .collect()
}

#[tonic::async_trait]
impl BpmnLite for BpmnLiteService {
    async fn compile(
        &self,
        request: Request<CompileRequest>,
    ) -> Result<Response<CompileResponse>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        self.limits.check_bytes(
            "bpmn_xml",
            req.bpmn_xml.len(),
            self.limits.max_bpmn_xml_bytes,
        )?;
        let result = self
            .engine
            .compile(&req.bpmn_xml)
            .await
            .map_err(|e| Status::invalid_argument(format!("Compilation failed: {:#}", e)))?;

        // Run static analysis on the compiled program and surface findings as diagnostics.
        let analysis_diagnostics = if !req.validate_only {
            // Load the program we just compiled to run analysis.
            // The engine stores it; reload via the engine's store.
            let maybe_program = self
                .engine
                .load_program(result.bytecode_version)
                .await
                .ok()
                .flatten();
            if let Some(program) = maybe_program {
                let report = bpmn_lite_analysis::analyse(&program);
                if report.warning_count() > 0 {
                    tracing::warn!(
                        warnings = report.warning_count(),
                        "static analysis found potential issues in compiled BPMN"
                    );
                }
                report
                    .findings
                    .into_iter()
                    .map(|f| Diagnostic {
                        severity: match f.severity {
                            bpmn_lite_analysis::Severity::Warning => "warning".to_string(),
                            bpmn_lite_analysis::Severity::Info => "info".to_string(),
                        },
                        message: f.message,
                        element_id: f.element_id.unwrap_or_default(),
                    })
                    .collect()
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        let mut diagnostics: Vec<Diagnostic> = result
            .diagnostics
            .into_iter()
            .map(|msg| Diagnostic {
                severity: "info".to_string(),
                message: msg,
                element_id: String::new(),
            })
            .collect();
        diagnostics.extend(analysis_diagnostics);

        Ok(Response::new(CompileResponse {
            bytecode_version: result.bytecode_version.to_vec(),
            diagnostics,
            flag_symbol_table: result.flag_symbol_table.into_iter().collect(),
        }))
    }

    async fn start_process(
        &self,
        request: Request<StartRequest>,
    ) -> Result<Response<StartResponse>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        self.limits.check_string("process_key", &req.process_key)?;
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        self.limits
            .check_string("correlation_id", &req.correlation_id)?;
        self.limits.check_bytes(
            "domain_payload",
            req.domain_payload.len(),
            self.limits.max_payload_bytes,
        )?;
        self.limits.check_bytes(
            "session_stack_json",
            req.session_stack_json.len(),
            self.limits.max_session_stack_bytes,
        )?;
        self.limits.check_orch_flags(&req.orch_flags)?;
        let bytecode_version = parse_bytecode_version(&req.bytecode_version)?;
        let hash = parse_hash(&req.domain_payload_hash)?;
        let actual_hash = bpmn_lite_vm::compute_hash(&req.domain_payload);
        if actual_hash != hash {
            return Err(Status::invalid_argument(
                "domain_payload_hash does not match domain_payload",
            ));
        }
        let session_stack = if req.session_stack_json.is_empty() {
            ob_poc_types::session_stack::SessionStackState::default()
        } else {
            serde_json::from_str(&req.session_stack_json)
                .map_err(|e| Status::invalid_argument(format!("invalid session_stack_json: {e}")))?
        };

        let engine = self.engine.for_tenant(tenant_id);
        let instance_id = engine
            .start_with_params(bpmn_lite_engine::StartParams {
                process_key: req.process_key.clone(),
                bytecode_version,
                domain_payload: req.domain_payload.clone(),
                domain_payload_hash: hash,
                correlation_id: req.correlation_id.clone(),
                session_stack,
                entry_id: parse_uuid(&req.entry_id)?,
                runbook_id: parse_uuid(&req.runbook_id)?,
            })
            .await
            .map_err(engine_err)?;

        // Tick the instance to kick off any initial work (jobs stay in queue for ActivateJobs)
        engine
            .tick_instance(instance_id)
            .await
            .map_err(engine_err)?;

        Ok(Response::new(StartResponse {
            process_instance_id: instance_id.to_string(),
        }))
    }

    async fn signal(
        &self,
        request: Request<SignalRequest>,
    ) -> Result<Response<SignalResponse>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        self.limits
            .check_string("message_name", &req.message_name)?;
        self.limits.check_string("msg_id", &req.msg_id)?;
        self.limits
            .check_bytes("payload", req.payload.len(), self.limits.max_payload_bytes)?;
        if req.msg_id.is_empty() {
            return Err(Status::invalid_argument("msg_id is required"));
        }
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        let corr_key = proto_to_correlation_value(&req.correlation_key)?;
        let instance_id = parse_uuid(&req.process_instance_id)?;

        let payload = if req.payload.is_empty() {
            None
        } else {
            Some(std::str::from_utf8(&req.payload).map_err(|e| {
                Status::invalid_argument(format!("payload must be valid UTF-8: {}", e))
            })?)
        };

        let hash = if req.payload.is_empty() {
            None
        } else {
            Some(bpmn_lite_vm::compute_hash(payload.unwrap_or_default()))
        };

        let engine = self.engine.for_tenant(tenant_id);
        engine
            .signal_with_value(
                instance_id,
                &req.message_name,
                corr_key,
                payload,
                hash,
                Some(req.msg_id.as_str()),
            )
            .await
            .map_err(engine_err)?;

        // Tick instance to advance past the signal (jobs stay in queue)
        engine
            .tick_instance(instance_id)
            .await
            .map_err(engine_err)?;

        Ok(Response::new(SignalResponse {}))
    }

    async fn cancel(
        &self,
        request: Request<CancelRequest>,
    ) -> Result<Response<CancelResponse>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        self.limits
            .check_string("process_instance_id", &req.process_instance_id)?;
        self.limits.check_string("reason", &req.reason)?;
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        let instance_id = parse_uuid(&req.process_instance_id)?;

        self.engine
            .for_tenant(tenant_id)
            .cancel(instance_id, &req.reason)
            .await
            .map_err(engine_err)?;

        Ok(Response::new(CancelResponse {}))
    }

    async fn inspect(
        &self,
        request: Request<InspectRequest>,
    ) -> Result<Response<InspectResponse>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        self.limits
            .check_string("process_instance_id", &req.process_instance_id)?;
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        let instance_id = parse_uuid(&req.process_instance_id)?;

        let inspection = self
            .engine
            .for_tenant(tenant_id)
            .inspect(instance_id)
            .await
            .map_err(engine_err)?;

        let state_str = match &inspection.state {
            bpmn_lite_types::ProcessState::Running => "RUNNING",
            bpmn_lite_types::ProcessState::Completed { .. } => "COMPLETED",
            bpmn_lite_types::ProcessState::Cancelled { .. } => "CANCELLED",
            bpmn_lite_types::ProcessState::Failed { .. } => "FAILED",
            bpmn_lite_types::ProcessState::Terminated { .. } => "TERMINATED",
        };

        let fibers: Vec<FiberInfo> = inspection
            .fibers
            .iter()
            .map(|f| {
                let ws = format!("{:?}", f.wait_state);
                FiberInfo {
                    fiber_id: f.fiber_id.to_string(),
                    pc: f.pc,
                    wait_state: ws,
                }
            })
            .collect();

        let waits: Vec<WaitInfo> = inspection
            .fibers
            .iter()
            .filter(|f| !matches!(f.wait_state, bpmn_lite_types::WaitState::Running))
            .map(|f| {
                let (wt, detail) = match &f.wait_state {
                    bpmn_lite_types::WaitState::Timer { .. } => {
                        ("TIMER".to_string(), String::new())
                    }
                    bpmn_lite_types::WaitState::Msg { .. } => {
                        ("MESSAGE".to_string(), String::new())
                    }
                    bpmn_lite_types::WaitState::Job { job_key } => {
                        ("JOB".to_string(), job_key.clone())
                    }
                    bpmn_lite_types::WaitState::Join { .. } => ("JOIN".to_string(), String::new()),
                    bpmn_lite_types::WaitState::Incident { incident_id } => {
                        ("INCIDENT".to_string(), incident_id.to_string())
                    }
                    bpmn_lite_types::WaitState::Race { race_id, .. } => {
                        ("RACE".to_string(), format!("race_{}", race_id))
                    }
                    bpmn_lite_types::WaitState::Running => unreachable!(),
                };
                WaitInfo {
                    fiber_id: f.fiber_id.to_string(),
                    wait_type: wt,
                    detail,
                }
            })
            .collect();

        Ok(Response::new(InspectResponse {
            state: state_str.to_string(),
            fibers,
            waits,
            bytecode_version: inspection.bytecode_version.to_vec(),
            domain_payload_hash: hex::encode(inspection.domain_payload_hash),
            tenant_id: inspection.tenant_id,
        }))
    }

    type ActivateJobsStream =
        tokio_stream::wrappers::ReceiverStream<Result<JobActivationMsg, Status>>;

    async fn activate_jobs(
        &self,
        request: Request<ActivateJobsRequest>,
    ) -> Result<Response<Self::ActivateJobsStream>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        if req.task_types.is_empty() {
            return Err(Status::invalid_argument("task_types must not be empty"));
        }
        if req.task_types.len() > self.limits.max_task_types {
            return Err(Status::resource_exhausted(format!(
                "task_types has {} entries; max is {}",
                req.task_types.len(),
                self.limits.max_task_types
            )));
        }
        for task_type in &req.task_types {
            self.limits.check_string("task_type", task_type)?;
        }
        self.limits.check_string("worker_id", &req.worker_id)?;
        if req.worker_id.is_empty() {
            return Err(Status::invalid_argument("worker_id is required"));
        }
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        let requested = req.max_jobs.max(1) as usize;
        let max_jobs = requested.min(self.limits.max_activate_jobs);
        let lease_ms = if req.timeout_ms > 0 {
            req.timeout_ms as u64
        } else {
            300_000
        };

        let jobs = self
            .engine
            .for_tenant(tenant_id)
            .activate_jobs_for_worker_with_lease(
                &req.task_types,
                max_jobs,
                &req.worker_id,
                lease_ms,
            )
            .await
            .map_err(engine_err)?;

        let (tx, rx) = tokio::sync::mpsc::channel(jobs.len().max(1));
        self.metrics
            .job_activations_total
            .fetch_add(jobs.len() as u64, Ordering::Relaxed);

        for job in jobs {
            let msg = JobActivationMsg {
                job_key: job.job_key,
                process_instance_id: job.process_instance_id.to_string(),
                task_type: job.task_type,
                service_task_id: job.service_task_id,
                domain_payload: job.domain_payload,
                domain_payload_hash: job.domain_payload_hash.to_vec(),
                session_stack_json: serde_json::to_string(&job.session_stack).map_err(|e| {
                    Status::internal(format!("failed to serialize job session_stack: {e}"))
                })?,
                orch_flags: job
                    .orch_flags
                    .iter()
                    .map(|(k, v)| (k.clone(), value_to_proto(v)))
                    .collect(),
                retries_remaining: job.retries_remaining as i32,
                entry_id: job.entry_id.to_string(),
                runbook_id: job.runbook_id.to_string(),
                worker_id: job.worker_id,
                claim_token: job.claim_token,
                tenant_id: job.tenant_id,
            };
            let _ = tx.send(Ok(msg)).await;
        }

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
    }

    async fn complete_job(
        &self,
        request: Request<CompleteJobRequest>,
    ) -> Result<Response<CompleteJobResponse>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        self.limits.check_string("job_key", &req.job_key)?;
        self.limits.check_string("worker_id", &req.worker_id)?;
        self.limits.check_string("claim_token", &req.claim_token)?;
        if req.worker_id.is_empty() || req.claim_token.is_empty() {
            return Err(Status::invalid_argument(
                "worker_id and claim_token are required",
            ));
        }
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        self.limits.check_bytes(
            "domain_payload",
            req.domain_payload.len(),
            self.limits.max_payload_bytes,
        )?;
        self.limits.check_orch_flags(&req.orch_flags)?;
        let hash = parse_hash(&req.domain_payload_hash)?;
        let orch_flags = proto_to_orch_flags(&req.orch_flags);

        // Extract instance_id from job_key before completing
        let instance_id = extract_instance_id_from_job_key(&req.job_key)?;

        let engine = self.engine.for_tenant(tenant_id);
        engine
            .complete_job_with_claim(
                &req.job_key,
                &req.domain_payload,
                hash,
                orch_flags,
                &req.worker_id,
                &req.claim_token,
            )
            .await
            .map_err(engine_err)?;
        self.metrics
            .job_completions_total
            .fetch_add(1, Ordering::Relaxed);

        // Tick the instance so the resumed fiber advances (may hit End or next ExecNative)
        engine
            .tick_instance(instance_id)
            .await
            .map_err(engine_err)?;

        Ok(Response::new(CompleteJobResponse {}))
    }

    async fn fail_job(
        &self,
        request: Request<FailJobRequest>,
    ) -> Result<Response<FailJobResponse>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        self.limits.check_string("job_key", &req.job_key)?;
        self.limits.check_string("worker_id", &req.worker_id)?;
        self.limits.check_string("claim_token", &req.claim_token)?;
        if req.worker_id.is_empty() || req.claim_token.is_empty() {
            return Err(Status::invalid_argument(
                "worker_id and claim_token are required",
            ));
        }
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        self.limits.check_string("error_class", &req.error_class)?;
        self.limits.check_string("message", &req.message)?;

        let error_class = match req.error_class.as_str() {
            "TRANSIENT" => ErrorClass::Transient,
            "CONTRACT_VIOLATION" => ErrorClass::ContractViolation,
            _ => ErrorClass::BusinessRejection {
                rejection_code: req.error_class.clone(),
            },
        };

        self.engine
            .for_tenant(tenant_id)
            .fail_job_with_claim(
                &req.job_key,
                error_class,
                &req.message,
                &req.worker_id,
                &req.claim_token,
            )
            .await
            .map_err(engine_err)?;
        self.metrics
            .job_failures_total
            .fetch_add(1, Ordering::Relaxed);

        Ok(Response::new(FailJobResponse {}))
    }

    type SubscribeEventsStream =
        tokio_stream::wrappers::ReceiverStream<Result<LifecycleEvent, Status>>;

    async fn subscribe_events(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        self.metrics.request_started();

        let req = request.into_inner();
        self.limits
            .check_string("process_instance_id", &req.process_instance_id)?;
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        let instance_id = parse_uuid(&req.process_instance_id)?;

        // Verify the instance exists before starting the tail.
        self.engine
            .for_tenant(tenant_id)
            .read_events(instance_id, 0)
            .await
            .map_err(engine_err)?;

        let permit = self
            .subscription_limiter
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                self.metrics.request_rejected();
                self.metrics
                    .subscription_rejections_total
                    .fetch_add(1, Ordering::Relaxed);
                Status::resource_exhausted("too many active event subscriptions")
            })?;

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let event_fanout = self.event_fanout.clone();
        let metrics = self.metrics.clone();
        let max_subscription_secs = self.limits.max_subscription_secs;

        // Own the limiter permit for the lifetime of the returned stream.
        tokio::spawn(async move {
            let _permit = permit;
            let _active_subscription = ActiveSubscriptionGuard::new(metrics);
            let started_at = std::time::Instant::now();
            let mut fanout_rx = match event_fanout.subscribe(instance_id).await {
                Ok(rx) => rx,
                Err(status) => {
                    let _ = tx.send(Err(status)).await;
                    return;
                }
            };

            loop {
                if started_at.elapsed() > std::time::Duration::from_secs(max_subscription_secs) {
                    break;
                }
                let remaining = std::time::Duration::from_secs(max_subscription_secs)
                    .saturating_sub(started_at.elapsed());
                let next = tokio::time::timeout(remaining, fanout_rx.recv()).await;
                match next {
                    Ok(Some(item)) => {
                        if tx.send(item).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) | Err(_) => break,
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        self.metrics.request_started();
        match self.engine.health_check().await {
            Ok(()) => Ok(Response::new(HealthResponse {
                ready: true,
                status: "ok".to_string(),
            })),
            Err(e) => Ok(Response::new(HealthResponse {
                ready: false,
                status: e.to_string(),
            })),
        }
    }

    async fn metrics(
        &self,
        _request: Request<MetricsRequest>,
    ) -> Result<Response<MetricsResponse>, Status> {
        self.metrics.request_started();
        Ok(Response::new(self.metrics.snapshot()))
    }

    async fn register_dmn_decision(
        &self,
        request: Request<RegisterDmnDecisionRequest>,
    ) -> Result<Response<RegisterDmnDecisionResponse>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        let publisher = if req.publisher.is_empty() {
            "server".to_string()
        } else {
            req.publisher.clone()
        };

        // Parse catalogue (empty TOML = minimal catalogue with no domains).
        let catalogue_toml = if req.catalogue_toml.is_empty() {
            concat!(
                "snapshot_id = \"00000000-0000-7000-8000-000000000000\"\n",
                "snapshot_version = \"v1\"\n",
                "created_at = \"2026-01-01T00:00:00Z\"\n",
            )
            .to_string()
        } else {
            req.catalogue_toml.clone()
        };
        let catalogue = load_catalogue_from_str(&catalogue_toml)
            .map_err(|e| Status::invalid_argument(format!("catalogue_toml invalid: {e}")))?;

        // Parse and compile the DMN source.
        let ast = parse(&req.dmn_source)
            .map_err(|e| Status::invalid_argument(format!("dmn_source parse error: {e}")))?;
        let decision = compile_and_verify(ast, &catalogue, &req.dmn_source)
            .map_err(|e| Status::invalid_argument(format!("dmn_source compile error: {e}")))?;

        // Convert proto field schemas.
        let input_schema = proto_fields_to_schema(&req.input_fields)?;
        let output_schema = proto_fields_to_schema(&req.output_fields)?;

        // Register with the DmnLiteOwner and publish the template.
        let template = self.ffi_owner.register_decision(
            decision,
            input_schema,
            output_schema,
            Idempotency::Idempotent,
            tenant_id.clone(),
            publisher,
        );

        let template_id_hex: String = template
            .template_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        self.ffi_store
            .publish(&template)
            .await
            .map_err(|e| Status::internal(format!("publish template: {e}")))?;

        // Refresh cache so the dispatcher sees the new template immediately.
        self.ffi_catalogue
            .load_into_cache(&tenant_id)
            .await
            .map_err(|e| Status::internal(format!("refresh catalogue cache: {e}")))?;

        tracing::info!(template_id = %template_id_hex, %tenant_id, "registered dmn-lite decision as FFI template");
        Ok(Response::new(RegisterDmnDecisionResponse {
            template_id_hex,
        }))
    }

    async fn register_http_template(
        &self,
        request: Request<RegisterHttpTemplateRequest>,
    ) -> Result<Response<RegisterHttpTemplateResponse>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        let publisher = if req.publisher.is_empty() {
            "server".to_string()
        } else {
            req.publisher.clone()
        };

        let method: HttpMethod = match req.method.to_uppercase().as_str() {
            "GET" => HttpMethod::Get,
            "POST" => HttpMethod::Post,
            other => {
                return Err(Status::invalid_argument(format!(
                    "unsupported HTTP method '{}'; use GET or POST",
                    other
                )))
            }
        };

        let idempotency = match req.idempotency.to_lowercase().as_str() {
            "idempotent" => HttpIdempotency::Idempotent,
            "non_idempotent" | "nonidempotent" => HttpIdempotency::NonIdempotent,
            "" => method.default_idempotency(),
            other => {
                return Err(Status::invalid_argument(format!(
                    "unknown idempotency '{}'; use idempotent or non_idempotent",
                    other
                )))
            }
        };

        let timeout_ms = if req.timeout_ms == 0 {
            5000
        } else {
            req.timeout_ms
        };
        let success_codes: Vec<u16> = if req.success_status_codes.is_empty() {
            vec![200]
        } else {
            req.success_status_codes.iter().map(|&c| c as u16).collect()
        };

        let input_schema = proto_fields_to_schema(&req.input_fields)?;
        let output_schema = proto_fields_to_schema(&req.output_fields)?;

        let template = self
            .http_ffi_owner
            .register_template(
                req.url,
                method,
                req.static_headers.into_iter().collect(),
                timeout_ms,
                req.path_params,
                success_codes,
                idempotency,
                input_schema,
                output_schema,
                tenant_id.clone(),
                publisher,
            )
            .map_err(|e| Status::invalid_argument(format!("invalid HTTP template: {:#}", e)))?;

        let template_id_hex: String = template
            .template_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        self.ffi_store
            .publish(&template)
            .await
            .map_err(|e| Status::internal(format!("publish: {e}")))?;
        self.ffi_catalogue
            .load_into_cache(&tenant_id)
            .await
            .map_err(|e| Status::internal(format!("refresh cache: {e}")))?;

        tracing::info!(template_id = %template_id_hex, %tenant_id, "registered HTTP template");
        Ok(Response::new(RegisterHttpTemplateResponse {
            template_id_hex,
        }))
    }

    async fn register_grpc_template(
        &self,
        request: Request<RegisterGrpcTemplateRequest>,
    ) -> Result<Response<RegisterGrpcTemplateResponse>, Status> {
        self.metrics.request_started();
        let req = request.into_inner();
        let tenant_id = request_tenant_id(&self.limits, &req.tenant_id)?;
        let publisher = if req.publisher.is_empty() {
            "server".to_string()
        } else {
            req.publisher.clone()
        };
        let timeout_ms = if req.timeout_ms == 0 {
            5000
        } else {
            req.timeout_ms
        };

        let idempotency = match req.idempotency.to_lowercase().as_str() {
            "idempotent" => Idempotency::Idempotent,
            "non_idempotent" | "nonidempotent" | "" => Idempotency::NonIdempotent,
            other => {
                return Err(Status::invalid_argument(format!(
                    "unknown idempotency '{}'",
                    other
                )))
            }
        };

        let input_schema = proto_fields_to_schema(&req.input_fields)?;
        let output_schema = proto_fields_to_schema(&req.output_fields)?;

        let template = self
            .grpc_ffi_owner
            .register_template(
                req.endpoint,
                timeout_ms,
                input_schema,
                output_schema,
                idempotency,
                tenant_id.clone(),
                publisher,
            )
            .map_err(|e| Status::invalid_argument(format!("invalid gRPC template: {:#}", e)))?;

        let template_id_hex: String = template
            .template_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        self.ffi_store
            .publish(&template)
            .await
            .map_err(|e| Status::internal(format!("{e}")))?;
        self.ffi_catalogue
            .load_into_cache(&tenant_id)
            .await
            .map_err(|e| Status::internal(format!("{e}")))?;

        tracing::info!(template_id = %template_id_hex, %tenant_id, "registered gRPC template");
        Ok(Response::new(RegisterGrpcTemplateResponse {
            template_id_hex,
        }))
    }
}
