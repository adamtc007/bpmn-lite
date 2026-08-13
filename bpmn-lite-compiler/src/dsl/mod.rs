//! bpmn-dsl s-expression compilation pipeline.
//!
//! Three-phase: **parse** → **lint** → **dag**.
//!
//! Entry point: [`compile`] runs all three phases and returns the validated
//! plan or a [`CompileError`] describing what went wrong.

mod ast;
mod closure;
mod dag;
mod emit;
mod frontend;
pub(crate) mod ir_plan;
mod lexer;
mod linter;
mod macros;
mod manifest_registry;
mod pack_build;
mod plan;
mod refactor;
mod repeat;
mod rpst;
mod unroll;

// H4.1 (EOP-PLAN-CRATE-HYGIENE-001): this block used to flatten all 15 dsl
// submodules onto `dsl::*` regardless of real usage. Trimmed to genuinely
// cross-crate-consumed symbols (grep-confirmed against the whole workspace,
// including fuzz targets and xtask, which are real separate-crate
// consumers) plus anything structurally required by a retained public
// item's own signature (e.g. `WorkflowSource` is `lint`/`repeat_n_times`'s
// parameter type; `UnrollError`/`LintError`/`DagError` are `CompileError`
// variant payloads). `JoinAst`/`JoinModeAst`/`NodeAst`/`WorkflowSource` are
// the only 4 of 13 `ast` types with a real external consumer
// (`bpmn-lite-authoring`); the other 9 AST node types, `frontend`'s
// `DslFrontend`/`WorkflowFrontend`, `linter`'s `SymbolResolution`,
// `parser`'s `parse_node_str`, `unroll`'s `unroll_loops`/
// `MAX_UNROLLED_NODES`, and `macros`'s `create_bounded_retry_macro` (with
// its otherwise-unused `LoopAst`/`TaskAst`/... AST return/param types) had
// zero consumers anywhere in the workspace — moved out of this re-export;
// the underlying items are untouched in their already-private submodules,
// still fully usable intra-crate.
pub use ast::{JoinAst, JoinModeAst, NodeAst, WorkflowSource};
pub use closure::{validate_path_family, Diagnostic};
pub use dag::{validate_dag, DagError};
pub use emit::{emit_dsl, DslEmitError, EmittedDsl, ProcessLevelDecls};
pub use frontend::{lower_plan, FrontendError};
pub use ir_plan::{project_ir, IrPlanError};
pub use linter::{lint, BindingDecl, LintError, PlaceholderRegistry, StubPlaceholderRegistry};
pub use macros::{
    create_parallel_split_join, create_xor_split_join, CustomMacroConfig, MacroConfigList,
    XorBranchConfig,
};
pub use manifest_registry::ManifestPlaceholderRegistry;
pub use pack_build::{
    derive_version, generate_closure, generate_manifest, validate_pack, PackClosureManifest,
    WorkflowPackDAG,
};
pub use parser::parse_workflow_str;
pub use unroll::UnrollError;
pub use plan::{
    DeliveryMode, EndExecNode, ExecutionNode, JoinExecNode, JoinMode,
    MessageWaitExecNode, PlaceholderSchema, PlaceholderSlot, SplitExecFlow, SplitExecNode,
    SplitMode, StartExecNode, TaskExecNode, WaitExecNode, WorkflowExecutionPlan,
    derive_delivery_mode,
};
pub use refactor::{AstMutator, ToSexpr};
pub use repeat::{repeat_n_times, RepeatNTimesError};
pub use rpst::verify_sese_nesting;

use lexer::lex;
use parser::Parser;
use unroll::unroll_loops;

mod parser;

/// All errors that can occur during bpmn-dsl compilation.
#[derive(Debug)]
pub enum CompileError {
    /// Parse-phase errors: each string is `"[offset] message"`.
    Parse(Vec<String>),
    /// G3.1/G3.2: loop-unrolling phase, between parse and lint.
    Unroll(UnrollError),
    Lint(Vec<LintError>),
    Dag(Vec<DagError>),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(errs) => {
                for e in errs {
                    writeln!(f, "parse: {e}")?;
                }
                Ok(())
            }
            Self::Unroll(err) => writeln!(f, "unroll: {err}"),
            Self::Lint(errs) => {
                for e in errs {
                    writeln!(f, "lint: {e}")?;
                }
                Ok(())
            }
            Self::Dag(errs) => {
                for e in errs {
                    writeln!(f, "dag: {e}")?;
                }
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
    let mut raw_errs: Vec<parser::ParseError> = lex_errors.into_iter().map(Into::into).collect();
    let ast = p.parse_workflow();
    raw_errs.extend(p.into_errors());
    if !raw_errs.is_empty() {
        let msgs = raw_errs
            .iter()
            .map(|e| format!("[{}] {}", e.offset, e.message))
            .collect();
        return Err(CompileError::Parse(msgs));
    }
    let mut ast = ast.ok_or_else(|| CompileError::Parse(vec!["empty workflow".into()]))?;

    // Phase 1.5: unroll (G3.1/G3.2) — every `NodeAst::Loop` becomes N
    // forward-chained copies before the linter ever sees it, so no cyclic
    // shape survives into `ExecutionNode`/the DAG/back-edge machinery.
    ast.nodes = unroll_loops(ast.nodes).map_err(CompileError::Unroll)?;

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
        assert_eq!(plan.nodes.len(), 10); // start + create + type-decision + gateway + 3×add + add-im + end + split-join
    }

    #[test]
    fn demo_model_has_correct_placeholder_schema() {
        let plan = compile(DEMO_SRC, &registry()).expect("compile failed");
        assert!(
            plan.placeholder_schema.slots.contains_key("@cbu"),
            "@cbu slot missing"
        );
        assert!(
            plan.placeholder_schema.slots.contains_key("@cbu-type"),
            "@cbu-type slot missing"
        );
        assert_eq!(
            plan.placeholder_schema.slots["@cbu"].produced_by,
            "create-cbu"
        );
        assert_eq!(
            plan.placeholder_schema.slots["@cbu-type"].produced_by,
            "type-decision"
        );
    }

    #[test]
    fn demo_model_gateway_has_three_flows() {
        let plan = compile(DEMO_SRC, &registry()).expect("compile failed");
        let gw = match plan.nodes.get("type-gateway").unwrap() {
            ExecutionNode::Split(gw) => gw,
            _ => panic!("expected split"),
        };
        assert_eq!(gw.flows.len(), 3);
        let values: Vec<&str> = gw
            .flows
            .iter()
            .map(|f| f.expected_value.as_ref().unwrap().as_str())
            .collect();
        assert!(values.contains(&"fund"));
        assert!(values.contains(&"corporate"));
        assert!(values.contains(&"trust"));
    }

    #[test]
    fn product_args_preserved_on_service_tasks() {
        let plan = compile(DEMO_SRC, &registry()).expect("compile failed");
        let node = match plan.nodes.get("add-fund").unwrap() {
            ExecutionNode::Task(t) => t,
            _ => panic!("expected task"),
        };
        assert_eq!(node.plug, "cbu.add-product");
        assert_eq!(
            node.static_args.get("product").map(|s| s.as_str()),
            Some("CUSTODY_FUND")
        );
    }

    #[test]
    fn all_three_product_paths_converge_on_add_im() {
        let plan = compile(DEMO_SRC, &registry()).expect("compile failed");
        for id in &["add-fund", "add-corp", "add-trust"] {
            let next = match plan.nodes.get(*id).unwrap() {
                ExecutionNode::Task(t) => &t.next,
                _ => panic!(),
            };
            assert_eq!(
                next, "type-gateway-join",
                "expected {id} → type-gateway-join"
            );
        }
    }

    #[test]
    fn compile_rejects_unresolved_verb() {
        let src = r#"(workflow test
          (start-event :id s :next t)
          (service-task :id t :verb no.such.verb :next e)
          (end-event :id e :status "done"))"#;
        assert!(matches!(
            compile(src, &registry()),
            Err(CompileError::Lint(_))
        ));
    }

    #[test]
    fn compile_rejects_unresolved_next() {
        let src = "(workflow test (start-event :id s :next missing))";
        assert!(matches!(
            compile(src, &registry()),
            Err(CompileError::Lint(_))
        ));
    }

    #[test]
    fn compile_rejects_unknown_placeholder_in_gateway() {
        let src = r#"(workflow test
          (start-event :id s :next gw)
          (exclusive-gateway :id gw
            (flow :condition (= @never-produced "x") :next e))
          (end-event :id e :status "done"))"#;
        assert!(matches!(
            compile(src, &registry()),
            Err(CompileError::Lint(_))
        ));
    }
}

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
        assert_eq!(plan.nodes.len(), 10);
    }

    #[test]
    fn namespaced_demo_preserves_namespaced_verb_fqn() {
        let plan = compile(NAMESPACED_DEMO_SRC, &namespaced_registry()).expect("compile failed");
        let create = match plan.nodes.get("create-cbu").unwrap() {
            ExecutionNode::Task(t) => t,
            _ => panic!("expected task"),
        };
        assert_eq!(create.plug, "ob-poc:cbu.create");
        let decision = match plan.nodes.get("type-decision").unwrap() {
            ExecutionNode::Task(t) => t,
            _ => panic!("expected task"),
        };
        assert_eq!(decision.plug, "dmn-lite:cbu_type_routing");
    }

    #[test]
    fn namespaced_demo_infers_cbu_and_cbu_type_placeholders() {
        let plan = compile(NAMESPACED_DEMO_SRC, &namespaced_registry()).expect("compile failed");
        assert!(plan.placeholder_schema.slots.contains_key("@cbu"));
        assert!(plan.placeholder_schema.slots.contains_key("@cbu-type"));
        assert_eq!(
            plan.placeholder_schema.slots["@cbu"].produced_by,
            "create-cbu"
        );
        assert_eq!(
            plan.placeholder_schema.slots["@cbu-type"].produced_by,
            "type-decision"
        );
    }

    #[test]
    fn unknown_domain_prefix_produces_structured_lint_error() {
        let src = r#"(workflow t
          (start-event :id s :next x)
          (service-task :id x :verb mystery:cbu.create :next e)
          (end-event :id e :status "done"))"#;
        match compile(src, &namespaced_registry()) {
            Err(CompileError::Lint(errs)) => {
                let msg = errs.first().expect("at least one error").message.as_str();
                assert!(msg.contains("references unknown domain"), "got: {msg}");
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
                assert!(
                    msg.contains("not found in 'dmn-lite' manifest"),
                    "got: {msg}"
                );
                assert!(msg.contains("1 decisions declared"), "got: {msg}");
            }
            other => panic!("expected Lint error, got {other:?}"),
        }
    }
}
