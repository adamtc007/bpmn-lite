use bpmn_lite_compiler::ir::GatewayDirection;
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
        }
    }
}

impl WorkflowGraphDto {
    /// Deterministic JSON: clone, sort nodes by id, sort edges by (from, to), serialize to pretty JSON.
    pub fn deterministic_json(&self) -> String {
        let mut dto = self.clone();
        dto.nodes.sort_by(|a, b| a.id().cmp(b.id()));
        dto.edges
            .sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.to.cmp(&b.to)));
        serde_json::to_string_pretty(&dto).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-AUTH-8: Deterministic JSON — same DTO with different node/edge order → identical JSON.
    #[test]
    fn t_auth_8_deterministic_json() {
        let dto1 = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "a".to_string(),
                    task_type: "do_work".to_string(),
                    bpmn_id: None,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "start".to_string(),
                    to: "a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        let dto2 = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "a".to_string(),
                    task_type: "do_work".to_string(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        assert_eq!(dto1.deterministic_json(), dto2.deterministic_json());
    }
}
