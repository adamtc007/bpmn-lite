//! §18 ruling K Part 2 remediation (blind-review finding, see
//! docs/todo/EOP-PLAN-BPMN-ISA-002.md): proves the gRPC boundary now
//! rejects an oversized/deep `Value::Array` supplied via `orch_flags`
//! BEFORE it is ever merged into an instance's `flags` — closing the
//! "poisoned instance" gap the blind review found (an unchecked array
//! reaching `instance.flags` made every subsequent `apply` call reject,
//! `Cancel`/`Terminate` included, with no exposed remedy).
//!
//! These tests drive the actual `BpmnLite::start_process` /
//! `BpmnLite::complete_job` trait-method request-handling path (not just
//! `RequestLimits::check_orch_flags` in isolation), so a regression that
//! reordered checks or dropped the call at either RPC entry point would
//! be caught here, not merely at the unit level.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_ffi_grpc::GrpcFfiOwner;
use bpmn_lite_ffi_http::HttpFfiOwner;
use bpmn_lite_server_runner::event_fanout::EventFanout;
use bpmn_lite_server_runner::grpc::proto::bpmn_lite_server::BpmnLite;
use bpmn_lite_server_runner::grpc::proto::{
    proto_value, CompleteJobRequest, ProtoValue, ProtoValueArray, StartRequest,
};
use bpmn_lite_server_runner::grpc::{BpmnLiteService, RequestLimits, ServerMetrics};
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::{MAX_VALUE_ARRAY_DEPTH, MAX_VALUE_ARRAY_LEN};
use dmn_lite_bridge::DmnLiteOwner;
use ffi_catalogue::{FfiCatalogue, MemoryFfiTemplateStore};
use tokio::sync::Semaphore;
use tonic::{Code, Request};

const MINIMAL_BPMN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="test_proc" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="do_work" />
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

fn build_service() -> BpmnLiteService {
    let store = Arc::new(MemoryStore::new());
    let engine = Arc::new(BpmnLiteEngine::new(store));
    let event_fanout = Arc::new(EventFanout::new(engine.clone(), Duration::from_secs(3600)));
    let ffi_store = Arc::new(MemoryFfiTemplateStore::new());
    let ffi_catalogue = Arc::new(FfiCatalogue::new(ffi_store.clone()));
    BpmnLiteService {
        engine,
        event_fanout,
        limits: RequestLimits::default(),
        metrics: Arc::new(ServerMetrics::default()),
        subscription_limiter: Arc::new(Semaphore::new(256)),
        ffi_owner: Arc::new(DmnLiteOwner::new()),
        http_ffi_owner: Arc::new(HttpFfiOwner::new()),
        grpc_ffi_owner: Arc::new(GrpcFfiOwner::new()),
        ffi_catalogue,
        ffi_store,
    }
}

/// A `ProtoValue::ArrayValue` with `MAX_VALUE_ARRAY_LEN + 1` scalar items
/// — exceeds the length bound at the top level.
fn oversized_len_array() -> ProtoValue {
    let items = (0..=MAX_VALUE_ARRAY_LEN as i64)
        .map(|n| ProtoValue {
            kind: Some(proto_value::Kind::I64Value(n)),
        })
        .collect();
    ProtoValue {
        kind: Some(proto_value::Kind::ArrayValue(ProtoValueArray { items })),
    }
}

/// A `ProtoValue::ArrayValue` nested `MAX_VALUE_ARRAY_DEPTH + 1` levels
/// deep — exceeds the depth bound while every level's element count is 1.
fn overly_deep_array() -> ProtoValue {
    let mut deep = ProtoValue {
        kind: Some(proto_value::Kind::I64Value(0)),
    };
    for _ in 0..=MAX_VALUE_ARRAY_DEPTH {
        deep = ProtoValue {
            kind: Some(proto_value::Kind::ArrayValue(ProtoValueArray {
                items: vec![deep],
            })),
        };
    }
    deep
}

#[tokio::test]
async fn start_process_rejects_oversized_array_in_orch_flags() {
    let service = build_service();

    let compile_result = service
        .engine
        .compile(MINIMAL_BPMN)
        .await
        .expect("compile MINIMAL_BPMN");

    let payload = r#"{"case":"array-limit-len"}"#;
    let hash = bpmn_lite_vm::compute_hash(payload);
    let mut orch_flags = HashMap::new();
    orch_flags.insert("poisoned".to_string(), oversized_len_array());

    let req = StartRequest {
        process_key: "test_proc".to_string(),
        bytecode_version: compile_result.bytecode_version.to_vec(),
        domain_payload: payload.to_string(),
        domain_payload_hash: hash.to_vec(),
        session_stack_json: String::new(),
        orch_flags,
        correlation_id: "corr-len".to_string(),
        entry_id: uuid::Uuid::new_v4().to_string(),
        runbook_id: uuid::Uuid::new_v4().to_string(),
        tenant_id: String::new(),
    };

    let status = service
        .start_process(Request::new(req))
        .await
        .expect_err("start_process must reject an oversized orch_flags array");
    // F-DSGN-1(b) (2026-07-27) supersedes the intake limit-walk at this
    // boundary: ANY non-empty orch_flags at start is rejected outright
    // (spawn-time flag seeding does not exist), so the oversized array
    // never reaches a limit check. Strictly stronger than the previous
    // cement; completion-path limit cement below is unchanged.
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("orch_flags"),
        "message should name the rejected field: {}",
        status.message()
    );
}

#[tokio::test]
async fn start_process_rejects_overly_deep_array_in_orch_flags() {
    let service = build_service();

    let compile_result = service
        .engine
        .compile(MINIMAL_BPMN)
        .await
        .expect("compile MINIMAL_BPMN");

    let payload = r#"{"case":"array-limit-depth"}"#;
    let hash = bpmn_lite_vm::compute_hash(payload);
    let mut orch_flags = HashMap::new();
    orch_flags.insert("poisoned".to_string(), overly_deep_array());

    let req = StartRequest {
        process_key: "test_proc".to_string(),
        bytecode_version: compile_result.bytecode_version.to_vec(),
        domain_payload: payload.to_string(),
        domain_payload_hash: hash.to_vec(),
        session_stack_json: String::new(),
        orch_flags,
        correlation_id: "corr-depth".to_string(),
        entry_id: uuid::Uuid::new_v4().to_string(),
        runbook_id: uuid::Uuid::new_v4().to_string(),
        tenant_id: String::new(),
    };

    let status = service
        .start_process(Request::new(req))
        .await
        .expect_err("start_process must reject an overly deep orch_flags array");
    // See F-DSGN-1(b) note in the oversized-array test above.
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(status.message().contains("orch_flags"));
}

/// F-DSGN-1(b) red: even a BENIGN, well-formed flag is rejected at start —
/// the reject is categorical (no spawn-time seeding exists), not a limit
/// check. This request PASSED validation before the fix and its flag was
/// silently discarded.
#[tokio::test]
async fn start_process_rejects_any_nonempty_orch_flags() {
    let service = build_service();

    let compile_result = service
        .engine
        .compile(MINIMAL_BPMN)
        .await
        .expect("compile MINIMAL_BPMN");

    let payload = r#"{"case":"benign-flag"}"#;
    let hash = bpmn_lite_vm::compute_hash(payload);
    let mut orch_flags = HashMap::new();
    orch_flags.insert(
        "flag_0".to_string(),
        ProtoValue {
            kind: Some(proto_value::Kind::BoolValue(true)),
        },
    );

    let req = StartRequest {
        process_key: "test_proc".to_string(),
        bytecode_version: compile_result.bytecode_version.to_vec(),
        domain_payload: payload.to_string(),
        domain_payload_hash: hash.to_vec(),
        session_stack_json: String::new(),
        orch_flags,
        correlation_id: "corr-benign".to_string(),
        entry_id: uuid::Uuid::new_v4().to_string(),
        runbook_id: uuid::Uuid::new_v4().to_string(),
        tenant_id: String::new(),
    };

    let status = service
        .start_process(Request::new(req))
        .await
        .expect_err("a benign flag at start must be rejected, not silently discarded");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(status.message().contains("F-DSGN-1"));
}

#[tokio::test]
async fn complete_job_rejects_oversized_array_in_orch_flags() {
    let service = build_service();

    let mut orch_flags = HashMap::new();
    orch_flags.insert("poisoned".to_string(), oversized_len_array());

    // job_key/worker_id/claim_token are deliberately nonsense — the
    // orch_flags check runs before any of that is resolved against real
    // engine state, so the rejection must happen first regardless.
    let req = CompleteJobRequest {
        job_key: "does-not-exist".to_string(),
        domain_payload: "{}".to_string(),
        domain_payload_hash: vec![0u8; 32],
        orch_flags,
        worker_id: "worker-1".to_string(),
        claim_token: "token-1".to_string(),
        tenant_id: String::new(),
    };

    let status = service
        .complete_job(Request::new(req))
        .await
        .expect_err("complete_job must reject an oversized orch_flags array");
    assert_eq!(status.code(), Code::ResourceExhausted);
    assert!(
        status.message().contains("poisoned"),
        "message should cite the offending key: {}",
        status.message()
    );
}
