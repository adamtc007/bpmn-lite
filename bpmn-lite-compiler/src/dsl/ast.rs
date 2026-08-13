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
    MessageWait(MessageWaitAst),
    Split(SplitAst),
    Join(JoinAst),
    Loop(LoopAst),
    BoundaryTimer(BoundaryTimerAst),
    BoundaryError(BoundaryErrorAst),
}

impl NodeAst {
    pub fn id(&self) -> &str {
        match self {
            Self::Start(n) => &n.id,
            Self::End(n) => &n.id,
            Self::Task(n) => &n.id,
            Self::MessageWait(n) => &n.id,
            Self::Split(n) => &n.id,
            Self::Join(n) => &n.id,
            Self::Loop(n) => &n.id,
            Self::BoundaryTimer(n) => &n.id,
            Self::BoundaryError(n) => &n.id,
        }
    }

    pub fn span(&self) -> bpmn_lite_types::SourceSpan {
        match self {
            Self::Start(n) => n.span,
            Self::End(n) => n.span,
            Self::Task(n) => n.span,
            Self::MessageWait(n) => n.span,
            Self::Split(n) => n.span,
            Self::Join(n) => n.span,
            Self::Loop(n) => n.span,
            Self::BoundaryTimer(n) => n.span,
            Self::BoundaryError(n) => n.span,
        }
    }
}

/// D1 (EOP-PLAN-DSL-PARITY-001): a boundary timer guard — a DECORATION
/// on its `:host` task, not a plan node. Lowers to a `GuardExecSpec` on
/// the host's `TaskExecNode`, never to an `ExecutionNode` of its own.
/// `next` is the escape-flow entry (exactly one, mirroring the
/// lowering's single-handler resolution).
#[derive(Debug, Clone)]
pub struct BoundaryTimerAst {
    pub id: String,
    pub host: String,
    pub spec: crate::ir::TimerSpec,
    pub interrupting: bool,
    pub budget: Option<u32>,
    pub next: String,
    pub span: bpmn_lite_types::SourceSpan,
}

/// D1: a boundary error guard — same decoration semantics as
/// [`BoundaryTimerAst`]; always interrupting (F2a — a rearming error
/// guard is not a representable construct, so no flag exists here).
#[derive(Debug, Clone)]
pub struct BoundaryErrorAst {
    pub id: String,
    pub host: String,
    pub error_code: Option<String>,
    pub budget: Option<u32>,
    pub next: String,
    pub span: bpmn_lite_types::SourceSpan,
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
    /// G3.1 provenance: the original (unqualified) loop id this task was
    /// unrolled from, or `None` if authored outside any loop. Never set by
    /// the parser — stamped only by `unroll::unroll_loops`. Lets
    /// `closure.rs`'s L6 idempotency-inside-a-retried-task check survive
    /// unrolling without needing `ExecutionNode::Loop` to still exist.
    pub loop_origin: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MessageWaitAst {
    pub id: String,
    pub name: String,
    pub correlation_source: String,
    pub next: String,
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
