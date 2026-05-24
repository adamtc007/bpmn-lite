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
    handler.dispatch(ctx(exec_id), outcome_with_bindings()).await.unwrap();
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
    let err = handler
        .dispatch(ctx(Uuid::now_v7()), outcome_with_bindings())
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
    let err = handler
        .dispatch(ctx(Uuid::now_v7()), outcome_with_bindings())
        .await
        .unwrap_err();
    assert!(matches!(err, BusServerError::Malformed(_)));
}

#[tokio::test]
async fn reject_invocation_dispatcher_responds_with_unknown_verb() {
    let h = RejectInvocationDispatcher;
    let err = h
        .dispatch(
            InvocationContext {
                idempotency_key: Uuid::now_v7(),
                source_domain: "ob-poc".into(),
                catalogue_version: "v1.0.0".into(),
                local_verb_id: "cbu.create".into(),
                result_callback_endpoint: String::new(),
            },
            vec![],
        )
        .await
        .unwrap_err();
    match err {
        BusServerError::UnknownVerb(msg) => assert!(msg.contains("bpmn-lite does not accept")),
        other => panic!("expected UnknownVerb, got {other:?}"),
    }
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
    handler.dispatch(ctx(exec_id), outcome).await.unwrap();
    let calls = advancer.calls.lock().unwrap();
    assert_eq!(
        calls[0].outcome_kind,
        ExecutionOutcomeKind::OutcomeUnspecified
    );
}
