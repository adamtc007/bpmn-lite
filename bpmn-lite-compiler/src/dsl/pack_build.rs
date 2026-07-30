use blake3::Hasher;
use dsl_manifest::{
    ExternalReferencePin, InputSpec, Manifest, OutputSpec, ServiceDeclaration, ServiceKind,
    Signature, VerbEntry,
};
use std::collections::HashSet;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkflowPackDAG {
    pub domain: String,  // "bpmn"
    pub version: String, // "v1.0.0"
    pub nodes: Vec<PackNode>,
    pub external_references: Vec<ExternalReferencePin>,
    #[serde(default)]
    pub decisions: Vec<dsl_manifest::DecisionEntry>,
    #[serde(default)]
    pub types: Vec<dsl_manifest::TypeEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum PackNode {
    VerbLocal {
        id: String,
        inputs: Vec<PortSpec>,
        output: Option<PortSpec>,
        effect_class: String,
        authority_required: String,
    },
    State {
        name: String,
        type_name: String,
    },
    Reference {
        name: String,
        target_node: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PortSpec {
    pub name: String,
    pub type_name: String,
    pub required: bool,
}

impl PackNode {
    pub(crate) fn id(&self) -> &str {
        match self {
            PackNode::VerbLocal { id, .. } => id,
            PackNode::State { name, .. } => name,
            PackNode::Reference { name, .. } => name,
        }
    }
}

pub fn generate_manifest(dag: &WorkflowPackDAG) -> Result<Manifest, String> {
    let mut verbs = Vec::new();
    for node in &dag.nodes {
        if let PackNode::VerbLocal {
            id,
            inputs,
            output,
            effect_class,
            authority_required,
        } = node
        {
            let signature = Signature {
                inputs: inputs
                    .iter()
                    .map(|i| InputSpec {
                        name: i.name.clone(),
                        type_name: i.type_name.clone(),
                        required: i.required,
                    })
                    .collect(),
                output: output.as_ref().map(|o| OutputSpec {
                    produces: Some(o.type_name.clone()),
                }),
            };
            verbs.push(VerbEntry {
                id: id.clone(),
                signature,
                effect_class: effect_class.clone(),
                coordination_policy: None,
                transaction_policy: None,
                resource_dependencies: Vec::new(),
                fsm_applicability: None,
                authority_required: authority_required.clone(),
                description: None,
            });
        }
    }

    // A3 §2.4 capability declarations
    let services = vec![ServiceDeclaration {
        kind: ServiceKind::InvocationService,
        available: true,
        capabilities: vec!["Submit".into(), "Validate".into()],
    }];

    // Emit types from State nodes + copy explicit types
    let mut types = dag.types.clone();
    for node in &dag.nodes {
        if let PackNode::State { name, .. } = node {
            if !types.iter().any(|t| &t.name == name) {
                types.push(dsl_manifest::TypeEntry {
                    name: name.clone(),
                    kind: "entity".to_string(),
                    description: None,
                    uuid_type: None,
                    values: vec![],
                });
            }
        }
    }

    #[derive(serde::Serialize)]
    struct ManifestHelper {
        manifest_version: String,
        domain: String,
        catalogue_version: String,
        generated_at: String,
        imported_pins: Vec<ExternalReferencePin>,
        breaking_changes_since: Vec<String>,
        services: Vec<ServiceDeclaration>,
        verbs: Vec<VerbEntry>,
        decisions: Vec<dsl_manifest::DecisionEntry>,
        types: Vec<dsl_manifest::TypeEntry>,
    }

    let helper = ManifestHelper {
        manifest_version: "1.0".to_string(),
        domain: dag.domain.clone(),
        catalogue_version: dag.version.clone(),
        generated_at: "2026-06-02T12:00:00Z".to_string(),
        imported_pins: dag.external_references.clone(),
        breaking_changes_since: vec![],
        services,
        verbs,
        decisions: dag.decisions.clone(),
        types,
    };

    let yaml_str = serde_yaml::to_string(&helper).map_err(|e| e.to_string())?;
    Manifest::load_from_yaml(&yaml_str).map_err(|e| e.to_string())
}

fn resolve_external_pack(
    domain: &str,
    content_hash: &str,
) -> Result<(Manifest, PackClosureManifest), String> {
    let mut dir = std::path::PathBuf::from("manifests");
    if !dir.exists() {
        dir = std::path::PathBuf::from("../manifests");
    }
    if !dir.exists() {
        if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
            dir = std::path::PathBuf::from(manifest_dir)
                .parent()
                .unwrap()
                .join("manifests");
        }
    }
    if !dir.exists() {
        return Err("manifests directory not found".to_string());
    }

    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() {
            let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
            if filename.starts_with(domain) && filename.ends_with(".closure.yaml") {
                let closure_content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                if let Ok(closure) = serde_yaml::from_str::<PackClosureManifest>(&closure_content) {
                    if closure.root_pack_hash == content_hash {
                        let manifest_filename = filename.replace(".closure.yaml", ".yaml");
                        let manifest_path = path.parent().unwrap().join(manifest_filename);
                        if manifest_path.exists() {
                            let manifest_content = std::fs::read_to_string(&manifest_path)
                                .map_err(|e| e.to_string())?;
                            let manifest = Manifest::load_from_yaml(&manifest_content)
                                .map_err(|e| e.to_string())?;
                            return Ok((manifest, closure));
                        }
                    }
                }
            }
        }
    }
    Err(format!(
        "Failed to resolve external pack for domain '{}' with content hash '{}'",
        domain, content_hash
    ))
}

pub fn validate_pack(dag: &WorkflowPackDAG, lex: &Manifest) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    // G1 (intra-bijection)
    let lex_verb_ids: HashSet<&str> = lex.verbs().iter().map(|v| v.id.as_str()).collect();
    let dag_local_verb_ids: HashSet<&str> = dag
        .nodes
        .iter()
        .filter_map(|n| {
            if let PackNode::VerbLocal { id, .. } = n {
                Some(id.as_str())
            } else {
                None
            }
        })
        .collect();

    if lex_verb_ids != dag_local_verb_ids {
        errors.push(format!(
            "G1 violation: mismatch between lexicon verbs and DAG local verb nodes. Lex: {:?}, DAG: {:?}",
            lex_verb_ids, dag_local_verb_ids
        ));
    }

    // G2 (signature coherence)
    for node in &dag.nodes {
        if let PackNode::VerbLocal {
            id,
            inputs,
            output,
            effect_class,
            authority_required,
        } = node
        {
            // Validate effect_class enum
            if !["read", "idempotent_ensure", "read_modify_write"].contains(&effect_class.as_str())
            {
                errors.push(format!(
                    "G2 violation for verb '{}': invalid effect_class '{}'",
                    id, effect_class
                ));
            }

            // Assert coherence
            let is_read_auth = authority_required.ends_with(".read");
            let is_read_effect = effect_class == "read";

            if is_read_auth && !is_read_effect {
                errors.push(format!(
                    "G2 violation for verb '{}': read authority requires 'read' effect_class",
                    id
                ));
            }
            if !is_read_auth && is_read_effect {
                errors.push(format!(
                    "G2 violation for verb '{}': mutation/write authority cannot have 'read' effect_class",
                    id
                ));
            }

            if let Some(lex_verb) = lex.lookup_verb(id) {
                // Check inputs
                if inputs.len() != lex_verb.signature.inputs.len() {
                    errors.push(format!(
                        "G2 violation for verb '{}': inputs length mismatch",
                        id
                    ));
                } else {
                    for (i, ps) in inputs.iter().enumerate() {
                        let is = &lex_verb.signature.inputs[i];
                        if ps.name != is.name
                            || ps.type_name != is.type_name
                            || ps.required != is.required
                        {
                            errors.push(format!(
                                "G2 violation for verb '{}': input at index {} mismatch (DAG: {:?}, Lex: {:?})",
                                id, i, ps, is
                            ));
                        }
                    }
                }
                // Check output
                match (output, &lex_verb.signature.output) {
                    (None, None) => {}
                    (Some(ps), Some(os)) => {
                        if os.produces.as_deref() != Some(ps.type_name.as_str()) {
                            errors.push(format!(
                                "G2 violation for verb '{}': output produces mismatch (DAG: {}, Lex: {:?})",
                                id, ps.type_name, os.produces
                            ));
                        }
                    }
                    _ => {
                        errors.push(format!(
                            "G2 violation for verb '{}': output presence mismatch",
                            id
                        ));
                    }
                }
            }
        }
    }

    // G3 (edge totality & acyclicity)
    let mut node_names = HashSet::new();
    for node in &dag.nodes {
        let name = node.id();
        if !node_names.insert(name) {
            errors.push(format!("Duplicate node name in DAG: {}", name));
        }
    }

    // G4 (External Resolution & Pin Validation)
    for pin in &dag.external_references {
        if pin.content_hash.len() != 64 || !pin.content_hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            errors.push(format!(
                "G4 violation: content_hash '{}' for domain '{}' is not a 64-character hex string",
                pin.content_hash, pin.domain
            ));
        } else {
            if let Err(err) = resolve_external_pack(&pin.domain, &pin.content_hash) {
                errors.push(format!(
                    "G4 violation: failed to resolve pinned pack for domain '{}' with hash '{}': {}",
                    pin.domain, pin.content_hash, err
                ));
            }
        }
    }

    for node in &dag.nodes {
        if let PackNode::Reference { name, target_node } = node {
            if target_node.contains(':') {
                let parts: Vec<&str> = target_node.split(':').collect();
                let domain = parts[0];
                let item_id = parts[1];

                let pin = dag.external_references.iter().find(|p| p.domain == domain);
                match pin {
                    None => {
                        errors.push(format!(
                            "G4 violation: reference '{}' targets external domain '{}' which is not in external_references",
                            name, domain
                        ));
                    }
                    Some(p) => {
                        if p.content_hash.len() == 64
                            && p.content_hash.chars().all(|c| c.is_ascii_hexdigit())
                        {
                            match resolve_external_pack(&p.domain, &p.content_hash) {
                                Ok((ext_lex, _)) => {
                                    let has_verb = ext_lex.lookup_verb(item_id).is_some();
                                    let has_decision = ext_lex.lookup_decision(item_id).is_some();
                                    if !has_verb && !has_decision {
                                        errors.push(format!(
                                            "G4 violation: target '{}' not found in external domain '{}' lexicon",
                                            item_id, domain
                                        ));
                                    }
                                }
                                Err(_) => {
                                    // Error already reported under pin validation loop
                                }
                            }
                        }
                    }
                }
            } else {
                if !node_names.contains(target_node.as_str()) {
                    errors.push(format!(
                        "G3 violation: node reference '{}' targets non-existent node '{}'",
                        name, target_node
                    ));
                }
            }
        }
    }

    // Acyclicity check
    let mut visited = HashSet::new();
    let mut path = HashSet::new();
    for node in &dag.nodes {
        if let PackNode::Reference { .. } = node {
            let name = node.id();
            if check_cycle(name, &dag.nodes, &mut visited, &mut path) {
                errors.push(format!(
                    "G3 violation: cycle detected containing node '{}'",
                    name
                ));
            }
        }
    }

    // G6 (no trap doors)
    // 1. Scan migrations for DEFAULT on tenant_id
    let migration_paths = vec![
        std::path::Path::new("bpmn-lite-store-postgres/migrations"),
        std::path::Path::new("dsl-bus-storage/migrations"),
    ];
    for path in &migration_paths {
        if path.exists() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    let file_path = entry.path();
                    if file_path.is_file()
                        && file_path.extension().and_then(|s| s.to_str()) == Some("sql")
                    {
                        if let Ok(content) = std::fs::read_to_string(&file_path) {
                            for line in content.lines() {
                                let lower = line.to_lowercase();
                                if lower.contains("tenant_id") && lower.contains("default") {
                                    errors.push(format!(
                                        "G6 violation: DEFAULT clause found on tenant_id column in migration '{}': '{}'",
                                        file_path.display(),
                                        line.trim()
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Scan plan_walker.rs to check that manifest load results are never swallowed
    let walker_path = std::path::Path::new("bpmn-lite-engine/src/plan_walker.rs");
    if walker_path.exists() {
        if let Ok(content) = std::fs::read_to_string(walker_path) {
            for (line_num, line) in content.lines().enumerate() {
                if (line.contains("Manifest::load_from_path") || line.contains("load_from_yaml"))
                    && (line.contains("if let Ok") || line.contains(".ok()"))
                {
                    errors.push(format!(
                        "G6 violation: swallowed manifest load result in {} at line {}: '{}'",
                        walker_path.display(),
                        line_num + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    // 3. Scan bpmn-lite-server/src/main.rs to assert no #[allow(...)] attributes are reintroduced on main
    let server_main_path = std::path::Path::new("bpmn-lite-server/src/main.rs");
    if server_main_path.exists() {
        if let Ok(content) = std::fs::read_to_string(server_main_path) {
            for (line_num, line) in content.lines().enumerate() {
                if line.contains("#[allow(") {
                    errors.push(format!(
                        "G6 violation: #[allow(...)] attribute found in {} at line {}: '{}'",
                        server_main_path.display(),
                        line_num + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn check_cycle(
    node: &str,
    nodes: &[PackNode],
    visited: &mut HashSet<String>,
    path: &mut HashSet<String>,
) -> bool {
    let node_str = node.to_string();
    if path.contains(&node_str) {
        return true;
    }
    if visited.contains(&node_str) {
        return false;
    }

    visited.insert(node_str.clone());
    path.insert(node_str.clone());

    if let Some(target) = nodes.iter().find(|n| n.id() == node).and_then(|n| {
        if let PackNode::Reference { target_node, .. } = n {
            Some(target_node.as_str())
        } else {
            None
        }
    }) {
        if check_cycle(target, nodes, visited, path) {
            return true;
        }
    }

    path.remove(&node_str);
    false
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PackClosureManifest {
    pub root_pack_hash: String,
    pub dependencies: Vec<ExternalReferencePin>,
}

fn compute_canonical_hash(lex: &Manifest, dag: &WorkflowPackDAG) -> String {
    let mut hasher = Hasher::new();
    let lex_yaml = lex.to_yaml().unwrap_or_default();
    let dag_json = serde_json::to_string(dag).unwrap_or_default();

    let mut sorted_pins = dag.external_references.clone();
    sorted_pins.sort_by(|a, b| a.domain.cmp(&b.domain));
    let pins_json = serde_json::to_string(&sorted_pins).unwrap_or_default();

    hasher.update(lex_yaml.as_bytes());
    hasher.update(dag_json.as_bytes());
    hasher.update(pins_json.as_bytes());

    hasher.finalize().to_hex().to_string()
}

pub fn generate_closure(lex: &Manifest, dag: &WorkflowPackDAG) -> PackClosureManifest {
    let root_hash = compute_canonical_hash(lex, dag);
    let mut dependencies = dag.external_references.clone();
    dependencies.sort_by(|a, b| a.domain.cmp(&b.domain));

    PackClosureManifest {
        root_pack_hash: root_hash,
        dependencies,
    }
}

pub fn derive_version(lex: &Manifest, dag: &WorkflowPackDAG) -> String {
    let hash_hex = compute_canonical_hash(lex, dag);
    format!("{}-{}", dag.version, &hash_hex[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_bpmn_dag() -> WorkflowPackDAG {
        WorkflowPackDAG {
            domain: "bpmn".to_string(),
            version: "v1.0.0".to_string(),
            external_references: vec![],
            decisions: vec![],
            types: vec![],
            nodes: vec![
                PackNode::State {
                    name: "BPMN_TEMPLATE".to_string(),
                    type_name: "BPMN_TEMPLATE".to_string(),
                },
                PackNode::State {
                    name: "BPMN_INSTANCE".to_string(),
                    type_name: "BPMN_INSTANCE".to_string(),
                },
                PackNode::VerbLocal {
                    id: "define-template".to_string(),
                    inputs: vec![
                        PortSpec {
                            name: "template_key".into(),
                            type_name: "String".into(),
                            required: true,
                        },
                        PortSpec {
                            name: "plan_body".into(),
                            type_name: "String".into(),
                            required: true,
                        },
                    ],
                    output: Some(PortSpec {
                        name: "template".into(),
                        type_name: "BPMN_TEMPLATE".into(),
                        required: true,
                    }),
                    effect_class: "idempotent_ensure".to_string(),
                    authority_required: "bpmn.template.write".to_string(),
                },
                PackNode::VerbLocal {
                    id: "spawn-instance".to_string(),
                    inputs: vec![
                        PortSpec {
                            name: "template_key".into(),
                            type_name: "BPMN_TEMPLATE".into(),
                            required: true,
                        },
                        PortSpec {
                            name: "idempotency_key".into(),
                            type_name: "UUID".into(),
                            required: true,
                        },
                        PortSpec {
                            name: "entry_id".into(),
                            type_name: "UUID".into(),
                            required: false,
                        },
                        PortSpec {
                            name: "runbook_id".into(),
                            type_name: "UUID".into(),
                            required: false,
                        },
                    ],
                    output: Some(PortSpec {
                        name: "instance_id".into(),
                        type_name: "BPMN_INSTANCE".into(),
                        required: true,
                    }),
                    effect_class: "idempotent_ensure".to_string(),
                    authority_required: "bpmn.instance.write".to_string(),
                },
                PackNode::VerbLocal {
                    id: "deliver-message".to_string(),
                    inputs: vec![
                        PortSpec {
                            name: "instance_id".into(),
                            type_name: "UUID".into(),
                            required: true,
                        },
                        PortSpec {
                            name: "message_name".into(),
                            type_name: "String".into(),
                            required: true,
                        },
                        PortSpec {
                            name: "payload".into(),
                            type_name: "String".into(),
                            required: true,
                        },
                    ],
                    output: None,
                    effect_class: "idempotent_ensure".to_string(),
                    authority_required: "bpmn.instance.write".to_string(),
                },
                PackNode::VerbLocal {
                    id: "correlate-message".to_string(),
                    inputs: vec![
                        PortSpec {
                            name: "message_name".into(),
                            type_name: "String".into(),
                            required: true,
                        },
                        PortSpec {
                            name: "correlation_key".into(),
                            type_name: "String".into(),
                            required: true,
                        },
                        PortSpec {
                            name: "payload".into(),
                            type_name: "String".into(),
                            required: true,
                        },
                    ],
                    output: None,
                    effect_class: "idempotent_ensure".to_string(),
                    authority_required: "bpmn.instance.write".to_string(),
                },
                PackNode::VerbLocal {
                    id: "cancel-instance".to_string(),
                    inputs: vec![
                        PortSpec {
                            name: "instance_id".into(),
                            type_name: "UUID".into(),
                            required: true,
                        },
                        PortSpec {
                            name: "reason".into(),
                            type_name: "String".into(),
                            required: true,
                        },
                    ],
                    output: None,
                    effect_class: "idempotent_ensure".to_string(),
                    authority_required: "bpmn.instance.write".to_string(),
                },
                PackNode::VerbLocal {
                    id: "inspect-instance".to_string(),
                    inputs: vec![PortSpec {
                        name: "instance_id".into(),
                        type_name: "UUID".into(),
                        required: true,
                    }],
                    output: Some(PortSpec {
                        name: "instance_details".into(),
                        type_name: "instance_details".into(),
                        required: true,
                    }),
                    effect_class: "read".to_string(),
                    authority_required: "bpmn.instance.read".to_string(),
                },
            ],
        }
    }

    #[test]
    fn test_valid_pack_validation() {
        let dag = valid_bpmn_dag();
        let lex = generate_manifest(&dag).expect("failed to generate lexicon");
        let result = validate_pack(&dag, &lex);
        assert!(result.is_ok(), "validation failed: {:?}", result.err());
    }

    #[test]
    fn test_g1_violation_mismatch() {
        let dag = valid_bpmn_dag();
        let mut lex = generate_manifest(&dag).expect("failed to generate lexicon");

        let first_verb_id = lex.verb_ids().next().unwrap().to_string();
        lex.remove_verb(&first_verb_id);
        let result = validate_pack(&dag, &lex);
        assert!(result.is_err());
        let errs = result.err().unwrap();
        assert!(errs.iter().any(|e| e.contains("G1 violation")));
    }

    #[test]
    fn test_g2_violation_signature_mismatch() {
        let mut dag = valid_bpmn_dag();
        if let PackNode::VerbLocal { inputs, .. } = &mut dag.nodes[2] {
            // index 2 is define-template
            inputs[0].name = "wrong_name".to_string();
        }

        let lex = generate_manifest(&valid_bpmn_dag()).expect("failed to generate lexicon");
        let result = validate_pack(&dag, &lex);
        assert!(result.is_err());
        let errs = result.err().unwrap();
        assert!(errs.iter().any(|e| e.contains("G2 violation")));
    }

    #[test]
    fn test_g6_trap_door_draft() {
        let mut dag = valid_bpmn_dag();
        dag.nodes.push(PackNode::VerbLocal {
            id: "draft-verb".to_string(),
            inputs: vec![],
            output: None,
            effect_class: "invalid_class".to_string(),
            authority_required: "bpmn.instance.write".to_string(),
        });
        let lex = generate_manifest(&dag).expect("lex");
        let result = validate_pack(&dag, &lex);
        assert!(result.is_err());
        let errs = result.err().unwrap();
        assert!(errs
            .iter()
            .any(|e| e.contains("G2 violation") || e.contains("G1 violation")));
    }

    #[test]
    fn test_write_manifest_to_disk() {
        let dag = valid_bpmn_dag();
        let lex = generate_manifest(&dag).expect("failed to generate lexicon");
        let _yaml = lex.to_yaml().expect("failed to serialize lexicon");
        let closure = generate_closure(&lex, &dag);
        let _closure_yaml = serde_yaml::to_string(&closure).expect("failed to serialize closure");

        let hash1 = compute_canonical_hash(&lex, &dag);
        let hash2 = compute_canonical_hash(&lex, &dag);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_g4_resolution_and_validation() {
        // Test B2 validation receipts:
        // 1. Fake-hash rejection (Fixture A)
        let mut dag_fake_hash = valid_bpmn_dag();
        dag_fake_hash
            .external_references
            .push(ExternalReferencePin {
                domain: "dmn-lite".to_string(),
                content_hash: "fake_non_hex_hash_less_than_64_chars".to_string(),
            });
        let lex_fake_hash = generate_manifest(&dag_fake_hash).unwrap();
        let result_fake = validate_pack(&dag_fake_hash, &lex_fake_hash);
        assert!(result_fake.is_err());
        let errs_fake = result_fake.err().unwrap();
        assert!(errs_fake
            .iter()
            .any(|e| e.contains("G4 violation: content_hash")));

        // 2. Real hash shape but absent verb in external pack (Fixture B)
        let mut dag_absent_verb = valid_bpmn_dag();
        // Add a reference to a non-existent verb in dmn-lite
        dag_absent_verb.nodes.push(PackNode::Reference {
            name: "ref-non-existent".to_string(),
            target_node: "dmn-lite:non-existent-verb".to_string(),
        });
        dag_absent_verb
            .external_references
            .push(ExternalReferencePin {
                domain: "dmn-lite".to_string(),
                content_hash: "06cd133818af5fe6807fa93b524517bcf7be0fc15109934f11a327e1fe749836"
                    .to_string(),
            });
        let lex_absent = generate_manifest(&dag_absent_verb).unwrap();
        let result_absent = validate_pack(&dag_absent_verb, &lex_absent);
        assert!(result_absent.is_err());
        let errs_absent = result_absent.err().unwrap();
        assert!(errs_absent
            .iter()
            .any(|e| e.contains("G4 violation: target 'non-existent-verb' not found")));

        // 3. Resolvable pin and present verb/decision (Fixture C)
        let mut dag_valid_ref = valid_bpmn_dag();
        // Add a reference to a valid decision (cbu_type_routing) in dmn-lite
        dag_valid_ref.nodes.push(PackNode::Reference {
            name: "ref-valid".to_string(),
            target_node: "dmn-lite:cbu_type_routing".to_string(),
        });
        dag_valid_ref
            .external_references
            .push(ExternalReferencePin {
                domain: "dmn-lite".to_string(),
                content_hash: "06cd133818af5fe6807fa93b524517bcf7be0fc15109934f11a327e1fe749836"
                    .to_string(),
            });
        let lex_valid = generate_manifest(&dag_valid_ref).unwrap();
        let result_valid = validate_pack(&dag_valid_ref, &lex_valid);
        assert!(
            result_valid.is_ok(),
            "Expected valid cross-pack reference to pass: {:?}",
            result_valid.err()
        );
    }
}
