use std::collections::{HashMap, HashSet, VecDeque};
use super::plan::{WorkflowExecutionPlan, ExecutionNode, DeliveryMode, JoinMode, SplitMode};
use super::linter::{PlaceholderRegistry, BindingDecl};
use super::rpst::verify_sese_nesting;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub node_id: String,
    pub message: String,
    pub missing_placeholder: Option<String>,
}

pub fn validate_path_family(
    plan: &WorkflowExecutionPlan,
    registry: &dyn PlaceholderRegistry,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // ── L1: Reachability check ──
    let mut adj = HashMap::new();
    let mut incoming = HashMap::new();
    for (id, node) in &plan.nodes {
        let nexts = match node {
            ExecutionNode::Start(n) => vec![n.next.as_str()],
            ExecutionNode::Task(n) => vec![n.next.as_str()],
            ExecutionNode::Split(n) => n.flows.iter().map(|f| f.next.as_str()).collect(),
            ExecutionNode::Join(n) => vec![n.next.as_str()],
            ExecutionNode::Loop(n) => {
                let mut v = vec![n.next.as_str()];
                if let Some(first) = n.body.first() {
                    v.push(first.as_str());
                }
                v
            }
            ExecutionNode::End(_) => vec![],
        };
        for &next in &nexts {
            incoming.entry(next.to_string()).or_insert_with(Vec::new).push(id.clone());
        }
        adj.insert(id.clone(), nexts);
    }

    let mut reached = HashSet::new();
    let mut q = std::collections::VecDeque::new();
    if plan.nodes.contains_key(&plan.start_node) {
        q.push_back(plan.start_node.clone());
    }
    while let Some(curr) = q.pop_front() {
        if reached.insert(curr.clone()) {
            if let Some(nexts) = adj.get(&curr) {
                for &next in nexts {
                    q.push_back(next.to_string());
                }
            }
        }
    }

    for id in plan.nodes.keys() {
        if !reached.contains(id) {
            diagnostics.push(Diagnostic {
                node_id: id.clone(),
                message: format!("node '{}' is unreachable from start", id),
                missing_placeholder: None,
            });
        }
    }

    // ── L2: Co-reachability check ──
    let mut coreached = HashSet::new();
    let mut q_back = std::collections::VecDeque::new();
    let end_nodes = plan.end_nodes();
    for end_node in &end_nodes {
        q_back.push_back(end_node.to_string());
    }
    while let Some(curr) = q_back.pop_front() {
        if coreached.insert(curr.clone()) {
            if let Some(preds) = incoming.get(&curr) {
                for pred in preds {
                    q_back.push_back(pred.clone());
                }
            }
        }
    }

    for id in plan.nodes.keys() {
        if !coreached.contains(id) {
            diagnostics.push(Diagnostic {
                node_id: id.clone(),
                message: format!("node '{}' is a dead end (cannot reach any end-event)", id),
                missing_placeholder: None,
            });
        }
    }

    // Call Activity existence, co-reachability deadlock, and recursion checks:
    let mut visited_workflows = HashSet::new();
    for (id, node) in &plan.nodes {
        if let ExecutionNode::Task(t) = node {
            if t.plug.len() == 64 && t.plug.chars().all(|c| c.is_ascii_hexdigit()) {
                if !registry.workflow_exists(&t.plug) {
                    diagnostics.push(Diagnostic {
                        node_id: id.clone(),
                        message: format!(
                            "Task '{}' invokes child workflow '{}' which does not exist in registry",
                            id, t.plug
                        ),
                        missing_placeholder: None,
                    });
                } else {
                    if t.delivery_mode == DeliveryMode::Blocking
                        && !registry.workflow_satisfies_l2(&t.plug)
                    {
                        diagnostics.push(Diagnostic {
                            node_id: id.clone(),
                            message: format!(
                                "Task '{}' invokes blocking child workflow '{}' which fails co-reachability (potential deadlock)",
                                id, t.plug
                            ),
                            missing_placeholder: None,
                        });
                    }

                    let mut path = vec![t.plug.clone()];
                    if has_workflow_cycle(&t.plug, registry, &mut visited_workflows, &mut path) {
                        diagnostics.push(Diagnostic {
                            node_id: id.clone(),
                            message: format!(
                                "Task '{}' detects cyclic child workflow recursion: {}",
                                id, path.join(" -> ")
                            ),
                            missing_placeholder: None,
                        });
                    }
                }
            }
        }
    }

    // ── L3: SESE balance check ──
    if let Err(sese_err) = verify_sese_nesting(plan) {
        diagnostics.push(Diagnostic {
            node_id: plan.start_node.clone(),
            message: format!("SESE balance failed: {sese_err}"),
            missing_placeholder: None,
        });
    }

    // ── L6: Bounded loops validation ──
    let mut enclosing_loops: HashMap<String, Vec<String>> = HashMap::new();
    for (id, node) in &plan.nodes {
        if let ExecutionNode::Loop(lp) = node {
            if lp.ceiling == 0 {
                diagnostics.push(Diagnostic {
                    node_id: id.clone(),
                    message: format!("Loop '{}' lacks a finite ceiling", id),
                    missing_placeholder: None,
                });
            }
            for child_id in &lp.body {
                enclosing_loops.entry(child_id.clone()).or_default().push(id.clone());
            }
        }
    }

    // Back-edge check: if node's successor points back to a Loop head,
    // verify that the Loop head is an enclosing loop of this node.
    for (id, node) in &plan.nodes {
        let successors = adj.get(id).cloned().unwrap_or_default();
        for succ in successors {
            if let Some(ExecutionNode::Loop(_)) = plan.nodes.get(succ) {
                let enclosers = enclosing_loops.get(id);
                let is_enclosed = enclosers.map(|v| v.contains(&succ.to_string())).unwrap_or(false);
                if !is_enclosed {
                    diagnostics.push(Diagnostic {
                        node_id: id.clone(),
                        message: format!(
                            "Back-edge from '{}' targets Loop '{}' which does not enclose it",
                            id, succ
                        ),
                        missing_placeholder: None,
                    });
                }
            }
        }
    }

    // ── L4: Data closure (monotone dataflow analysis over SESE join modes) ──
    let mut entrance_avail: HashMap<String, HashSet<String>> = HashMap::new();
    let mut exit_avail: HashMap<String, HashSet<String>> = HashMap::new();
    for id in plan.nodes.keys() {
        entrance_avail.insert(id.clone(), HashSet::new());
        exit_avail.insert(id.clone(), HashSet::new());
    }

    let mut changed = true;
    while changed {
        changed = false;
        for (id, node) in &plan.nodes {
            let preds = incoming.get(id);
            let mut computed_entrance = HashSet::new();

            if let Some(p_list) = preds {
                if !p_list.is_empty() {
                    let is_and_join = match node {
                        ExecutionNode::Join(jn) => jn.mode == JoinMode::Parallel,
                        _ => false,
                    };

                    let mut is_or_join = false;
                    let mut or_meet = HashSet::new();
                    if let ExecutionNode::Join(jn) = node {
                        if jn.mode == JoinMode::Inclusive {
                            if let Some(ExecutionNode::Split(sp)) = plan.nodes.get(&jn.split) {
                                if let Some(ref socket) = sp.routing_socket {
                                    if let Some(subsets) = registry.get_decision_enum_values(socket) {
                                        is_or_join = true;
                                        let mut label_to_pred = HashMap::new();
                                        for flow in &sp.flows {
                                            if let Some(ref val) = flow.expected_value {
                                                let is_direct = flow.next == *id;
                                                if is_direct {
                                                    label_to_pred.insert(val.clone(), sp.id.clone());
                                                } else {
                                                    for p in p_list {
                                                        if is_reachable_without_node(&flow.next, p, id, &adj) {
                                                            label_to_pred.insert(val.clone(), p.clone());
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        let mut first_combo = true;
                                        for combo in &subsets {
                                            if combo.is_empty() || combo == "empty" || combo == "∅" {
                                                continue;
                                            }
                                            let mut combo_union = HashSet::new();
                                            let parts: Vec<&str> = combo.split(',').map(|s| s.trim()).collect();
                                            for part in parts {
                                                if let Some(pred_id) = label_to_pred.get(part) {
                                                    combo_union.extend(exit_avail.get(pred_id).cloned().unwrap_or_default());
                                                }
                                            }

                                            if first_combo {
                                                or_meet = combo_union;
                                                first_combo = false;
                                            } else {
                                                or_meet = or_meet.intersection(&combo_union).cloned().collect();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if is_or_join {
                        computed_entrance = or_meet;
                    } else if is_and_join {
                        for p in p_list {
                            computed_entrance.extend(exit_avail.get(p).cloned().unwrap_or_default());
                        }
                    } else {
                        let mut intersect = exit_avail.get(&p_list[0]).cloned().unwrap_or_default();
                        for p in &p_list[1..] {
                            let p_avail = exit_avail.get(p).cloned().unwrap_or_default();
                            intersect = intersect.intersection(&p_avail).cloned().collect();
                        }
                        computed_entrance = intersect;
                    }
                }
            }

            if computed_entrance != entrance_avail[id] {
                entrance_avail.insert(id.clone(), computed_entrance.clone());
                changed = true;
            }

            let mut computed_exit = computed_entrance;
            match node {
                ExecutionNode::Task(t) => {
                    if let Some(ref prod) = t.produces_placeholder {
                        computed_exit.insert(prod.clone());
                    }
                }
                ExecutionNode::Split(sp) => {
                    let decl = sp.routing_socket.as_ref()
                        .and_then(|plug| registry.decision_bindings(plug))
                        .unwrap_or_default();
                    if let Some(ref prod) = decl.produces {
                        computed_exit.insert(prod.clone());
                    }
                }
                _ => {}
            }

            if computed_exit != exit_avail[id] {
                exit_avail.insert(id.clone(), computed_exit);
                changed = true;
            }
        }
    }

    for (id, node) in &plan.nodes {
        let entrance = entrance_avail.get(id).unwrap();
        match node {
            ExecutionNode::Task(t) => {
                for consumed in &t.consumes_placeholders {
                    if !entrance.contains(consumed) {
                        diagnostics.push(Diagnostic {
                            node_id: id.clone(),
                            message: format!(
                                "Task '{}' consumes placeholder '{}' which is not produced upstream on all paths",
                                id, consumed
                            ),
                            missing_placeholder: Some(consumed.clone()),
                        });
                    }
                }

                if t.plug.len() == 64 && t.plug.chars().all(|c| c.is_ascii_hexdigit()) {
                    if let Some(sig) = registry.get_workflow_signature(&t.plug) {
                        for consumed in &sig.consumes {
                            if !entrance.contains(consumed) {
                                diagnostics.push(Diagnostic {
                                    node_id: id.clone(),
                                    message: format!(
                                        "Task '{}' invokes child workflow with missing input parameter '{}' which is not available at call site",
                                        id, consumed
                                    ),
                                    missing_placeholder: Some(consumed.clone()),
                                });
                            }
                        }
                    }
                }
            }
            ExecutionNode::Split(sp) => {
                let decl = sp.routing_socket.as_ref()
                    .and_then(|plug| registry.decision_bindings(plug))
                    .unwrap_or_default();
                for consumed in &decl.consumes {
                    if !entrance.contains(consumed) {
                        diagnostics.push(Diagnostic {
                            node_id: id.clone(),
                            message: format!(
                                "Split '{}' routes on placeholder '{}' which is not produced upstream on all paths",
                                id, consumed
                            ),
                            missing_placeholder: Some(consumed.clone()),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    // ── L5: Exhaustiveness check ──
    for (id, node) in &plan.nodes {
        if let ExecutionNode::Split(sp) = node {
            if sp.mode == SplitMode::Exclusive || sp.mode == SplitMode::Inclusive {
                let mut socket_id = sp.routing_socket.clone();
                if socket_id.is_none() {
                    if let Some(first_flow) = sp.flows.first() {
                        if let Some(ref ph) = first_flow.placeholder {
                            if let Some(slot) = plan.placeholder_schema.slots.get(ph) {
                                if let Some(ExecutionNode::Task(t)) = plan.nodes.get(&slot.produced_by) {
                                    socket_id = Some(t.plug.clone());
                                }
                            }
                        }
                    }
                }
                if let Some(ref socket) = socket_id {
                    // Assert type kinds match gateway mode
                    if let Some((_type_name, type_kind)) = registry.get_decision_output_type_info(socket) {
                        if sp.mode == SplitMode::Exclusive && type_kind != "enum" {
                            diagnostics.push(Diagnostic {
                                node_id: id.clone(),
                                message: format!(
                                    "Split '{}' (XOR) requires Enum output from decision '{}', but got '{}'",
                                    id, socket, type_kind
                                ),
                                missing_placeholder: None,
                            });
                        }
                        if sp.mode == SplitMode::Inclusive && type_kind != "named_subset" {
                            diagnostics.push(Diagnostic {
                                node_id: id.clone(),
                                message: format!(
                                    "Split '{}' (OR) requires Named Subset output from decision '{}', but got '{}'",
                                    id, socket, type_kind
                                ),
                                missing_placeholder: None,
                            });
                        }
                    }

                    if let Some(enum_values) = registry.get_decision_enum_values(socket) {
                        let split_branches: HashSet<String> = sp.flows.iter()
                            .filter_map(|f| f.expected_value.clone())
                            .collect();

                        // Build set of allowed labels from decider's output domain
                        let mut allowed_labels = HashSet::new();
                        for val in &enum_values {
                            if sp.mode == SplitMode::Exclusive {
                                allowed_labels.insert(val.clone());
                            } else if sp.mode == SplitMode::Inclusive {
                                if val != "empty" && val != "∅" && !val.is_empty() {
                                    let parts: Vec<&str> = val.split(',').map(|s| s.trim()).collect();
                                    for part in parts {
                                        if !part.is_empty() {
                                            allowed_labels.insert(part.to_string());
                                        }
                                    }
                                }
                            }
                        }

                        // Ensure branches ⊆ output (catches output domain shrink / dead branches)
                        for val in &split_branches {
                            if !allowed_labels.contains(val) {
                                diagnostics.push(Diagnostic {
                                    node_id: id.clone(),
                                    message: format!(
                                        "Split '{}' contains branch for value '{}' which is not in the output domain of decision '{}' (dead branch)",
                                        id, val, socket
                                    ),
                                    missing_placeholder: None,
                                });
                            }
                        }

                        for val in &enum_values {
                            if sp.mode == SplitMode::Exclusive {
                                if !split_branches.contains(val) {
                                    diagnostics.push(Diagnostic {
                                        node_id: id.clone(),
                                        message: format!(
                                            "Split '{}' is not exhaustive: missing branch for value '{}' from decision '{}'",
                                            id, val, socket
                                        ),
                                        missing_placeholder: None,
                                    });
                                }
                            } else if sp.mode == SplitMode::Inclusive {
                                // OR: each legal combination value represents a subset of labels
                                // e.g. "fund,corporate" or "∅". We assert that the empty set is not in the legal combinations.
                                if val.is_empty() || val == "empty" || val == "∅" {
                                    diagnostics.push(Diagnostic {
                                        node_id: id.clone(),
                                        message: format!(
                                            "Split '{}' (OR) has illegal empty set combination in decision '{}'",
                                            id, socket
                                        ),
                                        missing_placeholder: None,
                                    });
                                } else {
                                    // Parse comma-separated branch labels in the subset
                                    let parts: Vec<&str> = val.split(',').map(|s| s.trim()).collect();
                                    for part in parts {
                                        if !part.is_empty() && !split_branches.contains(part) {
                                            diagnostics.push(Diagnostic {
                                                node_id: id.clone(),
                                                message: format!(
                                                    "Split '{}' (OR) is not exhaustive: missing branch for value '{}' from decision '{}'",
                                                    id, part, socket
                                                ),
                                                missing_placeholder: None,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if sp.mode == SplitMode::Inclusive && sp.flows.is_empty() {
                    diagnostics.push(Diagnostic {
                        node_id: id.clone(),
                        message: format!("Split '{}' (Inclusive OR) has no outgoing branches", id),
                        missing_placeholder: None,
                    });
                }
            }
        }
    }

    // ── L8: Derived delivery mode & legality ──
    for (id, node) in &plan.nodes {
        if let ExecutionNode::Task(t) = node {
            if t.delivery_mode == DeliveryMode::BestEffort {
                if let Some(ref prod) = t.produces_placeholder {
                    let has_downstream_consumers = plan.placeholder_schema.slots.get(prod)
                        .map(|slot| !slot.consumed_by.is_empty())
                        .unwrap_or(false);
                    if has_downstream_consumers {
                        diagnostics.push(Diagnostic {
                            node_id: id.clone(),
                            message: format!(
                                "Task '{}' is best-effort but its output '{}' is consumed downstream",
                                id, prod
                            ),
                            missing_placeholder: None,
                        });
                    }
                }
            }

            let decl = registry.verb_bindings(&t.plug).unwrap_or_default();
            let is_must_complete = matches!(decl.effect_class.as_deref(), Some("read_modify_write") | Some("write_obligation"));
            if is_must_complete && t.delivery_mode == DeliveryMode::BestEffort {
                diagnostics.push(Diagnostic {
                    node_id: id.clone(),
                    message: format!(
                        "Task '{}' carries must-complete verb '{}' but is configured with best-effort delivery",
                        id, t.plug
                    ),
                    missing_placeholder: None,
                });
            }

            let is_inside_loop = enclosing_loops.contains_key(id);
            if is_inside_loop {
                let is_idempotent = matches!(decl.effect_class.as_deref(), Some("idempotent_ensure") | Some("read"));
                if !is_idempotent && !t.static_args.contains_key("idempotency_key") {
                    diagnostics.push(Diagnostic {
                        node_id: id.clone(),
                        message: format!(
                            "Task '{}' inside a Loop is non-idempotent and lacks an idempotency guard",
                            id
                        ),
                        missing_placeholder: None,
                    });
                }
            }
        }
    }

    diagnostics
}

fn has_workflow_cycle(
    curr_hash: &str,
    registry: &dyn PlaceholderRegistry,
    visited: &mut HashSet<String>,
    path: &mut Vec<String>,
) -> bool {
    // If we're already visiting this node in the current path, it's a cycle
    if path.iter().take(path.len() - 1).any(|x| x == curr_hash) {
        return true;
    }
    // If we've already fully processed this subtree and found no cycles, skip it
    if visited.contains(curr_hash) {
        return false;
    }
    visited.insert(curr_hash.to_string());

    for child in registry.get_workflow_child_calls(curr_hash) {
        path.push(child.clone());
        if has_workflow_cycle(&child, registry, visited, path) {
            return true;
        }
        path.pop();
    }
    false
}

fn is_reachable_without_node(
    start: &str,
    target: &str,
    avoid: &str,
    adj: &HashMap<String, Vec<&str>>,
) -> bool {
    let mut visited = HashSet::new();
    let mut q = VecDeque::new();
    q.push_back(start);
    while let Some(curr) = q.pop_front() {
        if curr == target {
            return true;
        }
        if curr == avoid {
            continue;
        }
        if visited.insert(curr.to_string()) {
            if let Some(nexts) = adj.get(curr) {
                for &next in nexts {
                    q.push_back(next);
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{compile, linter::StubPlaceholderRegistry, BindingDecl};

    fn registry_with_enums() -> StubPlaceholderRegistry {
        StubPlaceholderRegistry::new().with_demo_bindings()
    }

    #[test]
    fn test_valid_demo_path_family() {
        let src = r#"(workflow custody-cbu-onboarding
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
        
        let plan = compile(src, &registry_with_enums()).expect("compile");
        let diags = validate_path_family(&plan, &registry_with_enums());
        assert!(diags.is_empty(), "expected zero diagnostics, got: {:?}", diags);
    }

    #[test]
    fn test_gw2_non_exhaustive_gateway() {
        let src = r#"(workflow custody-cbu-onboarding
          (start-event :id start :next create-cbu)
          (service-task :id create-cbu :verb cbu.create :next type-decision)
          (business-rule-task :id type-decision :decision cbu_type_routing :next type-gateway)
          (exclusive-gateway :id type-gateway
            (flow :condition (= @cbu-type "fund")      :next add-fund)
            (flow :condition (= @cbu-type "corporate") :next add-corp))
          (service-task :id add-fund  :verb cbu.add-product :args (:product "CUSTODY_FUND")  :next end)
          (service-task :id add-corp  :verb cbu.add-product :args (:product "CUSTODY_CORP")  :next end)
          (end-event :id end :status "Operational"))"#;

        let plan = compile(src, &registry_with_enums()).expect("compile");
        let diags = validate_path_family(&plan, &registry_with_enums());
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("is not exhaustive: missing branch for value 'trust'"));
    }

    #[test]
    fn test_gw3_branch_closure_violation() {
        let mut reg = registry_with_enums();
        reg.register_verb("cbu.add-product-special", BindingDecl {
            produces: None,
            consumes: vec!["@cbu".into(), "@special-token".into()],
            effect_class: Some("idempotent_ensure".into()),
        });

        reg.register_verb("special.produce", BindingDecl {
            produces: Some("@special-token".into()),
            consumes: vec![],
            effect_class: Some("idempotent_ensure".into()),
        });

        let src_with_producer = r#"(workflow custody-cbu-onboarding
          (start-event :id start :next create-cbu)
          (service-task :id create-cbu :verb cbu.create :next type-decision)
          (business-rule-task :id type-decision :decision cbu_type_routing :next type-gateway)
          (exclusive-gateway :id type-gateway
            (flow :condition (= @cbu-type "fund")      :next add-fund)
            (flow :condition (= @cbu-type "corporate") :next prod-special)
            (flow :condition (= @cbu-type "trust")     :next add-trust))
          (service-task :id add-fund  :verb cbu.add-product :args (:product "CUSTODY_FUND")  :next add-im)
          
          (service-task :id prod-special :verb special.produce :next add-corp)
          (service-task :id add-corp  :verb cbu.add-product-special :next add-im)
          
          (service-task :id add-trust :verb cbu.add-product-special :next add-im)
          (service-task :id add-im    :verb instrument-matrix.attach :next end)
          (end-event :id end :status "Operational"))"#;

        let plan = compile(src_with_producer, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        
        assert!(!diags.is_empty(), "expected diagnostics, got none");
        assert!(diags.iter().any(|d| d.node_id == "add-trust" && d.missing_placeholder.as_deref() == Some("@special-token")));
    }

    #[test]
    fn test_l6_safety() {
        let reg = registry_with_enums();

        // 1. Loop with ceiling 0 (invalid)
        let src_ceiling_0 = r#"(workflow test-loop
          (start-event :id start :next my-loop)
          (loop :id my-loop :ceiling 0 :body (
             (service-task :id inside-task :verb cbu.create :next my-loop)
          ) :next end)
          (end-event :id end :status "Operational"))"#;
        let plan = compile(src_ceiling_0, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(diags.iter().any(|d| d.node_id == "my-loop" && d.message.contains("lacks a finite ceiling")), "expected ceiling error, got: {:?}", diags);

        // 2. Loop with correct ceiling (valid)
        let src_valid = r#"(workflow test-loop
          (start-event :id start :next my-loop)
          (loop :id my-loop :ceiling 5 :body (
             (service-task :id inside-task :verb cbu.create :next my-loop)
          ) :next end)
          (end-event :id end :status "Operational"))"#;
        let plan = compile(src_valid, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(!diags.iter().any(|d| d.message.contains("lacks a finite ceiling")), "unexpected ceiling error: {:?}", diags);

        // 3. Back-edge escaping loop scope (invalid)
        let src_escaping = r#"(workflow test-loop
          (start-event :id start :next my-loop)
          (loop :id my-loop :ceiling 5 :body (
             (service-task :id inside-task :verb cbu.create :next other-loop)
          ) :next end)
          (loop :id other-loop :ceiling 5 :body (
             (service-task :id other-task :verb cbu.create :next other-loop)
          ) :next end)
          (end-event :id end :status "Operational"))"#;
        let plan = compile(src_escaping, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(diags.iter().any(|d| d.node_id == "inside-task" && d.message.contains("targets Loop 'other-loop' which does not enclose it")), "expected back-edge error, got: {:?}", diags);
    }

    #[test]
    fn test_split_exhaustiveness() {
        let mut reg = registry_with_enums();

        // Register a named subset decision for OR gateway testing:
        reg.register_decision("cbu_or_routing", BindingDecl {
            produces: Some("@cbu-type".into()),
            consumes: vec!["@cbu".into()],
            effect_class: None,
        });
        reg.register_decision_enum("cbu_or_routing", vec!["fund,corporate".into(), "trust".into()]);
        reg.register_decision_type_info("cbu_or_routing", "CbuOrType".to_string(), "named_subset".to_string());

        // 1. XOR Split with enum output mismatching/non-exhaustive branches
        let src_xor_non_exhaustive = r#"(workflow test-xor
          (start-event :id start :next create-cbu)
          (service-task :id create-cbu :verb cbu.create :next type-decision)
          (business-rule-task :id type-decision :decision cbu_type_routing :next type-gateway)
          (split-xor :id type-gateway :plug cbu_type_routing :join type-gateway-join
            (flow :condition (= @cbu-type "fund")      :next type-gateway-join)
            (flow :condition (= @cbu-type "corporate") :next type-gateway-join))
          (join-xor :id type-gateway-join :split type-gateway :next end)
          (end-event :id end :status "Operational"))"#;
        let plan = compile(src_xor_non_exhaustive, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(diags.iter().any(|d| d.node_id == "type-gateway" && d.message.contains("is not exhaustive: missing branch for value 'trust'")), "expected exhaustiveness warning, got: {:?}", diags);

        // 2. OR Split but decision plug returns enum instead of named_subset
        let src_or_type_mismatch = r#"(workflow test-or-mismatch
          (start-event :id start :next create-cbu)
          (service-task :id create-cbu :verb cbu.create :next type-decision)
          (business-rule-task :id type-decision :decision cbu_type_routing :next type-gateway)
          (split-or :id type-gateway :plug cbu_type_routing :join type-gateway-join
            (flow :condition (= @cbu-type "fund")      :next type-gateway-join)
            (flow :condition (= @cbu-type "corporate") :next type-gateway-join)
            (flow :condition (= @cbu-type "trust")     :next type-gateway-join))
          (join-or :id type-gateway-join :split type-gateway :next end)
          (end-event :id end :status "Operational"))"#;
        let plan = compile(src_or_type_mismatch, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(diags.iter().any(|d| d.node_id == "type-gateway" && d.message.contains("requires Named Subset output")), "expected type mismatch, got: {:?}", diags);

        // 3. OR Split with named_subset and exhaustive branches (valid)
        let src_or_valid = r#"(workflow test-or-valid
          (start-event :id start :next create-cbu)
          (service-task :id create-cbu :verb cbu.create :next type-decision)
          (business-rule-task :id type-decision :decision cbu_or_routing :next type-gateway)
          (split-or :id type-gateway :plug cbu_or_routing :join type-gateway-join
            (flow :condition (= @cbu-type "fund")      :next type-gateway-join)
            (flow :condition (= @cbu-type "corporate") :next type-gateway-join)
            (flow :condition (= @cbu-type "trust")     :next type-gateway-join))
          (join-or :id type-gateway-join :split type-gateway :next end)
          (end-event :id end :status "Operational"))"#;
        let plan = compile(src_or_valid, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(!diags.iter().any(|d| d.node_id == "type-gateway" && d.message.contains("is not exhaustive")), "unexpected exhaustiveness error: {:?}", diags);

        // 4. OR Split with empty set combination (invalid)
        let mut reg_with_empty = registry_with_enums();
        reg_with_empty.register_decision("cbu_or_routing_empty", BindingDecl {
            produces: Some("@cbu-type".into()),
            consumes: vec!["@cbu".into()],
            effect_class: None,
        });
        reg_with_empty.register_decision_enum("cbu_or_routing_empty", vec!["fund,corporate".into(), "∅".into()]);
        reg_with_empty.register_decision_type_info("cbu_or_routing_empty", "CbuOrType".to_string(), "named_subset".to_string());

        let src_or_empty = r#"(workflow test-or-empty
          (start-event :id start :next create-cbu)
          (service-task :id create-cbu :verb cbu.create :next type-decision)
          (business-rule-task :id type-decision :decision cbu_or_routing_empty :next type-gateway)
          (split-or :id type-gateway :plug cbu_or_routing_empty :join type-gateway-join
            (flow :condition (= @cbu-type "fund")      :next type-gateway-join)
            (flow :condition (= @cbu-type "corporate") :next type-gateway-join))
          (join-or :id type-gateway-join :split type-gateway :next end)
          (end-event :id end :status "Operational"))"#;
        let plan = compile(src_or_empty, &reg_with_empty).expect("compile");
        let diags = validate_path_family(&plan, &reg_with_empty);
        assert!(diags.iter().any(|d| d.node_id == "type-gateway" && d.message.contains("has illegal empty set combination")), "expected empty set error, got: {:?}", diags);
    }

    #[test]
    fn test_call_activity() {
        let mut reg = registry_with_enums();
        let child_hash = "1111111111111111111111111111111111111111111111111111111111111111";
        let missing_child_hash = "9999999999999999999999999999999999999999999999999999999999999999";

        // Register valid child workflow: consumes @cbu
        reg.register_workflow(child_hash, BindingDecl {
            produces: None,
            consumes: vec!["@cbu".into()],
            effect_class: Some("idempotent_ensure".into()),
        }, true);

        // 1. Missing child hash
        let src_missing_child = format!(
            r#"(workflow parent-wf
              (start-event :id start :next my-task)
              (service-task :id my-task :verb cbu.create :next child-call)
              (task :id child-call :plug {} :next end)
              (end-event :id end :status "done"))"#,
            missing_child_hash
        );
        let plan = compile(&src_missing_child, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(diags.iter().any(|d| d.node_id == "child-call" && d.message.contains("does not exist in registry")), "expected missing error, got: {:?}", diags);

        // 2. Child workflow exists but parameter mismatch (parent hasn't produced @cbu at call site)
        let src_param_mismatch = format!(
            r#"(workflow parent-wf
              (start-event :id start :next child-call)
              (task :id child-call :plug {} :next end)
              (end-event :id end :status "done"))"#,
            child_hash
        );
        let plan = compile(&src_param_mismatch, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(diags.iter().any(|d| d.node_id == "child-call" && d.message.contains("missing input parameter '@cbu'")), "expected param mismatch error, got: {:?}", diags);

        // 3. Valid child call
        let src_valid = format!(
            r#"(workflow parent-wf
              (start-event :id start :next create-cbu)
              (service-task :id create-cbu :verb cbu.create :next child-call)
              (task :id child-call :plug {} :next end)
              (end-event :id end :status "done"))"#,
            child_hash
        );
        let plan = compile(&src_valid, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(!diags.iter().any(|d| d.node_id == "child-call"), "unexpected error for valid call: {:?}", diags);

        // 4. Cyclic recursion child -> parent
        let mut reg_cyclic = registry_with_enums();
        let wf_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let wf_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        reg_cyclic.register_workflow(wf_a, BindingDecl::default(), true);
        reg_cyclic.register_workflow(wf_b, BindingDecl::default(), true);
        reg_cyclic.register_workflow_child_calls(wf_a, vec![wf_b.to_string()]);
        reg_cyclic.register_workflow_child_calls(wf_b, vec![wf_a.to_string()]);

        let src_cyclic = format!(
            r#"(workflow parent-wf
              (start-event :id start :next child-call)
              (task :id child-call :plug {} :next end)
              (end-event :id end :status "done"))"#,
            wf_a
        );
        let plan = compile(&src_cyclic, &reg_cyclic).expect("compile");
        let diags = validate_path_family(&plan, &reg_cyclic);
        assert!(diags.iter().any(|d| d.node_id == "child-call" && d.message.contains("detects cyclic child workflow recursion")), "expected recursion error, got: {:?}", diags);
    }

    #[test]
    fn test_l4_parallel_union() {
        let mut reg = registry_with_enums();

        reg.register_verb("cbu.consume-two", BindingDecl {
            produces: None,
            consumes: vec!["@cbu-part1".into(), "@cbu-part2".into()],
            effect_class: Some("idempotent_ensure".into()),
        });

        reg.register_verb("cbu.produce-part1", BindingDecl {
            produces: Some("@cbu-part1".into()),
            consumes: vec![],
            effect_class: Some("idempotent_ensure".into()),
        });

        reg.register_verb("cbu.produce-part2", BindingDecl {
            produces: Some("@cbu-part2".into()),
            consumes: vec![],
            effect_class: Some("idempotent_ensure".into()),
        });

        // 1. Parallel AND Join (valid): union of placeholders -> both available downstream
        let src_and_union_valid = r#"(workflow test-and-union
          (start-event :id start :next split-gateway)
          (split-and :id split-gateway :join join-gateway
            (flow :next prod-1)
            (flow :next prod-2))
          (service-task :id prod-1 :verb cbu.produce-part1 :next join-gateway)
          (service-task :id prod-2 :verb cbu.produce-part2 :next join-gateway)
          (join-and :id join-gateway :split split-gateway :next consumer)
          (service-task :id consumer :verb cbu.consume-two :next end)
          (end-event :id end :status "done"))"#;
        let plan = compile(src_and_union_valid, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(!diags.iter().any(|d| d.node_id == "consumer"), "unexpected error for valid AND union: {:?}", diags);

        // 2. XOR Join (invalid for union): intersection -> neither is available downstream
        let src_xor_intersection_invalid = r#"(workflow test-xor-intersection
          (start-event :id start :next create-cbu)
          (service-task :id create-cbu :verb cbu.create :next type-decision)
          (business-rule-task :id type-decision :decision cbu_type_routing :next type-gateway)
          (split-xor :id type-gateway :plug cbu_type_routing :join type-gateway-join
            (flow :condition (= @cbu-type "fund")      :next prod-1)
            (flow :condition (= @cbu-type "corporate") :next prod-2)
            (flow :condition (= @cbu-type "trust")     :next prod-1))
          (service-task :id prod-1 :verb cbu.produce-part1 :next type-gateway-join)
          (service-task :id prod-2 :verb cbu.produce-part2 :next type-gateway-join)
          (join-xor :id type-gateway-join :split type-gateway :next consumer)
          (service-task :id consumer :verb cbu.consume-two :next end)
          (end-event :id end :status "done"))"#;
        let plan = compile(src_xor_intersection_invalid, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        assert!(diags.iter().any(|d| d.node_id == "consumer" && d.missing_placeholder.as_deref() == Some("@cbu-part1")), "expected missing @cbu-part1 error");
        assert!(diags.iter().any(|d| d.node_id == "consumer" && d.missing_placeholder.as_deref() == Some("@cbu-part2")), "expected missing @cbu-part2 error");
    }

    #[test]
    fn test_substitution_variance_output_shrink() {
        // Swapping a decider to a shrunken output domain (e.g. from ["fund", "corporate", "trust"] to ["fund", "corporate"])
        // must trigger a diagnostic failure for the now-orphaned/dead branch (trust).
        let mut reg = registry_with_enums();
        // type-gateway split expects fund, corporate, and trust from cbu_type_routing.
        // We register cbu_type_routing with only fund and corporate (shrinking output).
        reg.register_decision("cbu_type_routing", BindingDecl {
            produces: Some("@cbu-type".into()),
            consumes: vec!["@cbu".into()],
            effect_class: None,
        });
        reg.register_decision_enum("cbu_type_routing", vec!["fund".into(), "corporate".into()]);
        reg.register_decision_type_info("cbu_type_routing", "CbuType".to_string(), "enum".to_string());

        let src = r#"(workflow custody-cbu-onboarding
          (start-event :id start :next create-cbu)
          (service-task :id create-cbu :verb cbu.create :next type-decision)
          (business-rule-task :id type-decision :decision cbu_type_routing :next type-gateway)
          (exclusive-gateway :id type-gateway
            (flow :condition (= @cbu-type "fund")      :next add-fund)
            (flow :condition (= @cbu-type "corporate") :next add-corp)
            (flow :condition (= @cbu-type "trust")     :next add-trust))
          (service-task :id add-fund  :verb cbu.add-product :args (:product "CUSTODY_FUND")  :next end)
          (service-task :id add-corp  :verb cbu.add-product :args (:product "CUSTODY_CORP")  :next end)
          (service-task :id add-trust :verb cbu.add-product :args (:product "CUSTODY_TRUST") :next end)
          (end-event :id end :status "Operational"))"#;

        let plan = compile(src, &reg).expect("compile");
        let diags = validate_path_family(&plan, &reg);
        
        assert!(!diags.is_empty(), "expected output-shrink failure, got none");
        assert!(
            diags.iter().any(|d| d.node_id == "type-gateway" && d.message.contains("contains branch for value 'trust' which is not in the output domain of decision 'cbu_type_routing'")),
            "unexpected error message: {:?}", diags
        );
    }
}
