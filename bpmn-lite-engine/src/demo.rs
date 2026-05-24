//! §10 demo: custody-cbu-onboarding three-domain workflow.
//!
//! One function call (`build_demo_plan`) compiles the §10 bpmn-dsl source
//! against the real ob-poc + dmn-lite manifests (produced by T2B.3/T2B.4)
//! and returns a `WorkflowExecutionPlan` ready for `PlanWalker::start_process`.
//!
//! `demo_initial_vars(client_type)` builds the initial placeholder map for
//! each of the three demo paths: `"fund"`, `"corporate"`, `"trust"`.

use std::collections::HashMap;

use bpmn_lite_compiler::dsl::{
    compile, CompileError, ManifestPlaceholderRegistry, StubPlaceholderRegistry,
    WorkflowExecutionPlan,
};
use dsl_manifest::Manifest;

/// The §10 demo bpmn-dsl source — three-domain federation with exclusive
/// gateway routed by DMN `cbu_type_routing`.
///
/// Verb FQNs are namespaced (e.g. `ob-poc:cbu.create`); the compiler's T1
/// pipeline resolves them against imported `dsl_manifest::Manifest` objects.
///
/// Initial caller data (client name, client type) is passed as
/// `initial_variables` to `PlanWalker::start_process` and forwarded to each
/// callout automatically via `placeholder_values`.
pub const DEMO_SOURCE: &str = r#"(workflow custody-cbu-onboarding
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

/// ob-poc manifest. CARGO_MANIFEST_DIR = bpmn-lite-engine/; manifests/ is
/// one level up at the workspace root (bpmn-lite/manifests/).
const OB_POC_MANIFEST_YAML: &str = include_str!(
    concat!(env!("CARGO_MANIFEST_DIR"), "/../manifests/ob-poc-v1.0.0.yaml")
);

/// dmn-lite manifest at the same workspace root.
const DMN_LITE_MANIFEST_YAML: &str = include_str!(
    concat!(env!("CARGO_MANIFEST_DIR"), "/../manifests/dmn-lite-v1.0.0.yaml")
);

/// Compile the §10 demo workflow against the real ob-poc + dmn-lite manifests.
///
/// Returns a validated `WorkflowExecutionPlan` with namespaced verb FQNs
/// preserved on all nodes. The plan is ready for `PlanWalker::start_process`.
pub fn build_demo_plan() -> Result<WorkflowExecutionPlan, CompileError> {
    let ob_poc = Manifest::load_from_yaml(OB_POC_MANIFEST_YAML)
        .expect("ob-poc manifest must load — check manifests/ob-poc-v1.0.0.yaml");
    let dmn_lite = Manifest::load_from_yaml(DMN_LITE_MANIFEST_YAML)
        .expect("dmn-lite manifest must load — check manifests/dmn-lite-v1.0.0.yaml");

    let mut registry =
        ManifestPlaceholderRegistry::new(StubPlaceholderRegistry::new().with_demo_bindings());
    registry.import(ob_poc);
    registry.import(dmn_lite);

    compile(DEMO_SOURCE, &registry)
}

/// Build initial variables for a demo run.
///
/// `client_type` should be one of `"FUND_MANDATE"`, `"CORPORATE"`, `"TRUST"`.
/// These match the input values expected by the `cbu_type_routing` DMN decision.
pub fn demo_initial_vars(client_name: &str, client_type: &str) -> HashMap<String, serde_json::Value> {
    let mut vars = HashMap::new();
    vars.insert(
        "@input-name".to_owned(),
        serde_json::Value::String(client_name.to_owned()),
    );
    vars.insert(
        "@input-client-type".to_owned(),
        serde_json::Value::String(client_type.to_owned()),
    );
    vars
}

/// Reset helper — clears plan-based process instances from `store` for a
/// clean demo run. For `MemoryStore` this is a no-op (construct a fresh
/// store instead). For Postgres it would truncate the relevant tables — that
/// is handled separately in the Docker reset script (T7 scope).
pub fn reset_demo_state_comment() -> &'static str {
    "For Postgres: truncate bpmn_process_instance, bpmn_pending_invocation, outbox, inbox. \
     For MemoryStore: create a fresh Arc<MemoryStore>."
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_compiler::dsl::ExecutionNode;

    #[test]
    fn demo_plan_compiles_successfully() {
        let plan = build_demo_plan().expect("§10 demo compile failed");
        assert_eq!(plan.workflow_id, "custody-cbu-onboarding");
        assert_eq!(plan.start_node, "start");
        assert_eq!(plan.nodes.len(), 9); // start + create-cbu + type-decision + gateway + 3×add + attach-im + end
    }

    #[test]
    fn demo_plan_has_namespaced_verbs() {
        let plan = build_demo_plan().expect("compile");
        let create = match plan.nodes.get("create-cbu").unwrap() {
            ExecutionNode::ServiceTask(t) => t,
            _ => panic!("expected ServiceTask"),
        };
        assert_eq!(create.verb_fqn, "ob-poc:cbu.create");

        let decision = match plan.nodes.get("type-decision").unwrap() {
            ExecutionNode::BusinessRuleTask(t) => t,
            _ => panic!("expected BusinessRuleTask"),
        };
        assert_eq!(decision.decision_id, "dmn-lite:cbu_type_routing");

        let im = match plan.nodes.get("attach-im").unwrap() {
            ExecutionNode::ServiceTask(t) => t,
            _ => panic!("expected ServiceTask"),
        };
        assert_eq!(im.verb_fqn, "ob-poc:instrument-matrix.attach");
    }

    #[test]
    fn demo_plan_infers_cbu_and_cbu_type_placeholders() {
        let plan = build_demo_plan().expect("compile");
        assert!(plan.placeholder_schema.slots.contains_key("@cbu"), "@cbu missing");
        assert!(plan.placeholder_schema.slots.contains_key("@cbu-type"), "@cbu-type missing");
        assert_eq!(plan.placeholder_schema.slots["@cbu"].produced_by, "create-cbu");
        assert_eq!(plan.placeholder_schema.slots["@cbu-type"].produced_by, "type-decision");
    }

    #[test]
    fn demo_plan_gateway_routes_all_three_types() {
        let plan = build_demo_plan().expect("compile");
        let gw = match plan.nodes.get("type-gateway").unwrap() {
            ExecutionNode::ExclusiveGateway(gw) => gw,
            _ => panic!("expected gateway"),
        };
        assert_eq!(gw.flows.len(), 3);
        let targets: Vec<&str> = gw.flows.iter().map(|f| f.expected_value.as_str()).collect();
        assert!(targets.contains(&"fund"));
        assert!(targets.contains(&"corporate"));
        assert!(targets.contains(&"trust"));
    }

    #[test]
    fn demo_plan_product_tasks_have_static_args() {
        let plan = build_demo_plan().expect("compile");
        for (id, expected_product) in [
            ("add-fund", "fund"),
            ("add-corp", "corporate"),
            ("add-trust", "trust"),
        ] {
            let task = match plan.nodes.get(id).unwrap() {
                ExecutionNode::ServiceTask(t) => t,
                _ => panic!("expected ServiceTask for {id}"),
            };
            assert_eq!(
                task.static_args.get("product").map(String::as_str),
                Some(expected_product),
                "{id} product arg wrong"
            );
            assert_eq!(&task.next, "attach-im", "{id} should point to attach-im");
        }
    }

    #[test]
    fn demo_initial_vars_contains_required_keys() {
        let vars = demo_initial_vars("Allianz AM", "FUND_MANDATE");
        assert!(vars.contains_key("@input-name"));
        assert!(vars.contains_key("@input-client-type"));
        assert_eq!(vars["@input-name"], serde_json::Value::String("Allianz AM".into()));
        assert_eq!(vars["@input-client-type"], serde_json::Value::String("FUND_MANDATE".into()));
    }
}
