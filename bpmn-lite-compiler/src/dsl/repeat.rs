//! G3.3 — `RepeatNTimes`: the sanctioned constructor for "wrap this task so
//! it repeats N times," replacing direct `AstMutator` orchestration at the
//! call site.
//!
//! CLAUDE.md's "Template ≠ macro" line and the gameboard plan's `production`
//! vocabulary (a pure `fn(bindings) -> composition`, never touching mutable
//! structure itself) don't map cleanly onto this pipeline — `LoopAst`/
//! `AstMutator` are DSL/S-expression-authoring concepts with no
//! `designer-graph::Operation` analogue (ruled: G3 stays DSL-pipeline-only,
//! see the G3 receipt). What *does* transfer from that vocabulary is the
//! shape: one purpose-built, named entry point that owns the whole
//! multi-step edit, so a caller (`bpmn-lite-server-designer`'s
//! `apply_dsl_macro` REST handler) states the intent once instead of
//! re-deriving `remove_node`/`rewire_next`/`insert_after` orchestration
//! itself. `AstMutator` is not deleted — it is still the correct tool for
//! `XorSplit`/`ParallelSplit`/custom macros, which G3 does not touch — but
//! it no longer has a *direct* caller for the loop case.

use super::ast::{NodeAst, TaskAst, WorkflowSource};
use super::macros::create_bounded_retry_macro;
use super::refactor::AstMutator;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatNTimesError {
    pub message: String,
}

impl std::fmt::Display for RepeatNTimesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RepeatNTimesError {}

impl From<String> for RepeatNTimesError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

/// Wrap the existing task `target_node_id` in `workflow` so it repeats
/// `ceiling` times: the task is lifted out of its current position and
/// re-inserted as the body of a `(loop ...)` node with the same entry
/// point, preserving every predecessor's routing and the original exit
/// target. The loop is authored, not unrolled here — `unroll::unroll_loops`
/// (run inside `compile()`) later expands it to `ceiling` forward copies;
/// this function only builds the pre-unroll `LoopAst` shape.
pub fn repeat_n_times(
    workflow: &mut WorkflowSource,
    target_node_id: &str,
    ceiling: u32,
    loop_id: Option<&str>,
) -> Result<(), RepeatNTimesError> {
    let target_task: TaskAst = {
        let mut mutator = AstMutator::new(workflow);
        match mutator.remove_node(target_node_id) {
            Some(NodeAst::Task(t)) => t,
            Some(_) => {
                return Err(format!("node '{target_node_id}' exists but is not a task").into())
            }
            None => return Err(format!("node '{target_node_id}' not found").into()),
        }
    };
    let exit_next = target_task.next.clone();
    let loop_id = loop_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("{target_node_id}-retry-loop"));

    let loop_node = NodeAst::Loop(create_bounded_retry_macro(
        target_task,
        ceiling,
        &loop_id,
        &exit_next,
    ));

    // Every predecessor that used to flow into the target now flows into
    // the LOOP (not past it to `exit_next`) — a predecessor rewired to
    // `exit_next` would silently bypass the retry entirely. This is the
    // D2-review-surfaced fix: the previous version rewired every
    // predecessor to `exit_next` and then separately spliced the loop in
    // after only ONE (arbitrarily chosen) predecessor, leaving every
    // OTHER predecessor's callers never retried, with no error raised.
    let preds = find_all_predecessor_ids(workflow, target_node_id);
    for pred in &preds {
        let mut mutator = AstMutator::new(workflow);
        mutator.rewire_next(pred, &loop_id)?;
    }

    // Injection position is a text-layout choice only — flow is
    // graph-driven via `next`/`id`, not source order — so any one
    // (already-rewired) predecessor's scope is a valid anchor. Use
    // `inject_into_same_scope` directly (not `insert_after`): every
    // predecessor was just rewired to `loop_id` above, so re-rewiring
    // the anchor via `insert_after`'s own anchor-rewire step would be
    // redundant at best and wrong if `insert_after`'s next-stealing
    // semantics ever diverged from this loop's.
    let anchor = preds
        .first()
        .cloned()
        .or_else(|| workflow.nodes.first().map(|n| n.id().to_string()))
        .ok_or_else(|| RepeatNTimesError::from("empty workflow scope".to_string()))?;

    let mut mutator = AstMutator::new(workflow);
    mutator.inject_into_same_scope(&anchor, loop_node)?;
    Ok(())
}

fn find_all_predecessor_ids(workflow: &WorkflowSource, target_id: &str) -> Vec<String> {
    let mut preds = Vec::new();
    find_all_predecessor_ids_rec(&workflow.nodes, target_id, &mut preds);
    preds
}

fn find_all_predecessor_ids_rec(nodes: &[NodeAst], target_id: &str, acc: &mut Vec<String>) {
    for node in nodes {
        // Exhaustive over NodeAst — NO wildcard arm: the D2 blind review
        // proved the old `_ => {}` silently skipped TimerWait (and D1's
        // guard) predecessors, so `repeat_n_times` left their `:next`
        // dangling at the wrapped task while returning Ok. A new variant
        // must break this compile, not fall through (B0's structural
        // fail-closed rule; same convention as
        // diagnostics_executor::find_all_predecessors_rec, this
        // function's twin).
        match node {
            NodeAst::Start(s) => {
                if s.next == target_id {
                    acc.push(s.id.clone());
                }
            }
            NodeAst::Task(t) => {
                if t.next == target_id {
                    acc.push(t.id.clone());
                }
            }
            NodeAst::MessageWait(w) => {
                if w.next == target_id {
                    acc.push(w.id.clone());
                }
            }
            NodeAst::TimerWait(w) => {
                if w.next == target_id {
                    acc.push(w.id.clone());
                }
            }
            NodeAst::Join(j) => {
                if j.next == target_id {
                    acc.push(j.id.clone());
                }
            }
            NodeAst::Loop(l) => {
                if l.next == target_id {
                    acc.push(l.id.clone());
                }
            }
            NodeAst::Split(sp) => {
                for flow in &sp.flows {
                    if flow.next == target_id {
                        acc.push(sp.id.clone());
                    }
                }
            }
            // D1 guards: `next` is the escape-flow entry — a flow
            // reference like any other for predecessor discovery (same
            // ruling as the diagnostics-executor twin).
            NodeAst::BoundaryTimer(g) => {
                if g.next == target_id {
                    acc.push(g.id.clone());
                }
            }
            NodeAst::BoundaryError(g) => {
                if g.next == target_id {
                    acc.push(g.id.clone());
                }
            }
            NodeAst::End(_) => {}
        }
        if let NodeAst::Loop(l) = node {
            find_all_predecessor_ids_rec(&l.body, target_id, acc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parser::parse_workflow_str;

    #[test]
    fn repeat_n_times_wraps_the_task_and_preserves_predecessor_and_exit_routing() {
        let mut workflow = parse_workflow_str(
            r#"(workflow test
  (start-event :id start :next charge)
  (service-task :id charge :verb billing.charge :next end)
  (end-event :id end :status "done")
)"#,
        )
        .expect("parse");

        repeat_n_times(&mut workflow, "charge", 3, Some("charge-loop")).expect("repeat_n_times");

        let start = workflow
            .nodes
            .iter()
            .find_map(|n| match n {
                NodeAst::Start(s) => Some(s),
                _ => None,
            })
            .expect("start node");
        assert_eq!(start.next, "charge-loop", "predecessor rewired to the loop");

        let loop_node = workflow
            .nodes
            .iter()
            .find_map(|n| match n {
                NodeAst::Loop(l) if l.id == "charge-loop" => Some(l),
                _ => None,
            })
            .expect("loop node present");
        assert_eq!(loop_node.ceiling, 3);
        assert_eq!(loop_node.next, "end", "loop exits to the task's original next");
        match loop_node.body.first() {
            Some(NodeAst::Task(t)) => {
                assert_eq!(t.id, "charge");
                assert_eq!(t.plug, "billing.charge");
            }
            other => panic!("expected the original task as the loop body, got {other:?}"),
        }

        assert!(
            !workflow
                .nodes
                .iter()
                .any(|n| matches!(n, NodeAst::Task(t) if t.id == "charge")),
            "the bare task must no longer exist outside the loop"
        );
    }

    /// D2 blind-review red→green: with the old `_ => {}` wildcard in
    /// `find_all_predecessor_ids_rec`, a timer-wait predecessor was
    /// silently skipped — `repeat_n_times` returned Ok while leaving
    /// `w1 :next charge` dangling at the removed task and splicing the
    /// loop at the wrong anchor (the resulting source failed compile
    /// with "':next charge' references unknown node"). Exhaustive match
    /// fixes it; this cements the rewire.
    #[test]
    fn repeat_n_times_rewires_a_timer_wait_predecessor() {
        let mut workflow = parse_workflow_str(
            r#"(workflow test
  (start-event :id start :next w1)
  (timer-wait :id w1 :duration-ms 1000 :next charge)
  (service-task :id charge :verb billing.charge :next end)
  (end-event :id end :status "done")
)"#,
        )
        .expect("parse");

        repeat_n_times(&mut workflow, "charge", 3, Some("charge-loop")).expect("repeat_n_times");

        let w1 = workflow
            .nodes
            .iter()
            .find_map(|n| match n {
                NodeAst::TimerWait(w) if w.id == "w1" => Some(w),
                _ => None,
            })
            .expect("timer-wait present");
        assert_eq!(w1.next, "charge-loop", "timer-wait predecessor rewired to the loop");

        // The whole result must still be a compilable workflow.
        let mut reg = crate::dsl::linter::StubPlaceholderRegistry::new();
        reg.register_verb("billing.charge", crate::dsl::linter::BindingDecl::default());
        let printed = crate::dsl::refactor::ToSexpr::to_sexpr(&workflow, 0);
        crate::dsl::compile(&printed, &reg).expect("rewired workflow must recompile");
    }

    /// Same axis for a D1 boundary guard whose escape enters the wrapped
    /// task: the guard predecessor is rewired ONTO the loop, same as
    /// every other predecessor kind (strengthened from the D2-review
    /// placeholder assertion `!= "charge"` now that the multi-predecessor
    /// splice defect below is fixed).
    #[test]
    fn repeat_n_times_rewires_a_guard_escape_predecessor_onto_the_loop() {
        let mut workflow = parse_workflow_str(
            r#"(workflow test
  (start-event :id start :next protected)
  (service-task :id protected :verb cbu.host :next charge)
  (boundary-error :id g1 :host protected :next charge)
  (service-task :id charge :verb billing.charge :next end)
  (end-event :id end :status "done")
)"#,
        )
        .expect("parse");

        repeat_n_times(&mut workflow, "charge", 3, Some("charge-loop")).expect("repeat_n_times");

        let g1 = workflow
            .nodes
            .iter()
            .find_map(|n| match n {
                NodeAst::BoundaryError(g) if g.id == "g1" => Some(g),
                _ => None,
            })
            .expect("guard present");
        assert_eq!(g1.next, "charge-loop", "guard escape rewired to the loop");

        let mut reg = crate::dsl::linter::StubPlaceholderRegistry::new();
        reg.register_verb("billing.charge", crate::dsl::linter::BindingDecl::default());
        reg.register_verb("cbu.host", crate::dsl::linter::BindingDecl::default());
        let printed = crate::dsl::refactor::ToSexpr::to_sexpr(&workflow, 0);
        crate::dsl::compile(&printed, &reg).expect("rewired workflow must recompile");
    }

    /// The multi-predecessor splice defect itself (D2-review fork,
    /// ruled): a diamond where two independent branches both flow
    /// directly into the wrapped task. Before the fix, only the
    /// arbitrarily-chosen anchor predecessor was rewired to the loop —
    /// the other was left pointed at `exit_next` (`end`), silently
    /// bypassing the retry for callers reaching `charge` via that
    /// branch. Both must now point at the loop.
    #[test]
    fn repeat_n_times_rewires_every_predecessor_in_a_diamond() {
        let mut workflow = parse_workflow_str(
            r#"(workflow test
  (start-event :id start :next split1)
  (split-and :id split1 :join charge
    (flow :next branch-a)
    (flow :next branch-b))
  (service-task :id branch-a :verb cbu.a :next charge)
  (service-task :id branch-b :verb cbu.b :next charge)
  (service-task :id charge :verb billing.charge :next end)
  (end-event :id end :status "done")
)"#,
        )
        .expect("parse");

        repeat_n_times(&mut workflow, "charge", 3, Some("charge-loop")).expect("repeat_n_times");

        let next_of = |id: &str| -> String {
            workflow
                .nodes
                .iter()
                .find_map(|n| match n {
                    NodeAst::Task(t) if t.id == id => Some(t.next.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("task '{id}' not found"))
        };
        assert_eq!(
            next_of("branch-a"),
            "charge-loop",
            "branch-a must be rewired to the loop, not left at exit_next"
        );
        assert_eq!(
            next_of("branch-b"),
            "charge-loop",
            "branch-b must be rewired to the loop, not left at exit_next"
        );

        let mut reg = crate::dsl::linter::StubPlaceholderRegistry::new();
        reg.register_verb("cbu.a", crate::dsl::linter::BindingDecl::default());
        reg.register_verb("cbu.b", crate::dsl::linter::BindingDecl::default());
        reg.register_verb("billing.charge", crate::dsl::linter::BindingDecl::default());
        let printed = crate::dsl::refactor::ToSexpr::to_sexpr(&workflow, 0);
        crate::dsl::compile(&printed, &reg).expect("rewired diamond workflow must recompile");
    }

    /// D2.1 blind-review finding: a Split node can itself be a direct
    /// predecessor of the wrapped task (a flow's `:next` targeting it,
    /// legal per the grammar — `parse_split_flow` places no restriction
    /// on `:next`). `rewire_next` refuses ANY `NodeAst::Split` unconditionally
    /// (`"Cannot directly rewire Split next; edit flow paths instead"`),
    /// so this predecessor kind was NEVER rewireable by `repeat_n_times`
    /// — before or after this tranche's fix. This cements that the
    /// failure is closed (a named `Err`, not a panic and not a silent
    /// partial rewire) rather than leaving the gap untested.
    ///
    /// NOT fixed here (surfaced, not decided — out of this tranche's
    /// scope): `repeat_n_times` is non-transactional. By the time this
    /// `Err` is produced, the target task has already been removed from
    /// `workflow`, and any predecessor rewired earlier in the same
    /// iteration already points at a `loop_id` that was never created
    /// (the loop node is injected AFTER the rewire loop) — the caller's
    /// `WorkflowSource` is left corrupted despite the `Err` return. This
    /// is pre-existing (the pre-D2.1 code had the identical shape: remove
    /// target, then loop-and-rewire with `?`), not introduced by this
    /// fix. Harmless today only because the sole caller
    /// (`bpmn-lite-server-designer`'s `apply_dsl_macro` handler) discards
    /// the mutated workflow on error rather than persisting it. A future
    /// fix would clone-and-restore-on-error or validate all predecessor
    /// kinds up front before mutating.
    #[test]
    fn repeat_n_times_refuses_a_split_predecessor_without_corrupting_silently() {
        let mut workflow = parse_workflow_str(
            r#"(workflow test
  (start-event :id start :next split1)
  (split-xor :id split1 :plug pick :join charge
    (flow :condition (= @flag "a") :next charge)
    (flow :condition (= @flag "b") :next other))
  (service-task :id charge :verb billing.charge :next end)
  (service-task :id other :verb billing.other :next end)
  (end-event :id end :status "done")
)"#,
        )
        .expect("parse");
        let err = repeat_n_times(&mut workflow, "charge", 3, Some("charge-loop")).unwrap_err();
        assert!(
            err.message.contains("Cannot directly rewire Split next"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn repeat_n_times_refuses_a_missing_target() {
        let mut workflow = parse_workflow_str(
            r#"(workflow test
  (start-event :id start :next end)
  (end-event :id end :status "done")
)"#,
        )
        .expect("parse");
        let err = repeat_n_times(&mut workflow, "does-not-exist", 3, None).unwrap_err();
        assert!(err.message.contains("not found"), "got: {}", err.message);
    }
}
