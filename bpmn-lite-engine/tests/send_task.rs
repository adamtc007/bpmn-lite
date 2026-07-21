//! Send Task calibration probe — proves end-to-end:
//!
//!   BPMN <sendTask> in XML
//!     → parser::parse_bpmn → IRNode::SendTask
//!     → lowering::lower → Instr::PublishMessage
//!     → Vm::tick_fiber → store::buffer_message
//!     → fiber advances past send → instance completes
//!
//! Trace assertions (in order):
//!   1. parse + lower succeed (the parser arm and the lowering arm both fire)
//!   2. CompiledProgram contains exactly one `Instr::PublishMessage`
//!   3. Tick to quiescence: instance reaches ProcessState::Completed
//!   4. Zero fibers remain (the send-task fiber advanced to End and was reaped)
//!   5. The message was actually buffered: `claim_buffered_message` returns it
//!   6. A `RuntimeEvent::MessageBuffered` was emitted into the event log
//!
//! This is the FLOOR of per-element cost for widening bpmn-lite's parser: the
//! Send Task lands because message-buffer infrastructure already existed.

use bpmn_lite_compiler::{lower, parse_bpmn};
use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_store::store::WorkflowStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::events::RuntimeEvent;
use bpmn_lite_types::*;
use bpmn_lite_vm::compute_hash;
use std::sync::Arc;

const SEND_TASK_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="send_proc">
    <bpmn:startEvent id="start"/>
    <bpmn:sendTask id="send_msg" name="payment_requested"/>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="send_msg"/>
    <bpmn:sequenceFlow id="f2" sourceRef="send_msg" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>
"#;

#[tokio::test]
async fn send_task_publishes_message_and_advances() {
    // ── 1 & 2: compile XML → bytecode and confirm PublishMessage opcode emitted
    let graph = parse_bpmn(SEND_TASK_XML).expect("parse_bpmn");
    let program = lower(&graph).expect("lower");
    let publish_count = program
        .program()
        .iter()
        .filter(|i| matches!(i, Instr::PublishMessage { .. }))
        .count();
    assert_eq!(
        publish_count,
        1,
        "expected exactly one PublishMessage in bytecode; got program={:?}",
        program.program()
    );

    // ── Engine setup
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());
    store
        .store_program(program.bytecode_version(), &program)
        .await
        .unwrap();

    // ── Start instance + tick to quiescence
    let payload = r#"{"trigger":"calibration"}"#;
    let hash = compute_hash(payload);
    let iid = engine
        .start(
            "send_proc",
            program.bytecode_version(),
            payload,
            hash,
            "cal-1",
        )
        .await
        .expect("start");
    engine.tick_instance(iid).await.expect("tick_instance");

    // ── 3: instance completes
    let inspection = engine.inspect(iid).await.expect("inspect");
    assert!(
        matches!(inspection.state, ProcessState::Completed { .. }),
        "expected Completed; got {:?}",
        inspection.state
    );

    // ── 4: no fibers remain
    let fibers = store
        .load_fibers(&bpmn_lite_types::TenantId::new("default").unwrap(), iid)
        .await
        .unwrap();
    assert!(
        fibers.is_empty(),
        "expected no surviving fibers; got {}",
        fibers.len()
    );

    // ── 5: message actually buffered (correlation key = "b:false" because
    //      register 0 is uninitialised → Value::Bool(false) at publish time)
    let claimed = store
        .claim_buffered_message(
            &bpmn_lite_types::TenantId::new("default").unwrap(),
            "payment_requested",
            "b:false",
            60_000,
        )
        .await
        .unwrap();
    assert!(
        claimed.is_some(),
        "expected a buffered message for (payment_requested, b:false) — found none"
    );

    // ── 6: MessageBuffered event in event log
    let events = store
        .read_events(&bpmn_lite_types::TenantId::new("default").unwrap(), iid, 0)
        .await
        .unwrap();
    let saw_buffered = events.iter().any(|(_, e)| {
        matches!(
            e,
            RuntimeEvent::MessageBuffered { message_name, .. }
                if message_name == "payment_requested"
        )
    });
    assert!(
        saw_buffered,
        "expected MessageBuffered event in log; got {:?}",
        events.iter().map(|(_, e)| e).collect::<Vec<_>>()
    );
}
