//! G2 receipts — EOP-VS-BPMN-DESIGN-003 §16 Phase-D2 gate, Designer-graph
//! half: "a competent author can construct … the solicit-document workflow
//! (§6.3) end to end with every declaration surviving round-trip, and every
//! deliberately-invalid edit in a red-team script is refused at staging
//! with the correct theorem named."
//!
//! §6.3's temporal impl is a LINEAR chain — create solicitation → resolve
//! route → send request → guard the wait with a bounded reminder cycle
//! (GUARD-N> + GUARD-TIMER-CYCLE>) → register document → review evidence →
//! return. The four-way `Received | Rejected | Expired | Cancelled` is the
//! durable VERB's typed return (§6.3 "Durable contract"), not four End
//! nodes: no gateway is authored here.
//!
//! **SUBSTRATE FORK (surfaced 2026-07-27, G2 blocked-in-part on Adam's
//! ruling):** §6.3 says "guard the WAIT". The substrate cannot: the
//! verifier's legal BoundaryTimer hosts are ServiceTask | FfiServiceTask |
//! HumanWait (verifier.rs §7a), so a reminder guard on the document
//! MessageWait is REJECTED at admission (fail-closed — receipt below), and
//! a guard on a HumanWait host is ADMITTED but silently DROPPED by
//! lowering (the HumanWait arm never consults `boundary_lookup` —
//! fail-OPEN defect, receipt below; flip that test when fixed). Guards
//! lower correctly on task hosts only. Until the ruling, this file proves
//! everything else end-to-end and cements both gap behaviours.

#![cfg(test)]

use crate::ops::{apply, GuardTrigger, Operation};
use crate::productions::{apply_production, request_and_wait, RequestAndWaitBindings};
use crate::schema::{DesignerDag, NodeKey, Provenance};
use bpmn_lite_compiler::ir::{IRNode, TimerSpec};
use bpmn_lite_types::ffi_bindings::{DataObjectRole, DataObjectType, PrimitiveType};
use bpmn_lite_types::types::Instr;
use uuid::Uuid;

fn key() -> NodeKey {
    NodeKey(Uuid::new_v4())
}

fn task(id: &str) -> IRNode {
    IRNode::ServiceTask {
        id: id.into(),
        name: id.into(),
        task_type: "noop".into(),
    }
}

fn reminder_trigger() -> GuardTrigger {
    // §6.3's bounded reminder cycle: daily, at most 3 fires
    // (GUARD-N> + GUARD-TIMER-CYCLE> once lowered).
    GuardTrigger::Timer(TimerSpec::Cycle { interval_ms: 86_400_000, max_fires: 3 })
}

/// Every key the edit log references, so red-team edits can name real nodes.
struct SolicitKeys {
    start: NodeKey,
    create: NodeKey,
    resolve: NodeKey,
    send: NodeKey,
    wait: NodeKey,
    register: NodeKey,
    review: NodeKey,
}

/// Base for the authoring session: a Start node plus the declared
/// correlation data object. Everything else arrives through the edit log.
fn solicit_base() -> (DesignerDag, NodeKey) {
    let mut dag = DesignerDag::new("solicit_document");
    dag.default_guard_budget = Some(3);
    let start = dag
        .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
        .unwrap();
    dag.insert_node(
        key(),
        IRNode::DataObject {
            id: "solicitation_ref".into(),
            name: "solicitation_ref".into(),
            type_decl: DataObjectType::Primitive(PrimitiveType::String),
            role: DataObjectRole::Internal,
        },
        None,
        Provenance::default(),
    )
    .unwrap();
    (dag, start)
}

/// The §6.3 chain as ONE `Vec<Operation>` (Q5: this vector IS the persisted
/// session artifact) — minus the reminder guard, which is the surfaced
/// fork (module docs). `review_evidence` is a real HumanWait: §6.3's
/// review step is human review, correlated like the wait.
fn solicit_ops(k: &SolicitKeys) -> Vec<Operation> {
    let mut ops = vec![
        Operation::AppendNode {
            anchor: k.start,
            key: k.create,
            node: task("create_solicitation"),
            edge_id: "f_create".into(),
        },
        Operation::AppendNode {
            anchor: k.create,
            key: k.resolve,
            node: task("resolve_route"),
            edge_id: "f_resolve".into(),
        },
    ];
    ops.extend(request_and_wait(RequestAndWaitBindings {
        anchor: k.resolve,
        send_key: k.send,
        send_node: task("send_request"),
        send_edge_id: "f_send".into(),
        wait_key: k.wait,
        wait_node: IRNode::MessageWait {
            id: "wait_document".into(),
            name: "wait_document".into(),
            corr_key_source: "solicitation_ref".into(),
        },
        wait_edge_id: "f_wait".into(),
    }));
    ops.extend([
        Operation::AppendNode {
            anchor: k.wait,
            key: k.register,
            node: task("register_document"),
            edge_id: "f_register".into(),
        },
        Operation::AppendNode {
            anchor: k.register,
            key: k.review,
            node: IRNode::HumanWait {
                id: "review_evidence".into(),
                name: "review_evidence".into(),
                task_kind: "review".into(),
                corr_key_source: "solicitation_ref".into(),
            },
            edge_id: "f_review".into(),
        },
        Operation::AppendNode {
            anchor: k.review,
            key: key(),
            node: IRNode::End { id: "end_solicit".into(), terminate: false },
            edge_id: "f_end".into(),
        },
    ]);
    ops
}

fn build_solicit() -> (DesignerDag, SolicitKeys, Vec<Operation>) {
    let (base, start) = solicit_base();
    let keys = SolicitKeys {
        start,
        create: key(),
        resolve: key(),
        send: key(),
        wait: key(),
        register: key(),
        review: key(),
    };
    let ops = solicit_ops(&keys);
    (base, keys, ops)
}

/// GREEN half of the gate: the §6.3 chain, authored entirely through the
/// edit log, admits through the FULL direct-compilation chain; the process
/// default budget and both correlation declarations survive round-trip.
#[test]
fn g2_solicit_document_admits_end_to_end() {
    let (base, _keys, ops) = build_solicit();
    let staged = apply_production(&base, ops, Provenance::default())
        .expect("the §6.3 edit log must stage");
    let wf = staged
        .candidate
        .admit()
        .expect("solicit_document must admit through verify + lowering");

    assert_eq!(
        wf.envelope().metadata().default_guard_budget().max_failures(),
        3,
        "process default_guard_budget must reach the sealed envelope"
    );
    let ir = staged.candidate.to_ir().expect("projection must succeed");
    assert!(
        ir.node_weights().any(|n| matches!(
            n,
            IRNode::MessageWait { id, corr_key_source, .. }
                if id == "wait_document" && corr_key_source == "solicitation_ref"
        )),
        "wait correlation declaration must survive"
    );
    assert!(
        ir.node_weights().any(|n| matches!(
            n,
            IRNode::HumanWait { id, corr_key_source, .. }
                if id == "review_evidence" && corr_key_source == "solicitation_ref"
        )),
        "human-review correlation declaration must survive"
    );
}

/// Q5 receipt on the same artifact: the WHOLE §6.3 edit log round-trips
/// serde and re-applies to a bit-identical projection.
#[test]
fn g2_solicit_edit_log_round_trips() {
    let (base, _keys, ops) = build_solicit();
    let json = serde_json::to_string(&ops).expect("edit log must serialize");
    let back: Vec<Operation> = serde_json::from_str(&json).expect("edit log must deserialize");
    let a = apply_production(&base, ops, Provenance::default()).unwrap();
    let b = apply_production(&base, back, Provenance::default()).unwrap();
    assert_eq!(
        format!("{:?}", a.candidate.to_ir().unwrap()),
        format!("{:?}", b.candidate.to_ir().unwrap()),
        "replayed edit log must project identically"
    );
}

/// Declarations round-trip on a SUPPORTED guard host (ServiceTask): the
/// reminder cycle spec, non-interrupting kind, per-guard budget override,
/// and GUARD-N>/GUARD-TIMER-CYCLE> opcodes all reach the sealed envelope.
#[test]
fn g2_guard_declarations_survive_on_task_host() {
    let (base, keys, ops) = build_solicit();
    let staged = apply_production(&base, ops, Provenance::default()).unwrap();
    let guard_key = key();
    let reminder_key = key();
    let mut graph = staged.candidate;
    for op in [
        Operation::AttachRearmingGuard {
            host: keys.send,
            key: guard_key,
            guard_id: "g_reminder".into(),
            trigger: reminder_trigger(),
        },
        Operation::SetGuardBudget { guard: guard_key, failure_budget: Some(2) },
        // slice-2 topology rule: the guard's escape path owns its terminal
        Operation::AppendNode {
            anchor: guard_key,
            key: reminder_key,
            node: task("send_reminder"),
            edge_id: "f_reminder".into(),
        },
        Operation::AppendNode {
            anchor: reminder_key,
            key: key(),
            node: IRNode::End { id: "end_reminder".into(), terminate: false },
            edge_id: "f_reminder_end".into(),
        },
    ] {
        graph = apply(&graph, op, Provenance::default()).expect("guard edits must stage").candidate;
    }

    let wf = graph.admit().expect("guarded task host must admit");
    let instrs = wf.envelope().instructions();
    assert!(
        instrs.iter().any(|i| matches!(i, Instr::V2GuardN { .. })),
        "GUARD-N> must be emitted for the non-interrupting reminder guard"
    );
    assert!(
        instrs.iter().any(|i| matches!(i, Instr::V2GuardTimerCycle { max_fires: 3 })),
        "GUARD-TIMER-CYCLE> with the declared bound must be emitted"
    );
    let ir = graph.to_ir().unwrap();
    assert!(
        ir.node_weights().any(|n| matches!(
            n,
            IRNode::BoundaryTimer { id, interrupting: false, failure_budget: Some(2),
                spec: TimerSpec::Cycle { interval_ms: 86_400_000, max_fires: 3 }, .. }
                if id == "g_reminder"
        )),
        "cycle spec + budget override declarations must survive projection"
    );
}

/// FORK RECEIPT (fail-closed half): §6.3's literal shape — the reminder
/// guard on the document MessageWait — stages in the Designer but is
/// REJECTED at admission with the verifier naming guard and host. This is
/// the substrate/spec mismatch surfaced to Adam; the test cements that
/// the refusal is loud, not silent.
#[test]
fn g2_fork_receipt_reminder_guard_on_message_wait_rejected_at_admission() {
    let (base, keys, ops) = build_solicit();
    let staged = apply_production(&base, ops, Provenance::default()).unwrap();
    let broken = apply(
        &staged.candidate,
        Operation::AttachRearmingGuard {
            host: keys.wait,
            key: key(),
            guard_id: "g_reminder".into(),
            trigger: reminder_trigger(),
        },
        Provenance::default(),
    )
    .expect("staging accepts the edit — the theorem lives in the verifier");
    let errs = broken.candidate.admit().expect_err("guard on MessageWait must reject");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("g_reminder") && e.message.contains("wait_document")),
        "refusal must name guard and host: {errs:?}"
    );
}

/// FAIL-OPEN DEFECT — CLOSED (red→green, 2026-07-27). RED (proven before
/// the fix): the verifier listed HumanWait as a legal BoundaryTimer host,
/// but lowering's HumanWait arm never consults `boundary_lookup`, so a
/// reminder guard on the review step ADMITTED with ZERO guard opcodes in
/// the envelope — never armed, escalation chain orphaned. GREEN (cemented
/// here): the verifier now rejects the un-lowerable host loudly, naming
/// guard and host. If Adam rules to support guard-on-wait (§6.3's literal
/// shape), lowering learns to wrap wait hosts FIRST, then this cement is
/// rewritten to prove the emitted guard scope.
#[test]
fn g2_boundary_timer_on_human_wait_rejected_not_dropped() {
    let (base, keys, ops) = build_solicit();
    let staged = apply_production(&base, ops, Provenance::default()).unwrap();
    let guard_key = key();
    let reminder_key = key();
    let mut graph = staged.candidate;
    for op in [
        Operation::AttachRearmingGuard {
            host: keys.review,
            key: guard_key,
            guard_id: "g_review_reminder".into(),
            trigger: reminder_trigger(),
        },
        Operation::AppendNode {
            anchor: guard_key,
            key: reminder_key,
            node: task("review_nudge"),
            edge_id: "f_nudge".into(),
        },
        Operation::AppendNode {
            anchor: reminder_key,
            key: key(),
            node: IRNode::End { id: "end_nudge".into(), terminate: false },
            edge_id: "f_nudge_end".into(),
        },
    ] {
        graph = apply(&graph, op, Provenance::default()).expect("staging accepts the edit").candidate;
    }
    let errs = graph
        .admit()
        .expect_err("guard on a HumanWait host must reject, not silently drop");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("g_review_reminder")
                && e.message.contains("review_evidence")),
        "refusal must name guard and host: {errs:?}"
    );
}

/// RED half of the gate — the red-team script. Each deliberately-invalid
/// edit against the STAGED solicit-document graph must be refused with a
/// diagnostic naming the offending element and the violated rule; the
/// staged candidate is never mutated (I18 — re-admission stays green).
#[test]
fn g2_red_team_script_every_invalid_edit_refused_with_theorem_named() {
    let (base, keys, ops) = build_solicit();
    let staged = apply_production(&base, ops, Provenance::default()).unwrap();
    // Arm a guard on the (supported) send task so guarded-host rules fire.
    let graph = apply(
        &staged.candidate,
        Operation::AttachRearmingGuard {
            host: keys.send,
            key: key(),
            guard_id: "g_reminder".into(),
            trigger: reminder_trigger(),
        },
        Provenance::default(),
    )
    .unwrap()
    .candidate;

    // (1) F3 duplicate BPMN id: a second create_solicitation.
    let err = apply(
        &graph,
        Operation::InsertAfter {
            anchor: keys.review,
            key: key(),
            node: task("create_solicitation"),
            edge_id: "f_dup".into(),
        },
        Provenance::default(),
    )
    .expect_err("duplicate BPMN id must refuse");
    assert!(err.to_string().contains("create_solicitation"), "must name the id: {err}");

    // (2) I23 backward edge: review_evidence → create_solicitation.
    let err = apply(
        &graph,
        Operation::Connect {
            from: keys.review,
            to: keys.create,
            edge_id: "f_back".into(),
            condition: None,
        },
        Provenance::default(),
    )
    .expect_err("backward connect must refuse (I23)");
    let msg = err.to_string();
    assert!(
        msg.contains("review_evidence") && msg.contains("create_solicitation"),
        "must name both ids: {msg}"
    );

    // (3) Guarded-host protection: deleting the send task would dangle g_reminder.
    let err = apply(&graph, Operation::DeleteNode { target: keys.send }, Provenance::default())
        .expect_err("deleting a guarded host must refuse");
    let msg = err.to_string();
    assert!(
        msg.contains("send_request") && msg.contains("g_reminder"),
        "must name host and guard: {msg}"
    );

    // (4) Same rule on ReplaceNode: replace-under-guard is explicit, never implicit.
    let err = apply(
        &graph,
        Operation::ReplaceNode { target: keys.send, key: key(), node: task("send_request") },
        Provenance::default(),
    )
    .expect_err("replacing a guarded host must refuse");
    assert!(err.to_string().contains("send_request"), "must name the host: {err}");

    // (5) Boundary rule 6: a cycle trigger on an INTERRUPTING guard is a
    // contradiction (an interrupting guard cannot re-fire).
    let err = apply(
        &graph,
        Operation::AttachGuard {
            host: keys.register,
            key: key(),
            guard_id: "g_bad_cycle".into(),
            trigger: GuardTrigger::Timer(TimerSpec::Cycle { interval_ms: 1000, max_fires: 2 }),
        },
        Provenance::default(),
    )
    .expect_err("cycle trigger on interrupting guard must refuse");
    assert!(err.to_string().contains("g_bad_cycle"), "must name the guard: {err}");

    // (6) Admission-level theorem: correlation source naming an undeclared
    // data object must REJECT at admit(), naming the missing producer.
    // (Staging accepts the edit — the theorem lives in the verifier, and
    // the gate is that it FIRES, not that staging pre-guesses it.)
    let broken = apply(
        &staged.candidate,
        Operation::SetCorrelationSource {
            node: keys.wait,
            corr_key_source: "undeclared_ref".into(),
        },
        Provenance::default(),
    )
    .expect("setting the correlation source is a legal edit");
    let errs = broken
        .candidate
        .admit()
        .expect_err("undeclared correlation source must reject at admission");
    assert!(
        errs.iter().any(|e| e.message.contains("undeclared_ref")),
        "verifier must name the missing producer: {errs:?}"
    );

    // I18 backstop: after the whole script, the staged graph still admits.
    staged.candidate.admit().expect("red-team script must never mutate the staged graph");
}
