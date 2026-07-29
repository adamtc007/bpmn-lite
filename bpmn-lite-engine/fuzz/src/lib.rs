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
//! Grammar scope (2026-07-26 — v2 nesting widening + v3 alphabet
//! widening landed):
//! - Gateways NEST: an AND/OR branch or XOR region is itself a nested
//!   SESE region (1-3 blocks), recursively, up to `MAX_DEPTH` levels, with
//!   `BLOCK_BUDGET` a hard ceiling on total emitted blocks. This is the
//!   shape family where the compiler's dominance-based pairing/region
//!   logic does its real work; a legal-SESE compile rejection at depth is
//!   the finding this widening exists to catch (surfaced by G-A, never
//!   silenced).
//! - XOR carries an optional TASK-BEARING default region: the untaken
//!   side's bound is 0 in BOTH directions (guarded when the flag is
//!   false, default when it is true) — the two-sided tear catch the
//!   empty-default shape masked via merge-task job-key dedupe. Receipt:
//!   `xor_default_and_or_bounds_are_two_sided`,
//!   `routing_follows_delivered_flags`.
//! - OR (inclusiveGateway) is a gateway letter: 2-3 branches, each with
//!   its own activation flag; named-subset outcomes {both, one, none} —
//!   all-false is the ruling-J zero-match probe (incident, not skip, not
//!   crash; cemented in `routing_follows_delivered_flags`).
//! - Boundary-timer blocks emit wherever NO And/Or ANCESTOR exists
//!   (`under_barrier` in the grammar): a boundary handler routes to its
//!   own end event, which escapes an enclosing synchronizing barrier and
//!   leaves its join open forever (V-1). XOR is not a barrier, so
//!   Boundary nests under XOR — but an XOR wrapper does not launder a
//!   boundary out of an enclosing AND/OR (barrier-ANCESTOR rule; full
//!   matrix cemented in `boundary_in_parallel_branch_is_correctly_rejected`).
//! - FLAG DELIVERY: the engine's flag table starts empty and the start
//!   payload is opaque domain data — routing flags are only writable via
//!   `orch_flags` (`flag_<u32>`) at job completion. Every graph therefore
//!   opens with an `init` task and the driver passes the shape's full
//!   flag-intent set on every completion. Before this (harness defect,
//!   found by the two-sided widening going red): the guarded XOR branch
//!   was UNREACHABLE in every run, invisibly, because G-T is
//!   upper-bound-only. `routing_follows_delivered_flags` is the
//!   lower-bound cement.
//! - Timers: tape-driven `FuzzClock` (shared with F5) — tick arms jump
//!   logical time, so PT1S boundary timers genuinely fire in-exec.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use bpmn_lite_engine::{BpmnLiteEngine, RuntimeContext, RuntimeContextError};
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_store::WorkflowStore;
use bpmn_lite_types::{EffectId, ErrorClass, TenantId, Timestamp, Uuid, Value};

pub mod covering;
pub mod fault;

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
    const GENESIS_MS: i64 = 1_750_000_000_000;

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
pub(crate) enum Block {
    /// One plain service task.
    Task,
    /// Parallel fork/join; each branch is a nested SESE region (≥1 block).
    And { branches: Vec<Vec<Block>> },
    /// Exclusive split: a nested guarded region behind `= g{n} == true`,
    /// reconverging with the default path on a shared merge task. The
    /// default path is empty when `default` is empty (the original
    /// corpus-proven shape) or a task-bearing nested region — the
    /// two-sided tear catch: with the guard TAKEN, the default region's
    /// bound is 0, so a token down both paths is no longer dedupe-masked
    /// at the merge.
    Xor {
        take_guarded: bool,
        guarded: Vec<Block>,
        default: Vec<Block>,
    },
    /// Inclusive (OR) split/join: every branch carries its own activation
    /// flag (`= o{n}b{i} == true`); the activated SUBSET runs and the
    /// converging inclusive gateway synchronizes exactly that subset (the
    /// named-subset interlocking). All-false probes the zero-match
    /// incident path at runtime.
    Or { branches: Vec<(bool, Vec<Block>)> },
    /// Host task with an attached PT1S boundary timer and a handler task
    /// routed to its own end event. Leaf; legal only with NO synchronizing
    /// barrier (And/Or) ancestor — the handler's end event escapes such a
    /// branch and the join barrier never closes (V-1).
    Boundary { interrupting: bool },
    /// Host task with an attached ERROR boundary (guard-error family,
    /// GUARD-ERROR> arms) and a handler task routed to its own end event.
    /// The specific arm catches errorCode "R7" — the code the drive loop
    /// deliberately throws — so match and miss are both reachable; the
    /// catch-all arm (no errorRef) catches any BusinessRejection.
    /// Interrupting only: the parser models no non-interrupting error
    /// boundary. Leaf; same barrier-ancestor rule as Boundary.
    ErrBoundary { catch_all: bool },
    /// Parallel multi-instance task over a payload collection
    /// (`maxInstances="4"`); length 0 probes the V-11 zero-match rule. Leaf.
    Mi { collection_len: u8 },
    /// Message wait (intermediateCatchEvent + content correlation, §28):
    /// the token parks until an external `signal_with_value` delivers the
    /// matching message name AND content key (`k{u}` resolved from the
    /// domain payload → "corr{u}"). The drive loop's publish action sends
    /// matching and non-matching keys — the sleeping-token-unblocked-by-
    /// external-event path, end-to-end through the compiler. Leaf; legal
    /// anywhere (no barrier escape — the token stays in its region).
    MsgWait,
}

#[derive(Debug, Clone)]
pub struct Shape {
    pub blocks: Vec<Block>,
}

pub(crate) fn gen_shape(tape: &mut Tape) -> Shape {
    let mut budget = BLOCK_BUDGET;
    Shape {
        blocks: gen_blocks(tape, MAX_DEPTH, false, &mut budget),
    }
}

/// A region: 1-3 blocks, each drawn at `depth`. Always ≥1 block so no
/// gateway branch is empty (an empty branch is a distinct shape with no
/// compiler receipt yet). `under_barrier` is true iff any ancestor is a
/// synchronizing barrier (And/Or branch) — the Boundary legality flag.
fn gen_blocks(tape: &mut Tape, depth: u8, under_barrier: bool, budget: &mut u32) -> Vec<Block> {
    let count = 1 + tape.u8() % 3;
    (0..count)
        .map(|_| gen_block(tape, depth, under_barrier, budget))
        .collect()
}

fn gen_block(tape: &mut Tape, depth: u8, under_barrier: bool, budget: &mut u32) -> Block {
    // Budget exhausted → force a leaf, no decrement, no further branching.
    if *budget == 0 {
        return Block::Task;
    }
    *budget -= 1;
    match tape.u8() % 10 {
        // Gateway blocks only while depth remains; branches recurse one
        // level shallower. When depth is exhausted these selectors fall
        // through to a plain task.
        3 | 4 if depth > 0 => Block::And {
            branches: (0..(2 + tape.u8() % 2))
                .map(|_| gen_blocks(tape, depth - 1, true, budget))
                .collect(),
        },
        5 if depth > 0 => {
            let take_guarded = tape.bool();
            let has_default = tape.bool();
            let guarded = gen_blocks(tape, depth - 1, under_barrier, budget);
            let default = if has_default {
                gen_blocks(tape, depth - 1, under_barrier, budget)
            } else {
                Vec::new()
            };
            Block::Xor {
                take_guarded,
                guarded,
                default,
            }
        }
        // Boundary iff NO synchronizing-barrier ancestor: inside an
        // And/Or branch its handler's end event escapes the branch and
        // the join barrier never closes (V-1) — the compiler correctly
        // rejects that (cemented in the legality-matrix test). Inside an
        // XOR region (no barrier) it is legal and now emitted.
        6 if !under_barrier => {
            if tape.bool() {
                Block::ErrBoundary {
                    catch_all: tape.bool(),
                }
            } else {
                Block::Boundary {
                    interrupting: tape.bool(),
                }
            }
        }
        7 => {
            if tape.bool() {
                Block::MsgWait
            } else {
                Block::Mi {
                    collection_len: tape.u8() % 5,
                }
            }
        }
        8 | 9 if depth > 0 => Block::Or {
            branches: (0..(2 + tape.u8() % 2))
                .map(|_| {
                    let active = tape.bool();
                    (active, gen_blocks(tape, depth - 1, true, budget))
                })
                .collect(),
        },
        _ => Block::Task,
    }
}

// ─── Emission: shape → (XML, per-task bounds, start payload) ─────────

struct GeneratedProcess {
    pub xml: String,
    /// task_type → max distinct job keys this shape can legally produce.
    pub bounds: BTreeMap<String, u32>,
    /// Start payload carrying every MI collection. Routing flags do NOT
    /// live here: the engine's flag table starts empty and the start
    /// payload is opaque domain data — flags are only writable through
    /// `orch_flags` at job completion (kernel `apply_completion`,
    /// `flag_<u32>` keys). The driver delivers `flag_intents` there.
    pub payload: String,
    /// Symbolic flag name → intended value (XOR `g{u}` = take_guarded,
    /// OR `o{u}b{i}` = branch active). The driver resolves names to
    /// interned `flag_<u32>` keys via the compile result's
    /// `flag_symbol_table` and passes them on EVERY `complete_job` —
    /// the emitted `init` task guarantees at least one completion lands
    /// before any split can evaluate a condition.
    pub flag_intents: BTreeMap<String, bool>,
    /// Message waits. The driver's publish action draws from this list —
    /// matching keys unblock the parked token, non-matching keys must NOT.
    pub msg_waits: Vec<MsgWaitDecl>,
}

/// One emitted message wait: `signal_with_value(msg_name, corr_value)`
/// unblocks it; `key_field` is the domain-payload field its subscription
/// resolves the expected key from.
#[derive(Debug, Clone)]
struct MsgWaitDecl {
    pub msg_name: String,
    pub key_field: String,
    pub corr_value: String,
}

/// Emission scratch state. A single monotonic `uid` gives every node a
/// globally-unique id (block-index numbering is not unique once branches
/// nest).
#[derive(Default)]
struct EmitCtx {
    elements: String,
    flows: String,
    /// Definitions-level elements (error catalog) — the parser collects
    /// `<bpmn:error>` only OUTSIDE the process, and boundary-close
    /// resolution reads the catalog mid-parse, so these must precede the
    /// process element in document order.
    definitions: String,
    bounds: BTreeMap<String, u32>,
    payload_fields: Vec<String>,
    flag_intents: BTreeMap<String, bool>,
    msg_waits: Vec<MsgWaitDecl>,
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
            default,
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
            ctx.flag_intents.insert(flag.clone(), *take_guarded);
            // Exclusive routing: exactly one path runs. Guarded region is
            // zeroed when the flag is false; the default region is zeroed
            // when the flag is TRUE — the two-sided tear catch: a token
            // down both paths now trips a bound-0 task instead of being
            // dedupe-masked at the merge.
            let guarded_mult = mult * u32::from(*take_guarded);
            let condition = format!("= {flag} == true");
            emit_region(guarded, &split, &merge, Some(&condition), guarded_mult, ctx);
            if default.is_empty() {
                ctx.flow(&split, &merge, None); // empty default flow
            } else {
                let default_mult = mult * u32::from(!*take_guarded);
                emit_region(default, &split, &merge, None, default_mult, ctx);
            }
            (split, merge)
        }
        Block::Or { branches } => {
            let u = ctx.uid();
            let fork = format!("of{u}");
            let join = format!("oj{u}");
            let _ = write!(
                ctx.elements,
                r#"    <bpmn:inclusiveGateway id="{fork}" gatewayDirection="Diverging"/>
    <bpmn:inclusiveGateway id="{join}" gatewayDirection="Converging"/>
"#
            );
            // Every branch carries its own activation flag; the activated
            // subset runs and the converging inclusive gateway
            // synchronizes exactly that subset. All-false = zero-match at
            // runtime (every branch bound 0).
            for (branch_n, (active, region)) in branches.iter().enumerate() {
                let flag = format!("o{u}b{branch_n}");
                ctx.flag_intents.insert(flag.clone(), *active);
                let condition = format!("= {flag} == true");
                let branch_mult = mult * u32::from(*active);
                emit_region(region, &fork, &join, Some(&condition), branch_mult, ctx);
            }
            (fork, join)
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
        Block::ErrBoundary { catch_all } => {
            let u = ctx.uid();
            let host = format!("h{u}");
            let handler = format!("r{u}");
            let handler_end = format!("be{u}");
            let boundary = format!("bx{u}");
            ctx.service_task(&host);
            ctx.service_task(&handler);
            ctx.bounds.insert(host.clone(), mult);
            ctx.bounds.insert(handler.clone(), mult);
            let event_definition = if *catch_all {
                // No errorRef → error_code None → catch-all arm.
                "<bpmn:errorEventDefinition/>".to_string()
            } else {
                // Specific arm: errorRef resolves through the
                // definitions-level catalog to errorCode "R7" — the code
                // the drive loop deliberately throws.
                let error_def = format!("errdef{u}");
                let _ = write!(
                    ctx.definitions,
                    r#"  <bpmn:error id="{error_def}" errorCode="R7"/>
"#
                );
                format!(r#"<bpmn:errorEventDefinition errorRef="{error_def}"/>"#)
            };
            let _ = write!(
                ctx.elements,
                r#"    <bpmn:boundaryEvent id="{boundary}" attachedToRef="{host}">
      {event_definition}
    </bpmn:boundaryEvent>
"#
            );
            ctx.flow(&boundary, &handler, None);
            ctx.flow(&handler, &handler_end, None);
            let _ = write!(
                ctx.elements,
                r#"    <bpmn:endEvent id="{handler_end}"/>
"#
            );
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
        Block::MsgWait => {
            let u = ctx.uid();
            let wait = format!("w{u}");
            let key_field = format!("k{u}");
            let msg_name = format!("msg{u}");
            let corr = format!("corr{u}");
            let _ = write!(
                ctx.elements,
                r#"    <bpmn:dataObject id="{key_field}" name="{key_field}"></bpmn:dataObject>
    <bpmn:intermediateCatchEvent id="{wait}" name="{msg_name}">
      <bpmn:messageEventDefinition messageRef="m{u}"/>
      <bpmn:extensionElements>
        <zeebe:subscription correlationKey="={key_field}"/>
      </bpmn:extensionElements>
    </bpmn:intermediateCatchEvent>
"#
            );
            // Content correlation (§28): the key resolves from the domain
            // payload at park time, so the field rides the start payload
            // AND every completion payload the driver sends.
            ctx.payload_fields
                .push(format!(r#""{key_field}":"{corr}""#));
            ctx.msg_waits.push(MsgWaitDecl {
                msg_name,
                key_field,
                corr_value: corr,
            });
            (wait.clone(), wait)
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

pub(crate) fn emit_process(shape: &Shape) -> GeneratedProcess {
    let mut ctx = EmitCtx::default();
    // Routing flags are deliverable only via a job completion's
    // `orch_flags`, so every graph opens with an `init` task: its
    // completion carries the full flag-intent set BEFORE any split can
    // evaluate a condition. Without it a leading XOR/OR reads an empty
    // flag table (LoadFlag defaults false) and routing is untestable.
    ctx.service_task("init");
    ctx.bounds.insert("init".to_string(), 1);
    ctx.flow("start", "init", None);
    emit_region(&shape.blocks, "init", "end", None, 1, &mut ctx);
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
{definitions}  <bpmn:process id="fuzz_graph" isExecutable="true">
    <bpmn:startEvent id="start"/>
{elements}    <bpmn:endEvent id="end"/>
{flows}  </bpmn:process>
</bpmn:definitions>"#,
        definitions = ctx.definitions,
        elements = ctx.elements,
        flows = ctx.flows,
    );
    let payload = format!("{{{}}}", ctx.payload_fields.join(","));
    GeneratedProcess {
        xml,
        bounds: ctx.bounds,
        payload,
        flag_intents: ctx.flag_intents,
        msg_waits: ctx.msg_waits,
    }
}

// ─── G-T conservation tracker ────────────────────────────────────────

#[derive(Default)]
pub(crate) struct ConservationTracker {
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

    // Flag delivery: resolve symbolic intents to interned `flag_<u32>`
    // keys through the compile result's symbol table; passed on EVERY
    // completion (idempotent) so routing is armed from the `init`
    // completion onward.
    let orch_flags: BTreeMap<String, Value> = compiled
        .flag_symbol_table
        .iter()
        .filter_map(|(key, name)| {
            generated
                .flag_intents
                .get(name)
                .map(|intent| (format!("flag_{key}"), Value::Bool(*intent)))
        })
        .collect();

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

    // Completion payloads carry the correlation content fields so message
    // waits that park AFTER a completion still resolve their keys (§28
    // content correlation reads the CURRENT domain payload at park time).
    let corr_fields: String = generated
        .msg_waits
        .iter()
        .map(|wait| format!(r#","{}":"{}""#, wait.key_field, wait.corr_value))
        .collect();

    let mut tracker = ConservationTracker::default();
    let mut job_keys: Vec<String> = Vec::new();
    let mut msg_seq = 0u32;
    let steps = 8 + usize::from(tape.u8() % 17);
    for _ in 0..steps {
        match tape.u8() % 12 {
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
                let result_payload = format!(r#"{{"result":{}{corr_fields}}}"#, tape.u8());
                let hash = if tape.bool() { current_hash } else { [tape.u8(); 32] };
                if engine
                    .complete_job(&key, &result_payload, hash, orch_flags.clone())
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
                let error_class = match tape.u8() % 4 {
                    0 => ErrorClass::Transient,
                    1 => ErrorClass::ContractViolation,
                    // "R7" is the code every specific error-boundary arm
                    // catches — a deliberate MATCH when the failed job is
                    // an ErrBoundary host, an unmatched rejection
                    // (incident path) anywhere else.
                    2 => ErrorClass::BusinessRejection {
                        rejection_code: "R7".to_string(),
                    },
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
            10 => {
                // External-event delivery: publish a message with a
                // matching OR non-matching content key. Matching keys
                // unblock the parked wait; non-matching keys must not
                // (content-correlation discrimination under fuzz).
                if generated.msg_waits.is_empty() {
                    continue;
                }
                let wait =
                    &generated.msg_waits[usize::from(tape.u8()) % generated.msg_waits.len()];
                let key = if tape.bool() {
                    wait.corr_value.clone()
                } else {
                    format!("junk{}", tape.u8())
                };
                msg_seq += 1;
                let msg_id = format!("m{msg_seq}");
                let _ = engine
                    .signal_with_value(
                        instance_id,
                        &wait.msg_name,
                        key,
                        None,
                        None,
                        Some(&msg_id),
                    )
                    .await;
                let _ = engine.tick_instance(instance_id).await;
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

// ─── F8.7 flag-storm driver ──────────────────────────────────────────

/// F8.7: routing flags are instance-global MUTABLE state — every
/// completion may rewrite them. The intent-faithful driver
/// (`drive_shape`) always delivers one consistent assignment; this
/// driver deliberately delivers INCONSISTENT flag histories (each
/// completion re-draws every flag from the tape) to hammer split
/// evaluation, guard rollback snapshots, and OR-subset synchronization
/// under mid-run re-routing.
///
/// Oracles are the structural subset — G-T's intent-derived bounds are
/// UNSOUND here by construction (a split evaluates whatever the flags
/// say at split time), so conservation is relaxed to shape membership:
///   S-O1 no-panic; S-O2 every activated task belongs to the generated
///   shape (an off-shape task type is a routing corruption regardless of
///   flag history); S-O3 = E-O5 final cancel on non-terminal states.
pub async fn drive_flag_storm(data: &[u8]) {
    let mut tape = Tape::new(data);
    let shape = gen_shape(&mut tape);
    let generated = emit_process(&shape);

    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let clock = Arc::new(FuzzClock::new());
    let engine =
        BpmnLiteEngine::new_with_runtime_context(store, TenantId::default(), clock.clone());
    let compiled = engine.compile(&generated.xml).await.unwrap_or_else(|error| {
        panic!("G-A red (flag-storm tier): {error}\nshape: {shape:?}")
    });
    let flag_keys: Vec<(String, String)> = compiled
        .flag_symbol_table
        .iter()
        .filter(|(_, name)| generated.flag_intents.contains_key(*name))
        .map(|(key, name)| (format!("flag_{key}"), name.clone()))
        .collect();

    let mut current_hash = EffectId::content_hash(generated.payload.as_bytes());
    let Ok(instance_id) = engine
        .start(
            "fuzz_graph",
            compiled.bytecode_version,
            &generated.payload,
            current_hash,
            "corr-flagstorm",
        )
        .await
    else {
        return;
    };

    let steps = 8 + usize::from(tape.u8() % 17);
    for _ in 0..steps {
        match tape.u8() % 8 {
            0..=3 => {
                if let Ok(activations) = engine.run_instance(instance_id).await {
                    for job in &activations {
                        // S-O2: membership, not bounds — flag histories
                        // are inconsistent on purpose.
                        assert!(
                            generated.bounds.contains_key(&job.task_type),
                            "S-O2: off-shape task '{}' activated under flag storm\nshape: {shape:?}",
                            job.task_type
                        );
                    }
                    let jobs: Vec<_> = activations.into_iter().collect();
                    for job in jobs {
                        // Every completion re-draws EVERY flag.
                        let storm_flags: BTreeMap<String, Value> = flag_keys
                            .iter()
                            .map(|(key, _)| (key.clone(), Value::Bool(tape.bool())))
                            .collect();
                        let result_payload = format!(r#"{{"s":{}}}"#, tape.u8());
                        if engine
                            .complete_job(&job.job_key, &result_payload, current_hash, storm_flags)
                            .await
                            .is_ok()
                        {
                            current_hash = EffectId::content_hash(result_payload.as_bytes());
                        }
                    }
                }
            }
            4 => {
                clock.advance(i64::from(tape.u8()) * 100);
                let _ = engine.tick_instance(instance_id).await;
            }
            5 => {
                let _ = engine.inspect(instance_id).await;
            }
            6 => {
                let _ = engine
                    .fail_job(
                        "nonexistent-key",
                        ErrorClass::Transient,
                        "flag-storm noise",
                    )
                    .await;
            }
            _ => {
                clock.advance(i64::from(tape.u8()) * 100);
                let _ = engine.tick_all().await;
            }
        }
    }

    // S-O3 = E-O5: cancel discipline holds regardless of flag history.
    if let Ok(inspection) = engine.inspect(instance_id).await {
        if !inspection.state.is_terminal() {
            engine
                .cancel(instance_id, "flag-storm final cancel")
                .await
                .expect("E-O5: cancel rejected on a non-terminal instance (flag storm)");
        }
    }
}

// ─── F8.1 raw-XML robustness driver ──────────────────────────────────

/// F8.1 (EOP-FUZZ §10): arbitrary bytes at the XML frontend. Unlike F6,
/// nothing here is grammar-legal — the parser/lowering/verifier chain is
/// the system under test against HOSTILE input.
///
/// Oracles:
///   X-O1 no-panic     — any byte sequence either compiles or is rejected
///                       with an error; a panic/abort in parse, lowering,
///                       or admission is the finding.
///   X-O2 admit-honest — an ADMITTED artifact is a promise, not a parse:
///                       whatever compile accepts must then start, step,
///                       and cancel without panic (E-O5 re-asserted on a
///                       reachable non-terminal state). This is the
///                       fail-closed complement of G-A: G-A says legal
///                       shapes must be admitted; X-O2 says whatever IS
///                       admitted must behave.
pub async fn drive_xml_compile(data: &[u8]) {
    let xml = String::from_utf8_lossy(data);
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine =
        BpmnLiteEngine::new_with_runtime_context(store, TenantId::default(), Arc::new(FuzzClock::new()));
    let Ok(compiled) = engine.compile(&xml).await else {
        return; // X-O1: rejection is the legal outcome for hostile bytes
    };
    let payload = "{}";
    let hash = EffectId::content_hash(payload.as_bytes());
    let Ok(instance_id) = engine
        .start("fuzz_xml", compiled.bytecode_version, payload, hash, "corr-xml")
        .await
    else {
        return; // start may legitimately reject (e.g. key/artifact mismatch)
    };
    for _ in 0..6 {
        let Ok(jobs) = engine.run_instance(instance_id).await else {
            break;
        };
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            let _ = engine
                .complete_job(&job.job_key, payload, hash, BTreeMap::new())
                .await;
        }
    }
    // X-O2 ∋ E-O5: an admitted artifact's non-terminal instance must be
    // cancellable — arbitrary-XML artifacts get no exemption.
    if let Ok(inspection) = engine.inspect(instance_id).await {
        if !inspection.state.is_terminal() {
            engine
                .cancel(instance_id, "xml fuzz final cancel")
                .await
                .expect("X-O2/E-O5: cancel rejected on a non-terminal instance (admitted XML)");
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
                Block::Xor { take_guarded: false, guarded: vec![Block::Task], default: vec![] }, // uid 2 (xm2,g2), inner t3
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
        assert_eq!(generated.bounds.get("init"), Some(&1), "flag-carrier task");
        // Payload carries only MI collections; routing flags travel as
        // completion orch_flags intents.
        assert_eq!(generated.payload, r#"{"c4":[0,1,2]}"#);
        assert_eq!(generated.flag_intents.get("g2"), Some(&false));
    }

    /// Two-sided XOR/OR bounds: the untaken side is bounded at 0 in BOTH
    /// directions (guarded when the flag is false, task-bearing default
    /// when the flag is true; OR branches per activation flag) — the
    /// unmasked two-sided tear catch.
    #[test]
    fn xor_default_and_or_bounds_are_two_sided() {
        // uid 1 (xs1/xm1/g1), guarded t2, default t3.
        let taken = emit_process(&Shape {
            blocks: vec![Block::Xor {
                take_guarded: true,
                guarded: vec![Block::Task],
                default: vec![Block::Task],
            }],
        });
        assert_eq!(taken.bounds.get("t2"), Some(&1), "taken guarded task");
        assert_eq!(taken.bounds.get("t3"), Some(&0), "default zeroed when guard taken");
        let untaken = emit_process(&Shape {
            blocks: vec![Block::Xor {
                take_guarded: false,
                guarded: vec![Block::Task],
                default: vec![Block::Task],
            }],
        });
        assert_eq!(untaken.bounds.get("t2"), Some(&0), "guarded zeroed when untaken");
        assert_eq!(untaken.bounds.get("t3"), Some(&1), "default runs when untaken");
        // uid 1 (of1/oj1), branch0 t2 active, branch1 t3 inactive.
        let or = emit_process(&Shape {
            blocks: vec![Block::Or {
                branches: vec![(true, vec![Block::Task]), (false, vec![Block::Task])],
            }],
        });
        assert_eq!(or.bounds.get("t2"), Some(&1), "activated OR branch");
        assert_eq!(or.bounds.get("t3"), Some(&0), "inactive OR branch");
        assert_eq!(or.flag_intents.get("o1b0"), Some(&true));
        assert_eq!(or.flag_intents.get("o1b1"), Some(&false));
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
                default: vec![],
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
            // XOR is not a synchronizing barrier: the handler's end event
            // escapes nothing — legal, and the grammar now emits it.
            assert!(
                compiles(Shape {
                    blocks: vec![Block::Xor {
                        take_guarded: true,
                        guarded: vec![Block::Boundary { interrupting: false }],
                        default: vec![],
                    }],
                })
                .await,
                "boundary inside an XOR region must compile (no barrier to escape)"
            );
            // Barrier-ANCESTOR rule, not barrier-parent: an XOR wrapper
            // does not launder a boundary out of an enclosing AND barrier.
            assert!(
                !compiles(Shape {
                    blocks: vec![Block::And {
                        branches: vec![
                            vec![Block::Xor {
                                take_guarded: true,
                                guarded: vec![Block::Boundary { interrupting: false }],
                                default: vec![],
                            }],
                            vec![Block::Task],
                        ],
                    }],
                })
                .await,
                "boundary under XOR-inside-AND must be rejected (AND ancestor barrier)"
            );
            // OR joins synchronize their activated subset — same escape
            // hazard as AND.
            assert!(
                !compiles(Shape {
                    blocks: vec![Block::Or {
                        branches: vec![
                            (true, vec![Block::Boundary { interrupting: false }]),
                            (true, vec![Block::Task]),
                        ],
                    }],
                })
                .await,
                "boundary inside an OR branch must be rejected (inclusive join barrier)"
            );
            // Error boundaries obey the same barrier-ancestor rule: the
            // handler's end event has the identical V-1 escape.
            assert!(
                compiles(Shape {
                    blocks: vec![Block::ErrBoundary { catch_all: false }],
                })
                .await,
                "top-level error boundary must compile"
            );
            assert!(
                compiles(Shape {
                    blocks: vec![Block::Xor {
                        take_guarded: true,
                        guarded: vec![Block::ErrBoundary { catch_all: true }],
                        default: vec![],
                    }],
                })
                .await,
                "error boundary inside an XOR region must compile"
            );
            assert!(
                !compiles(Shape {
                    blocks: vec![Block::And {
                        branches: vec![
                            vec![Block::ErrBoundary { catch_all: false }],
                            vec![Block::Task],
                        ],
                    }],
                })
                .await,
                "error boundary inside an AND branch must be rejected (barrier escape)"
            );
        });
    }

    /// F8.4 runtime receipt for the guard-error family: a BusinessRejection
    /// with the arm's code routes to the handler (specific arm), any code
    /// routes on a catch-all arm, and an UNMATCHED code on a specific arm
    /// raises an incident — reject, don't skip — leaving the instance
    /// cancellable.
    #[test]
    fn error_boundary_routing_matches_and_misses() {
        async fn drive(
            catch_all: bool,
            rejection_code: &str,
        ) -> (bool /* handler ran */, usize /* incidents */, bool /* terminal */) {
            let shape = Shape {
                blocks: vec![Block::ErrBoundary { catch_all }],
            };
            let generated = emit_process(&shape);
            let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
            let engine = BpmnLiteEngine::new_with_runtime_context(
                store,
                TenantId::default(),
                Arc::new(FuzzClock::new()),
            );
            let compiled = engine.compile(&generated.xml).await.expect("compile");
            let mut hash = EffectId::content_hash(generated.payload.as_bytes());
            let instance_id = engine
                .start("fuzz_graph", compiled.bytecode_version, &generated.payload, hash, "c")
                .await
                .expect("start");
            let mut handler_ran = false;
            for _ in 0..12 {
                let jobs = engine.run_instance(instance_id).await.expect("run");
                if jobs.is_empty() {
                    break;
                }
                for job in jobs {
                    // uid 1: host h1, handler r1.
                    if job.task_type == "h1" {
                        engine
                            .fail_job(
                                &job.job_key,
                                ErrorClass::BusinessRejection {
                                    rejection_code: rejection_code.to_string(),
                                },
                                "cement failure",
                            )
                            .await
                            .expect("fail host");
                    } else {
                        handler_ran |= job.task_type == "r1";
                        engine
                            .complete_job(&job.job_key, r#"{"ok":1}"#, hash, BTreeMap::new())
                            .await
                            .expect("complete");
                        hash = EffectId::content_hash(br#"{"ok":1}"#);
                    }
                }
            }
            let inspection = engine.inspect(instance_id).await.expect("inspect");
            let terminal = inspection.state.is_terminal();
            if !terminal {
                engine
                    .cancel(instance_id, "cement cancel")
                    .await
                    .expect("E-O5 on incidented instance");
            }
            (handler_ran, inspection.incidents.len(), terminal)
        }

        runtime().block_on(async {
            let (handler_ran, incidents, terminal) = drive(false, "R7").await;
            assert!(handler_ran, "specific arm must catch its own code R7");
            assert_eq!(incidents, 0, "matched rejection raises no incident");
            assert!(terminal, "handler path must run to completion");

            let (handler_ran, incidents, terminal) = drive(false, "R9").await;
            assert!(!handler_ran, "specific arm must NOT catch a foreign code");
            assert!(incidents >= 1, "unmatched rejection must raise an incident");
            assert!(!terminal, "unmatched rejection parks the instance");

            let (handler_ran, incidents, terminal) = drive(true, "R9").await;
            assert!(handler_ran, "catch-all arm must catch any code");
            assert_eq!(incidents, 0);
            assert!(terminal);
        });
    }

    /// Routing fidelity — the LOWER-bound receipt G-T (upper-bound-only)
    /// cannot give: the branch the delivered flags select actually RUNS,
    /// and the deselected branch does not. Red before the orch_flags
    /// delivery fix: the flag table stayed empty, LoadFlag defaulted
    /// false, and every XOR fell to its default — the guarded branch was
    /// unreachable in ALL runs (masked because 0 ≤ bound always passes).
    /// Also cements the OR named-subset outcomes and the zero-match
    /// incident (ruling J) live at the engine tier.
    #[test]
    fn routing_follows_delivered_flags() {
        /// Deterministically complete every activation until quiescence;
        /// distinct job keys per task_type, plus open-incident count.
        async fn run_counts(shape: &Shape) -> (BTreeMap<String, usize>, usize) {
            let generated = emit_process(shape);
            let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
            let engine = BpmnLiteEngine::new_with_runtime_context(
                store,
                TenantId::default(),
                Arc::new(FuzzClock::new()),
            );
            let compiled = engine.compile(&generated.xml).await.expect("compile");
            let orch_flags: BTreeMap<String, Value> = compiled
                .flag_symbol_table
                .iter()
                .filter_map(|(key, name)| {
                    generated
                        .flag_intents
                        .get(name)
                        .map(|intent| (format!("flag_{key}"), Value::Bool(*intent)))
                })
                .collect();
            let mut hash = EffectId::content_hash(generated.payload.as_bytes());
            let instance_id = engine
                .start(
                    "fuzz_graph",
                    compiled.bytecode_version,
                    &generated.payload,
                    hash,
                    "corr-routing",
                )
                .await
                .expect("start");
            let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for _ in 0..32 {
                let Ok(jobs) = engine.run_instance(instance_id).await else {
                    break;
                };
                if jobs.is_empty() {
                    break;
                }
                for job in jobs {
                    seen.entry(job.task_type.clone())
                        .or_default()
                        .insert(job.job_key.clone());
                    engine
                        .complete_job(&job.job_key, r#"{"ok":true}"#, hash, orch_flags.clone())
                        .await
                        .expect("complete");
                    hash = EffectId::content_hash(br#"{"ok":true}"#);
                }
            }
            let incidents = engine
                .inspect(instance_id)
                .await
                .expect("inspect")
                .incidents
                .len();
            let counts = seen.into_iter().map(|(t, keys)| (t, keys.len())).collect();
            (counts, incidents)
        }

        fn xor_default(take_guarded: bool) -> Shape {
            Shape {
                blocks: vec![Block::Xor {
                    take_guarded,
                    guarded: vec![Block::Task],
                    default: vec![Block::Task],
                }],
            }
        }
        fn or2(first: bool, second: bool) -> Shape {
            Shape {
                blocks: vec![Block::Or {
                    branches: vec![(first, vec![Block::Task]), (second, vec![Block::Task])],
                }],
            }
        }

        runtime().block_on(async {
            // uid 1 = gateway; guarded/branch0 task = t2, default/branch1 = t3.
            let (counts, incidents) = run_counts(&xor_default(true)).await;
            assert_eq!(counts.get("t2"), Some(&1), "taken guard must run its branch");
            assert_eq!(counts.get("t3"), None, "default must NOT run when guard taken");
            assert_eq!(incidents, 0);

            let (counts, incidents) = run_counts(&xor_default(false)).await;
            assert_eq!(counts.get("t2"), None, "untaken guard branch must not run");
            assert_eq!(counts.get("t3"), Some(&1), "default must run when guard untaken");
            assert_eq!(incidents, 0);

            let (counts, incidents) = run_counts(&or2(true, true)).await;
            assert_eq!(counts.get("t2"), Some(&1), "OR both: first branch runs");
            assert_eq!(counts.get("t3"), Some(&1), "OR both: second branch runs");
            assert_eq!(incidents, 0);

            let (counts, incidents) = run_counts(&or2(true, false)).await;
            assert_eq!(counts.get("t2"), Some(&1), "OR one: active branch runs");
            assert_eq!(counts.get("t3"), None, "OR one: inactive branch must not run");
            assert_eq!(incidents, 0);

            // All-false subset: ruling J zero-match — an incident, not a
            // silent skip and not a crash.
            let (counts, incidents) = run_counts(&or2(false, false)).await;
            assert_eq!(counts.get("t2"), None, "OR none: no branch runs");
            assert_eq!(counts.get("t3"), None, "OR none: no branch runs");
            assert_eq!(incidents, 1, "OR zero-match must raise exactly one incident");
        });
    }

    /// MsgWait widening cement: the sleeping-token contract end-to-end
    /// through the compiler — a token parked on a message wait stays
    /// parked on a NON-matching content key and unblocks (running its
    /// downstream task to completion) only on the matching key.
    #[test]
    fn message_wait_unblocks_only_on_matching_signal() {
        // uid 1 = wait (w1/k1/msg1/corr1), uid 2 = t2 downstream.
        let shape = Shape {
            blocks: vec![Block::MsgWait, Block::Task],
        };
        runtime().block_on(async {
            let generated = emit_process(&shape);
            let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
            let engine = BpmnLiteEngine::new_with_runtime_context(
                store,
                TenantId::default(),
                Arc::new(FuzzClock::new()),
            );
            let compiled = engine.compile(&generated.xml).await.expect("compile");
            let mut hash = EffectId::content_hash(generated.payload.as_bytes());
            let instance_id = engine
                .start("fuzz_graph", compiled.bytecode_version, &generated.payload, hash, "c")
                .await
                .expect("start");

            // init completes WITH the correlation field preserved, so the
            // wait resolves its key from the post-completion payload.
            let jobs = engine.run_instance(instance_id).await.expect("run");
            assert_eq!(jobs.len(), 1, "init first");
            let init_payload = format!(r#"{{"done":1,"k1":"corr1"}}"#);
            engine
                .complete_job(&jobs[0].job_key, &init_payload, hash, BTreeMap::new())
                .await
                .expect("complete init");
            hash = EffectId::content_hash(init_payload.as_bytes());

            // Parked: no activations, non-terminal.
            let jobs = engine.run_instance(instance_id).await.expect("run parked");
            assert!(jobs.is_empty(), "token must be parked on the message wait");

            // Wrong content key: must stay parked.
            engine
                .signal_with_value(instance_id, "msg1", "junk".to_string(), None, None, Some("m-w"))
                .await
                .expect("wrong-key signal delivers (buffered), it just must not correlate");
            let _ = engine.tick_instance(instance_id).await;
            let jobs = engine.run_instance(instance_id).await.expect("run still parked");
            assert!(
                jobs.is_empty(),
                "non-matching content key must NOT wake the token"
            );

            // Matching key: unblocks, downstream task runs, completes.
            engine
                .signal_with_value(
                    instance_id,
                    "msg1",
                    "corr1".to_string(),
                    None,
                    None,
                    Some("m-r"),
                )
                .await
                .expect("matching signal");
            let _ = engine.tick_instance(instance_id).await;
            let jobs = engine.run_instance(instance_id).await.expect("run woken");
            assert_eq!(jobs.len(), 1, "downstream task must activate after the match");
            assert_eq!(jobs[0].task_type, "t2");
            engine
                .complete_job(&jobs[0].job_key, r#"{"ok":1}"#, hash, BTreeMap::new())
                .await
                .expect("complete t2");
            // Drain: the end-event advance happens on the next run pass.
            for _ in 0..4 {
                let jobs = engine.run_instance(instance_id).await.expect("drain");
                if jobs.is_empty() {
                    break;
                }
            }
            let inspection = engine.inspect(instance_id).await.expect("inspect");
            assert!(
                inspection.state.is_terminal(),
                "instance must complete after the unblocked path, got {:?}",
                inspection.state
            );
        });
    }

    /// F8.7: deterministic tape population through the flag-storm driver
    /// — inconsistent flag histories every completion, structural oracles
    /// quiet throughout.
    #[test]
    fn flag_storm_driver_steps_clean_over_tape_population() {
        let mut seed: u64 = 0x8F0C_ED0D_2F8A_11B7;
        runtime().block_on(async {
            for _ in 0..60 {
                let bytes = lcg_bytes(&mut seed, 128);
                drive_flag_storm(&bytes).await;
            }
        });
    }

    /// F8.1 X-O1 red receipts: hostile bytes must REJECT, never panic —
    /// each case runs the full driver (no-panic half) AND must be Err at
    /// compile (fail-closed half; an Ok here would mean the parser
    /// admitted garbage).
    #[test]
    fn hostile_xml_rejects_without_panic() {
        let generated = emit_process(&Shape {
            blocks: vec![Block::Task],
        });
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"not xml at all".to_vec(),
            vec![0xff, 0xfe, 0x00, 0x80, 0xc3], // invalid UTF-8
            b"<bpmn:definitions".to_vec(),      // truncated open tag
            format!("<a>{}</a>", "<b>".repeat(4000)).into_bytes(), // nesting bomb
            generated
                .xml
                .replace("</bpmn:process>", "")
                .into_bytes(), // unclosed process
            generated
                .xml
                .replace(r#"targetRef="end""#, r#"targetRef="nowhere""#)
                .into_bytes(), // dangling flow ref
            // F8-COMPILER-001 minimal shape: an MI activity with NO
            // outgoing flow reached lowering and panicked at the
            // successor-address expect instead of rejecting. Found by
            // this target's first 30-min soak (spliced-document mutant);
            // all 9 successor-resolution sites in lowering.rs are now
            // fail-closed Errs naming the node.
            br#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="p" isExecutable="true">
    <bpmn:startEvent id="start"/>
    <bpmn:serviceTask id="mi1">
      <bpmn:extensionElements><zeebe:taskDefinition type="mi1"/></bpmn:extensionElements>
      <bpmn:multiInstanceLoopCharacteristics isSequential="false">
        <bpmn:extensionElements><zeebe:loopCharacteristics inputCollection="c" maxInstances="4"/></bpmn:extensionElements>
      </bpmn:multiInstanceLoopCharacteristics>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end"/>
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="mi1"/>
  </bpmn:process>
</bpmn:definitions>"#
                .to_vec(), // MI with no successor (F8-COMPILER-001)
        ];
        runtime().block_on(async {
            for case in &cases {
                // No-panic: the whole admit-or-reject-then-step driver.
                drive_xml_compile(case).await;
                // Fail-closed: these specific corruptions must reject.
                let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
                let engine = BpmnLiteEngine::new_with_runtime_context(
                    store,
                    TenantId::default(),
                    Arc::new(FuzzClock::new()),
                );
                assert!(
                    engine.compile(&String::from_utf8_lossy(case)).await.is_err(),
                    "hostile XML admitted: {:?}",
                    String::from_utf8_lossy(case).chars().take(80).collect::<String>()
                );
            }
        });
    }

    /// F8.1 X-O2 green receipt: valid XML through the SAME raw-bytes
    /// driver compiles, starts, steps, and cancels clean — the admitted
    /// path of the robustness target is live, not vacuous.
    #[test]
    fn admitted_xml_from_raw_bytes_steps_clean() {
        let shapes = [
            Shape { blocks: vec![Block::Task] },
            Shape {
                blocks: vec![Block::And {
                    branches: vec![vec![Block::Task], vec![Block::Task]],
                }],
            },
            Shape {
                blocks: vec![Block::Xor {
                    take_guarded: true,
                    guarded: vec![Block::Task],
                    default: vec![Block::Task],
                }],
            },
        ];
        runtime().block_on(async {
            for shape in &shapes {
                let generated = emit_process(shape);
                drive_xml_compile(generated.xml.as_bytes()).await;
            }
        });
    }

    /// F8.1 seed writer (run via `cargo xtask fuzz seed`): valid BPMN
    /// documents (one per covering single) so the fuzzer mutates from
    /// well-formed structure instead of rediscovering XML. Pre-cleans
    /// stale xml-*.xml.
    #[test]
    #[ignore = "writes files; invoked by cargo xtask fuzz seed"]
    fn write_xml_seeds() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds/xml_compile");
        std::fs::create_dir_all(&dir).expect("create seeds dir");
        for entry in std::fs::read_dir(&dir).expect("read seeds dir") {
            let path = entry.expect("dir entry").path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("xml-") && n.ends_with(".xml"))
            {
                std::fs::remove_file(&path).expect("remove stale seed");
            }
        }
        for (index, archetype) in covering::ALL_ARCHETYPES.iter().enumerate() {
            let generated = emit_process(&Shape {
                blocks: vec![archetype.block()],
            });
            std::fs::write(dir.join(format!("xml-{index:03}.xml")), generated.xml.as_bytes())
                .expect("write seed");
        }
        println!(
            "wrote {} xml seeds to {}",
            covering::ALL_ARCHETYPES.len(),
            dir.display()
        );
    }
}
