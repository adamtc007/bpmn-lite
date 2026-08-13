//! G4.1 — the parameter manifest: a typed classification of every data
//! reference in a `WorkflowGraphDto` that the workflow itself cannot
//! produce, and therefore must be supplied at instantiation time (or, for
//! an element-scoped reference, resolved per MI iteration and never
//! supplied at all).
//!
//! This is new ground, not an extension of an existing pass (checked, not
//! assumed — see the G4 receipt's substrate-fork writeup): the DSL
//! pipeline's `linter.rs` has no data/parameter-reference walk (its
//! "unresolved symbol" diagnostic is verb/plug resolution, a different
//! axis), and it cannot see MI regions at all (`IRNode::MultiInstance` is
//! XML/DTO-only). The authoring pipeline's `lints.rs` (`lint_contracts`,
//! L1/L3) already walks the same reference sites this pass needs
//! (`FlagCondition.flag`, `corr_key_source`) to check *provenance*
//! (is this flag written upstream) — this pass reuses that same site
//! inventory but asks a different question (can the workflow produce this
//! value *at all*, and if not, what shape does the caller need to supply)
//! and emits a durable, typed manifest instead of an in-the-moment
//! diagnostic.
use crate::contracts::ContractRegistry;
use crate::dto::{NodeDto, WorkflowGraphDto};
use crate::lints::{LintDiagnostic, LintLevel};
use bpmn_lite_compiler::Expression;
use bpmn_lite_types::DataObjectRole;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// How a manifest slot must be resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SlotKind {
    /// One value, supplied once per instance.
    Scalar,
    /// One `Value::Array`-shaped value, supplied once per instance.
    /// `element_shape` names the per-element fields an MI body inside this
    /// collection's region reads (empty if the MI declares no per-element
    /// input bindings).
    Collection { element_shape: Vec<String> },
    /// Resolved once per MI iteration from `collection_slot`'s elements.
    /// **Never suppliable at instantiation time** — Gate G4's own
    /// requirement, enforced structurally by `ParameterManifest::suppliable`
    /// rather than left to callers to filter correctly.
    ElementScoped {
        collection_slot: String,
        field: String,
    },
}

/// One typed data reference the workflow cannot resolve internally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterSlot {
    /// The reference's name: a flag/data-object name for `Scalar`/
    /// `Collection`, or the dotted `VarRef` path (e.g. `"director.name"`)
    /// for `ElementScoped`.
    pub name: String,
    pub kind: SlotKind,
    /// Every node id whose reference contributed to this slot, in
    /// first-seen order — the "why is this here" trail for diagnostics.
    pub node_ids: Vec<String>,
}

/// The full set of externally-unresolved data references in one workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ParameterManifest {
    pub slots: Vec<ParameterSlot>,
}

impl ParameterManifest {
    /// Slots a caller must supply at instantiation time. Element-scoped
    /// slots are excluded by construction — Gate G4 requires an
    /// element-scoped reference is *never* presented as suppliable, so
    /// that exclusion lives here once rather than being a filter every
    /// call site must remember to apply.
    pub fn suppliable(&self) -> impl Iterator<Item = &ParameterSlot> {
        self.slots
            .iter()
            .filter(|s| !matches!(s.kind, SlotKind::ElementScoped { .. }))
    }
}

/// Derive the parameter manifest for `dto` against `registry`.
///
/// A reference is classified as externally-unresolved (and gets a slot)
/// when no `ServiceTask` in the workflow whose contract is registered
/// writes it, UNLESS a `DataObject` node explicitly declares it
/// `Internal`/`Output` (an explicit in-workflow declaration overrides the
/// heuristic). A `DataObject` declared `Input`, or a flag in the
/// registry's `known_workflow_inputs` allow-list, is always unresolved
/// regardless of whether some task happens to also write it — an explicit
/// "this is external" declaration always wins.
pub(crate) fn derive_parameter_manifest(
    dto: &WorkflowGraphDto,
    registry: &ContractRegistry,
) -> ParameterManifest {
    let mut produced: HashSet<&str> = HashSet::new();
    for node in &dto.nodes {
        if let NodeDto::ServiceTask { task_type, .. } = node {
            if let Some(contract) = registry.get(task_type) {
                produced.extend(contract.writes_flags.iter().map(String::as_str));
            }
        }
    }

    let mut declared_input: HashSet<&str> = HashSet::new();
    let mut declared_internal_or_output: HashSet<&str> = HashSet::new();
    for node in &dto.nodes {
        if let NodeDto::DataObject { name, role, .. } = node {
            match role {
                DataObjectRole::Input => {
                    declared_input.insert(name.as_str());
                }
                DataObjectRole::Internal | DataObjectRole::Output => {
                    declared_internal_or_output.insert(name.as_str());
                }
            }
        }
    }

    let is_unresolved = |name: &str| -> bool {
        if declared_internal_or_output.contains(name) {
            return false;
        }
        declared_input.contains(name) || registry.is_known_input(name) || !produced.contains(name)
    };

    let mut slots: Vec<ParameterSlot> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    let upsert = |slots: &mut Vec<ParameterSlot>,
                       index: &mut HashMap<String, usize>,
                       key: String,
                       name: String,
                       kind: SlotKind,
                       node_id: &str| {
        if let Some(&i) = index.get(&key) {
            let entry = &mut slots[i];
            if !entry.node_ids.iter().any(|n| n == node_id) {
                entry.node_ids.push(node_id.to_string());
            }
        } else {
            index.insert(key, slots.len());
            slots.push(ParameterSlot {
                name,
                kind,
                node_ids: vec![node_id.to_string()],
            });
        }
    };

    // G7.4: a name driving an MI region (`collection_flag`) is classified
    // Collection by the walk below, never Scalar by this loop -- even when
    // it also carries an explicit `Input` declaration (the now-mandatory
    // shape for a client-supplied collection, since collections must be
    // declared). `upsert` never widens an existing entry's `kind`, so
    // registering Scalar here first would permanently shadow the more
    // specific Collection classification.
    let collection_flag_names: HashSet<&str> = dto
        .nodes
        .iter()
        .filter_map(|node| match node {
            NodeDto::MultiInstance { collection_flag, .. } => Some(collection_flag.as_str()),
            _ => None,
        })
        .collect();

    // Explicit DataObject::Input declarations — always a scalar slot, even
    // if nothing in the workflow references the name yet.
    for node in &dto.nodes {
        if let NodeDto::DataObject { id, name, role, .. } = node {
            if *role == DataObjectRole::Input && !collection_flag_names.contains(name.as_str()) {
                upsert(
                    &mut slots,
                    &mut index,
                    name.clone(),
                    name.clone(),
                    SlotKind::Scalar,
                    id,
                );
            }
        }
    }

    // MultiInstance nodes: the collection itself, plus any per-element
    // input bindings (G4.0's `inputs: Vec<FfiInputBinding>`).
    for node in &dto.nodes {
        if let NodeDto::MultiInstance {
            id,
            collection_flag,
            inputs,
            ..
        } = node
        {
            if is_unresolved(collection_flag) {
                let element_shape: Vec<String> =
                    inputs.iter().map(|b| b.target_field.clone()).collect();
                // Two MI regions can legitimately iterate the SAME external
                // collection with different bodies (e.g. one reads
                // `full_name`, another `role`) — merge `element_shape`
                // (union, first-seen order) rather than keeping only
                // whichever MI node's fields the walk saw first, which
                // would silently drop the second region's shape.
                if let Some(&i) = index.get(collection_flag) {
                    let entry = &mut slots[i];
                    if let SlotKind::Collection {
                        element_shape: existing,
                    } = &mut entry.kind
                    {
                        for field in &element_shape {
                            if !existing.contains(field) {
                                existing.push(field.clone());
                            }
                        }
                    }
                    if !entry.node_ids.iter().any(|n| n == id) {
                        entry.node_ids.push(id.clone());
                    }
                } else {
                    index.insert(collection_flag.clone(), slots.len());
                    slots.push(ParameterSlot {
                        name: collection_flag.clone(),
                        kind: SlotKind::Collection { element_shape },
                        node_ids: vec![id.clone()],
                    });
                }
            }

            for binding in inputs {
                if let Expression::VarRef(path) = &binding.expression {
                    let field_name = path.join(".");
                    let key = format!("{id}::{}", binding.target_field);
                    upsert(
                        &mut slots,
                        &mut index,
                        key,
                        field_name,
                        SlotKind::ElementScoped {
                            collection_slot: collection_flag.clone(),
                            field: binding.target_field.clone(),
                        },
                        id,
                    );
                }
            }
        }
    }

    // Edge conditions and correlation sources — the same site inventory
    // L1/L3 already walk, reused here for classification rather than
    // provenance-ordering.
    for edge in &dto.edges {
        if let Some(cond) = &edge.condition {
            if is_unresolved(&cond.flag) {
                upsert(
                    &mut slots,
                    &mut index,
                    cond.flag.clone(),
                    cond.flag.clone(),
                    SlotKind::Scalar,
                    &edge.from,
                );
            }
        }
    }

    for node in &dto.nodes {
        let (node_id, corr_key) = match node {
            NodeDto::MessageWait {
                id,
                corr_key_source,
                ..
            } => (id, corr_key_source),
            NodeDto::HumanWait {
                id,
                corr_key_source,
                ..
            } => (id, corr_key_source),
            _ => continue,
        };
        if corr_key == "instance_id" {
            continue;
        }
        if is_unresolved(corr_key) {
            upsert(
                &mut slots,
                &mut index,
                corr_key.clone(),
                corr_key.clone(),
                SlotKind::Scalar,
                node_id,
            );
        }
    }

    ParameterManifest { slots }
}

/// G4.3 — render manifest state as diagnostics, reusing the same
/// `LintDiagnostic` shape `lint_contracts` (L1-L5) already produces and
/// `PublishResult.lint_diagnostics` already carries — no new surface to
/// invent, just a new producer feeding the existing one. Only suppliable
/// slots get a diagnostic (`ParameterManifest::suppliable` — an
/// element-scoped slot is never presented as suppliable, so it never gets
/// a "supply this" prompt; its role is documented on the collection slot's
/// message instead).
pub(crate) fn manifest_diagnostics(manifest: &ParameterManifest) -> Vec<LintDiagnostic> {
    manifest
        .suppliable()
        .map(|slot| {
            let message = match &slot.kind {
                SlotKind::Scalar => {
                    format!("This template needs '{}' supplied before it can run.", slot.name)
                }
                SlotKind::Collection { element_shape } if element_shape.is_empty() => format!(
                    "This template needs a '{}' collection supplied before it can run.",
                    slot.name
                ),
                SlotKind::Collection { element_shape } => format!(
                    "This template needs a '{}' collection supplied before it can run \
                     (each element resolves: {}).",
                    slot.name,
                    element_shape.join(", ")
                ),
                SlotKind::ElementScoped { .. } => unreachable!(
                    "ParameterManifest::suppliable() excludes ElementScoped by construction"
                ),
            };
            LintDiagnostic {
                rule: "P1".to_string(),
                level: LintLevel::Info,
                message,
                node_id: slot.node_ids.first().cloned(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::VerbContract;
    use crate::dto::{EdgeDto, FlagCondition, FlagOp, FlagValue};
    use bpmn_lite_compiler::{Expression, FfiInputBinding, IrLiteral};
    use bpmn_lite_types::DataObjectType;

    fn wf(nodes: Vec<NodeDto>, edges: Vec<EdgeDto>) -> WorkflowGraphDto {
        WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes,
            edges,
            default_guard_budget: None,
            default_retry_policy: None,
        }
    }

    /// T-MANIFEST-1: an edge condition on a flag no task writes → one
    /// scalar slot.
    #[test]
    fn t_manifest_1_scalar_from_unproduced_condition() {
        let dto = wf(
            vec![
                NodeDto::Start {
                    id: "start".into(),
                },
                NodeDto::ExclusiveGateway { id: "xor".into() },
                NodeDto::ServiceTask {
                    id: "yes".into(),
                    task_type: "do_yes".into(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".into(),
                    terminate: false,
                },
            ],
            vec![
                EdgeDto {
                    from: "start".into(),
                    to: "xor".into(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".into(),
                    to: "yes".into(),
                    condition: Some(FlagCondition {
                        flag: "client_reference".into(),
                        op: FlagOp::Eq,
                        value: FlagValue::Bool(true),
                    }),
                    is_default: false,
                    on_error: None,
                },
            ],
        );
        let manifest = derive_parameter_manifest(&dto, &ContractRegistry::new());
        assert_eq!(manifest.slots.len(), 1);
        assert_eq!(manifest.slots[0].name, "client_reference");
        assert_eq!(manifest.slots[0].kind, SlotKind::Scalar);
    }

    /// T-MANIFEST-2: a flag written by a registered task's contract is
    /// resolved internally → no slot.
    #[test]
    fn t_manifest_2_produced_flag_is_not_a_slot() {
        let dto = wf(
            vec![
                NodeDto::Start {
                    id: "start".into(),
                },
                NodeDto::ServiceTask {
                    id: "check".into(),
                    task_type: "check_sanctions".into(),
                    bpmn_id: None,
                },
                NodeDto::ExclusiveGateway { id: "xor".into() },
                NodeDto::ServiceTask {
                    id: "approve".into(),
                    task_type: "approve".into(),
                    bpmn_id: None,
                },
            ],
            vec![
                EdgeDto {
                    from: "start".into(),
                    to: "check".into(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "check".into(),
                    to: "xor".into(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".into(),
                    to: "approve".into(),
                    condition: Some(FlagCondition {
                        flag: "sanctions_clear".into(),
                        op: FlagOp::Eq,
                        value: FlagValue::Bool(true),
                    }),
                    is_default: false,
                    on_error: None,
                },
            ],
        );
        let mut registry = ContractRegistry::new();
        registry.register(VerbContract {
            task_type: "check_sanctions".into(),
            reads_flags: HashSet::new(),
            writes_flags: ["sanctions_clear".to_string()].into(),
            may_raise_errors: HashSet::new(),
            produces_correlation: vec![],
        });
        let manifest = derive_parameter_manifest(&dto, &registry);
        assert!(
            manifest.slots.is_empty(),
            "expected no slots, got {:?}",
            manifest.slots
        );
    }

    /// T-MANIFEST-3: gate scenario — a scalar (DataObject::Input), a
    /// collection (MultiInstance.collection_flag), and an element-scoped
    /// reference inside the MI body (an `inputs` VarRef binding), all
    /// typed correctly in one manifest. The element-scoped slot must never
    /// appear in `suppliable()`.
    #[test]
    fn t_manifest_3_gate_scenario_all_three_kinds() {
        let dto = wf(
            vec![
                NodeDto::Start {
                    id: "start".into(),
                },
                NodeDto::DataObject {
                    id: "client_ref_decl".into(),
                    name: "client_reference".into(),
                    type_decl: DataObjectType::Primitive(bpmn_lite_types::PrimitiveType::String),
                    role: DataObjectRole::Input,
                },
                NodeDto::MultiInstance {
                    id: "verify_each_director".into(),
                    task_type: "verify_director".into(),
                    bpmn_id: None,
                    collection_flag: "directors".into(),
                    declared_max: 10,
                    inputs: vec![FfiInputBinding {
                        target_field: "full_name".into(),
                        expression: Expression::VarRef(vec![
                            "director".to_string(),
                            "name".to_string(),
                        ]),
                    }],
                },
                NodeDto::End {
                    id: "end".into(),
                    terminate: false,
                },
            ],
            vec![],
        );
        let manifest = derive_parameter_manifest(&dto, &ContractRegistry::new());

        let scalar = manifest
            .slots
            .iter()
            .find(|s| s.name == "client_reference")
            .expect("scalar slot present");
        assert_eq!(scalar.kind, SlotKind::Scalar);

        let collection = manifest
            .slots
            .iter()
            .find(|s| s.name == "directors")
            .expect("collection slot present");
        assert_eq!(
            collection.kind,
            SlotKind::Collection {
                element_shape: vec!["full_name".to_string()]
            }
        );

        let element_scoped = manifest
            .slots
            .iter()
            .find(|s| s.name == "director.name")
            .expect("element-scoped slot present");
        assert_eq!(
            element_scoped.kind,
            SlotKind::ElementScoped {
                collection_slot: "directors".to_string(),
                field: "full_name".to_string(),
            }
        );

        let suppliable_names: Vec<&str> =
            manifest.suppliable().map(|s| s.name.as_str()).collect();
        assert!(suppliable_names.contains(&"client_reference"));
        assert!(suppliable_names.contains(&"directors"));
        assert!(
            !suppliable_names.contains(&"director.name"),
            "an element-scoped reference must never be presented as suppliable, got {:?}",
            suppliable_names
        );
        assert_eq!(
            manifest.slots.len(),
            3,
            "expected exactly scalar + collection + element-scoped, got {:?}",
            manifest.slots
        );
    }

    /// T-MANIFEST-4: a literal MI input binding (not a VarRef) contributes
    /// to the collection's element_shape but produces no separate
    /// element-scoped slot — nothing there is a reference to resolve.
    #[test]
    fn t_manifest_4_literal_mi_input_is_not_a_slot() {
        let dto = wf(
            vec![NodeDto::MultiInstance {
                id: "mi".into(),
                task_type: "noop".into(),
                bpmn_id: None,
                collection_flag: "items".into(),
                declared_max: 3,
                inputs: vec![FfiInputBinding {
                    target_field: "mode".into(),
                    expression: Expression::Literal(IrLiteral::String("fixed".into())),
                }],
            }],
            vec![],
        );
        let manifest = derive_parameter_manifest(&dto, &ContractRegistry::new());
        assert_eq!(manifest.slots.len(), 1, "only the collection slot");
        assert_eq!(manifest.slots[0].name, "items");
    }

    /// T-MANIFEST-5: `known_workflow_inputs` allow-list flags are still
    /// unresolved (they need external supply, that's what the allow-list
    /// means) even though L1 would only warn, not error, on them.
    #[test]
    fn t_manifest_5_known_workflow_input_is_still_a_slot() {
        let dto = wf(
            vec![
                NodeDto::Start {
                    id: "start".into(),
                },
                NodeDto::ExclusiveGateway { id: "xor".into() },
            ],
            vec![EdgeDto {
                from: "start".into(),
                to: "xor".into(),
                condition: Some(FlagCondition {
                    flag: "orch_high_risk".into(),
                    op: FlagOp::Eq,
                    value: FlagValue::Bool(true),
                }),
                is_default: false,
                on_error: None,
            }],
        );
        let mut registry = ContractRegistry::new();
        registry.add_known_input("orch_high_risk");
        let manifest = derive_parameter_manifest(&dto, &registry);
        assert_eq!(manifest.slots.len(), 1);
        assert_eq!(manifest.slots[0].name, "orch_high_risk");
    }

    /// T-MANIFEST-8 — blind-review regression: two MI regions sharing the
    /// same `collection_flag` (a legitimate shape — two loop bodies over
    /// the same external array) must merge into ONE collection slot whose
    /// `element_shape` is the union of both regions' fields, not silently
    /// keep only whichever region the walk saw first.
    #[test]
    fn t_manifest_8_two_mi_regions_sharing_a_collection_merge_element_shape() {
        let dto = wf(
            vec![
                NodeDto::MultiInstance {
                    id: "mi_a".into(),
                    task_type: "verify_director_identity".into(),
                    bpmn_id: None,
                    collection_flag: "directors".into(),
                    declared_max: 10,
                    inputs: vec![FfiInputBinding {
                        target_field: "full_name".into(),
                        expression: Expression::VarRef(vec![
                            "director".to_string(),
                            "name".to_string(),
                        ]),
                    }],
                },
                NodeDto::MultiInstance {
                    id: "mi_b".into(),
                    task_type: "check_director_sanctions".into(),
                    bpmn_id: None,
                    collection_flag: "directors".into(),
                    declared_max: 10,
                    inputs: vec![FfiInputBinding {
                        target_field: "role".into(),
                        expression: Expression::VarRef(vec![
                            "director".to_string(),
                            "role".to_string(),
                        ]),
                    }],
                },
            ],
            vec![],
        );
        let manifest = derive_parameter_manifest(&dto, &ContractRegistry::new());
        let collection_slots: Vec<_> = manifest
            .slots
            .iter()
            .filter(|s| s.name == "directors")
            .collect();
        assert_eq!(
            collection_slots.len(),
            1,
            "expected exactly one merged collection slot, got {:?}",
            manifest.slots
        );
        assert_eq!(
            collection_slots[0].kind,
            SlotKind::Collection {
                element_shape: vec!["full_name".to_string(), "role".to_string()]
            },
            "element_shape must be the union of both MI regions' fields"
        );
        assert_eq!(
            collection_slots[0].node_ids,
            vec!["mi_a".to_string(), "mi_b".to_string()]
        );
    }

    /// T-MANIFEST-7 — G4.3: `manifest_diagnostics` emits one diagnostic per
    /// suppliable slot and NONE for the element-scoped slot — Gate G4's
    /// "never presented as suppliable" holds at the diagnostic-rendering
    /// layer too, not just in `suppliable()`'s iterator.
    #[test]
    fn t_manifest_7_diagnostics_skip_element_scoped() {
        let dto = wf(
            vec![
                NodeDto::DataObject {
                    id: "client_ref_decl".into(),
                    name: "client_reference".into(),
                    type_decl: DataObjectType::Primitive(bpmn_lite_types::PrimitiveType::String),
                    role: DataObjectRole::Input,
                },
                NodeDto::MultiInstance {
                    id: "verify_each_director".into(),
                    task_type: "verify_director".into(),
                    bpmn_id: None,
                    collection_flag: "directors".into(),
                    declared_max: 10,
                    inputs: vec![FfiInputBinding {
                        target_field: "full_name".into(),
                        expression: Expression::VarRef(vec![
                            "director".to_string(),
                            "name".to_string(),
                        ]),
                    }],
                },
            ],
            vec![],
        );
        let manifest = derive_parameter_manifest(&dto, &ContractRegistry::new());
        let diags = manifest_diagnostics(&manifest);
        assert_eq!(diags.len(), 2, "expected scalar + collection only, got {diags:?}");
        assert!(diags.iter().all(|d| d.level == LintLevel::Info));
        assert!(diags
            .iter()
            .any(|d| d.message.contains("client_reference")));
        assert!(diags.iter().any(|d| d.message.contains("directors")
            && d.message.contains("full_name")));
        assert!(
            !diags.iter().any(|d| d.message.contains("director.name")),
            "element-scoped reference must not get its own suppliable diagnostic, got {diags:?}"
        );
    }

    /// T-MANIFEST-6: a `DataObject` explicitly declared `Internal`
    /// suppresses the slot even if no task produces it — an explicit
    /// in-workflow declaration overrides the heuristic.
    #[test]
    fn t_manifest_6_declared_internal_overrides_heuristic() {
        let dto = wf(
            vec![
                NodeDto::DataObject {
                    id: "scratch_decl".into(),
                    name: "scratch_flag".into(),
                    type_decl: DataObjectType::Primitive(bpmn_lite_types::PrimitiveType::Bool),
                    role: DataObjectRole::Internal,
                },
                NodeDto::Start {
                    id: "start".into(),
                },
                NodeDto::ExclusiveGateway { id: "xor".into() },
            ],
            vec![EdgeDto {
                from: "start".into(),
                to: "xor".into(),
                condition: Some(FlagCondition {
                    flag: "scratch_flag".into(),
                    op: FlagOp::Eq,
                    value: FlagValue::Bool(true),
                }),
                is_default: false,
                on_error: None,
            }],
        );
        let manifest = derive_parameter_manifest(&dto, &ContractRegistry::new());
        assert!(
            manifest.slots.is_empty(),
            "expected no slots, got {:?}",
            manifest.slots
        );
    }
}
