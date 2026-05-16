use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Contract describing what a verb (service task) reads, writes, and may raise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerbContract {
    pub task_type: String,
    pub reads_flags: HashSet<String>,
    pub writes_flags: HashSet<String>,
    /// Error codes the verb may raise. `"*"` = catch-all (satisfies any error code check).
    pub may_raise_errors: HashSet<String>,
    pub produces_correlation: Vec<CorrelationContract>,
}

/// Declares a correlation key that a verb produces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationContract {
    pub key_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Registry of verb contracts + known workflow inputs.
///
/// `known_workflow_inputs` is an allow-list of flags that are valid as workflow-level
/// inputs (e.g., flags set by the caller before the workflow starts). When L1 (flag
/// provenance) encounters a flag in this set, it emits a Warning instead of an Error.
#[derive(Debug, Clone, Default)]
pub struct ContractRegistry {
    contracts: HashMap<String, VerbContract>,
    known_workflow_inputs: HashSet<String>,
}

// ── YAML format for deserialization ──

#[derive(Debug, Deserialize)]
struct ContractRegistryYaml {
    #[serde(default)]
    known_workflow_inputs: Vec<String>,
    #[serde(default)]
    contracts: Vec<VerbContractYaml>,
}

#[derive(Debug, Deserialize)]
struct VerbContractYaml {
    task_type: String,
    #[serde(default)]
    reads_flags: Vec<String>,
    #[serde(default)]
    writes_flags: Vec<String>,
    #[serde(default)]
    may_raise_errors: Vec<String>,
    #[serde(default)]
    produces_correlation: Vec<CorrelationContractYaml>,
}

#[derive(Debug, Deserialize)]
struct CorrelationContractYaml {
    key_source: String,
    #[serde(default)]
    description: Option<String>,
}

impl ContractRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a contract for a task type. Replaces any existing contract.
    pub fn register(&mut self, contract: VerbContract) {
        self.contracts.insert(contract.task_type.clone(), contract);
    }

    /// Get the contract for a task type.
    pub fn get(&self, task_type: &str) -> Option<&VerbContract> {
        self.contracts.get(task_type)
    }

    /// Check if a contract exists for the given task type.
    pub fn has(&self, task_type: &str) -> bool {
        self.contracts.contains_key(task_type)
    }

    /// Check if a flag is in the known workflow inputs allow-list.
    pub fn is_known_input(&self, flag: &str) -> bool {
        self.known_workflow_inputs.contains(flag)
    }

    /// Add a flag to the known workflow inputs allow-list.
    pub fn add_known_input(&mut self, flag: impl Into<String>) {
        self.known_workflow_inputs.insert(flag.into());
    }

    /// Builder: set all known workflow inputs at once.
    pub fn with_known_inputs(
        mut self,
        inputs: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.known_workflow_inputs = inputs.into_iter().map(Into::into).collect();
        self
    }

    /// Parse a `ContractRegistry` from YAML.
    ///
    /// ```yaml
    /// known_workflow_inputs: [orch_high_risk, document_request_id]
    /// contracts:
    ///   - task_type: check_sanctions
    ///     reads_flags: [case_created]
    ///     writes_flags: [sanctions_clear]
    ///     may_raise_errors: [SANCTIONS_HIT, TIMEOUT]
    ///     produces_correlation:
    ///       - key_source: document_request_id
    /// ```
    pub fn from_yaml_str(yaml: &str) -> Result<Self, serde_yaml::Error> {
        let raw: ContractRegistryYaml = serde_yaml::from_str(yaml)?;
        let mut registry = ContractRegistry {
            known_workflow_inputs: raw.known_workflow_inputs.into_iter().collect(),
            contracts: HashMap::new(),
        };
        for c in raw.contracts {
            let contract = VerbContract {
                task_type: c.task_type,
                reads_flags: c.reads_flags.into_iter().collect(),
                writes_flags: c.writes_flags.into_iter().collect(),
                may_raise_errors: c.may_raise_errors.into_iter().collect(),
                produces_correlation: c
                    .produces_correlation
                    .into_iter()
                    .map(|cc| CorrelationContract {
                        key_source: cc.key_source,
                        description: cc.description,
                    })
                    .collect(),
            };
            registry.register(contract);
        }
        Ok(registry)
    }

    /// Iterate over all registered contracts.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &VerbContract)> {
        self.contracts.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_get() {
        let mut reg = ContractRegistry::new();
        reg.register(VerbContract {
            task_type: "check_sanctions".to_string(),
            reads_flags: ["case_created".to_string()].into(),
            writes_flags: ["sanctions_clear".to_string()].into(),
            may_raise_errors: ["SANCTIONS_HIT".to_string()].into(),
            produces_correlation: vec![],
        });
        assert!(reg.has("check_sanctions"));
        assert!(!reg.has("nonexistent"));
        let c = reg.get("check_sanctions").unwrap();
        assert!(c.writes_flags.contains("sanctions_clear"));
    }

    #[test]
    fn test_known_inputs() {
        let reg =
            ContractRegistry::new().with_known_inputs(["orch_high_risk", "document_request_id"]);
        assert!(reg.is_known_input("orch_high_risk"));
        assert!(reg.is_known_input("document_request_id"));
        assert!(!reg.is_known_input("unknown"));
    }

    #[test]
    fn test_from_yaml() {
        let yaml = r#"
known_workflow_inputs: [orch_high_risk, document_request_id]
contracts:
  - task_type: check_sanctions
    reads_flags: [case_created]
    writes_flags: [sanctions_clear]
    may_raise_errors: [SANCTIONS_HIT, TIMEOUT]
    produces_correlation:
      - key_source: document_request_id
        description: "Links to document request"
  - task_type: collect_docs
    reads_flags: []
    writes_flags: [docs_collected]
    may_raise_errors: ["*"]
    produces_correlation: []
"#;
        let reg = ContractRegistry::from_yaml_str(yaml).unwrap();
        assert!(reg.is_known_input("orch_high_risk"));
        assert!(reg.has("check_sanctions"));
        assert!(reg.has("collect_docs"));

        let sanctions = reg.get("check_sanctions").unwrap();
        assert!(sanctions.reads_flags.contains("case_created"));
        assert!(sanctions.writes_flags.contains("sanctions_clear"));
        assert!(sanctions.may_raise_errors.contains("SANCTIONS_HIT"));
        assert_eq!(sanctions.produces_correlation.len(), 1);
        assert_eq!(
            sanctions.produces_correlation[0].key_source,
            "document_request_id"
        );

        let docs = reg.get("collect_docs").unwrap();
        assert!(docs.may_raise_errors.contains("*"));
    }
}
