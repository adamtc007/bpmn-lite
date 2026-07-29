use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use bpmn_lite_compiler::dsl::{AstMutator, JoinAst, JoinModeAst, NodeAst, ToSexpr, parse_workflow_str};
use dsl_manifest::{DecisionEntry, DecisionOutput, Manifest, Signature, VerbEntry};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum FixAction {
    ImportManifest {
        domain: String,
    },
    AddVerbStub {
        domain: String,
        verb: String,
    },
    AddDecisionStub {
        domain: String,
        decision: String,
    },
    WireDeadEnd {
        node_id: String,
        target_id: String,
    },
    InjectJoinNode {
        split_id: String,
        join_id: String,
        predecessor_id: String,
        next_id: String,
    },
    RemoveUnusedNode {
        node_id: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct DiagnosticResolution {
    pub error_id: String,
    pub title: String,
    pub description: String,
    pub action: FixAction,
}

/// Helper to locate a manifest file for a given domain in the manifests directory.
fn find_manifest_path(manifests_dir: &Path, domain: &str) -> Option<PathBuf> {
    if let Ok(entries) = fs::read_dir(manifests_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                    if filename.starts_with(domain) && filename.ends_with(".yaml") {
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

/// Executes an auto-fix action on the S-expression source code and/or manifest files.
pub fn execute_autofix(
    source_code: &str,
    action: &FixAction,
    manifests_dir: &Path,
) -> Result<String, String> {
    match action {
        FixAction::ImportManifest { domain } => {
            // Ensure manifest file exists. If not, generate a stub.
            let path = find_manifest_path(manifests_dir, domain)
                .unwrap_or_else(|| manifests_dir.join(format!("{}-v1.0.0.yaml", domain)));
            if !path.exists() {
                let stub_yaml = format!(
                    r#"manifest_version: "1.0"
domain: "{}"
catalogue_version: "v1.0.0"
generated_at: "2026-06-04T12:00:00Z"
verbs: []
decisions: []
types: []
"#,
                    domain
                );
                fs::write(&path, stub_yaml)
                    .map_err(|e| format!("Failed to create stub manifest: {}", e))?;
            }
            Ok(source_code.to_string())
        }
        FixAction::AddVerbStub { domain, verb } => {
            // Find manifest or create stub
            let path = find_manifest_path(manifests_dir, domain)
                .unwrap_or_else(|| manifests_dir.join(format!("{}-v1.0.0.yaml", domain)));

            let mut manifest = if path.exists() {
                Manifest::load_from_path(&path)
                    .map_err(|e| format!("Failed to load manifest: {}", e))?
            } else {
                let default_manifest_yaml = format!(
                    r#"manifest_version: "1.0"
domain: "{}"
catalogue_version: "v1.0.0"
generated_at: "2026-06-04T12:00:00Z"
"#,
                    domain
                );
                Manifest::load_from_yaml(&default_manifest_yaml)
                    .map_err(|e| format!("Failed to create default manifest: {}", e))?
            };

            // Check if verb already exists (stripped namespace if any)
            let verb_id = if verb.contains(':') {
                verb.split(':').collect::<Vec<&str>>()[1].to_string()
            } else {
                verb.clone()
            };

            if manifest.lookup_verb(&verb_id).is_none() {
                manifest.add_verb(VerbEntry {
                    id: verb_id,
                    signature: Signature {
                        inputs: vec![],
                        output: None,
                    },
                    effect_class: "idempotent_ensure".to_string(),
                    coordination_policy: None,
                    transaction_policy: None,
                    resource_dependencies: vec![],
                    fsm_applicability: None,
                    authority_required: format!("{}.write", domain),
                    description: Some("Auto-generated mock stub".to_string()),
                });
                let yaml = manifest
                    .to_yaml()
                    .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
                fs::write(&path, yaml).map_err(|e| format!("Failed to write manifest: {}", e))?;
            }

            Ok(source_code.to_string())
        }
        FixAction::AddDecisionStub { domain, decision } => {
            let path = find_manifest_path(manifests_dir, domain)
                .unwrap_or_else(|| manifests_dir.join(format!("{}-v1.0.0.yaml", domain)));

            let mut manifest = if path.exists() {
                Manifest::load_from_path(&path)
                    .map_err(|e| format!("Failed to load manifest: {}", e))?
            } else {
                let default_manifest_yaml = format!(
                    r#"manifest_version: "1.0"
domain: "{}"
catalogue_version: "v1.0.0"
generated_at: "2026-06-04T12:00:00Z"
"#,
                    domain
                );
                Manifest::load_from_yaml(&default_manifest_yaml)
                    .map_err(|e| format!("Failed to create default manifest: {}", e))?
            };

            let decision_id = if decision.contains(':') {
                decision.split(':').collect::<Vec<&str>>()[1].to_string()
            } else {
                decision.clone()
            };

            if manifest.lookup_decision(&decision_id).is_none() {
                manifest.add_decision(DecisionEntry {
                    id: decision_id,
                    inputs: vec![],
                    output: DecisionOutput {
                        type_name: "string".to_string(),
                        enum_values: vec![],
                    },
                    description: Some("Auto-generated mock stub".to_string()),
                });
                let yaml = manifest
                    .to_yaml()
                    .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
                fs::write(&path, yaml).map_err(|e| format!("Failed to write manifest: {}", e))?;
            }

            Ok(source_code.to_string())
        }
        FixAction::WireDeadEnd { node_id, target_id } => {
            let mut workflow = parse_workflow_str(source_code)?;
            {
                let mut mutator = AstMutator::new(&mut workflow);
                mutator.rewire_next(node_id, target_id)?;
            }
            Ok(workflow.to_sexpr(0))
        }
        FixAction::InjectJoinNode {
            split_id,
            join_id,
            predecessor_id,
            next_id,
        } => {
            let mut workflow = parse_workflow_str(source_code)?;
            {
                let mut mutator = AstMutator::new(&mut workflow);
                let join_node = NodeAst::Join(JoinAst {
                    id: join_id.clone(),
                    mode: JoinModeAst::Xor,
                    split: split_id.clone(),
                    next: next_id.clone(),
                    span: bpmn_lite_types::SourceSpan::new(0, 0),
                });
                mutator.insert_after(predecessor_id, join_node)?;
            }
            Ok(workflow.to_sexpr(0))
        }
        FixAction::RemoveUnusedNode { node_id } => {
            let mut workflow = parse_workflow_str(source_code)?;
            {
                let removed_next = {
                    let mut mutator = AstMutator::new(&mut workflow);
                    let node = mutator
                        .find_node_mut(node_id)
                        .ok_or_else(|| format!("Node '{}' not found for removal", node_id))?;
                    match node {
                        NodeAst::Start(_) => return Err("Cannot remove start event".to_string()),
                        NodeAst::End(_) => return Err("Cannot remove end event".to_string()),
                        NodeAst::Task(t) => t.next.clone(),
                        NodeAst::Join(j) => j.next.clone(),
                        NodeAst::Loop(l) => l.next.clone(),
                        NodeAst::Split(_) => {
                            return Err("Cannot automatically remove split node safely".to_string())
                        }
                    }
                };

                let pred_ids = find_all_predecessors(&workflow.nodes, node_id);
                {
                    let mut mutator = AstMutator::new(&mut workflow);
                    for pred in pred_ids {
                        mutator.rewire_next(&pred, &removed_next)?;
                    }
                    mutator
                        .remove_node(node_id)
                        .ok_or_else(|| format!("Failed to remove node '{}'", node_id))?;
                }
            }
            Ok(workflow.to_sexpr(0))
        }
    }
}

fn find_all_predecessors(nodes: &[NodeAst], target_id: &str) -> Vec<String> {
    let mut preds = Vec::new();
    find_all_predecessors_rec(nodes, target_id, &mut preds);
    preds
}

fn find_all_predecessors_rec(nodes: &[NodeAst], target_id: &str, acc: &mut Vec<String>) {
    for node in nodes {
        match node {
            NodeAst::Start(s) => {
                if s.next == target_id {
                    acc.push(s.id.clone());
                }
            }
            NodeAst::Task(t) => {
                if t.next == target_id {
                    acc.push(t.id.clone());
                }
            }
            NodeAst::Join(j) => {
                if j.next == target_id {
                    acc.push(j.id.clone());
                }
            }
            NodeAst::Loop(l) => {
                if l.next == target_id {
                    acc.push(l.id.clone());
                }
                find_all_predecessors_rec(&l.body, target_id, acc);
            }
            NodeAst::Split(s) => {
                for flow in &s.flows {
                    if flow.next == target_id {
                        acc.push(s.id.clone());
                    }
                }
            }
            NodeAst::End(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_autofix_wire_dead_end() {
        let bad_dsl = r#"(workflow test
  (start-event :id start :next task1)
  (service-task :id task1 :verb ob-poc:cbu.create :next dead)
  (end-event :id end :status "completed")
)"#;
        let action = FixAction::WireDeadEnd {
            node_id: "task1".to_string(),
            target_id: "end".to_string(),
        };
        let fixed = execute_autofix(bad_dsl, &action, Path::new("")).unwrap();
        assert!(fixed.contains("(service-task :id task1 :verb ob-poc:cbu.create :next end)"));
    }
}
