use bpmn_lite_types::ffi_bindings::{DataObjectRole, DataObjectType};
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};

/// Gateway direction for parallel/exclusive gateways.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatewayDirection {
    Diverging,
    Converging,
}

/// Timer specification.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TimerSpec {
    Duration { ms: u64 },
    Date { deadline_ms: u64 },
    Cycle { interval_ms: u64, max_fires: u32 },
}

/// Condition expression for XOR gateway edges.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConditionExpr {
    pub flag_name: String,
    pub op: ConditionOp,
    pub literal: ConditionLiteral,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ConditionOp {
    Eq,
    Neq,
    Lt,
    Gt,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ConditionLiteral {
    Bool(bool),
    I64(i64),
}

/// IR node — one per BPMN element.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum IRNode {
    Start {
        id: String,
    },
    End {
        id: String,
        terminate: bool,
    },
    ServiceTask {
        id: String,
        name: String,
        task_type: String,
    },
    GatewayXor {
        id: String,
        name: String,
    },
    GatewayAnd {
        id: String,
        name: String,
        direction: GatewayDirection,
    },
    TimerWait {
        id: String,
        spec: TimerSpec,
    },
    MessageWait {
        id: String,
        name: String,
        corr_key_source: String,
    },
    HumanWait {
        id: String,
        name: String,
        task_kind: String,
        corr_key_source: String,
    },
    BoundaryTimer {
        id: String,
        attached_to: String,
        spec: TimerSpec,
        interrupting: bool,
    },
    BoundaryError {
        id: String,
        attached_to: String,
        error_code: Option<String>,
    },
    GatewayInclusive {
        id: String,
        name: String,
        direction: GatewayDirection,
    },

    /// A BPMN data object declaration with a type annotation.
    ///
    /// These are structural nodes: no sequence-flow edges and zero bytecode
    /// instructions. They participate in the graph only so the lowering
    /// pre-pass can discover them alongside process-flow nodes in a single
    /// traversal. `estimate_instr_count` returns 0 for DataObject.
    DataObject {
        id: String,
        name: String,
        type_decl: DataObjectType,
        role: DataObjectRole,
    },

    /// A BPMN ServiceTask annotated with `<bpmn:taskDefinition implementation="...">`.
    ///
    /// Distinct from `ServiceTask` (which uses `<zeebe:taskDefinition type="...">` and
    /// dispatches via the external-job path). `FfiServiceTask` lowers to
    /// `Instr::ExecFfi` and stores a `FfiTaskDecl` in `CompiledProgram.ffi_task_decls`.
    FfiServiceTask {
        id: String,
        name: String,
        /// Decoded 32-byte BLAKE3 template_id from the `implementation="<64hex>"` attribute.
        template_id: [u8; 32],
        inputs: Vec<FfiInputBinding>,
        outputs: Vec<FfiOutputBinding>,
    },
}

// ── C-minimal expression language ────────────────────────────────────────────

/// Literal value types at the IR (pre-lowering) level.
///
/// Maps 1:1 to `bpmn_lite_types::ffi_bindings::Literal` after lowering.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IrLiteral {
    Bool(bool),
    I64(i64),
    F64(f64),
    String(String),
}

/// A C-minimal expression from a `<bpmn:input expression="...">` attribute.
///
/// Per A2 §5. At lowering time, `VarRef` is resolved against `data_objects`
/// to produce a `BindingSource`. `Literal` is copied as-is.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "expr", rename_all = "snake_case")]
pub enum Expression {
    Literal(IrLiteral),
    /// Dotted variable path, e.g. `${customer.jurisdiction}` → `["customer", "jurisdiction"]`.
    VarRef(Vec<String>),
}

/// One `<bpmn:input>` element inside a `FfiServiceTask` extension.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FfiInputBinding {
    /// FFI template input field name (`target=` attribute).
    pub target_field: String,
    pub expression: Expression,
}

/// One `<bpmn:output>` element inside a `FfiServiceTask` extension.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FfiOutputBinding {
    /// FFI template output field name (`source=` attribute).
    pub source_field: String,
    /// Process variable name (`target=` attribute) — resolved to a
    /// `DataObjectDecl` during lowering.
    pub target_variable: String,
}

impl IRNode {
    pub fn id(&self) -> &str {
        match self {
            IRNode::Start { id } => id,
            IRNode::End { id, .. } => id,
            IRNode::ServiceTask { id, .. } => id,
            IRNode::GatewayXor { id, .. } => id,
            IRNode::GatewayAnd { id, .. } => id,
            IRNode::TimerWait { id, .. } => id,
            IRNode::MessageWait { id, .. } => id,
            IRNode::HumanWait { id, .. } => id,
            IRNode::BoundaryTimer { id, .. } => id,
            IRNode::BoundaryError { id, .. } => id,
            IRNode::GatewayInclusive { id, .. } => id,
            IRNode::DataObject { id, .. } => id,
            IRNode::FfiServiceTask { id, .. } => id,
        }
    }
}

/// IR edge — one per sequence flow.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IREdge {
    pub id: String,
    pub condition: Option<ConditionExpr>,
}

/// The intermediate representation — a directed graph of BPMN elements.
pub type IRGraph = DiGraph<IRNode, IREdge>;

/// Helper to find a node by its BPMN element id.
pub fn find_node_by_id(graph: &IRGraph, element_id: &str) -> Option<NodeIndex> {
    graph
        .node_indices()
        .find(|&idx| graph[idx].id() == element_id)
}

/// Helper to find the start node.
pub fn find_start(graph: &IRGraph) -> Option<NodeIndex> {
    graph
        .node_indices()
        .find(|&idx| matches!(&graph[idx], IRNode::Start { .. }))
}
