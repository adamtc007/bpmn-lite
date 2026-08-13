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
    let t = dag
        .insert_node(
            key(),
            IRNode::TimerWait {
                id: "wait".into(),
                spec: bpmn_lite_compiler::TimerSpec::Duration { ms: 1000 },
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
            assert_eq!(*kind, "TimerWait");
        }
        other => panic!("expected UnsupportedNode(TimerWait), got {other:?}"),
    }
    let after = DesignerDag::graph_state_hash(&dag.to_ir().unwrap());
    assert_eq!(before, after, "a refusal must not change graph identity");
}
