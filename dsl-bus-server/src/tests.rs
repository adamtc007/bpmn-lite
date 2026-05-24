//! End-to-end coverage for [`BusServer`].
//!
//! Each test stands up a real tonic gRPC server on an ephemeral port,
//! connects with the protocol's generated client, and verifies that
//! the v0.6 §8.6 receive flow (idempotency, atomic inbox + outbox,
//! status mapping) holds.

use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use dsl_bus_protocol::v1::invocation_service_client::InvocationServiceClient;
use dsl_bus_protocol::v1::result_service_client::ResultServiceClient;
use dsl_bus_protocol::v1::{
    ExecutionOutcome, ExecutionOutcomeKind, InvocationRequest, InvocationResult, ReceiptStatus,
    ResolvedBinding, SubmissionStatus, Uuid as ProtoUuid,
};
use dsl_bus_storage::{lookup_inbox, BusEndpoint};
use sqlx::PgPool;
use uuid::Uuid;

use crate::services::{
    InvocationContext, InvocationDispatcher, InvocationOutcome, ResultContext, ResultDispatcher,
};
use crate::{BusServer, BusServerError};

const DEFAULT_TEST_DATABASE_URL: &str = "postgresql://localhost/dsl_bus_test";

// ── Test harness ─────────────────────────────────────────────────────

async fn setup_pool() -> PgPool {
    let url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_owned());
    let pool = PgPool::connect(&url).await.expect("connect");
    sqlx::migrate!("../dsl-bus-storage/migrations")
        .run(&pool)
        .await
        .expect("migrations");
    sqlx::query("TRUNCATE outbox").execute(&pool).await.unwrap();
    sqlx::query("TRUNCATE inbox").execute(&pool).await.unwrap();
    pool
}

fn ephemeral_addr() -> SocketAddr {
    // Bind / drop a std listener to learn a free port; tonic will rebind it.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    addr
}

/// A2 §2: every `BusServer` requires an `OutboxNotifier`. Tests don't
/// need a fully-wired `BusClient`, but the only public way to obtain a
/// notifier is via `BusClient::outbox_notifier()`. Build a minimal
/// client just for this purpose.
async fn test_outbox_notifier(pool: PgPool) -> dsl_bus_client::OutboxNotifier {
    let client = dsl_bus_client::BusClient::builder()
        .pool(pool)
        .local_domain("test-notifier")
        .build()
        .await
        .expect("test BusClient");
    client.outbox_notifier()
}

/// Connect with a short retry loop — `BusServer::serve` spawns the tonic
/// listener on a background task, and the first connect can land before
/// it's accepting. Five attempts at 30 ms is more than enough.
async fn connect_invocation_client(
    addr: SocketAddr,
) -> InvocationServiceClient<tonic::transport::Channel> {
    let url = format!("http://{addr}");
    for _ in 0..20 {
        if let Ok(client) = InvocationServiceClient::connect(url.clone()).await {
            return client;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    InvocationServiceClient::connect(url)
        .await
        .expect("connect to bus server")
}

async fn connect_result_client(
    addr: SocketAddr,
) -> ResultServiceClient<tonic::transport::Channel> {
    let url = format!("http://{addr}");
    for _ in 0..20 {
        if let Ok(client) = ResultServiceClient::connect(url.clone()).await {
            return client;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    ResultServiceClient::connect(url)
        .await
        .expect("connect to bus server")
}

fn proto_uuid(u: Uuid) -> ProtoUuid {
    ProtoUuid {
        value: u.as_bytes().to_vec(),
    }
}

fn sample_request(idempotency_key: Uuid, verb_id: &str) -> InvocationRequest {
    InvocationRequest {
        idempotency_key: Some(proto_uuid(idempotency_key)),
        verb_id: verb_id.into(),
        inputs: vec![ResolvedBinding {
            name: "name".into(),
            value: Some(dsl_bus_protocol::v1::TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                    "Allianz".into(),
                )),
                type_name: "String".into(),
            }),
        }],
        authority: None,
        source_domain: "bpmn-lite".into(),
        catalogue_version: "v1.0.0".into(),
        snapshot_pin: None,
        result_callback_endpoint: "http://bpmn-lite/result".into(),
        timeout_at: None,
    }
}

// ── Dispatchers ──────────────────────────────────────────────────────

struct AcceptingInvocationDispatcher {
    calls: Arc<AtomicU32>,
    /// Optional override that lets a test inject typed failures.
    next_error: Arc<Mutex<Option<BusServerError>>>,
    /// Optional override of the catalogue_version expected.
    require_version: Option<String>,
    seen_contexts: Arc<Mutex<Vec<InvocationContext>>>,
}

impl AcceptingInvocationDispatcher {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicU32::new(0)),
            next_error: Arc::new(Mutex::new(None)),
            require_version: None,
            seen_contexts: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl InvocationDispatcher for AcceptingInvocationDispatcher {
    async fn dispatch(
        &self,
        ctx: InvocationContext,
        _inputs: Vec<ResolvedBinding>,
    ) -> Result<InvocationOutcome, BusServerError> {
        if let Some(required) = &self.require_version
            && &ctx.catalogue_version != required
        {
            return Err(BusServerError::VersionIncompatible(format!(
                "expected {required}, got {}",
                ctx.catalogue_version
            )));
        }
        if let Some(err) = self.next_error.lock().unwrap().take() {
            return Err(err);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen_contexts.lock().unwrap().push(ctx);

        Ok(InvocationOutcome {
            execution_id: Uuid::now_v7(),
            outcome: ExecutionOutcome {
                kind: ExecutionOutcomeKind::Committed as i32,
                detail: "ok".into(),
                bindings: vec![],
            },
        })
    }
}

struct RecordingResultDispatcher {
    calls: Arc<AtomicU32>,
}

impl RecordingResultDispatcher {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicU32::new(0)),
        }
    }
}

#[async_trait]
impl ResultDispatcher for RecordingResultDispatcher {
    async fn dispatch(
        &self,
        _ctx: ResultContext,
        _outcome: ExecutionOutcome,
    ) -> Result<(), BusServerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn invocation_round_trip_inserts_inbox_and_enqueues_result_outbox() {
    let pool = setup_pool().await;
    let inv = AcceptingInvocationDispatcher::new();
    let inv_calls = inv.calls.clone();
    let seen = inv.seen_contexts.clone();
    let res = RecordingResultDispatcher::new();

    let server = BusServer::builder()
        .pool(pool.clone())
        .local_domain("ob-poc")
        .invocation_dispatcher(inv)
        .result_dispatcher(res)
        .outbox_notifier(test_outbox_notifier(pool.clone()).await)
        .bind(ephemeral_addr())
        .build();
    let handle = server.serve().await.unwrap();

    let mut client = connect_invocation_client(handle.local_addr()).await;

    let key = Uuid::now_v7();
    let resp = client
        .submit(sample_request(key, "ob-poc:cbu.create"))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(resp.status, SubmissionStatus::Accepted as i32);
    assert!(resp.execution_id.is_some());
    assert_eq!(inv_calls.load(Ordering::SeqCst), 1);

    // Dispatcher saw the stripped verb id and the carried source domain.
    let ctx = seen.lock().unwrap().first().unwrap().clone();
    assert_eq!(ctx.local_verb_id, "cbu.create");
    assert_eq!(ctx.source_domain, "bpmn-lite");

    // Inbox row exists; outbox carries a result for the caller.
    let inbox_row = lookup_inbox(&pool, key).await.unwrap().expect("inbox row");
    assert_eq!(inbox_row.endpoint, BusEndpoint::Invocation);
    assert!(inbox_row.execution_id.is_some());

    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE idempotency_key = $1 AND target_endpoint = 'result'",
    )
    .bind(key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(outbox_count, 1);

    handle.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn duplicate_invocation_returns_cached_execution_id_without_redispatch() {
    let pool = setup_pool().await;
    let inv = AcceptingInvocationDispatcher::new();
    let inv_calls = inv.calls.clone();
    let res = RecordingResultDispatcher::new();

    let server = BusServer::builder()
        .pool(pool.clone())
        .local_domain("ob-poc")
        .invocation_dispatcher(inv)
        .result_dispatcher(res)
        .outbox_notifier(test_outbox_notifier(pool.clone()).await)
        .bind(ephemeral_addr())
        .build();
    let handle = server.serve().await.unwrap();
    let mut client = connect_invocation_client(handle.local_addr()).await;

    let key = Uuid::now_v7();
    let first = client
        .submit(sample_request(key, "ob-poc:cbu.create"))
        .await
        .unwrap()
        .into_inner();
    let second = client
        .submit(sample_request(key, "ob-poc:cbu.create"))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(first.status, SubmissionStatus::Accepted as i32);
    assert_eq!(second.status, SubmissionStatus::Duplicate as i32);
    assert_eq!(first.execution_id, second.execution_id);
    assert_eq!(inv_calls.load(Ordering::SeqCst), 1, "dispatcher must not run twice");

    handle.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn unknown_verb_returns_rejected_verb_unknown() {
    let pool = setup_pool().await;
    let inv = AcceptingInvocationDispatcher::new();
    *inv.next_error.lock().unwrap() = Some(BusServerError::UnknownVerb("no such verb".into()));
    let res = RecordingResultDispatcher::new();

    let server = BusServer::builder()
        .pool(pool.clone())
        .local_domain("ob-poc")
        .invocation_dispatcher(inv)
        .result_dispatcher(res)
        .outbox_notifier(test_outbox_notifier(pool.clone()).await)
        .bind(ephemeral_addr())
        .build();
    let handle = server.serve().await.unwrap();
    let mut client = connect_invocation_client(handle.local_addr()).await;

    let resp = client
        .submit(sample_request(Uuid::now_v7(), "ob-poc:cbu.nope"))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.status, SubmissionStatus::RejectedVerbUnknown as i32);
    assert!(resp.detail.contains("no such verb"));

    handle.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn version_mismatch_returns_rejected_version_incompatible() {
    let pool = setup_pool().await;
    let mut inv = AcceptingInvocationDispatcher::new();
    inv.require_version = Some("v9.9.9".into());
    let res = RecordingResultDispatcher::new();

    let server = BusServer::builder()
        .pool(pool.clone())
        .local_domain("ob-poc")
        .invocation_dispatcher(inv)
        .result_dispatcher(res)
        .outbox_notifier(test_outbox_notifier(pool.clone()).await)
        .bind(ephemeral_addr())
        .build();
    let handle = server.serve().await.unwrap();
    let mut client = connect_invocation_client(handle.local_addr()).await;

    let resp = client
        .submit(sample_request(Uuid::now_v7(), "ob-poc:cbu.create"))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        resp.status,
        SubmissionStatus::RejectedVersionIncompatible as i32
    );

    handle.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn missing_idempotency_key_returns_rejected_malformed() {
    let pool = setup_pool().await;
    let inv = AcceptingInvocationDispatcher::new();
    let res = RecordingResultDispatcher::new();

    let server = BusServer::builder()
        .pool(pool.clone())
        .local_domain("ob-poc")
        .invocation_dispatcher(inv)
        .result_dispatcher(res)
        .outbox_notifier(test_outbox_notifier(pool.clone()).await)
        .bind(ephemeral_addr())
        .build();
    let handle = server.serve().await.unwrap();
    let mut client = connect_invocation_client(handle.local_addr()).await;

    let mut req = sample_request(Uuid::now_v7(), "ob-poc:cbu.create");
    req.idempotency_key = None;

    let resp = client.submit(req).await.unwrap().into_inner();
    assert_eq!(resp.status, SubmissionStatus::RejectedMalformed as i32);

    handle.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn deliver_result_invokes_dispatcher_and_records_inbox() {
    let pool = setup_pool().await;
    let inv = AcceptingInvocationDispatcher::new();
    let res = RecordingResultDispatcher::new();
    let res_calls = res.calls.clone();

    let server = BusServer::builder()
        .pool(pool.clone())
        .local_domain("bpmn-lite")
        .invocation_dispatcher(inv)
        .result_dispatcher(res)
        .outbox_notifier(test_outbox_notifier(pool.clone()).await)
        .bind(ephemeral_addr())
        .build();
    let handle = server.serve().await.unwrap();
    let mut client = connect_result_client(handle.local_addr()).await;

    let key = Uuid::now_v7();
    let exec = Uuid::now_v7();
    let req = InvocationResult {
        execution_id: Some(proto_uuid(exec)),
        idempotency_key: Some(proto_uuid(key)),
        outcome: Some(ExecutionOutcome {
            kind: ExecutionOutcomeKind::Committed as i32,
            detail: "ok".into(),
            bindings: vec![],
        }),
        source_domain: "ob-poc".into(),
        executed_at: None,
        plan_id: None,
        audit_reference: "audit://ob-poc/run".into(),
    };

    let resp = client.deliver_result(req).await.unwrap().into_inner();
    assert_eq!(resp.status, ReceiptStatus::Received as i32);
    assert_eq!(res_calls.load(Ordering::SeqCst), 1);

    let inbox_row = lookup_inbox(&pool, key).await.unwrap().expect("inbox row");
    assert_eq!(inbox_row.endpoint, BusEndpoint::Result);
    assert_eq!(inbox_row.execution_id, Some(exec));

    handle.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn duplicate_deliver_result_returns_duplicate_ignored() {
    let pool = setup_pool().await;
    let inv = AcceptingInvocationDispatcher::new();
    let res = RecordingResultDispatcher::new();
    let res_calls = res.calls.clone();

    let server = BusServer::builder()
        .pool(pool.clone())
        .local_domain("bpmn-lite")
        .invocation_dispatcher(inv)
        .result_dispatcher(res)
        .outbox_notifier(test_outbox_notifier(pool.clone()).await)
        .bind(ephemeral_addr())
        .build();
    let handle = server.serve().await.unwrap();
    let mut client = connect_result_client(handle.local_addr()).await;

    let key = Uuid::now_v7();
    let exec = Uuid::now_v7();
    let make_req = || InvocationResult {
        execution_id: Some(proto_uuid(exec)),
        idempotency_key: Some(proto_uuid(key)),
        outcome: Some(ExecutionOutcome {
            kind: ExecutionOutcomeKind::Committed as i32,
            detail: "ok".into(),
            bindings: vec![],
        }),
        source_domain: "ob-poc".into(),
        executed_at: None,
        plan_id: None,
        audit_reference: String::new(),
    };

    let _ = client.deliver_result(make_req()).await.unwrap();
    let dup = client.deliver_result(make_req()).await.unwrap().into_inner();
    assert_eq!(dup.status, ReceiptStatus::DuplicateIgnored as i32);
    assert_eq!(res_calls.load(Ordering::SeqCst), 1, "dispatcher must not re-run");

    handle.shutdown().await.unwrap();
}

#[test]
fn strip_domain_prefix_unit() {
    use crate::services::strip_domain_prefix;
    assert_eq!(strip_domain_prefix("ob-poc:cbu.create"), "cbu.create");
    assert_eq!(strip_domain_prefix("cbu.create"), "cbu.create");
    assert_eq!(
        strip_domain_prefix("dmn-lite:cbu_type_routing"),
        "cbu_type_routing"
    );
}

// ── A3 §3.7 — Protocol-shape tests for federated-service stubs ──────

use dsl_bus_protocol::v1::entity_service_client::EntityServiceClient;
use dsl_bus_protocol::v1::sem_os_service_client::SemOsServiceClient;
use dsl_bus_protocol::v1::{
    DagPackOutcome, DagPackRequest, EntityQuery, EntityResolutionRequest, ResolutionOutcome,
    ValidationOutcome,
};

async fn connect_entity_client(
    addr: SocketAddr,
) -> EntityServiceClient<tonic::transport::Channel> {
    let url = format!("http://{addr}");
    for _ in 0..20 {
        if let Ok(client) = EntityServiceClient::connect(url.clone()).await {
            return client;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    EntityServiceClient::connect(url)
        .await
        .expect("connect to bus server")
}

async fn connect_sem_os_client(
    addr: SocketAddr,
) -> SemOsServiceClient<tonic::transport::Channel> {
    let url = format!("http://{addr}");
    for _ in 0..20 {
        if let Ok(client) = SemOsServiceClient::connect(url.clone()).await {
            return client;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    SemOsServiceClient::connect(url)
        .await
        .expect("connect to bus server")
}

async fn spawn_bus_server(
    pool: PgPool,
    enable_entity: bool,
    enable_sem_os: bool,
) -> crate::ServerHandle {
    let mut builder = BusServer::builder()
        .pool(pool.clone())
        .local_domain("test-domain")
        .invocation_dispatcher(AcceptingInvocationDispatcher::new())
        .result_dispatcher(RecordingResultDispatcher::new())
        .outbox_notifier(test_outbox_notifier(pool).await)
        .bind(ephemeral_addr());
    if enable_entity {
        builder = builder.enable_entity_service();
    }
    if enable_sem_os {
        builder = builder.enable_sem_os_service();
    }
    builder.build().serve().await.unwrap()
}

#[tokio::test]
#[ignore]
async fn validate_stub_returns_not_implemented() {
    // A3 §3.7 — Validate ships as a wire-only stub in v0.6. The
    // discipline rule (A3 §6 #1) is "stubs return NOT_IMPLEMENTED
    // consistently" — no conditional real-vs-stub paths.
    let pool = setup_pool().await;
    let handle = spawn_bus_server(pool, false, false).await;

    let mut client = connect_invocation_client(handle.local_addr()).await;
    let response = client
        .validate(sample_request(Uuid::now_v7(), "ob-poc:cbu.create"))
        .await
        .expect("validate RPC succeeds")
        .into_inner();

    assert_eq!(response.outcome, ValidationOutcome::NotImplemented as i32);
    assert_eq!(response.issues.len(), 1);
    assert_eq!(response.issues[0].issue_kind, "not_implemented");
    assert!(response.validation_id.is_some(),
        "validation_id is a transient trace UUID even for stubs");

    handle.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn entity_resolve_stub_returns_not_implemented() {
    // A3 §3.7 — When EntityService is REGISTERED (enable_entity_service
    // was called), the stub returns RESOLUTION_NOT_IMPLEMENTED. This is
    // distinct from the route being absent.
    let pool = setup_pool().await;
    let handle = spawn_bus_server(pool, true, false).await;

    let mut client = connect_entity_client(handle.local_addr()).await;
    let response = client
        .resolve(EntityResolutionRequest {
            authority: None,
            queries: vec![EntityQuery {
                entity_type: "CBU".into(),
                lookup_by: Some(
                    dsl_bus_protocol::v1::entity_query::LookupBy::NaturalKey(
                        "Allianz".into(),
                    ),
                ),
                include_state: true,
                include_audit_pointer: false,
            }],
            catalogue_version: "v1.0.0".into(),
        })
        .await
        .expect("entity resolve RPC succeeds")
        .into_inner();

    assert_eq!(response.resolutions.len(), 1);
    assert_eq!(
        response.resolutions[0].outcome,
        ResolutionOutcome::ResolutionNotImplemented as i32
    );
    assert!(response.resolution_id.is_some());

    handle.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn sem_os_fetch_dag_packs_stub_returns_not_implemented() {
    let pool = setup_pool().await;
    let handle = spawn_bus_server(pool, false, true).await;

    let mut client = connect_sem_os_client(handle.local_addr()).await;
    let response = client
        .fetch_dag_packs(DagPackRequest {
            authority: None,
            dag_pack_ids: vec!["ob-poc.cbu".into()],
            verb_ids: vec![],
            include_constellation_maps: false,
            include_derivation_chains: false,
            include_fsm_applicability: false,
            catalogue_version: "v1.0.0".into(),
        })
        .await
        .expect("fetch dag packs RPC succeeds")
        .into_inner();

    assert_eq!(response.packs.len(), 1);
    assert_eq!(
        response.packs[0].outcome,
        DagPackOutcome::DagPackNotImplemented as i32
    );
    assert!(response.response_id.is_some());

    handle.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn server_without_entity_service_returns_grpc_unimplemented() {
    // A3 §3.7 + §6 discipline #4 — A domain that does NOT declare
    // EntityService (e.g. dmn-lite) leaves the route absent. The gRPC
    // server returns `UNIMPLEMENTED` natively; callers see a Status
    // error rather than a structured NOT_IMPLEMENTED response.
    let pool = setup_pool().await;
    let handle = spawn_bus_server(pool, false, false).await;

    let mut client = connect_entity_client(handle.local_addr()).await;
    let err = client
        .resolve(EntityResolutionRequest {
            authority: None,
            queries: vec![],
            catalogue_version: "v1.0.0".into(),
        })
        .await
        .expect_err("EntityService route must be absent");
    assert_eq!(err.code(), tonic::Code::Unimplemented);

    handle.shutdown().await.unwrap();
}
