//! A11 — First end-to-end FFI proof.
//!
//! Verifies the full call chain:
//!
//!   BPMN process (ExecFfi)
//!     → FfiDispatcher
//!     → DmnLiteOwner
//!     → dmn-lite stack VM
//!     → EvaluationOutput
//!     → output binding written back to process flags
//!     → gateway branches on result
//!
//! Test process:
//!   Start → CheckEligibility (ExecFfi) → XOR gateway → EligibleEnd / DeniedEnd
//!
//! dmn-lite decision: integer input `score`, integer output `tier`
//!   - score = 100 → tier = 1
//!   - catch-all    → tier = 0
//!
//! Data objects:
//!   do_score  (integer) → FlagKey (BindingSource::FlagRef)
//!   do_eligible   (integer) → FlagKey (BindingTarget::FlagWrite)
//!
//! The test pre-initialises `do_score = 100` in instance flags, runs the
//! process, then asserts `do_eligible == 1` and process state == Completed.

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::*;
use bpmn_lite_vm::compute_hash;
use dmn_lite_bridge::DmnLiteOwner;
use dmn_lite_compiler::{compile_and_verify, load_catalogue_from_str};
use dmn_lite_parser::parse;
use ffi_catalogue::{FfiCatalogue, FfiTemplateStore, MemoryFfiTemplateStore};
use ffi_dispatcher::FfiDispatcher;
use ffi_types::{FieldSchema, Idempotency, SchemaKind};

use std::sync::Arc;

const INT_CAT: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"
[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "integers"
"#;

/// Compile and register the dmn-lite decision; return (template_id_hex, DmnLiteOwner, FfiCatalogue).
///
/// Decision: integer `score` input → bool `eligible` output.
///   score = 100 → eligible = true
///   catch-all   → eligible = false
///
/// Bool output → FlagWrite (fits in bpmn_lite_types::Value::Bool).
/// Integer input → FlagRef (pre-initialised by test before running).
async fn setup_ffi() -> (String, Arc<DmnLiteOwner>, Arc<FfiCatalogue>) {
    let catalogue = load_catalogue_from_str(INT_CAT).expect("catalogue load");
    let src = r#"(define-decision check :hit-policy first
        :inputs  ((score    :type integer :domain N))
        :outputs ((eligible :type bool    :domain N))
        :rules   ((rule r001 :when ((score = 100)) :then ((eligible = true)))
                  (rule r999 :when (*) :then ((eligible = false)))))"#;
    let decision = compile_and_verify(parse(src).expect("parse"), &catalogue, src)
        .expect("compile_and_verify");

    let owner = Arc::new(DmnLiteOwner::new());
    let template = owner.register_decision(
        decision,
        vec![FieldSchema {
            name: "score".to_string(),
            kind: SchemaKind::I64,
            required: true,
        }],
        vec![FieldSchema {
            name: "eligible".to_string(),
            kind: SchemaKind::Bool,
            required: false,
        }],
        Idempotency::Idempotent,
        "tenant-a".to_string(),
        "test".to_string(),
    );

    // Hex-encode the template_id.
    let template_id_hex: String = template
        .template_id
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    // Publish to FFI catalogue.
    let ffi_store = Arc::new(MemoryFfiTemplateStore::new());
    ffi_store
        .publish(&template)
        .await
        .expect("publish template");
    let ffi_cat = Arc::new(FfiCatalogue::new(ffi_store));
    ffi_cat
        .load_into_cache("tenant-a")
        .await
        .expect("load cache");

    (template_id_hex, owner, ffi_cat)
}

fn build_bpmn_xml(template_id_hex: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
  <bpmn:process id="eligibility" isExecutable="true">
    <bpmn:dataObject id="do_score">
      <bpmn:extensionElements>
        <bpmn:dataType primitive="integer" role="input"/>
      </bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:dataObject id="do_eligible">
      <bpmn:extensionElements>
        <bpmn:dataType primitive="bool" role="output"/>
      </bpmn:extensionElements>
    </bpmn:dataObject>
    <bpmn:startEvent id="start"/>
    <bpmn:serviceTask id="CheckEligibility" name="Check Eligibility">
      <bpmn:extensionElements>
        <bpmn:taskDefinition implementation="{template_id}">
          <bpmn:input  target="score" expression="${{do_score}}"/>
          <bpmn:output target="do_eligible" source="eligible"/>
        </bpmn:taskDefinition>
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:exclusiveGateway id="gw" name="Eligible?"/>
    <bpmn:endEvent id="eligible_end"/>
    <bpmn:endEvent id="denied_end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="CheckEligibility"/>
    <bpmn:sequenceFlow id="f2" sourceRef="CheckEligibility" targetRef="gw"/>
    <bpmn:sequenceFlow id="f3" sourceRef="gw" targetRef="eligible_end">
      <bpmn:conditionExpression>= do_eligible == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="f4" sourceRef="gw" targetRef="denied_end"/>
  </bpmn:process>
</bpmn:definitions>"#,
        template_id = template_id_hex
    )
}

#[tokio::test]
async fn a11_ffi_call_updates_output_flag_and_process_completes() {
    let (template_id_hex, owner, ffi_cat) = setup_ffi().await;

    // Wire FFI dispatcher.
    let mut dispatcher = FfiDispatcher::new(ffi_cat);
    dispatcher.register_owner(owner).expect("register owner");
    let dispatcher = Arc::new(dispatcher);

    // Set up engine.
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone()).with_ffi_dispatcher(dispatcher);

    // Compile the BPMN process.
    let bpmn_xml = build_bpmn_xml(&template_id_hex);
    let compile_result = engine.compile(&bpmn_xml).await.expect("compile BPMN");
    let bytecode_version = compile_result.bytecode_version;

    // Load the compiled program to find flag keys.
    let program = store
        .load_program(bytecode_version)
        .await
        .expect("load program")
        .expect("program must exist");

    // Find FlagKey for "do_score" and "do_eligible" from the symbol table.
    let score_key = *program
        .flag_symbol_table
        .iter()
        .find(|(_, name)| *name == "do_score")
        .expect("do_score must be in flag_symbol_table")
        .0;
    let eligible_key = *program
        .flag_symbol_table
        .iter()
        .find(|(_, name)| *name == "do_eligible")
        .expect("do_eligible must be in flag_symbol_table")
        .0;

    // Start the process instance.
    let payload = "{}";
    let hash = compute_hash(payload);
    let instance_id = engine
        .start("eligibility", bytecode_version, payload, hash, "a11-corr")
        .await
        .expect("start instance");

    // Pre-initialise do_score = 100 in instance flags.
    let mut instance = store
        .load_instance(instance_id)
        .await
        .expect("load instance")
        .expect("instance must exist");
    instance.flags.insert(score_key, Value::I64(100));
    store.save_instance(&instance).await.expect("save instance");

    // Run the instance — ExecFfi fires, decision evaluates, do_eligible = 1.
    engine
        .run_instance(instance_id)
        .await
        .expect("run_instance");

    // Reload and verify.
    let instance = store
        .load_instance(instance_id)
        .await
        .expect("load instance after run")
        .expect("instance must exist");

    // Process should complete (all fibers ended).
    assert!(
        matches!(instance.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        instance.state
    );

    // do_eligible must be 1 (decision r001 matched: score=100 → tier=1).
    let eligible_value = instance.flags.get(&eligible_key).cloned();
    assert_eq!(
        eligible_value,
        Some(Value::Bool(true)),
        "expected do_eligible=1, got {:?}",
        eligible_value
    );
}

#[tokio::test]
async fn a11_no_match_score_produces_tier_zero() {
    let (template_id_hex, owner, ffi_cat) = setup_ffi().await;

    let mut dispatcher = FfiDispatcher::new(ffi_cat);
    dispatcher.register_owner(owner).expect("register owner");

    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone()).with_ffi_dispatcher(Arc::new(dispatcher));

    let bpmn_xml = build_bpmn_xml(&template_id_hex);
    let compile_result = engine.compile(&bpmn_xml).await.expect("compile");
    let bytecode_version = compile_result.bytecode_version;

    let program = store
        .load_program(bytecode_version)
        .await
        .expect("load program")
        .unwrap();
    let score_key = *program
        .flag_symbol_table
        .iter()
        .find(|(_, n)| *n == "do_score")
        .unwrap()
        .0;
    let eligible_key = *program
        .flag_symbol_table
        .iter()
        .find(|(_, n)| *n == "do_eligible")
        .unwrap()
        .0;

    let payload = "{}";
    let hash = compute_hash(payload);
    let instance_id = engine
        .start("eligibility", bytecode_version, payload, hash, "a11-corr-2")
        .await
        .expect("start");

    // Score = 99 → catch-all fires → tier = 0.
    let mut instance = store.load_instance(instance_id).await.unwrap().unwrap();
    instance.flags.insert(score_key, Value::I64(99));
    store.save_instance(&instance).await.unwrap();

    engine.run_instance(instance_id).await.expect("run");

    let instance = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(
        matches!(instance.state, ProcessState::Completed { .. }),
        "expected Completed, got {:?}",
        instance.state
    );
    assert_eq!(
        instance.flags.get(&eligible_key).cloned(),
        Some(Value::Bool(false)),
        "catch-all should produce tier=0"
    );
}

#[tokio::test]
async fn a11_no_ffi_dispatcher_creates_incident() {
    let (template_id_hex, _owner, _ffi_cat) = setup_ffi().await;

    // Intentionally do NOT attach a dispatcher.
    let store: Arc<dyn ProcessStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone()); // no with_ffi_dispatcher

    let bpmn_xml = build_bpmn_xml(&template_id_hex);
    let compile_result = engine.compile(&bpmn_xml).await.expect("compile");
    let bytecode_version = compile_result.bytecode_version;

    let program = store.load_program(bytecode_version).await.unwrap().unwrap();
    let score_key = *program
        .flag_symbol_table
        .iter()
        .find(|(_, n)| *n == "do_score")
        .unwrap()
        .0;

    let payload = "{}";
    let hash = compute_hash(payload);
    let instance_id = engine
        .start(
            "eligibility",
            bytecode_version,
            payload,
            hash,
            "a11-incident",
        )
        .await
        .expect("start");

    let mut instance = store.load_instance(instance_id).await.unwrap().unwrap();
    instance.flags.insert(score_key, Value::I64(100));
    store.save_instance(&instance).await.unwrap();

    engine.run_instance(instance_id).await.expect("run");

    let instance = store.load_instance(instance_id).await.unwrap().unwrap();
    assert!(
        matches!(instance.state, ProcessState::Failed { .. }),
        "expected Failed (no dispatcher = incident), got {:?}",
        instance.state
    );
}
