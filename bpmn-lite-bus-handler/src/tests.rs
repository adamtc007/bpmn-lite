use super::*;
use dsl_bus_protocol::v1::TypedValue;
use std::sync::Mutex;

#[derive(Default)]
struct RecordingAdvancer {
    calls: Mutex<Vec<ProcessAdvanceInput>>,
}

#[async_trait]
impl ProcessAdvancer for RecordingAdvancer {
    async fn advance(&self, input: ProcessAdvanceInput) -> Result<(), ProcessAdvancerError> {
        self.calls.lock().unwrap().push(input);
        Ok(())
    }
}

fn ctx(execution_id: Uuid) -> ResultContext {
    ResultContext {
        tenant_id: "default".to_string(),
        idempotency_key: Uuid::now_v7(),
        execution_id,
        source_domain: "ob-poc".into(),
        audit_reference: "audit://ob-poc/abc".into(),
    }
}

fn outcome_with_bindings() -> ExecutionOutcome {
    ExecutionOutcome {
        kind: ExecutionOutcomeKind::Committed as i32,
        detail: "ok".into(),
        bindings: vec![ResolvedBinding {
            name: "cbu".into(),
            value: Some(TypedValue {
                value: Some(dsl_bus_protocol::v1::typed_value::Value::UuidValue(
                    dsl_bus_protocol::v1::Uuid {
                        value: Uuid::now_v7().as_bytes().to_vec(),
                    },
                )),
                type_name: "CBU".into(),
            }),
        }],
    }
}

#[tokio::test]
async fn dispatch_records_input_via_concrete_arc() {
    // Hold the recording advancer through an `Arc` so we can read
    // back the captured calls without downcasting trait objects.
    let advancer = Arc::new(RecordingAdvancer::default());
    let handler = BpmnLiteBusHandler::from_arc(advancer.clone());
    let exec_id = Uuid::now_v7();
    ResultDispatcher::dispatch(&handler, ctx(exec_id), outcome_with_bindings())
        .await
        .unwrap();
    let calls = advancer.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].execution_id, exec_id);
    assert_eq!(calls[0].source_domain, "ob-poc");
    assert_eq!(calls[0].outcome_kind, ExecutionOutcomeKind::Committed);
    assert_eq!(calls[0].bindings.len(), 1);
    assert_eq!(calls[0].bindings[0].name, "cbu");
    assert_eq!(calls[0].audit_reference, "audit://ob-poc/abc");
}

#[tokio::test]
async fn unknown_execution_advancer_error_maps_to_internal() {
    struct U;
    #[async_trait]
    impl ProcessAdvancer for U {
        async fn advance(&self, input: ProcessAdvanceInput) -> Result<(), ProcessAdvancerError> {
            Err(ProcessAdvancerError::UnknownExecution(input.execution_id))
        }
    }
    let handler = BpmnLiteBusHandler::new(U);
    let err = ResultDispatcher::dispatch(&handler, ctx(Uuid::now_v7()), outcome_with_bindings())
        .await
        .unwrap_err();
    match err {
        BusServerError::Internal(msg) => assert!(msg.contains("unknown execution_id")),
        other => panic!("expected Internal, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_advancer_error_maps_to_malformed() {
    struct M;
    #[async_trait]
    impl ProcessAdvancer for M {
        async fn advance(&self, _i: ProcessAdvanceInput) -> Result<(), ProcessAdvancerError> {
            Err(ProcessAdvancerError::Malformed("binding mismatch".into()))
        }
    }
    let handler = BpmnLiteBusHandler::new(M);
    let err = ResultDispatcher::dispatch(&handler, ctx(Uuid::now_v7()), outcome_with_bindings())
        .await
        .unwrap_err();
    assert!(matches!(err, BusServerError::Malformed(_)));
}

fn invocation_ctx(local_verb_id: &str) -> InvocationContext {
    InvocationContext {
        idempotency_key: Uuid::now_v7(),
        source_domain: "ob-poc".into(),
        catalogue_version: "test".into(),
        local_verb_id: local_verb_id.into(),
        result_callback_endpoint: "bus://ob-poc/result".into(),
        authority: Some(dsl_bus_protocol::v1::AuthorityContext {
            service_identity: "ob-poc".into(),
            user_identity: "test".into(),
            // Every role any dispatch arm's `assert_scope` might require, so
            // this test proves whether a verb id reaches a real match arm,
            // not whether it happens to hold the right scope.
            roles: vec![
                "bpmn.template.write".into(),
                "bpmn.template.read".into(),
                "bpmn.instance.write".into(),
                "bpmn.instance.read".into(),
            ],
            signed_token: vec![],
        }),
        tenant_id: "default".into(),
        snapshot_pin: None,
    }
}

/// `HANDLED_VERBS` (declared next to the dispatch match in `lib.rs`) must
/// name exactly the verb ids the match actually routes, no more and no
/// less — see that const's doc comment for why: `cargo xtask pack-check
/// bpmn` trusts it instead of an independently hand-maintained copy, so a
/// silent drift here would make that gate meaningless.
///
/// This handler is built with `engine: None, pool: None` (`BpmnLiteBusHandler::new`),
/// so every real arm fails deep inside on a missing-engine/missing-binding
/// error once past authority -- the discriminator this test needs is only
/// "did we reach a real arm at all", i.e. anything other than
/// `BusServerError::UnknownVerb`.
#[tokio::test]
async fn handled_verbs_matches_every_dispatch_arm() {
    struct NoopAdvancer;
    #[async_trait]
    impl ProcessAdvancer for NoopAdvancer {
        async fn advance(&self, _i: ProcessAdvanceInput) -> Result<(), ProcessAdvancerError> {
            Ok(())
        }
    }
    let handler = BpmnLiteBusHandler::new(NoopAdvancer);

    for verb in HANDLED_VERBS {
        let result =
            InvocationDispatcher::dispatch(&handler, invocation_ctx(verb), Vec::new()).await;
        assert!(
            !matches!(result, Err(BusServerError::UnknownVerb(_))),
            "HANDLED_VERBS lists '{verb}' but dispatch fell through to the \
             UnknownVerb fallback -- the match arm for it is gone; remove it \
             from HANDLED_VERBS (and from cargo xtask pack-check's expected \
             invocation surface if it was a real handler-backed verb)"
        );
    }

    let result =
        InvocationDispatcher::dispatch(&handler, invocation_ctx("not-a-real-verb"), Vec::new())
            .await;
    assert!(
        matches!(&result, Err(BusServerError::UnknownVerb(id)) if id == "not-a-real-verb"),
        "an id absent from every match arm must fall through to UnknownVerb, got {result:?}"
    );
}

#[tokio::test]
async fn outcome_kind_unspecified_when_proto_value_unknown() {
    // Concrete advancer captures the input.
    let advancer = Arc::new(RecordingAdvancer::default());
    let handler = BpmnLiteBusHandler::from_arc(advancer.clone());
    let exec_id = Uuid::now_v7();
    let outcome = ExecutionOutcome {
        kind: 999, // outside the defined enum range
        detail: String::new(),
        bindings: vec![],
    };
    ResultDispatcher::dispatch(&handler, ctx(exec_id), outcome)
        .await
        .unwrap();
    let calls = advancer.calls.lock().unwrap();
    assert_eq!(
        calls[0].outcome_kind,
        ExecutionOutcomeKind::OutcomeUnspecified
    );
}
