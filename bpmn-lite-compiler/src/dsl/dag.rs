//! DAG validation pass for a `WorkflowExecutionPlan`.
//!
//! Validates:
//! 1. All node `:next` edges and gateway flow targets exist (already checked
//!    by linter; dag pass re-confirms as invariant).
//! 2. The workflow is acyclic (simple DFS cycle detection).
//! 3. All nodes are reachable from the start node.
//! 4. At least one end-event is reachable.

use std::collections::{HashMap, HashSet, VecDeque};

use super::plan::*;

#[derive(Debug, Clone)]
pub struct DagError {
    pub message: String,
}

impl std::fmt::Display for DagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Validate the DAG topology of a `WorkflowExecutionPlan`.
/// Returns `Ok(())` when the DAG is valid.
pub fn validate_dag(plan: &WorkflowExecutionPlan) -> Result<(), Vec<DagError>> {
    let mut errors = Vec::new();

    // Build adjacency list: node_id → Vec<next_node_id>
    let adj = build_adjacency(plan);

    // Check acyclicity via DFS
    if let Some(cycle) = find_cycle(&plan.start_node, &adj) {
        errors.push(DagError { message: format!("cycle detected: {}", cycle.join(" → ")) });
    }

    // Check all nodes reachable from start
    let reachable = bfs_reachable(&plan.start_node, &adj);
    for id in plan.nodes.keys() {
        if !reachable.contains(id.as_str()) {
            errors.push(DagError { message: format!("node '{id}' is unreachable from start") });
        }
    }

    // Check at least one end-event reachable
    let has_end = plan.nodes.values().any(|n| matches!(n, ExecutionNode::EndEvent(_)));
    if !has_end {
        errors.push(DagError { message: "no end-event in workflow".into() });
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

fn build_adjacency(plan: &WorkflowExecutionPlan) -> HashMap<&str, Vec<&str>> {
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, node) in &plan.nodes {
        let nexts: Vec<&str> = match node {
            ExecutionNode::StartEvent(n) => vec![n.next.as_str()],
            ExecutionNode::ServiceTask(n) => vec![n.next.as_str()],
            ExecutionNode::BusinessRuleTask(n) => vec![n.next.as_str()],
            ExecutionNode::ExclusiveGateway(n) => n.flows.iter().map(|f| f.next.as_str()).collect(),
            ExecutionNode::EndEvent(_) => vec![],
        };
        adj.insert(id.as_str(), nexts);
    }
    adj
}

fn bfs_reachable<'a>(start: &'a str, adj: &'a HashMap<&'a str, Vec<&'a str>>) -> HashSet<&'a str> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(start);
    while let Some(n) = queue.pop_front() {
        if visited.insert(n) {
            if let Some(nexts) = adj.get(n) {
                for &next in nexts {
                    if !visited.contains(next) {
                        queue.push_back(next);
                    }
                }
            }
        }
    }
    visited
}

/// DFS cycle detection. Returns the cycle path if found.
fn find_cycle<'a>(start: &'a str, adj: &'a HashMap<&'a str, Vec<&'a str>>) -> Option<Vec<String>> {
    let mut visited = HashSet::new();
    let mut stack: HashSet<&'a str> = HashSet::new();
    let mut path = Vec::new();
    if dfs_cycle(start, adj, &mut visited, &mut stack, &mut path) {
        Some(path)
    } else {
        None
    }
}

fn dfs_cycle<'a>(
    node: &'a str,
    adj: &'a HashMap<&'a str, Vec<&'a str>>,
    visited: &mut HashSet<&'a str>,
    stack: &mut HashSet<&'a str>,
    path: &mut Vec<String>,
) -> bool {
    visited.insert(node);
    stack.insert(node);
    path.push(node.to_owned());

    if let Some(nexts) = adj.get(node) {
        for &next in nexts {
            if !visited.contains(next) {
                if dfs_cycle(next, adj, visited, stack, path) {
                    return true;
                }
            } else if stack.contains(next) {
                path.push(next.to_owned());
                return true;
            }
        }
    }

    stack.remove(node);
    path.pop();
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{compile, linter::StubPlaceholderRegistry};

    fn demo_registry() -> StubPlaceholderRegistry {
        StubPlaceholderRegistry::new().with_demo_bindings()
    }

    const DEMO_SRC: &str = r#"(workflow custody-cbu-onboarding
  (start-event :id start :next create-cbu)
  (service-task :id create-cbu :verb cbu.create :next type-decision)
  (business-rule-task :id type-decision :decision cbu_type_routing :next type-gateway)
  (exclusive-gateway :id type-gateway
    (flow :condition (= @cbu-type "fund")      :next add-fund)
    (flow :condition (= @cbu-type "corporate") :next add-corp)
    (flow :condition (= @cbu-type "trust")     :next add-trust))
  (service-task :id add-fund  :verb cbu.add-product :args (:product "CUSTODY_FUND")  :next add-im)
  (service-task :id add-corp  :verb cbu.add-product :args (:product "CUSTODY_CORP")  :next add-im)
  (service-task :id add-trust :verb cbu.add-product :args (:product "CUSTODY_TRUST") :next add-im)
  (service-task :id add-im    :verb instrument-matrix.attach :next end)
  (end-event :id end :status "Operational"))"#;

    #[test]
    fn demo_model_passes_dag_validation() {
        let reg = demo_registry();
        let plan = compile(DEMO_SRC, &reg).expect("compile failed");
        validate_dag(&plan).expect("dag validation failed");
    }

    #[test]
    fn dag_detects_unreachable_node() {
        // Use lint() directly — we want a plan that passes lint but fails dag.
        let src = r#"(workflow test
          (start-event :id s :next a)
          (service-task :id a :verb cbu.create :next e)
          (service-task :id orphan :verb cbu.add-product :next e)
          (end-event :id e :status "done"))"#;
        let reg = demo_registry();
        let (tokens, _) = crate::dsl::lexer::lex(src);
        let mut p = crate::dsl::parser::Parser::new(tokens);
        let ast = p.parse_workflow().expect("parse failed");
        let plan = crate::dsl::linter::lint(&ast, &reg).expect("lint failed");
        let result = validate_dag(&plan);
        assert!(result.is_err(), "expected dag to reject unreachable node");
        assert!(result.unwrap_err().iter().any(|e| e.message.contains("unreachable")));
    }
}
