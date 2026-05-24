use super::*;
use dsl_bus_protocol::v1::TypedValue;
use std::sync::Mutex;

#[derive(Default)]
struct MockEvaluator {
    calls: Mutex<Vec<(String, String, usize)>>,
}

#[async_trait]
impl DecisionEvaluator for MockEvaluator {
    async fn evaluate(
        &self,
        decision: &str,
        catalogue: &str,
        inputs: Vec<ResolvedBinding>,
    ) -> Result<DecisionOutcome, DecisionEvaluatorError> {
        self.calls.lock().unwrap().push((
            decision.to_owned(),
            catalogue.to_owned(),
            inputs.len(),
        ));
        Ok(DecisionOutcome {
            execution_id: Uuid::now_v7(),
            kind: ExecutionOutcomeKind::Committed,
            detail: format!("decided {decision}"),
            bindings: vec![ResolvedBinding {
                name: "cbu-type".into(),
                value: Some(TypedValue {
                    value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                        "fund".into(),
                    )),
                    type_name: "CbuType".into(),
                }),
            }],
        })
    }
}

fn ctx(decision: &str) -> InvocationContext {
    InvocationContext {
        idempotency_key: Uuid::now_v7(),
        source_domain: "bpmn-lite".into(),
        catalogue_version: "v1.0.0".into(),
        local_verb_id: decision.into(),
        result_callback_endpoint: "http://bpmn-lite/result".into(),
    }
}

#[tokio::test]
async fn dispatch_routes_to_evaluator_and_carries_bindings_back() {
    let handler = DmnLiteBusHandler::new(MockEvaluator::default());
    let inputs = vec![ResolvedBinding {
        name: "cbu-client-type".into(),
        value: Some(TypedValue {
            value: Some(dsl_bus_protocol::v1::typed_value::Value::StringValue(
                "FUND_MANDATE".into(),
            )),
            type_name: "CbuClientType".into(),
        }),
    }];
    let outcome = handler
        .dispatch(ctx("cbu_type_routing"), inputs)
        .await
        .unwrap();
    assert_eq!(outcome.outcome.kind, ExecutionOutcomeKind::Committed as i32);
    assert_eq!(outcome.outcome.bindings.len(), 1);
    assert_eq!(outcome.outcome.bindings[0].name, "cbu-type");
}

#[tokio::test]
async fn unknown_decision_evaluator_error_maps_to_bus_unknown_verb() {
    struct R;
    #[async_trait]
    impl DecisionEvaluator for R {
        async fn evaluate(
            &self,
            d: &str,
            _c: &str,
            _i: Vec<ResolvedBinding>,
        ) -> Result<DecisionOutcome, DecisionEvaluatorError> {
            Err(DecisionEvaluatorError::UnknownDecision(format!(
                "decision '{d}' not in catalogue"
            )))
        }
    }
    let handler = DmnLiteBusHandler::new(R);
    let err = handler.dispatch(ctx("nope"), vec![]).await.unwrap_err();
    match err {
        BusServerError::UnknownVerb(msg) => assert!(msg.contains("nope")),
        other => panic!("expected UnknownVerb, got {other:?}"),
    }
}

#[tokio::test]
async fn version_incompatible_maps_through() {
    struct V;
    #[async_trait]
    impl DecisionEvaluator for V {
        async fn evaluate(
            &self,
            _d: &str,
            _c: &str,
            _i: Vec<ResolvedBinding>,
        ) -> Result<DecisionOutcome, DecisionEvaluatorError> {
            Err(DecisionEvaluatorError::VersionIncompatible(
                "schema drift".into(),
            ))
        }
    }
    let err = DmnLiteBusHandler::new(V)
        .dispatch(ctx("cbu_type_routing"), vec![])
        .await
        .unwrap_err();
    assert!(matches!(err, BusServerError::VersionIncompatible(_)));
}

#[tokio::test]
async fn malformed_input_maps_through() {
    struct M;
    #[async_trait]
    impl DecisionEvaluator for M {
        async fn evaluate(
            &self,
            _d: &str,
            _c: &str,
            _i: Vec<ResolvedBinding>,
        ) -> Result<DecisionOutcome, DecisionEvaluatorError> {
            Err(DecisionEvaluatorError::Malformed(
                "cbu-client-type missing".into(),
            ))
        }
    }
    let err = DmnLiteBusHandler::new(M)
        .dispatch(ctx("cbu_type_routing"), vec![])
        .await
        .unwrap_err();
    assert!(matches!(err, BusServerError::Malformed(_)));
}

#[tokio::test]
async fn noop_result_dispatcher_rejects() {
    let err = NoopResultDispatcher
        .dispatch(
            ResultContext {
                idempotency_key: Uuid::now_v7(),
                execution_id: Uuid::now_v7(),
                source_domain: "bpmn-lite".into(),
                audit_reference: String::new(),
            },
            ExecutionOutcome {
                kind: ExecutionOutcomeKind::Committed as i32,
                detail: String::new(),
                bindings: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, BusServerError::UnknownVerb(_)));
}
