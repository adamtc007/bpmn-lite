//! G3.1/G3.2 — loop unrolling pass.
//!
//! Runs on the raw parsed AST, between parse and lint (see `compile()` in
//! `mod.rs`). Expands every `NodeAst::Loop{ceiling}` into `ceiling` forward
//! chained copies of its body, so no `NodeAst::Loop` (and, downstream, no
//! `ExecutionNode::Loop`) ever reaches the linter or the DAG/back-edge
//! machinery — the loop concept survives only as the *authoring*
//! abstraction (`LoopAst`, `create_bounded_retry_macro`/`repeat_n_times`),
//! never as a runtime back-edge.
//!
//! Per-copy node ids are derived deterministically from loop id + iteration
//! index (I33 — this is the identity class fought three times already).
//! Confirmed, not assumed: no AST node in this language reads an iteration
//! index — `TaskAst`/`SplitAst`/`JoinAst`/`MessageWaitAst` all carry only
//! literal, source-authored fields — so unrolling never needs to bind a
//! per-copy literal; it is pure structural repetition.

use super::ast::{
    BoundaryErrorAst, BoundaryTimerAst, JoinAst, LoopAst, MessageWaitAst, NodeAst, SplitAst,
    TimerWaitAst,
    SplitFlowAst, TaskAst,
};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrollError {
    pub message: String,
}

impl std::fmt::Display for UnrollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for UnrollError {}

/// G3.2: total-unrolled-node cap, artifact-resident and verifier-checked on
/// the same footing as the MI runtime maximum (see
/// `t_mi_v2_exceeds_declared_max_is_typed_error_not_silent_truncation`) — a
/// hard, typed compile-time reject, never silent truncation. Counted across
/// the *whole* unroll (I32 — nesting multiplies, loop-inside-loop
/// multiplies again), not per loop.
///
/// This is a genuinely new check, not a rediscovery of an existing one: the
/// DSL pipeline's only prior size signal for loops was `BrCounterLt`'s
/// `limit`-driven `loop_multiplier` scaling of `VerifiedLimits.max_steps`
/// (`bpmn-lite-types/src/artifact.rs`), which is a *runtime* budget, not a
/// compile-time reject — and G3.3 deletes that mechanism's only producer
/// entirely. Post-G3, this cap is the sole enforcement point for
/// loop-driven artifact size, so it must reject with a typed error, not
/// merely exist as an assumption.
// H4.1 (EOP-PLAN-CRATE-HYGIENE-001): pub(super) — zero external consumers;
// `compile()` (dsl::mod) still calls `unroll_loops` via a private `use`.
pub(super) const MAX_UNROLLED_NODES: usize = 2_048;

/// Expand every `NodeAst::Loop` in `nodes` into forward-chained copies.
/// Nested loops are unrolled innermost-correct by construction: a nested
/// loop's `id` is already iteration-qualified by the time its own
/// `unroll_loop` call runs, so its body's qualified ids nest hierarchically
/// and can never collide across outer iterations.
pub(super) fn unroll_loops(nodes: Vec<NodeAst>) -> Result<Vec<NodeAst>, UnrollError> {
    let mut budget = MAX_UNROLLED_NODES;
    unroll_nodes(nodes, &mut budget)
}

fn unroll_nodes(nodes: Vec<NodeAst>, budget: &mut usize) -> Result<Vec<NodeAst>, UnrollError> {
    // A predecessor sibling in THIS SAME list may point its `:next` (or a
    // split flow) directly at a loop's own id — e.g. `(start-event :next
    // my-loop)`. Once the loop expands, that id no longer names any node,
    // so every such external reference must be retargeted to the loop's
    // first iteration's entry before the loop itself is expanded away.
    // (A loop's *internal* back-edge to its own id is a different case,
    // handled separately by `remap_next`/`exit_target` inside
    // `clone_body_iteration` — this map only ever holds entries for loops
    // directly in `nodes`, never the loop's own body members.)
    let mut loop_entries: HashMap<String, String> = HashMap::new();
    for node in &nodes {
        if let NodeAst::Loop(loop_ast) = node {
            if let Some(first) = loop_ast.body.first() {
                loop_entries.insert(
                    loop_ast.id.clone(),
                    qualified_id(first.id(), &loop_ast.id, 0),
                );
            }
        }
    }

    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        match node {
            NodeAst::Loop(mut loop_ast) => {
                // A loop's own `:next` is exactly as much an external
                // reference as any sibling's — e.g. two sequential bounded
                // loops chained back-to-back (`loop-a :next loop-b`) — so it
                // needs the same retarget as any other node's `.next`
                // before `loop-a`'s own id disappears.
                loop_ast.next = loop_entries
                    .get(&loop_ast.next)
                    .cloned()
                    .unwrap_or(loop_ast.next);
                out.extend(unroll_loop(loop_ast, budget)?);
            }
            other => {
                charge(budget, 1)?;
                out.push(retarget_external_refs(other, &loop_entries));
            }
        }
    }
    Ok(out)
}

/// Rewrite `node`'s outgoing flow-target field(s) so any reference to a
/// sibling loop's id instead points at that loop's first-iteration entry.
/// A no-op for a node with no such reference.
fn retarget_external_refs(node: NodeAst, loop_entries: &HashMap<String, String>) -> NodeAst {
    if loop_entries.is_empty() {
        return node;
    }
    let retarget = |next: &str| {
        loop_entries
            .get(next)
            .cloned()
            .unwrap_or_else(|| next.to_string())
    };
    match node {
        NodeAst::Start(mut s) => {
            s.next = retarget(&s.next);
            NodeAst::Start(s)
        }
        NodeAst::Task(mut t) => {
            t.next = retarget(&t.next);
            NodeAst::Task(t)
        }
        NodeAst::MessageWait(mut w) => {
            w.next = retarget(&w.next);
            NodeAst::MessageWait(w)
        }
        NodeAst::TimerWait(mut w) => {
            w.next = retarget(&w.next);
            NodeAst::TimerWait(w)
        }
        NodeAst::Split(mut sp) => {
            for flow in &mut sp.flows {
                flow.next = retarget(&flow.next);
            }
            NodeAst::Split(sp)
        }
        NodeAst::Join(mut j) => {
            j.next = retarget(&j.next);
            NodeAst::Join(j)
        }
        NodeAst::End(e) => NodeAst::End(e),
        // D1: a guard's `next` is its escape-flow entry — retargeted like
        // any other next; the `host` reference is untouched here (it
        // names a node, not a flow target).
        NodeAst::BoundaryTimer(mut g) => {
            g.next = retarget(&g.next);
            NodeAst::BoundaryTimer(g)
        }
        NodeAst::BoundaryError(mut g) => {
            g.next = retarget(&g.next);
            NodeAst::BoundaryError(g)
        }
        NodeAst::Loop(_) => unreachable!("Loop nodes are expanded by the caller, never reach here"),
    }
}

fn charge(budget: &mut usize, n: usize) -> Result<(), UnrollError> {
    match budget.checked_sub(n) {
        Some(rest) => {
            *budget = rest;
            Ok(())
        }
        None => Err(UnrollError {
            message: format!(
                "loop unrolling exceeds the total-unrolled-node cap ({MAX_UNROLLED_NODES}) — \
                 reduce loop ceilings or nesting depth"
            ),
        }),
    }
}

fn unroll_loop(loop_ast: LoopAst, budget: &mut usize) -> Result<Vec<NodeAst>, UnrollError> {
    let LoopAst {
        id,
        ceiling,
        body,
        next,
        ..
    } = loop_ast;
    if ceiling == 0 {
        return Err(UnrollError {
            message: format!("loop '{id}' has ceiling 0 — a loop must run at least once"),
        });
    }
    if body.is_empty() {
        return Err(UnrollError {
            message: format!("loop '{id}' has an empty body"),
        });
    }
    let entry_id = body[0].id().to_string();
    let mut out = Vec::new();
    for index in 0..ceiling {
        let exit_target = if index + 1 < ceiling {
            qualified_id(&entry_id, &id, index + 1)
        } else {
            next.clone()
        };
        let copy = clone_body_iteration(&body, &id, index, &exit_target);
        // `unroll_nodes` below charges each node in `copy` individually (via
        // its `other` branch, or transitively through further nested
        // `unroll_loop` calls) — charging `copy.len()` here too would
        // double-count every iteration and halve the real
        // `MAX_UNROLLED_NODES` budget silently.
        out.extend(unroll_nodes(copy, budget)?);
    }
    Ok(out)
}

/// Deterministic per-copy node key: loop id + iteration index (I33).
fn qualified_id(base_id: &str, loop_id: &str, index: u32) -> String {
    format!("{base_id}__{loop_id}_{index}")
}

fn remap_next(
    next: &str,
    loop_id: &str,
    id_map: &HashMap<String, String>,
    exit_target: &str,
) -> String {
    if next == loop_id {
        exit_target.to_string()
    } else if let Some(mapped) = id_map.get(next) {
        mapped.clone()
    } else {
        // Reference to a node outside this loop body — left unchanged; dag
        // validation (which runs after unrolling) catches anything
        // structurally illegal, same as it would for any other node.
        next.to_string()
    }
}

fn clone_body_iteration(
    body: &[NodeAst],
    loop_id: &str,
    index: u32,
    exit_target: &str,
) -> Vec<NodeAst> {
    let id_map: HashMap<String, String> = body
        .iter()
        .map(|n| (n.id().to_string(), qualified_id(n.id(), loop_id, index)))
        .collect();

    body.iter()
        .map(|node| clone_node_iteration(node, loop_id, &id_map, exit_target))
        .collect()
}

fn clone_node_iteration(
    node: &NodeAst,
    loop_id: &str,
    id_map: &HashMap<String, String>,
    exit_target: &str,
) -> NodeAst {
    let new_id = |old: &str| id_map.get(old).cloned().unwrap_or_else(|| old.to_string());
    match node {
        NodeAst::Task(t) => NodeAst::Task(TaskAst {
            id: new_id(&t.id),
            plug: t.plug.clone(),
            args: t.args.clone(),
            next: remap_next(&t.next, loop_id, id_map, exit_target),
            delivery_mode: t.delivery_mode.clone(),
            span: t.span,
            // Preserve the outermost loop's id if already stamped (nested
            // loops re-clone their body once per outer iteration); only
            // stamp fresh when this is the first (possibly only) loop
            // enclosing this task.
            loop_origin: t.loop_origin.clone().or_else(|| Some(loop_id.to_string())),
        }),
        NodeAst::MessageWait(w) => NodeAst::MessageWait(MessageWaitAst {
            id: new_id(&w.id),
            name: w.name.clone(),
            correlation_source: w.correlation_source.clone(),
            next: remap_next(&w.next, loop_id, id_map, exit_target),
            span: w.span,
        }),
        NodeAst::TimerWait(w) => NodeAst::TimerWait(TimerWaitAst {
            id: new_id(&w.id),
            spec: w.spec.clone(),
            next: remap_next(&w.next, loop_id, id_map, exit_target),
            span: w.span,
        }),
        NodeAst::Split(s) => NodeAst::Split(SplitAst {
            id: new_id(&s.id),
            mode: s.mode,
            plug: s.plug.clone(),
            flows: s
                .flows
                .iter()
                .map(|f| SplitFlowAst {
                    condition: f.condition.clone(),
                    next: remap_next(&f.next, loop_id, id_map, exit_target),
                })
                .collect(),
            join: id_map.get(&s.join).cloned().unwrap_or_else(|| s.join.clone()),
            span: s.span,
        }),
        NodeAst::Join(j) => NodeAst::Join(JoinAst {
            id: new_id(&j.id),
            mode: j.mode,
            split: id_map
                .get(&j.split)
                .cloned()
                .unwrap_or_else(|| j.split.clone()),
            next: remap_next(&j.next, loop_id, id_map, exit_target),
            span: j.span,
        }),
        // A loop body wrapping a Start/End is not a shape any current
        // constructor (parser or macro) produces; kept verbatim rather than
        // rejected here — dag validation, which runs right after
        // unrolling, already rejects duplicate/misplaced Start/End nodes
        // with a precise diagnostic, so there is no silent acceptance.
        NodeAst::Start(_) | NodeAst::End(_) => node.clone(),
        // D1: a guard inside a loop body clones per iteration with its id
        // remapped; `host` goes through the SAME id map, so a guard on an
        // in-body host follows that host's clone (a guard on an
        // outside-body host keeps the original reference — the lint
        // duplicate-guard check then owns whether that shape is legal).
        NodeAst::BoundaryTimer(g) => NodeAst::BoundaryTimer(BoundaryTimerAst {
            id: new_id(&g.id),
            host: new_id(&g.host),
            spec: g.spec.clone(),
            interrupting: g.interrupting,
            budget: g.budget,
            next: remap_next(&g.next, loop_id, id_map, exit_target),
            span: g.span,
        }),
        NodeAst::BoundaryError(g) => NodeAst::BoundaryError(BoundaryErrorAst {
            id: new_id(&g.id),
            host: new_id(&g.host),
            error_code: g.error_code.clone(),
            budget: g.budget,
            next: remap_next(&g.next, loop_id, id_map, exit_target),
            span: g.span,
        }),
        NodeAst::Loop(inner) => NodeAst::Loop(LoopAst {
            id: new_id(&inner.id),
            ceiling: inner.ceiling,
            // Not remapped here: the outer `unroll_nodes` call that
            // receives this clone recurses into it and unrolls it in its
            // own right, at which point its (already iteration-qualified)
            // `id` becomes the qualification base for its own body.
            body: inner.body.clone(),
            next: remap_next(&inner.next, loop_id, id_map, exit_target),
            span: inner.span,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::macros::create_bounded_retry_macro;
    use bpmn_lite_types::SourceSpan;

    fn task(id: &str, next: &str) -> TaskAst {
        TaskAst {
            id: id.to_string(),
            plug: "cbu.create".to_string(),
            args: vec![],
            next: next.to_string(),
            delivery_mode: None,
            span: SourceSpan::new(0, 0),
            loop_origin: None,
        }
    }

    #[test]
    fn single_iteration_loop_unrolls_to_one_forward_task() {
        let body_task = task("retry-task", "loop-1"); // back-edge to the loop id
        let loop_ast = create_bounded_retry_macro(body_task, 1, "loop-1", "exit");
        let out = unroll_loops(vec![NodeAst::Loop(loop_ast)]).expect("unroll");
        assert_eq!(out.len(), 1);
        match &out[0] {
            NodeAst::Task(t) => {
                assert_eq!(t.id, "retry-task__loop-1_0");
                assert_eq!(t.next, "exit", "single iteration exits directly, no back-edge");
            }
            other => panic!("expected Task, got {other:?}"),
        }
    }

    #[test]
    fn three_iteration_loop_unrolls_to_three_forward_chained_copies() {
        let body_task = task("retry-task", "loop-1");
        let loop_ast = create_bounded_retry_macro(body_task, 3, "loop-1", "exit");
        let out = unroll_loops(vec![NodeAst::Loop(loop_ast)]).expect("unroll");
        assert_eq!(out.len(), 3, "3 ceiling -> 3 distinct copies (I33/G3.4 audit position)");

        let ids: Vec<&str> = out
            .iter()
            .map(|n| match n {
                NodeAst::Task(t) => t.id.as_str(),
                other => panic!("expected Task, got {other:?}"),
            })
            .collect();
        assert_eq!(ids, ["retry-task__loop-1_0", "retry-task__loop-1_1", "retry-task__loop-1_2"]);

        let nexts: Vec<&str> = out
            .iter()
            .map(|n| match n {
                NodeAst::Task(t) => t.next.as_str(),
                other => panic!("expected Task, got {other:?}"),
            })
            .collect();
        // forward-only chain: copy 0 -> copy 1 -> copy 2 -> exit. No node
        // ever points backward to an earlier copy or to the loop id itself.
        assert_eq!(
            nexts,
            ["retry-task__loop-1_1", "retry-task__loop-1_2", "exit"]
        );
    }

    #[test]
    fn zero_ceiling_is_a_typed_reject_not_a_silent_no_op() {
        let body_task = task("retry-task", "loop-1");
        let loop_ast = create_bounded_retry_macro(body_task, 0, "loop-1", "exit");
        let err = unroll_loops(vec![NodeAst::Loop(loop_ast)]).unwrap_err();
        assert!(err.message.contains("ceiling 0"), "got: {}", err.message);
    }

    #[test]
    fn oversized_unroll_is_refused_with_a_typed_error_naming_the_cap() {
        let body_task = task("retry-task", "loop-1");
        let loop_ast =
            create_bounded_retry_macro(body_task, MAX_UNROLLED_NODES as u32 + 1, "loop-1", "exit");
        let err = unroll_loops(vec![NodeAst::Loop(loop_ast)]).unwrap_err();
        assert!(
            err.message.contains(&MAX_UNROLLED_NODES.to_string()),
            "error must name the cap, got: {}",
            err.message
        );
    }

    #[test]
    fn nested_loop_ids_stay_unique_across_outer_iterations() {
        let inner_task = task("inner-task", "inner-loop");
        let inner_loop = create_bounded_retry_macro(inner_task, 2, "inner-loop", "outer-loop");
        // Outer loop's body is the inner loop itself (nesting), wired back
        // to the outer loop id per the same SESE convention macros use.
        let mut outer_body_loop = inner_loop;
        outer_body_loop.next = "outer-loop".to_string();
        let outer_loop = LoopAst {
            id: "outer-loop".to_string(),
            ceiling: 2,
            body: vec![NodeAst::Loop(outer_body_loop)],
            next: "exit".to_string(),
            span: SourceSpan::new(0, 0),
        };

        let out = unroll_loops(vec![NodeAst::Loop(outer_loop)]).expect("unroll");
        assert_eq!(out.len(), 4, "2 outer x 2 inner = 4 distinct forward tasks");
        let ids: Vec<&str> = out
            .iter()
            .map(|n| match n {
                NodeAst::Task(t) => t.id.as_str(),
                other => panic!("expected Task, got {other:?}"),
            })
            .collect();
        let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), 4, "no id collisions across outer x inner iterations, got {ids:?}");
    }

    // Blind-review regression (2026-08-11): a loop's own `:next` was never
    // retargeted through `loop_entries`, so two sequential bounded loops
    // chained back-to-back (`loop-a :next loop-b`) miscompiled — `loop-a`'s
    // last iteration pointed at the literal string "loop-b", which no
    // longer names any node once loop-b itself unrolls away.
    #[test]
    fn a_loops_own_next_pointing_at_a_sibling_loop_is_retargeted_to_its_entry() {
        let task_a = task("task-a", "loop-a");
        let loop_a = LoopAst {
            id: "loop-a".to_string(),
            ceiling: 2,
            body: vec![NodeAst::Task(task_a)],
            next: "loop-b".to_string(), // points at a SIBLING loop, not an exit id
            span: SourceSpan::new(0, 0),
        };
        let task_b = task("task-b", "loop-b");
        let loop_b = create_bounded_retry_macro(task_b, 2, "loop-b", "exit");

        let out = unroll_loops(vec![NodeAst::Loop(loop_a), NodeAst::Loop(loop_b)]).expect("unroll");
        assert_eq!(out.len(), 4, "2 + 2 forward copies, no dangling reference");

        let last_of_a = out
            .iter()
            .find_map(|n| match n {
                NodeAst::Task(t) if t.id == "task-a__loop-a_1" => Some(t),
                _ => None,
            })
            .expect("loop-a's last iteration present");
        assert_eq!(
            last_of_a.next, "task-b__loop-b_0",
            "loop-a's exit must resolve to loop-b's first-iteration entry, not the vanished 'loop-b' id"
        );
    }

    // Blind-review regression (2026-08-11): `unroll_loop` charged
    // `copy.len()` against the budget AND `unroll_nodes` independently
    // charged 1 per node while processing that same `copy` -- doubling the
    // real cost per node and silently halving `MAX_UNROLLED_NODES` from its
    // documented/tested value.
    #[test]
    fn budget_is_charged_once_per_node_not_twice() {
        let body_task = task("t", "loop-1");
        // Exactly at the documented cap must succeed...
        let at_cap = create_bounded_retry_macro(body_task.clone(), MAX_UNROLLED_NODES as u32, "loop-1", "exit");
        let out = unroll_loops(vec![NodeAst::Loop(at_cap)]).expect("exactly MAX_UNROLLED_NODES must be admitted");
        assert_eq!(out.len(), MAX_UNROLLED_NODES);

        // ...and one node past it must still be the exact boundary that fails.
        let over_cap = create_bounded_retry_macro(body_task, MAX_UNROLLED_NODES as u32 + 1, "loop-1", "exit");
        let err = unroll_loops(vec![NodeAst::Loop(over_cap)]).unwrap_err();
        assert!(err.message.contains(&MAX_UNROLLED_NODES.to_string()));
    }
}
