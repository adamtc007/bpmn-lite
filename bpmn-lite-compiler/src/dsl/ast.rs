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
    Start(StartAst),
    End(EndAst),
    Task(TaskAst),
    Split(SplitAst),
    Join(JoinAst),
    Loop(LoopAst),
}

impl NodeAst {
    pub fn id(&self) -> &str {
        match self {
            Self::Start(n) => &n.id,
            Self::End(n) => &n.id,
            Self::Task(n) => &n.id,
            Self::Split(n) => &n.id,
            Self::Join(n) => &n.id,
            Self::Loop(n) => &n.id,
        }
    }

    pub fn span(&self) -> bpmn_lite_types::SourceSpan {
        match self {
            Self::Start(n) => n.span,
            Self::End(n) => n.span,
            Self::Task(n) => n.span,
            Self::Split(n) => n.span,
            Self::Join(n) => n.span,
            Self::Loop(n) => n.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StartAst {
    pub id: String,
    pub next: String,
    pub span: bpmn_lite_types::SourceSpan,
}

#[derive(Debug, Clone)]
pub struct EndAst {
    pub id: String,
    pub status: String,
    pub span: bpmn_lite_types::SourceSpan,
}

#[derive(Debug, Clone)]
pub struct TaskAst {
    pub id: String,
    pub plug: String,
    pub args: Vec<(String, String)>,
    pub next: String,
    pub delivery_mode: Option<String>,
    pub span: bpmn_lite_types::SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitModeAst {
    Xor,
    Or,
    And,
}

#[derive(Debug, Clone)]
pub struct SplitAst {
    pub id: String,
    pub mode: SplitModeAst,
    pub plug: Option<String>,
    pub flows: Vec<SplitFlowAst>,
    pub join: String,
    pub span: bpmn_lite_types::SourceSpan,
}

#[derive(Debug, Clone)]
pub struct SplitFlowAst {
    pub condition: Option<ConditionAst>,
    pub next: String,
}

#[derive(Debug, Clone)]
pub enum ConditionAst {
    Eq { placeholder: String, value: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinModeAst {
    Xor,
    Or,
    And,
}

#[derive(Debug, Clone)]
pub struct JoinAst {
    pub id: String,
    pub mode: JoinModeAst,
    pub split: String,
    pub next: String,
    pub span: bpmn_lite_types::SourceSpan,
}

#[derive(Debug, Clone)]
pub struct LoopAst {
    pub id: String,
    pub ceiling: u32,
    pub body: Vec<NodeAst>,
    pub next: String,
    pub span: bpmn_lite_types::SourceSpan,
}
