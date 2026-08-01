//! Deterministic binding extraction — the serving-side slot resolver
//! that turns a policy `Candidate` disposition into concrete
//! `designer_graph::ops::Operation`s (or an honest missing-bindings
//! list), downstream of `policy::decide` and strictly outside it.
//!
//! WHY THIS CRATE (dependency direction): binding needs the utterance
//! text, the session's `DesignerDag`, the anchor, and the candidate id —
//! a serving-layer closure. `designer-graph` stays utterance-agnostic
//! graph algebra; `utterance-engine`'s policy stays evidence-in /
//! disposition-out (I27). When the DIR-002 SLM lands, it replaces the
//! RULES in this module, not the seam: same inputs, same
//! `BindingOutcome`, and the SLM's output remains evidence — the dry
//! stage + human ratify downstream never move (EOP-SPEC-SLM-TRAIN-001
//! §S7: zero direct model-authorised executions by construction).
//!
//! DETERMINISTIC RULES (each documented at its use site; the catalogue):
//! - R1 anchor: the operation's anchor/host/target NodeKey is the
//!   utterance's explicit `anchor` (already resolved by the endpoint).
//!   No anchor supplied → missing `"anchor"`. Never inferred from text.
//! - R2 fresh identity: new `NodeKey`s are minted (`Uuid::new_v4`) —
//!   identity minting, not semantic inference (same F4 discipline as a
//!   REST client authoring a graph-edit). BPMN ids/edge ids are DERIVED
//!   deterministically from the bound name (`flow_{name}`,
//!   `{name}_guard`, `{name}_end`); a derived id colliding with an
//!   existing one is refused by the dry stage (fail closed — the user
//!   re-utters with a different name), never silently uniquified.
//! - R3 names: quoted spans in the utterance ('…' or "…"), in order of
//!   appearance, sanitized to snake_case ids. A binder needing N names
//!   and finding fewer reports them missing. Never derived from
//!   unquoted "salient" words — that would be guessing.
//! - R4 task_type: created `ServiceTask`s get `task_type = "noop"`.
//!   Judged semantically safe: it is the substrate's draft placeholder
//!   (every designer fixture uses it) and is refined by later edits;
//!   an utterance never carries a plug binding.
//! - R5 durations: `<N> <unit>` plain forms (ms/second/minute/hour/
//!   day/week) and simple ISO-8601 (`PnDTnHnMnS`). A duration preceded
//!   by the word "every" is an INTERVAL (cycle trigger); otherwise it
//!   is a plain duration (timeout trigger). First match of each class
//!   wins.
//! - R6 counts: `<N> times` binds a cycle's `max_fires`. A bare integer
//!   that is neither a duration magnitude nor a `times` count binds
//!   `declared_max` / `failure_budget` where a binder needs one.
//! - R7 data references: a `corr_key_source` / `collection_flag_name`
//!   must name an EXISTING `DataObject` — bound only when a declared
//!   data object's id appears verbatim as a token of the utterance.
//!   No fabricated data objects, ever.
//!
//! A binding that cannot be derived by these rules is MISSING, not
//! defaulted. Candidates with no deterministic rule at all (Connect's
//! two endpoints, CreateBranch's condition, inclusive-region
//! conditions, bare guard attachment whose escape flow can never admit
//! alone) report their unresolvable bindings — coverage honesty over
//! breadth.

use designer_graph::ops::{GuardTrigger, Operation, RegionBranch};
use designer_graph::productions;
use designer_graph::schema::{DesignerDag, NodeKey};
use bpmn_lite_compiler::{IRNode, TimerSpec};
use uuid::Uuid;

/// A successfully bound proposal: the concrete operation sequence and a
/// human-facing description of what ratifying it will do.
pub(crate) struct BoundProposal {
    pub ops: Vec<Operation>,
    pub description: String,
}

pub(crate) enum BindingOutcome {
    Bound(BoundProposal),
    /// The candidate is real but these binding names could not be
    /// derived under the documented rules — populates the
    /// `MissingArguments` response shape. Nothing is staged.
    Missing(Vec<String>),
}

fn fresh_key() -> NodeKey {
    NodeKey(Uuid::new_v4())
}

fn task(name: &str) -> IRNode {
    // R4: draft placeholder task_type.
    IRNode::ServiceTask { id: name.into(), name: name.into(), task_type: "noop".into() }
}

fn end_node(id: String) -> IRNode {
    IRNode::End { id, terminate: false }
}

/// R3: quoted spans, in order, sanitized to snake_case ids.
fn quoted_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' || c == '"' {
            let quote = c;
            let mut span = String::new();
            for d in chars.by_ref() {
                if d == quote {
                    break;
                }
                span.push(d);
            }
            let sanitized = sanitize(&span);
            if !sanitized.is_empty() {
                out.push(sanitized);
            }
        }
    }
    out
}

fn sanitize(s: &str) -> String {
    let mut out = String::new();
    let mut last_us = true; // suppress leading underscores
    for c in s.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
            last_us = false;
        } else if !last_us {
            out.push('_');
            last_us = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_owned())
        .filter(|w| !w.is_empty())
        .collect()
}

fn unit_ms(unit: &str) -> Option<u64> {
    match unit {
        "ms" | "millisecond" | "milliseconds" => Some(1),
        "second" | "seconds" | "sec" | "secs" => Some(1_000),
        "minute" | "minutes" | "min" | "mins" => Some(60_000),
        "hour" | "hours" | "hr" | "hrs" => Some(3_600_000),
        "day" | "days" => Some(86_400_000),
        "week" | "weeks" => Some(604_800_000),
        _ => None,
    }
}

/// Minimal ISO-8601 duration: `PnW` / `PnD` / `PTnH` / `PTnM` / `PTnS`
/// and their concatenations. Anything unparseable is simply not a
/// duration token (no error — R5's plain forms remain available).
fn iso8601_ms(word: &str) -> Option<u64> {
    let w = word.to_uppercase();
    let rest = w.strip_prefix('P')?;
    let (date_part, time_part) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };
    let mut ms: u64 = 0;
    let mut any = false;
    let mut parse_segments = |s: &str, units: &[(char, u64)]| -> Option<()> {
        let mut num = String::new();
        for c in s.chars() {
            if c.is_ascii_digit() {
                num.push(c);
            } else {
                let (_, mult) = units.iter().find(|(u, _)| *u == c)?;
                let n: u64 = num.parse().ok()?;
                ms = ms.checked_add(n.checked_mul(*mult)?)?;
                any = true;
                num.clear();
            }
        }
        if num.is_empty() { Some(()) } else { None }
    };
    parse_segments(date_part, &[('W', 604_800_000), ('D', 86_400_000)])?;
    parse_segments(time_part, &[('H', 3_600_000), ('M', 60_000), ('S', 1_000)])?;
    if any { Some(ms) } else { None }
}

struct ParsedDuration {
    ms: u64,
    /// R5: preceded by "every" → interval (cycle), else plain duration.
    every: bool,
}

fn durations(text: &str) -> Vec<ParsedDuration> {
    let ws = words(text);
    let mut out = Vec::new();
    for (i, w) in ws.iter().enumerate() {
        let every = i > 0 && ws[i - 1] == "every";
        if let Some(ms) = iso8601_ms(w) {
            out.push(ParsedDuration { ms, every });
            continue;
        }
        if let Ok(n) = w.parse::<u64>() {
            if let Some(mult) = ws.get(i + 1).and_then(|u| unit_ms(u)) {
                if let Some(ms) = n.checked_mul(mult) {
                    out.push(ParsedDuration { ms, every });
                }
            }
        }
    }
    out
}

/// R5: the first plain (non-"every") duration.
fn plain_duration_ms(text: &str) -> Option<u64> {
    durations(text).into_iter().find(|d| !d.every).map(|d| d.ms)
}

/// R5: the first "every"-prefixed duration (a cycle interval).
fn interval_ms(text: &str) -> Option<u64> {
    durations(text).into_iter().find(|d| d.every).map(|d| d.ms)
}

/// R6: `<N> times`.
fn max_fires(text: &str) -> Option<u32> {
    let ws = words(text);
    for (i, w) in ws.iter().enumerate() {
        if let Ok(n) = w.parse::<u32>() {
            if matches!(ws.get(i + 1).map(String::as_str), Some("time") | Some("times")) {
                return Some(n);
            }
        }
    }
    None
}

/// R6: first bare integer that is neither a duration magnitude nor a
/// `times` count.
fn bare_integer(text: &str) -> Option<u32> {
    let ws = words(text);
    for (i, w) in ws.iter().enumerate() {
        if let Ok(n) = w.parse::<u32>() {
            let next = ws.get(i + 1).map(String::as_str);
            let is_duration = next.map(|u| unit_ms(u).is_some()).unwrap_or(false);
            let is_times = matches!(next, Some("time") | Some("times"));
            if !is_duration && !is_times {
                return Some(n);
            }
        }
    }
    None
}

/// R7: the first declared `DataObject` whose id appears verbatim as a
/// token of the utterance (tokenized on non-alphanumerics EXCEPT `_`,
/// so `corr_flag` survives as one token).
fn matched_data_object(dag: &DesignerDag, text: &str) -> anyhow::Result<Option<String>> {
    let ir = dag.to_ir()?;
    let data_ids: Vec<String> = ir
        .node_indices()
        .filter_map(|i| match &ir[i] {
            IRNode::DataObject { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let tokens: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(data_ids.into_iter().find(|d| tokens.contains(&d.to_lowercase())))
}

fn anchor_bpmn_id(dag: &DesignerDag, anchor: NodeKey) -> anyhow::Result<String> {
    let ir = dag.to_ir()?;
    // to_ir preserves ids; resolve via the dag's own reverse lookup.
    for i in ir.node_indices() {
        if dag.key_for_bpmn_id(ir[i].id()) == Some(anchor) {
            return Ok(ir[i].id().to_owned());
        }
    }
    anyhow::bail!("anchor key resolves to no node")
}

/// Attempt to bind `candidate_id` (a canonical id from the board /
/// disposition) against the utterance text at `anchor`. `Ok(Missing)`
/// is the honest not-derivable outcome; `Err` is an internal fault
/// (projection failure), never a user-facing shape.
pub(crate) fn bind_candidate(
    dag: &DesignerDag,
    anchor: Option<NodeKey>,
    candidate_id: &str,
    text: &str,
) -> anyhow::Result<BindingOutcome> {
    use BindingOutcome::{Bound, Missing};

    // R1: every implemented binder needs the explicit anchor.
    let need_anchor = |missing: &mut Vec<String>| -> Option<NodeKey> {
        if anchor.is_none() {
            missing.push("anchor".into());
        }
        anchor
    };
    let names = quoted_names(text);
    let mut missing: Vec<String> = Vec::new();

    let outcome = match candidate_id {
        // ── Single-task insertions: anchor + one quoted name ─────────
        "op.append_node" | "op.insert_after" | "op.insert_before" | "op.replace_node" => {
            let a = need_anchor(&mut missing);
            let name = names.first().cloned();
            if name.is_none() {
                missing.push("task_name (quote the new step's name)".into());
            }
            match (a, name) {
                (Some(a), Some(name)) => {
                    let key = fresh_key();
                    let node = task(&name);
                    let edge_id = format!("flow_{name}");
                    let anchor_id = anchor_bpmn_id(dag, a)?;
                    let (op, verb) = match candidate_id {
                        "op.append_node" => (
                            Operation::AppendNode { anchor: a, key, node, edge_id },
                            "append",
                        ),
                        "op.insert_after" => (
                            Operation::InsertAfter { anchor: a, key, node, edge_id },
                            "insert after",
                        ),
                        "op.insert_before" => (
                            Operation::InsertBefore { anchor: a, key, node, edge_id },
                            "insert before",
                        ),
                        _ => (Operation::ReplaceNode { target: a, key, node }, "replace"),
                    };
                    Bound(BoundProposal {
                        ops: vec![op],
                        description: format!("{verb} '{anchor_id}': service task '{name}'"),
                    })
                }
                _ => Missing(missing),
            }
        }
        // ── DeleteSubgraph → DeleteNode: anchor only ─────────────────
        "op.delete_subgraph" => match need_anchor(&mut missing) {
            Some(a) => {
                let anchor_id = anchor_bpmn_id(dag, a)?;
                Bound(BoundProposal {
                    ops: vec![Operation::DeleteNode { target: a }],
                    description: format!("delete node '{anchor_id}'"),
                })
            }
            None => Missing(missing),
        },
        // ── SetCorrelationSource: anchor + R7 data-object match ──────
        "op.set_correlation_source" => {
            let a = need_anchor(&mut missing);
            let dobj = matched_data_object(dag, text)?;
            if dobj.is_none() {
                missing.push("correlation_source (name a declared data object)".into());
            }
            match (a, dobj) {
                (Some(a), Some(d)) => Bound(BoundProposal {
                    ops: vec![Operation::SetCorrelationSource {
                        node: a,
                        corr_key_source: d.clone(),
                    }],
                    description: format!("set correlation source to '{d}'"),
                }),
                _ => Missing(missing),
            }
        }
        // ── SetGuardBudget: anchor (the guard) + R6 bare integer ─────
        "op.set_guard_budget" => {
            let a = need_anchor(&mut missing);
            let n = bare_integer(text);
            if n.is_none() {
                missing.push("failure_budget (a number)".into());
            }
            match (a, n) {
                (Some(a), Some(n)) => Bound(BoundProposal {
                    ops: vec![Operation::SetGuardBudget { guard: a, failure_budget: Some(n) }],
                    description: format!("set guard failure budget to {n}"),
                }),
                _ => Missing(missing),
            }
        }
        // ── SetGuardTrigger: cycle ("every N … M times") else plain
        //    duration; neither derivable → missing ────────────────────
        "op.set_guard_trigger" => {
            let a = need_anchor(&mut missing);
            let trigger = match (interval_ms(text), max_fires(text), plain_duration_ms(text)) {
                (Some(interval), Some(fires), _) => {
                    Some(GuardTrigger::Timer(TimerSpec::Cycle {
                        interval_ms: interval,
                        max_fires: fires,
                    }))
                }
                (_, _, Some(ms)) => Some(GuardTrigger::Timer(TimerSpec::Duration { ms })),
                _ => None,
            };
            if trigger.is_none() {
                missing.push(
                    "trigger (a duration like '3 days', or 'every 2 hours' plus 'N times')"
                        .into(),
                );
            }
            match (a, trigger) {
                (Some(a), Some(t)) => Bound(BoundProposal {
                    ops: vec![Operation::SetGuardTrigger { guard: a, trigger: t }],
                    description: "set guard trigger from the stated schedule".into(),
                }),
                _ => Missing(missing),
            }
        }
        // ── Multi-instance: anchor + name + R7 collection + R6 max ───
        "op.create_multi_instance_region" | "prod.for_each_with_ceiling" => {
            let a = need_anchor(&mut missing);
            let name = names.first().cloned();
            if name.is_none() {
                missing.push("task_name (quote the per-element step's name)".into());
            }
            let coll = matched_data_object(dag, text)?;
            if coll.is_none() {
                missing.push("collection_flag_name (name a declared data object)".into());
            }
            let max = bare_integer(text);
            if max.is_none() {
                missing.push("declared_max (a number)".into());
            }
            match (a, name, coll, max) {
                (Some(a), Some(name), Some(coll), Some(max)) => Bound(BoundProposal {
                    description: format!(
                        "for each element of '{coll}' (max {max}): task '{name}'"
                    ),
                    ops: productions::for_each_with_ceiling(
                        productions::ForEachWithCeilingBindings {
                            anchor: a,
                            key: fresh_key(),
                            node: IRNode::MultiInstance {
                                id: name.clone(),
                                name: name.clone(),
                                task_type: "noop".into(), // R4
                                collection_flag_name: coll,
                                declared_max: max,
                            },
                            edge_id: format!("flow_{name}"),
                        },
                    ),
                }),
                _ => Missing(missing),
            }
        }
        // ── Parallel region: anchor + ≥2 quoted branch names ─────────
        "op.create_parallel_region" | "prod.parallel_checks_and_join" => {
            let a = need_anchor(&mut missing);
            if names.len() < 2 {
                missing.push("branch_names (quote at least two step names)".into());
            }
            match a {
                Some(a) if names.len() >= 2 => {
                    let base = names[0].clone();
                    let branches: Vec<RegionBranch> = names
                        .iter()
                        .map(|n| RegionBranch {
                            key: fresh_key(),
                            node: task(n),
                            in_edge_id: format!("flow_in_{n}"),
                            out_edge_id: format!("flow_out_{n}"),
                            condition: None,
                        })
                        .collect();
                    Bound(BoundProposal {
                        description: format!("parallel branches: {}", names.join(", ")),
                        ops: productions::parallel_checks_and_join(
                            productions::ParallelChecksAndJoinBindings {
                                anchor: a,
                                fork_key: fresh_key(),
                                fork_node_id: format!("{base}_fork"),
                                join_key: fresh_key(),
                                join_node_id: format!("{base}_join"),
                                entry_edge_id: format!("flow_{base}_entry"),
                                branches,
                            },
                        ),
                    })
                }
                _ => Missing(missing),
            }
        }
        // ── RequestAndWait: anchor + two names (send, wait) + R7 ─────
        "prod.request_and_wait" => {
            let a = need_anchor(&mut missing);
            if names.len() < 2 {
                missing.push(
                    "send_and_wait_names (quote two names: the send step, then the wait)".into(),
                );
            }
            let dobj = matched_data_object(dag, text)?;
            if dobj.is_none() {
                missing.push("correlation_source (name a declared data object)".into());
            }
            match (a, dobj) {
                (Some(a), Some(d)) if names.len() >= 2 => {
                    let (send, wait) = (names[0].clone(), names[1].clone());
                    Bound(BoundProposal {
                        description: format!(
                            "send '{send}' then wait '{wait}' correlated on '{d}'"
                        ),
                        ops: productions::request_and_wait(productions::RequestAndWaitBindings {
                            anchor: a,
                            send_key: fresh_key(),
                            send_node: task(&send),
                            send_edge_id: format!("flow_{send}"),
                            wait_key: fresh_key(),
                            wait_node: IRNode::MessageWait {
                                id: wait.clone(),
                                name: wait.clone(),
                                corr_key_source: d,
                            },
                            wait_edge_id: format!("flow_{wait}"),
                        }),
                    })
                }
                _ => Missing(missing),
            }
        }
        // ── InterruptingTimeout: anchor + plain duration + name ──────
        "prod.interrupting_timeout" => {
            let a = need_anchor(&mut missing);
            let ms = plain_duration_ms(text);
            if ms.is_none() {
                missing.push("timeout_duration (e.g. '3 days' or 'PT45M')".into());
            }
            let name = names.first().cloned();
            if name.is_none() {
                missing.push("continuation_name (quote the timeout step's name)".into());
            }
            match (a, ms, name) {
                (Some(a), Some(ms), Some(name)) => Bound(BoundProposal {
                    description: format!("interrupting timeout → '{name}'"),
                    ops: productions::interrupting_timeout(
                        productions::InterruptingTimeoutBindings {
                            anchor: a,
                            guard_key: fresh_key(),
                            guard_id: format!("{name}_guard"),
                            duration_ms: ms,
                            continuation_key: fresh_key(),
                            continuation_node: task(&name),
                            continuation_edge_id: format!("flow_{name}"),
                            continuation_end_key: fresh_key(),
                            continuation_end_node: end_node(format!("{name}_end")),
                            continuation_end_edge_id: format!("flow_{name}_end"),
                        },
                    ),
                }),
                _ => Missing(missing),
            }
        }
        // ── Cycle-guard productions: anchor + interval + fires + name ─
        "prod.non_interrupting_notification" | "prod.reminder_then_escalate" => {
            let a = need_anchor(&mut missing);
            let interval = interval_ms(text);
            if interval.is_none() {
                missing.push("interval (say 'every <duration>')".into());
            }
            let fires = max_fires(text);
            if fires.is_none() {
                missing.push("max_fires (say '<N> times')".into());
            }
            let name = names.first().cloned();
            if name.is_none() {
                missing.push("step_name (quote the notification/escalation step's name)".into());
            }
            match (a, interval, fires, name) {
                (Some(a), Some(interval), Some(fires), Some(name)) => {
                    let ops = if candidate_id == "prod.reminder_then_escalate" {
                        productions::reminder_then_escalate(
                            productions::ReminderThenEscalateBindings {
                                anchor: a,
                                guard_key: fresh_key(),
                                guard_id: format!("{name}_guard"),
                                cycle: TimerSpec::Cycle {
                                    interval_ms: interval,
                                    max_fires: fires,
                                },
                                escalation_key: fresh_key(),
                                escalation_node: task(&name),
                                escalation_edge_id: format!("flow_{name}"),
                                escalation_end_key: fresh_key(),
                                escalation_end_node: end_node(format!("{name}_end")),
                                escalation_end_edge_id: format!("flow_{name}_end"),
                            },
                        )
                    } else {
                        productions::non_interrupting_notification(
                            productions::NonInterruptingNotificationBindings {
                                anchor: a,
                                guard_key: fresh_key(),
                                guard_id: format!("{name}_guard"),
                                interval_ms: interval,
                                max_fires: fires,
                                notification_key: fresh_key(),
                                notification_node: task(&name),
                                notification_edge_id: format!("flow_{name}"),
                                notification_end_key: fresh_key(),
                                notification_end_node: end_node(format!("{name}_end")),
                                notification_end_edge_id: format!("flow_{name}_end"),
                            },
                        )
                    };
                    Bound(BoundProposal {
                        description: format!("re-arming cycle guard → '{name}'"),
                        ops,
                    })
                }
                _ => Missing(missing),
            }
        }
        // ── Honest non-bindables: real candidates whose bindings have
        //    no deterministic rule today ───────────────────────────────
        "op.connect" => Missing(vec!["from_node".into(), "to_node".into()]),
        "op.create_branch" => Missing(vec!["target_node".into(), "branch_condition".into()]),
        "op.create_inclusive_region" => Missing(vec![
            "branch_names (quote at least two step names)".into(),
            "branch_conditions (one per branch — inclusive branches must be conditional)".into(),
        ]),
        // A bare boundary guard can never admit (verifier §7a: "no
        // outgoing sequence flow" is a reject) — the guard's escape flow
        // is unresolvable from an attach-only utterance. The bindable
        // route is the corresponding production.
        "op.attach_guard" => Missing(vec![
            "escape_flow (use the interrupting-timeout production: state a duration and quote the continuation step)".into(),
        ]),
        "op.attach_rearming_guard" => Missing(vec![
            "escape_flow (use the notification/reminder production: say 'every <duration>', '<N> times', and quote the step)".into(),
        ]),
        // Anything else (never-boarded catalogue entries, Abstain,
        // future ids): no rule — honest missing, never a guess.
        other => Missing(vec![format!("no deterministic binding rule for '{other}'")]),
    };
    Ok(outcome)
}
