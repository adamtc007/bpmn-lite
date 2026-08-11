//! V5.4 — DSL vs XML differential harness.
//!
//! Proves that a DSL-authored `WorkflowExecutionPlan` and an XML(BPMN)-
//! authored equivalent, both expressing the same business logic (parallel
//! split into two tasks, joined, then a follow-up task, then end), reach
//! *equivalent terminal state* when run through the real engine — even
//! though `bpmn-lite-compiler`'s two frontends (`dsl::frontend::lower_plan`
//! and `lowering::lower`) produce different instruction addressing for that
//! topology.
//!
//! Modeled on `dmn-lite-engine/tests/differential/{mod,compare}.rs`'s
//! structure (compile both ways → run → compare outcomes) — no shared code,
//! that harness is a different subsystem (DMN decision evaluation, not BPMN
//! process execution).
//!
//! Both frontends re-lower `GatewayAnd`/`Split(Parallel)`+`Join(Parallel)`
//! to `Instr::V2Fork`/`Instr::V2Join` (V5.1/V5.2, landed). This harness is
//! the regression-proofing step for that re-lowering: it does NOT compare
//! canonical bytes or instruction addresses (those are legitimately
//! different between the two frontends), it compares the business-visible
//! outcome — final `ProcessState`, `domain_payload`, and orchestration
//! flags — after both programs fork two tasks, complete both jobs, join,
//! run a trailing task, and complete it.
//!
//! ## Known kernel defect found while building this harness (NOT fixed here)
//!
//! `Instr::End` (`bpmn-lite-kernel/src/lib.rs` ~line 1583) checks
//! `snapshot.fibers().len() == 1` to decide whether the current fiber is the
//! last one alive and the instance can transition to `ProcessState::Completed`.
//! `snapshot` is the *pre-transition* view fixed at the start of the
//! `apply_tick` call. When a `V2Join`'s last-arrival fibre cancels its
//! sibling(s) (`changes.fibers_delete`, kernel ~line 1793) and then, in the
//! *same* transition (no intervening park), runs straight through `Jump`
//! into `End` — the default shape whenever a parallel join is immediately
//! followed by `End` — `snapshot.fibers().len()` still counts the
//! already-cancelled sibling(s), so the `== 1` check never passes.
//! `instance.state` is never set to `Completed`, but the fibers (self +
//! cancelled siblings) are deleted from the store anyway: the instance is
//! left permanently `Running` with zero live fibers — undead, since
//! `tick_instance` requires a `WaitState::Running` fiber to exist to make
//! any further progress. Confirmed independently of this harness by reading
//! the kernel's own golden V4 test for this exact code path
//! (`bpmn-lite-kernel/src/lib.rs`, the `V2Join` LAST-ARRIVAL → `V2GuardEnd`
//! → `End`-in-one-transition case, comment "the oracle's §2 step 5 golden
//! transition") — it asserts `fibers_delete` and `concurrency_mutations`
//! but never `instance.state`, so nothing previously caught this.
//!
//! This affects BOTH frontends identically (it's a kernel bug, not a
//! lowering divergence), so it is not a DSL/XML equivalence failure — but it
//! does mean "parallel split → join → end" with nothing between join and
//! end is currently broken end-to-end for every caller, DSL or XML. Per
//! CLAUDE.md ("surface forks, don't decide them"; boundary-timer/V2Guard/
//! V2RaceArm kernel code is explicitly out of scope for this task), this is
//! reported, not patched, here. The fixture below inserts a real trailing
//! task between join and end specifically so the survivor fibre parks
//! (a legitimate, common workflow shape — converge, then a follow-up step)
//! before reaching `End`, which avoids tripping the bug while still fully
//! exercising `V2Fork`/`V2Join` fork, arrival-counting, and barrier
//! retirement end to end.

use bpmn_lite_compiler::dsl::{
    DeliveryMode, EndExecNode, ExecutionNode, JoinExecNode, JoinMode, PlaceholderSchema,
    SplitExecFlow, SplitExecNode, SplitMode, StartExecNode, TaskExecNode, WorkflowExecutionPlan,
};
use bpmn_lite_engine::BpmnLiteEngine;
use bpmn_lite_store::store::WorkflowStore;
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::{ProcessInstance, ProcessState, TenantId};
use bpmn_lite_vm::compute_hash;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

const TASK_A_TYPE: &str = "diff-task-a";
const TASK_B_TYPE: &str = "diff-task-b";
const TASK_AFTER_JOIN_TYPE: &str = "diff-task-after-join";
const DOMAIN_PAYLOAD: &str = r#"{"case":"differential-v5.4"}"#;

/// BPMN XML equivalent of the DSL parallel-split fixture below: start →
/// parallel-fork → { task_a, task_b } → parallel-join → task_after_join →
/// end. Mirrors the `<bpmn:parallelGateway gatewayDirection="...">` shape
/// `parser.rs` requires (Converging must be explicit; Diverging is the
/// default), same shape already exercised by
/// `lowering.rs::test_parallel_fork_join_lowering` (hand-built IR there;
/// this is the raw-XML equivalent through the real `parser::parse_bpmn` →
/// `Compiler::lower` path `engine.compile` uses).
const PARALLEL_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="diff_parallel_xml" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:parallelGateway id="fork1" gatewayDirection="Diverging" />
    <bpmn:serviceTask id="task_a" name="Task A">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="diff-task-a" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:serviceTask id="task_b" name="Task B">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="diff-task-b" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:parallelGateway id="join1" gatewayDirection="Converging" />
    <bpmn:serviceTask id="task_after_join" name="After Join">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="diff-task-after-join" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="fork1" />
    <bpmn:sequenceFlow id="f2" sourceRef="fork1" targetRef="task_a" />
    <bpmn:sequenceFlow id="f3" sourceRef="fork1" targetRef="task_b" />
    <bpmn:sequenceFlow id="f4" sourceRef="task_a" targetRef="join1" />
    <bpmn:sequenceFlow id="f5" sourceRef="task_b" targetRef="join1" />
    <bpmn:sequenceFlow id="f6" sourceRef="join1" targetRef="task_after_join" />
    <bpmn:sequenceFlow id="f7" sourceRef="task_after_join" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#;

/// DSL equivalent: start → `Split(Parallel)` → { task_a, task_b } →
/// `Join(Parallel)` → task_after_join → end. Same shape as `dsl/frontend.rs`'s
/// private `parallel_plan()` test fixture, with the task plugs renamed to
/// match this harness's XML-side task types 1:1 so the same job-completion
/// driver code works against both compiled programs (`ExecDslTask` and
/// `ExecNative` both park on a `WaitState::Job{job_key}` with the same
/// `task_type` string surfaced through `JobActivation`).
fn dsl_parallel_plan() -> WorkflowExecutionPlan {
    let nodes = BTreeMap::from([
            (
                "start".to_string(),
                ExecutionNode::Start(StartExecNode {
                    id: "start".to_string(),
                    next: "split".to_string(),
                    span: None,
                }),
            ),
            (
                "split".to_string(),
                ExecutionNode::Split(SplitExecNode {
                    id: "split".to_string(),
                    mode: SplitMode::Parallel,
                    routing_socket: None,
                    flows: vec![
                        SplitExecFlow {
                            placeholder: None,
                            expected_value: None,
                            next: "task_a".to_string(),
                        },
                        SplitExecFlow {
                            placeholder: None,
                            expected_value: None,
                            next: "task_b".to_string(),
                        },
                    ],
                    join: "join".to_string(),
                    produces_placeholder: None,
                    span: None,
                }),
            ),
            (
                "task_a".to_string(),
                ExecutionNode::Task(TaskExecNode {
                    id: "task_a".to_string(),
                    plug: TASK_A_TYPE.to_string(),
                    delivery_mode: DeliveryMode::GuaranteedAsync,
                    static_args: HashMap::new(),
                    next: "join".to_string(),
                    produces_placeholder: None,
                    consumes_placeholders: Vec::new(),
                    guards: Vec::new(),
                    span: None,
                    loop_origin: None,
                }),
            ),
            (
                "task_b".to_string(),
                ExecutionNode::Task(TaskExecNode {
                    id: "task_b".to_string(),
                    plug: TASK_B_TYPE.to_string(),
                    delivery_mode: DeliveryMode::GuaranteedAsync,
                    static_args: HashMap::new(),
                    next: "join".to_string(),
                    produces_placeholder: None,
                    consumes_placeholders: Vec::new(),
                    guards: Vec::new(),
                    span: None,
                    loop_origin: None,
                }),
            ),
            (
                "join".to_string(),
                ExecutionNode::Join(JoinExecNode {
                    id: "join".to_string(),
                    mode: JoinMode::Parallel,
                    split: "split".to_string(),
                    next: "task_after_join".to_string(),
                    span: None,
                }),
            ),
            (
                "task_after_join".to_string(),
                ExecutionNode::Task(TaskExecNode {
                    id: "task_after_join".to_string(),
                    plug: TASK_AFTER_JOIN_TYPE.to_string(),
                    delivery_mode: DeliveryMode::GuaranteedAsync,
                    static_args: HashMap::new(),
                    next: "end".to_string(),
                    produces_placeholder: None,
                    consumes_placeholders: Vec::new(),
                    guards: Vec::new(),
                    span: None,
                    loop_origin: None,
                }),
            ),
            (
                "end".to_string(),
                ExecutionNode::End(EndExecNode {
                    id: "end".to_string(),
                    status: "done".to_string(),
                    span: None,
                }),
            ),
    ]);
    WorkflowExecutionPlan::new(
        "diff_parallel_dsl".to_string(),
        nodes,
        "start".to_string(),
        PlaceholderSchema::default(),
        None,
        None,
    )
}

/// Compile `process_key`'s program, start an instance, drive it to
/// completion by activating and completing both parallel jobs (matched by
/// `TASK_A_TYPE`/`TASK_B_TYPE`), then the trailing post-join job
/// (`TASK_AFTER_JOIN_TYPE`), then return the terminal `ProcessInstance`.
///
/// `compile` is a closure so the same driver works for both the XML path
/// (`engine.compile(xml)`) and the DSL path (`engine.compile_dsl(&plan)`) —
/// `CompileResult` is identical either way, only the frontend differs.
async fn run_to_completion<F, Fut>(process_key: &str, compile: F) -> ProcessInstance
where
    F: FnOnce(BpmnLiteEngine) -> Fut,
    Fut: std::future::Future<Output = (BpmnLiteEngine, [u8; 32])>,
{
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
    let engine = BpmnLiteEngine::new(store.clone());
    let (engine, bytecode_version) = compile(engine).await;

    let hash = compute_hash(DOMAIN_PAYLOAD);
    let instance_id = engine
        .start(
            process_key,
            bytecode_version,
            DOMAIN_PAYLOAD,
            hash,
            "diff-corr",
        )
        .await
        .unwrap();

    // Drive the fork: both branch fibers run to their task park point.
    // tick_instance loops internally until no fiber is left Running (see
    // engine.rs::tick_instance_inner), but each Command::Tick only drives
    // ONE selected fiber to its own next park point (bpmn-lite-kernel's
    // apply_tick picks a single Running fiber per call) — so run_instance's
    // single tick_instance call still needs a following activate_jobs pass
    // to dequeue whichever fiber parked second.
    let mut activations = engine.run_instance(instance_id).await.unwrap();
    for _ in 0..8 {
        if activations.len() >= 2 {
            break;
        }
        let more = engine
            .activate_jobs(&[TASK_A_TYPE.to_string(), TASK_B_TYPE.to_string()], 10)
            .await
            .unwrap();
        if more.is_empty() {
            break;
        }
        activations.extend(more);
    }

    assert_eq!(
        activations.len(),
        2,
        "expected exactly 2 parked jobs (one per parallel branch) for {process_key}, got {activations:?}"
    );

    let job_a = activations
        .iter()
        .find(|activation| activation.task_type == TASK_A_TYPE)
        .unwrap_or_else(|| panic!("no {TASK_A_TYPE} activation for {process_key}: {activations:?}"));
    let job_b = activations
        .iter()
        .find(|activation| activation.task_type == TASK_B_TYPE)
        .unwrap_or_else(|| panic!("no {TASK_B_TYPE} activation for {process_key}: {activations:?}"));

    // Complete both jobs, echoing the same domain payload back unchanged so
    // the expected-hash chain stays simple (each completion's expected hash
    // is the payload hash still in effect after the previous completion).
    engine
        .complete_job(&job_a.job_key, DOMAIN_PAYLOAD, hash, BTreeMap::new())
        .await
        .unwrap();
    engine
        .complete_job(&job_b.job_key, DOMAIN_PAYLOAD, hash, BTreeMap::new())
        .await
        .unwrap();

    // Drive the join, then the trailing task park point.
    let mut after_join_activation = None;
    for _ in 0..8 {
        engine.tick_instance(instance_id).await.unwrap();
        let more = engine
            .activate_jobs(&[TASK_AFTER_JOIN_TYPE.to_string()], 10)
            .await
            .unwrap();
        if let Some(activation) = more.into_iter().next() {
            after_join_activation = Some(activation);
            break;
        }
    }
    let after_join_activation = after_join_activation.unwrap_or_else(|| {
        panic!("post-join task {TASK_AFTER_JOIN_TYPE} never activated for {process_key}")
    });
    engine
        .complete_job(&after_join_activation.job_key, DOMAIN_PAYLOAD, hash, BTreeMap::new())
        .await
        .unwrap();

    // Drive to End.
    for _ in 0..8 {
        engine.tick_instance(instance_id).await.unwrap();
        let instance = store
            .load_instance(&TenantId::new("default").unwrap(), instance_id)
            .await
            .unwrap()
            .unwrap();
        if instance.state.is_terminal() {
            return instance;
        }
    }

    store
        .load_instance(&TenantId::new("default").unwrap(), instance_id)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("instance {instance_id} vanished from store for {process_key}"))
}

/// V5.4: DSL-authored and XML-authored parallel-split/join workflows reach
/// equivalent terminal state.
///
/// Compares business-visible outcome only (§ this file's module doc) —
/// NOT raw canonical bytes or bytecode addresses, which are legitimately
/// different between `dsl::frontend::lower_plan` and `lowering::lower` for
/// the same topology.
#[tokio::test]
async fn dsl_and_xml_parallel_fork_join_reach_equivalent_terminal_state() {
    let xml_instance = run_to_completion("diff-xml", |engine| async move {
        let compiled = engine.compile(PARALLEL_XML).await.unwrap();
        (engine, compiled.bytecode_version)
    })
    .await;

    let dsl_instance = run_to_completion("diff-dsl", |engine| async move {
        let plan = dsl_parallel_plan();
        let compiled = engine.compile_dsl(&plan).await.unwrap();
        (engine, compiled.bytecode_version)
    })
    .await;

    // Both frontends compile to genuinely different bytecode (different
    // addressing, different instruction counts before the fork) — the
    // point of this harness is that this divergence is expected and
    // irrelevant to the comparison below.
    assert_ne!(
        xml_instance.bytecode_version, dsl_instance.bytecode_version,
        "sanity check: XML and DSL frontends must NOT produce identical bytecode for this fixture \
         (if they do, this test stops proving anything about cross-frontend equivalence)"
    );

    // 1. Both reach ProcessState::Completed (not Failed/Cancelled/stuck Running).
    assert!(
        matches!(xml_instance.state, ProcessState::Completed { .. }),
        "XML-authored instance did not complete: {:?}",
        xml_instance.state
    );
    assert!(
        matches!(dsl_instance.state, ProcessState::Completed { .. }),
        "DSL-authored instance did not complete: {:?}",
        dsl_instance.state
    );

    // 2. Equivalent domain payload — both branches echoed the same payload
    // back unchanged, so the join's downstream state must match byte-for-byte.
    assert_eq!(
        xml_instance.domain_payload.as_ref(),
        dsl_instance.domain_payload.as_ref(),
    );
    assert_eq!(
        xml_instance.domain_payload_hash, dsl_instance.domain_payload_hash,
    );

    // 3. Equivalent orchestration flags (both empty here — neither fixture
    // sets a flag — but compared explicitly rather than assumed, since a
    // divergence here would mean the two forks left different runtime
    // state despite running the "same" workflow).
    assert_eq!(xml_instance.flags, dsl_instance.flags);

    // 4. Equivalent counters/join_expected (both empty — neither fixture
    // uses bounded loops or inclusive/dynamic-arity joins) — a proxy for
    // "no stray dynamic-join bookkeeping survived to instance state" on
    // either frontend.
    assert_eq!(xml_instance.counters, dsl_instance.counters);
    assert_eq!(xml_instance.join_expected, dsl_instance.join_expected);
}
