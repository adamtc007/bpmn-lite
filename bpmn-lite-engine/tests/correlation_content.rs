//! §28 content-correlation parity probe — proves the case that was
//! *inexpressible* under the old intern-scalar substrate: two process
//! instances waiting on the same message name, discriminated purely by the
//! content of a dynamic string business key (`case_id`) resolved from each
//! instance's own domain payload.
//!
//!   - Instance A starts with `{"case_id":"ACME-1"}`, instance B with `"ACME-2"`.
//!   - Both park on the same message name `case_update`.
//!   - Publishing with correlation key `"ACME-1"` wakes A only; B stays parked.
//!   - Publishing with `"ACME-2"` then wakes B.
//!
//! Before §28 every wait correlated on the constant `"b:false"`, so this
//! discrimination was impossible.

use bpmn_lite_compiler::{lower, parse_bpmn};
use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_store::store::WorkflowStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::ProcessState;
use std::sync::Arc;

const CORR_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="corr_proc" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:dataObject id="case_id" name="case_id"></bpmn:dataObject>
    <bpmn:intermediateCatchEvent id="wait" name="case_update">
      <bpmn:messageEventDefinition messageRef="m"/>
      <bpmn:extensionElements>
        <zeebe:subscription correlationKey="=case_id"/>
      </bpmn:extensionElements>
    </bpmn:intermediateCatchEvent>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="wait"/>
    <bpmn:sequenceFlow id="f2" sourceRef="wait" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>
"#;

#[tokio::test]
async fn dynamic_string_business_key_correlates_two_instances_independently() {
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());

    let graph = parse_bpmn(CORR_XML).expect("parse_bpmn");
    let program = lower(&graph).expect("lower");
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    // Two instances, distinct dynamic business keys in their own payloads.
    let start = |case: &'static str| {
        let engine = &engine;
        let bv = program.bytecode_version();
        async move {
            let payload = format!(r#"{{"case_id":"{case}"}}"#);
            let hash = bpmn_lite_types::EffectId::content_hash((payload).as_bytes());
            let iid = engine
                .start("corr_proc", bv, &payload, hash, case)
                .await
                .expect("start");
            engine.tick_instance(iid).await.expect("tick");
            iid
        }
    };
    let a = start("ACME-1").await;
    let b = start("ACME-2").await;

    // Both parked on the message wait.
    for iid in [a, b] {
        assert!(
            !matches!(engine.inspect(iid).await.unwrap().state, ProcessState::Completed { .. }),
            "instance must be parked on the message wait before delivery"
        );
    }

    // The discrimination proof: deliver the WRONG content key to A (its key is
    // ACME-1, we send ACME-2). A must stay parked — the message name matches
    // but the resolved content key does not. Under the old constant-key
    // substrate this would have woken A regardless.
    engine
        .signal_with_value(a, "case_update", "ACME-2".to_string(), None, None, Some("m-wrong"))
        .await
        .unwrap();
    engine.tick_instance(a).await.unwrap();
    assert!(
        !matches!(engine.inspect(a).await.unwrap().state, ProcessState::Completed { .. }),
        "A must NOT wake on a wrong content key (ACME-2 != its own ACME-1)"
    );

    // Deliver A's own key — now A wakes.
    engine
        .signal_with_value(a, "case_update", "ACME-1".to_string(), None, None, Some("m-a"))
        .await
        .unwrap();
    engine.tick_instance(a).await.unwrap();
    assert!(
        matches!(engine.inspect(a).await.unwrap().state, ProcessState::Completed { .. }),
        "A correlated on its own case_id=ACME-1 and completed"
    );

    // B, independent, still parked; wakes only on its own key ACME-2.
    assert!(
        !matches!(engine.inspect(b).await.unwrap().state, ProcessState::Completed { .. }),
        "B is independent — unaffected by any delivery to A"
    );
    engine
        .signal_with_value(b, "case_update", "ACME-2".to_string(), None, None, Some("m-b"))
        .await
        .unwrap();
    engine.tick_instance(b).await.unwrap();
    assert!(
        matches!(engine.inspect(b).await.unwrap().state, ProcessState::Completed { .. }),
        "B correlated on its own case_id=ACME-2 and completed"
    );
}
