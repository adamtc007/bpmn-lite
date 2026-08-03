//! `WorkflowExecutionPlan` — the linted, DAG-validated output of the
//! bpmn-dsl compilation pipeline.

use std::collections::{BTreeMap, HashMap};

/// A compiled, validated workflow ready for execution.
///
/// Fields are `pub(crate)` — the dsl compilation pipeline (`ir_plan`,
/// `linter`, `rpst`, `frontend`, `dag`, `closure`) constructs and inspects
/// plans directly; everything outside `bpmn-lite-compiler` goes through
/// [`WorkflowExecutionPlan::new`] and the accessor/mutator methods below.
/// This is deliberate: `mathematically_proved` / `unsafe_breeches` are a
/// governance invariant (the compiler's SESE/topology proof), not plain
/// data — a caller field-assigning `mathematically_proved = true` directly
/// would silently forge a proof the compiler never actually ran.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkflowExecutionPlan {
    pub(crate) workflow_id: String,
    /// Nodes in the workflow, keyed by node id.
    pub(crate) nodes: BTreeMap<String, ExecutionNode>,
    /// Id of the start node (entry point).
    pub(crate) start_node: String,
    /// Placeholder schema: what gets inferred and threaded between nodes.
    pub(crate) placeholder_schema: PlaceholderSchema,
    #[serde(default)]
    pub(crate) closure_manifest: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) regime_version: Option<String>,
    #[serde(default = "default_true")]
    pub(crate) mathematically_proved: bool,
    #[serde(default)]
    pub(crate) unsafe_breeches: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// Pass-6 delivery-mode derivation (P6/L8, CLAUDE.md "Blocking is derived,
/// not chosen"): an explicit author annotation always wins; otherwise a
/// consumed output forces `Blocking` (the caller needs the value before it
/// can proceed), a must-complete effect with no consumed output forces
/// `GuaranteedAsync` (fire-and-forget through the outbox), and anything
/// else defaults to `BestEffort`. Pure function so the DSL path
/// (`linter.rs`'s Pass 6, which computes `output_consumed`/`is_must_complete`
/// from the built plan + registry), the IR path (`ir_plan.rs`, which has no
/// catalogue signal for graph-authored `ServiceTask` nodes and always passes
/// `false`/`false`), and the BPMN-XML importer (`bpmn-lite-authoring`'s
/// `importer.rs`, which has the same "no catalogue signal" situation as
/// `ir_plan.rs` for its `ServiceTask` case) all share the identical formula —
/// never two divergent implementations of the same rule. `pub`, not
/// `pub(crate)` (pub-scope audit, 2026-07-29): the importer lives in a
/// different crate and has no other way to reach this.
pub fn derive_delivery_mode(
    explicit: Option<DeliveryMode>,
    output_consumed: bool,
    is_must_complete: bool,
) -> DeliveryMode {
    if let Some(mode) = explicit {
        return mode;
    }
    if output_consumed {
        DeliveryMode::Blocking
    } else if is_must_complete {
        DeliveryMode::GuaranteedAsync
    } else {
        DeliveryMode::BestEffort
    }
}

impl WorkflowExecutionPlan {
    /// Construct a freshly-compiled plan and run [`Self::analyze_safety`]
    /// on it before returning. Every dsl-pipeline construction site
    /// (`ir_plan::project_ir`, `linter::lint`) and the BPMN-XML importer
    /// (`bpmn-lite-authoring`) built this exact same
    /// construct-then-analyze sequence by hand; this is the single place
    /// that invariant now lives.
    pub fn new(
        workflow_id: String,
        nodes: BTreeMap<String, ExecutionNode>,
        start_node: String,
        placeholder_schema: PlaceholderSchema,
        closure_manifest: Option<serde_json::Value>,
        regime_version: Option<String>,
    ) -> Self {
        let mut plan = Self {
            workflow_id,
            nodes,
            start_node,
            placeholder_schema,
            closure_manifest,
            regime_version,
            mathematically_proved: true,
            unsafe_breeches: Vec::new(),
        };
        plan.analyze_safety();
        plan
    }

    pub fn workflow_id(&self) -> &str {
        &self.workflow_id
    }

    pub fn nodes(&self) -> &BTreeMap<String, ExecutionNode> {
        &self.nodes
    }

    pub fn start_node(&self) -> &str {
        &self.start_node
    }

    pub fn placeholder_schema(&self) -> &PlaceholderSchema {
        &self.placeholder_schema
    }

    pub fn closure_manifest(&self) -> Option<&serde_json::Value> {
        self.closure_manifest.as_ref()
    }

    pub fn regime_version(&self) -> Option<&str> {
        self.regime_version.as_deref()
    }

    pub fn mathematically_proved(&self) -> bool {
        self.mathematically_proved
    }

    pub fn unsafe_breeches(&self) -> &[String] {
        &self.unsafe_breeches
    }

    /// Re-run [`Self::analyze_safety`], but never let it silently upgrade a
    /// plan a caller had already marked unproven back to proven — a
    /// caller-submitted `plan_body` may have failed a check
    /// `analyze_safety` doesn't cover (e.g. bytecode admission), so a prior
    /// `false` claim is preserved regardless of what this re-analysis
    /// finds.
    pub fn reverify_preserving_distrust(&mut self) {
        let was_proved = self.mathematically_proved;
        self.analyze_safety();
        if !was_proved {
            self.mathematically_proved = false;
        }
    }

    /// Record that this plan depends on a transitively-unproven child
    /// sub-workflow. Idempotent — calling it more than once does not
    /// duplicate the breach entry.
    pub fn mark_transitive_unproved_call(&mut self) {
        const BREACH: &str = "TRANSITIVE_UNPROVED_CALL";
        if !self.unsafe_breeches.iter().any(|b| b == BREACH) {
            self.unsafe_breeches.push(BREACH.to_string());
        }
    }

    /// BFS from `entry` over flow successors + guard escape entries:
    /// does any path reach an End node? Used by `analyze_safety`'s guard
    /// escape-closure check.
    fn reaches_end(&self, entry: &str) -> bool {
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::from([entry]);
        while let Some(id) = queue.pop_front() {
            if !visited.insert(id) {
                continue;
            }
            match self.nodes.get(id) {
                Some(ExecutionNode::End(_)) => return true,
                Some(node) => {
                    queue.extend(node.flow_successors());
                    queue.extend(node.guard_escape_entries());
                }
                None => {}
            }
        }
        false
    }

    /// Return all end-event node ids.
    pub fn end_nodes(&self) -> Vec<&str> {
        self.nodes
            .values()
            .filter_map(|n| match n {
                ExecutionNode::End(e) => Some(e.id.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Check the plan's components for safety and populate proof indicators.
    pub fn analyze_safety(&mut self) {
        let mut breaches = Vec::new();

        // 1. Check for SESE topology violations using our rpst utility
        if crate::dsl::rpst::verify_sese_nesting(self).is_err() {
            breaches.push("BPMN_NON_SESE_TOPOLOGY".to_string());
        }

        // 2. Check each node
        for node in self.nodes.values() {
            match node {
                ExecutionNode::Task(t) => {
                    if t.plug == "bpmn:unsafe-placeholder" {
                        if let Some(kind) = t.static_args.get("original_kind") {
                            if kind.contains("BoundaryTimer")
                                || kind.contains("BoundaryError")
                                || kind.contains("Boundary")
                            {
                                breaches.push("BPMN_BOUNDARY_EVENT_BYPASS".to_string());
                            } else {
                                breaches.push("BPMN_UNSUPPORTED_NODE_BYPASS".to_string());
                            }
                        } else {
                            breaches.push("BPMN_UNSUPPORTED_NODE_BYPASS".to_string());
                        }
                    } else if t.plug.contains("boundary-timer")
                        || t.plug.contains("boundary-error")
                        || t.plug.contains("boundary")
                    {
                        breaches.push("BPMN_BOUNDARY_EVENT_BYPASS".to_string());
                    }
                }
                ExecutionNode::Split(s) => {
                    for flow in &s.flows {
                        if let Some(ref placeholder) = flow.placeholder {
                            if placeholder == "@feel_eval_warning" {
                                breaches.push("FEEL_EVALUATION_WARNING".to_string());
                            }
                        }
                        if let Some(ref val) = flow.expected_value {
                            if val == "unparsed_expression" {
                                breaches.push("FEEL_EVALUATION_WARNING".to_string());
                            }
                        }
                    }
                    if !self.nodes.contains_key(&s.join) {
                        breaches.push("BPMN_NON_SESE_TOPOLOGY".to_string());
                    }
                }
                ExecutionNode::Join(j) if !self.nodes.contains_key(&j.split) => {
                    breaches.push("BPMN_NON_SESE_TOPOLOGY".to_string());
                }
                _ => {}
            }
        }

        // 3. Guard escape closure (WS-D D1): every guard's escape_entry
        // must exist, and every escape subgraph must reach an End — a
        // guard whose handler leads nowhere is a control transfer off the
        // proved surface. `project_ir` can't produce these shapes (its
        // single-successor rule + validate_dag close them structurally),
        // but a caller-submitted plan_body can, and the proof must hold
        // for the PLAN, not for one construction path.
        for node in self.nodes.values() {
            for entry in node.guard_escape_entries() {
                if !self.nodes.contains_key(entry) {
                    breaches.push("BPMN_GUARD_ESCAPE_DANGLING".to_string());
                } else if !self.reaches_end(entry) {
                    breaches.push("BPMN_GUARD_ESCAPE_OPEN".to_string());
                }
            }
        }

        // Deduplicate breaches
        breaches.sort();
        breaches.dedup();

        if !breaches.is_empty() {
            self.mathematically_proved = false;
            self.unsafe_breeches = breaches;
        } else {
            self.mathematically_proved = true;
            self.unsafe_breeches = Vec::new();
        }
    }
}

/// One resolved node in the execution plan.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ExecutionNode {
    Start(StartExecNode),
    Task(TaskExecNode),
    Split(SplitExecNode),
    Join(JoinExecNode),
    Loop(LoopExecNode),
    Wait(WaitExecNode),
    End(EndExecNode),
}

impl ExecutionNode {
    pub fn id(&self) -> &str {
        match self {
            Self::Start(n) => &n.id,
            Self::Task(n) => &n.id,
            Self::Split(n) => &n.id,
            Self::Join(n) => &n.id,
            Self::Loop(n) => &n.id,
            Self::Wait(n) => &n.id,
            Self::End(n) => &n.id,
        }
    }

    /// Plain control-flow successors of this node — the single shared
    /// successor formula for every plan traversal (`dag.rs` adjacency,
    /// `analyze_safety`'s escape-closure walk), so a new node kind can
    /// never be silently skipped by one walker while another learned it.
    /// Guard escape entries are deliberately NOT included here: a guard is
    /// a decoration, not a flow edge, and each traversal decides
    /// explicitly whether escape subgraphs are in its scope (see
    /// `guard_escape_entries`).
    pub fn flow_successors(&self) -> Vec<&str> {
        match self {
            Self::Start(n) => vec![n.next.as_str()],
            Self::Task(n) => vec![n.next.as_str()],
            Self::Split(n) => n.flows.iter().map(|f| f.next.as_str()).collect(),
            Self::Join(n) => vec![n.next.as_str()],
            Self::Loop(n) => {
                let mut v = vec![n.next.as_str()];
                if let Some(first) = n.body.first() {
                    v.push(first.as_str());
                }
                v
            }
            Self::Wait(n) => vec![n.next.as_str()],
            Self::End(_) => vec![],
        }
    }

    /// Escape-flow entry node ids of every guard attached to this node
    /// (empty for anything but a guarded Task). The companion to
    /// [`Self::flow_successors`] for traversals that must also cover
    /// escape subgraphs (reachability, cycle detection, escape closure).
    pub fn guard_escape_entries(&self) -> Vec<&str> {
        match self {
            Self::Task(n) => n.guards.iter().map(|g| g.escape_entry.as_str()).collect(),
            _ => vec![],
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StartExecNode {
    pub id: String,
    pub next: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<bpmn_lite_types::SourceSpan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeliveryMode {
    Blocking,
    GuaranteedAsync,
    BestEffort,
}

/// Unified task node governing a plug (service verb, decision, call-activity)
/// and its delivery mode.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskExecNode {
    pub id: String,
    /// The plug name/hash (e.g. `"ob-poc:cbu.create"`, `"dmn-lite:cbu_type_routing"`).
    pub plug: String,
    /// Derived/configured delivery mode.
    pub delivery_mode: DeliveryMode,
    /// Static args passed to the plug.
    pub static_args: HashMap<String, String>,
    pub next: String,
    /// Placeholder this node produces.
    pub produces_placeholder: Option<String>,
    /// Placeholders this node consumes.
    pub consumes_placeholders: Vec<String>,
    /// Boundary guards attached to this task (WS-D D1). A guard is a
    /// DECORATION on its host, not a flow node — mirroring the kernel's
    /// model, where guard arming is emitted inline in the host's own
    /// lowering (`lowering.rs`'s `lower_boundary_guarded_task_v2`) and the
    /// escape flow is ordinary downstream bytecode. `serde(default)` so
    /// every plan serialized before this field existed deserializes as
    /// unguarded, which is exactly what it was.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guards: Vec<GuardExecSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<bpmn_lite_types::SourceSpan>,
}

/// One boundary guard attached to a [`TaskExecNode`] (WS-D D1).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GuardExecSpec {
    /// The guard's own node id in the authored graph (diagnostics +
    /// stable identity; the guard is not a plan node).
    pub guard_id: String,
    pub trigger: GuardTriggerExec,
    /// Per-guard failure budget (verifier V8/§31); `None` inherits the
    /// workflow default.
    pub failure_budget: Option<u32>,
    /// Entry node id of the guard's escape flow — exactly one, mirroring
    /// the lowering's single-successor handler resolution (`handler` is
    /// one bytecode address). The escape subgraph itself is ordinary plan
    /// nodes.
    pub escape_entry: String,
}

/// Guard trigger, plan-side. `interrupting` lives INSIDE the `Timer`
/// variant because a boundary error is interrupting-only in this substrate
/// (designer-graph F2a; BPMN has no non-interrupting error boundary) — the
/// type makes "non-interrupting error" unrepresentable rather than
/// runtime-checked.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GuardTriggerExec {
    Timer {
        spec: crate::ir::TimerSpec,
        interrupting: bool,
    },
    Error {
        /// `None` = catch-all route (sorted last at lowering, matching
        /// kernel `error_routes` precedence).
        error_code: Option<String>,
    },
}

/// Standalone timer wait (`IRNode::TimerWait`), WS-D D1. Replaces the
/// dead `"bpmn:timer-wait"` Task-plug shoehorn (no consumer of that plug
/// ever existed) with a first-class node the frontend can lower to real
/// `V2WaitFor`/`V2WaitUntil` opcodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WaitExecNode {
    pub id: String,
    pub spec: crate::ir::TimerSpec,
    pub next: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<bpmn_lite_types::SourceSpan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SplitMode {
    Exclusive,
    Inclusive,
    Parallel,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SplitExecNode {
    pub id: String,
    pub mode: SplitMode,
    pub routing_socket: Option<String>,
    pub flows: Vec<SplitExecFlow>,
    pub join: String,
    pub produces_placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<bpmn_lite_types::SourceSpan>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SplitExecFlow {
    /// Placeholder name being tested (e.g. `"@cbu-type"`).
    pub placeholder: Option<String>,
    /// Expected value (e.g. `"fund"`).
    pub expected_value: Option<String>,
    pub next: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JoinMode {
    Exclusive,
    Inclusive,
    Parallel,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JoinExecNode {
    pub id: String,
    pub mode: JoinMode,
    pub split: String,
    pub next: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<bpmn_lite_types::SourceSpan>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoopExecNode {
    pub id: String,
    pub ceiling: u32,
    pub body: Vec<String>,
    pub next: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<bpmn_lite_types::SourceSpan>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EndExecNode {
    pub id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<bpmn_lite_types::SourceSpan>,
}

/// Inferred binding flow across the workflow.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PlaceholderSchema {
    /// All placeholder slots, keyed by name (e.g. `"@cbu"`).
    pub slots: HashMap<String, PlaceholderSlot>,
}

/// One inferred placeholder slot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlaceholderSlot {
    /// Slot name including `@` prefix (e.g. `"@cbu"`).
    pub name: String,
    /// Id of the node that produces this slot's value.
    pub produced_by: String,
    /// Ids of nodes that consume this slot's value.
    pub consumed_by: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::TimerSpec;
    use std::collections::BTreeMap;

    /// start → t1(guarded) → end, with the guard's escape_entry set by
    /// the caller — hand-built (not via project_ir) precisely because
    /// analyze_safety must prove the PLAN, not one construction path.
    fn guarded_plan(escape_entry: &str, extra: Vec<(String, ExecutionNode)>) -> WorkflowExecutionPlan {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "start".to_string(),
            ExecutionNode::Start(StartExecNode {
                id: "start".into(),
                next: "t1".into(),
                span: None,
            }),
        );
        nodes.insert(
            "t1".to_string(),
            ExecutionNode::Task(TaskExecNode {
                id: "t1".into(),
                plug: "noop".into(),
                delivery_mode: DeliveryMode::BestEffort,
                static_args: Default::default(),
                next: "end".into(),
                produces_placeholder: None,
                consumes_placeholders: Vec::new(),
                guards: vec![GuardExecSpec {
                    guard_id: "bt".into(),
                    trigger: GuardTriggerExec::Timer {
                        spec: TimerSpec::Duration { ms: 1000 },
                        interrupting: true,
                    },
                    failure_budget: None,
                    escape_entry: escape_entry.into(),
                }],
                span: None,
            }),
        );
        nodes.insert(
            "end".to_string(),
            ExecutionNode::End(EndExecNode {
                id: "end".into(),
                status: "done".into(),
                span: None,
            }),
        );
        for (id, node) in extra {
            nodes.insert(id, node);
        }
        WorkflowExecutionPlan::new(
            "wf".into(),
            nodes,
            "start".into(),
            PlaceholderSchema::default(),
            None,
            None,
        )
    }

    /// RED (WS-D D1): a guard whose escape_entry names no node is a
    /// dangling handler — proof must fall.
    #[test]
    fn dangling_guard_escape_breaks_the_proof() {
        let plan = guarded_plan("nowhere", vec![]);
        assert!(!plan.mathematically_proved());
        assert!(plan
            .unsafe_breeches()
            .contains(&"BPMN_GUARD_ESCAPE_DANGLING".to_string()));
    }

    /// RED (WS-D D1): an escape subgraph that never reaches an End is an
    /// open route off the proved surface — proof must fall.
    #[test]
    fn open_guard_escape_breaks_the_proof() {
        // escape_entry exists but its own successor dangles, so no End is
        // ever reached from it.
        let stranded = ExecutionNode::Task(TaskExecNode {
            id: "esc".into(),
            plug: "noop".into(),
            delivery_mode: DeliveryMode::BestEffort,
            static_args: Default::default(),
            next: "nowhere".into(),
            produces_placeholder: None,
            consumes_placeholders: Vec::new(),
            guards: Vec::new(),
            span: None,
        });
        let plan = guarded_plan("esc", vec![("esc".to_string(), stranded)]);
        assert!(!plan.mathematically_proved());
        assert!(plan
            .unsafe_breeches()
            .contains(&"BPMN_GUARD_ESCAPE_OPEN".to_string()));
    }

    /// GREEN (WS-D D1): a self-contained escape subgraph ending in its
    /// own End keeps the proof intact.
    #[test]
    fn closed_guard_escape_keeps_the_proof() {
        let esc = ExecutionNode::Task(TaskExecNode {
            id: "esc".into(),
            plug: "noop".into(),
            delivery_mode: DeliveryMode::BestEffort,
            static_args: Default::default(),
            next: "end_esc".into(),
            produces_placeholder: None,
            consumes_placeholders: Vec::new(),
            guards: Vec::new(),
            span: None,
        });
        let esc_end = ExecutionNode::End(EndExecNode {
            id: "end_esc".into(),
            status: "escalated".into(),
            span: None,
        });
        let plan = guarded_plan(
            "esc",
            vec![("esc".to_string(), esc), ("end_esc".to_string(), esc_end)],
        );
        assert!(
            plan.mathematically_proved(),
            "breaches: {:?}",
            plan.unsafe_breeches()
        );
    }
}
