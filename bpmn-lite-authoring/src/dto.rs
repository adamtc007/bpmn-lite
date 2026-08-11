use bpmn_lite_compiler::{FfiInputBinding, GatewayDirection, RetryPolicyDecl};
use serde::{Deserialize, Serialize};

// ── Helper defaults for serde ──

fn default_corr() -> String {
    "instance_id".to_string()
}

fn default_true() -> bool {
    true
}

fn is_false(v: &bool) -> bool {
    !v
}

// ── Top-level DTO ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowGraphDto {
    pub id: String,
    #[serde(default)]
    pub meta: Option<TemplateMeta>,
    pub nodes: Vec<NodeDto>,
    pub edges: Vec<EdgeDto>,
    /// G5.1 — workflow-level default guard failure budget. Mirrors
    /// `DesignerDag.default_guard_budget`; validated (non-zero) at
    /// `lower_with_default` time, not here.
    #[serde(default)]
    pub default_guard_budget: Option<u32>,
    /// G5.2 — workflow-level default retry policy. Mirrors
    /// `DesignerDag.default_retry_policy`; raw/unvalidated (see
    /// `RetryPolicyDecl`'s doc comment).
    #[serde(default)]
    pub default_retry_policy: Option<RetryPolicyDecl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateMeta {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

// ── Edge ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeDto {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<FlagCondition>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_error: Option<ErrorEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagCondition {
    pub flag: String,
    pub op: FlagOp,
    pub value: FlagValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FlagOp {
    #[serde(rename = "==")]
    Eq,
    #[serde(rename = "!=")]
    Neq,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = ">")]
    Gt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FlagValue {
    Bool(bool),
    I64(i64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEdge {
    pub error_code: String,
    #[serde(default)]
    pub retries: u32,
}

// ── Node (tagged enum) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum NodeDto {
    Start {
        id: String,
    },
    End {
        id: String,
        #[serde(default)]
        terminate: bool,
    },
    ServiceTask {
        id: String,
        task_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bpmn_id: Option<String>,
    },
    ExclusiveGateway {
        id: String,
    },
    ParallelGateway {
        id: String,
        direction: GatewayDirection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        join: Option<String>,
    },
    InclusiveGateway {
        id: String,
        direction: GatewayDirection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        join: Option<String>,
    },
    TimerWait {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deadline_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cycle_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cycle_max: Option<u32>,
    },
    MessageWait {
        id: String,
        name: String,
        #[serde(default = "default_corr")]
        corr_key_source: String,
    },
    HumanWait {
        id: String,
        task_kind: String,
        #[serde(default = "default_corr")]
        corr_key_source: String,
    },
    RaceWait {
        id: String,
        arms: Vec<RaceArm>,
    },
    BoundaryTimer {
        id: String,
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deadline_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cycle_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cycle_max: Option<u32>,
        #[serde(default = "default_true")]
        interrupting: bool,
    },
    BoundaryError {
        id: String,
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
    },
    /// §18 ruling K — a parallel multi-instance activity. See
    /// `bpmn_lite_compiler::IRNode::MultiInstance`'s doc comment for
    /// the full design and the array/collection-value scope note.
    MultiInstance {
        id: String,
        task_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bpmn_id: Option<String>,
        /// Data-object/flag name carrying the collection's actual
        /// `Value::Array` data (§18 ruling K Part 2 — length is derived
        /// from it, not tracked as a separate `I64` flag).
        collection_flag: String,
        declared_max: u32,
        /// G4.0 — per-element input bindings; see
        /// `bpmn_lite_compiler::IRNode::MultiInstance::inputs`'s doc
        /// comment. A `VarRef` here is what the G4.1 parameter manifest
        /// classifies as element-scoped.
        #[serde(default)]
        inputs: Vec<FfiInputBinding>,
    },
    /// A BPMN data-object declaration (save-as-template wiring,
    /// 2026-07-30): structural, no sequence-flow edges — see
    /// `bpmn_lite_compiler::IRNode::DataObject`. Added so graph-authored
    /// sessions (whose `DesignerDag`s carry data objects, e.g. a
    /// `MessageWait`'s correlation source) can round-trip through the
    /// DTO on the `ir_to_dto` path. Reuses `bpmn_lite_types`'
    /// serde-ready `DataObjectType`/`DataObjectRole` verbatim.
    DataObject {
        id: String,
        name: String,
        type_decl: bpmn_lite_types::DataObjectType,
        #[serde(default = "default_internal_role")]
        role: bpmn_lite_types::DataObjectRole,
    },
}

fn default_internal_role() -> bpmn_lite_types::DataObjectRole {
    bpmn_lite_types::DataObjectRole::Internal
}

// ── RaceArm ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaceArm {
    pub arm_id: String,
    pub kind: RaceArmKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RaceArmKind {
    Timer {
        duration_ms: u64,
        #[serde(default = "default_true")]
        interrupting: bool,
    },
    Message {
        name: String,
        #[serde(default = "default_corr")]
        corr_key_source: String,
    },
}

// ── NodeDto helpers ──

impl NodeDto {
    /// Returns the id regardless of variant.
    pub fn id(&self) -> &str {
        match self {
            NodeDto::Start { id } => id,
            NodeDto::End { id, .. } => id,
            NodeDto::ServiceTask { id, .. } => id,
            NodeDto::ExclusiveGateway { id } => id,
            NodeDto::ParallelGateway { id, .. } => id,
            NodeDto::InclusiveGateway { id, .. } => id,
            NodeDto::TimerWait { id, .. } => id,
            NodeDto::MessageWait { id, .. } => id,
            NodeDto::HumanWait { id, .. } => id,
            NodeDto::RaceWait { id, .. } => id,
            NodeDto::BoundaryTimer { id, .. } => id,
            NodeDto::BoundaryError { id, .. } => id,
            NodeDto::MultiInstance { id, .. } => id,
            NodeDto::DataObject { id, .. } => id,
        }
    }
}
