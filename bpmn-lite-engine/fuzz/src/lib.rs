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
//!                        collection length; folded MULTIPLICATIVELY
//!                        through nesting — an untaken guard zeroes every
//!                        task inside it). Sound because job keys are
//!                        `{instance}:{task_id}:{pc}:{loop_epoch}` —
//!                        stable across retries and lease-expiry
//!                        redeliveries — so a SECOND distinct key at a
//!                        bound-1 task is a duplicated token, never
//!                        at-least-once delivery. Subsumes E-O3: the
//!                        guarded branch's bound is 0 when its flag is
//!                        false.
//!
//! Grammar scope (2026-07-26 — v2 nesting widening landed):
//! - Gateways NEST: an AND branch or XOR guarded branch is itself a nested
//!   SESE region (1-3 blocks), recursively, up to `MAX_DEPTH` levels, with
//!   `BLOCK_BUDGET` a hard ceiling on total emitted blocks. This is the
//!   shape family where the compiler's dominance-based pairing/region
//!   logic does its real work; a legal-SESE compile rejection at depth is
//!   the finding this widening exists to catch (surfaced by G-A, never
//!   silenced). Leaf blocks: Task / Boundary-timer / parallel-MI.
//! - Boundary-timer blocks emit at the TOP-LEVEL region only: a boundary
//!   handler routes to its own end event, which escapes an enclosing
//!   parallel branch and leaves that branch's join barrier open forever
//!   (V-1) — a real SESE constraint the compiler enforces, not a defect
//!   (cemented in `boundary_in_parallel_branch_is_correctly_rejected`).
//!   Nested AND/XOR/MI have no such escape and nest freely.
//! - A both-paths XOR tear with the EMPTY default branch can be masked at
//!   the merge task by job-key dedupe (the key excludes fiber id); the
//!   two-sided catch needs a task-bearing default branch (pending a
//!   compiler receipt for that shape). Flag-false guarded activation IS
//!   caught (bound 0, folded through nesting).
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

pub mod covering;

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

/// Max gateway-nesting depth: a gateway may contain gateways up to this
/// many levels deep inside its branches (v2 widening, 2026-07-26).
const MAX_DEPTH: u8 = 3;
/// Hard ceiling on emitted blocks across the whole tree — a backstop
/// against a tape that picks maximal fan-out at every level. Combined with
/// tape exhaustion (an exhausted tape reads 0 → Task), most graphs stay
/// modest and the deep tail is reachable but rare.
const BLOCK_BUDGET: u32 = 24;

#[derive(Debug, Clone)]
pub enum Block {
    /// One plain service task.
    Task,
    /// Parallel fork/join; each branch is a nested SESE region (≥1 block).
    And { branches: Vec<Vec<Block>> },
    /// Exclusive split: a nested guarded region behind `= g{n} == true`,
    /// empty default flow, both reconverging on a shared merge task (the
    /// corpus-proven no-converging-gateway shape).
    Xor { take_guarded: bool, guarded: Vec<Block> },
    /// Host task with an attached PT1S boundary timer and a handler task
    /// routed to its own end event. Leaf (no nesting inside).
    Boundary { interrupting: bool },
    /// Parallel multi-instance task over a payload collection
    /// (`maxInstances="4"`); length 0 probes the V-11 zero-match rule. Leaf.
    Mi { collection_len: u8 },
}

#[derive(Debug, Clone)]
pub struct Shape {
    pub blocks: Vec<Block>,
}

pub fn gen_shape(tape: &mut Tape) -> Shape {
    let mut budget = BLOCK_BUDGET;
    Shape {
        blocks: gen_blocks(tape, MAX_DEPTH, &mut budget),
    }
}

/// A region: 1-3 blocks, each drawn at `depth`. Always ≥1 block so no
/// gateway branch is empty (an empty branch is a distinct shape with no
/// compiler receipt yet).
fn gen_blocks(tape: &mut Tape, depth: u8, budget: &mut u32) -> Vec<Block> {
    let count = 1 + tape.u8() % 3;
    (0..count).map(|_| gen_block(tape, depth, budget)).collect()
}

fn gen_block(tape: &mut Tape, depth: u8, budget: &mut u32) -> Block {
    // Budget exhausted → force a leaf, no decrement, no further branching.
    if *budget == 0 {
        return Block::Task;
    }
    *budget -= 1;
    match tape.u8() % 8 {
        // Gateway blocks only while depth remains; branches recurse one
        // level shallower. When depth is exhausted these selectors fall
        // through to a plain task.
        3 | 4 if depth > 0 => Block::And {
            branches: (0..(2 + tape.u8() % 2))
                .map(|_| gen_blocks(tape, depth - 1, budget))
                .collect(),
        },
        5 if depth > 0 => Block::Xor {
            take_guarded: tape.bool(),
            guarded: gen_blocks(tape, depth - 1, budget),
        },
        // Boundary ONLY at the top-level region (depth == MAX_DEPTH): its
        // handler routes to a separate end event, which escapes any
        // enclosing parallel branch and leaves that branch's join barrier
        // permanently open (V-1) — a legal-SESE violation the compiler
        // correctly rejects, proven in
        // `boundary_in_parallel_branch_is_correctly_rejected`. So a nested
        // selector-6 degrades to a plain task instead.
        6 if depth == MAX_DEPTH => Block::Boundary {
            interrupting: tape.bool(),
        },
        7 => Block::Mi {
            collection_len: tape.u8() % 5,
        },
        _ => Block::Task,
    }
}

// ─── Emission: shape → (XML, per-task bounds, start payload) ─────────

pub struct GeneratedProcess {
    pub xml: String,
    /// task_type → max distinct job keys this shape can legally produce.
    pub bounds: BTreeMap<String, u32>,
    /// Start payload carrying every XOR flag and MI collection.
    pub payload: String,
}

/// Emission scratch state. A single monotonic `uid` gives every node a
/// globally-unique id (block-index numbering is not unique once branches
/// nest).
#[derive(Default)]
struct EmitCtx {
    elements: String,
    flows: String,
    bounds: BTreeMap<String, u32>,
    payload_fields: Vec<String>,
    flow_n: usize,
    uid: usize,
}

impl EmitCtx {
    fn uid(&mut self) -> usize {
        self.uid += 1;
        self.uid
    }

    fn flow(&mut self, from: &str, to: &str, condition: Option<&str>) {
        self.flow_n += 1;
        let n = self.flow_n;
        match condition {
            None => {
                let _ = write!(
                    self.flows,
                    r#"    <bpmn:sequenceFlow id="f{n}" sourceRef="{from}" targetRef="{to}"/>
"#
                );
            }
            Some(expr) => {
                let _ = write!(
                    self.flows,
                    r#"    <bpmn:sequenceFlow id="f{n}" sourceRef="{from}" targetRef="{to}">
      <bpmn:conditionExpression>{expr}</bpmn:conditionExpression>
    </bpmn:sequenceFlow>
"#
                );
            }
        }
    }

    fn service_task(&mut self, id: &str) {
        let _ = write!(
            self.elements,
            r#"    <bpmn:serviceTask id="{id}"><bpmn:extensionElements><zeebe:taskDefinition type="{id}"/></bpmn:extensionElements></bpmn:serviceTask>
"#
        );
    }
}

/// Emit one block's internal nodes/flows; return `(entry, exit)` — the
/// node the enclosing region flows INTO and the node it flows OUT OF on
/// the main control path. `mult` is the max distinct activations any task
/// in this block can legally see given the enclosing routing (1 normally,
/// 0 inside an untaken guarded branch, ×collection under MI), folded
/// multiplicatively as regions nest.
fn emit_block(block: &Block, mult: u32, ctx: &mut EmitCtx) -> (String, String) {
    match block {
        Block::Task => {
            let id = format!("t{}", ctx.uid());
            ctx.service_task(&id);
            ctx.bounds.insert(id.clone(), mult);
            (id.clone(), id)
        }
        Block::And { branches } => {
            let u = ctx.uid();
            let fork = format!("af{u}");
            let join = format!("aj{u}");
            let _ = write!(
                ctx.elements,
                r#"    <bpmn:parallelGateway id="{fork}" gatewayDirection="Diverging"/>
    <bpmn:parallelGateway id="{join}" gatewayDirection="Converging"/>
"#
            );
            // Each branch runs exactly once → mult unchanged.
            for branch in branches {
                emit_region(branch, &fork, &join, None, mult, ctx);
            }
            (fork, join)
        }
        Block::Xor {
            take_guarded,
            guarded,
        } => {
            let u = ctx.uid();
            let split = format!("xs{u}");
            let merge = format!("xm{u}");
            let flag = format!("g{u}");
            let _ = write!(
                ctx.elements,
                r#"    <bpmn:exclusiveGateway id="{split}"/>
"#
            );
            ctx.service_task(&merge);
            ctx.bounds.insert(merge.clone(), mult); // merge always reached
            ctx.payload_fields.push(format!(r#""{flag}":{take_guarded}"#));
            // Guarded region: zeroed out when the flag is false, folded
            // through the enclosing mult otherwise.
            let guarded_mult = mult * u32::from(*take_guarded);
            let condition = format!("= {flag} == true");
            emit_region(guarded, &split, &merge, Some(&condition), guarded_mult, ctx);
            ctx.flow(&split, &merge, None); // empty default flow
            (split, merge)
        }
        Block::Boundary { interrupting } => {
            let u = ctx.uid();
            let host = format!("h{u}");
            let handler = format!("r{u}");
            let handler_end = format!("be{u}");
            let timer = format!("bt{u}");
            ctx.service_task(&host);
            ctx.service_task(&handler);
            ctx.bounds.insert(host.clone(), mult);
            ctx.bounds.insert(handler.clone(), mult);
            let cancel_activity = if *interrupting { "true" } else { "false" };
            let _ = write!(
                ctx.elements,
                r#"    <bpmn:boundaryEvent id="{timer}" attachedToRef="{host}" cancelActivity="{cancel_activity}">
      <bpmn:timerEventDefinition><bpmn:timeDuration>PT1S</bpmn:timeDuration></bpmn:timerEventDefinition>
    </bpmn:boundaryEvent>
    <bpmn:endEvent id="{handler_end}"/>
"#
            );
            ctx.flow(&timer, &handler, None);
            ctx.flow(&handler, &handler_end, None);
            (host.clone(), host)
        }
        Block::Mi { collection_len } => {
            let u = ctx.uid();
            let id = format!("mi{u}");
            let flag = format!("c{u}");
            let _ = write!(
                ctx.elements,
                r#"    <bpmn:serviceTask id="{id}">
      <bpmn:extensionElements><zeebe:taskDefinition type="{id}"/></bpmn:extensionElements>
      <bpmn:multiInstanceLoopCharacteristics isSequential="false">
        <bpmn:extensionElements><zeebe:loopCharacteristics inputCollection="{flag}" maxInstances="4"/></bpmn:extensionElements>
      </bpmn:multiInstanceLoopCharacteristics>
    </bpmn:serviceTask>
"#
            );
            ctx.bounds.insert(id.clone(), mult * u32::from(*collection_len));
            let items: Vec<String> = (0..*collection_len).map(|i| i.to_string()).collect();
            ctx.payload_fields
                .push(format!(r#""{flag}":[{}]"#, items.join(",")));
            (id.clone(), id)
        }
    }
}

/// Chain a region's blocks `entry → b1 → … → bN → exit`. `entry_condition`
/// decorates only the first flow (an XOR guarded branch carries its guard
/// on the flow leaving the split).
fn emit_region(
    blocks: &[Block],
    entry: &str,
    exit: &str,
    entry_condition: Option<&str>,
    mult: u32,
    ctx: &mut EmitCtx,
) {
    if blocks.is_empty() {
        ctx.flow(entry, exit, entry_condition);
        return;
    }
    let mut prev_exit = entry.to_string();
    for (i, block) in blocks.iter().enumerate() {
        let (b_entry, b_exit) = emit_block(block, mult, ctx);
        let cond = if i == 0 { entry_condition } else { None };
        ctx.flow(&prev_exit, &b_entry, cond);
        prev_exit = b_exit;
    }
    ctx.flow(&prev_exit, exit, None);
}

pub fn emit_process(shape: &Shape) -> GeneratedProcess {
    let mut ctx = EmitCtx::default();
    emit_region(&shape.blocks, "start", "end", None, 1, &mut ctx);
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="fuzz_graph" isExecutable="true">
    <bpmn:startEvent id="start"/>
{elements}    <bpmn:endEvent id="end"/>
{flows}  </bpmn:process>
</bpmn:definitions>"#,
        elements = ctx.elements,
        flows = ctx.flows,
    );
    let payload = format!("{{{}}}", ctx.payload_fields.join(","));
    GeneratedProcess {
        xml,
        bounds: ctx.bounds,
        payload,
    }
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
            return Err(format!(
                "task '{task_type}' activated but is not in the generated shape"
            ));
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
    drive_shape(&shape, &mut tape).await;
}

/// Drive one shape with the remaining tape as runtime dynamics — split
/// from `drive_graph` so the covering corpus (F7) can drive an ENUMERATED
/// shape directly, without round-tripping through tape decoding.
pub async fn drive_shape(shape: &Shape, tape: &mut Tape<'_>) {
    let generated = emit_process(shape);

    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let clock = Arc::new(FuzzClock::new());
    let engine =
        BpmnLiteEngine::new_with_runtime_context(store, TenantId::default(), clock.clone());

    // G-A: the grammar emits only legal SESE graphs — a rejection is a
    // compiler/lowering finding. The shape is in the panic payload.
    let compiled = engine.compile(&generated.xml).await.unwrap_or_else(|error| {
        panic!(
            "G-A: generated SESE graph must compile: {error}\nshape: {shape:?}\n{}",
            generated.xml
        )
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
                            panic!(
                                "G-T: token conservation violated: {violation}\nshape: {shape:?}"
                            );
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
                *seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
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

    /// Deepest gateway nesting reached by a shape — a receipt that the
    /// widened grammar actually produces nested gateways, not just deeper
    /// linear chains.
    fn max_gateway_depth(blocks: &[Block]) -> u8 {
        blocks
            .iter()
            .map(|block| match block {
                Block::And { branches } => {
                    1 + branches.iter().map(|b| max_gateway_depth(b)).max().unwrap_or(0)
                }
                Block::Xor { guarded, .. } => 1 + max_gateway_depth(guarded),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }

    /// G-A green receipt: every graph the grammar emits over a
    /// deterministic tape population compiles through the real compiler.
    /// A failure here names the shape — it is either a grammar overreach
    /// (fix the grammar, record the constraint) or a genuine lowering
    /// finding (surface it); never silence it. Also asserts the widened
    /// grammar actually exercises gateway NESTING, not just flat chains.
    #[test]
    fn every_generated_graph_compiles() {
        let mut seed: u64 = 0xA076_1D64_78BD_642F;
        let mut deepest = 0u8;
        runtime().block_on(async {
            for _ in 0..200 {
                let bytes = lcg_bytes(&mut seed, 96);
                let mut tape = Tape::new(&bytes);
                let shape = gen_shape(&mut tape);
                deepest = deepest.max(max_gateway_depth(&shape.blocks));
                let generated = emit_process(&shape);
                let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
                let engine = BpmnLiteEngine::new_with_runtime_context(
                    store,
                    TenantId::default(),
                    Arc::new(FuzzClock::new()),
                );
                if let Err(error) = engine.compile(&generated.xml).await {
                    panic!(
                        "G-A red: shape failed to compile: {error}\nshape: {shape:?}\n{}",
                        generated.xml
                    );
                }
            }
        });
        assert!(
            deepest >= 2,
            "widened grammar never nested a gateway ≥2 deep (max {deepest}) — nesting not exercised"
        );
    }

    /// G-A red receipt: the oracle can actually fail — a dangling flow
    /// target must be rejected by the same compile path.
    #[test]
    fn broken_graph_is_rejected() {
        let generated = emit_process(&Shape {
            blocks: vec![Block::Task],
        });
        let broken = generated
            .xml
            .replace(r#"targetRef="end""#, r#"targetRef="nowhere""#);
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

    /// G-T bounds derive from the shape under the new uid id scheme —
    /// guarded-XOR tasks 0 when the flag is false, MI = collection len,
    /// AND branch tasks 1. Preorder uid assignment makes the ids exact.
    #[test]
    fn bounds_derive_from_shape() {
        let shape = Shape {
            blocks: vec![
                Block::Task,                                                  // uid 1 → t1
                Block::Xor { take_guarded: false, guarded: vec![Block::Task] }, // uid 2 (xm2,g2), inner t3
                Block::Mi { collection_len: 3 },                             // uid 4 → mi4,c4
                Block::And { branches: vec![vec![Block::Task], vec![Block::Task]] }, // uid 5, t6/t7
            ],
        };
        let generated = emit_process(&shape);
        assert_eq!(generated.bounds.get("t1"), Some(&1));
        assert_eq!(generated.bounds.get("xm2"), Some(&1), "merge reached via default");
        assert_eq!(generated.bounds.get("t3"), Some(&0), "untaken guarded task");
        assert_eq!(generated.bounds.get("mi4"), Some(&3), "MI = collection len");
        assert_eq!(generated.bounds.get("t6"), Some(&1));
        assert_eq!(generated.bounds.get("t7"), Some(&1));
        assert_eq!(generated.payload, r#"{"g2":false,"c4":[0,1,2]}"#);
    }

    /// The v2 widening's load-bearing property: an untaken guard zeroes
    /// every task NESTED inside it, folded multiplicatively — a nested AND
    /// under a false guard has both its branch tasks bounded at 0.
    #[test]
    fn nested_bound_folds_through_untaken_guard() {
        let shape = Shape {
            blocks: vec![Block::Xor {
                take_guarded: false,
                guarded: vec![Block::And {
                    branches: vec![vec![Block::Task], vec![Block::Task]],
                }],
            }],
        };
        let generated = emit_process(&shape);
        // uid 1 = Xor (xm1 merge, g1 flag); uid 2 = And (af2/aj2);
        // uid 3/4 = the two branch tasks, both under the false guard.
        assert_eq!(generated.bounds.get("xm1"), Some(&1), "merge still reached");
        assert_eq!(generated.bounds.get("t3"), Some(&0), "nested task under false guard");
        assert_eq!(generated.bounds.get("t4"), Some(&0), "nested task under false guard");
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

    /// The grammar's Boundary-placement constraint, cemented: a boundary
    /// event inside a parallel AND branch is correctly rejected — its
    /// handler routes to a separate end event, escaping the branch, so the
    /// parallel join's barrier can never close (V-1). A real SESE
    /// constraint, not a compiler defect; the grammar must never emit it.
    #[test]
    fn boundary_in_parallel_branch_is_correctly_rejected() {
        runtime().block_on(async {
            async fn compiles(shape: Shape) -> bool {
                let generated = emit_process(&shape);
                let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
                let engine = BpmnLiteEngine::new_with_runtime_context(
                    store,
                    TenantId::default(),
                    Arc::new(FuzzClock::new()),
                );
                engine.compile(&generated.xml).await.is_ok()
            }
            assert!(
                !compiles(Shape {
                    blocks: vec![Block::And {
                        branches: vec![
                            vec![Block::Boundary { interrupting: false }],
                            vec![Block::Task],
                        ],
                    }],
                })
                .await,
                "boundary inside an AND branch must be rejected (barrier escape)"
            );
            assert!(
                compiles(Shape {
                    blocks: vec![Block::Boundary { interrupting: false }],
                })
                .await,
                "top-level boundary must compile"
            );
        });
    }
}
