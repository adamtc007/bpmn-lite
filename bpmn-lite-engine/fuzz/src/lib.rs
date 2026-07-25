//! F6 — graph-shape generator for the engine tier (EOP-FUZZ §9 ruling,
//! 2026-07-25): the SHAPE is the fuzzed variable. The tape draws a
//! bounded SESE process graph from a shape grammar, emits real BPMN XML,
//! and runs it through the REAL compiler — so lowering itself is under
//! test, not just execution over fixed fixtures. Because the harness
//! authored the shape, the shape IS the shadow model: per-task activation
//! bounds are computed from the tree and asserted against observed
//! distinct job keys.
//!
//! Oracles (beyond F5's E-O1 no-panic / E-O5 cancel discipline, which the
//! drive loop re-asserts):
//!   G-A  must-admit    — every graph the grammar emits is a legal SESE
//!                        process; a compile rejection is a
//!                        compiler/lowering finding ("every declared
//!                        control provably live" — the product thesis),
//!                        not fuzz noise.
//!   G-T  conservation  — per task, #distinct observed job keys ≤ the
//!                        shape-derived bound (plain/host/handler/merge:
//!                        1; XOR-guarded with flag false: 0; MI:
//!                        collection length). Sound because job keys are
//!                        `{instance}:{task_id}:{pc}:{loop_epoch}` —
//!                        stable across retries and lease-expiry
//!                        redeliveries — so a SECOND distinct key at a
//!                        bound-1 task is a duplicated token, never
//!                        at-least-once delivery. Subsumes E-O3: the
//!                        guarded branch's bound is 0 when its flag is
//!                        false.
//!
//! Recorded scope limits (no silent gaps):
//! - v1 grammar is FLAT composition: sequences of
//!   task / AND-split-join (2-3 branches × 1-2 tasks) / XOR (guarded
//!   branch 1-2 tasks + empty default, reconverging on a shared merge
//!   task) / boundary-timer task (both interrupting and non-interrupting)
//!   / parallel MI. Gateway nesting inside branches is the v2 widening —
//!   a legal-SESE compile rejection there would be a *real* finding, but
//!   land the flat family green first.
//! - A both-paths XOR tear with the EMPTY default branch can be masked at
//!   the merge task by job-key dedupe (the key excludes fiber id); the
//!   two-sided catch needs a task-bearing default branch (v2, pending a
//!   compiler receipt for that shape). Flag-false guarded activation IS
//!   caught (bound 0).
//! - Timers: tape-driven `FuzzClock` (shared with F5) — tick arms jump
//!   logical time, so PT1S boundary timers genuinely fire in-exec.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use bpmn_lite_engine::{BpmnLiteEngine, RuntimeContext, RuntimeContextError};
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_store::WorkflowStore;
use bpmn_lite_types::{EffectId, ErrorClass, TenantId, Timestamp, Uuid};

// ─── Tape (shared with the engine_commands target) ───────────────────

pub struct Tape<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Tape<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn u8(&mut self) -> u8 {
        let byte = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        byte
    }

    pub fn bool(&mut self) -> bool {
        self.u8() & 1 == 1
    }
}

// ─── Clock (shared with the engine_commands target) ──────────────────

/// Tape-driven clock + deterministic ID source — the fuzz exec's only
/// time/identity boundary. Wall time never enters an exec.
pub struct FuzzClock {
    now_ms: AtomicI64,
    next_id: AtomicU64,
}

impl FuzzClock {
    /// Deterministic epoch (mid-2025 in ms), far from 0 and from the
    /// timestamp range edge even after a full tape of maximal jumps.
    pub const GENESIS_MS: i64 = 1_750_000_000_000;

    pub fn new() -> Self {
        Self {
            now_ms: AtomicI64::new(Self::GENESIS_MS),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn advance(&self, delta_ms: i64) {
        self.now_ms.fetch_add(delta_ms, Ordering::Relaxed);
    }
}

impl Default for FuzzClock {
    fn default() -> Self {
        Self::new()
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

// ─── Shape grammar ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Block {
    /// One plain service task.
    Task,
    /// Parallel fork/join; `branches[i]` = task count (1-2) on branch i.
    And { branches: Vec<u8> },
    /// Exclusive split: guarded branch (`guarded_len` tasks, 1-2) behind
    /// `= g{n} == true`, empty default flow; both reconverge on a shared
    /// merge task (the corpus-proven no-converging-gateway shape).
    Xor { take_guarded: bool, guarded_len: u8 },
    /// Host task with an attached PT1S boundary timer and a handler task
    /// routed to its own end event.
    Boundary { interrupting: bool },
    /// Parallel multi-instance task over a payload collection
    /// (`maxInstances="4"`); length 0 probes the V-11 zero-match rule.
    Mi { collection_len: u8 },
}

#[derive(Debug, Clone)]
pub struct Shape {
    pub blocks: Vec<Block>,
}

pub fn gen_shape(tape: &mut Tape) -> Shape {
    let count = 1 + tape.u8() % 3;
    let mut blocks = Vec::new();
    for _ in 0..count {
        blocks.push(match tape.u8() % 8 {
            0..=2 => Block::Task,
            3 | 4 => Block::And {
                branches: (0..(2 + tape.u8() % 2)).map(|_| 1 + tape.u8() % 2).collect(),
            },
            5 => Block::Xor {
                take_guarded: tape.bool(),
                guarded_len: 1 + tape.u8() % 2,
            },
            6 => Block::Boundary {
                interrupting: tape.bool(),
            },
            _ => Block::Mi {
                collection_len: tape.u8() % 5,
            },
        });
    }
    Shape { blocks }
}

// ─── Emission: shape → (XML, per-task bounds, start payload) ─────────

pub struct GeneratedProcess {
    pub xml: String,
    /// task_type → max distinct job keys this shape can legally produce.
    pub bounds: BTreeMap<String, u32>,
    /// Start payload carrying every XOR flag and MI collection.
    pub payload: String,
}

fn service_task(elements: &mut String, id: &str) {
    let _ = write!(
        elements,
        r#"    <bpmn:serviceTask id="{id}"><bpmn:extensionElements><zeebe:taskDefinition type="{id}"/></bpmn:extensionElements></bpmn:serviceTask>
"#
    );
}

pub fn emit_process(shape: &Shape) -> GeneratedProcess {
    let mut elements = String::new();
    let mut flows = String::new();
    let mut bounds = BTreeMap::new();
    let mut payload_fields: Vec<String> = Vec::new();
    let mut flow_n = 0usize;
    let mut prev = "start".to_string();
    let flow = |flows: &mut String, n: &mut usize, from: &str, to: &str, condition: Option<&str>| {
        *n += 1;
        match condition {
            None => {
                let _ = write!(
                    flows,
                    r#"    <bpmn:sequenceFlow id="f{n}" sourceRef="{from}" targetRef="{to}"/>
"#,
                    n = *n
                );
            }
            Some(expr) => {
                let _ = write!(
                    flows,
                    r#"    <bpmn:sequenceFlow id="f{n}" sourceRef="{from}" targetRef="{to}">
      <bpmn:conditionExpression>{expr}</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
"#,
                    n = *n
                );
            }
        }
    };

    for (block_n, block) in shape.blocks.iter().enumerate() {
        match block {
            Block::Task => {
                let id = format!("t{block_n}");
                service_task(&mut elements, &id);
                bounds.insert(id.clone(), 1);
                flow(&mut flows, &mut flow_n, &prev, &id, None);
                prev = id;
            }
            Block::And { branches } => {
                let fork = format!("and{block_n}f");
                let join = format!("and{block_n}j");
                let _ = write!(
                    elements,
                    r#"    <bpmn:parallelGateway id="{fork}" gatewayDirection="Diverging"/>
    <bpmn:parallelGateway id="{join}" gatewayDirection="Converging"/>
"#
                );
                flow(&mut flows, &mut flow_n, &prev, &fork, None);
                for (branch_n, task_count) in branches.iter().enumerate() {
                    let mut branch_prev = fork.clone();
                    for task_n in 0..*task_count {
                        let id = format!("t{block_n}b{branch_n}x{task_n}");
                        service_task(&mut elements, &id);
                        bounds.insert(id.clone(), 1);
                        flow(&mut flows, &mut flow_n, &branch_prev, &id, None);
                        branch_prev = id;
                    }
                    flow(&mut flows, &mut flow_n, &branch_prev, &join, None);
                }
                prev = join;
            }
            Block::Xor { take_guarded, guarded_len } => {
                let split = format!("xor{block_n}");
                let merge = format!("m{block_n}");
                let flag = format!("g{block_n}");
                let _ = write!(elements, r#"    <bpmn:exclusiveGateway id="{split}"/>
"#);
                service_task(&mut elements, &merge);
                bounds.insert(merge.clone(), 1);
                payload_fields.push(format!(r#""{flag}":{take_guarded}"#));
                flow(&mut flows, &mut flow_n, &prev, &split, None);
                let mut branch_prev = split.clone();
                for task_n in 0..*guarded_len {
                    let id = format!("t{block_n}g{task_n}");
                    service_task(&mut elements, &id);
                    bounds.insert(id.clone(), u32::from(*take_guarded));
                    let condition = (task_n == 0).then(|| format!("= {flag} == true"));
                    flow(&mut flows, &mut flow_n, &branch_prev, &id, condition.as_deref());
                    branch_prev = id;
                }
                flow(&mut flows, &mut flow_n, &branch_prev, &merge, None);
                flow(&mut flows, &mut flow_n, &split, &merge, None); // default
                prev = merge;
            }
            Block::Boundary { interrupting } => {
                let host = format!("h{block_n}");
                let timer = format!("bt{block_n}");
                let handler = format!("r{block_n}");
                let handler_end = format!("be{block_n}");
                service_task(&mut elements, &host);
                service_task(&mut elements, &handler);
                bounds.insert(host.clone(), 1);
                bounds.insert(handler.clone(), 1);
                let cancel_activity = if *interrupting { "true" } else { "false" };
                let _ = write!(
                    elements,
                    r#"    <bpmn:boundaryEvent id="{timer}" attachedToRef="{host}" cancelActivity="{cancel_activity}">
      <bpmn:timerEventDefinition><bpmn:timeDuration>PT1S</bpmn:timeDuration></bpmn:timerEventDefinition>
    </bpmn:boundaryEvent>
    <bpmn:endEvent id="{handler_end}"/>
"#
                );
                flow(&mut flows, &mut flow_n, &prev, &host, None);
                flow(&mut flows, &mut flow_n, &timer, &handler, None);
                flow(&mut flows, &mut flow_n, &handler, &handler_end, None);
                prev = host;
            }
            Block::Mi { collection_len } => {
                let id = format!("mi{block_n}");
                let flag = format!("c{block_n}");
                let _ = write!(
                    elements,
                    r#"    <bpmn:serviceTask id="{id}">
      <bpmn:extensionElements><zeebe:taskDefinition type="{id}"/></bpmn:extensionElements>
      <bpmn:multiInstanceLoopCharacteristics isSequential="false">
        <bpmn:extensionElements><zeebe:loopCharacteristics inputCollection="{flag}" maxInstances="4"/></bpmn:extensionElements>
      </bpmn:multiInstanceLoopCharacteristics>
    </bpmn:serviceTask>
"#
                );
                bounds.insert(id.clone(), u32::from(*collection_len));
                let items: Vec<String> = (0..*collection_len).map(|i| i.to_string()).collect();
                payload_fields.push(format!(r#""{flag}":[{}]"#, items.join(",")));
                flow(&mut flows, &mut flow_n, &prev, &id, None);
                prev = id;
            }
        }
    }
    flow(&mut flows, &mut flow_n, &prev, "end", None);

    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="fuzz_graph" isExecutable="true">
    <bpmn:startEvent id="start"/>
{elements}    <bpmn:endEvent id="end"/>
{flows}  </bpmn:process>
</bpmn:definitions>"#
    );
    let payload = format!("{{{}}}", payload_fields.join(","));
    GeneratedProcess { xml, bounds, payload }
}

// ─── G-T conservation tracker ────────────────────────────────────────

#[derive(Default)]
pub struct ConservationTracker {
    seen: BTreeMap<String, BTreeSet<String>>,
}

impl ConservationTracker {
    /// Record an observed activation; `Err` iff the task's distinct-key
    /// count exceeds its shape-derived bound (a duplicated token, or a
    /// route the flags prove untaken).
    pub fn record(
        &mut self,
        task_type: &str,
        job_key: &str,
        bounds: &BTreeMap<String, u32>,
    ) -> Result<(), String> {
        let Some(bound) = bounds.get(task_type) else {
            return Err(format!("task '{task_type}' activated but is not in the generated shape"));
        };
        let keys = self.seen.entry(task_type.to_string()).or_default();
        keys.insert(job_key.to_string());
        if keys.len() as u32 > *bound {
            return Err(format!(
                "task '{task_type}' has {} distinct job keys, shape bound is {bound}",
                keys.len()
            ));
        }
        Ok(())
    }
}

// ─── Drive loop ──────────────────────────────────────────────────────

pub async fn drive_graph(data: &[u8]) {
    let mut tape = Tape::new(data);
    let shape = gen_shape(&mut tape);
    let generated = emit_process(&shape);

    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let clock = Arc::new(FuzzClock::new());
    let engine =
        BpmnLiteEngine::new_with_runtime_context(store, TenantId::default(), clock.clone());

    // G-A: the grammar emits only legal SESE graphs — a rejection is a
    // compiler/lowering finding. The shape is in the panic payload.
    let compiled = engine.compile(&generated.xml).await.unwrap_or_else(|error| {
        panic!("G-A: generated SESE graph must compile: {error}\nshape: {shape:?}\n{}", generated.xml)
    });

    let mut current_hash = EffectId::content_hash(generated.payload.as_bytes());
    let Ok(instance_id) = engine
        .start(
            "fuzz_graph",
            compiled.bytecode_version,
            &generated.payload,
            current_hash,
            "corr-graph",
        )
        .await
    else {
        return;
    };

    let mut tracker = ConservationTracker::default();
    let mut job_keys: Vec<String> = Vec::new();
    let steps = 8 + usize::from(tape.u8() % 17);
    for _ in 0..steps {
        match tape.u8() % 11 {
            0..=3 => {
                if let Ok(activations) = engine.run_instance(instance_id).await {
                    for job in &activations {
                        // G-T: shape-derived conservation.
                        if let Err(violation) =
                            tracker.record(&job.task_type, &job.job_key, &generated.bounds)
                        {
                            panic!("G-T: token conservation violated: {violation}\nshape: {shape:?}");
                        }
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
                let hash = if tape.bool() { current_hash } else { [tape.u8(); 32] };
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
                // Tape-driven time jump (0..=25.5s, 100ms grains) so the
                // PT1S boundary timers straddle their due edge in-exec.
                clock.advance(i64::from(tape.u8()) * 100);
                let _ = engine.tick_instance(instance_id).await;
            }
            8 => {
                clock.advance(i64::from(tape.u8()) * 100);
                let _ = engine.tick_all().await;
            }
            9 => {
                let _ = engine.inspect(instance_id).await;
            }
            _ => {
                let _ = engine.cancel(instance_id, "fuzz mid-run cancel").await;
            }
        }
    }

    // E-O5 (re-asserted at this tier): non-terminal ⇒ cancellable.
    if let Ok(inspection) = engine.inspect(instance_id).await {
        if !inspection.state.is_terminal() {
            engine
                .cancel(instance_id, "fuzz final cancel")
                .await
                .expect("E-O5: cancel rejected on a non-terminal instance");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg_bytes(seed: &mut u64, len: usize) -> Vec<u8> {
        (0..len)
            .map(|_| {
                *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (*seed >> 33) as u8
            })
            .collect()
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build test runtime")
    }

    /// G-A green receipt: every graph the grammar emits over a
    /// deterministic tape population compiles through the real compiler.
    /// A failure here names the shape — it is either a grammar overreach
    /// (fix the grammar, record the constraint) or a genuine lowering
    /// finding (surface it); never silence it.
    #[test]
    fn every_generated_graph_compiles() {
        let mut seed: u64 = 0xA076_1D64_78BD_642F;
        runtime().block_on(async {
            for _ in 0..100 {
                let bytes = lcg_bytes(&mut seed, 64);
                let mut tape = Tape::new(&bytes);
                let shape = gen_shape(&mut tape);
                let generated = emit_process(&shape);
                let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
                let engine = BpmnLiteEngine::new_with_runtime_context(
                    store,
                    TenantId::default(),
                    Arc::new(FuzzClock::new()),
                );
                if let Err(error) = engine.compile(&generated.xml).await {
                    panic!("G-A red: shape failed to compile: {error}\nshape: {shape:?}\n{}", generated.xml);
                }
            }
        });
    }

    /// G-A red receipt: the oracle can actually fail — a dangling flow
    /// target must be rejected by the same compile path.
    #[test]
    fn broken_graph_is_rejected() {
        let generated = emit_process(&Shape { blocks: vec![Block::Task] });
        let broken = generated.xml.replace(r#"targetRef="end""#, r#"targetRef="nowhere""#);
        assert_ne!(generated.xml, broken, "tamper must change the XML");
        runtime().block_on(async {
            let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
            let engine = BpmnLiteEngine::new_with_runtime_context(
                store,
                TenantId::default(),
                Arc::new(FuzzClock::new()),
            );
            assert!(
                engine.compile(&broken).await.is_err(),
                "a dangling flow target must not compile"
            );
        });
    }

    /// G-T bounds: a known shape produces the expected per-task bounds —
    /// guarded-XOR tasks 0 when the flag is false, MI = collection len.
    #[test]
    fn bounds_derive_from_shape() {
        let shape = Shape {
            blocks: vec![
                Block::Task,
                Block::Xor { take_guarded: false, guarded_len: 2 },
                Block::Mi { collection_len: 3 },
                Block::And { branches: vec![1, 2] },
            ],
        };
        let generated = emit_process(&shape);
        assert_eq!(generated.bounds.get("t0"), Some(&1));
        assert_eq!(generated.bounds.get("t1g0"), Some(&0), "untaken guarded branch");
        assert_eq!(generated.bounds.get("t1g1"), Some(&0));
        assert_eq!(generated.bounds.get("m1"), Some(&1), "merge reached via default");
        assert_eq!(generated.bounds.get("mi2"), Some(&3), "MI = collection len");
        assert_eq!(generated.bounds.get("t3b0x0"), Some(&1));
        assert_eq!(generated.bounds.get("t3b1x1"), Some(&1));
        assert_eq!(generated.payload, r#"{"g1":false,"c2":[0,1,2]}"#);
    }

    /// G-T red→green receipt: a second DISTINCT key at a bound-1 task is
    /// flagged; the same key re-observed (retry / redelivery) is not; a
    /// task outside the shape is flagged.
    #[test]
    fn conservation_tracker_flags_duplicated_tokens_only() {
        let bounds: BTreeMap<String, u32> =
            [("t0".to_string(), 1), ("g".to_string(), 0)].into();
        let mut tracker = ConservationTracker::default();
        assert!(tracker.record("t0", "i:t0:3:0", &bounds).is_ok(), "green: first activation");
        assert!(tracker.record("t0", "i:t0:3:0", &bounds).is_ok(), "green: redelivery, same key");
        assert!(tracker.record("t0", "i:t0:3:1", &bounds).is_err(), "red: duplicated token");
        assert!(tracker.record("g", "i:g:9:0", &bounds).is_err(), "red: untaken route activated");
        assert!(tracker.record("ghost", "i:ghost:1:0", &bounds).is_err(), "red: task outside shape");
    }

    /// Green half of the drive loop itself: benign tapes step through the
    /// full oracle set without tripping.
    #[test]
    fn generated_graphs_step_clean_under_oracles() {
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        runtime().block_on(async {
            for _ in 0..25 {
                let bytes = lcg_bytes(&mut seed, 256);
                drive_graph(&bytes).await;
            }
        });
    }
}
