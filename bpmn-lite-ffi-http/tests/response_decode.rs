//! `HttpFfiOwner::invoke`'s live response-body decode + error-mapping path
//! under hostile mocked HTTP responses.
//!
//! Deliberately out of scope for `bpmn-lite-ffi-http/fuzz`'s
//! `owner_metadata_decode` target (see
//! `docs/receipts/fuzz-coverage-ffi-owner-metadata-2026-08-10.md`): a real
//! network round-trip per fuzz iteration is impractical for sustained
//! in-process fuzzing. `wiremock` gives the same "hostile bytes across a
//! real HTTP round-trip" coverage at integration-test speed instead —
//! already a declared dev-dependency here, previously unused.
//!
//! `FfiExecutionOwner::invoke`'s own contract (`ffi-types/src/owner.rs`):
//! "The owner MUST NOT panic; any error must be reported via
//! `FfiResult::Incident`." Every test here holds `invoke` to exactly that
//! bar, not just "returns something."

use std::collections::HashMap;

use bpmn_lite_ffi_http::{HttpFfiOwner, HttpIdempotency, HttpMethod};
use ffi_types::{FfiCall, FfiExecutionOwner, FfiIncidentClass, FfiResult, FfiTemplate};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn register(server: &MockServer) -> (HttpFfiOwner, FfiTemplate) {
    let owner = HttpFfiOwner::new();
    let template = owner
        .register_template(
            format!("{}/callout", server.uri()),
            HttpMethod::Post,
            HashMap::new(),
            2_000,
            Vec::new(),
            vec![200],
            HttpIdempotency::NonIdempotent,
            Vec::new(),
            Vec::new(),
            "tenant-1".to_string(),
            "test".to_string(),
        )
        .expect("template registration must succeed");
    (owner, template)
}

fn call_for(template: &FfiTemplate) -> FfiCall {
    FfiCall {
        invocation_id: Uuid::new_v4(),
        template_id: template.template_id,
        tenant_id: "tenant-1".to_string(),
        process_instance_id: Uuid::new_v4(),
        caller_task_id: "task-1".to_string(),
        input_payload: serde_json::to_vec(&serde_json::json!({})).unwrap(),
    }
}

#[tokio::test]
async fn success_on_valid_json_object_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"result": "ok"})))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    match result {
        FfiResult::Success { output_payload, .. } => {
            let body: serde_json::Value = serde_json::from_slice(&output_payload).unwrap();
            assert_eq!(body["result"], "ok");
        }
        other => panic!("expected Success, got {other:?}"),
    }
}

#[tokio::test]
async fn no_match_on_empty_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    assert!(matches!(result, FfiResult::NoMatch { .. }));
}

#[tokio::test]
async fn no_match_on_null_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(b"null".to_vec(), "application/json"))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    assert!(matches!(result, FfiResult::NoMatch { .. }));
}

#[tokio::test]
async fn no_match_on_empty_object_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    assert!(matches!(result, FfiResult::NoMatch { .. }));
}

#[tokio::test]
async fn incident_on_non_object_json_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([1, 2, 3])))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    match result {
        FfiResult::Incident { error_class, message, .. } => {
            assert_eq!(error_class, FfiIncidentClass::ContractViolation);
            assert!(message.contains("array"), "message: {message}");
        }
        other => panic!("expected Incident, got {other:?}"),
    }
}

#[tokio::test]
async fn incident_on_malformed_json_body_on_success_status_never_panics() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(b"not json at all {{{".to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    match result {
        FfiResult::Incident { error_class, message, .. } => {
            assert_eq!(error_class, FfiIncidentClass::ContractViolation);
            assert!(message.contains("not valid JSON"), "message: {message}");
        }
        other => panic!("expected Incident, got {other:?}"),
    }
}

#[tokio::test]
async fn contract_violation_on_400() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    match result {
        FfiResult::Incident { error_class, retry_hint_ms, .. } => {
            assert_eq!(error_class, FfiIncidentClass::ContractViolation);
            assert_eq!(retry_hint_ms, None);
        }
        other => panic!("expected Incident, got {other:?}"),
    }
}

#[tokio::test]
async fn business_rejection_on_404() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    match result {
        FfiResult::Incident { error_class, .. } => {
            assert_eq!(
                error_class,
                FfiIncidentClass::BusinessRejection {
                    rejection_code: "HTTP_NOT_FOUND".to_string()
                }
            );
        }
        other => panic!("expected Incident, got {other:?}"),
    }
}

#[tokio::test]
async fn business_rejection_on_409() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(409))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    match result {
        FfiResult::Incident { error_class, .. } => {
            assert_eq!(
                error_class,
                FfiIncidentClass::BusinessRejection {
                    rejection_code: "HTTP_CONFLICT".to_string()
                }
            );
        }
        other => panic!("expected Incident, got {other:?}"),
    }
}

#[tokio::test]
async fn transient_with_retry_hint_on_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    match result {
        FfiResult::Incident { error_class, retry_hint_ms, .. } => {
            assert_eq!(error_class, FfiIncidentClass::Transient);
            assert_eq!(retry_hint_ms, Some(1000));
        }
        other => panic!("expected Incident, got {other:?}"),
    }
}

/// Regression for the `body_excerpt` char-boundary panic fixed alongside
/// this test suite (see `docs/receipts/fuzz-coverage-ffi-owner-metadata-2026-08-10.md`'s
/// follow-up receipt): an error body with a multi-byte character straddling
/// the 256-byte excerpt truncation point must produce a typed `Incident`,
/// never panic, through the real HTTP round-trip — not just the isolated
/// unit test on `body_excerpt` directly.
#[tokio::test]
async fn incident_never_panics_on_error_body_straddling_the_excerpt_boundary() {
    let server = MockServer::start().await;
    let mut body = vec![b'a'; 253];
    body.extend_from_slice("𝄞".as_bytes()); // U+1D11E, 4 bytes, straddles byte 256
    body.extend_from_slice(b"trailing content past the truncation point to be safe");
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(400).set_body_raw(body, "text/plain"))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    match result {
        FfiResult::Incident { error_class, message, .. } => {
            assert_eq!(error_class, FfiIncidentClass::ContractViolation);
            assert!(message.contains("..."), "message: {message}");
        }
        other => panic!("expected Incident, got {other:?}"),
    }
}

/// Same boundary case again with genuinely invalid UTF-8 (not just a
/// multi-byte char split across the boundary), on a raw binary error body.
#[tokio::test]
async fn incident_never_panics_on_invalid_utf8_error_body() {
    let server = MockServer::start().await;
    let mut body = vec![b'a'; 255];
    body.push(0xC2); // lone leading byte of a 2-byte sequence, truncated
    body.extend_from_slice(&[0xFF; 100]); // more invalid bytes past the limit
    Mock::given(method("POST"))
        .and(path("/callout"))
        .respond_with(ResponseTemplate::new(500).set_body_raw(body, "application/octet-stream"))
        .mount(&server)
        .await;
    let (owner, template) = register(&server).await;

    let result = owner.invoke(call_for(&template)).await.unwrap();
    assert!(matches!(result, FfiResult::Incident { .. }));
}
