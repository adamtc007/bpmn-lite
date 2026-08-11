use super::plan::{ExecutionNode, WorkflowExecutionPlan};
use std::collections::HashSet;

/// Verify that the plan graph has a structured SESE topology using a DFS stack walk.
/// Returns Ok(()) if the graph is SESE-structured, or an error message if not.
pub fn verify_sese_nesting(plan: &WorkflowExecutionPlan) -> Result<(), String> {
    let mut visited = HashSet::new();
    let mut split_stack = Vec::new();
    let mut errors = Vec::new();

    dfs_walk(
        &plan.start_node,
        plan,
        &mut visited,
        &mut split_stack,
        &mut errors,
    );

    // Verify all splits in stack are closed
    if !split_stack.is_empty() {
        errors.push(format!(
            "Unclosed Split nodes: [{}]",
            split_stack.join(", ")
        ));
    }

    // WS-D D1: each guard escape subgraph is its own SESE region, walked
    // with a FRESH split stack — the guard fires outside the main flow's
    // split nesting (an interrupting guard has already unwound the host's
    // scope; a rearming one spawns a sibling fibre). Consequence, recorded
    // deliberately: an escape flow that merges into the main flow's own
    // Join pops an empty stack here and errors — only self-contained
    // escape subgraphs ending in their own End (the shape
    // `productions.rs`'s interrupting_timeout emits, and the only shape
    // the XML lowering's guardn-close walk supports) verify as SESE.
    // A merging escape therefore surfaces as a breach, never a silent
    // admit.
    for node in plan.nodes.values() {
        for entry in node.guard_escape_entries() {
            let mut escape_stack = Vec::new();
            dfs_walk(entry, plan, &mut visited, &mut escape_stack, &mut errors);
            if !escape_stack.is_empty() {
                errors.push(format!(
                    "Unclosed Split nodes in guard escape flow from '{entry}': [{}]",
                    escape_stack.join(", ")
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn dfs_walk(
    curr: &str,
    plan: &WorkflowExecutionPlan,
    visited: &mut HashSet<String>,
    split_stack: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    if !visited.insert(curr.to_string()) {
        if let Some(ExecutionNode::Join(jn)) = plan.nodes.get(curr) {
            if let Some(top_split) = split_stack.pop() {
                if top_split != jn.split {
                    errors.push(format!(
                        "Crossing split-join boundaries: Join '{}' corresponds to Split '{}', but nested Split '{}' is open",
                        jn.id, jn.split, top_split
                    ));
                }
            } else {
                errors.push(format!(
                    "Unmatched Join node '{}': no open Split found",
                    jn.id
                ));
            }
        }
        return;
    }

    let node = match plan.nodes.get(curr) {
        Some(n) => n,
        None => return,
    };

    match node {
        ExecutionNode::Split(sp) => {
            for flow in &sp.flows {
                let mut branch_stack = split_stack.clone();
                branch_stack.push(sp.id.clone());
                dfs_walk(&flow.next, plan, visited, &mut branch_stack, errors);
            }
        }
        ExecutionNode::Join(jn) => {
            if let Some(top_split) = split_stack.pop() {
                if top_split != jn.split {
                    errors.push(format!(
                        "Crossing split-join boundaries: Join '{}' corresponds to Split '{}', but nested Split '{}' is open",
                        jn.id, jn.split, top_split
                    ));
                }
            } else {
                errors.push(format!(
                    "Unmatched Join node '{}': no open Split found",
                    jn.id
                ));
            }
            dfs_walk(&jn.next, plan, visited, split_stack, errors);
        }
        ExecutionNode::Start(st) => {
            dfs_walk(&st.next, plan, visited, split_stack, errors);
        }
        ExecutionNode::Task(tk) => {
            dfs_walk(&tk.next, plan, visited, split_stack, errors);
        }
        ExecutionNode::Wait(w) => {
            dfs_walk(&w.next, plan, visited, split_stack, errors);
        }
        ExecutionNode::MessageWait(w) => {
            dfs_walk(&w.next, plan, visited, split_stack, errors);
        }
        ExecutionNode::End(_) => {
            split_stack.clear();
        }
        // G5.4a: linear node, same as Task/Wait/MessageWait above.
        ExecutionNode::MultiInstance(mi) => {
            dfs_walk(&mi.next, plan, visited, split_stack, errors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::plan::{
        EndExecNode, ExecutionNode, JoinExecNode, JoinMode, PlaceholderSchema, SplitExecFlow,
        SplitExecNode, SplitMode, StartExecNode, WorkflowExecutionPlan,
    };
    use std::collections::BTreeMap;

    #[test]
    fn test_invalid_sese() {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "start".to_string(),
            ExecutionNode::Start(StartExecNode {
                id: "start".to_string(),
                next: "s1".to_string(),
                span: None,
            }),
        );
        nodes.insert(
            "s1".to_string(),
            ExecutionNode::Split(SplitExecNode {
                id: "s1".to_string(),
                mode: SplitMode::Exclusive,
                routing_socket: None,
                flows: vec![
                    SplitExecFlow {
                        placeholder: None,
                        expected_value: None,
                        next: "s2".to_string(),
                    },
                    SplitExecFlow {
                        placeholder: None,
                        expected_value: None,
                        next: "j1".to_string(),
                    },
                ],
                join: "j1".to_string(),
                produces_placeholder: None,
                span: None,
            }),
        );
        nodes.insert(
            "s2".to_string(),
            ExecutionNode::Split(SplitExecNode {
                id: "s2".to_string(),
                mode: SplitMode::Exclusive,
                routing_socket: None,
                flows: vec![
                    SplitExecFlow {
                        placeholder: None,
                        expected_value: None,
                        next: "j1".to_string(),
                    },
                    SplitExecFlow {
                        placeholder: None,
                        expected_value: None,
                        next: "j2".to_string(),
                    },
                ],
                join: "j2".to_string(),
                produces_placeholder: None,
                span: None,
            }),
        );
        nodes.insert(
            "j1".to_string(),
            ExecutionNode::Join(JoinExecNode {
                id: "j1".to_string(),
                mode: JoinMode::Exclusive,
                split: "s1".to_string(),
                next: "j2".to_string(),
                span: None,
            }),
        );
        nodes.insert(
            "j2".to_string(),
            ExecutionNode::Join(JoinExecNode {
                id: "j2".to_string(),
                mode: JoinMode::Exclusive,
                split: "s2".to_string(),
                next: "end".to_string(),
                span: None,
            }),
        );
        nodes.insert(
            "end".to_string(),
            ExecutionNode::End(EndExecNode {
                id: "end".to_string(),
                status: "Completed".to_string(),
                span: None,
            }),
        );

        let plan = WorkflowExecutionPlan {
            workflow_id: "crossing-splits".to_string(),
            nodes,
            start_node: "start".to_string(),
            placeholder_schema: PlaceholderSchema::default(),
            closure_manifest: None,
            regime_version: None,
            mathematically_proved: true,
            unsafe_breeches: vec![],
        };

        let res = verify_sese_nesting(&plan);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.contains("Crossing split-join boundaries") || err.contains("Unclosed Split"));
    }

    #[test]
    fn test_valid_sese() {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "start".to_string(),
            ExecutionNode::Start(StartExecNode {
                id: "start".to_string(),
                next: "s1".to_string(),
                span: None,
            }),
        );
        nodes.insert(
            "s1".to_string(),
            ExecutionNode::Split(SplitExecNode {
                id: "s1".to_string(),
                mode: SplitMode::Exclusive,
                routing_socket: None,
                flows: vec![
                    SplitExecFlow {
                        placeholder: None,
                        expected_value: None,
                        next: "j1".to_string(),
                    },
                    SplitExecFlow {
                        placeholder: None,
                        expected_value: None,
                        next: "j1".to_string(),
                    },
                ],
                join: "j1".to_string(),
                produces_placeholder: None,
                span: None,
            }),
        );
        nodes.insert(
            "j1".to_string(),
            ExecutionNode::Join(JoinExecNode {
                id: "j1".to_string(),
                mode: JoinMode::Exclusive,
                split: "s1".to_string(),
                next: "end".to_string(),
                span: None,
            }),
        );
        nodes.insert(
            "end".to_string(),
            ExecutionNode::End(EndExecNode {
                id: "end".to_string(),
                status: "Completed".to_string(),
                span: None,
            }),
        );

        let plan = WorkflowExecutionPlan {
            workflow_id: "valid-sese".to_string(),
            nodes,
            start_node: "start".to_string(),
            placeholder_schema: PlaceholderSchema::default(),
            closure_manifest: None,
            regime_version: None,
            mathematically_proved: true,
            unsafe_breeches: vec![],
        };

        let res = verify_sese_nesting(&plan);
        assert!(res.is_ok(), "Expected valid SESE: {:?}", res);
    }
}
