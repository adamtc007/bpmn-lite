//! Semantic linter for the bpmn-dsl pipeline.
//!
//! Takes a parsed `WorkflowSource` and a `PlaceholderRegistry` (catalogue
//! binding declarations), then:
//!
//! 1. Checks all `:verb`, `:decision`, and `:next` references resolve.
//! 2. Infers `@cbu`-style placeholder flow from catalogue declarations.
//! 3. Validates gateway predicates reference known placeholders.
//! 4. Produces a `WorkflowExecutionPlan` ready for DAG validation.
//!
//! The linter is sync — no async catalogue calls. Real catalogue integration
//! happens at runtime (T2/T3).

use std::collections::HashMap;

use super::ast::*;
use super::plan::*;

// ── PlaceholderRegistry ───────────────────────────────────────────────────────

/// Binding declaration for one verb or decision in the catalogue.
#[derive(Debug, Clone, Default)]
pub struct BindingDecl {
    /// Placeholder produced (e.g. `"@cbu"`). `None` if verb produces nothing.
    pub produces: Option<String>,
    /// Placeholders consumed (e.g. `["@cbu"]`).
    pub consumes: Vec<String>,
}

/// Synchronous catalogue projection: binding declarations for verbs/decisions.
/// Implement this to wire real SemOS catalogue knowledge into the linter.
pub trait PlaceholderRegistry: Send + Sync {
    fn verb_bindings(&self, fqn: &str) -> Option<BindingDecl>;
    fn decision_bindings(&self, decision_id: &str) -> Option<BindingDecl>;
    fn verb_exists(&self, fqn: &str) -> bool {
        self.verb_bindings(fqn).is_some()
    }
    fn decision_exists(&self, decision_id: &str) -> bool {
        self.decision_bindings(decision_id).is_some()
    }

    /// Rich resolution for a `domain:verb` reference. Default impl uses
    /// [`verb_exists`] only — no domain distinction. Override on registries
    /// that import published manifests to surface unknown-domain vs
    /// unknown-verb-in-known-domain as distinct compile errors (v0.6 T1).
    fn resolve_verb(&self, fqn: &str) -> SymbolResolution {
        if self.verb_exists(fqn) {
            SymbolResolution::Known
        } else {
            SymbolResolution::Unresolved
        }
    }

    /// Rich resolution for a `domain:decision` reference. Default mirrors
    /// [`resolve_verb`].
    fn resolve_decision(&self, decision_id: &str) -> SymbolResolution {
        if self.decision_exists(decision_id) {
            SymbolResolution::Known
        } else {
            SymbolResolution::Unresolved
        }
    }
}

/// Outcome of resolving a `domain:symbol` reference against a registry.
///
/// `Known`: the registry recognises the reference and (for verbs/decisions
/// with placeholder semantics) can produce a [`BindingDecl`].
///
/// `UnknownDomain`: the symbol carries a `<prefix>:` namespace, but no
/// manifest is imported for that prefix. Compile error per v0.6 §7.5.
///
/// `UnknownInDomain`: the namespace prefix is recognised but the local
/// id is absent from that manifest. Compile error including a hint at how
/// many symbols *are* declared, to make typos obvious without dumping the
/// full catalogue.
///
/// `Unresolved`: the symbol carries no recognised namespace prefix and the
/// underlying registry has no local binding for it — falls back to the
/// pre-T1 "unresolved verb 'foo'" error path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolResolution {
    Known,
    UnknownDomain { domain: String },
    UnknownInDomain { domain: String, known_count: usize },
    Unresolved,
}

/// In-memory stub for tests and demo wiring.
#[derive(Default)]
pub struct StubPlaceholderRegistry {
    verbs: HashMap<String, BindingDecl>,
    decisions: HashMap<String, BindingDecl>,
}

impl StubPlaceholderRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn register_verb(&mut self, fqn: impl Into<String>, decl: BindingDecl) {
        self.verbs.insert(fqn.into(), decl);
    }

    pub fn register_decision(&mut self, id: impl Into<String>, decl: BindingDecl) {
        self.decisions.insert(id.into(), decl);
    }

    /// Seed with the Phase 5.5 demo model bindings.
    pub fn with_demo_bindings(mut self) -> Self {
        // cbu.create → produces @cbu
        self.register_verb("cbu.create", BindingDecl {
            produces: Some("@cbu".into()),
            consumes: vec![],
        });
        // cbu.add-product → consumes @cbu, no new placeholder
        self.register_verb("cbu.add-product", BindingDecl {
            produces: None,
            consumes: vec!["@cbu".into()],
        });
        // instrument-matrix.attach → consumes @cbu, no new placeholder
        self.register_verb("instrument-matrix.attach", BindingDecl {
            produces: None,
            consumes: vec!["@cbu".into()],
        });
        // cbu_type_routing DMN: consumes @cbu, produces @cbu-type
        self.register_decision("cbu_type_routing", BindingDecl {
            produces: Some("@cbu-type".into()),
            consumes: vec!["@cbu".into()],
        });
        self
    }
}

impl PlaceholderRegistry for StubPlaceholderRegistry {
    fn verb_bindings(&self, fqn: &str) -> Option<BindingDecl> {
        self.verbs.get(fqn).cloned()
    }
    fn decision_bindings(&self, decision_id: &str) -> Option<BindingDecl> {
        self.decisions.get(decision_id).cloned()
    }
}

// ── Lint error ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LintError {
    pub node_id: String,
    pub message: String,
}

impl std::fmt::Display for LintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.node_id, self.message)
    }
}

// ── Linter ────────────────────────────────────────────────────────────────────

pub fn lint(
    source: &WorkflowSource,
    registry: &dyn PlaceholderRegistry,
) -> Result<WorkflowExecutionPlan, Vec<LintError>> {
    Linter::new(registry).run(source)
}

struct Linter<'a> {
    registry: &'a dyn PlaceholderRegistry,
    errors: Vec<LintError>,
}

impl<'a> Linter<'a> {
    fn new(registry: &'a dyn PlaceholderRegistry) -> Self {
        Self { registry, errors: Vec::new() }
    }

    fn err(&mut self, node_id: &str, msg: impl Into<String>) {
        self.errors.push(LintError { node_id: node_id.into(), message: msg.into() });
    }

    fn run(mut self, source: &WorkflowSource) -> Result<WorkflowExecutionPlan, Vec<LintError>> {
        // ── Pass 1: collect all node ids ──────────────────────────────────────
        let node_ids: HashMap<String, ()> = source.nodes.iter()
            .map(|n| (n.id().to_owned(), ()))
            .collect();

        // ── Pass 2: check for duplicate ids ───────────────────────────────────
        {
            let mut seen: HashMap<String, usize> = HashMap::new();
            for node in &source.nodes {
                *seen.entry(node.id().to_owned()).or_default() += 1;
            }
            for (id, count) in &seen {
                if *count > 1 {
                    self.err(id, format!("duplicate node id '{id}'"));
                }
            }
        }

        // ── Pass 3: validate refs + resolve catalogue, build exec nodes ───────
        let mut exec_nodes: HashMap<String, ExecutionNode> = HashMap::new();
        let mut start_node = String::new();
        let mut placeholder_producers: HashMap<String, String> = HashMap::new(); // placeholder → node_id

        for node in &source.nodes {
            let id = node.id();
            let exec_node = match node {
                NodeAst::StartEvent(n) => {
                    if start_node.is_empty() {
                        start_node = n.id.clone();
                    } else {
                        self.err(id, "multiple start events");
                    }
                    self.check_next_ref(id, &n.next, &node_ids);
                    ExecutionNode::StartEvent(StartExecNode { id: n.id.clone(), next: n.next.clone() })
                }

                NodeAst::ServiceTask(n) => {
                    match self.registry.resolve_verb(&n.verb) {
                        SymbolResolution::Known => {}
                        SymbolResolution::UnknownDomain { domain } => self.err(
                            id,
                            format!(
                                "verb '{}' references unknown domain '{}:' — no manifest imported for this prefix",
                                n.verb, domain
                            ),
                        ),
                        SymbolResolution::UnknownInDomain { domain, known_count } => self.err(
                            id,
                            format!(
                                "verb '{}' not found in '{}' manifest ({} verbs declared)",
                                n.verb, domain, known_count
                            ),
                        ),
                        SymbolResolution::Unresolved => {
                            self.err(id, format!("unresolved verb '{}'", n.verb))
                        }
                    }
                    self.check_next_ref(id, &n.next, &node_ids);

                    let decl = self.registry.verb_bindings(&n.verb).unwrap_or_default();
                    if let Some(ref produced) = decl.produces {
                        placeholder_producers.insert(produced.clone(), n.id.clone());
                    }

                    let static_args: HashMap<String, String> = n.args.iter().cloned().collect();
                    ExecutionNode::ServiceTask(ServiceTaskExecNode {
                        id: n.id.clone(),
                        verb_fqn: n.verb.clone(),
                        static_args,
                        next: n.next.clone(),
                        produces_placeholder: decl.produces,
                        consumes_placeholders: decl.consumes,
                    })
                }

                NodeAst::BusinessRuleTask(n) => {
                    match self.registry.resolve_decision(&n.decision) {
                        SymbolResolution::Known => {}
                        SymbolResolution::UnknownDomain { domain } => self.err(
                            id,
                            format!(
                                "decision '{}' references unknown domain '{}:' — no manifest imported for this prefix",
                                n.decision, domain
                            ),
                        ),
                        SymbolResolution::UnknownInDomain { domain, known_count } => self.err(
                            id,
                            format!(
                                "decision '{}' not found in '{}' manifest ({} decisions declared)",
                                n.decision, domain, known_count
                            ),
                        ),
                        SymbolResolution::Unresolved => {
                            self.err(id, format!("unresolved decision '{}'", n.decision))
                        }
                    }
                    self.check_next_ref(id, &n.next, &node_ids);

                    let decl = self.registry.decision_bindings(&n.decision).unwrap_or_default();
                    if let Some(ref produced) = decl.produces {
                        placeholder_producers.insert(produced.clone(), n.id.clone());
                    }

                    ExecutionNode::BusinessRuleTask(BusinessRuleExecNode {
                        id: n.id.clone(),
                        decision_id: n.decision.clone(),
                        next: n.next.clone(),
                        produces_placeholder: decl.produces,
                        consumes_placeholders: decl.consumes,
                    })
                }

                NodeAst::ExclusiveGateway(n) => {
                    if n.flows.is_empty() {
                        self.err(id, "exclusive-gateway has no flows");
                    }
                    let mut exec_flows = Vec::new();
                    for flow in &n.flows {
                        self.check_next_ref(id, &flow.next, &node_ids);
                        let ConditionAst::Eq { placeholder, value } = &flow.condition;
                        exec_flows.push(GatewayExecFlow {
                            placeholder: placeholder.clone(),
                            expected_value: value.clone(),
                            next: flow.next.clone(),
                        });
                    }
                    ExecutionNode::ExclusiveGateway(GatewayExecNode {
                        id: n.id.clone(),
                        flows: exec_flows,
                    })
                }

                NodeAst::EndEvent(n) => {
                    ExecutionNode::EndEvent(EndExecNode { id: n.id.clone(), status: n.status.clone() })
                }
            };
            exec_nodes.insert(id.to_owned(), exec_node);
        }

        if start_node.is_empty() {
            self.errors.push(LintError { node_id: "<workflow>".into(), message: "no start-event found".into() });
        }

        // ── Pass 4: validate gateway predicates reference known placeholders ───
        for node in &source.nodes {
            if let NodeAst::ExclusiveGateway(gw) = node {
                for flow in &gw.flows {
                    let ConditionAst::Eq { placeholder, .. } = &flow.condition;
                    if !placeholder_producers.contains_key(placeholder) {
                        self.err(&gw.id, format!("gateway condition references unknown placeholder '{placeholder}'"));
                    }
                }
            }
        }

        if !self.errors.is_empty() {
            return Err(self.errors);
        }

        // ── Pass 5: build placeholder schema ──────────────────────────────────
        let mut slots: HashMap<String, PlaceholderSlot> = HashMap::new();
        for node in &source.nodes {
            let id = node.id();
            let (produces, consumes) = match node {
                NodeAst::ServiceTask(_) => {
                    let exec = exec_nodes.get(id).unwrap();
                    if let ExecutionNode::ServiceTask(t) = exec {
                        (t.produces_placeholder.as_deref(), t.consumes_placeholders.as_slice())
                    } else { (None, &[][..]) }
                }
                NodeAst::BusinessRuleTask(_) => {
                    let exec = exec_nodes.get(id).unwrap();
                    if let ExecutionNode::BusinessRuleTask(t) = exec {
                        (t.produces_placeholder.as_deref(), t.consumes_placeholders.as_slice())
                    } else { (None, &[][..]) }
                }
                _ => (None, &[][..]),
            };

            if let Some(p) = produces {
                slots.entry(p.to_owned()).or_insert_with(|| PlaceholderSlot {
                    name: p.to_owned(),
                    produced_by: id.to_owned(),
                    consumed_by: Vec::new(),
                });
            }
            for c in consumes {
                slots.entry(c.clone()).and_modify(|s| {
                    s.consumed_by.push(id.to_owned());
                });
            }
        }

        Ok(WorkflowExecutionPlan {
            workflow_id: source.name.clone(),
            nodes: exec_nodes,
            start_node,
            placeholder_schema: PlaceholderSchema { slots },
        })
    }

    fn check_next_ref(&mut self, node_id: &str, next: &str, known: &HashMap<String, ()>) {
        if !known.contains_key(next) {
            self.err(node_id, format!("':next {next}' references unknown node"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parser::Parser;
    use crate::dsl::lexer::lex;

    fn parse_and_lint(src: &str) -> Result<WorkflowExecutionPlan, String> {
        let (tokens, _) = lex(src);
        let mut p = Parser::new(tokens);
        let ast = p.parse_workflow().expect("parse failed");
        let reg = StubPlaceholderRegistry::new().with_demo_bindings();
        lint(&ast, &reg).map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
    }

    #[test]
    fn lint_rejects_unresolved_verb() {
        let src = "(workflow test (start-event :id s :next t) (service-task :id t :verb not.a.verb :next e) (end-event :id e :status \"done\"))";
        assert!(parse_and_lint(src).is_err());
    }

    #[test]
    fn lint_rejects_undefined_next() {
        let src = "(workflow test (start-event :id s :next does-not-exist))";
        assert!(parse_and_lint(src).is_err());
    }

    #[test]
    fn lint_rejects_gateway_referencing_unknown_placeholder() {
        let src = r#"(workflow test
          (start-event :id s :next gw)
          (exclusive-gateway :id gw
            (flow :condition (= @unknown "fund") :next e))
          (end-event :id e :status "done"))"#;
        assert!(parse_and_lint(src).is_err());
    }

    #[test]
    fn lint_infers_cbu_placeholder_from_create() {
        let src = "(workflow test
          (start-event :id s :next create)
          (service-task :id create :verb cbu.create :next e)
          (end-event :id e :status \"done\"))";
        let plan = parse_and_lint(src).expect("lint failed");
        assert!(plan.placeholder_schema.slots.contains_key("@cbu"));
        let slot = &plan.placeholder_schema.slots["@cbu"];
        assert_eq!(slot.produced_by, "create");
    }
}
