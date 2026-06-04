use crate::dsl::ast::*;
use bpmn_lite_types::SourceSpan;

#[derive(Debug, Clone)]
pub struct XorBranchConfig {
    pub condition_value: String,
    pub target_next: String,
}

/// Generates a LoopAst wrapping a task as its SESE body block.
pub fn create_bounded_retry_macro(
    target_task: TaskAst,
    retry_ceiling: u32,
    loop_id: &str,
    exit_next: &str,
) -> LoopAst {
    let mut body_task = target_task.clone();
    body_task.next = loop_id.to_string(); 
    
    LoopAst {
        id: loop_id.to_string(),
        ceiling: retry_ceiling,
        body: vec![NodeAst::Task(body_task)],
        next: exit_next.to_string(),
        span: SourceSpan::new(0, 0),
    }
}

/// Helper to generate paired Split and Join AST structures.
pub fn create_xor_split_join(
    split_id: &str,
    placeholder: &str,
    branches: Vec<XorBranchConfig>,
    join_id: &str,
    join_next: &str,
) -> (SplitAst, JoinAst) {
    let flows = branches.into_iter()
        .map(|b| SplitFlowAst {
            condition: Some(ConditionAst::Eq {
                placeholder: placeholder.to_string(),
                value: b.condition_value,
            }),
            next: b.target_next,
        })
        .collect();

    let split = SplitAst {
        id: split_id.to_string(),
        mode: SplitModeAst::Xor,
        plug: None,
        flows,
        join: join_id.to_string(),
        span: SourceSpan::new(0, 0),
    };

    let join = JoinAst {
        id: join_id.to_string(),
        mode: JoinModeAst::Xor,
        split: split_id.to_string(),
        next: join_next.to_string(),
        span: SourceSpan::new(0, 0),
    };

    (split, join)
}

pub fn create_parallel_split_join(
    split_id: &str,
    branch_entry_nodes: Vec<String>,
    join_id: &str,
    join_next: &str,
) -> (SplitAst, JoinAst) {
    let flows = branch_entry_nodes.into_iter()
        .map(|node_id| SplitFlowAst {
            condition: None,
            next: node_id,
        })
        .collect();

    let split = SplitAst {
        id: split_id.to_string(),
        mode: SplitModeAst::And,
        plug: None,
        flows,
        join: join_id.to_string(),
        span: SourceSpan::new(0, 0),
    };

    let join = JoinAst {
        id: join_id.to_string(),
        mode: JoinModeAst::And,
        split: split_id.to_string(),
        next: join_next.to_string(),
        span: SourceSpan::new(0, 0),
    };

    (split, join)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CustomMacroConfig {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub template: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MacroConfigList {
    pub macros: Vec<CustomMacroConfig>,
}

impl MacroConfigList {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let contents = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read macro config file: {}", e))?;
        serde_yaml::from_str(&contents)
            .map_err(|e| format!("Failed to parse macro config YAML: {}", e))
    }

    pub fn load_from_str(yaml_str: &str) -> Result<Self, String> {
        serde_yaml::from_str(yaml_str)
            .map_err(|e| format!("Failed to parse macro config YAML: {}", e))
    }
}

impl CustomMacroConfig {
    pub fn instantiate(&self, params: &HashMap<String, String>) -> Result<NodeAst, String> {
        let mut instantiated_str = self.template.clone();
        for (k, v) in params {
            let placeholder = format!("%{}%", k);
            instantiated_str = instantiated_str.replace(&placeholder, v);
        }
        let bytes = instantiated_str.as_bytes();
        let mut idx = 0;
        while idx < bytes.len() {
            if bytes[idx] == b'%' {
                let mut end = idx + 1;
                while end < bytes.len() && bytes[end] != b'%' {
                    end += 1;
                }
                if end < bytes.len() {
                    let name_bytes = &bytes[idx + 1..end];
                    if !name_bytes.is_empty() && name_bytes.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
                        if let Ok(name) = std::str::from_utf8(name_bytes) {
                            if !params.contains_key(name) {
                                return Err(format!("Unreplaced placeholder: %{}%", name));
                            }
                        }
                    }
                    idx = end + 1;
                    continue;
                }
            }
            idx += 1;
        }
        crate::dsl::parser::parse_node_str(&instantiated_str)
    }
}

use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_macro_loading_and_instantiation() {
        let yaml = r#"
macros:
  - id: "support_alert"
    name: "Support Alert"
    description: "Inserts a support alert task"
    template: "(service-task :id %id% :verb support.alert :next %next%)"
"#;
        let config = MacroConfigList::load_from_str(yaml).unwrap();
        assert_eq!(config.macros.len(), 1);
        let macro_config = &config.macros[0];
        assert_eq!(macro_config.id, "support_alert");

        let mut params = HashMap::new();
        params.insert("id".to_string(), "my-alert-task".to_string());
        params.insert("next".to_string(), "end".to_string());

        let node = macro_config.instantiate(&params).unwrap();
        match node {
            NodeAst::Task(t) => {
                assert_eq!(t.id, "my-alert-task");
                assert_eq!(t.plug, "support.alert");
                assert_eq!(t.next, "end");
            }
            _ => panic!("Expected task node"),
        }
    }
}

