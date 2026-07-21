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
impl ContractRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_manifest(m: &dsl_manifest::Manifest) -> Self {
        let mut registry = ContractRegistry::default();
        for verb in &m.verbs {
            let mut reads_flags = HashSet::new();
            let mut writes_flags = HashSet::new();

            for input in &verb.signature.inputs {
                if input.type_name.to_lowercase() == "bool"
                    || input.type_name.to_lowercase() == "boolean"
                {
                    reads_flags.insert(input.name.clone());
                }
            }
            if let Some(ref output) = verb.signature.output {
                if let Some(ref produces) = output.produces {
                    writes_flags.insert(produces.clone());
                }
            }

            registry.register(VerbContract {
                task_type: verb.id.clone(),
                reads_flags,
                writes_flags,
                may_raise_errors: HashSet::from(["*".to_string()]),
                produces_correlation: vec![],
            });
        }
        registry
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
        let mut reg = ContractRegistry::new();
        reg.add_known_input("orch_high_risk");
        reg.add_known_input("document_request_id");
        assert!(reg.is_known_input("orch_high_risk"));
        assert!(reg.is_known_input("document_request_id"));
        assert!(!reg.is_known_input("unknown"));
    }
}
