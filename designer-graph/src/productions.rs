//! WS-A.2 slice 4 — the first production catalogue entries
//! (EOP-PLAN-BPMN-DESIGN-003 v0.2 §12.2, §WS-A.2). The rest of the
//! `ProductionId` catalogue (`board_candidate.rs`) lands in later slices;
//! this module supplies exactly the four named in the slice-4 dispatch
//! brief.
//!
//! **P9 (binding, verbatim from the dispatch brief): productions are
//! deterministic COMPOSITIONS of existing `Operation`s.** A production
//! function builds a `Vec<Operation>` from caller-supplied bindings and
//! nothing else — it never touches `DesignerDag` directly, never mints
//! identity (every `NodeKey`/BPMN id/flow id arrives on the bindings
//! struct, same F4 discipline as `ops.rs`), and never runs admission
//! itself. `apply_production` is the single place a `Vec<Operation>`
//! becomes a `StagedCandidate`: it folds `ops::apply` over the sequence
//! against ONE staged candidate, so step 2 sees step 1's result. The
//! first refusal aborts the whole production — `base` is never touched,
//! because `ops::apply` itself always operates on a clone (I18).
//!
//! Every production's `Vec<Operation>` is also an edit-log entry (Q5): it
//! must round-trip serde bit-for-bit, since `Operation` already derives
//! `Serialize`/`Deserialize` (`ops.rs`) — no new serde plumbing is added
//! here, only exercised by a receipt test below.

use crate::ops::{apply, GuardTrigger, Operation, RegionBranch, StagedCandidate};
use crate::schema::{DesignerDag, NodeKey, Provenance};
use anyhow::Result;
use bpmn_lite_compiler::ir::IRNode;

/// `prod.request_and_wait` (§12.2): a `ServiceTask` (send) immediately
/// followed by a correlated `MessageWait` (receive), both inserted after
/// `anchor`. Composed from `InsertAfter` x2 — the send node becomes the
/// wait node's anchor, so the two land in send-then-wait order on the
/// normal path.
pub struct RequestAndWaitBindings {
    pub anchor: NodeKey,
    pub send_key: NodeKey,
    /// Must be `IRNode::ServiceTask` (or another send-shaped kind); not
    /// type-enforced here — `InsertAfter`/admission reject a bad shape.
    pub send_node: IRNode,
    pub send_edge_id: String,
    pub wait_key: NodeKey,
    /// Must be `IRNode::MessageWait`.
    pub wait_node: IRNode,
    pub wait_edge_id: String,
}

pub fn request_and_wait(b: RequestAndWaitBindings) -> Vec<Operation> {
    vec![
        Operation::InsertAfter {
            anchor: b.anchor,
            key: b.send_key,
            node: b.send_node,
            edge_id: b.send_edge_id,
        },
        Operation::InsertAfter {
            anchor: b.send_key,
            key: b.wait_key,
            node: b.wait_node,
            edge_id: b.wait_edge_id,
        },
    ]
}

/// `prod.parallel_checks_and_join` (§12.2): a `CreateParallelRegion` with
/// N `ServiceTask` branches after `anchor`. A single-op production —
/// `CreateParallelRegion` already constructs the region CLOSED (slice 3),
/// so there is nothing else to compose.
pub struct ParallelChecksAndJoinBindings {
    pub anchor: NodeKey,
    pub fork_key: NodeKey,
    pub fork_node_id: String,
    pub join_key: NodeKey,
    pub join_node_id: String,
    pub entry_edge_id: String,
    /// Each branch's `condition` must be `None` (parallel, not
    /// inclusive) — `CreateParallelRegion` itself refuses otherwise.
    pub branches: Vec<RegionBranch>,
}

pub fn parallel_checks_and_join(b: ParallelChecksAndJoinBindings) -> Vec<Operation> {
    vec![Operation::CreateParallelRegion {
        anchor: b.anchor,
        fork_key: b.fork_key,
        fork_node_id: b.fork_node_id,
        join_key: b.join_key,
        join_node_id: b.join_node_id,
        entry_edge_id: b.entry_edge_id,
        branches: b.branches,
    }]
}

/// `prod.for_each_with_ceiling` (§12.2): a `CreateMultiInstanceRegion`
/// after `anchor`. The collection flag name, declared max, and inner
/// task type all live on the `IRNode::MultiInstance` node the caller
/// supplies (I24 — the mandatory declaration arrives in the bindings,
/// never defaulted here). A single-op production: the MI node IS the
/// region (ruling K), nothing else to compose.
pub struct ForEachWithCeilingBindings {
    pub anchor: NodeKey,
    pub key: NodeKey,
    /// Must be `IRNode::MultiInstance`; `CreateMultiInstanceRegion`
    /// itself refuses any other kind.
    pub node: IRNode,
    pub edge_id: String,
}

pub fn for_each_with_ceiling(b: ForEachWithCeilingBindings) -> Vec<Operation> {
    vec![Operation::CreateMultiInstanceRegion {
        anchor: b.anchor,
        key: b.key,
        node: b.node,
        edge_id: b.edge_id,
    }]
}

/// `prod.reminder_then_escalate` (§12.2): a non-interrupting, re-arming
/// boundary guard on `anchor` with a bounded `TimerSpec::Cycle` trigger,
/// composed with an escalation `ServiceTask` appended AFTER `anchor` on
/// the NORMAL path (`InsertAfter`, forward rework — not a backward
/// edge). The guard itself carries no sequence edges (slice-2 pattern:
/// `AttachRearmingGuard` only wires the boundary attachment); the
/// escalation node is a plain continuation edit on the host, independent
/// of the guard's attachment.
pub struct ReminderThenEscalateBindings {
    pub anchor: NodeKey,
    pub guard_key: NodeKey,
    pub guard_id: String,
    /// Must be `bpmn_lite_compiler::ir::TimerSpec::Cycle { .. }` — cycle
    /// triggers are only legal on a non-interrupting guard (boundary
    /// rule 6), which `AttachRearmingGuard` always produces.
    pub cycle: bpmn_lite_compiler::ir::TimerSpec,
    pub escalation_key: NodeKey,
    pub escalation_node: IRNode,
    pub escalation_edge_id: String,
    /// The escape flow's own terminal (slice-2 topology rule: a guard's
    /// escape path ends in its own End or merges through a gateway,
    /// never a bare shared End).
    pub escalation_end_key: NodeKey,
    pub escalation_end_node: IRNode,
    pub escalation_end_edge_id: String,
}

pub fn reminder_then_escalate(b: ReminderThenEscalateBindings) -> Vec<Operation> {
    // §12.2: "GUARD-N> + bounded cycle + escalation continuation" — the
    // escalation IS the guard's escape flow (brief-defect remediation,
    // slice-4 review): the production alone must admit; nothing is left
    // for the caller to complete.
    vec![
        Operation::AttachRearmingGuard {
            host: b.anchor,
            key: b.guard_key,
            guard_id: b.guard_id,
            trigger: GuardTrigger::Timer(b.cycle),
        },
        Operation::AppendNode {
            anchor: b.guard_key,
            key: b.escalation_key,
            node: b.escalation_node,
            edge_id: b.escalation_edge_id,
        },
        Operation::AppendNode {
            anchor: b.escalation_key,
            key: b.escalation_end_key,
            node: b.escalation_end_node,
            edge_id: b.escalation_end_edge_id,
        },
    ]
}

/// `prod.interrupting_timeout` (§12.2): an INTERRUPTING boundary guard on
/// `anchor` with a `TimerSpec::Duration` trigger, composed with a
/// timeout-continuation `AppendNode` off the guard and that continuation's
/// own `End` (same shape as `reminder_then_escalate`'s remediation — the
/// continuation IS the guard's escape flow, owned by the production; the
/// production ALONE must admit, nothing left for the caller to complete).
///
/// `duration_ms` is typed as a bare `u64`, not a `TimerSpec` — the
/// `TimerSpec::Duration` is built INSIDE this function. This is
/// deliberate: a `Cycle` trigger is refused by `ops::apply`'s boundary
/// rule 6 on an INTERRUPTING guard (`AttachGuard` only ever builds
/// `interrupting: true`), so if `duration_ms` were instead a `TimerSpec`
/// field, a caller could construct a `TimerSpec::Cycle { .. }` value that
/// is accepted by the bindings struct but refused at `apply` time — a
/// RED path with no corresponding GREEN. Typing the field `u64` makes
/// that state UNREPRESENTABLE through these bindings: there is no test
/// to write here, because there is no cycle-shaped value to construct.
/// (The companion RED receipt, `production_ops_reject_via_ops_gates`,
/// instead hand-builds the illegal `Vec<Operation>` directly — bypassing
/// this production's bindings entirely — to prove the gate `ops::apply`
/// itself enforces is still live.)
pub struct InterruptingTimeoutBindings {
    pub anchor: NodeKey,
    pub guard_key: NodeKey,
    pub guard_id: String,
    pub duration_ms: u64,
    pub continuation_key: NodeKey,
    pub continuation_node: IRNode,
    pub continuation_edge_id: String,
    /// The continuation's own terminal (slice-2 topology rule: a guard's
    /// escape path ends in its own End or merges through a gateway, never
    /// a bare shared End).
    pub continuation_end_key: NodeKey,
    pub continuation_end_node: IRNode,
    pub continuation_end_edge_id: String,
}

pub fn interrupting_timeout(b: InterruptingTimeoutBindings) -> Vec<Operation> {
    vec![
        Operation::AttachGuard {
            host: b.anchor,
            key: b.guard_key,
            guard_id: b.guard_id,
            trigger: GuardTrigger::Timer(bpmn_lite_compiler::ir::TimerSpec::Duration {
                ms: b.duration_ms,
            }),
        },
        Operation::AppendNode {
            anchor: b.guard_key,
            key: b.continuation_key,
            node: b.continuation_node,
            edge_id: b.continuation_edge_id,
        },
        Operation::AppendNode {
            anchor: b.continuation_key,
            key: b.continuation_end_key,
            node: b.continuation_end_node,
            edge_id: b.continuation_end_edge_id,
        },
    ]
}

/// `prod.non_interrupting_notification` (§12.2): a NON-INTERRUPTING
/// (re-arming) boundary guard on `anchor` with a `TimerSpec::Cycle`
/// trigger, composed with a notification `AppendNode` off the guard and
/// that notification's own `End` (same shape as `reminder_then_escalate`
/// — the notification IS the guard's escape flow, owned by the
/// production; the production ALONE must admit).
///
/// `interval_ms`/`max_fires` are typed as bare `u64`/`u32`, not a
/// `TimerSpec` — the `TimerSpec::Cycle` is built INSIDE this function,
/// same discipline as `interrupting_timeout`'s `duration_ms` (F4: no new
/// identity or ambiguity is introduced by exposing the raw `TimerSpec`
/// enum on the bindings surface).
pub struct NonInterruptingNotificationBindings {
    pub anchor: NodeKey,
    pub guard_key: NodeKey,
    pub guard_id: String,
    pub interval_ms: u64,
    pub max_fires: u32,
    pub notification_key: NodeKey,
    pub notification_node: IRNode,
    pub notification_edge_id: String,
    /// The notification's own terminal (slice-2 topology rule, as above).
    pub notification_end_key: NodeKey,
    pub notification_end_node: IRNode,
    pub notification_end_edge_id: String,
}

pub fn non_interrupting_notification(b: NonInterruptingNotificationBindings) -> Vec<Operation> {
    vec![
        Operation::AttachRearmingGuard {
            host: b.anchor,
            key: b.guard_key,
            guard_id: b.guard_id,
            trigger: GuardTrigger::Timer(bpmn_lite_compiler::ir::TimerSpec::Cycle {
                interval_ms: b.interval_ms,
                max_fires: b.max_fires,
            }),
        },
        Operation::AppendNode {
            anchor: b.guard_key,
            key: b.notification_key,
            node: b.notification_node,
            edge_id: b.notification_edge_id,
        },
        Operation::AppendNode {
            anchor: b.notification_key,
            key: b.notification_end_key,
            node: b.notification_end_node,
            edge_id: b.notification_end_edge_id,
        },
    ]
}

/// Fold `ops` over ONE staged candidate cloned from `base`: step 2 sees
/// step 1's result, step 3 sees step 2's, etc. The first refusal aborts
/// immediately — `ops::apply` always operates on a clone (I18), so `base`
/// is never touched regardless of where the sequence fails. Does NOT run
/// admission itself (same contract as `ops::apply`) — the caller stages
/// then calls `candidate.admit()`.
///
/// `StagedCandidate::applied` on the returned value is the LAST operation
/// in the sequence (the step that produced the final candidate); the full
/// sequence is `ops` itself, which the caller retains for the edit log.
pub fn apply_production(
    base: &DesignerDag,
    ops: Vec<Operation>,
    provenance: Provenance,
) -> Result<StagedCandidate> {
    if ops.is_empty() {
        return Err(anyhow::anyhow!(
            "apply_production refused: empty operation sequence"
        ));
    }
    let mut candidate: DesignerDag = base.clone();
    let mut last: Option<Operation> = None;
    for op in ops {
        let staged = apply(&candidate, op, provenance.clone())?;
        candidate = staged.candidate;
        last = Some(staged.applied);
    }
    Ok(StagedCandidate {
        candidate,
        applied: last.expect("non-empty ops checked above"),
        provenance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_compiler::ir::{IRNode, TimerSpec};
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

    fn end_node(id: &str) -> IRNode {
        IRNode::End {
            id: id.into(),
            terminate: false,
        }
    }

    /// start -> t1 -> end
    fn linear(name: &str) -> (DesignerDag, NodeKey, NodeKey, NodeKey) {
        use crate::schema::DesignerEdge;
        let mut dag = DesignerDag::new(name);
        let s = dag
            .insert_node(key(), IRNode::Start { id: "start".into() }, None, Provenance::default())
            .unwrap();
        let t = dag.insert_node(key(), task("t1"), None, Provenance::default()).unwrap();
        let e = dag
            .insert_node(key(), end_node("end"), None, Provenance::default())
            .unwrap();
        dag.insert_edge(
            s,
            t,
            DesignerEdge { id: "e1".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        dag.insert_edge(
            t,
            e,
            DesignerEdge { id: "e2".into(), condition: None, provenance: Provenance::default() },
        )
        .unwrap();
        (dag, s, t, e)
    }

    fn message_wait(id: &str, corr: &str) -> IRNode {
        IRNode::MessageWait {
            id: id.into(),
            name: id.into(),
            corr_key_source: corr.into(),
        }
    }

    fn mi_node(id: &str) -> IRNode {
        IRNode::MultiInstance {
            id: id.into(),
            name: id.into(),
            task_type: "noop".into(),
            collection_flag_name: "items".into(),
            declared_max: 5,
        }
    }

    /// Receipt: `request_and_wait` -> apply_production -> full-chain
    /// admit() green.
    #[test]
    fn request_and_wait_admits() {
        use bpmn_lite_types::ffi_bindings::{DataObjectRole, DataObjectType, PrimitiveType};

        let (mut base, _s, t1, _e) = linear("prod-raw");
        // The MessageWait's correlation source names a real data object —
        // the verifier resolves `corr_key_source` against declared data
        // objects, so the fixture must declare one (this is a fixture
        // requirement of the verifier, not a production concern).
        base.insert_node(
            key(),
            IRNode::DataObject {
                id: "corr_flag".into(),
                name: "corr_flag".into(),
                type_decl: DataObjectType::Primitive(PrimitiveType::String),
                role: DataObjectRole::Internal,
            },
            None,
            Provenance::default(),
        )
        .unwrap();
        let ops = request_and_wait(RequestAndWaitBindings {
            anchor: t1,
            send_key: key(),
            send_node: task("send1"),
            send_edge_id: "e_send".into(),
            wait_key: key(),
            wait_node: message_wait("wait1", "corr_flag"),
            wait_edge_id: "e_wait".into(),
        });
        let staged = apply_production(&base, ops, Provenance::default())
            .expect("request_and_wait production must apply");
        staged.candidate.admit().expect("request_and_wait must admit");
    }

    /// Receipt: `parallel_checks_and_join` -> apply_production ->
    /// full-chain admit() green.
    #[test]
    fn parallel_checks_and_join_admits() {
        let (base, _s, t1, _e) = linear("prod-pcj");
        let ops = parallel_checks_and_join(ParallelChecksAndJoinBindings {
            anchor: t1,
            fork_key: key(),
            fork_node_id: "pcj_fork".into(),
            join_key: key(),
            join_node_id: "pcj_join".into(),
            entry_edge_id: "pcj_entry".into(),
            branches: vec![
                RegionBranch {
                    key: key(),
                    node: task("check_a"),
                    in_edge_id: "in_a".into(),
                    out_edge_id: "out_a".into(),
                    condition: None,
                },
                RegionBranch {
                    key: key(),
                    node: task("check_b"),
                    in_edge_id: "in_b".into(),
                    out_edge_id: "out_b".into(),
                    condition: None,
                },
            ],
        });
        let staged = apply_production(&base, ops, Provenance::default())
            .expect("parallel_checks_and_join production must apply");
        staged.candidate.admit().expect("parallel_checks_and_join must admit");
    }

    /// Receipt: `for_each_with_ceiling` -> apply_production -> full-chain
    /// admit() green.
    #[test]
    fn for_each_with_ceiling_admits() {
        let (base, _s, t1, _e) = linear("prod-fewc");
        let ops = for_each_with_ceiling(ForEachWithCeilingBindings {
            anchor: t1,
            key: key(),
            node: mi_node("mi_fewc"),
            edge_id: "e_mi".into(),
        });
        let staged = apply_production(&base, ops, Provenance::default())
            .expect("for_each_with_ceiling production must apply");
        staged.candidate.admit().expect("for_each_with_ceiling must admit");
    }

    /// Receipt: `reminder_then_escalate` -> apply_production -> full-chain
    /// admit() green; the guard's projected IR is non-interrupting with a
    /// Cycle trigger (both asserted, not assumed).
    #[test]
    fn reminder_then_escalate_admits_with_non_interrupting_cycle_guard() {
        let (base, _s, t1, _e) = linear("prod-rte");
        let guard_key = key();
        let ops = reminder_then_escalate(ReminderThenEscalateBindings {
            anchor: t1,
            guard_key,
            guard_id: "reminder_guard".into(),
            cycle: TimerSpec::Cycle {
                interval_ms: 5_000,
                max_fires: 3,
            },
            escalation_key: key(),
            escalation_node: task("escalate"),
            escalation_edge_id: "e_escalate".into(),
            escalation_end_key: key(),
            escalation_end_node: end_node("reminder_escalation_end"),
            escalation_end_edge_id: "e_escalate_end".into(),
        });
        // The production ALONE must admit — the escalation continuation
        // is the guard's escape flow, owned by the production (§12.2;
        // slice-4 review remediation of the brief's mis-spec).
        let staged = apply_production(&base, ops, Provenance::default())
            .expect("reminder_then_escalate production must apply");

        let ir = staged.candidate.to_ir().unwrap();
        let (interrupting, spec) = ir
            .node_indices()
            .find_map(|i| match &ir[i] {
                IRNode::BoundaryTimer { id, interrupting, spec, .. } if id == "reminder_guard" => {
                    Some((*interrupting, spec.clone()))
                }
                _ => None,
            })
            .expect("guard projected");
        assert!(!interrupting, "reminder guard must be non-interrupting (re-arming)");
        assert!(matches!(spec, TimerSpec::Cycle { .. }), "guard trigger must be Cycle: {spec:?}");

        // The escalation node sits on the normal (forward) path off the
        // anchor, not a backward edge — confirmed by admission succeeding
        // (a backward edge would be refused at Operation::apply time, not
        // here, but the full chain admitting is the end-to-end witness).
        staged.candidate.admit().expect("reminder_then_escalate must admit");
    }

    /// Receipt (RED): `production_aborts_atomically` — a production whose
    /// SECOND op collides on an id is refused; `base` is unchanged. Built
    /// directly from raw `Operation`s (not one of the four named
    /// productions) since it only needs to exercise `apply_production`'s
    /// fold-and-abort behaviour, not any particular production's shape.
    #[test]
    fn production_aborts_atomically() {
        let (base, _s, t1, _e) = linear("prod-abort");
        let base_count_before = base.node_count();

        let colliding_key = key();
        let ops = vec![
            Operation::InsertAfter {
                anchor: t1,
                key: colliding_key,
                node: task("step_one"),
                edge_id: "e_step_one".into(),
            },
            // Second op reuses `colliding_key` for a DIFFERENT node — a
            // duplicate NodeKey, refused by `insert_node`'s F3 check.
            Operation::AppendNode {
                anchor: colliding_key,
                key: colliding_key,
                node: task("step_two"),
                edge_id: "e_step_two".into(),
            },
        ];

        let err = apply_production(&base, ops, Provenance::default())
            .expect_err("a production whose 2nd op collides on an id must be refused");
        assert!(
            err.to_string().contains(&format!("{colliding_key:?}")),
            "refusal must name the colliding key: {err}"
        );

        // base untouched: same node count, and the colliding key still
        // resolves to nothing beyond the ORIGINAL linear() shape (no
        // partial first-op residue leaked onto base).
        assert_eq!(base.node_count(), base_count_before);
        assert_eq!(base.node_count(), 3);
    }

    /// Receipt (GREEN): `interrupting_timeout` -> apply_production ->
    /// full-chain admit() green, ALONE (no receipt-added completion ops
    /// — slice-5 binding rule from slice-4's remediation). Projected IR
    /// asserted: interrupting=true, TimerSpec::Duration.
    #[test]
    fn interrupting_timeout_admits_alone_with_interrupting_duration_guard() {
        let (base, _s, t1, _e) = linear("prod-itimeout");
        let ops = interrupting_timeout(InterruptingTimeoutBindings {
            anchor: t1,
            guard_key: key(),
            guard_id: "itimeout_guard".into(),
            duration_ms: 30_000,
            continuation_key: key(),
            continuation_node: task("itimeout_continuation"),
            continuation_edge_id: "e_itimeout_continuation".into(),
            continuation_end_key: key(),
            continuation_end_node: end_node("itimeout_continuation_end"),
            continuation_end_edge_id: "e_itimeout_continuation_end".into(),
        });
        let staged = apply_production(&base, ops, Provenance::default())
            .expect("interrupting_timeout production must apply");

        let ir = staged.candidate.to_ir().unwrap();
        let (interrupting, spec) = ir
            .node_indices()
            .find_map(|i| match &ir[i] {
                IRNode::BoundaryTimer { id, interrupting, spec, .. } if id == "itimeout_guard" => {
                    Some((*interrupting, spec.clone()))
                }
                _ => None,
            })
            .expect("guard projected");
        assert!(interrupting, "interrupting_timeout must produce an INTERRUPTING guard");
        assert!(
            matches!(spec, TimerSpec::Duration { .. }),
            "guard trigger must be Duration: {spec:?}"
        );

        // The production ALONE must admit — no receipt-added completion
        // ops (slice-5 binding rule).
        staged.candidate.admit().expect("interrupting_timeout must admit alone");
    }

    /// Receipt (GREEN): `non_interrupting_notification` -> apply_production
    /// -> full-chain admit() green, ALONE. Projected IR asserted:
    /// interrupting=false, TimerSpec::Cycle.
    #[test]
    fn non_interrupting_notification_admits_alone_with_non_interrupting_cycle_guard() {
        let (base, _s, t1, _e) = linear("prod-notify");
        let ops = non_interrupting_notification(NonInterruptingNotificationBindings {
            anchor: t1,
            guard_key: key(),
            guard_id: "notify_guard".into(),
            interval_ms: 10_000,
            max_fires: 5,
            notification_key: key(),
            notification_node: task("notify_task"),
            notification_edge_id: "e_notify_task".into(),
            notification_end_key: key(),
            notification_end_node: end_node("notify_task_end"),
            notification_end_edge_id: "e_notify_task_end".into(),
        });
        let staged = apply_production(&base, ops, Provenance::default())
            .expect("non_interrupting_notification production must apply");

        let ir = staged.candidate.to_ir().unwrap();
        let (interrupting, spec) = ir
            .node_indices()
            .find_map(|i| match &ir[i] {
                IRNode::BoundaryTimer { id, interrupting, spec, .. } if id == "notify_guard" => {
                    Some((*interrupting, spec.clone()))
                }
                _ => None,
            })
            .expect("guard projected");
        assert!(
            !interrupting,
            "non_interrupting_notification must produce a NON-interrupting guard"
        );
        assert!(matches!(spec, TimerSpec::Cycle { .. }), "guard trigger must be Cycle: {spec:?}");

        // The production ALONE must admit — no receipt-added completion
        // ops (slice-5 binding rule).
        staged.candidate.admit().expect("non_interrupting_notification must admit alone");
    }

    /// Receipt (RED): `production_ops_reject_via_ops_gates` — a
    /// cycle-timeout through `interrupting_timeout` is UNREPRESENTABLE
    /// (the bindings only accept `duration_ms: u64`; see the doc comment
    /// on `InterruptingTimeoutBindings`, which IS the receipt for that
    /// claim). This test instead hand-builds the illegal op vector
    /// directly (an `AttachGuard` with a `Cycle` trigger — the shape a
    /// buggy/hostile caller of `apply_production` might submit bypassing
    /// the production function entirely) and shows `apply_production`
    /// refuses it via the same slice-2 `ops::apply` gate (boundary rule
    /// 6: Cycle is only legal on a non-interrupting guard).
    #[test]
    fn production_ops_reject_via_ops_gates() {
        let (base, _s, t1, _e) = linear("prod-reject-gate");
        let base_count_before = base.node_count();
        let illegal_ops = vec![Operation::AttachGuard {
            host: t1,
            key: key(),
            guard_id: "illegal_cycle_guard".into(),
            trigger: GuardTrigger::Timer(TimerSpec::Cycle {
                interval_ms: 1_000,
                max_fires: 2,
            }),
        }];

        let err = apply_production(&base, illegal_ops, Provenance::default())
            .expect_err("Cycle-on-interrupting must be refused by ops::apply's gate");
        assert!(
            err.to_string().contains("illegal_cycle_guard"),
            "refusal must name the guard id: {err}"
        );
        assert_eq!(base.node_count(), base_count_before);
    }

    /// Receipt: every production's `Vec<Operation>` round-trips serde
    /// (Q5 — these are the edit-log entries) and re-applies identically
    /// after the round trip.
    #[test]
    fn production_ops_serde_round_trips_and_reapplies_identically() {
        let (base, _s, t1, _e) = linear("prod-serde");
        let guard_key = key();
        let ops = reminder_then_escalate(ReminderThenEscalateBindings {
            anchor: t1,
            guard_key,
            guard_id: "serde_guard".into(),
            cycle: TimerSpec::Cycle {
                interval_ms: 2_000,
                max_fires: 4,
            },
            escalation_key: key(),
            escalation_node: task("serde_escalate"),
            escalation_edge_id: "e_serde_escalate".into(),
            escalation_end_key: key(),
            escalation_end_node: end_node("serde_escalate_end"),
            escalation_end_edge_id: "e_serde_escalate_end".into(),
        });

        let json = serde_json::to_string(&ops).expect("Vec<Operation> must serialize");
        let round_tripped: Vec<Operation> =
            serde_json::from_str(&json).expect("Vec<Operation> must deserialize");

        let staged_direct = apply_production(&base, ops, Provenance::default())
            .expect("direct application must succeed");
        let staged_round_tripped = apply_production(&base, round_tripped, Provenance::default())
            .expect("round-tripped application must succeed");

        // The production owns its complete escape flow (slice-4 review
        // remediation) — both candidates are admission-complete as
        // applied; nothing to add.
        let ir_direct = staged_direct.candidate.to_ir().unwrap();
        let ir_round_tripped = staged_round_tripped.candidate.to_ir().unwrap();

        let mut node_ids_direct: Vec<&str> =
            ir_direct.node_indices().map(|i| ir_direct[i].id()).collect();
        let mut node_ids_round_tripped: Vec<&str> =
            ir_round_tripped.node_indices().map(|i| ir_round_tripped[i].id()).collect();
        node_ids_direct.sort();
        node_ids_round_tripped.sort();
        assert_eq!(node_ids_direct, node_ids_round_tripped);

        let mut edge_ids_direct: Vec<&str> =
            ir_direct.edge_indices().map(|i| ir_direct[i].id.as_str()).collect();
        let mut edge_ids_round_tripped: Vec<&str> =
            ir_round_tripped.edge_indices().map(|i| ir_round_tripped[i].id.as_str()).collect();
        edge_ids_direct.sort();
        edge_ids_round_tripped.sort();
        assert_eq!(edge_ids_direct, edge_ids_round_tripped);

        staged_round_tripped
            .candidate
            .admit()
            .expect("round-tripped candidate must still admit");
    }

    /// Receipt: `interrupting_timeout`'s `Vec<Operation>` round-trips
    /// serde (Q5) and re-applies identically after the round trip, ALONE
    /// (slice-5 binding rule — no receipt-added completion ops).
    #[test]
    fn interrupting_timeout_ops_serde_round_trips_and_reapplies_identically() {
        let (base, _s, t1, _e) = linear("prod-itimeout-serde");
        let ops = interrupting_timeout(InterruptingTimeoutBindings {
            anchor: t1,
            guard_key: key(),
            guard_id: "itimeout_serde_guard".into(),
            duration_ms: 45_000,
            continuation_key: key(),
            continuation_node: task("itimeout_serde_continuation"),
            continuation_edge_id: "e_itimeout_serde_continuation".into(),
            continuation_end_key: key(),
            continuation_end_node: end_node("itimeout_serde_continuation_end"),
            continuation_end_edge_id: "e_itimeout_serde_continuation_end".into(),
        });

        let json = serde_json::to_string(&ops).expect("Vec<Operation> must serialize");
        let round_tripped: Vec<Operation> =
            serde_json::from_str(&json).expect("Vec<Operation> must deserialize");

        let staged_direct = apply_production(&base, ops, Provenance::default())
            .expect("direct application must succeed");
        let staged_round_tripped = apply_production(&base, round_tripped, Provenance::default())
            .expect("round-tripped application must succeed");

        let ir_direct = staged_direct.candidate.to_ir().unwrap();
        let ir_round_tripped = staged_round_tripped.candidate.to_ir().unwrap();

        let mut node_ids_direct: Vec<&str> =
            ir_direct.node_indices().map(|i| ir_direct[i].id()).collect();
        let mut node_ids_round_tripped: Vec<&str> =
            ir_round_tripped.node_indices().map(|i| ir_round_tripped[i].id()).collect();
        node_ids_direct.sort();
        node_ids_round_tripped.sort();
        assert_eq!(node_ids_direct, node_ids_round_tripped);

        let mut edge_ids_direct: Vec<&str> =
            ir_direct.edge_indices().map(|i| ir_direct[i].id.as_str()).collect();
        let mut edge_ids_round_tripped: Vec<&str> =
            ir_round_tripped.edge_indices().map(|i| ir_round_tripped[i].id.as_str()).collect();
        edge_ids_direct.sort();
        edge_ids_round_tripped.sort();
        assert_eq!(edge_ids_direct, edge_ids_round_tripped);

        // The production ALONE must admit — no receipt-added completion
        // ops (slice-5 binding rule).
        staged_round_tripped
            .candidate
            .admit()
            .expect("round-tripped interrupting_timeout candidate must still admit alone");
    }
}
