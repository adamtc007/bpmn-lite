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
        /// V8 (§31) — per-guard failure budget (`max_failures`) declared on
        /// the boundary event; `None` inherits the workflow default.
        failure_budget: Option<u32>,
    },
    BoundaryError {
        id: String,
        attached_to: String,
        error_code: Option<String>,
        /// V8 (§31) — see `BoundaryTimer::failure_budget`.
        failure_budget: Option<u32>,
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

    /// BPMN Send Task — publishes a message and continues.
    ///
    /// Fire-and-continue semantics: a `(message_name, correlation_key)` pair is
    /// published into the engine's message buffer at execution time, then the
    /// token advances on the outgoing flow. No waiting (that is Receive Task).
    SendTask {
        id: String,
        name: String,
        /// Message name to publish. Taken from the BPMN task `name` attribute
        /// (mirroring `IntermediateCatchEvent` message-name convention).
        message_name: String,
        /// Register index whose value is used as the correlation key at publish
        /// time (mirrors `IRNode::MessageWait::corr_key_source`).
        corr_key_source: String,
    },

    /// A parallel multi-instance activity (§18 ruling K) — Camunda 8's
    /// "MI body wraps one inner activity" model, lowered onto ruling H's
    /// `V2Fork`/`V2Join` dynamic-arity mechanism: `declared_max` static
    /// synthesized branches, each running the inner activity's own
    /// `task_type` if its index is live (`collection_flag_name`'s runtime
    /// value), skip-to-join otherwise. v2-only (`LoweringTarget::V2`) — no
    /// v1 lowering exists, matching inclusive gateways' and boundary
    /// timers' own v1/v2 split. XML-only, same as boundary timers: no DSL
    /// AST hook exists or is added by this construct (checked, not
    /// assumed — see the V5 plan-doc writeup).
    ///
    /// **Revised for ruling K Part 2 (per-element value access, landed
    /// 2026-07-23).** The prior landing's `length_flag_name` field named a
    /// flag carrying the collection's runtime LENGTH ONLY (an `I64`)
    /// because no array-valued `Value` existed. `Value::Array` now exists,
    /// so `length_flag_name` is renamed `collection_flag_name` and now
    /// names a flag carrying the collection's actual `Value::Array` data —
    /// length is derived from it, not tracked separately (see
    /// `Instr::V2MiIndexLive`'s doc comment for why a redundant length
    /// flag was rejected as a footgun). The BPMN XML shape is unchanged
    /// (`zeebe:loopCharacteristics inputCollection="<flag-name>"` still
    /// names one flag); only the runtime type the parser/lowering expect
    /// that flag to hold has changed.
    MultiInstance {
        id: String,
        name: String,
        /// The inner activity's dispatch identity — same convention as
        /// `ServiceTask::task_type` (external-job task type string). MI
        /// wrapping an `FfiServiceTask`/`HumanWait`/other activity kind is
        /// out of scope for this step.
        task_type: String,
        /// Name of the data-object/flag carrying the collection's actual
        /// `Value::Array` data. Same `flag_name` convention `ConditionExpr`
        /// already uses for XOR/inclusive-gateway conditions, reused
        /// rather than inventing a second name-reference shape.
        collection_flag_name: String,
        /// The artifact-declared maximum instance count (ruling K delta
        /// (a) — Zeebe has no such ceiling; this is a required, forced
        /// deviation, not an oversight). `V2Fork`'s static `targets.len()`
        /// equals this value exactly.
        declared_max: u32,
    },
}

// ── C-minimal expression language ────────────────────────────────────────────

/// Literal value types at the IR (pre-lowering) level.
///
/// Maps 1:1 to `bpmn_lite_types::ffi_bindings::Literal` after lowering.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum IrLiteral {
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
pub(crate) struct FfiInputBinding {
    /// FFI template input field name (`target=` attribute).
    pub target_field: String,
    pub expression: Expression,
}

/// One `<bpmn:output>` element inside a `FfiServiceTask` extension.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FfiOutputBinding {
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
            IRNode::SendTask { id, .. } => id,
            IRNode::MultiInstance { id, .. } => id,
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
pub(crate) fn find_node_by_id(graph: &IRGraph, element_id: &str) -> Option<NodeIndex> {
    graph
        .node_indices()
        .find(|&idx| graph[idx].id() == element_id)
}

/// Helper to find the start node.
pub(crate) fn find_start(graph: &IRGraph) -> Option<NodeIndex> {
    graph
        .node_indices()
        .find(|&idx| matches!(&graph[idx], IRNode::Start { .. }))
}
