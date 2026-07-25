#![no_main]

// EOP-FUZZ F5 (engine tier, un-deferred 2026-07-25): drive the FULL engine
// over MemoryStore — compile → start → an adversarial tape of public-API
// calls (run/complete/fail/signal/tick/cancel/inspect). This reaches what
// the pure-kernel targets cannot: scheduler/claim/lease logic, the job
// queue, dedupe, payload-hash discipline, and the engine's own
// command-assembly around kernel::apply.
//
// Oracles:
//   E-O1 no-panic anywhere (every Err is a legal reject; panics are
//        findings — tokio runtime included).
//   E-O2 static fixtures must COMPILE: these XMLs are known-good; a
//        compile rejection is a compiler/verifier regression, not fuzz
//        noise.
//   E-O5 engine-level terminate discipline: `cancel` on a non-terminal
//        instance must succeed (the engine-level regression net over
//        F2-KERNEL-001, which made mid-fork instances un-cancellable).
//
// Known coverage gaps (no silent caps): fixtures don't yet include
// exclusive-gateway placeholder routing, boundary timers, or
// multi-instance — those XML shapes should be added as fixtures once
// their canonical admitted forms are lifted from the engine test corpus.

use std::collections::BTreeMap;
use std::sync::Arc;

use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_store::WorkflowStore;
use bpmn_lite_types::{EffectId, ErrorClass};
use libfuzzer_sys::fuzz_target;

const LINEAR_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="fuzz_linear" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="Do Work">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="do_work" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

const PARALLEL_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="fuzz_parallel" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:parallelGateway id="and_fork" gatewayDirection="Diverging"/>
    <bpmn:serviceTask id="task_p1"><bpmn:extensionElements><zeebe:taskDefinition type="task_p1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_p2"><bpmn:extensionElements><zeebe:taskDefinition type="task_p2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:parallelGateway id="and_join" gatewayDirection="Converging"/>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f0" sourceRef="start" targetRef="and_fork"/>
    <bpmn:sequenceFlow id="f1" sourceRef="and_fork" targetRef="task_p1"/>
    <bpmn:sequenceFlow id="f2" sourceRef="and_fork" targetRef="task_p2"/>
    <bpmn:sequenceFlow id="f3" sourceRef="task_p1" targetRef="and_join"/>
    <bpmn:sequenceFlow id="f4" sourceRef="task_p2" targetRef="and_join"/>
    <bpmn:sequenceFlow id="f5" sourceRef="and_join" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#;

struct Tape<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Tape<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn u8(&mut self) -> u8 {
        let byte = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        byte
    }
    fn bool(&mut self) -> bool {
        self.u8() & 1 == 1
    }
}

async fn drive(data: &[u8]) {
    let mut tape = Tape::new(data);
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store);

    let (process_key, xml) = if tape.bool() {
        ("fuzz_linear", LINEAR_XML)
    } else {
        ("fuzz_parallel", PARALLEL_XML)
    };
    // E-O2: known-good fixture — a rejection here is a real regression.
    let compiled = engine
        .compile(xml)
        .await
        .expect("E-O2: known-good fixture must compile");

    let payload = format!(r#"{{"case":{}}}"#, tape.u8());
    let mut current_hash = EffectId::content_hash(payload.as_bytes());
    let Ok(instance_id) = engine
        .start(
            process_key,
            compiled.bytecode_version,
            &payload,
            current_hash,
            "corr-fuzz",
        )
        .await
    else {
        return;
    };

    let mut job_keys: Vec<String> = Vec::new();
    let steps = 8 + usize::from(tape.u8() % 17);
    for _ in 0..steps {
        match tape.u8() % 12 {
            0..=3 => {
                if let Ok(activations) = engine.run_instance(instance_id).await {
                    job_keys.extend(activations.into_iter().map(|job| job.job_key));
                }
            }
            4 | 5 => {
                if job_keys.is_empty() {
                    continue;
                }
                let key = job_keys[usize::from(tape.u8()) % job_keys.len()].clone();
                let result_payload = format!(r#"{{"result":{}}}"#, tape.u8());
                // Tape decides between the tracked (valid) hash and a wild
                // one — the wild arm probes the hash-discipline reject path.
                let hash = if tape.bool() {
                    current_hash
                } else {
                    [tape.u8(); 32]
                };
                if engine
                    .complete_job(&key, &result_payload, hash, BTreeMap::new())
                    .await
                    .is_ok()
                {
                    current_hash = EffectId::content_hash(result_payload.as_bytes());
                }
            }
            6 => {
                if job_keys.is_empty() {
                    continue;
                }
                let key = job_keys[usize::from(tape.u8()) % job_keys.len()].clone();
                let error_class = match tape.u8() % 3 {
                    0 => ErrorClass::Transient,
                    1 => ErrorClass::ContractViolation,
                    _ => ErrorClass::BusinessRejection {
                        rejection_code: format!("R{}", tape.u8()),
                    },
                };
                let _ = engine.fail_job(&key, error_class, "fuzz failure").await;
            }
            7 => {
                let _ = engine
                    .signal(
                        instance_id,
                        "msg-fuzz",
                        &format!("corr-{}", tape.u8() % 4),
                        None,
                        None,
                        None,
                    )
                    .await;
            }
            8 => {
                let _ = engine.tick_instance(instance_id).await;
            }
            9 => {
                let _ = engine.tick_all().await;
            }
            10 => {
                let _ = engine.inspect(instance_id).await;
            }
            _ => {
                // Mid-run cancel: E-O5 asserted below on the final state;
                // here it simply must not panic.
                let _ = engine.cancel(instance_id, "fuzz mid-run cancel").await;
            }
        }
    }

    // E-O5: a non-terminal instance must be cancellable (engine-level net
    // over F2-KERNEL-001's un-cancellable mid-fork instances).
    if let Ok(inspection) = engine.inspect(instance_id).await {
        if !inspection.state.is_terminal() {
            engine
                .cancel(instance_id, "fuzz final cancel")
                .await
                .expect("E-O5: cancel rejected on a non-terminal instance");
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build current-thread tokio runtime");
    runtime.block_on(drive(data));
});
