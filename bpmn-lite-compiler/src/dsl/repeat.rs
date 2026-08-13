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

    // Every predecessor that used to flow into the target now flows past
    // it, to whatever the target used to point at — the loop is spliced
    // back in at that same position below.
    for pred in find_all_predecessor_ids(workflow, target_node_id) {
        let mut mutator = AstMutator::new(workflow);
        mutator.rewire_next(&pred, &exit_next)?;
    }

    let loop_node = NodeAst::Loop(create_bounded_retry_macro(
        target_task,
        ceiling,
        &loop_id,
        &exit_next,
    ));

    let anchor = find_all_predecessor_ids(workflow, &exit_next)
        .into_iter()
        .next()
        .or_else(|| workflow.nodes.first().map(|n| n.id().to_string()))
        .ok_or_else(|| RepeatNTimesError::from("empty workflow scope".to_string()))?;

    let mut mutator = AstMutator::new(workflow);
    mutator.insert_after(&anchor, loop_node)?;
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
    /// task: discovery must find the guard predecessor so its `next` is
    /// rewired OFF the removed task (no dangling reference; the result
    /// recompiles). NOTE this cements discovery only: `repeat_n_times`
    /// has a PRE-EXISTING multi-predecessor defect (not guard-specific —
    /// two service-task predecessors show it too) where only the anchor
    /// predecessor is routed through the loop and every other one is
    /// left rewired to the exit, bypassing the retry. Surfaced at the D2
    /// gate for a separate ruling; do not strengthen this assertion to
    /// "== charge-loop" until that ruling lands.
    #[test]
    fn repeat_n_times_rewires_a_guard_escape_predecessor_off_the_removed_task() {
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
        assert_ne!(
            g1.next, "charge",
            "guard escape must not dangle at the removed task"
        );

        let mut reg = crate::dsl::linter::StubPlaceholderRegistry::new();
        reg.register_verb("billing.charge", crate::dsl::linter::BindingDecl::default());
        reg.register_verb("cbu.host", crate::dsl::linter::BindingDecl::default());
        let printed = crate::dsl::refactor::ToSexpr::to_sexpr(&workflow, 0);
        crate::dsl::compile(&printed, &reg).expect("rewired workflow must recompile");
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
