//! B2 round-trip proof harness — EOP-PLAN-GRAPH-DSL-BRIDGE-001's
//! keystone gate: the emitted canonical DSL, recompiled through the real
//! `dsl::compile` route with the derived empty-bindings registry, must
//! produce a `WorkflowExecutionPlan` equal FIELD-BY-FIELD to
//! `project_ir(to_ir(dag))` — spans excluded by name (the ONLY excluded
//! field, per the B0-frozen equality table: the DSL path stamps source
//! positions, `ir_plan` stamps `None`; source positions exist only for
//! textual source).
//!
//! Per green fixture (B0 catalogue G1–G7), four proofs:
//!   1. witness — `DslReceipt.graph_state_hash` equals the content-derived
//!      hash of the same projection;
//!   2. idempotence — second emission is byte-identical (canonical means
//!      canonical);
//!   3. reparse identity — the printed source re-parses with zero errors
//!      and re-prints byte-identically (printer/parser desync detector,
//!      the V&S §7 stop condition);
//!   4. plan equality — `compile(emitted, derived_registry)` ≡
//!      `project_ir(to_ir(dag))` under span-stripped JSON equality.
//!
//! Red side: a refusal emits no artifact (structural — `Result`) and
//! leaves the graph identity untouched (cemented below).
//!
//! CI: this module rides the workspace test job in
//! `.github/workflows/production-gates.yml` (the "Migrations, RLS,
//! recovery, integration, and property tests" step, `cargo test
//! --workspace ...`) — it runs on every PR, not only under a local
//! `cargo test` (the gate that doesn't run is not a gate).
//!
//! Cement-locked: once green, these receipts are permanent. The
//! mutation red-trace for this harness (deliberately corrupting the
//! emitter's End sentinel makes `g5_terminate_end` fail plan-equality)
//! is recorded in the B2 tranche receipt, not committed as code.

#![cfg(test)]

use bpmn_lite_compiler::dsl::{
    compile, parse_workflow_str, project_ir, BindingDecl, StubPlaceholderRegistry, ToSexpr,
};
use bpmn_lite_compiler::{GatewayDirection, IRNode};
use uuid::Uuid;

use crate::schema::{DesignerDag, DesignerEdge, DslReceipt, NodeKey, Provenance};

fn key() -> NodeKey {
    NodeKey(Uuid::new_v4())
}

fn edge(id: &str) -> DesignerEdge {
    DesignerEdge {
        id: id.to_owned(),
        condition: None,
        provenance: Provenance::default(),
    }
}

fn start(id: &str) -> IRNode {
    IRNode::Start { id: id.into() }
}

fn end(id: &str, terminate: bool) -> IRNode {
    IRNode::End {
        id: id.into(),
        terminate,
    }
}

fn task(id: &str, task_type: &str) -> IRNode {
    IRNode::ServiceTask {
        id: id.into(),
        name: id.into(),
        task_type: task_type.into(),
        loop_origin: None,
    }
}

fn msg(id: &str, name: &str, corr: &str) -> IRNode {
    IRNode::MessageWait {
        id: id.into(),
        name: name.into(),
        corr_key_source: corr.into(),
    }
}

fn and_gw(id: &str, direction: GatewayDirection) -> IRNode {
    IRNode::GatewayAnd {
        id: id.into(),
        name: String::new(),
        direction,
    }
}

fn xor_gw(id: &str, direction: GatewayDirection) -> IRNode {
    IRNode::GatewayXor {
        id: id.into(),
        name: String::new(),
        direction,
    }
}

fn inclusive_gw(id: &str, direction: GatewayDirection) -> IRNode {
    IRNode::GatewayInclusive {
        id: id.into(),
        name: String::new(),
        direction,
    }
}

/// D5: a diverging edge with an Eq condition — the only operator both
/// `project_ir` and `emit_dsl` represent (DSL's own `ConditionAst` is
/// Eq-only).
fn cond_edge(id: &str, flag_name: &str, value: bool) -> DesignerEdge {
    DesignerEdge {
        id: id.to_owned(),
        condition: Some(bpmn_lite_compiler::ConditionExpr {
            flag_name: flag_name.to_owned(),
            op: bpmn_lite_compiler::ConditionOp::Eq,
            literal: bpmn_lite_compiler::ConditionLiteral::Bool(value),
        }),
        provenance: Provenance::default(),
    }
}

fn derived_registry(receipt: &DslReceipt) -> StubPlaceholderRegistry {
    // The B0 equivalence contract: every required symbol declared with an
    // EMPTY BindingDecl — the honest mirror of "no catalogue signal
    // exists for graph-authored tasks" that ir_plan's own
    // derive_delivery_mode(None, false, false) call already encodes.
    let mut reg = StubPlaceholderRegistry::new();
    for sym in &receipt.emitted.required_symbols {
        reg.register_verb(sym, BindingDecl::default());
    }
    reg
}

/// Normalize a serialized plan for the frozen equality comparison:
/// remove the `span` field from every node — the field the B0 equality
/// table excludes BY NAME (targeted at the known JSON shape, not a
/// blanket recursive strip, so an accidental future data field named
/// "span" elsewhere would still be compared).
///
/// History: between B2 and D0 this also sorted each Split's `flows` as a
/// multiset, because `project_ir` wrote flows in petgraph arena order
/// (edit-order-derived) while emission is edge-id-sorted — this
/// harness's own first run caught that divergence (G3/G4/G6 red). D0
/// (EOP-PLAN-DSL-PARITY-001) made `project_ir` sort by edge id too, so
/// flow order is now content-canonical on BOTH paths and the comparison
/// is back to strict ordered equality — the B2-era multiset amendment is
/// reversed. `project_ir_flow_order_is_content_canonical` below cements
/// the projection-side property directly.
fn normalize_plan(plan: &mut serde_json::Value) {
    if let Some(nodes) = plan.get_mut("nodes").and_then(|n| n.as_object_mut()) {
        for node in nodes.values_mut() {
            if let Some(tagged) = node.as_object_mut() {
                for inner in tagged.values_mut() {
                    if let Some(fields) = inner.as_object_mut() {
                        fields.remove("span");
                    }
                }
            }
        }
    }
}

/// The four green proofs for one fixture. Returns the receipt so a
/// fixture can add shape-specific assertions on top.
fn assert_roundtrip(dag: &DesignerDag, workflow_id: &str) -> DslReceipt {
    let receipt = dag.emit_dsl(workflow_id).expect("green fixture must emit");

    // 1. witness
    let ir = dag.to_ir().expect("to_ir");
    assert_eq!(
        receipt.graph_state_hash,
        DesignerDag::graph_state_hash(&ir),
        "witness must be the content-derived hash of the same projection"
    );

    // 2. idempotence
    let again = dag.emit_dsl(workflow_id).expect("second emission");
    assert_eq!(
        receipt.emitted.source, again.emitted.source,
        "emission must be byte-idempotent"
    );

    // 3. reparse identity (print → parse → print fixpoint, zero errors)
    let reparsed = parse_workflow_str(&receipt.emitted.source)
        .expect("emitted source must re-parse with zero errors");
    assert_eq!(
        receipt.emitted.source,
        reparsed.to_sexpr(0),
        "print→parse→print must be a fixpoint"
    );

    // 4. plan equality, spans excluded by name
    let dsl_plan = compile(&receipt.emitted.source, &derived_registry(&receipt))
        .expect("emitted source must recompile in-contract");
    let graph_plan =
        project_ir(&ir, workflow_id.to_owned()).expect("green fixture must project");
    let mut a = serde_json::to_value(&dsl_plan).expect("serialize dsl plan");
    let mut b = serde_json::to_value(&graph_plan).expect("serialize graph plan");
    normalize_plan(&mut a);
    normalize_plan(&mut b);
    assert_eq!(
        a, b,
        "DSL-compiled plan must equal project_ir plan field-by-field (spans excluded)"
    );

    receipt
}

fn linear(name: &str, task_type: &str, terminate: bool) -> DesignerDag {
    let mut dag = DesignerDag::new(name);
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let t = dag
        .insert_node(key(), task("t1", task_type), None, Provenance::default())
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", terminate), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, t, edge("f1")).unwrap();
    dag.insert_edge(t, e, edge("f2")).unwrap();
    dag
}

/// G1 — linear start→task→end(completed).
#[test]
fn g1_linear_completed() {
    let dag = linear("g1", "cbu.create", false);
    let receipt = assert_roundtrip(&dag, "g1");
    assert_eq!(receipt.emitted.required_symbols, vec!["cbu.create".to_owned()]);
}

/// G2 — task chain with a MessageWait in the middle.
#[test]
fn g2_task_message_wait_task() {
    let mut dag = DesignerDag::new("g2");
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let t1 = dag
        .insert_node(key(), task("t1", "cbu.request"), None, Provenance::default())
        .unwrap();
    let m = dag
        .insert_node(
            key(),
            msg("wait-reply", "reply-received", "case-id"),
            None,
            Provenance::default(),
        )
        .unwrap();
    let t2 = dag
        .insert_node(key(), task("t2", "cbu.record"), None, Provenance::default())
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, t1, edge("f1")).unwrap();
    dag.insert_edge(t1, m, edge("f2")).unwrap();
    dag.insert_edge(m, t2, edge("f3")).unwrap();
    dag.insert_edge(t2, e, edge("f4")).unwrap();
    assert_roundtrip(&dag, "g2");
}

fn and_block(name: &str, branches: usize) -> DesignerDag {
    let mut dag = DesignerDag::new(name);
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let sp = dag
        .insert_node(
            key(),
            and_gw("split1", GatewayDirection::Diverging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let j = dag
        .insert_node(
            key(),
            and_gw("join1", GatewayDirection::Converging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, sp, edge("f-in")).unwrap();
    for i in 0..branches {
        let t = dag
            .insert_node(
                key(),
                task(&format!("branch-{i}"), &format!("cbu.b{i}")),
                None,
                Provenance::default(),
            )
            .unwrap();
        dag.insert_edge(sp, t, edge(&format!("f-out-{i}"))).unwrap();
        dag.insert_edge(t, j, edge(&format!("f-back-{i}"))).unwrap();
    }
    dag.insert_edge(j, e, edge("f-end")).unwrap();
    dag
}

/// G3 — matched And block, 2 branches.
#[test]
fn g3_and_block_two_branches() {
    assert_roundtrip(&and_block("g3", 2), "g3");
}

/// G4 — matched And block, 3 branches.
#[test]
fn g4_and_block_three_branches() {
    assert_roundtrip(&and_block("g4", 3), "g4");
}

/// G5 — terminate end (the `"terminated"` sentinel both projections
/// share; the harness's mutation red-trace target).
#[test]
fn g5_terminate_end() {
    let dag = linear("g5", "cbu.close", true);
    let receipt = assert_roundtrip(&dag, "g5");
    assert!(receipt.emitted.source.contains(":status \"terminated\""));
}

/// G6 — nested And blocks (an inner block inside one outer branch).
#[test]
fn g6_nested_and_blocks() {
    let mut dag = DesignerDag::new("g6");
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let spo = dag
        .insert_node(
            key(),
            and_gw("outer-split", GatewayDirection::Diverging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let spi = dag
        .insert_node(
            key(),
            and_gw("inner-split", GatewayDirection::Diverging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let ta = dag
        .insert_node(key(), task("inner-a", "cbu.ia"), None, Provenance::default())
        .unwrap();
    let tb = dag
        .insert_node(key(), task("inner-b", "cbu.ib"), None, Provenance::default())
        .unwrap();
    let ji = dag
        .insert_node(
            key(),
            and_gw("inner-join", GatewayDirection::Converging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let tc = dag
        .insert_node(key(), task("outer-b", "cbu.ob"), None, Provenance::default())
        .unwrap();
    let jo = dag
        .insert_node(
            key(),
            and_gw("outer-join", GatewayDirection::Converging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, spo, edge("f1")).unwrap();
    dag.insert_edge(spo, spi, edge("f2")).unwrap();
    dag.insert_edge(spo, tc, edge("f3")).unwrap();
    dag.insert_edge(spi, ta, edge("f4")).unwrap();
    dag.insert_edge(spi, tb, edge("f5")).unwrap();
    dag.insert_edge(ta, ji, edge("f6")).unwrap();
    dag.insert_edge(tb, ji, edge("f7")).unwrap();
    dag.insert_edge(ji, jo, edge("f8")).unwrap();
    dag.insert_edge(tc, jo, edge("f9")).unwrap();
    dag.insert_edge(jo, e, edge("f10")).unwrap();
    assert_roundtrip(&dag, "g6");
}

/// G7 — several tasks sharing one task_type: required_symbols dedups.
#[test]
fn g7_required_symbols_dedup() {
    let mut dag = DesignerDag::new("g7");
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let t1 = dag
        .insert_node(key(), task("t1", "cbu.same"), None, Provenance::default())
        .unwrap();
    let t2 = dag
        .insert_node(key(), task("t2", "cbu.same"), None, Provenance::default())
        .unwrap();
    let t3 = dag
        .insert_node(key(), task("t3", "cbu.same"), None, Provenance::default())
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, t1, edge("f1")).unwrap();
    dag.insert_edge(t1, t2, edge("f2")).unwrap();
    dag.insert_edge(t2, t3, edge("f3")).unwrap();
    dag.insert_edge(t3, e, edge("f4")).unwrap();
    let receipt = assert_roundtrip(&dag, "g7");
    assert_eq!(receipt.emitted.required_symbols, vec!["cbu.same".to_owned()]);
}

/// D0 cement (EOP-PLAN-DSL-PARITY-001): two different edit orders that
/// build `ir_graphs_equivalent` DAGs must project BYTE-IDENTICAL plans —
/// `project_ir`'s flow order is content-canonical (edge-id-sorted), not
/// edit-order-derived. Before D0 this failed: petgraph arena order made
/// the flows arrays differ, giving the same design content different
/// stored plan bytes/hashes and different lowered V2Fork target order.
#[test]
fn project_ir_flow_order_is_content_canonical() {
    // Same And-block content, branches inserted in opposite orders.
    let build = |branch_order: &[usize]| {
        let mut dag = DesignerDag::new("d0-canonical");
        let s = dag
            .insert_node(key(), start("start"), None, Provenance::default())
            .unwrap();
        let sp = dag
            .insert_node(
                key(),
                and_gw("split1", GatewayDirection::Diverging),
                None,
                Provenance::default(),
            )
            .unwrap();
        let j = dag
            .insert_node(
                key(),
                and_gw("join1", GatewayDirection::Converging),
                None,
                Provenance::default(),
            )
            .unwrap();
        let e = dag
            .insert_node(key(), end("end", false), None, Provenance::default())
            .unwrap();
        dag.insert_edge(s, sp, edge("f-in")).unwrap();
        for &i in branch_order {
            let t = dag
                .insert_node(
                    key(),
                    task(&format!("branch-{i}"), &format!("cbu.b{i}")),
                    None,
                    Provenance::default(),
                )
                .unwrap();
            dag.insert_edge(sp, t, edge(&format!("f-out-{i}"))).unwrap();
            dag.insert_edge(t, j, edge(&format!("f-back-{i}"))).unwrap();
        }
        dag.insert_edge(j, e, edge("f-end")).unwrap();
        dag
    };
    let a = build(&[0, 1, 2]);
    let b = build(&[2, 1, 0]);
    let ir_a = a.to_ir().unwrap();
    let ir_b = b.to_ir().unwrap();
    assert!(DesignerDag::ir_graphs_equivalent(&ir_a, &ir_b));
    let plan_a = project_ir(&ir_a, "d0".to_owned()).unwrap();
    let plan_b = project_ir(&ir_b, "d0".to_owned()).unwrap();
    assert_eq!(
        serde_json::to_string(&plan_a).unwrap(),
        serde_json::to_string(&plan_b).unwrap(),
        "equivalent edit orders must project byte-identical plans"
    );
}

/// D1 guard-fixture builder: a linear host graph plus `guards` attached
/// to `t1`, each with its own escape End.
fn guarded_linear(
    name: &str,
    guards: &[bpmn_lite_compiler::IRNode],
) -> DesignerDag {
    let mut dag = DesignerDag::new(name);
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let t = dag
        .insert_node(key(), task("t1", "cbu.host"), None, Provenance::default())
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, t, edge("f1")).unwrap();
    dag.insert_edge(t, e, edge("f2")).unwrap();
    for (i, g) in guards.iter().enumerate() {
        let gk = dag
            .insert_node(key(), g.clone(), Some(t), Provenance::default())
            .unwrap();
        let esc = dag
            .insert_node(
                key(),
                end(&format!("escape-end-{i}"), false),
                None,
                Provenance::default(),
            )
            .unwrap();
        dag.insert_edge(gk, esc, edge(&format!("f-esc-{i}"))).unwrap();
    }
    dag
}

fn timer_guard(id: &str, spec: bpmn_lite_compiler::TimerSpec, interrupting: bool) -> bpmn_lite_compiler::IRNode {
    bpmn_lite_compiler::IRNode::BoundaryTimer {
        id: id.into(),
        attached_to: "t1".into(),
        spec,
        interrupting,
        failure_budget: None,
    }
}

fn error_guard(id: &str, code: Option<&str>, budget: Option<u32>) -> bpmn_lite_compiler::IRNode {
    bpmn_lite_compiler::IRNode::BoundaryError {
        id: id.into(),
        attached_to: "t1".into(),
        error_code: code.map(Into::into),
        failure_budget: budget,
    }
}

/// G8 (D1) — interrupting duration-timer guard: the full four-proof
/// round trip now covers `TaskExecNode.guards` (compared ORDERED per the
/// D1.0 equality delta — both paths guard-id-sort).
#[test]
fn g8_interrupting_timer_guard() {
    let dag = guarded_linear(
        "g8",
        &[timer_guard(
            "g-timeout",
            bpmn_lite_compiler::TimerSpec::Duration { ms: 60_000 },
            true,
        )],
    );
    let receipt = assert_roundtrip(&dag, "g8");
    assert!(receipt
        .emitted
        .source
        .contains(":duration-ms 60000 :interrupting true"));
}

/// G9 (D1) — non-interrupting (rearming) cycle timer with max-fires.
#[test]
fn g9_rearming_cycle_timer_guard() {
    let dag = guarded_linear(
        "g9",
        &[timer_guard(
            "g-remind",
            bpmn_lite_compiler::TimerSpec::Cycle {
                interval_ms: 30_000,
                max_fires: 5,
            },
            false,
        )],
    );
    let receipt = assert_roundtrip(&dag, "g9");
    assert!(receipt
        .emitted
        .source
        .contains(":cycle-ms 30000 :max-fires 5 :interrupting false"));
}

/// G10 (D1) — error guard with code and budget.
#[test]
fn g10_error_guard_code_and_budget() {
    let dag = guarded_linear("g10", &[error_guard("g-err", Some("E42"), Some(3))]);
    let receipt = assert_roundtrip(&dag, "g10");
    assert!(receipt
        .emitted
        .source
        .contains(":error-code \"E42\" :budget 3"));
}

/// G11 (D1) — two error guards on one host: guard-order canonicality
/// through the FULL round trip (extends D0's projection-side cement).
#[test]
fn g11_two_error_guards_ordered() {
    let dag = guarded_linear(
        "g11",
        &[
            error_guard("g-b", Some("EB"), None),
            error_guard("g-a", Some("EA"), None),
        ],
    );
    let receipt = assert_roundtrip(&dag, "g11");
    let a = receipt.emitted.source.find("g-a").unwrap();
    let b = receipt.emitted.source.find("g-b").unwrap();
    assert!(a < b, "guards must emit in guard-id order");
}

/// G12 (D1) — escape chain containing a task: the escape subgraph is
/// ordinary flow on both paths.
#[test]
fn g12_escape_chain_with_task() {
    let mut dag = DesignerDag::new("g12");
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let t = dag
        .insert_node(key(), task("t1", "cbu.host"), None, Provenance::default())
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, t, edge("f1")).unwrap();
    dag.insert_edge(t, e, edge("f2")).unwrap();
    let g = dag
        .insert_node(
            key(),
            timer_guard(
                "g-esc",
                bpmn_lite_compiler::TimerSpec::Duration { ms: 1000 },
                true,
            ),
            Some(t),
            Provenance::default(),
        )
        .unwrap();
    let et = dag
        .insert_node(key(), task("cleanup", "cbu.clean"), None, Provenance::default())
        .unwrap();
    let ee = dag
        .insert_node(key(), end("escape-end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(g, et, edge("f-esc-1")).unwrap();
    dag.insert_edge(et, ee, edge("f-esc-2")).unwrap();
    assert_roundtrip(&dag, "g12");
}

/// D2 fixture builder: start → timer-wait(spec) → end.
fn timer_linear(name: &str, spec: bpmn_lite_compiler::TimerSpec) -> DesignerDag {
    let mut dag = DesignerDag::new(name);
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let w = dag
        .insert_node(
            key(),
            IRNode::TimerWait {
                id: "w1".into(),
                spec,
            },
            None,
            Provenance::default(),
        )
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, w, edge("f1")).unwrap();
    dag.insert_edge(w, e, edge("f2")).unwrap();
    dag
}

/// G13 (D2) — duration timer wait through the four-proof round trip:
/// the plan's `ExecutionNode::Wait` now round-trips the bridge.
#[test]
fn g13_timer_wait_duration() {
    let dag = timer_linear("g13", bpmn_lite_compiler::TimerSpec::Duration { ms: 1000 });
    assert_roundtrip(&dag, "g13");
}

/// G14 (D2) — date (deadline) timer wait.
#[test]
fn g14_timer_wait_deadline() {
    let dag = timer_linear(
        "g14",
        bpmn_lite_compiler::TimerSpec::Date {
            deadline_ms: 1_755_000_000_000,
        },
    );
    assert_roundtrip(&dag, "g14");
}

/// G15 (D2) — cycle timer wait, round-tripping BOTH integers
/// (interval_ms and max_fires).
#[test]
fn g15_timer_wait_cycle_max_fires() {
    let dag = timer_linear(
        "g15",
        bpmn_lite_compiler::TimerSpec::Cycle {
            interval_ms: 60_000,
            max_fires: 3,
        },
    );
    assert_roundtrip(&dag, "g15");
}

/// D3 fixture builder: start → multi-instance(declared_max) → end.
fn multi_instance_linear(name: &str, declared_max: u32) -> DesignerDag {
    let mut dag = DesignerDag::new(name);
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let m = dag
        .insert_node(
            key(),
            IRNode::MultiInstance {
                id: "m1".into(),
                name: String::new(),
                task_type: "review-doc".into(),
                collection_flag_name: "docs".into(),
                declared_max,
                inputs: Vec::new(),
            },
            None,
            Provenance::default(),
        )
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, m, edge("f1")).unwrap();
    dag.insert_edge(m, e, edge("f2")).unwrap();
    dag
}

/// G16 (D3) — multi-instance through the four-proof round trip: the
/// plan's `ExecutionNode::MultiInstance` now round-trips the bridge,
/// with `declared_max` (ruling K's required ceiling) proven to survive
/// print→parse→print and DSL-vs-graph plan equality.
#[test]
fn g16_multi_instance_declared_max_round_trip() {
    let dag = multi_instance_linear("g16", 50);
    let receipt = assert_roundtrip(&dag, "g16");
    assert!(receipt.emitted.required_symbols.is_empty());
}

/// D4 (fork E) fixture builder: start -> task(loop_origin) -> end. No
/// graph-side operation sets `IRNode::ServiceTask.loop_origin` today (D4.0
/// design note §1 — no loop-authoring op exists), so this hand-builds the
/// node directly via `insert_node`, same convention G13-G16 already use
/// for shapes with no authoring op of their own.
fn loop_origin_task_linear(name: &str, loop_origin: &str) -> DesignerDag {
    let mut dag = DesignerDag::new(name);
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let t = dag
        .insert_node(
            key(),
            IRNode::ServiceTask {
                id: "t1".into(),
                name: "t1".into(),
                task_type: "noop".into(),
                loop_origin: Some(loop_origin.into()),
            },
            None,
            Provenance::default(),
        )
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, t, edge("f1")).unwrap();
    dag.insert_edge(t, e, edge("f2")).unwrap();
    dag
}

/// G17 (D4, fork E, Piece 1), red axis — `IRNode::ServiceTask.loop_origin`
/// has no `ToSexpr` grammar surface, so `emit_dsl` REFUSES a graph that
/// carries it rather than silently emitting source that would recompile
/// to a plan missing the provenance. This is the fixture that caught the
/// tranche's own first-draft defect: a naive `project_ir`/`emit_dsl`
/// pass-through looked plausible in isolation but broke this exact
/// four-proof round trip (DSL-recompiled plan lacked `loop_origin`, graph
/// plan had it) the moment it was checked here — refusal, not a lossy
/// green flip, is the fail-closed disposition.
#[test]
fn g17_service_task_loop_origin_refuses_at_emission() {
    let dag = loop_origin_task_linear("g17", "retry-loop");
    match dag.emit_dsl("g17") {
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("loop provenance") && msg.contains("t1"),
                "unexpected refusal message: {msg}"
            );
        }
        Ok(_) => panic!("expected LoopOriginUnrepresentable refusal"),
    }
}

/// `project_ir` still projects a `loop_origin`-carrying graph cleanly —
/// only the DSL-text emission direction refuses (see
/// `g17_service_task_loop_origin_refuses_at_emission` above). Confirms the
/// refusal is specific to the unprintable direction, not a graph-admission
/// defect.
#[test]
fn g17b_service_task_loop_origin_still_projects_to_plan() {
    let dag = loop_origin_task_linear("g17b", "retry-loop");
    let ir = dag.to_ir().expect("graph must project to IR");
    let plan = project_ir(&ir, "g17b".into())
        .expect("project_ir must accept a loop_origin-carrying graph");
    match plan.nodes().get("t1").unwrap() {
        bpmn_lite_compiler::dsl::ExecutionNode::Task(t) => {
            assert_eq!(t.loop_origin.as_deref(), Some("retry-loop"));
        }
        other => panic!("expected Task, got {other:?}"),
    }
}

/// D5 fixture builder: start -> XOR-split(cond A / default B) -> join ->
/// end. Shared by g18 (emit refusal) and g18b (project_ir still works).
fn xor_block_conditioned(name: &str) -> DesignerDag {
    let mut dag = DesignerDag::new(name);
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let sp = dag
        .insert_node(
            key(),
            xor_gw("split1", GatewayDirection::Diverging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let ta = dag
        .insert_node(key(), task("branch-a", "cbu.a"), None, Provenance::default())
        .unwrap();
    let tb = dag
        .insert_node(key(), task("branch-b", "cbu.b"), None, Provenance::default())
        .unwrap();
    let j = dag
        .insert_node(
            key(),
            xor_gw("join1", GatewayDirection::Converging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, sp, edge("f-in")).unwrap();
    dag.insert_edge(sp, ta, cond_edge("f-a", "@approved", true))
        .unwrap();
    dag.insert_edge(sp, tb, edge("f-b")).unwrap();
    dag.insert_edge(ta, j, edge("f-a-back")).unwrap();
    dag.insert_edge(tb, j, edge("f-b-back")).unwrap();
    dag.insert_edge(j, e, edge("f-end")).unwrap();
    dag
}

/// G18 (D5, EOP-PLAN-DSL-PARITY-001, fork B/P3), red axis — this
/// tranche's own B2 harness caught that `dsl/parser.rs::parse_split`
/// requires a `:plug` (a bound decision-verb reference) and a
/// `:condition` on every flow for `split-xor` — `IRNode::GatewayXor`
/// carries neither, so NO graph-authored diverging GatewayXor can ever
/// satisfy the grammar (Adam's ruling, 2026-08-14: refuse unconditionally
/// rather than fabricate a plug or loosen the grammar). This fixture
/// would have looked plausible as a GREEN round-trip in isolation — the
/// harness rejected it exactly as g17's loop_origin refusal did.
#[test]
fn g18_xor_split_refuses_at_emission() {
    let dag = xor_block_conditioned("g18");
    match dag.emit_dsl("g18") {
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("split1") && msg.contains("GatewayXor"),
                "unexpected refusal message: {msg}"
            );
        }
        Ok(_) => panic!("expected GatewayXorSplitUnrepresentable refusal"),
    }
}

/// `project_ir` still projects a matched GatewayXor pair cleanly — only
/// the DSL-text emission direction refuses (see
/// `g18_xor_split_refuses_at_emission` above). Confirms the refusal is
/// specific to the unprintable grammar direction, not a graph-admission
/// defect — same pattern as g17b for loop_origin.
#[test]
fn g18b_xor_split_still_projects_to_plan() {
    let dag = xor_block_conditioned("g18b");
    let ir = dag.to_ir().expect("graph must project to IR");
    let plan = project_ir(&ir, "g18b".into()).expect("project_ir must accept a matched XOR pair");
    match plan.nodes().get("split1").unwrap() {
        bpmn_lite_compiler::dsl::ExecutionNode::Split(s) => {
            assert_eq!(s.mode, bpmn_lite_compiler::dsl::SplitMode::Exclusive);
            assert_eq!(s.join, "join1");
            assert_eq!(s.flows.len(), 2);
        }
        other => panic!("expected Split, got {other:?}"),
    }
}

/// G19 (D5), red axis — an unmatched CONVERGING GatewayXor (no paired
/// diverging node reachable by `gateway_pairs`' post-dominator match)
/// refuses via `UnmatchedGateway`, mirroring G-series AND coverage for
/// the newly admitted kind. Deliberately has NO diverging GatewayXor
/// anywhere in the graph: a diverging XOR always refuses first
/// (`GatewayXorSplitUnrepresentable`, see g18), so exercising the
/// Converging arm's own `UnmatchedGateway` path requires a graph where
/// the canonical scan reaches a converging XOR without ever visiting a
/// diverging one.
#[test]
fn g19_unmatched_converging_xor_gateway_refuses_at_emission() {
    let mut dag = DesignerDag::new("g19");
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let t = dag
        .insert_node(key(), task("t1", "cbu.t"), None, Provenance::default())
        .unwrap();
    let j = dag
        .insert_node(
            key(),
            xor_gw("j1", GatewayDirection::Converging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, t, edge("f1")).unwrap();
    dag.insert_edge(t, j, edge("f2")).unwrap();
    dag.insert_edge(j, e, edge("f3")).unwrap();
    match dag.emit_dsl("g19") {
        Err(err) => assert!(
            err.to_string().contains("no matching join"),
            "unexpected refusal message: {err}"
        ),
        Ok(_) => panic!("expected UnmatchedGateway refusal"),
    }
}

/// D6 fixture builder: start -> INCLUSIVE-split(cond A / default B) ->
/// join -> end. Shared by g20 (emit refusal) and g20b (project_ir still
/// works) — identical shape to `xor_block_conditioned`, per the D6.0
/// design note's finding that GatewayInclusive faces the exact same
/// `:plug`-required structural gap D5 found for GatewayXor.
fn inclusive_block_conditioned(name: &str) -> DesignerDag {
    let mut dag = DesignerDag::new(name);
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let sp = dag
        .insert_node(
            key(),
            inclusive_gw("split1", GatewayDirection::Diverging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let ta = dag
        .insert_node(key(), task("branch-a", "cbu.a"), None, Provenance::default())
        .unwrap();
    let tb = dag
        .insert_node(key(), task("branch-b", "cbu.b"), None, Provenance::default())
        .unwrap();
    let j = dag
        .insert_node(
            key(),
            inclusive_gw("join1", GatewayDirection::Converging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, sp, edge("f-in")).unwrap();
    dag.insert_edge(sp, ta, cond_edge("f-a", "@approved", true))
        .unwrap();
    dag.insert_edge(sp, tb, edge("f-b")).unwrap();
    dag.insert_edge(ta, j, edge("f-a-back")).unwrap();
    dag.insert_edge(tb, j, edge("f-b-back")).unwrap();
    dag.insert_edge(j, e, edge("f-end")).unwrap();
    dag
}

/// G20 (D6, EOP-PLAN-DSL-PARITY-001, fork D/P1½), red axis — mirrors
/// g18 exactly: `dsl/parser.rs::parse_split` requires `:plug` and a
/// `:condition` on every flow for `split-or`, and `IRNode::GatewayInclusive`
/// carries neither, so no graph-authored diverging GatewayInclusive can
/// ever satisfy the grammar (D6.0 design note, Adam's ruling: option (a)).
#[test]
fn g20_inclusive_split_refuses_at_emission() {
    let dag = inclusive_block_conditioned("g20");
    match dag.emit_dsl("g20") {
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("split1") && msg.contains("GatewayInclusive"),
                "unexpected refusal message: {msg}"
            );
        }
        Ok(_) => panic!("expected GatewayInclusiveSplitUnrepresentable refusal"),
    }
}

/// `project_ir` still projects a matched GatewayInclusive pair cleanly —
/// only the DSL-text emission direction refuses (see
/// `g20_inclusive_split_refuses_at_emission` above). Mirrors g18b.
#[test]
fn g20b_inclusive_split_still_projects_to_plan() {
    let dag = inclusive_block_conditioned("g20b");
    let ir = dag.to_ir().expect("graph must project to IR");
    let plan =
        project_ir(&ir, "g20b".into()).expect("project_ir must accept a matched Inclusive pair");
    match plan.nodes().get("split1").unwrap() {
        bpmn_lite_compiler::dsl::ExecutionNode::Split(s) => {
            assert_eq!(s.mode, bpmn_lite_compiler::dsl::SplitMode::Inclusive);
            assert_eq!(s.join, "join1");
            assert_eq!(s.flows.len(), 2);
        }
        other => panic!("expected Split, got {other:?}"),
    }
}

/// G21 (D6), red axis — an unmatched CONVERGING GatewayInclusive refuses
/// via `UnmatchedGateway`, mirroring g19's isolation of the Converging
/// arm's own pairing-refusal path (no diverging Inclusive anywhere in
/// the graph, since a diverging one always refuses first at
/// `GatewayInclusiveSplitUnrepresentable`, see g20).
#[test]
fn g21_unmatched_converging_inclusive_gateway_refuses_at_emission() {
    let mut dag = DesignerDag::new("g21");
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    let t = dag
        .insert_node(key(), task("t1", "cbu.t"), None, Provenance::default())
        .unwrap();
    let j = dag
        .insert_node(
            key(),
            inclusive_gw("j1", GatewayDirection::Converging),
            None,
            Provenance::default(),
        )
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, t, edge("f1")).unwrap();
    dag.insert_edge(t, j, edge("f2")).unwrap();
    dag.insert_edge(j, e, edge("f3")).unwrap();
    match dag.emit_dsl("g21") {
        Err(err) => assert!(
            err.to_string().contains("no matching join"),
            "unexpected refusal message: {err}"
        ),
        Ok(_) => panic!("expected UnmatchedGateway refusal"),
    }
}

/// D0 blind-review cement: guard order is content-canonical too. Two
/// verifier-legal error guards on one host, attached in opposite edit
/// orders, must project byte-identical plans — before the guard sort,
/// `guards_by_host`'s per-host Vec was `node_indices()` (edit) order and
/// this failed (reproduced empirically by the D0 review). Guard order
/// feeds lowering's guard arms, so this is determinism, not cosmetics.
#[test]
fn project_ir_guard_order_is_content_canonical() {
    let build = |guard_order: &[usize]| {
        let mut dag = DesignerDag::new("d0-guards");
        let s = dag
            .insert_node(key(), start("start"), None, Provenance::default())
            .unwrap();
        let t = dag
            .insert_node(key(), task("t1", "cbu.host"), None, Provenance::default())
            .unwrap();
        let e = dag
            .insert_node(key(), end("end", false), None, Provenance::default())
            .unwrap();
        dag.insert_edge(s, t, edge("f1")).unwrap();
        dag.insert_edge(t, e, edge("f2")).unwrap();
        for &i in guard_order {
            let g = dag
                .insert_node(
                    key(),
                    bpmn_lite_compiler::IRNode::BoundaryError {
                        id: format!("guard-{i}"),
                        attached_to: "t1".into(),
                        error_code: Some(format!("E{i}")),
                        failure_budget: None,
                    },
                    Some(t),
                    Provenance::default(),
                )
                .unwrap();
            let esc = dag
                .insert_node(
                    key(),
                    end(&format!("escape-end-{i}"), false),
                    None,
                    Provenance::default(),
                )
                .unwrap();
            dag.insert_edge(g, esc, edge(&format!("f-esc-{i}"))).unwrap();
        }
        dag
    };
    let a = build(&[0, 1]);
    let b = build(&[1, 0]);
    let ir_a = a.to_ir().unwrap();
    let ir_b = b.to_ir().unwrap();
    assert!(DesignerDag::ir_graphs_equivalent(&ir_a, &ir_b));
    let plan_a = project_ir(&ir_a, "d0g".to_owned()).unwrap();
    let plan_b = project_ir(&ir_b, "d0g".to_owned()).unwrap();
    assert_eq!(
        serde_json::to_string(&plan_a).unwrap(),
        serde_json::to_string(&plan_b).unwrap(),
        "equivalent guard attachment orders must project byte-identical plans"
    );
}

/// Red side — a refusal leaves the graph identity untouched (the B0 red
/// rule "no partial artifact and unchanged graph_state_hash"; "no
/// artifact" is structural via `Result`, the identity half is cemented
/// here at the layer that owns the hash).
#[test]
fn red_refusal_leaves_identity_untouched() {
    let mut dag = DesignerDag::new("red-identity");
    let s = dag
        .insert_node(key(), start("start"), None, Provenance::default())
        .unwrap();
    // D2 cement update: the refusal VEHICLE was TimerWait until it
    // joined the emission core; the invariant under test (refusal ⇒
    // unchanged graph identity) is kind-agnostic, so the vehicle is now
    // HumanWait — still out of core.
    let t = dag
        .insert_node(
            key(),
            IRNode::HumanWait {
                id: "wait".into(),
                name: String::new(),
                task_kind: String::new(),
                corr_key_source: String::new(),
            },
            None,
            Provenance::default(),
        )
        .unwrap();
    let e = dag
        .insert_node(key(), end("end", false), None, Provenance::default())
        .unwrap();
    dag.insert_edge(s, t, edge("f1")).unwrap();
    dag.insert_edge(t, e, edge("f2")).unwrap();

    let before = DesignerDag::graph_state_hash(&dag.to_ir().unwrap());
    let err = dag.emit_dsl("red-identity").unwrap_err();
    match err.downcast_ref::<bpmn_lite_compiler::dsl::DslEmitError>() {
        Some(bpmn_lite_compiler::dsl::DslEmitError::UnsupportedNode { kind, .. }) => {
            assert_eq!(*kind, "HumanWait");
        }
        other => panic!("expected UnsupportedNode(HumanWait), got {other:?}"),
    }
    let after = DesignerDag::graph_state_hash(&dag.to_ir().unwrap());
    assert_eq!(before, after, "a refusal must not change graph identity");
}
