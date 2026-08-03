//! Shared board fixtures — the 18 enumeration-class board states (spec
//! S3: "every class is CONSTRUCTED — a class that cannot be built through
//! seed+ops does not exist here"). Originally lived only inside
//! `examples/corpus_gen.rs`; extracted 2026-07-28 so the Phase D eval-set
//! enrichment tool (`examples/eval_enrich.rs`) builds the exact same
//! boards the training corpus was generated against, rather than a
//! second hand-maintained copy that could silently drift from it.
//!
//! One fixture set, like A1's one serializer: the alternative (two
//! independently-authored graph constructors) is exactly the
//! five-independent-declarations disease the DAG-is-normative rule
//! exists to prevent.

use anyhow::Result;
use bpmn_lite_compiler::{ConditionExpr, ConditionLiteral, ConditionOp, IRNode, TimerSpec};
use designer_graph::ops::{apply, GuardTrigger, Operation, RegionBranch};
use designer_graph::schema::{DesignerDag, NodeKey, Provenance};

fn key() -> NodeKey {
    NodeKey(uuid::Uuid::new_v4())
}

fn task(id: &str) -> IRNode {
    IRNode::ServiceTask {
        id: id.into(),
        name: id.into(),
        task_type: "noop".into(),
    }
}

/// One enumeration-class board state: the built graph plus the anchor's
/// BPMN id (None = whole-graph).
pub struct ClassState {
    pub class_id: &'static str,
    pub dag: DesignerDag,
    pub anchor_key: Option<NodeKey>,
    pub anchor_id: Option<&'static str>,
}

pub fn enumeration_classes() -> Result<Vec<ClassState>> {
    let p = Provenance::default;
    let mut out = Vec::new();

    // Shared base: start [+ corr data object], chain via ops.
    let base = |with_data: bool| -> Result<(DesignerDag, NodeKey)> {
        let mut dag = DesignerDag::new("gen-base");
        let start = dag.seed(key(), IRNode::Start { id: "start".into() }, p())?;
        if with_data {
            dag.seed(
                key(),
                IRNode::DataObject {
                    id: "case_ref".into(),
                    name: "case_ref".into(),
                    type_decl: bpmn_lite_types::DataObjectType::Primitive(
                        bpmn_lite_types::PrimitiveType::String,
                    ),
                    role: bpmn_lite_types::DataObjectRole::Internal,
                },
                p(),
            )?;
        }
        Ok((dag, start))
    };

    // empty_graph: NOTA-only board, whole-graph position.
    out.push(ClassState {
        class_id: "empty_graph",
        dag: DesignerDag::new("empty"),
        anchor_key: None,
        anchor_id: None,
    });

    // mid_sequence_task: start→t_review_docs→end, anchored on the task.
    {
        let (dag, start) = base(false)?;
        let t = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("review_documents"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "mid_sequence_task",
            dag: g,
            anchor_key: Some(t),
            anchor_id: Some("review_documents"),
        });
    }

    // guarded_task + guard_node: same graph, two anchors. The guard
    // carries its escalation continuation (escalate_case -> end_esc):
    // a boundary guard without an outgoing escape flow cannot admit
    // (verifier 7c), so the bare shape used before 2026-08-03 was a
    // train/serve skew — the serving path only ever sees admitted
    // graphs, whose guards ALWAYS have a continuation. Root-caused from
    // the money-receipt regression (context projection `end x1` vs
    // `end x2` on otherwise identical boards).
    {
        let (dag, start) = base(false)?;
        let t = key();
        let guard = key();
        let esc = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("chase_client"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
            Operation::AttachRearmingGuard {
                host: t,
                key: guard,
                guard_id: "g_reminder".into(),
                trigger: GuardTrigger::Timer(TimerSpec::Cycle {
                    interval_ms: 86_400_000,
                    max_fires: 3,
                }),
            },
            Operation::AppendNode {
                anchor: guard,
                key: esc,
                node: task("escalate_case"),
                edge_id: "f3".into(),
            },
            Operation::AppendNode {
                anchor: esc,
                key: key(),
                node: IRNode::End { id: "end_esc".into(), terminate: false },
                edge_id: "f4".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "guarded_task",
            dag: g.clone(),
            anchor_key: Some(t),
            anchor_id: Some("chase_client"),
        });
        out.push(ClassState {
            class_id: "guard_node",
            dag: g,
            anchor_key: Some(guard),
            anchor_id: Some("g_reminder"),
        });
    }

    // human_wait: start→prep→human review→end, anchored on the review.
    {
        let (dag, start) = base(true)?;
        let t = key();
        let h = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("prepare_pack"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: h,
                node: IRNode::HumanWait {
                    id: "review_evidence".into(),
                    name: "review_evidence".into(),
                    task_kind: "review".into(),
                    corr_key_source: "case_ref".into(),
                },
                edge_id: "f2".into(),
            },
            Operation::AppendNode {
                anchor: h,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f3".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "human_wait",
            dag: g,
            anchor_key: Some(h),
            anchor_id: Some("review_evidence"),
        });
    }

    // send_task anchor.
    {
        let (dag, start) = base(true)?;
        let s = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: s,
                node: IRNode::SendTask {
                    id: "notify_client".into(),
                    name: "notify_client".into(),
                    message_name: "client_notice".into(),
                    corr_key_source: "case_ref".into(),
                },
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: s,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "send_task",
            dag: g,
            anchor_key: Some(s),
            anchor_id: Some("notify_client"),
        });
    }

    // xor_gateway (with a legal forward target for CreateBranch) +
    // downstream shared end.
    {
        let (dag, start) = base(false)?;
        let t = key();
        let x = key();
        let h1 = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("assess_case"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: x,
                node: IRNode::GatewayXor { id: "outcome".into(), name: "outcome".into() },
                edge_id: "f2".into(),
            },
            Operation::AppendNode {
                anchor: x,
                key: h1,
                node: task("handle_approved"),
                edge_id: "f3".into(),
            },
            Operation::AppendNode {
                anchor: h1,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f4".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "xor_gateway",
            dag: g,
            anchor_key: Some(x),
            anchor_id: Some("outcome"),
        });
    }

    // parallel_branch_interior + mi_node: region constructs, anchored inside.
    {
        let (dag, start) = base(false)?;
        let t = key();
        let b1 = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("collect_inputs"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
            Operation::CreateParallelRegion {
                anchor: t,
                fork_key: key(),
                fork_node_id: "fork1".into(),
                join_key: key(),
                join_node_id: "join1".into(),
                entry_edge_id: "f_fork".into(),
                branches: vec![
                    designer_graph::ops::RegionBranch {
                        key: b1,
                        node: task("screen_sanctions"),
                        in_edge_id: "b1_in".into(),
                        out_edge_id: "b1_out".into(),
                        condition: None,
                    },
                    designer_graph::ops::RegionBranch {
                        key: key(),
                        node: task("screen_pep"),
                        in_edge_id: "b2_in".into(),
                        out_edge_id: "b2_out".into(),
                        condition: None,
                    },
                ],
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "parallel_branch_interior",
            dag: g,
            anchor_key: Some(b1),
            anchor_id: Some("screen_sanctions"),
        });
    }
    {
        let (dag, start) = base(false)?;
        let t = key();
        let mi = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("gather_documents"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
            Operation::CreateMultiInstanceRegion {
                anchor: t,
                key: mi,
                node: IRNode::MultiInstance {
                    id: "verify_each_document".into(),
                    name: "verify_each_document".into(),
                    task_type: "noop".into(),
                    collection_flag_name: "documents".into(),
                    declared_max: 10,
                },
                edge_id: "f_mi".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "mi_node",
            dag: g,
            anchor_key: Some(mi),
            anchor_id: Some("verify_each_document"),
        });
    }

    // end_anchor / start_anchor / data_object: positional reuse of a
    // simple chain graph.
    {
        let (dag, start) = base(true)?;
        let t = key();
        let e = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("finalise_case"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: e,
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "end_anchor",
            dag: g.clone(),
            anchor_key: Some(e),
            anchor_id: Some("end"),
        });
        out.push(ClassState {
            class_id: "start_anchor",
            dag: g.clone(),
            anchor_key: Some(start),
            anchor_id: Some("start"),
        });
    }
    // data_object anchor: dedicated graph so the key is in hand.
    {
        let mut dag = DesignerDag::new("gen-data");
        let start = dag.seed(key(), IRNode::Start { id: "start".into() }, p())?;
        let d = dag.seed(
            key(),
            IRNode::DataObject {
                id: "case_ref".into(),
                name: "case_ref".into(),
                type_decl: bpmn_lite_types::DataObjectType::Primitive(
                    bpmn_lite_types::PrimitiveType::String,
                ),
                role: bpmn_lite_types::DataObjectRole::Internal,
            },
            p(),
        )?;
        let t = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("register_case"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "data_object",
            dag: g,
            anchor_key: Some(d),
            anchor_id: Some("case_ref"),
        });
    }

    // message_wait: start→t_send→wait→end, anchored on the wait.
    {
        let (dag, start) = base(true)?;
        let t = key();
        let w = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("send_request"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: w,
                node: IRNode::MessageWait {
                    id: "await_documents".into(),
                    name: "await_documents".into(),
                    corr_key_source: "case_ref".into(),
                },
                edge_id: "f2".into(),
            },
            Operation::AppendNode {
                anchor: w,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f3".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "message_wait",
            dag: g,
            anchor_key: Some(w),
            anchor_id: Some("await_documents"),
        });
    }

    // timer_wait: a bare inline timer (not a boundary guard) — start ->
    // prep -> timer_wait -> end, anchored on the wait itself.
    {
        let (dag, start) = base(false)?;
        let t = key();
        let w = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("prepare_dispatch"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: w,
                node: IRNode::TimerWait {
                    id: "cooling_off_period".into(),
                    spec: TimerSpec::Duration { ms: 3 * 86_400_000 },
                },
                edge_id: "f2".into(),
            },
            Operation::AppendNode {
                anchor: w,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f3".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "timer_wait",
            dag: g,
            anchor_key: Some(w),
            anchor_id: Some("cooling_off_period"),
        });
    }

    // boundary_error: an interrupting error guard on a task, with its own
    // escape path (verifier 7c — a boundary guard needs an outgoing flow
    // distinct from its host's). Anchored on the guard itself.
    {
        let (dag, start) = base(false)?;
        let t = key();
        let guard = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("submit_filing"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
            Operation::AttachGuard {
                host: t,
                key: guard,
                guard_id: "on_filing_rejected".into(),
                trigger: GuardTrigger::Error {
                    error_code: Some("FILING_REJECTED".into()),
                },
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        g = apply(
            &g,
            Operation::AppendNode {
                anchor: guard,
                key: key(),
                node: IRNode::End { id: "end_rejected".into(), terminate: false },
                edge_id: "f_guard_out".into(),
            },
            p(),
        )?
        .candidate;
        out.push(ClassState {
            class_id: "boundary_error",
            dag: g,
            anchor_key: Some(guard),
            anchor_id: Some("on_filing_rejected"),
        });
    }

    // ffi_service_task: bare FFI-dispatched task (Zeebe-style external
    // job), anchored on the task itself.
    {
        let (dag, start) = base(false)?;
        let t = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: IRNode::FfiServiceTask {
                    id: "run_sanctions_screen".into(),
                    name: "run_sanctions_screen".into(),
                    template_id: [0u8; 32],
                    inputs: vec![],
                    outputs: vec![],
                },
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "ffi_service_task",
            dag: g,
            anchor_key: Some(t),
            anchor_id: Some("run_sanctions_screen"),
        });
    }

    // and_gateway_node / or_gateway_node: the fork node ITSELF as anchor
    // (parallel_branch_interior anchors INSIDE the region — this pair
    // covers the gateway node as the position, closing the gap for
    // utterances that address the gate directly rather than a branch).
    {
        let (dag, start) = base(false)?;
        let t = key();
        let fork = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("intake_application"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
            Operation::CreateParallelRegion {
                anchor: t,
                fork_key: fork,
                fork_node_id: "checks_fork".into(),
                join_key: key(),
                join_node_id: "checks_join".into(),
                entry_edge_id: "f_fork".into(),
                branches: vec![
                    RegionBranch {
                        key: key(),
                        node: task("screen_sanctions"),
                        in_edge_id: "b1_in".into(),
                        out_edge_id: "b1_out".into(),
                        condition: None,
                    },
                    RegionBranch {
                        key: key(),
                        node: task("screen_pep"),
                        in_edge_id: "b2_in".into(),
                        out_edge_id: "b2_out".into(),
                        condition: None,
                    },
                ],
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "and_gateway_node",
            dag: g,
            anchor_key: Some(fork),
            anchor_id: Some("checks_fork"),
        });
    }
    {
        let (dag, start) = base(false)?;
        let t = key();
        let fork = key();
        let mut g = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t,
                node: task("assess_risk_profile"),
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t,
                key: key(),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
            Operation::CreateInclusiveRegion {
                anchor: t,
                fork_key: fork,
                fork_node_id: "notify_fork".into(),
                join_key: key(),
                join_node_id: "notify_join".into(),
                entry_edge_id: "f_fork".into(),
                branches: vec![
                    RegionBranch {
                        key: key(),
                        node: task("notify_compliance"),
                        in_edge_id: "b1_in".into(),
                        out_edge_id: "b1_out".into(),
                        condition: Some(ConditionExpr {
                            flag_name: "high_risk".into(),
                            op: ConditionOp::Eq,
                            literal: ConditionLiteral::Bool(true),
                        }),
                    },
                    RegionBranch {
                        key: key(),
                        node: task("notify_relationship_manager"),
                        in_edge_id: "b2_in".into(),
                        out_edge_id: "b2_out".into(),
                        condition: Some(ConditionExpr {
                            flag_name: "client_facing".into(),
                            op: ConditionOp::Eq,
                            literal: ConditionLiteral::Bool(true),
                        }),
                    },
                ],
            },
        ] {
            g = apply(&g, op, p())?.candidate;
        }
        out.push(ClassState {
            class_id: "or_gateway_node",
            dag: g,
            anchor_key: Some(fork),
            anchor_id: Some("notify_fork"),
        });
    }

    Ok(out)
}
