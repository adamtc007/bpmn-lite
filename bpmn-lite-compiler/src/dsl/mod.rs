//! bpmn-dsl s-expression compilation pipeline.
//!
//! Three-phase: **parse** → **lint** → **dag**.
//!
//! ```text
//! bpmn-dsl source (s-expression text)
//!     ↓ parse()
//! WorkflowSource (AST)
//!     ↓ lint(registry)
//! WorkflowExecutionPlan (semantic DAG)
//!     ↓ validate_dag()
//! WorkflowExecutionPlan (validated — ready for executor)
//! ```
//!
//! Entry point: [`compile`] runs all three phases and returns the validated
//! plan or a [`CompileError`] describing what went wrong.

pub mod ast;
pub mod dag;
pub mod lexer;
pub mod linter;
pub mod manifest_registry;
pub mod plan;

pub use ast::{
    ConditionAst, EndEventAst, ExclusiveGatewayAst, FlowAst, NodeAst, ServiceTaskAst,
    BusinessRuleTaskAst, StartEventAst, WorkflowSource,
};
pub use dag::{validate_dag, DagError};
pub use linter::{
    lint, BindingDecl, LintError, PlaceholderRegistry, StubPlaceholderRegistry, SymbolResolution,
};
pub use manifest_registry::ManifestPlaceholderRegistry;
pub use plan::{
    BusinessRuleExecNode, EndExecNode, ExecutionNode, GatewayExecFlow, GatewayExecNode,
    PlaceholderSchema, PlaceholderSlot, ServiceTaskExecNode, StartExecNode, WorkflowExecutionPlan,
};

use lexer::lex;
use parser::Parser;

mod parser;

/// All errors that can occur during bpmn-dsl compilation.
#[derive(Debug)]
pub enum CompileError {
    /// Parse-phase errors: each string is `"[offset] message"`.
    Parse(Vec<String>),
    Lint(Vec<LintError>),
    Dag(Vec<DagError>),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(errs) => {
                for e in errs { writeln!(f, "parse: {e}")?; }
                Ok(())
            }
            Self::Lint(errs) => {
                for e in errs { writeln!(f, "lint: {e}")?; }
                Ok(())
            }
            Self::Dag(errs) => {
                for e in errs { writeln!(f, "dag: {e}")?; }
                Ok(())
            }
        }
    }
}

impl std::error::Error for CompileError {}

/// Compile a bpmn-dsl source string to a validated `WorkflowExecutionPlan`.
///
/// `registry` provides catalogue binding declarations for placeholder inference.
/// Use [`StubPlaceholderRegistry::with_demo_bindings`] for tests and demos.
pub fn compile(
    source: &str,
    registry: &dyn PlaceholderRegistry,
) -> Result<WorkflowExecutionPlan, CompileError> {
    // Phase 1: parse
    let (tokens, lex_errors) = lex(source);
    let mut p = Parser::new(tokens);
    let mut raw_errs: Vec<parser::ParseError> =
        lex_errors.into_iter().map(Into::into).collect();
    let ast = p.parse_workflow();
    raw_errs.extend(p.into_errors());
    if !raw_errs.is_empty() {
        let msgs = raw_errs.iter().map(|e| format!("[{}] {}", e.offset, e.message)).collect();
        return Err(CompileError::Parse(msgs));
    }
    let ast = ast.ok_or_else(|| CompileError::Parse(vec!["empty workflow".into()]))?;

    // Phase 2: lint
    let plan = lint(&ast, registry).map_err(CompileError::Lint)?;

    // Phase 3: dag
    validate_dag(&plan).map_err(CompileError::Dag)?;

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> StubPlaceholderRegistry {
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
    fn demo_model_compiles_successfully() {
        let plan = compile(DEMO_SRC, &registry()).expect("compile failed");
        assert_eq!(plan.workflow_id, "custody-cbu-onboarding");
        assert_eq!(plan.start_node, "start");
        assert_eq!(plan.nodes.len(), 9); // start + create + type-decision + gateway + 3×add + add-im + end
    }

    #[test]
    fn demo_model_has_correct_placeholder_schema() {
        let plan = compile(DEMO_SRC, &registry()).expect("compile failed");
        assert!(plan.placeholder_schema.slots.contains_key("@cbu"), "@cbu slot missing");
        assert!(plan.placeholder_schema.slots.contains_key("@cbu-type"), "@cbu-type slot missing");
        assert_eq!(plan.placeholder_schema.slots["@cbu"].produced_by, "create-cbu");
        assert_eq!(plan.placeholder_schema.slots["@cbu-type"].produced_by, "type-decision");
    }

    #[test]
    fn demo_model_gateway_has_three_flows() {
        let plan = compile(DEMO_SRC, &registry()).expect("compile failed");
        let gw = match plan.nodes.get("type-gateway").unwrap() {
            ExecutionNode::ExclusiveGateway(gw) => gw,
            _ => panic!("expected gateway"),
        };
        assert_eq!(gw.flows.len(), 3);
        let values: Vec<&str> = gw.flows.iter().map(|f| f.expected_value.as_str()).collect();
        assert!(values.contains(&"fund"));
        assert!(values.contains(&"corporate"));
        assert!(values.contains(&"trust"));
    }

    #[test]
    fn product_args_preserved_on_service_tasks() {
        let plan = compile(DEMO_SRC, &registry()).expect("compile failed");
        let node = match plan.nodes.get("add-fund").unwrap() {
            ExecutionNode::ServiceTask(t) => t,
            _ => panic!("expected service task"),
        };
        assert_eq!(node.verb_fqn, "cbu.add-product");
        assert_eq!(node.static_args.get("product").map(|s| s.as_str()), Some("CUSTODY_FUND"));
    }

    #[test]
    fn all_three_product_paths_converge_on_add_im() {
        let plan = compile(DEMO_SRC, &registry()).expect("compile failed");
        for id in &["add-fund", "add-corp", "add-trust"] {
            let next = match plan.nodes.get(*id).unwrap() {
                ExecutionNode::ServiceTask(t) => &t.next,
                _ => panic!(),
            };
            assert_eq!(next, "add-im", "expected {id} → add-im");
        }
    }

    #[test]
    fn compile_rejects_unresolved_verb() {
        let src = r#"(workflow test
          (start-event :id s :next t)
          (service-task :id t :verb no.such.verb :next e)
          (end-event :id e :status "done"))"#;
        assert!(matches!(compile(src, &registry()), Err(CompileError::Lint(_))));
    }

    #[test]
    fn compile_rejects_unresolved_next() {
        let src = "(workflow test (start-event :id s :next missing))";
        assert!(matches!(compile(src, &registry()), Err(CompileError::Lint(_))));
    }

    #[test]
    fn compile_rejects_unknown_placeholder_in_gateway() {
        let src = r#"(workflow test
          (start-event :id s :next gw)
          (exclusive-gateway :id gw
            (flow :condition (= @never-produced "x") :next e))
          (end-event :id e :status "done"))"#;
        assert!(matches!(compile(src, &registry()), Err(CompileError::Lint(_))));
    }
}

// ── v0.6 T1: namespaced verb / decision resolution via imported manifests ─────
//
// These tests exercise the §10 demo model rewritten with namespaced verb
// references (`ob-poc:cbu.create`, `dmn-lite:cbu_type_routing`) against a
// `ManifestPlaceholderRegistry` that has imported real `dsl_manifest::Manifest`
// instances. The `:inputs (...)` syntax shown verbatim in §10 is a planned
// syntactic upgrade — these tests use the current `:args (:key "value")`
// syntax which is semantically equivalent for the demo paths.
#[cfg(test)]
mod namespaced_tests {
    use super::*;
    use dsl_manifest::Manifest;

    const OB_POC_YAML: &str = r#"
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"
verbs:
  - id: "cbu.create"
    signature: { inputs: [] }
    effect_class: "idempotent_ensure"
    authority_required: "cbu.write"
  - id: "cbu.add-product"
    signature: { inputs: [] }
    effect_class: "idempotent_ensure"
    authority_required: "cbu.write"
  - id: "instrument-matrix.attach"
    signature: { inputs: [] }
    effect_class: "idempotent_ensure"
    authority_required: "cbu.write"
"#;

    const DMN_LITE_YAML: &str = r#"
manifest_version: "1.0"
domain: "dmn-lite"
catalogue_version: "v0.1.0"
generated_at: "2026-05-20T10:00:00Z"
verbs: []
decisions:
  - id: "cbu_type_routing"
    inputs:
      - name: "cbu_client_type"
        type: "CbuClientType"
    output:
      type: "CbuType"
      enum_values: ["fund", "corporate", "trust"]
"#;

    // §10 demo model rewritten with namespaced verbs. Placeholder bindings come
    // from the inner `StubPlaceholderRegistry::with_demo_bindings()` (keyed by
    // unqualified ids), which `ManifestPlaceholderRegistry` reaches via local-id
    // delegation after stripping the domain prefix.
    const NAMESPACED_DEMO_SRC: &str = r#"(workflow custody-cbu-onboarding
  (start-event :id start :next create-cbu)
  (service-task :id create-cbu :verb ob-poc:cbu.create :next type-decision)
  (business-rule-task :id type-decision :decision dmn-lite:cbu_type_routing :next type-gateway)
  (exclusive-gateway :id type-gateway
    (flow :condition (= @cbu-type "fund")      :next add-fund)
    (flow :condition (= @cbu-type "corporate") :next add-corp)
    (flow :condition (= @cbu-type "trust")     :next add-trust))
  (service-task :id add-fund  :verb ob-poc:cbu.add-product :args (:product "fund")      :next attach-im)
  (service-task :id add-corp  :verb ob-poc:cbu.add-product :args (:product "corporate") :next attach-im)
  (service-task :id add-trust :verb ob-poc:cbu.add-product :args (:product "trust")     :next attach-im)
  (service-task :id attach-im :verb ob-poc:instrument-matrix.attach :next end)
  (end-event :id end :status "Operational"))"#;

    fn namespaced_registry() -> ManifestPlaceholderRegistry<StubPlaceholderRegistry> {
        let mut reg =
            ManifestPlaceholderRegistry::new(StubPlaceholderRegistry::new().with_demo_bindings());
        reg.import(Manifest::load_from_yaml(OB_POC_YAML).expect("ob-poc manifest"));
        reg.import(Manifest::load_from_yaml(DMN_LITE_YAML).expect("dmn-lite manifest"));
        reg
    }

    #[test]
    fn namespaced_demo_compiles_via_imported_manifests() {
        let plan = compile(NAMESPACED_DEMO_SRC, &namespaced_registry()).expect("compile failed");
        assert_eq!(plan.workflow_id, "custody-cbu-onboarding");
        assert_eq!(plan.start_node, "start");
        assert_eq!(plan.nodes.len(), 9);
    }

    #[test]
    fn namespaced_demo_preserves_namespaced_verb_fqn() {
        let plan = compile(NAMESPACED_DEMO_SRC, &namespaced_registry()).expect("compile failed");
        let create = match plan.nodes.get("create-cbu").unwrap() {
            ExecutionNode::ServiceTask(t) => t,
            _ => panic!("expected service-task"),
        };
        assert_eq!(create.verb_fqn, "ob-poc:cbu.create");
        let decision = match plan.nodes.get("type-decision").unwrap() {
            ExecutionNode::BusinessRuleTask(t) => t,
            _ => panic!("expected business-rule-task"),
        };
        assert_eq!(decision.decision_id, "dmn-lite:cbu_type_routing");
    }

    #[test]
    fn namespaced_demo_infers_cbu_and_cbu_type_placeholders() {
        let plan = compile(NAMESPACED_DEMO_SRC, &namespaced_registry()).expect("compile failed");
        assert!(plan.placeholder_schema.slots.contains_key("@cbu"));
        assert!(plan.placeholder_schema.slots.contains_key("@cbu-type"));
        assert_eq!(plan.placeholder_schema.slots["@cbu"].produced_by, "create-cbu");
        assert_eq!(
            plan.placeholder_schema.slots["@cbu-type"].produced_by,
            "type-decision"
        );
    }

    #[test]
    fn unknown_domain_prefix_produces_structured_lint_error() {
        // 'mystery:' is not an imported manifest.
        let src = r#"(workflow t
          (start-event :id s :next x)
          (service-task :id x :verb mystery:cbu.create :next e)
          (end-event :id e :status "done"))"#;
        match compile(src, &namespaced_registry()) {
            Err(CompileError::Lint(errs)) => {
                let msg = errs.first().expect("at least one error").message.as_str();
                assert!(msg.contains("unknown domain"), "got: {msg}");
                assert!(msg.contains("mystery"), "got: {msg}");
            }
            other => panic!("expected Lint error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_verb_in_known_domain_produces_structured_lint_error() {
        let src = r#"(workflow t
          (start-event :id s :next x)
          (service-task :id x :verb ob-poc:cbu.does-not-exist :next e)
          (end-event :id e :status "done"))"#;
        match compile(src, &namespaced_registry()) {
            Err(CompileError::Lint(errs)) => {
                let msg = errs.first().expect("at least one error").message.as_str();
                assert!(msg.contains("not found in 'ob-poc' manifest"), "got: {msg}");
                assert!(msg.contains("3 verbs declared"), "got: {msg}");
            }
            other => panic!("expected Lint error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_decision_in_known_domain_produces_structured_lint_error() {
        let src = r#"(workflow t
          (start-event :id s :next x)
          (business-rule-task :id x :decision dmn-lite:not_a_decision :next e)
          (end-event :id e :status "done"))"#;
        match compile(src, &namespaced_registry()) {
            Err(CompileError::Lint(errs)) => {
                let msg = errs.first().expect("at least one error").message.as_str();
                assert!(msg.contains("not found in 'dmn-lite' manifest"), "got: {msg}");
                assert!(msg.contains("1 decisions declared"), "got: {msg}");
            }
            other => panic!("expected Lint error, got {other:?}"),
        }
    }
}
