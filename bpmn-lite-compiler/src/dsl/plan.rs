//! `WorkflowExecutionPlan` — the linted, DAG-validated output of the
//! bpmn-dsl compilation pipeline.
//!
//! This is the **bpmn-lite workflow DAG** (Commitment B scope =
//! process-instance, long-lived). It is **not** a Phase 5
//! [`ExecutablePlan`]: those are the inner per-callout plans the bus
//! emits at runtime when a service-task or business-rule-task is
//! dispatched to a receiver domain (ob-poc, dmn-lite, …).
//!
//! Lifecycle (v0.6 §3, T0 audit gap C):
//!
//! ```text
//! bpmn-dsl source
//!     ↓ parse / lint / dag   (workflow-compile-time, this crate)
//! WorkflowExecutionPlan      ←── this module's type
//!     ↓ start process instance (bpmn-lite engine)
//! ProcessInstance
//!     ↓ fire ServiceTaskExecNode N
//!     ↓ bus dispatch to target domain with N's static_args + bound placeholders
//!     ↓ receiver compiles inputs into Phase 5 ExecutablePlan ←── inner plan, NOT this type
//!     ↓ receiver executes ExecutablePlan, returns result
//! ProcessInstance advances to next node
//! ```
//!
//! The workflow plan cannot pre-compile inner ExecutablePlans because
//! placeholder values (`@cbu`, `@cbu-type`) are not known until the
//! upstream node has executed. Inner-plan compilation is a per-callout,
//! submit-time concern owned by the bus path (T2B).
//!
//! [`ExecutablePlan`]: ../../../../docs/todo/phase-5_5-bpmn-demo-plan-v0_6.md

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
}

impl WorkflowExecutionPlan {
    /// Return all end-event node ids.
    pub fn end_nodes(&self) -> Vec<&str> {
        self.nodes
            .values()
            .filter_map(|n| match n {
                ExecutionNode::EndEvent(e) => Some(e.id.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// One resolved node in the execution plan.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ExecutionNode {
    StartEvent(StartExecNode),
    ServiceTask(ServiceTaskExecNode),
    BusinessRuleTask(BusinessRuleExecNode),
    ExclusiveGateway(GatewayExecNode),
    EndEvent(EndExecNode),
}

impl ExecutionNode {
    pub fn id(&self) -> &str {
        match self {
            Self::StartEvent(n) => &n.id,
            Self::ServiceTask(n) => &n.id,
            Self::BusinessRuleTask(n) => &n.id,
            Self::ExclusiveGateway(n) => &n.id,
            Self::EndEvent(n) => &n.id,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StartExecNode {
    pub id: String,
    pub next: String,
}

/// Workflow node that dispatches a verb to a receiver domain via the bus.
///
/// At workflow-compile-time the node carries:
/// - the verb FQN (e.g. `ob-poc:cbu.create`)
/// - static args (literal bindings the DSL author wrote inline)
/// - placeholder producer / consumer wiring
///
/// At runtime, when the process instance reaches this node, the bus path
/// (T2B) builds an `InvocationRequest` from `verb_fqn` + `static_args` +
/// bound placeholders. The **receiver** domain compiles that request into
/// a Phase 5 `ExecutablePlan` and runs it locally. The inner plan is not
/// constructed here.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceTaskExecNode {
    pub id: String,
    /// Resolved verb FQN from catalogue. May be namespaced (`ob-poc:cbu.create`).
    pub verb_fqn: String,
    /// Static args (e.g. `product = "CUSTODY_FUND"`).
    pub static_args: HashMap<String, String>,
    pub next: String,
    /// Placeholder this node produces (inferred from catalogue).
    pub produces_placeholder: Option<String>,
    /// Placeholders this node consumes (inferred from catalogue).
    pub consumes_placeholders: Vec<String>,
}

/// Workflow node that dispatches a DMN decision to a receiver domain.
///
/// Mirrors [`ServiceTaskExecNode`]: workflow-compile-time records identity
/// and placeholder wiring; the bus path emits the inner ExecutablePlan to
/// the receiver at submit-time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BusinessRuleExecNode {
    pub id: String,
    /// Resolved decision id. May be namespaced (`dmn-lite:cbu_type_routing`).
    pub decision_id: String,
    pub next: String,
    pub produces_placeholder: Option<String>,
    pub consumes_placeholders: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GatewayExecNode {
    pub id: String,
    pub flows: Vec<GatewayExecFlow>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GatewayExecFlow {
    /// Placeholder name being tested (e.g. `"@cbu-type"`).
    pub placeholder: String,
    /// Expected value (e.g. `"fund"`).
    pub expected_value: String,
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
