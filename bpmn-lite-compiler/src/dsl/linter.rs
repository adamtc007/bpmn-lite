//! Semantic linter for the bpmn-dsl pipeline.
//!
//! Takes a parsed `WorkflowSource` and a `PlaceholderRegistry` (catalogue
//! binding declarations), then:
//!
//! 1. Checks all `:verb`, `:decision`, and `:next` references resolve.
//! 2. Infers `@cbu`-style placeholder flow from catalogue declarations.
//! 3. Validates gateway/split predicates reference known placeholders.
//! 4. Produces a `WorkflowExecutionPlan` ready for DAG validation.

use std::collections::{BTreeMap, HashMap};

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
    /// Effect class of the verb (e.g. `"idempotent_ensure"`, `"read"`, `"read_modify_write"`).
    pub effect_class: Option<String>,
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

    fn get_decision_enum_values(&self, _decision_id: &str) -> Option<Vec<String>> {
        None
    }

    fn get_decision_output_type_info(&self, _decision_id: &str) -> Option<(String, String)> {
        None
    }

    fn workflow_exists(&self, _hash: &str) -> bool {
        false
    }
    fn workflow_satisfies_l2(&self, _hash: &str) -> bool {
        true
    }
    fn get_workflow_signature(&self, _hash: &str) -> Option<BindingDecl> {
        None
    }
    fn get_workflow_child_calls(&self, _hash: &str) -> Vec<String> {
        Vec::new()
    }
}

/// Outcome of resolving a `domain:symbol` reference against a registry.
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
    decision_enums: HashMap<String, Vec<String>>,
    workflows: HashMap<String, BindingDecl>,
    workflow_l2: HashMap<String, bool>,
    decision_type_kinds: HashMap<String, (String, String)>,
    workflow_child_calls: HashMap<String, Vec<String>>,
}

impl StubPlaceholderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_verb(&mut self, fqn: impl Into<String>, decl: BindingDecl) {
        self.verbs.insert(fqn.into(), decl);
    }

    pub fn register_decision(&mut self, id: impl Into<String>, decl: BindingDecl) {
        self.decisions.insert(id.into(), decl);
    }

    pub fn register_decision_enum(&mut self, id: impl Into<String>, values: Vec<String>) {
        self.decision_enums.insert(id.into(), values);
    }

    pub fn register_decision_type_info(
        &mut self,
        id: impl Into<String>,
        type_name: String,
        kind: String,
    ) {
        self.decision_type_kinds
            .insert(id.into(), (type_name, kind));
    }

    pub fn register_workflow(
        &mut self,
        hash: impl Into<String>,
        decl: BindingDecl,
        satisfies_l2: bool,
    ) {
        let h = hash.into();
        self.workflows.insert(h.clone(), decl);
        self.workflow_l2.insert(h, satisfies_l2);
    }

    pub fn register_workflow_child_calls(&mut self, hash: impl Into<String>, calls: Vec<String>) {
        self.workflow_child_calls.insert(hash.into(), calls);
    }

    /// Seed with the Phase 5.5 demo model bindings.
    pub fn with_demo_bindings(mut self) -> Self {
        // cbu.create → produces @cbu
        self.register_verb(
            "cbu.create",
            BindingDecl {
                produces: Some("@cbu".into()),
                consumes: vec![],
                effect_class: Some("idempotent_ensure".into()),
            },
        );
        // cbu.add-product → consumes @cbu, no new placeholder
        self.register_verb(
            "cbu.add-product",
            BindingDecl {
                produces: None,
                consumes: vec!["@cbu".into()],
                effect_class: Some("idempotent_ensure".into()),
            },
        );
        // instrument-matrix.attach → consumes @cbu, no new placeholder
        self.register_verb(
            "instrument-matrix.attach",
            BindingDecl {
                produces: None,
                consumes: vec!["@cbu".into()],
                effect_class: Some("idempotent_ensure".into()),
            },
        );
        // cbu_type_routing DMN: consumes @cbu, produces @cbu-type
        self.register_decision(
            "cbu_type_routing",
            BindingDecl {
                produces: Some("@cbu-type".into()),
                consumes: vec!["@cbu".into()],
                effect_class: None,
            },
        );
        self.register_decision_enum(
            "cbu_type_routing",
            vec!["fund".into(), "corporate".into(), "trust".into()],
        );
        self.register_decision_type_info(
            "cbu_type_routing",
            "CbuType".to_string(),
            "enum".to_string(),
        );

        // check_eligibility DMN: consumes @cbu, produces @eligible (boolean result)
        self.register_decision(
            "check_eligibility",
            BindingDecl {
                produces: Some("@eligible".into()),
                consumes: vec!["@cbu".into()],
                effect_class: None,
            },
        );
        self.register_decision_enum("check_eligibility", vec!["true".into(), "false".into()]);
        self.register_decision_type_info(
            "check_eligibility",
            "boolean".to_string(),
            "bool".to_string(),
        );
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
    fn get_decision_enum_values(&self, decision_id: &str) -> Option<Vec<String>> {
        self.decision_enums.get(decision_id).cloned()
    }
    fn get_decision_output_type_info(&self, decision_id: &str) -> Option<(String, String)> {
        self.decision_type_kinds.get(decision_id).cloned()
    }
    fn workflow_exists(&self, hash: &str) -> bool {
        self.workflows.contains_key(hash)
    }
    fn workflow_satisfies_l2(&self, hash: &str) -> bool {
        self.workflow_l2.get(hash).copied().unwrap_or(true)
    }
    fn get_workflow_signature(&self, hash: &str) -> Option<BindingDecl> {
        self.workflows.get(hash).cloned()
    }
    fn get_workflow_child_calls(&self, hash: &str) -> Vec<String> {
        self.workflow_child_calls
            .get(hash)
            .cloned()
            .unwrap_or_default()
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
        Self {
            registry,
            errors: Vec::new(),
        }
    }

    fn err(&mut self, node_id: &str, msg: impl Into<String>) {
        self.errors.push(LintError {
            node_id: node_id.into(),
            message: msg.into(),
        });
    }

    fn run(mut self, source: &WorkflowSource) -> Result<WorkflowExecutionPlan, Vec<LintError>> {
        // Flatten the hierarchical AST nodes (extract nested loop body nodes)
        let mut flat_ast_nodes = Vec::new();
        self.flatten_nodes(&source.nodes, &mut flat_ast_nodes);

        // ── Pass 1: collect all node ids ──────────────────────────────────────
        let node_ids: HashMap<String, ()> = flat_ast_nodes
            .iter()
            .map(|n| (n.id().to_owned(), ()))
            .collect();

        // ── Pass 2: check for duplicate ids ───────────────────────────────────
        {
            let mut seen: HashMap<String, usize> = HashMap::new();
            for node in &flat_ast_nodes {
                *seen.entry(node.id().to_owned()).or_default() += 1;
            }
            for (id, count) in &seen {
                if *count > 1 {
                    self.err(id, format!("duplicate node id '{id}'"));
                }
            }
        }

        // ── Pass 3: validate refs + resolve catalogue, build exec nodes ───────
        let mut exec_nodes: BTreeMap<String, ExecutionNode> = BTreeMap::new();
        let mut start_node = String::new();
        let mut placeholder_producers: HashMap<String, String> = HashMap::new(); // placeholder → node_id

        for node in &flat_ast_nodes {
            let id = node.id();
            let exec_node = match node {
                NodeAst::Start(n) => {
                    if start_node.is_empty() {
                        start_node = n.id.clone();
                    } else {
                        self.err(id, "multiple start events");
                    }
                    self.check_next_ref(id, &n.next, &node_ids);
                    ExecutionNode::Start(StartExecNode {
                        id: n.id.clone(),
                        next: n.next.clone(),
                        span: Some(n.span),
                    })
                }

                NodeAst::Task(n) => {
                    let mut decl = BindingDecl::default();
                    let mut is_known = false;

                    let verb_res = self.registry.resolve_verb(&n.plug);
                    let dec_res = self.registry.resolve_decision(&n.plug);

                    if verb_res == SymbolResolution::Known {
                        decl = self.registry.verb_bindings(&n.plug).unwrap_or_default();
                        is_known = true;
                    } else if dec_res == SymbolResolution::Known {
                        decl = self.registry.decision_bindings(&n.plug).unwrap_or_default();
                        is_known = true;
                    } else if n.plug.len() == 64 && n.plug.chars().all(|c| c.is_ascii_hexdigit()) {
                        is_known = true;
                        if let Some(sig) = self.registry.get_workflow_signature(&n.plug) {
                            decl = sig;
                        }
                    }

                    if !is_known {
                        match verb_res {
                            SymbolResolution::UnknownDomain { domain } => {
                                self.err(
                                    id,
                                    format!(
                                        "verb '{}' references unknown domain '{}:' — no manifest imported for this prefix",
                                        n.plug, domain
                                    ),
                                );
                            }
                            SymbolResolution::UnknownInDomain {
                                domain,
                                known_count,
                            } => {
                                let mut is_decision_err = false;
                                if let SymbolResolution::UnknownInDomain {
                                    domain: d,
                                    known_count: k,
                                } = dec_res
                                {
                                    if k > 0 {
                                        self.err(
                                            id,
                                            format!(
                                                "decision '{}' not found in '{}' manifest ({} decisions declared)",
                                                n.plug, d, k
                                            ),
                                        );
                                        is_decision_err = true;
                                    }
                                }
                                if !is_decision_err {
                                    self.err(
                                        id,
                                        format!(
                                            "verb '{}' not found in '{}' manifest ({} verbs declared)",
                                            n.plug, domain, known_count
                                        ),
                                    );
                                }
                            }
                            _ => {
                                self.err(id, format!("unresolved symbol '{}'", n.plug));
                            }
                        }
                    }
                    self.check_next_ref(id, &n.next, &node_ids);

                    if let Some(ref produced) = decl.produces {
                        placeholder_producers.insert(produced.clone(), n.id.clone());
                    }

                    let static_args: HashMap<String, String> = n.args.iter().cloned().collect();
                    let delivery_mode = match n.delivery_mode.as_deref() {
                        Some("best-effort") => DeliveryMode::BestEffort,
                        Some("guaranteed-async") => DeliveryMode::GuaranteedAsync,
                        _ => DeliveryMode::Blocking,
                    };

                    ExecutionNode::Task(TaskExecNode {
                        id: n.id.clone(),
                        plug: n.plug.clone(),
                        delivery_mode,
                        static_args,
                        next: n.next.clone(),
                        produces_placeholder: decl.produces,
                        consumes_placeholders: decl.consumes,
                        guards: Vec::new(),
                        span: Some(n.span),
                    })
                }

                NodeAst::Split(n) => {
                    if n.flows.is_empty() {
                        self.err(id, "split has no flows");
                    }
                    let mut exec_flows = Vec::new();
                    for flow in &n.flows {
                        self.check_next_ref(id, &flow.next, &node_ids);
                        let mut flow_placeholder = None;
                        let mut flow_expected = None;
                        if let Some(condition) = &flow.condition {
                            let ConditionAst::Eq { placeholder, value } = condition;
                            flow_placeholder = Some(placeholder.clone());
                            flow_expected = Some(value.clone());
                        }
                        exec_flows.push(SplitExecFlow {
                            placeholder: flow_placeholder,
                            expected_value: flow_expected,
                            next: flow.next.clone(),
                        });
                    }

                    let mode = match n.mode {
                        SplitModeAst::Xor => SplitMode::Exclusive,
                        SplitModeAst::Or => SplitMode::Inclusive,
                        SplitModeAst::And => SplitMode::Parallel,
                    };

                    let mut produces = None;
                    if let Some(plug) = &n.plug {
                        let decl = self.registry.decision_bindings(plug).unwrap_or_default();
                        produces = decl.produces;
                    }
                    if let Some(ref produced) = produces {
                        placeholder_producers.insert(produced.clone(), n.id.clone());
                    }

                    ExecutionNode::Split(SplitExecNode {
                        id: n.id.clone(),
                        mode,
                        routing_socket: n.plug.clone(),
                        flows: exec_flows,
                        join: n.join.clone(),
                        produces_placeholder: produces.clone(),
                        span: Some(n.span),
                    })
                }

                NodeAst::Join(n) => {
                    self.check_next_ref(id, &n.next, &node_ids);
                    let mode = match n.mode {
                        JoinModeAst::Xor => JoinMode::Exclusive,
                        JoinModeAst::Or => JoinMode::Inclusive,
                        JoinModeAst::And => JoinMode::Parallel,
                    };
                    ExecutionNode::Join(JoinExecNode {
                        id: n.id.clone(),
                        mode,
                        split: n.split.clone(),
                        next: n.next.clone(),
                        span: Some(n.span),
                    })
                }

                NodeAst::Loop(n) => {
                    self.check_next_ref(id, &n.next, &node_ids);

                    ExecutionNode::Loop(LoopExecNode {
                        id: n.id.clone(),
                        ceiling: n.ceiling,
                        body: n.body.iter().map(|child| child.id().to_owned()).collect(),
                        next: n.next.clone(),
                        span: Some(n.span),
                    })
                }

                NodeAst::End(n) => ExecutionNode::End(EndExecNode {
                    id: n.id.clone(),
                    status: n.status.clone(),
                    span: Some(n.span),
                }),
            };
            exec_nodes.insert(id.to_owned(), exec_node);
        }

        // ── Pass 3.5: Synthesize missing Join nodes for Splits ─────────────────
        let mut synthesized_joins = Vec::new();
        for (split_id, node) in &exec_nodes {
            if let ExecutionNode::Split(sp) = node {
                if !node_ids.contains_key(&sp.join) {
                    let mut meet_point: Option<String> = None;
                    for flow in &sp.flows {
                        if let Some(target_node) = exec_nodes.get(&flow.next) {
                            let next_of_target = match target_node {
                                ExecutionNode::Task(t) => Some(t.next.clone()),
                                ExecutionNode::Split(s) => Some(s.join.clone()),
                                ExecutionNode::Loop(lp) => Some(lp.next.clone()),
                                ExecutionNode::Join(j) => Some(j.next.clone()),
                                ExecutionNode::Start(st) => Some(st.next.clone()),
                                ExecutionNode::Wait(w) => Some(w.next.clone()),
                                ExecutionNode::End(e) => Some(e.id.clone()),
                            };
                            if let Some(next_id) = next_of_target {
                                if let Some(ref current_meet) = meet_point {
                                    if current_meet != &next_id {
                                        // Ignore mismatches for fallback, pick first
                                    }
                                } else {
                                    meet_point = Some(next_id);
                                }
                            }
                        }
                    }

                    if let Some(m) = meet_point {
                        let join_mode = match sp.mode {
                            SplitMode::Exclusive => JoinMode::Exclusive,
                            SplitMode::Inclusive => JoinMode::Inclusive,
                            SplitMode::Parallel => JoinMode::Parallel,
                        };
                        let join_node = ExecutionNode::Join(JoinExecNode {
                            id: sp.join.clone(),
                            mode: join_mode,
                            split: split_id.clone(),
                            next: m.clone(),
                            span: None,
                        });
                        synthesized_joins.push((sp.join.clone(), join_node, m, split_id.clone()));
                    }
                }
            }
        }

        for (join_id, join_node, meet_id, split_id) in synthesized_joins {
            let mut branch_nodes = std::collections::HashSet::new();
            if let Some(ExecutionNode::Split(sp)) = exec_nodes.get(&split_id) {
                for flow in &sp.flows {
                    branch_nodes.insert(flow.next.clone());
                }
            }
            for (id, node) in exec_nodes.iter_mut() {
                if branch_nodes.contains(id) {
                    match node {
                        ExecutionNode::Task(t) => {
                            if t.next == meet_id {
                                t.next = join_id.clone();
                            }
                        }
                        ExecutionNode::Split(s) => {
                            if s.join == meet_id {
                                s.join = join_id.clone();
                            }
                        }
                        ExecutionNode::Loop(lp) => {
                            if lp.next == meet_id {
                                lp.next = join_id.clone();
                            }
                        }
                        ExecutionNode::Join(j) => {
                            if j.next == meet_id {
                                j.next = join_id.clone();
                            }
                        }
                        ExecutionNode::Start(st) => {
                            if st.next == meet_id {
                                st.next = join_id.clone();
                            }
                        }
                        ExecutionNode::Wait(w) => {
                            if w.next == meet_id {
                                w.next = join_id.clone();
                            }
                        }
                        ExecutionNode::End(_) => {}
                    }
                }
            }
            exec_nodes.insert(join_id, join_node);
        }

        if start_node.is_empty() {
            self.errors.push(LintError {
                node_id: "<workflow>".into(),
                message: "no start node found".into(),
            });
        }

        // ── Pass 4: validate gateway/split predicates reference known placeholders ───
        for node in &flat_ast_nodes {
            if let NodeAst::Split(sp) = node {
                for flow in &sp.flows {
                    if let Some(ConditionAst::Eq { placeholder, .. }) = &flow.condition {
                        if !placeholder_producers.contains_key(placeholder) {
                            self.err(&sp.id, format!("split condition references unknown placeholder '{placeholder}'"));
                        }
                    }
                }
            }
        }

        if !self.errors.is_empty() {
            return Err(self.errors);
        }

        // ── Pass 5: build placeholder schema ──────────────────────────────────
        let mut slots: HashMap<String, PlaceholderSlot> = HashMap::new();
        for node in &flat_ast_nodes {
            let id = node.id();
            let (produces, consumes) = match node {
                NodeAst::Task(_) => {
                    let exec = exec_nodes.get(id).unwrap();
                    if let ExecutionNode::Task(t) = exec {
                        (
                            t.produces_placeholder.clone(),
                            t.consumes_placeholders.clone(),
                        )
                    } else {
                        (None, Vec::new())
                    }
                }
                NodeAst::Split(_) => {
                    let exec = exec_nodes.get(id).unwrap();
                    if let ExecutionNode::Split(s) = exec {
                        let decl = s
                            .routing_socket
                            .as_ref()
                            .and_then(|plug| self.registry.decision_bindings(plug))
                            .unwrap_or_default();
                        (decl.produces, decl.consumes)
                    } else {
                        (None, Vec::new())
                    }
                }
                _ => (None, Vec::new()),
            };

            let produces_clone = produces.clone();
            if let Some(p) = produces_clone {
                slots.entry(p.clone()).or_insert_with(|| PlaceholderSlot {
                    name: p,
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

        let regime_version = std::env::var("BPMN_LITE_REGIME_VERSION").ok();
        let mut plan = WorkflowExecutionPlan::new(
            source.name.clone(),
            exec_nodes,
            start_node,
            PlaceholderSchema { slots },
            Some(serde_json::json!({ "dependencies": [] })),
            regime_version,
        );

        // Derive task delivery modes based on dataflow (P6 / L8)
        let registry = self.registry;
        let ast_delivery_modes: HashMap<String, Option<String>> = flat_ast_nodes
            .iter()
            .filter_map(|node| {
                if let NodeAst::Task(t) = node {
                    Some((t.id.clone(), t.delivery_mode.clone()))
                } else {
                    None
                }
            })
            .collect();

        for (id, node) in &mut plan.nodes {
            if let ExecutionNode::Task(t) = node {
                let explicit = ast_delivery_modes.get(id).and_then(|opt| opt.as_ref()).map(
                    |raw| match raw.as_str() {
                        "best-effort" => DeliveryMode::BestEffort,
                        "guaranteed-async" => DeliveryMode::GuaranteedAsync,
                        _ => DeliveryMode::Blocking,
                    },
                );

                let mut output_consumed = false;
                if let Some(ref prod) = t.produces_placeholder {
                    if let Some(slot) = plan.placeholder_schema.slots.get(prod) {
                        if !slot.consumed_by.is_empty() {
                            output_consumed = true;
                        }
                    }
                }

                let decl = registry
                    .verb_bindings(&t.plug)
                    .or_else(|| registry.get_workflow_signature(&t.plug))
                    .unwrap_or_default();
                let is_must_complete = matches!(
                    decl.effect_class.as_deref(),
                    Some("read_modify_write") | Some("write_obligation")
                );

                t.delivery_mode = derive_delivery_mode(explicit, output_consumed, is_must_complete);
            }
        }

        Ok(plan)
    }

    fn flatten_nodes(&mut self, nodes: &[NodeAst], result: &mut Vec<NodeAst>) {
        for node in nodes {
            result.push(node.clone());
            if let NodeAst::Loop(lp) = node {
                self.flatten_nodes(&lp.body, result);
            }
        }
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
    use crate::dsl::lexer::lex;
    use crate::dsl::parser::Parser;

    fn parse_and_lint(src: &str) -> Result<WorkflowExecutionPlan, String> {
        let (tokens, _) = lex(src);
        let mut p = Parser::new(tokens);
        let ast = p.parse_workflow().expect("parse failed");
        let reg = StubPlaceholderRegistry::new().with_demo_bindings();
        lint(&ast, &reg).map_err(|errs| {
            errs.iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        })
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
