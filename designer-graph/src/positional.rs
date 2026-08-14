//! Positional legality — the real `LegalityOracle` over a `DesignerDag`
//! (V&S §11.7: "the board changes with position"; DIR-002 A2's board-state
//! enumeration depends on this being real, not whole-graph-everything).
//!
//! Legality here means: **the candidate is meaningfully proposable at this
//! anchor** — its canonical instantiation stages through `ops::apply`, AND
//! it does not violate a KNOWN admission theorem of the substrate. The
//! second clause matters because some theorems live in the verifier, not
//! staging (the F-DSGN-3 host rule: guards attach to task hosts only) — a
//! board that proposes a candidate the verifier is guaranteed to reject
//! is lying to the user and poisoning the training corpus.
//!
//! Two-layer rule table (each row cites its enforcement site):
//! - staging-enforced: `AppendNode` needs an outgoing-free anchor;
//!   `DeleteNode`/`ReplaceNode` refuse guarded hosts; `CreateBranch`
//!   needs a `GatewayXor`; forward-only targets (I23). Mirrored here,
//!   consistency-cemented against `apply` below.
//! - admission-enforced: guard attachment on `ServiceTask`/`FfiServiceTask`
//!   only (verifier §7a, F-DSGN-3); correlation sources live on
//!   wait/send nodes. Mirrored here, cemented by the G2 receipts.
//!
//! Exclusions (never on any board): `CloseParallelRegion` (regions
//! constructed closed), `HumanReviewWithRework` (representable since
//! the XOR trace, but the production is not yet implemented — a board
//! never proposes what the builder cannot construct). `CreateRace`/
//! `TimerMessageRace`, `CallSubprocess`/`CallDurableSubprocess`, and
//! `AttachRollbackGuard` (2026-07-27 traces: no frontend path —
//! kernel-complete, IR-absent) are no longer catalogue entries at all
//! (G1.3, 2026-08-11) — removed rather than perpetually excluded.
//!
//! `anchor = None` (whole-graph position) is the union over all nodes —
//! deterministic, sorted, deduped. An empty graph legitimately yields an
//! empty candidate set (the board is then NOTA-only): no operation in the
//! catalogue applies without an anchor node.

use crate::board_candidate::{LegalityOracle, OperationKind, ProductionId};
use crate::schema::{DesignerDag, NodeKey};
use bpmn_lite_compiler::IRNode;
use petgraph::Direction;

/// The positional oracle: borrows the dag it answers for.
pub struct PositionalLegality<'a> {
    pub dag: &'a DesignerDag,
}

fn is_task_host(ir: &IRNode) -> bool {
    matches!(
        ir,
        IRNode::ServiceTask { .. } | IRNode::FfiServiceTask { .. }
    )
}

/// Legal guard hosts = verifier §7a's set: task hosts PLUS MessageWait/
/// HumanWait since the guarded-wait ruling (Adam, 2026-07-27) — lowering
/// wraps wait bodies in the guard scope. SendTask is NOT a host.
fn is_guard_host(ir: &IRNode) -> bool {
    is_task_host(ir) || matches!(ir, IRNode::MessageWait { .. } | IRNode::HumanWait { .. })
}

fn is_guard(ir: &IRNode) -> bool {
    matches!(
        ir,
        IRNode::BoundaryTimer { .. } | IRNode::BoundaryError { .. }
    )
}

fn is_corr_carrier(ir: &IRNode) -> bool {
    matches!(
        ir,
        IRNode::MessageWait { .. } | IRNode::HumanWait { .. } | IRNode::SendTask { .. }
    )
}

/// Sequence-flow participants: everything except structural declarations
/// (`DataObject`) and boundary guards (which carry no normal-path flow).
fn is_flow(ir: &IRNode) -> bool {
    !matches!(ir, IRNode::DataObject { .. }) && !is_guard(ir)
}

fn is_end(ir: &IRNode) -> bool {
    matches!(ir, IRNode::End { .. })
}

fn is_start(ir: &IRNode) -> bool {
    matches!(ir, IRNode::Start { .. })
}

impl<'a> PositionalLegality<'a> {
    fn ops_at(&self, key: NodeKey) -> Vec<OperationKind> {
        let Some(idx) = self.dag.index_of(key) else {
            return Vec::new(); // unknown anchor: nothing is proposable
        };
        let g = self.dag.graph();
        let ir = &g[idx].ir;
        let has_outgoing = g.edges_directed(idx, Direction::Outgoing).next().is_some();
        let guarded = !self.dag.nodes_attached_to(key).is_empty();
        // A forward target exists if some OTHER flow node is not an
        // ancestor of the anchor (I23 pre-gate would admit the edge).
        let has_forward_target = g.node_indices().any(|other| {
            other != idx
                && is_flow(&g[other].ir)
                && !petgraph::algo::has_path_connecting(g, other, idx, None)
        });

        let mut out = Vec::new();
        if is_flow(ir) {
            if !is_end(ir) {
                out.push(OperationKind::InsertAfter);
                out.push(OperationKind::CreateParallelRegion);
                out.push(OperationKind::CreateInclusiveRegion);
                out.push(OperationKind::CreateMultiInstanceRegion);
            }
            if !is_start(ir) {
                out.push(OperationKind::InsertBefore);
            }
            if has_forward_target {
                out.push(OperationKind::Connect);
            }
        }
        // G7.1 (ruled 2026-08-13): CreateDataObject is a structural
        // declaration, not flow, and has no natural anchor. Offered only
        // at Start — always exactly one, always present post-seed — so
        // it stays utterance-reachable without a new anchorless
        // LegalityOracle path.
        if is_start(ir) {
            out.push(OperationKind::CreateDataObject);
        }
        // AppendNode: chain extension — flow (non-End) or a guard opening
        // its escape path; only where no outgoing edge exists (staging
        // rule, mirrored).
        if !has_outgoing && ((is_flow(ir) && !is_end(ir)) || is_guard(ir)) {
            out.push(OperationKind::AppendNode);
        }
        if !guarded && !is_start(ir) {
            out.push(OperationKind::DeleteSubgraph);
            if is_flow(ir) {
                out.push(OperationKind::ReplaceNode);
            }
        }
        if is_guard_host(ir) {
            // Verifier §7a alignment (guarded-wait ruling): guards attach
            // to task hosts AND MessageWait/HumanWait — all four lowered.
            out.push(OperationKind::AttachGuard);
            out.push(OperationKind::AttachRearmingGuard);
        }
        if is_guard(ir) {
            out.push(OperationKind::SetGuardTrigger);
            out.push(OperationKind::SetGuardBudget);
        }
        if is_corr_carrier(ir) {
            out.push(OperationKind::SetCorrelationSource);
        }
        if matches!(ir, IRNode::GatewayXor { .. }) && has_forward_target {
            out.push(OperationKind::CreateBranch);
        }
        out
    }

    fn prods_at(&self, key: NodeKey) -> Vec<ProductionId> {
        let Some(idx) = self.dag.index_of(key) else {
            return Vec::new();
        };
        let ir = &self.dag.graph()[idx].ir;
        let mut out = Vec::new();
        if is_flow(ir) && !is_end(ir) {
            out.push(ProductionId::RequestAndWait);
        }
        if is_guard_host(ir) {
            out.push(ProductionId::ReminderThenEscalate);
            out.push(ProductionId::InterruptingTimeout);
            out.push(ProductionId::NonInterruptingNotification);
        }
        // HumanReviewWithRework: representable but unimplemented. Never
        // proposed.
        out
    }

    fn all_keys(&self) -> Vec<NodeKey> {
        self.dag.graph().node_weights().map(|n| n.key).collect()
    }
}

impl<'a> LegalityOracle for PositionalLegality<'a> {
    type NodeKey = NodeKey;

    fn legal_operations(&self, anchor: Option<&NodeKey>) -> Vec<OperationKind> {
        let mut out = match anchor {
            Some(k) => self.ops_at(*k),
            None => self
                .all_keys()
                .into_iter()
                .flat_map(|k| self.ops_at(k))
                .collect(),
        };
        out.sort();
        out.dedup();
        out
    }

    fn legal_productions(&self, anchor: Option<&NodeKey>) -> Vec<ProductionId> {
        let mut out = match anchor {
            Some(k) => self.prods_at(*k),
            None => self
                .all_keys()
                .into_iter()
                .flat_map(|k| self.prods_at(k))
                .collect(),
        };
        out.sort();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{apply, GuardTrigger, Operation};
    use crate::schema::{DesignerEdge, Provenance};
    use bpmn_lite_compiler::TimerSpec;
    use uuid::Uuid;

    fn key() -> NodeKey {
        NodeKey(Uuid::new_v4())
    }

    fn task(id: &str) -> IRNode {
        IRNode::ServiceTask {
            id: id.into(),
            name: id.into(),
            task_type: "noop".into(),
            loop_origin: None,
        }
    }

    struct Fx {
        dag: DesignerDag,
        start: NodeKey,
        t1: NodeKey,
        wait: NodeKey,
        guard: NodeKey,
        end: NodeKey,
        data: NodeKey,
    }

    /// start → t1(ServiceTask, guarded) → wait(MessageWait) → end, plus a
    /// DataObject and the guard on t1.
    fn fixture() -> Fx {
        let mut dag = DesignerDag::new("positional-fx");
        let p = Provenance::default;
        let start = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, p())
            .unwrap();
        let t1 = dag.insert_node(key(), task("t1"), None, p()).unwrap();
        let wait = dag
            .insert_node(
                key(),
                IRNode::MessageWait {
                    id: "wait1".into(),
                    name: "wait1".into(),
                    corr_key_source: "ref1".into(),
                },
                None,
                p(),
            )
            .unwrap();
        let end = dag
            .insert_node(
                key(),
                IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                None,
                p(),
            )
            .unwrap();
        let data = dag
            .insert_node(
                key(),
                IRNode::DataObject {
                    id: "ref1".into(),
                    name: "ref1".into(),
                    type_decl: bpmn_lite_types::DataObjectType::Primitive(
                        bpmn_lite_types::PrimitiveType::String,
                    ),
                    role: bpmn_lite_types::DataObjectRole::Internal,
                },
                None,
                p(),
            )
            .unwrap();
        for (a, b, id) in [(start, t1, "e1"), (t1, wait, "e2"), (wait, end, "e3")] {
            dag.insert_edge(
                a,
                b,
                DesignerEdge {
                    id: id.into(),
                    condition: None,
                    provenance: p(),
                },
            )
            .unwrap();
        }
        let guard = dag
            .insert_node(
                key(),
                IRNode::BoundaryTimer {
                    id: "g1".into(),
                    attached_to: "t1".into(),
                    spec: TimerSpec::Cycle {
                        interval_ms: 1000,
                        max_fires: 2,
                    },
                    interrupting: false,
                    failure_budget: None,
                },
                Some(t1),
                p(),
            )
            .unwrap();
        Fx {
            dag,
            start,
            t1,
            wait,
            guard,
            end,
            data,
        }
    }

    fn ops(fx: &Fx, k: NodeKey) -> Vec<OperationKind> {
        PositionalLegality { dag: &fx.dag }.legal_operations(Some(&k))
    }

    /// Verifier-§7a alignment cement (guarded-wait ruling): guard
    /// attachment is proposed at task hosts AND waits — matching exactly
    /// the hosts lowering wraps (G2 guarded-wait receipts are the
    /// admission-side proof). The board never proposes what the verifier
    /// rejects, and never withholds what it admits.
    #[test]
    fn guard_attachment_offered_on_all_lowerable_hosts() {
        let fx = fixture();
        assert!(ops(&fx, fx.t1).contains(&OperationKind::AttachRearmingGuard));
        assert!(ops(&fx, fx.wait).contains(&OperationKind::AttachRearmingGuard));
        assert!(ops(&fx, fx.wait).contains(&OperationKind::AttachGuard));
        assert!(
            PositionalLegality { dag: &fx.dag }
                .legal_productions(Some(&fx.wait))
                .contains(&ProductionId::ReminderThenEscalate),
            "reminder production follows guard-host legality"
        );
    }

    /// Correlation sources are wait/send business; guard knobs are guard
    /// business; data objects propose nothing.
    #[test]
    fn kind_scoped_candidates() {
        let fx = fixture();
        assert!(ops(&fx, fx.wait).contains(&OperationKind::SetCorrelationSource));
        assert!(!ops(&fx, fx.t1).contains(&OperationKind::SetCorrelationSource));
        let guard_ops = ops(&fx, fx.guard);
        assert!(guard_ops.contains(&OperationKind::SetGuardBudget));
        assert!(guard_ops.contains(&OperationKind::SetGuardTrigger));
        assert!(
            guard_ops.contains(&OperationKind::AppendNode),
            "guard escape path opens here"
        );
        assert!(!guard_ops.contains(&OperationKind::InsertAfter));
        assert_eq!(
            ops(&fx, fx.data),
            vec![OperationKind::DeleteSubgraph],
            "a DataObject anchor proposes exactly its own deletion — no flow edits"
        );
    }

    /// Position-sensitivity the SLM must learn (DIR-002 A2.2's mechanism):
    /// the same op is legal at one position and absent at another.
    #[test]
    fn position_changes_the_board() {
        let fx = fixture();
        // AppendNode: only where no outgoing edge exists.
        assert!(!ops(&fx, fx.t1).contains(&OperationKind::AppendNode));
        let no_out = ops(&fx, fx.end);
        assert!(
            !no_out.contains(&OperationKind::AppendNode),
            "End never extends the chain"
        );
        assert!(!no_out.contains(&OperationKind::InsertAfter));
        assert!(no_out.contains(&OperationKind::InsertBefore));
        // Guarded host refuses delete/replace; unguarded wait allows delete.
        assert!(!ops(&fx, fx.t1).contains(&OperationKind::DeleteSubgraph));
        assert!(ops(&fx, fx.wait).contains(&OperationKind::DeleteSubgraph));
        // Start: no InsertBefore, no DeleteNode.
        let s = ops(&fx, fx.start);
        assert!(!s.contains(&OperationKind::InsertBefore));
        assert!(!s.contains(&OperationKind::DeleteSubgraph));
    }

    /// Consistency cement against the op layer (staging side): where the
    /// oracle excludes for a STAGING-enforced reason, the canonical
    /// instantiation must refuse; where it includes, it must stage.
    #[test]
    fn oracle_agrees_with_apply_on_staging_rules() {
        let fx = fixture();
        let p = Provenance::default;
        // Included: AttachRearmingGuard at t1 stages.
        apply(
            &fx.dag,
            Operation::AttachRearmingGuard {
                host: fx.t1,
                key: key(),
                guard_id: "g_new".into(),
                trigger: GuardTrigger::Timer(TimerSpec::Cycle {
                    interval_ms: 5,
                    max_fires: 1,
                }),
            },
            p(),
        )
        .expect("oracle-included candidate must stage");
        // Excluded: AppendNode at t1 (has outgoing) must refuse.
        apply(
            &fx.dag,
            Operation::AppendNode {
                anchor: fx.t1,
                key: key(),
                node: task("tx"),
                edge_id: "ex".into(),
            },
            p(),
        )
        .expect_err("oracle-excluded AppendNode must refuse at staging");
        // Excluded: DeleteNode at guarded t1 must refuse.
        apply(&fx.dag, Operation::DeleteNode { target: fx.t1 }, p())
            .expect_err("oracle-excluded DeleteNode must refuse at staging");
        // Included: DeleteNode at wait stages.
        apply(&fx.dag, Operation::DeleteNode { target: fx.wait }, p())
            .expect("oracle-included DeleteNode must stage");
    }

    /// Whole-graph position = deterministic union; unknown key = empty;
    /// empty graph = empty (NOTA-only board).
    #[test]
    fn union_unknown_and_empty_positions() {
        let fx = fixture();
        let oracle = PositionalLegality { dag: &fx.dag };
        let union = oracle.legal_operations(None);
        for k in [fx.start, fx.t1, fx.wait, fx.guard, fx.end] {
            for op in ops(&fx, k) {
                assert!(union.contains(&op), "union must cover {op:?}");
            }
        }
        assert!(union.windows(2).all(|w| w[0] < w[1]), "sorted, deduped");
        assert!(
            oracle.legal_operations(Some(&key())).is_empty(),
            "unknown anchor"
        );
        let empty = DesignerDag::new("empty");
        let empty_oracle = PositionalLegality { dag: &empty };
        assert!(empty_oracle.legal_operations(None).is_empty());
        assert!(empty_oracle.legal_productions(None).is_empty());
    }

    /// The exclusion list is absolute: no position ever proposes the
    /// unrepresentable or unimplemented candidates — unlike the interim
    /// whole-graph oracle, which boards the full catalogue.
    #[test]
    fn excluded_candidates_never_appear() {
        let fx = fixture();
        let oracle = PositionalLegality { dag: &fx.dag };
        let all_ops = oracle.legal_operations(None);
        for banned in [OperationKind::CloseParallelRegion] {
            assert!(
                !all_ops.contains(&banned),
                "{banned:?} must never be boarded"
            );
        }
        let all = oracle.legal_productions(None);
        for banned in [ProductionId::HumanReviewWithRework] {
            assert!(!all.contains(&banned), "{banned:?} must never be boarded");
        }
        assert!(oracle
            .legal_productions(Some(&fx.t1))
            .contains(&ProductionId::ReminderThenEscalate));
        assert!(!oracle
            .legal_productions(Some(&fx.data))
            .contains(&ProductionId::ReminderThenEscalate));
    }
}
