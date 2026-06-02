use std::collections::HashSet;
use super::plan::{WorkflowExecutionPlan, ExecutionNode};

/// Verify that the plan graph has a structured SESE topology using a DFS stack walk.
/// Returns Ok(()) if the graph is SESE-structured, or an error message if not.
pub fn verify_sese_nesting(plan: &WorkflowExecutionPlan) -> Result<(), String> {
    let mut visited = HashSet::new();
    let mut split_stack = Vec::new();
    let mut errors = Vec::new();

    dfs_walk(&plan.start_node, plan, &mut visited, &mut split_stack, &mut errors);

    // Verify all splits in stack are closed
    if !split_stack.is_empty() {
        errors.push(format!(
            "Unclosed Split nodes: [{}]",
            split_stack.join(", ")
        ));
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
        ExecutionNode::Loop(lp) => {
            for child_id in &lp.body {
                let mut child_stack = split_stack.clone();
                dfs_walk(child_id, plan, visited, &mut child_stack, errors);
            }
            dfs_walk(&lp.next, plan, visited, split_stack, errors);
        }
        ExecutionNode::Start(st) => {
            dfs_walk(&st.next, plan, visited, split_stack, errors);
        }
        ExecutionNode::Task(tk) => {
            dfs_walk(&tk.next, plan, visited, split_stack, errors);
        }
        ExecutionNode::End(_) => {
            split_stack.clear();
        }
    }
}
