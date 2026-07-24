use crate::ir::*;
use anyhow::{anyhow, Result};
use bpmn_lite_types::{Addr, CompiledProgram, Instr};
use petgraph::graph::NodeIndex;
use petgraph::visit::Dfs;
use std::collections::HashMap;

/// Verification errors.
#[derive(Debug, Clone)]
pub struct VerifyError {
    pub message: String,
    pub element_id: Option<String>,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(id) = &self.element_id {
            write!(f, "[{}] {}", id, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

/// Verify structural invariants of the IR graph.
///
/// Returns a list of errors. Empty list means the graph is valid.
pub fn verify(graph: &IRGraph) -> Vec<VerifyError> {
    let mut errors = Vec::new();

    // 1. Exactly one StartEvent
    let starts: Vec<_> = graph
        .node_indices()
        .filter(|&idx| matches!(&graph[idx], IRNode::Start { .. }))
        .collect();

    if starts.is_empty() {
        errors.push(VerifyError {
            message: "No StartEvent found".to_string(),
            element_id: None,
        });
    } else if starts.len() > 1 {
        errors.push(VerifyError {
            message: format!("Multiple StartEvents found ({})", starts.len()),
            element_id: None,
        });
    }

    // 2. At least one EndEvent
    let ends: Vec<_> = graph
        .node_indices()
        .filter(|&idx| matches!(&graph[idx], IRNode::End { .. }))
        .collect();

    if ends.is_empty() {
        errors.push(VerifyError {
            message: "No EndEvent found".to_string(),
            element_id: None,
        });
    }

    // 2b. No cycles. Independent-review finding (2026-07-24, against
    // `lowering.rs`'s dominance-based `compute_post_dominators`): that
    // function's single-pass (no fixed-point iteration) reverse-postorder
    // dominance computation is only correct on an acyclic graph, and
    // nothing in this XML→IR pipeline previously enforced that — a raw
    // BPMN sequence-flow back edge (a gateway routed to an earlier task,
    // no `multiInstanceLoopCharacteristics`/`GUARD-TIMER-CYCLE>` involved)
    // would parse and reach `lower()` unrejected, silently producing a
    // WRONG (not merely missing) post-dominator for the affected node
    // rather than failing loudly — demonstrated by direct construction,
    // not merely argued. Per CLAUDE.md's settled decisions ("Workflow
    // topology is SESE only," "Loops are finitely bounded"), this pipeline
    // has no sanctioned raw back-edge construct at all — a bounded loop is
    // expressed via `MultiInstance`/`GUARD-TIMER-CYCLE>`, neither of which
    // creates an `IRGraph` back edge (confirmed: zero `Instr::BrCounterLt`
    // — the bounded-loop backward-jump opcode — is ever emitted by this
    // pipeline's `lowering.rs`, only by the separate `dsl/frontend.rs`
    // S-expression pipeline, which has its own, unrelated DAG-vs-cycle
    // story). A cyclic `IRGraph` reaching this point is therefore always
    // illegitimate input, not an untested-but-legal case — rejected here,
    // structurally, rather than left as an unenforced assumption three
    // downstream functions each independently hope holds.
    if petgraph::algo::is_cyclic_directed(graph) {
        errors.push(VerifyError {
            message: "Graph contains a cycle — raw sequence-flow back edges are not supported; \
                       express a bounded loop via multi-instance or a cyclic boundary timer \
                       instead"
                .to_string(),
            element_id: None,
        });
    }

    // 3. All nodes reachable from Start (or from BoundaryTimer nodes,
    //    which are alternative entry points for escalation paths)
    if let Some(start_idx) = starts.first() {
        let mut reachable = std::collections::HashSet::new();

        // DFS from Start
        let mut dfs = Dfs::new(graph, *start_idx);
        while let Some(nx) = dfs.next(graph) {
            reachable.insert(nx);
        }

        // Also DFS from each BoundaryTimer/BoundaryError node (escalation/error paths)
        for idx in graph.node_indices() {
            let is_boundary = matches!(
                &graph[idx],
                IRNode::BoundaryTimer { .. } | IRNode::BoundaryError { .. }
            );
            if is_boundary && !reachable.contains(&idx) {
                reachable.insert(idx);
                let mut bdfs = Dfs::new(graph, idx);
                while let Some(nx) = bdfs.next(graph) {
                    reachable.insert(nx);
                }
            }
        }

        for idx in graph.node_indices() {
            // DataObject nodes are structural declarations with no sequence-flow
            // edges; they are intentionally unconnected and must not be flagged.
            if matches!(&graph[idx], IRNode::DataObject { .. }) {
                continue;
            }
            if !reachable.contains(&idx) {
                errors.push(VerifyError {
                    message: format!("Unreachable node: {}", graph[idx].id()),
                    element_id: Some(graph[idx].id().to_string()),
                });
            }
        }
    }

    // 4. Parallel gateways: check fork/join pairs
    let forks: Vec<_> = graph
        .node_indices()
        .filter(|&idx| {
            matches!(
                &graph[idx],
                IRNode::GatewayAnd {
                    direction: GatewayDirection::Diverging,
                    ..
                }
            )
        })
        .collect();

    let joins: Vec<_> = graph
        .node_indices()
        .filter(|&idx| {
            matches!(
                &graph[idx],
                IRNode::GatewayAnd {
                    direction: GatewayDirection::Converging,
                    ..
                }
            )
        })
        .collect();

    if forks.len() != joins.len() {
        errors.push(VerifyError {
            message: format!(
                "Mismatched parallel gateways: {} forks, {} joins",
                forks.len(),
                joins.len()
            ),
            element_id: None,
        });
    }

    // 4a. Parallel/inclusive gateways: structural well-nestedness (SESE),
    // not just matching counts. Adam, 2026-07-22: V5's XML->V2Fork/V2Join
    // lowering pre-pass pairs each Converging gateway with a Diverging one
    // via a stack, correct only for well-nested topology — a property this
    // codebase has always called "SESE-only" (CLAUDE.md's settled
    // decision, never itself promoted into a V&S theorem) but which
    // nothing in this pipeline actually checked: `dto_to_ir` -> `verify`
    // -> `lower` never ran the DSL's `rpst::verify_sese_nesting` (that
    // lives on a separate importer path, into `WorkflowExecutionPlan`, not
    // `IRGraph`). Mirrors the DSL check's DFS-second-visit algorithm as
    // closely as `IRNode`'s schema allows: neither `GatewayAnd` nor
    // `GatewayInclusive` carries an explicit fork/join name reference
    // (unlike the DSL's `JoinExecNode.split`), so this catches unmatched
    // forks/joins and gross stack-order/cross-kind crossing structurally,
    // but cannot independently cross-check a join against its *intended*
    // fork by name the way the DSL's check does — a strictly weaker, not
    // equivalent, guarantee.
    //
    // V6 (2026-07-24): `check_gateway_and_nesting` used to thread a single
    // `fork_stack` through the WHOLE recursion without cloning it before
    // recursing into each of a branch point's neighbors — so a push/pop
    // from one branch leaked into a sibling branch's traversal, causing
    // this check to REJECT genuinely well-nested topology (two
    // independently-nested `GatewayAnd` pairs in sibling branches of an
    // outer fork, at different branch lengths) with a misleading "Unmatched
    // GatewayAnd (converging)" message. Fixed by porting
    // `dsl::rpst::dfs_walk`'s clone-the-stack-per-branch-before-recursing
    // discipline (mirroring `lowering.rs`'s `compute_gateway_pairing`,
    // fixed in the same landing for the analogous pairing-derivation bug).
    // Also extended, in the same pass, to cover `GatewayInclusive` — it
    // previously tracked ONLY `GatewayAnd` — using ONE unified stack tagged
    // by kind (`GatewayKind`), not two independent per-kind stacks: see
    // `lowering.rs`'s `compute_gateway_pairing` doc comment for the full
    // framing-decision writeup (the two kinds can legally nest inside each
    // other's branches today, and a unified stack is what lets this check
    // also catch a genuine cross-kind crossing hazard, not just each kind's
    // own internal mis-nesting). This does not interact with §9's
    // blanket "≤1 `GatewayInclusive` pair" admission gate below — that
    // gate's own count-based rejection logic is untouched; it still fires
    // first (verifier errors are non-fatal-until-aggregated, all still
    // collected here) whenever more than one `GatewayInclusive` pair is
    // present, regardless of what this nesting check independently finds.
    if let Some(start_idx) = find_start(graph) {
        let mut visited = std::collections::HashSet::new();
        let mut fork_stack: Vec<(GatewayKind, String, u32)> = Vec::new();
        let mut tracker = GatewayClosureTracker::default();
        check_gateway_and_nesting(
            graph,
            start_idx,
            &mut visited,
            &mut fork_stack,
            &mut tracker,
            &mut errors,
        );
        // `fork_stack.is_empty()` is NOT the closure check (see
        // `GatewayClosureTracker`'s doc comment — the clone-per-branch fix
        // means this top-level variable can never receive a push). The real
        // check: every diverging gateway's EVERY immediate branch (`0..
        // out_degree`) must have been closed by a matching-kind pop
        // somewhere in that branch's own subtree — a diverging node with
        // some but not all branches closed still dangles.
        let mut unclosed: Vec<String> = tracker
            .all_diverging
            .iter()
            .filter(|(id, out_degree)| {
                (0..**out_degree).any(|branch_index| {
                    !tracker.closed.contains(&((*id).clone(), branch_index))
                })
            })
            .map(|(id, _)| id.clone())
            .collect();
        unclosed.sort();
        if !unclosed.is_empty() {
            errors.push(VerifyError {
                message: format!(
                    "Unclosed diverging gateway node(s) — a branch never reaches its \
                     matching converging gateway: [{}]",
                    unclosed
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                element_id: None,
            });
        }
    }

    // 5. All task_type references are non-empty (ServiceTask)
    for idx in graph.node_indices() {
        if let IRNode::ServiceTask { id, task_type, .. } = &graph[idx] {
            if task_type.is_empty() {
                errors.push(VerifyError {
                    message: "ServiceTask has empty task_type".to_string(),
                    element_id: Some(id.clone()),
                });
            }
        }
    }

    // 6. XOR diverging gateways should have at least one outgoing edge with a condition
    //    and exactly one default (no condition)
    for idx in graph.node_indices() {
        if matches!(&graph[idx], IRNode::GatewayXor { .. }) {
            let outgoing: Vec<_> = graph
                .edges_directed(idx, petgraph::Direction::Outgoing)
                .collect();

            if outgoing.len() > 1 {
                let with_condition = outgoing
                    .iter()
                    .filter(|e| e.weight().condition.is_some())
                    .count();
                let without_condition = outgoing.len() - with_condition;

                if without_condition != 1 {
                    errors.push(VerifyError {
                        message: format!(
                            "XOR gateway should have exactly 1 default edge, found {}",
                            without_condition
                        ),
                        element_id: Some(graph[idx].id().to_string()),
                    });
                }
            }
        }
    }

    // 7. Boundary event validation
    {
        let mut host_boundary_count: HashMap<String, Vec<String>> = HashMap::new();

        for idx in graph.node_indices() {
            if let IRNode::BoundaryTimer {
                id,
                attached_to,
                interrupting,
                spec,
                failure_budget: _,
            } = &graph[idx]
            {
                // 7a. attached_to must reference an existing ServiceTask,
                // FfiServiceTask, or HumanWait. `FfiServiceTask` was
                // missing from this check pre-existing this fix — found
                // while wiring `GUARD-TIMER-CYCLE>` through the frontend
                // (`lowering.rs`'s `IRNode::FfiServiceTask` boundary-timer
                // arm already fully lowers a boundary timer host on an
                // FFI service task, but no BPMN XML could ever reach that
                // code end-to-end, because this check rejected the
                // BoundaryTimer's own `attachedToRef` before lowering ever
                // ran). Not a design fork — `FfiServiceTask` is exactly as
                // valid a boundary-timer host as `ServiceTask`, the
                // lowering support for it already existed, and no test
                // ever exercised the combination through the full
                // pipeline to catch the omission.
                let host_exists = graph.node_indices().any(|other| {
                    matches!(&graph[other],
                        IRNode::ServiceTask { id: host_id, .. }
                        | IRNode::FfiServiceTask { id: host_id, .. }
                        | IRNode::HumanWait { id: host_id, .. }
                        if host_id == attached_to
                    )
                });
                if !host_exists {
                    errors.push(VerifyError {
                        message: format!(
                            "BoundaryTimer '{}' attachedToRef '{}' does not reference a task",
                            id, attached_to
                        ),
                        element_id: Some(id.clone()),
                    });
                }

                // 7b. Cycle timers MUST be non-interrupting (cycle + interrupting is invalid)
                if let TimerSpec::Cycle { .. } = &spec {
                    if *interrupting {
                        errors.push(VerifyError {
                            message: format!(
                                "BoundaryTimer '{}': cycle timers must be non-interrupting (cancelActivity=\"false\")",
                                id
                            ),
                            element_id: Some(id.clone()),
                        });
                    }
                }

                // 7c. Must have at least one outgoing edge
                let outgoing = graph
                    .edges_directed(idx, petgraph::Direction::Outgoing)
                    .count();
                if outgoing == 0 {
                    errors.push(VerifyError {
                        message: format!("BoundaryTimer '{}' has no outgoing sequence flow", id),
                        element_id: Some(id.clone()),
                    });
                }

                host_boundary_count
                    .entry(attached_to.clone())
                    .or_default()
                    .push(id.clone());
            }
        }

        // 7d. Phase 2: max 1 boundary timer per host task
        for (host_id, boundary_ids) in &host_boundary_count {
            if boundary_ids.len() > 1 {
                errors.push(VerifyError {
                    message: format!(
                        "Task '{}' has {} boundary timers (max 1 supported in this version): [{}]",
                        host_id,
                        boundary_ids.len(),
                        boundary_ids.join(", ")
                    ),
                    element_id: Some(host_id.clone()),
                });
            }
        }
    }

    // 8. Boundary error event validation
    {
        // Track catch-all count per host task
        let mut host_catch_all_count: HashMap<String, Vec<String>> = HashMap::new();
        // Track specific-code count per (host task, error code) — a route
        // whose code never appears twice is unambiguous; a second
        // `BoundaryError` on the same host with the same code makes
        // `record.error_routes`'s runtime `.find()` (kernel `lib.rs`) match
        // only the first-armed one, leaving the second silently
        // unreachable rather than rejected — the same failure class 8d
        // already closes for catch-all, generalized to specific codes.
        let mut host_specific_code_ids: HashMap<(String, String), Vec<String>> = HashMap::new();

        for idx in graph.node_indices() {
            if let IRNode::BoundaryError {
                id,
                attached_to,
                error_code,
                failure_budget: _,
            } = &graph[idx]
            {
                // 8a. attached_to must reference an existing ServiceTask or
                // FfiServiceTask. `FfiServiceTask` was missing from this
                // check pre-existing this fix — same class of bug as 7a's
                // BoundaryTimer host-existence gap (fixed above): lowering
                // (`lowering.rs`) already fully supports a boundary error
                // attached to an FFI service task host, but no BPMN XML
                // could ever reach that code end-to-end, because this check
                // rejected the BoundaryError's own `attachedToRef` before
                // lowering ever ran.
                let host_exists = graph.node_indices().any(|other| {
                    matches!(&graph[other],
                        IRNode::ServiceTask { id: host_id, .. }
                        | IRNode::FfiServiceTask { id: host_id, .. }
                        if host_id == attached_to
                    )
                });
                if !host_exists {
                    errors.push(VerifyError {
                        message: format!(
                            "BoundaryError '{}' attachedToRef '{}' does not reference a task",
                            id, attached_to
                        ),
                        element_id: Some(id.clone()),
                    });
                }

                // 8b. Must have exactly 1 outgoing edge
                let outgoing = graph
                    .edges_directed(idx, petgraph::Direction::Outgoing)
                    .count();
                if outgoing != 1 {
                    errors.push(VerifyError {
                        message: format!(
                            "BoundaryError '{}' must have exactly 1 outgoing edge, found {}",
                            id, outgoing
                        ),
                        element_id: Some(id.clone()),
                    });
                }

                // 8c. Track catch-all (error_code: None) per host, and
                // specific codes per (host, code).
                match error_code {
                    None => {
                        host_catch_all_count
                            .entry(attached_to.clone())
                            .or_default()
                            .push(id.clone());
                    }
                    Some(code) => {
                        host_specific_code_ids
                            .entry((attached_to.clone(), code.clone()))
                            .or_default()
                            .push(id.clone());
                    }
                }
            }
        }

        // 8d. At most 1 catch-all BoundaryError per host task
        for (host_id, catch_all_ids) in &host_catch_all_count {
            if catch_all_ids.len() > 1 {
                errors.push(VerifyError {
                    message: format!(
                        "Task '{}' has {} catch-all error boundaries (max 1): [{}]",
                        host_id,
                        catch_all_ids.len(),
                        catch_all_ids.join(", ")
                    ),
                    element_id: Some(host_id.clone()),
                });
            }
        }

        // 8e. At most 1 BoundaryError per (host task, specific error code)
        for ((host_id, code), ids) in &host_specific_code_ids {
            if ids.len() > 1 {
                errors.push(VerifyError {
                    message: format!(
                        "Task '{}' has {} error boundaries for code '{}' (max 1): [{}]",
                        host_id,
                        ids.len(),
                        code,
                        ids.join(", ")
                    ),
                    element_id: Some(host_id.clone()),
                });
            }
        }
    }

    // 9. Inclusive gateway validation
    {
        for idx in graph.node_indices() {
            match &graph[idx] {
                IRNode::GatewayInclusive {
                    id,
                    direction: GatewayDirection::Diverging,
                    ..
                } => {
                    let outgoing = graph
                        .edges_directed(idx, petgraph::Direction::Outgoing)
                        .count();
                    if outgoing < 2 {
                        errors.push(VerifyError {
                            message: format!(
                                "Inclusive gateway (diverging) must have ≥2 outgoing edges, found {}",
                                outgoing
                            ),
                            element_id: Some(id.clone()),
                        });
                    }
                }
                IRNode::GatewayInclusive {
                    id,
                    direction: GatewayDirection::Converging,
                    ..
                } => {
                    let incoming = graph
                        .edges_directed(idx, petgraph::Direction::Incoming)
                        .count();
                    if incoming < 2 {
                        errors.push(VerifyError {
                            message: format!(
                                "Inclusive gateway (converging) must have ≥2 incoming edges, found {}",
                                incoming
                            ),
                            element_id: Some(id.clone()),
                        });
                    }
                    let outgoing = graph
                        .edges_directed(idx, petgraph::Direction::Outgoing)
                        .count();
                    if outgoing != 1 {
                        errors.push(VerifyError {
                            message: format!(
                                "Inclusive gateway (converging) must have exactly 1 outgoing edge, found {}",
                                outgoing
                            ),
                            element_id: Some(id.clone()),
                        });
                    }
                }
                _ => {}
            }
        }

        // Multiple inclusive-gateway pairs per process are now admitted.
        // History: this section used to reject `diverging_count > 1` /
        // `converging_count > 1` outright, because `lowering.rs`'s old
        // `inclusive_pairing_stack` paired fork↔join identity via a stack
        // popped in `lower()`'s BFS traversal order, not the graph's true
        // nesting structure — for two or more pairs, ANY topology where a
        // second pair's diverging node was discovered before the first
        // pair's converging node was reached (including one inclusive pair
        // nested in a single branch of another) popped the wrong
        // still-open diverging node and silently emitted a `BrIfNot`
        // header skipping to the wrong join address. Only fully sequential
        // pairs were known-safe, and nothing distinguished that from the
        // unsafe general case, so the blanket count rejection stood in as
        // the admission gate.
        //
        // Direction A (2026-07-24, `docs/todo/EOP-VS-BPMN-ISA-002.md` §19;
        // see also `EOP-PLAN-BPMN-ISA-002.md`'s "Direction A" post-close
        // entry) replaced the BFS-order stack with `lowering.rs`'s
        // `compute_gateway_pairing`/`gateway_pairing_dfs`/
        // `gateway_pairing_pop` — a DFS-recursive walk that clones the
        // fork-identity stack per branch before recursing (mirroring
        // `dsl::rpst::dfs_walk`), which derives fork↔join identity from the
        // graph's true nesting rather than discovery order, on one unified
        // kind-tagged stack covering both `GatewayAnd` and
        // `GatewayInclusive`. `check_gateway_and_nesting` (§4a above) is the
        // identical algorithm run on the admission side: it structurally
        // rejects any topology `compute_gateway_pairing` could mispair
        // (crossing gateway-kind boundaries, non-well-nested stack order,
        // and — via `GatewayClosureTracker` — a branch that dangles without
        // ever reaching its matching converging node), for any number of
        // `GatewayAnd`/`GatewayInclusive` pairs, sequential, nested, or
        // cross-kind-nested. §4a therefore now provides the real structural
        // guarantee this count check used to stand in for, so the blanket
        // "at most one pair" rejection is lifted — a well-nested N-pair
        // topology is admitted; a mispairing-prone topology is caught
        // structurally by §4a instead, with a precise diagnostic naming the
        // offending node.
    }

    errors
}

/// Which gateway kind a `check_gateway_and_nesting` stack entry belongs to.
/// Independent local type — not shared with `lowering.rs`'s
/// `GatewayPairKind` — since this module's admission-side nesting check and
/// `lowering.rs`'s pairing-DERIVATION are deliberately separate concerns
/// that happen to need the same kind-tagging discriminator; see this
/// function's doc comment and `lowering.rs`'s `compute_gateway_pairing` doc
/// comment for the shared framing rationale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GatewayKind {
    And,
    Inclusive,
}

impl std::fmt::Display for GatewayKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatewayKind::And => write!(f, "GatewayAnd"),
            GatewayKind::Inclusive => write!(f, "GatewayInclusive"),
        }
    }
}

/// Independent review finding (2026-07-24), confirmed by direct
/// reproduction: the clone-per-branch fix above (`fork_stack` cloned before
/// each recursion, never mutated on the caller's own copy) made the §4a
/// call site's `if !fork_stack.is_empty() { "Unclosed diverging gateway" }`
/// check permanently vacuous — that top-level variable can structurally
/// never receive a push anymore, for ANY graph, so it can never be
/// non-empty. This silently re-admitted a real defect the OLD (buggy,
/// mispairing) code used to catch as an accidental side effect: a diverging
/// gateway with a branch that dangles straight to `End` (or anywhere else)
/// without ever reaching a matching converging node. That topology is
/// genuinely non-well-nested — the corresponding `V2Fork` branch never
/// arrives at its barrier, hanging the join forever at runtime — and
/// nothing else in `verify()` catches it (§4's count check only sums global
/// diverging/converging counts, no per-pair reachability).
///
/// Fix: closure must be tracked per ORIGINAL BRANCH of the diverging node,
/// not merely "was it popped at least once anywhere" — a first cut at this
/// (closure as a plain per-id set) turned out to be too weak: a 2-branch
/// fork where only ONE branch reaches the join still records one successful
/// pop, which a plain "popped at least once" check accepts even though the
/// other branch dangled. Each diverging node's `push_entry` is therefore
/// tagged with a `branch_index` (its position among the node's own
/// immediate outgoing edges) — this tag is inherited unchanged through any
/// FURTHER nested splitting inside that branch (cloning preserves it), so a
/// pop anywhere downstream in that branch's subtree still credits the
/// correct original branch, however deeply nested. `all_diverging` records
/// each diverging node's id → its immediate out-degree (the branch count a
/// complete closure requires); `closed` records exactly which
/// `(id, branch_index)` pairs were ever actually popped. After the full
/// walk, a diverging node is fully closed iff `closed` contains EVERY
/// `branch_index` in `0..out_degree` for its id — computed at the §4a call
/// site, not here (this struct only accumulates raw facts from the walk).
#[derive(Default)]
struct GatewayClosureTracker {
    all_diverging: std::collections::HashMap<String, u32>,
    closed: std::collections::HashSet<(String, u32)>,
}

/// Structural well-nestedness (SESE) check for `GatewayAnd` AND
/// `GatewayInclusive` fork/join pairs (both kinds, tracked on one unified
/// kind-tagged stack — see the §4a call site's doc comment for why),
/// mirroring `dsl::rpst::verify_sese_nesting`'s DFS-second-visit algorithm:
/// entering a Diverging gateway pushes `(kind, id)`; the *second* (and
/// every later) time DFS reaches a node (i.e. `visited.insert` returns
/// `false`, meaning another path already explored it) is when a Converging
/// gateway's pairing is checked and popped — mirrored by the FIRST-visit
/// case too, when the first-reached node IS itself a Converging gateway
/// (see `check_gateway_nesting_pop`, called from both places). Unlike the
/// DSL's check, neither `IRNode::GatewayAnd` nor `IRNode::GatewayInclusive`
/// carries an explicit fork/join name reference to cross-validate against,
/// so this can only detect stack-order crossing, cross-kind crossing, and
/// unmatched joins structurally — not verify a join closes the *specific*
/// fork a BPMN author intended.
///
/// The stack is CLONED before recursing into each of a branch point's
/// (`>1` outgoing edge — `GatewayXor`/`GatewayAnd`/`GatewayInclusive`
/// diverging, or any future multi-successor node) neighbors, ported from
/// `dsl::rpst::dfs_walk`'s `Split` arm — this is the actual fix for the
/// defect this function used to have (a single stack threaded through the
/// whole recursion let one branch's pushes/pops leak into a sibling
/// branch's traversal).
#[allow(clippy::too_many_arguments)]
fn check_gateway_and_nesting(
    graph: &IRGraph,
    curr: NodeIndex,
    visited: &mut std::collections::HashSet<NodeIndex>,
    fork_stack: &mut Vec<(GatewayKind, String, u32)>,
    tracker: &mut GatewayClosureTracker,
    errors: &mut Vec<VerifyError>,
) {
    if !visited.insert(curr) {
        check_gateway_nesting_pop(graph, curr, fork_stack, tracker, errors);
        return;
    }

    // Is `curr` itself a Diverging gateway? If so, remember its kind/id —
    // `branch_index` (which of `curr`'s own immediate outgoing edges a given
    // clone descends from) is assigned per-neighbor below, not here; see
    // `GatewayClosureTracker`'s doc comment for why the tag must travel with
    // the branch, not the node.
    let diverging_kind_id = match &graph[curr] {
        IRNode::GatewayAnd {
            direction: GatewayDirection::Diverging,
            ..
        } => Some((GatewayKind::And, graph[curr].id().to_string())),
        IRNode::GatewayInclusive {
            direction: GatewayDirection::Diverging,
            ..
        } => Some((GatewayKind::Inclusive, graph[curr].id().to_string())),
        IRNode::GatewayAnd {
            direction: GatewayDirection::Converging,
            ..
        }
        | IRNode::GatewayInclusive {
            direction: GatewayDirection::Converging,
            ..
        } => {
            // First arrival at a converging node — pop-and-match now; the
            // recursion below still walks on to its own (single) successor.
            check_gateway_nesting_pop(graph, curr, fork_stack, tracker, errors);
            None
        }
        _ => None,
    };

    let neighbors: Vec<NodeIndex> = graph.neighbors(curr).collect();
    if diverging_kind_id.is_none() && neighbors.len() <= 1 {
        // `push_entry`: what a Diverging gateway would push — but, mirroring
        // `dsl::rpst::dfs_walk`'s `Split` arm precisely, it is pushed ONLY
        // into each branch's own CLONE below (the `else` arm), never into
        // the incoming `fork_stack` reference directly — this branch is
        // reached only when `curr` is NOT itself diverging, so there is
        // nothing to push here regardless.
        for neighbor in neighbors {
            check_gateway_and_nesting(graph, neighbor, visited, fork_stack, tracker, errors);
        }
    } else {
        if let Some((_, id)) = &diverging_kind_id {
            tracker
                .all_diverging
                .insert(id.clone(), neighbors.len() as u32);
        }
        for (branch_index, neighbor) in neighbors.iter().enumerate() {
            let mut branch_stack = fork_stack.clone();
            if let Some((kind, id)) = &diverging_kind_id {
                branch_stack.push((*kind, id.clone(), branch_index as u32));
            }
            check_gateway_and_nesting(graph, *neighbor, visited, &mut branch_stack, tracker, errors);
        }
    }
}

/// Pop the top-of-stack entry for a converging gateway `curr` reached on
/// one branch, and check it: `None` is an unmatched join; a kind mismatch
/// is a cross-kind crossing hazard (see `check_gateway_and_nesting`'s doc
/// comment); a matching kind records the popped diverging node's id in
/// `tracker.closed` (see `GatewayClosureTracker`'s doc comment) and is
/// otherwise silently accepted (this check cannot cross-validate the
/// *specific* fork/join identity, only kind-tagged stack order — see the
/// module-level doc comment above).
fn check_gateway_nesting_pop(
    graph: &IRGraph,
    curr: NodeIndex,
    fork_stack: &mut Vec<(GatewayKind, String, u32)>,
    tracker: &mut GatewayClosureTracker,
    errors: &mut Vec<VerifyError>,
) {
    let this_kind = match &graph[curr] {
        IRNode::GatewayAnd {
            direction: GatewayDirection::Converging,
            ..
        } => GatewayKind::And,
        IRNode::GatewayInclusive {
            direction: GatewayDirection::Converging,
            ..
        } => GatewayKind::Inclusive,
        _ => return,
    };
    match fork_stack.pop() {
        None => {
            errors.push(VerifyError {
                message: format!(
                    "Unmatched {this_kind} (converging): no open diverging {this_kind} \
                     found — non-well-nested gateway topology"
                ),
                element_id: Some(graph[curr].id().to_string()),
            });
        }
        Some((open_kind, open_id, _)) if open_kind != this_kind => {
            errors.push(VerifyError {
                message: format!(
                    "Crossing gateway-kind boundaries: {this_kind} (converging) '{}' \
                     reached while a nested {open_kind} '{}' is still open — \
                     non-well-nested gateway topology",
                    graph[curr].id(),
                    open_id
                ),
                element_id: Some(graph[curr].id().to_string()),
            });
        }
        Some((_, open_id, branch_index)) => {
            tracker.closed.insert((open_id, branch_index));
        }
    }
}

/// Verify bytecode for bounded-loop safety.
///
/// Rejects backward `Jump`/`BrIf`/`BrIfNot` (infinite loop risk).
/// Allows backward `BrCounterLt` (bounded by counter limit).
pub fn verify_bytecode(program: &CompiledProgram) -> Vec<VerifyError> {
    let mut errors = Vec::new();
    let program_len = Addr::new(program.program().len() as u32);
    for (addr, instr) in program.program().iter().enumerate() {
        let addr = Addr::new(addr as u32);
        match instr {
            Instr::Jump { target } | Instr::BrIf { target } | Instr::BrIfNot { target } => {
                check_target(&mut errors, program, addr, *target, program_len);
                if *target < addr {
                    errors.push(VerifyError {
                        message: format!(
                            "Backward jump at addr {} to {} — only BrCounterLt may jump backward",
                            addr, target
                        ),
                        element_id: program.debug_map().get(&addr).cloned(),
                    });
                }
            }
            Instr::BrCounterLt { target, .. } => {
                check_target(&mut errors, program, addr, *target, program_len);
                // BrCounterLt is allowed to jump backward (it's bounded by limit)
            }
            Instr::V2Fork { targets, .. } => {
                for target in targets.iter().copied() {
                    check_target(&mut errors, program, addr, target, program_len);
                }
            }
            _ => {}
        }
    }
    errors
}

fn check_target(
    errors: &mut Vec<VerifyError>,
    program: &CompiledProgram,
    addr: Addr,
    target: Addr,
    program_len: Addr,
) {
    if target >= program_len {
        errors.push(VerifyError {
            message: format!(
                "Bytecode target out of bounds at addr {}: target {} >= program len {}",
                addr, target, program_len
            ),
            element_id: program.debug_map().get(&addr).cloned(),
        });
    }
}

/// Verify and return Result — convenience wrapper.
pub fn verify_or_err(graph: &IRGraph) -> Result<()> {
    let errors = verify(graph);
    if errors.is_empty() {
        Ok(())
    } else {
        let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        Err(anyhow!("Verification failed:\n{}", msgs.join("\n")))
    }
}

/// Verify data-object declarations and variable-reference resolution in the IR.
///
/// Per A2 §11 / A5 scope. Checks:
///
/// 1. No duplicate data-object ids.
/// 2. Every `Expression::VarRef` in `FfiServiceTask` input bindings resolves
///    to a declared data-object id.
/// 3. Every `FfiOutputBinding.target_variable` resolves to a declared
///    data-object id.
///
/// The verifier for FFI schema compatibility against the FFI catalogue is
/// `verify_ffi_schemas` (A6 — not yet implemented; requires catalogue access).
pub fn verify_data_objects(graph: &IRGraph) -> Vec<VerifyError> {
    let mut errors = Vec::new();

    // Collect declared data objects (id → node).
    let mut declared: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for idx in graph.node_indices() {
        if let IRNode::DataObject { id, .. } = &graph[idx] {
            if declared.insert(id.clone(), id.clone()).is_some() {
                errors.push(VerifyError {
                    message: format!("duplicate data-object id: '{}'", id),
                    element_id: Some(id.clone()),
                });
            }
        }
    }

    // Check all FfiServiceTask bindings resolve.
    for idx in graph.node_indices() {
        if let IRNode::FfiServiceTask {
            id,
            inputs,
            outputs,
            ..
        } = &graph[idx]
        {
            for binding in inputs {
                if let crate::ir::Expression::VarRef(path) = &binding.expression {
                    let first = path.first().map(|s| s.as_str()).unwrap_or("");
                    if !declared.contains_key(first) {
                        errors.push(VerifyError {
                            message: format!(
                                "unresolved input var ref '{}' in task '{}': \
                                 no data object with id '{}'",
                                path.join("."),
                                id,
                                first
                            ),
                            element_id: Some(id.clone()),
                        });
                    }
                }
            }
            for binding in outputs {
                if !declared.contains_key(&binding.target_variable) {
                    errors.push(VerifyError {
                        message: format!(
                            "unresolved output target '{}' in task '{}': \
                             no data object with id '{}'",
                            binding.target_variable, id, binding.target_variable
                        ),
                        element_id: Some(id.clone()),
                    });
                }
            }
        }
    }

    errors
}

/// Verify FFI task schema bindings against the FFI catalogue.
///
/// Per A2 §11. Called by the compiler after `verify_bytecode` succeeds, when
/// a catalogue snapshot is available. Can also be called independently in
/// tooling contexts (LSP, CI lint).
///
/// Produces structured `VerifyError` items for:
/// - Unknown template id
/// - Unknown input/output field names
/// - Type-incompatible input bindings
/// - Required inputs that are not bound
/// - Output bindings that target a `FlagWrite` with a kind that doesn't fit
///   in `bpmn_lite_types::Value` (non-Bool, non-I64)
#[cfg(test)]
mod tests {
    use super::*;

    /// A4.T5: Verifier rejects graph with no StartEvent
    #[test]
    fn test_no_start_event() {
        let mut graph = IRGraph::new();
        graph.add_node(IRNode::End {
            id: "end1".to_string(),
            terminate: false,
        });

        let errors = verify(&graph);
        assert!(errors.iter().any(|e| e.message.contains("No StartEvent")));
    }

    /// Independent review finding (2026-07-24): the branch-clone fix that
    /// stopped `check_gateway_and_nesting`'s false rejection of legal
    /// nested topology (see `test_two_nested_and_pairs_in_and_branches_now_
    /// admitted`) also made the old `fork_stack.is_empty()` "unclosed
    /// diverging gateway" check permanently vacuous — the top-level
    /// variable can never receive a push anymore. That check used to catch
    /// (as an accidental side effect of the old, buggy code) a genuinely
    /// non-well-nested topology: a fork with one branch that dangles
    /// straight to `End` without ever reaching its own join. Confirmed by
    /// direct reproduction (`cargo test` against the pre-fix code returned
    /// zero errors for this exact graph). Fixed via `GatewayClosureTracker`
    /// (branch-index-tagged closure, not a plain per-id set — a first cut
    /// using a plain set was still too weak: it accepted this exact
    /// topology too, since ONE of `fork1`'s two branches does successfully
    /// reach `join1`, which a plain "popped at least once" check treats as
    /// sufficient even though the sibling branch never arrives).
    #[test]
    fn test_dangling_and_branch_rejected() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let fork1 = graph.add_node(IRNode::GatewayAnd {
            id: "fork1".to_string(), name: "Fork1".to_string(), direction: GatewayDirection::Diverging,
        });
        let join1 = graph.add_node(IRNode::GatewayAnd {
            id: "join1".to_string(), name: "Join1".to_string(), direction: GatewayDirection::Converging,
        });
        let a = graph.add_node(IRNode::ServiceTask { id: "a".to_string(), name: "A".to_string(), task_type: "a".to_string() });
        let end_dangle = graph.add_node(IRNode::End { id: "end_dangle".to_string(), terminate: false });
        let fork2 = graph.add_node(IRNode::GatewayAnd {
            id: "fork2".to_string(), name: "Fork2".to_string(), direction: GatewayDirection::Diverging,
        });
        let join2 = graph.add_node(IRNode::GatewayAnd {
            id: "join2".to_string(), name: "Join2".to_string(), direction: GatewayDirection::Converging,
        });
        let c1 = graph.add_node(IRNode::ServiceTask { id: "c1".to_string(), name: "C1".to_string(), task_type: "c1".to_string() });
        let c2 = graph.add_node(IRNode::ServiceTask { id: "c2".to_string(), name: "C2".to_string(), task_type: "c2".to_string() });
        let end = graph.add_node(IRNode::End { id: "end".to_string(), terminate: false });

        graph.add_edge(start, fork1, IREdge { id: "e0".to_string(), condition: None });
        graph.add_edge(fork1, a, IREdge { id: "e1".to_string(), condition: None });
        graph.add_edge(a, join1, IREdge { id: "e2".to_string(), condition: None });
        graph.add_edge(fork1, end_dangle, IREdge { id: "e3".to_string(), condition: None }); // dangling branch: never reaches join1
        graph.add_edge(join1, fork2, IREdge { id: "e4".to_string(), condition: None });
        graph.add_edge(fork2, c1, IREdge { id: "e5".to_string(), condition: None });
        graph.add_edge(fork2, c2, IREdge { id: "e6".to_string(), condition: None });
        graph.add_edge(c1, join2, IREdge { id: "e7".to_string(), condition: None });
        graph.add_edge(c2, join2, IREdge { id: "e8".to_string(), condition: None });
        graph.add_edge(join2, end, IREdge { id: "e9".to_string(), condition: None });

        let errors = verify(&graph);
        assert!(
            errors.iter().any(|e| e.message.contains("Unclosed diverging gateway") && e.message.contains("fork1")),
            "dangling branch (fork1's second branch never reaches join1) must be rejected, got: {errors:?}"
        );
    }

    /// Independent-review finding (2026-07-24), against `lowering.rs`'s
    /// dominance-based `compute_post_dominators`: nothing in this pipeline
    /// previously rejected a raw cyclic `IRGraph` — a BPMN sequence-flow
    /// back edge with no `MultiInstance`/`GUARD-TIMER-CYCLE>` involved
    /// would parse and reach `lower()` unrejected, where the dominance
    /// computation's single-pass (no fixed-point iteration) reverse-
    /// postorder algorithm — valid only on a DAG — silently returns a
    /// WRONG real node, not a safe absence, for the affected diverging
    /// node (confirmed by the reviewer via direct construction of the
    /// same topology this test uses). Per CLAUDE.md's settled decisions
    /// ("SESE only," "loops are finitely bounded"), a raw back edge is
    /// always illegitimate input for this pipeline, not merely untested —
    /// rejected here structurally, closing the gap rather than leaving it
    /// as an unenforced doc-comment assumption.
    ///
    /// Topology: `b` diverges to `p` and `q`; `p -> m1 -> end1`; `q -> a`,
    /// and `a` diverges to a back edge to `b` AND to `m2 -> end2` — a
    /// genuine cycle (`b -> q -> a -> b`) with no bounded-loop construct
    /// anywhere in sight.
    #[test]
    fn test_cyclic_graph_rejected() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let b = graph.add_node(IRNode::GatewayXor { id: "b".to_string(), name: "B".to_string() });
        let p = graph.add_node(IRNode::ServiceTask { id: "p".to_string(), name: "P".to_string(), task_type: "p".to_string() });
        let q = graph.add_node(IRNode::ServiceTask { id: "q".to_string(), name: "Q".to_string(), task_type: "q".to_string() });
        let m1 = graph.add_node(IRNode::ServiceTask { id: "m1".to_string(), name: "M1".to_string(), task_type: "m1".to_string() });
        let end1 = graph.add_node(IRNode::End { id: "end1".to_string(), terminate: false });
        let a = graph.add_node(IRNode::GatewayXor { id: "a".to_string(), name: "A".to_string() });
        let m2 = graph.add_node(IRNode::ServiceTask { id: "m2".to_string(), name: "M2".to_string(), task_type: "m2".to_string() });
        let end2 = graph.add_node(IRNode::End { id: "end2".to_string(), terminate: false });

        graph.add_edge(start, b, IREdge { id: "e0".to_string(), condition: None });
        graph.add_edge(b, p, IREdge { id: "e1".to_string(), condition: None });
        graph.add_edge(b, q, IREdge { id: "e2".to_string(), condition: None });
        graph.add_edge(p, m1, IREdge { id: "e3".to_string(), condition: None });
        graph.add_edge(m1, end1, IREdge { id: "e4".to_string(), condition: None });
        graph.add_edge(q, a, IREdge { id: "e5".to_string(), condition: None });
        graph.add_edge(a, b, IREdge { id: "e6_back_edge".to_string(), condition: None });
        graph.add_edge(a, m2, IREdge { id: "e7".to_string(), condition: None });
        graph.add_edge(m2, end2, IREdge { id: "e8".to_string(), condition: None });

        let errors = verify(&graph);
        assert!(
            errors.iter().any(|e| e.message.contains("cycle")),
            "a raw sequence-flow back edge must be rejected structurally, got: {errors:?}"
        );
    }

    /// A4.T6: Verifier rejects unstructured parallel gateway
    #[test]
    fn test_unmatched_parallel_gateways() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let fork = graph.add_node(IRNode::GatewayAnd {
            id: "fork1".to_string(),
            name: "Fork".to_string(),
            direction: GatewayDirection::Diverging,
        });
        let end = graph.add_node(IRNode::End {
            id: "end1".to_string(),
            terminate: false,
        });

        graph.add_edge(
            start,
            fork,
            IREdge {
                id: "f1".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            fork,
            end,
            IREdge {
                id: "f2".to_string(),
                condition: None,
            },
        );

        let errors = verify(&graph);
        assert!(errors
            .iter()
            .any(|e| e.message.contains("Mismatched parallel gateways")));
    }

    /// V5.1 correction (2026-07-22): equal fork/join counts do not imply
    /// well-nested topology — the count check alone would admit this.
    /// A Converging `GatewayAnd` appears *before* its Diverging counterpart
    /// in the graph (1 fork, 1 join, but out of nesting order), which the
    /// XML->V2Fork/V2Join lowering pre-pass depends on never happening.
    #[test]
    fn test_out_of_order_gateway_and_rejected_despite_matching_counts() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let join_first = graph.add_node(IRNode::GatewayAnd {
            id: "join_first".to_string(),
            name: "Join".to_string(),
            direction: GatewayDirection::Converging,
        });
        let fork_later = graph.add_node(IRNode::GatewayAnd {
            id: "fork_later".to_string(),
            name: "Fork".to_string(),
            direction: GatewayDirection::Diverging,
        });
        let end1 = graph.add_node(IRNode::End {
            id: "end1".to_string(),
            terminate: false,
        });
        let end2 = graph.add_node(IRNode::End {
            id: "end2".to_string(),
            terminate: false,
        });

        graph.add_edge(
            start,
            join_first,
            IREdge {
                id: "f1".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            join_first,
            fork_later,
            IREdge {
                id: "f2".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            fork_later,
            end1,
            IREdge {
                id: "f3".to_string(),
                condition: None,
            },
        );
        graph.add_edge(
            fork_later,
            end2,
            IREdge {
                id: "f4".to_string(),
                condition: None,
            },
        );

        let errors = verify(&graph);
        // The old count-only check (§4) would find 1 fork, 1 join — equal,
        // no error. The structural check (§4a) must still reject it.
        assert!(
            !errors.iter().any(|e| e.message.contains("Mismatched parallel gateways")),
            "counts are equal — the count check alone must not fire here"
        );
        // V6 (2026-07-24): updated expected message. The OLD (buggy)
        // `check_gateway_and_nesting` mutated the CALLER's own stack
        // directly when pushing a diverging gateway's entry (rather than
        // only the per-branch clones), so `fork_later`'s push leaked back
        // up to the top-level "unclosed" check even though its own two
        // branches (`end1`/`end2`) each terminate independently without
        // ever reconverging — producing "Unclosed diverging GatewayAnd".
        // The fixed version pushes only into branch-local clones (mirroring
        // `dsl::rpst::dfs_walk`'s `Split` arm exactly, including that
        // model's own same limitation: a diverging node whose branches
        // never reconverge is not itself flagged "unclosed" by the
        // top-level check). The out-of-order pair here is still correctly
        // rejected — just via the MORE PRECISE error raised the moment
        // `join_first` is reached with nothing open on its own branch's
        // stack ("Unmatched GatewayAnd (converging)"), not a vague
        // end-of-walk "unclosed" list.
        assert!(
            errors.iter().any(|e| e.message.contains("Unmatched GatewayAnd (converging)")),
            "the structural nesting check must catch the out-of-order pair, got: {errors:?}"
        );
    }

    // ═══════════════════════════════════════════════════════════
    //  GUARD-TIMER-CYCLE> frontend wiring — §7b interrupting+Cycle
    //  rejection (this rule already existed here before the frontend
    //  wiring landed; these are its first tests).
    // ═══════════════════════════════════════════════════════════

    /// Build a minimal host-task + boundary-timer graph, parameterized on
    /// `interrupting`/`spec` — mirrors `lowering.rs`'s own
    /// `make_boundary_timer_graph` test helper (kept separate rather than
    /// shared across crate-private test modules).
    fn make_boundary_timer_verify_graph(interrupting: bool, spec: TimerSpec) -> IRGraph {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start {
            id: "start".to_string(),
        });
        let host = graph.add_node(IRNode::ServiceTask {
            id: "host".to_string(),
            name: "Host".to_string(),
            task_type: "long_work".to_string(),
        });
        let normal_end = graph.add_node(IRNode::End {
            id: "normal_end".to_string(),
            terminate: false,
        });
        let boundary = graph.add_node(IRNode::BoundaryTimer {
            id: "timeout".to_string(),
            attached_to: "host".to_string(),
            spec,
            interrupting,
            failure_budget: None,
        });
        let escalate = graph.add_node(IRNode::ServiceTask {
            id: "escalate".to_string(),
            name: "Escalate".to_string(),
            task_type: "escalate_work".to_string(),
        });
        let timeout_end = graph.add_node(IRNode::End {
            id: "timeout_end".to_string(),
            terminate: false,
        });

        graph.add_edge(start, host, IREdge { id: "f1".to_string(), condition: None });
        graph.add_edge(host, normal_end, IREdge { id: "f2".to_string(), condition: None });
        graph.add_edge(boundary, escalate, IREdge { id: "f3".to_string(), condition: None });
        graph.add_edge(escalate, timeout_end, IREdge { id: "f4".to_string(), condition: None });

        graph
    }

    /// §7b: an interrupting (`cancelActivity="true"`) boundary timer with
    /// a `timeCycle` spec is rejected — an interrupting timer fires at
    /// most once (it kills the host scope on trigger), so "repeat N
    /// times, killing the scope" has no coherent meaning. This is the gate
    /// the `GUARD-TIMER-CYCLE>` frontend-wiring fix relies on to guarantee
    /// `lower_boundary_guarded_task_v2`/the `FfiServiceTask` boundary-timer
    /// arm never see an interrupting + `Cycle` combination — a hard
    /// compile-time rejection, not a silent single-fire fallback.
    #[test]
    fn test_interrupting_boundary_timer_with_cycle_rejected() {
        let graph = make_boundary_timer_verify_graph(
            true,
            TimerSpec::Cycle {
                interval_ms: 3_600_000,
                max_fires: 3,
            },
        );
        let errors = verify(&graph);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("cycle timers must be non-interrupting")),
            "expected §7b rejection, got: {errors:?}"
        );
    }

    /// Same interrupting + `Cycle` graph, through the full
    /// `Compiler::lower` pipeline (`verify_or_err` + `lowering::lower`),
    /// proving the rejection actually gates compilation end-to-end, not
    /// merely the standalone `verify()` accumulator function above.
    #[test]
    fn test_interrupting_cycle_rejected_by_full_compiler_lower() {
        let graph = make_boundary_timer_verify_graph(
            true,
            TimerSpec::Cycle {
                interval_ms: 3_600_000,
                max_fires: 3,
            },
        );
        let err = crate::Compiler::lower(&graph)
            .expect_err("interrupting + Cycle boundary timer must be rejected");
        assert!(
            err.to_string().contains("cycle timers must be non-interrupting"),
            "expected §7b rejection in error, got: {err}"
        );
    }

    /// Sanity counterpart: non-interrupting + `Cycle` is legal — §7b only
    /// forbids the interrupting combination.
    #[test]
    fn test_non_interrupting_boundary_timer_with_cycle_admitted() {
        let graph = make_boundary_timer_verify_graph(
            false,
            TimerSpec::Cycle {
                interval_ms: 3_600_000,
                max_fires: 3,
            },
        );
        let errors = verify(&graph);
        assert!(
            !errors
                .iter()
                .any(|e| e.message.contains("cycle timers must be non-interrupting")),
            "non-interrupting + Cycle must not trigger §7b, got: {errors:?}"
        );
    }

    fn make_boundary_error_verify_graph(host: IRNode) -> IRGraph {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let host = graph.add_node(host);
        let normal_end = graph.add_node(IRNode::End { id: "normal_end".to_string(), terminate: false });
        let boundary = graph.add_node(IRNode::BoundaryError {
            id: "catch".to_string(),
            attached_to: "host".to_string(),
            error_code: Some("SOME_ERROR".to_string()),
            failure_budget: None,
        });
        let escalate = graph.add_node(IRNode::ServiceTask {
            id: "escalate".to_string(),
            name: "Escalate".to_string(),
            task_type: "escalate_work".to_string(),
        });
        let escalate_end = graph.add_node(IRNode::End { id: "escalate_end".to_string(), terminate: false });

        graph.add_edge(start, host, IREdge { id: "f1".to_string(), condition: None });
        graph.add_edge(host, normal_end, IREdge { id: "f2".to_string(), condition: None });
        graph.add_edge(boundary, escalate, IREdge { id: "f3".to_string(), condition: None });
        graph.add_edge(escalate, escalate_end, IREdge { id: "f4".to_string(), condition: None });

        graph
    }

    /// 8a bug fix: a boundary error attached to an `FfiServiceTask` host
    /// must NOT be rejected — this check previously only matched
    /// `IRNode::ServiceTask`, the same class of omission as the sibling
    /// BoundaryTimer 7a fix (a boundary error attached to an FFI task was
    /// fully lowerable but could never compile from XML because this check
    /// rejected its `attachedToRef` first).
    #[test]
    fn test_boundary_error_attached_to_ffi_service_task_is_admitted() {
        let graph = make_boundary_error_verify_graph(IRNode::FfiServiceTask {
            id: "host".to_string(),
            name: "Host".to_string(),
            template_id: [3u8; 32],
            inputs: vec![],
            outputs: vec![],
        });
        let errors = verify(&graph);
        assert!(
            !errors
                .iter()
                .any(|e| e.message.contains("does not reference a task")),
            "BoundaryError attached to FfiServiceTask must be admitted, got: {errors:?}"
        );
    }

    /// Sanity counterpart: a boundary error attached to a plain
    /// `ServiceTask` host remains admitted (unchanged behavior).
    #[test]
    fn test_boundary_error_attached_to_service_task_is_admitted() {
        let graph = make_boundary_error_verify_graph(IRNode::ServiceTask {
            id: "host".to_string(),
            name: "Host".to_string(),
            task_type: "long_work".to_string(),
        });
        let errors = verify(&graph);
        assert!(
            !errors
                .iter()
                .any(|e| e.message.contains("does not reference a task")),
            "BoundaryError attached to ServiceTask must be admitted, got: {errors:?}"
        );
    }

    /// Negative counterpart: a boundary error whose `attachedToRef` names
    /// no task at all (neither ServiceTask nor FfiServiceTask) is still
    /// correctly rejected — the fix widens the accepted host set, it does
    /// not disable the check.
    #[test]
    fn test_boundary_error_attached_to_nonexistent_host_is_rejected() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let boundary = graph.add_node(IRNode::BoundaryError {
            id: "catch".to_string(),
            attached_to: "no_such_host".to_string(),
            error_code: None,
            failure_budget: None,
        });
        let end = graph.add_node(IRNode::End { id: "end".to_string(), terminate: false });
        graph.add_edge(start, end, IREdge { id: "f1".to_string(), condition: None });
        graph.add_edge(boundary, end, IREdge { id: "f2".to_string(), condition: None });

        let errors = verify(&graph);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("does not reference a task")),
            "BoundaryError attached to a nonexistent host must still be rejected, got: {errors:?}"
        );
    }

    /// 8e: two `BoundaryError` nodes on the same host with the SAME
    /// specific error code compile and pair edges cleanly, but at runtime
    /// `record.error_routes`'s `.find()` (kernel `apply_job_failure`) only
    /// ever matches the first-armed route — the second is silently
    /// unreachable, not rejected. Must be caught here, not discovered at
    /// runtime.
    #[test]
    fn test_boundary_error_duplicate_specific_code_on_same_host_is_rejected() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let host = graph.add_node(IRNode::ServiceTask {
            id: "host".to_string(),
            name: "Host".to_string(),
            task_type: "long_work".to_string(),
        });
        let normal_end = graph.add_node(IRNode::End { id: "normal_end".to_string(), terminate: false });
        let boundary_a = graph.add_node(IRNode::BoundaryError {
            id: "catch_a".to_string(),
            attached_to: "host".to_string(),
            error_code: Some("SANCTIONS_HIT".to_string()),
            failure_budget: None,
        });
        let boundary_b = graph.add_node(IRNode::BoundaryError {
            id: "catch_b".to_string(),
            attached_to: "host".to_string(),
            error_code: Some("SANCTIONS_HIT".to_string()),
            failure_budget: None,
        });
        let handler_a = graph.add_node(IRNode::ServiceTask {
            id: "handler_a".to_string(),
            name: "Handler A".to_string(),
            task_type: "handle_a".to_string(),
        });
        let handler_b = graph.add_node(IRNode::ServiceTask {
            id: "handler_b".to_string(),
            name: "Handler B".to_string(),
            task_type: "handle_b".to_string(),
        });
        let end_a = graph.add_node(IRNode::End { id: "end_a".to_string(), terminate: false });
        let end_b = graph.add_node(IRNode::End { id: "end_b".to_string(), terminate: false });

        graph.add_edge(start, host, IREdge { id: "f1".to_string(), condition: None });
        graph.add_edge(host, normal_end, IREdge { id: "f2".to_string(), condition: None });
        graph.add_edge(boundary_a, handler_a, IREdge { id: "f3".to_string(), condition: None });
        graph.add_edge(handler_a, end_a, IREdge { id: "f4".to_string(), condition: None });
        graph.add_edge(boundary_b, handler_b, IREdge { id: "f5".to_string(), condition: None });
        graph.add_edge(handler_b, end_b, IREdge { id: "f6".to_string(), condition: None });

        let errors = verify(&graph);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("SANCTIONS_HIT") && e.message.contains("max 1")),
            "two BoundaryError nodes on one host with the same specific code must be rejected, got: {errors:?}"
        );
    }

    /// Sanity counterpart: two `BoundaryError` nodes on the same host with
    /// DIFFERENT specific codes remain admitted — 8e must not overreach
    /// into rejecting the legitimate multi-arm case this migration exists
    /// to support.
    #[test]
    fn test_boundary_error_distinct_specific_codes_on_same_host_is_admitted() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let host = graph.add_node(IRNode::ServiceTask {
            id: "host".to_string(),
            name: "Host".to_string(),
            task_type: "long_work".to_string(),
        });
        let normal_end = graph.add_node(IRNode::End { id: "normal_end".to_string(), terminate: false });
        let boundary_a = graph.add_node(IRNode::BoundaryError {
            id: "catch_a".to_string(),
            attached_to: "host".to_string(),
            error_code: Some("SANCTIONS_HIT".to_string()),
            failure_budget: None,
        });
        let boundary_b = graph.add_node(IRNode::BoundaryError {
            id: "catch_b".to_string(),
            attached_to: "host".to_string(),
            error_code: Some("TIMEOUT".to_string()),
            failure_budget: None,
        });
        let handler_a = graph.add_node(IRNode::ServiceTask {
            id: "handler_a".to_string(),
            name: "Handler A".to_string(),
            task_type: "handle_a".to_string(),
        });
        let handler_b = graph.add_node(IRNode::ServiceTask {
            id: "handler_b".to_string(),
            name: "Handler B".to_string(),
            task_type: "handle_b".to_string(),
        });
        let end_a = graph.add_node(IRNode::End { id: "end_a".to_string(), terminate: false });
        let end_b = graph.add_node(IRNode::End { id: "end_b".to_string(), terminate: false });

        graph.add_edge(start, host, IREdge { id: "f1".to_string(), condition: None });
        graph.add_edge(host, normal_end, IREdge { id: "f2".to_string(), condition: None });
        graph.add_edge(boundary_a, handler_a, IREdge { id: "f3".to_string(), condition: None });
        graph.add_edge(handler_a, end_a, IREdge { id: "f4".to_string(), condition: None });
        graph.add_edge(boundary_b, handler_b, IREdge { id: "f5".to_string(), condition: None });
        graph.add_edge(handler_b, end_b, IREdge { id: "f6".to_string(), condition: None });

        let errors = verify(&graph);
        assert!(
            !errors.iter().any(|e| e.message.contains("max 1")),
            "two BoundaryError nodes on one host with distinct codes must be admitted, got: {errors:?}"
        );
    }

    // ═══════════════════════════════════════════════════════════
    //  V&S Q-inclusive-multi (investigated 2026-07-23, LIFTED 2026-07-24):
    //  this section used to lock the count-based §9 rejection to the
    //  specific mispairing hazards it guarded against (see
    //  `lowering.rs`'s `test_two_sequential_inclusive_pairs_lower_
    //  correctly` doc comment and this module's §9 doc comment for the
    //  full writeup). Direction A (`docs/todo/EOP-VS-BPMN-ISA-002.md` §19)
    //  replaced the mispairing-prone BFS-order pairing stack with a
    //  DFS-recursive, clone-the-stack-per-branch mechanism
    //  (`lowering.rs`'s `compute_gateway_pairing`) and extended this
    //  module's `check_gateway_and_nesting` (§4a) to structurally validate
    //  `GatewayInclusive` nesting the same way it already validated
    //  `GatewayAnd` nesting — both graphs below are genuinely well-nested
    //  (SESE) topology, so both must now be ADMITTED, mirroring
    //  `test_two_nested_and_pairs_in_and_branches_now_admitted` below for
    //  the `GatewayAnd` case.
    // ═══════════════════════════════════════════════════════════

    /// Two `GatewayInclusive` pairs, one nested inside EACH branch of a
    /// `GatewayAnd` fork (branch B deliberately longer than branch A, which
    /// used to skew `lower()`'s BFS discovery order under the old pairing
    /// mechanism). `compute_gateway_pairing`'s DFS-recursive, per-branch-
    /// cloned stack derives each inner pair's fork↔join identity correctly
    /// regardless of branch length; `check_gateway_and_nesting` admits this
    /// topology structurally. Must now be admitted, not rejected.
    #[test]
    fn test_two_nested_inclusive_pairs_in_and_branches_now_admitted() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let and_fork = graph.add_node(IRNode::GatewayAnd {
            id: "and_fork".to_string(), name: "AndFork".to_string(), direction: GatewayDirection::Diverging,
        });
        let and_join = graph.add_node(IRNode::GatewayAnd {
            id: "and_join".to_string(), name: "AndJoin".to_string(), direction: GatewayDirection::Converging,
        });
        let end = graph.add_node(IRNode::End { id: "end".to_string(), terminate: false });

        let ig_fork_a = graph.add_node(IRNode::GatewayInclusive {
            id: "ig_fork_a".to_string(), name: "ForkA".to_string(), direction: GatewayDirection::Diverging,
        });
        let ig_join_a = graph.add_node(IRNode::GatewayInclusive {
            id: "ig_join_a".to_string(), name: "JoinA".to_string(), direction: GatewayDirection::Converging,
        });
        let a1 = graph.add_node(IRNode::ServiceTask { id: "a1".to_string(), name: "A1".to_string(), task_type: "a1".to_string() });
        let a2 = graph.add_node(IRNode::ServiceTask { id: "a2".to_string(), name: "A2".to_string(), task_type: "a2".to_string() });

        let ig_fork_b = graph.add_node(IRNode::GatewayInclusive {
            id: "ig_fork_b".to_string(), name: "ForkB".to_string(), direction: GatewayDirection::Diverging,
        });
        let ig_join_b = graph.add_node(IRNode::GatewayInclusive {
            id: "ig_join_b".to_string(), name: "JoinB".to_string(), direction: GatewayDirection::Converging,
        });
        let b1 = graph.add_node(IRNode::ServiceTask { id: "b1".to_string(), name: "B1".to_string(), task_type: "b1".to_string() });
        let b2 = graph.add_node(IRNode::ServiceTask { id: "b2".to_string(), name: "B2".to_string(), task_type: "b2".to_string() });
        let b3 = graph.add_node(IRNode::ServiceTask { id: "b3".to_string(), name: "B3".to_string(), task_type: "b3".to_string() });
        let b_pre = graph.add_node(IRNode::ServiceTask { id: "b_pre".to_string(), name: "BPre".to_string(), task_type: "b_pre".to_string() });

        graph.add_edge(start, and_fork, IREdge { id: "f0".to_string(), condition: None });
        graph.add_edge(and_fork, ig_fork_a, IREdge { id: "fa0".to_string(), condition: None });
        graph.add_edge(ig_fork_a, a1, IREdge { id: "fa1".to_string(), condition: None });
        graph.add_edge(ig_fork_a, a2, IREdge { id: "fa2".to_string(), condition: None });
        graph.add_edge(a1, ig_join_a, IREdge { id: "fa3".to_string(), condition: None });
        graph.add_edge(a2, ig_join_a, IREdge { id: "fa4".to_string(), condition: None });
        graph.add_edge(ig_join_a, and_join, IREdge { id: "fa5".to_string(), condition: None });
        graph.add_edge(and_fork, b_pre, IREdge { id: "fb_pre".to_string(), condition: None });
        graph.add_edge(b_pre, ig_fork_b, IREdge { id: "fb0".to_string(), condition: None });
        graph.add_edge(ig_fork_b, b1, IREdge { id: "fb1".to_string(), condition: None });
        graph.add_edge(ig_fork_b, b2, IREdge { id: "fb2".to_string(), condition: None });
        graph.add_edge(ig_fork_b, b3, IREdge { id: "fb3".to_string(), condition: None });
        graph.add_edge(b1, ig_join_b, IREdge { id: "fb4".to_string(), condition: None });
        graph.add_edge(b2, ig_join_b, IREdge { id: "fb5".to_string(), condition: None });
        graph.add_edge(b3, ig_join_b, IREdge { id: "fb6".to_string(), condition: None });
        graph.add_edge(ig_join_b, and_join, IREdge { id: "fb7".to_string(), condition: None });
        graph.add_edge(and_join, end, IREdge { id: "fend".to_string(), condition: None });

        let errors = verify(&graph);
        assert!(
            !errors.iter().any(|e| e.message.contains("Unmatched GatewayInclusive")
                || e.message.contains("Unclosed diverging gateway")
                || e.message.contains("Crossing gateway-kind boundaries")),
            "two independently-nested inclusive pairs must now be admitted, got: {errors:?}"
        );
    }

    /// A `GatewayInclusive` pair nested inside ONE branch of an outer
    /// `GatewayInclusive` pair, with the outer's OTHER branch a plain leaf
    /// task. Used to mispair under the old BFS-order stack even with no
    /// sibling `GatewayAnd` involved — the outer pair's own shorter leaf
    /// branch alone was enough to skew BFS order ahead of the nested
    /// pair's join. `compute_gateway_pairing`'s DFS-recursive walk is
    /// immune to branch-length skew; this is genuinely well-nested (true
    /// nested-OR, not merely OR-inside-AND) and must now be admitted.
    #[test]
    fn test_inclusive_pair_nested_in_single_outer_branch_now_admitted() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let outer_fork = graph.add_node(IRNode::GatewayInclusive {
            id: "outer_fork".to_string(), name: "OuterFork".to_string(), direction: GatewayDirection::Diverging,
        });
        let outer_join = graph.add_node(IRNode::GatewayInclusive {
            id: "outer_join".to_string(), name: "OuterJoin".to_string(), direction: GatewayDirection::Converging,
        });
        let end = graph.add_node(IRNode::End { id: "end".to_string(), terminate: false });
        let leaf = graph.add_node(IRNode::ServiceTask { id: "leaf".to_string(), name: "Leaf".to_string(), task_type: "leaf".to_string() });
        let inner_fork = graph.add_node(IRNode::GatewayInclusive {
            id: "inner_fork".to_string(), name: "InnerFork".to_string(), direction: GatewayDirection::Diverging,
        });
        let inner_join = graph.add_node(IRNode::GatewayInclusive {
            id: "inner_join".to_string(), name: "InnerJoin".to_string(), direction: GatewayDirection::Converging,
        });
        let c1 = graph.add_node(IRNode::ServiceTask { id: "c1".to_string(), name: "C1".to_string(), task_type: "c1".to_string() });
        let c2 = graph.add_node(IRNode::ServiceTask { id: "c2".to_string(), name: "C2".to_string(), task_type: "c2".to_string() });

        let cond = |flag: &str| Some(ConditionExpr { flag_name: flag.to_string(), op: ConditionOp::Eq, literal: ConditionLiteral::Bool(true) });

        graph.add_edge(start, outer_fork, IREdge { id: "f0".to_string(), condition: None });
        graph.add_edge(outer_fork, leaf, IREdge { id: "f1".to_string(), condition: cond("flag_leaf") });
        graph.add_edge(leaf, outer_join, IREdge { id: "f2".to_string(), condition: None });
        graph.add_edge(outer_fork, inner_fork, IREdge { id: "f3".to_string(), condition: cond("flag_inner") });
        graph.add_edge(inner_fork, c1, IREdge { id: "f4".to_string(), condition: cond("flag_c1") });
        graph.add_edge(inner_fork, c2, IREdge { id: "f5".to_string(), condition: cond("flag_c2") });
        graph.add_edge(c1, inner_join, IREdge { id: "f6".to_string(), condition: None });
        graph.add_edge(c2, inner_join, IREdge { id: "f7".to_string(), condition: None });
        graph.add_edge(inner_join, outer_join, IREdge { id: "f8".to_string(), condition: None });
        graph.add_edge(outer_join, end, IREdge { id: "f9".to_string(), condition: None });

        let errors = verify(&graph);
        assert!(
            !errors.iter().any(|e| e.message.contains("Unmatched GatewayInclusive")
                || e.message.contains("Unclosed diverging gateway")
                || e.message.contains("Crossing gateway-kind boundaries")),
            "inclusive pair nested in a single outer branch must now be admitted, got: {errors:?}"
        );
    }

    /// V6 (2026-07-24) — supersedes this test's prior framing. This used to
    /// be `test_two_nested_and_pairs_in_and_branches_currently_rejected_by
    /// _sese_check`, a deliberate masking-bug tripwire: it asserted this
    /// exact topology (two independently-nested `GatewayAnd` pairs in
    /// sibling branches of an outer fork, branch B deliberately longer to
    /// skew BFS discovery order) was REJECTED by `check_gateway_and_nesting`
    /// — correctly rejected in effect, but for the WRONG reason ("Unmatched
    /// GatewayAnd (converging): no open diverging GatewayAnd found"), since
    /// that check's own single-stack-threaded-without-cloning defect
    /// (pushes/pops from one branch leaking into a sibling branch's
    /// traversal) happened to also misfire here. That accidental rejection
    /// was the ONLY thing standing between well-formed input and
    /// `lower()`'s own analogous `fork_pairing` mispairing bug (same defect
    /// class, in `lowering.rs`'s BFS-order pairing stack — see that
    /// module's `test_two_independently_nested_and_pairs_pair_correctly`
    /// for its own red/green proof).
    ///
    /// Both defects are now fixed in the same landing (`lowering.rs`'s
    /// `compute_gateway_pairing` and this module's
    /// `check_gateway_and_nesting`, both ported from `dsl::rpst::dfs_walk`'s
    /// clone-the-stack-per-branch-before-recursing discipline). This
    /// topology is genuinely well-nested (SESE) — two independent inner
    /// fork/join pairs, each fully contained in its own outer-fork branch —
    /// so it must now be ADMITTED, not rejected for any reason. This is
    /// exactly the direction-A fix landing per Adam's ruling ("fix the
    /// pairing mechanism first, then the false rejection it was masking
    /// must also be fixed so legal topology is admitted, not left rejected
    /// for a wrong reason once the real reason no longer applies").
    #[test]
    fn test_two_nested_and_pairs_in_and_branches_now_admitted() {
        let mut graph = IRGraph::new();
        let start = graph.add_node(IRNode::Start { id: "start".to_string() });
        let outer_fork = graph.add_node(IRNode::GatewayAnd {
            id: "outer_fork".to_string(), name: "OuterFork".to_string(), direction: GatewayDirection::Diverging,
        });
        let outer_join = graph.add_node(IRNode::GatewayAnd {
            id: "outer_join".to_string(), name: "OuterJoin".to_string(), direction: GatewayDirection::Converging,
        });
        let end = graph.add_node(IRNode::End { id: "end".to_string(), terminate: false });
        let inner_fork_a = graph.add_node(IRNode::GatewayAnd {
            id: "inner_fork_a".to_string(), name: "InnerForkA".to_string(), direction: GatewayDirection::Diverging,
        });
        let inner_join_a = graph.add_node(IRNode::GatewayAnd {
            id: "inner_join_a".to_string(), name: "InnerJoinA".to_string(), direction: GatewayDirection::Converging,
        });
        let a1 = graph.add_node(IRNode::ServiceTask { id: "a1".to_string(), name: "A1".to_string(), task_type: "a1".to_string() });
        let a2 = graph.add_node(IRNode::ServiceTask { id: "a2".to_string(), name: "A2".to_string(), task_type: "a2".to_string() });
        let inner_fork_b = graph.add_node(IRNode::GatewayAnd {
            id: "inner_fork_b".to_string(), name: "InnerForkB".to_string(), direction: GatewayDirection::Diverging,
        });
        let inner_join_b = graph.add_node(IRNode::GatewayAnd {
            id: "inner_join_b".to_string(), name: "InnerJoinB".to_string(), direction: GatewayDirection::Converging,
        });
        let b1 = graph.add_node(IRNode::ServiceTask { id: "b1".to_string(), name: "B1".to_string(), task_type: "b1".to_string() });
        let b2 = graph.add_node(IRNode::ServiceTask { id: "b2".to_string(), name: "B2".to_string(), task_type: "b2".to_string() });
        let b3 = graph.add_node(IRNode::ServiceTask { id: "b3".to_string(), name: "B3".to_string(), task_type: "b3".to_string() });
        let b_pre = graph.add_node(IRNode::ServiceTask { id: "b_pre".to_string(), name: "BPre".to_string(), task_type: "b_pre".to_string() });

        graph.add_edge(start, outer_fork, IREdge { id: "f0".to_string(), condition: None });
        graph.add_edge(outer_fork, inner_fork_a, IREdge { id: "fa0".to_string(), condition: None });
        graph.add_edge(inner_fork_a, a1, IREdge { id: "fa1".to_string(), condition: None });
        graph.add_edge(inner_fork_a, a2, IREdge { id: "fa2".to_string(), condition: None });
        graph.add_edge(a1, inner_join_a, IREdge { id: "fa3".to_string(), condition: None });
        graph.add_edge(a2, inner_join_a, IREdge { id: "fa4".to_string(), condition: None });
        graph.add_edge(inner_join_a, outer_join, IREdge { id: "fa5".to_string(), condition: None });
        graph.add_edge(outer_fork, b_pre, IREdge { id: "fb_pre".to_string(), condition: None });
        graph.add_edge(b_pre, inner_fork_b, IREdge { id: "fb0".to_string(), condition: None });
        graph.add_edge(inner_fork_b, b1, IREdge { id: "fb1".to_string(), condition: None });
        graph.add_edge(inner_fork_b, b2, IREdge { id: "fb2".to_string(), condition: None });
        graph.add_edge(inner_fork_b, b3, IREdge { id: "fb3".to_string(), condition: None });
        graph.add_edge(b1, inner_join_b, IREdge { id: "fb4".to_string(), condition: None });
        graph.add_edge(b2, inner_join_b, IREdge { id: "fb5".to_string(), condition: None });
        graph.add_edge(b3, inner_join_b, IREdge { id: "fb6".to_string(), condition: None });
        graph.add_edge(inner_join_b, outer_join, IREdge { id: "fb7".to_string(), condition: None });
        graph.add_edge(outer_join, end, IREdge { id: "fend".to_string(), condition: None });

        let errors = verify(&graph);
        assert!(
            !errors.iter().any(|e| e.message.contains("Unmatched GatewayAnd")
                || e.message.contains("Unclosed diverging gateway")
                || e.message.contains("Crossing gateway-kind boundaries")),
            "well-nested sibling-branch AND pairs must now be admitted, got: {errors:?}"
        );
    }
}
