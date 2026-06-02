use std::collections::{HashMap, HashSet};
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

    // Blocking child workflow co-reachability deadlock check:
    for (id, node) in &plan.nodes {
        if let ExecutionNode::Task(t) = node {
            if t.delivery_mode == DeliveryMode::Blocking
                && t.plug.len() == 64
                && t.plug.chars().all(|c| c.is_ascii_hexdigit())
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

                    if is_and_join {
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
                    if let Some(enum_values) = registry.get_decision_enum_values(socket) {
                        let split_branches: HashSet<String> = sp.flows.iter()
                            .filter_map(|f| f.expected_value.clone())
                            .collect();

                        for val in &enum_values {
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
}
