//! `WorkflowExecutionPlan` — the linted, DAG-validated output of the
//! bpmn-dsl compilation pipeline.

use std::collections::HashMap;

/// A compiled, validated workflow ready for execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkflowExecutionPlan {
    pub workflow_id: String,
    /// Nodes in the workflow, keyed by node id.
    pub nodes: HashMap<String, ExecutionNode>,
    /// Id of the start node (entry point).
    pub start_node: String,
    /// Placeholder schema: what gets inferred and threaded between nodes.
    pub placeholder_schema: PlaceholderSchema,
    #[serde(default)]
    pub closure_manifest: Option<serde_json::Value>,
    #[serde(default)]
    pub regime_version: Option<String>,
}

impl WorkflowExecutionPlan {
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
}

/// One resolved node in the execution plan.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ExecutionNode {
    Start(StartExecNode),
    Task(TaskExecNode),
    Split(SplitExecNode),
    Join(JoinExecNode),
    Loop(LoopExecNode),
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
            Self::End(n) => &n.id,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StartExecNode {
    pub id: String,
    pub next: String,
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
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoopExecNode {
    pub id: String,
    pub ceiling: u32,
    pub body: Vec<String>,
    pub next: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EndExecNode {
    pub id: String,
    pub status: String,
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
