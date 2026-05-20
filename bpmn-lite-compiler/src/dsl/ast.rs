//! Typed AST for the bpmn-dsl s-expression workflow definition language.
//!
//! The AST is produced by the parser and consumed by the linter. It mirrors
//! the source text structure: no semantic enrichment happens here.

/// A parsed bpmn-dsl source file — one workflow definition.
#[derive(Debug, Clone)]
pub struct WorkflowSource {
    pub name: String,
    pub nodes: Vec<NodeAst>,
}

/// One node in the workflow graph.
#[derive(Debug, Clone)]
pub enum NodeAst {
    StartEvent(StartEventAst),
    ServiceTask(ServiceTaskAst),
    BusinessRuleTask(BusinessRuleTaskAst),
    ExclusiveGateway(ExclusiveGatewayAst),
    EndEvent(EndEventAst),
}

impl NodeAst {
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

#[derive(Debug, Clone)]
pub struct StartEventAst {
    pub id: String,
    pub next: String,
}

#[derive(Debug, Clone)]
pub struct ServiceTaskAst {
    pub id: String,
    /// Verb FQN from the catalogue (e.g. `"cbu.create"`, `"cbu.add-product"`).
    pub verb: String,
    /// Static arg overrides from `:args (:key "value" ...)`.
    pub args: Vec<(String, String)>,
    pub next: String,
}

#[derive(Debug, Clone)]
pub struct BusinessRuleTaskAst {
    pub id: String,
    pub decision: String,
    pub next: String,
}

#[derive(Debug, Clone)]
pub struct ExclusiveGatewayAst {
    pub id: String,
    pub flows: Vec<FlowAst>,
}

/// One outgoing sequence flow from an exclusive gateway.
#[derive(Debug, Clone)]
pub struct FlowAst {
    pub condition: ConditionAst,
    pub next: String,
}

/// Gateway predicate. T1 supports equality test against a string literal only.
/// `(= @placeholder "value")` → `Eq { placeholder, value }`.
#[derive(Debug, Clone)]
pub enum ConditionAst {
    Eq { placeholder: String, value: String },
}

#[derive(Debug, Clone)]
pub struct EndEventAst {
    pub id: String,
    pub status: String,
}
