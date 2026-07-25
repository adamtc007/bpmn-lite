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
//   E-O3 XOR exclusivity: `task_a1` sits behind `take_a == true`
//        (semantics cemented by the engine test corpus's
//        t_xor_v2_merge_unequal_branch_lengths test) — activating it on
//        any other payload means the split routed a token down a branch
//        whose guard is false.
//   E-O5 engine-level terminate discipline: `cancel` on a non-terminal
//        instance must succeed (the engine-level regression net over
//        F2-KERNEL-001, which made mid-fork instances un-cancellable).
//
// Time is TAPE-DRIVEN (`FuzzClock` via `new_with_runtime_context`): the
// tick arms jump logical time forward by tape-chosen deltas (0..=25.5s in
// 100ms grains), which is the only way the boundary fixture's PT1S timer
// — or any due-timer path — can actually fire inside a microsecond-scale
// exec. Under `SystemRuntimeContext` that coverage was dead, and IDs from
// `Uuid::now_v7()` made crash repro nondeterministic; both now come from
// the tape/counter.
//
// Fixture set (all lifted from the engine test corpus's known-good XML):
// linear, parallel fork/join, XOR with unequal branches reconverging on a
// shared task (flag-routed), non-interrupting boundary timer, and
// multi-instance (tape-varied collection length — length 0 probes the
// V-11 zero-match incident rule end-to-end).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use bpmn_lite_engine::{BpmnLiteEngine, RuntimeContext, RuntimeContextError};
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_store::WorkflowStore;
use bpmn_lite_types::{EffectId, ErrorClass, TenantId, Timestamp, Uuid};
use libfuzzer_sys::fuzz_target;

/// Tape-driven clock + deterministic ID source — the fuzz exec's only
/// time/identity boundary. Wall time never enters an exec.
struct FuzzClock {
    now_ms: AtomicI64,
    next_id: AtomicU64,
}

impl FuzzClock {
    /// Deterministic epoch (mid-2025 in ms), far from 0 and from the
    /// timestamp range edge even after a full tape of maximal jumps.
    const GENESIS_MS: i64 = 1_750_000_000_000;

    fn new() -> Self {
        Self {
            now_ms: AtomicI64::new(Self::GENESIS_MS),
            next_id: AtomicU64::new(1),
        }
    }

    fn advance(&self, delta_ms: i64) {
        self.now_ms.fetch_add(delta_ms, Ordering::Relaxed);
    }
}

impl RuntimeContext for FuzzClock {
    fn logical_time(&self) -> Result<Timestamp, RuntimeContextError> {
        Ok(self.now_ms.load(Ordering::Relaxed))
    }

    fn new_id(&self) -> Uuid {
        Uuid::from_u128(u128::from(self.next_id.fetch_add(1, Ordering::Relaxed)))
    }
}

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

const XOR_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="fuzz_xor" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:exclusiveGateway id="xor_split"/>
    <bpmn:serviceTask id="task_a1"><bpmn:extensionElements><zeebe:taskDefinition type="task_a1"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="task_a2"><bpmn:extensionElements><zeebe:taskDefinition type="task_a2"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:serviceTask id="merge_task"><bpmn:extensionElements><zeebe:taskDefinition type="merge_task"/></bpmn:extensionElements></bpmn:serviceTask>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f0" sourceRef="start" targetRef="xor_split"/>
    <bpmn:sequenceFlow id="fa0" sourceRef="xor_split" targetRef="task_a1">
      <bpmn:conditionExpression>= take_a == true</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
    <bpmn:sequenceFlow id="fa1" sourceRef="task_a1" targetRef="task_a2"/>
    <bpmn:sequenceFlow id="fa2" sourceRef="task_a2" targetRef="merge_task"/>
    <bpmn:sequenceFlow id="fb0" sourceRef="xor_split" targetRef="merge_task"/>
    <bpmn:sequenceFlow id="fend" sourceRef="merge_task" targetRef="end"/>
  </bpmn:process>
</bpmn:definitions>"#;

const BOUNDARY_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="fuzz_boundary" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="long_task" name="Long Running Task">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="long_work" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:boundaryEvent id="reminder" attachedToRef="long_task" cancelActivity="false">
      <bpmn:timerEventDefinition>
        <bpmn:timeDuration>PT1S</bpmn:timeDuration>
      </bpmn:timerEventDefinition>
    </bpmn:boundaryEvent>
    <bpmn:serviceTask id="send_reminder" name="Send Reminder">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="send_reminder" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end_normal" />
    <bpmn:endEvent id="end_reminder" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="long_task" />
    <bpmn:sequenceFlow id="f2" sourceRef="long_task" targetRef="end_normal" />
    <bpmn:sequenceFlow id="f3" sourceRef="reminder" targetRef="send_reminder" />
    <bpmn:sequenceFlow id="f4" sourceRef="send_reminder" targetRef="end_reminder" />
  </bpmn:process>
</bpmn:definitions>"#;

const MI_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="fuzz_mi" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:serviceTask id="verify_docs">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="verify_doc"/>
      </bpmn:extensionElements>
      <bpmn:multiInstanceLoopCharacteristics isSequential="false">
        <bpmn:extensionElements>
          <zeebe:loopCharacteristics inputCollection="doc_count" maxInstances="4"/>
        </bpmn:extensionElements>
      </bpmn:multiInstanceLoopCharacteristics>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="verify_docs"/>
    <bpmn:sequenceFlow id="f2" sourceRef="verify_docs" targetRef="end"/>
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
    let clock = Arc::new(FuzzClock::new());
    let engine =
        BpmnLiteEngine::new_with_runtime_context(store, TenantId::default(), clock.clone());

    let (process_key, xml) = match tape.u8() % 5 {
        0 => ("fuzz_linear", LINEAR_XML),
        1 => ("fuzz_parallel", PARALLEL_XML),
        2 => ("fuzz_xor", XOR_XML),
        3 => ("fuzz_boundary", BOUNDARY_XML),
        _ => ("fuzz_mi", MI_XML),
    };
    // E-O2: known-good fixture — a rejection here is a real regression.
    let compiled = engine
        .compile(xml)
        .await
        .expect("E-O2: known-good fixture must compile");

    let mut xor_take_a = false;
    let payload = match process_key {
        // XOR routing flag: true / false / absent (absent = default branch).
        "fuzz_xor" => match tape.u8() % 3 {
            0 => {
                xor_take_a = true;
                r#"{"take_a":true}"#.to_string()
            }
            1 => r#"{"take_a":false}"#.to_string(),
            _ => "{}".to_string(),
        },
        // MI collection: tape-varied length 0..=4; length 0 probes the
        // V-11 zero-match incident rule end-to-end.
        "fuzz_mi" => {
            let len = usize::from(tape.u8() % 5);
            let items: Vec<String> = (0..len).map(|i| i.to_string()).collect();
            format!(r#"{{"doc_count":[{}]}}"#, items.join(","))
        }
        _ => format!(r#"{{"case":{}}}"#, tape.u8()),
    };
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
                    // E-O3: the guarded branch must be unreachable unless
                    // its condition held at the split.
                    if process_key == "fuzz_xor" && !xor_take_a {
                        assert!(
                            activations.iter().all(|job| job.task_type != "task_a1"),
                            "E-O3: XOR split activated the guarded branch with take_a != true"
                        );
                    }
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
                // Tape-driven time jump (0..=25.5s, 100ms grains): the
                // PT1S boundary timer's due edge gets straddled from both
                // sides across execs; a 0-jump probes same-instant ticks.
                clock.advance(i64::from(tape.u8()) * 100);
                let _ = engine.tick_instance(instance_id).await;
            }
            9 => {
                clock.advance(i64::from(tape.u8()) * 100);
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
