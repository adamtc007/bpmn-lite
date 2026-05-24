//! Manifest YAML load + structural validation + round-trip tests.

use crate::{Manifest, ManifestError};

const OB_POC_MANIFEST: &str = r#"
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"
generated_from_snapshot: "sha256:abc123"
min_consumer_manifest_version: "1.0"
breaking_changes_since: []

verbs:
  - id: "cbu.create"
    signature:
      inputs:
        - name: "name"
          type: "String"
          required: true
        - name: "client_type"
          type: "CbuClientType"
          required: true
      output:
        produces: "CBU"
    effect_class: "idempotent_ensure"
    coordination_policy: "UniqueInsert"
    transaction_policy: "AtomicShort"
    resource_dependencies:
      - kind: "NaturalKey"
        from_input: "name"
        entity_type: "CBU"
    authority_required: "cbu.write"
    description: "Create a new CBU entity."
  - id: "cbu.add-product"
    signature:
      inputs:
        - name: "cbu"
          type: "CBU"
          required: true
        - name: "product_type"
          type: "ProductType"
          required: true
      output:
        produces: null
    effect_class: "idempotent_ensure"
    authority_required: "cbu.write"

decisions: []

types:
  - name: "CBU"
    kind: "entity"
    description: "Custody Banking Unit."
    uuid_type: "UUIDv7"
  - name: "CbuClientType"
    kind: "enum"
    values: ["fund", "corporate", "trust"]
  - name: "ProductType"
    kind: "enum"
    values: ["custody", "fa", "ta"]
"#;

const DMN_LITE_MANIFEST: &str = r#"
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
        required: true
    output:
      type: "CbuType"
      enum_values: ["fund", "corporate", "trust"]
    description: "Routes CBU to product attachment path."

types:
  - name: "CbuType"
    kind: "enum"
    values: ["fund", "corporate", "trust"]
  - name: "CbuClientType"
    kind: "enum"
    values: ["fund", "corporate", "trust"]
"#;

#[test]
fn loads_valid_ob_poc_manifest() {
    let m = Manifest::load_from_yaml(OB_POC_MANIFEST).expect("valid manifest");
    assert_eq!(m.domain, "ob-poc");
    assert_eq!(m.catalogue_version, "v1.0.0");
    assert_eq!(m.verbs.len(), 2);
    assert!(m.decisions.is_empty());
    assert_eq!(m.types.len(), 3);
}

#[test]
fn loads_valid_dmn_lite_manifest() {
    let m = Manifest::load_from_yaml(DMN_LITE_MANIFEST).expect("valid manifest");
    assert_eq!(m.domain, "dmn-lite");
    assert!(m.verbs.is_empty());
    assert_eq!(m.decisions.len(), 1);
}

#[test]
fn lookup_verb_finds_declared_verb() {
    let m = Manifest::load_from_yaml(OB_POC_MANIFEST).expect("valid");
    let v = m.lookup_verb("cbu.create").expect("cbu.create present");
    assert_eq!(v.effect_class, "idempotent_ensure");
    assert_eq!(v.authority_required, "cbu.write");
    assert_eq!(v.signature.inputs.len(), 2);
    assert_eq!(
        v.signature
            .output
            .as_ref()
            .and_then(|o| o.produces.as_deref()),
        Some("CBU")
    );
}

#[test]
fn lookup_verb_returns_none_for_unknown() {
    let m = Manifest::load_from_yaml(OB_POC_MANIFEST).expect("valid");
    assert!(m.lookup_verb("cbu.nope").is_none());
}

#[test]
fn lookup_decision_finds_declared_decision() {
    let m = Manifest::load_from_yaml(DMN_LITE_MANIFEST).expect("valid");
    let d = m.lookup_decision("cbu_type_routing").expect("present");
    assert_eq!(d.output.type_name, "CbuType");
    assert_eq!(d.output.enum_values.len(), 3);
}

#[test]
fn lookup_type_finds_declared_type() {
    let m = Manifest::load_from_yaml(OB_POC_MANIFEST).expect("valid");
    let t = m.lookup_type("CBU").expect("CBU present");
    assert_eq!(t.kind, "entity");
    assert_eq!(t.uuid_type.as_deref(), Some("UUIDv7"));

    let e = m.lookup_type("CbuClientType").expect("present");
    assert_eq!(e.kind, "enum");
    assert_eq!(e.values.len(), 3);
}

#[test]
fn verb_ids_iter_lists_all_verbs() {
    let m = Manifest::load_from_yaml(OB_POC_MANIFEST).expect("valid");
    let ids: Vec<_> = m.verb_ids().collect();
    assert_eq!(ids, vec!["cbu.create", "cbu.add-product"]);
}

#[test]
fn rejects_empty_manifest_version() {
    let yaml = r#"
manifest_version: ""
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"
"#;
    let err = Manifest::load_from_yaml(yaml).expect_err("should reject");
    assert!(matches!(err, ManifestError::Validation(_)), "got {err:?}");
}

#[test]
fn rejects_duplicate_verb_id() {
    let yaml = r#"
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"
verbs:
  - id: "cbu.create"
    signature: { inputs: [] }
    effect_class: "idempotent_ensure"
    authority_required: "cbu.write"
  - id: "cbu.create"
    signature: { inputs: [] }
    effect_class: "idempotent_ensure"
    authority_required: "cbu.write"
"#;
    let err = Manifest::load_from_yaml(yaml).expect_err("dup should reject");
    match err {
        ManifestError::Validation(msg) => assert!(msg.contains("duplicate verb id")),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn rejects_empty_effect_class() {
    let yaml = r#"
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"
verbs:
  - id: "cbu.create"
    signature: { inputs: [] }
    effect_class: ""
    authority_required: "cbu.write"
"#;
    let err = Manifest::load_from_yaml(yaml).expect_err("should reject");
    assert!(matches!(err, ManifestError::Validation(_)), "got {err:?}");
}

#[test]
fn rejects_unknown_type_kind() {
    let yaml = r#"
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"
types:
  - name: "Mystery"
    kind: "wormhole"
"#;
    let err = Manifest::load_from_yaml(yaml).expect_err("should reject");
    match err {
        ManifestError::Validation(msg) => assert!(msg.contains("unknown kind")),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn rejects_enum_with_no_values() {
    let yaml = r#"
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"
types:
  - name: "Empty"
    kind: "enum"
    values: []
"#;
    let err = Manifest::load_from_yaml(yaml).expect_err("should reject");
    match err {
        ManifestError::Validation(msg) => assert!(msg.contains("at least one value")),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn rejects_duplicate_type_name() {
    let yaml = r#"
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"
types:
  - name: "CBU"
    kind: "entity"
  - name: "CBU"
    kind: "entity"
"#;
    let err = Manifest::load_from_yaml(yaml).expect_err("should reject");
    match err {
        ManifestError::Validation(msg) => assert!(msg.contains("duplicate type name")),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn round_trip_through_yaml_preserves_lookup() {
    let m = Manifest::load_from_yaml(OB_POC_MANIFEST).expect("valid");
    let serialised = m.to_yaml().expect("serialise");
    let m2 = Manifest::load_from_yaml(&serialised).expect("re-parse");
    assert_eq!(m2.domain, m.domain);
    assert_eq!(m2.catalogue_version, m.catalogue_version);
    assert_eq!(m2.verbs.len(), m.verbs.len());
    // Indexes must be rebuilt on the round-tripped manifest.
    assert!(m2.lookup_verb("cbu.create").is_some());
    assert!(m2.lookup_type("CbuClientType").is_some());
}

#[test]
fn parse_error_for_malformed_yaml() {
    let err = Manifest::load_from_yaml("not: valid: yaml: :::").expect_err("malformed");
    assert!(matches!(err, ManifestError::Parse(_)), "got {err:?}");
}

// ── T2A.3 — round-trip without loss across every v0.6 §7 field ───────
//
// Build a manifest that exercises *every* optional field in §7
// (`generated_from_snapshot`, `min_consumer_manifest_version`,
// `breaking_changes_since`, the optional VerbEntry knobs, all type
// kinds), serialise to YAML, parse the YAML back, and assert
// field-by-field equivalence. Guards against accidental
// `#[serde(skip_serializing_if = ...)]` or rename drift.

const FULL_SURFACE_MANIFEST: &str = r#"
manifest_version: "1.0"
domain: "ob-poc"
catalogue_version: "v1.0.0"
generated_at: "2026-05-20T10:00:00Z"
generated_from_snapshot: "sha256:abc123"
min_consumer_manifest_version: "1.0"
breaking_changes_since:
  - "v0.9.0"
  - "v0.8.0"

verbs:
  - id: "cbu.create"
    signature:
      inputs:
        - name: "name"
          type: "String"
          required: true
        - name: "client_type"
          type: "CbuClientType"
          required: false
      output:
        produces: "CBU"
    effect_class: "idempotent_ensure"
    coordination_policy: "UniqueInsert"
    transaction_policy: "AtomicShort"
    resource_dependencies:
      - kind: "NaturalKey"
        from_input: "name"
        entity_type: "CBU"
      - kind: "EntityUuid"
        from_input: "client_type"
    fsm_applicability:
      entity: "CBU"
      preconditions:
        - "absent"
      postconditions:
        - "present"
        - "active"
    authority_required: "cbu.write"
    description: "Create a new CBU entity."

decisions:
  - id: "cbu_type_routing"
    inputs:
      - name: "cbu_client_type"
        type: "CbuClientType"
        required: true
    output:
      type: "CbuType"
      enum_values:
        - "fund"
        - "corporate"
        - "trust"
    description: "Routes CBU."

types:
  - name: "CBU"
    kind: "entity"
    description: "Custody Banking Unit."
    uuid_type: "UUIDv7"
  - name: "CbuType"
    kind: "enum"
    values: ["fund", "corporate", "trust"]
  - name: "String"
    kind: "primitive"
    description: "Native String."
"#;

#[test]
fn round_trip_preserves_every_top_level_field() {
    let m = Manifest::load_from_yaml(FULL_SURFACE_MANIFEST).expect("valid");
    let yaml = m.to_yaml().expect("serialise");
    let m2 = Manifest::load_from_yaml(&yaml).expect("re-parse");

    assert_eq!(m2.manifest_version, m.manifest_version);
    assert_eq!(m2.domain, m.domain);
    assert_eq!(m2.catalogue_version, m.catalogue_version);
    assert_eq!(m2.generated_at, m.generated_at);
    assert_eq!(m2.generated_from_snapshot, m.generated_from_snapshot);
    assert_eq!(
        m2.min_consumer_manifest_version,
        m.min_consumer_manifest_version
    );
    assert_eq!(m2.breaking_changes_since, m.breaking_changes_since);
}

#[test]
fn round_trip_preserves_verb_entry_optional_fields() {
    let m = Manifest::load_from_yaml(FULL_SURFACE_MANIFEST).expect("valid");
    let yaml = m.to_yaml().expect("serialise");
    let m2 = Manifest::load_from_yaml(&yaml).expect("re-parse");
    let v = m2.lookup_verb("cbu.create").expect("verb survives");

    assert_eq!(v.effect_class, "idempotent_ensure");
    assert_eq!(v.coordination_policy.as_deref(), Some("UniqueInsert"));
    assert_eq!(v.transaction_policy.as_deref(), Some("AtomicShort"));
    assert_eq!(v.authority_required, "cbu.write");
    assert_eq!(v.description.as_deref(), Some("Create a new CBU entity."));

    // Inputs preserved including required = false.
    assert_eq!(v.signature.inputs.len(), 2);
    assert_eq!(v.signature.inputs[0].name, "name");
    assert!(v.signature.inputs[0].required);
    assert_eq!(v.signature.inputs[1].name, "client_type");
    assert!(!v.signature.inputs[1].required);

    // Output produces survives the Option<String> path.
    assert_eq!(
        v.signature
            .output
            .as_ref()
            .and_then(|o| o.produces.as_deref()),
        Some("CBU")
    );

    // Resource deps survive in order, including optional entity_type.
    assert_eq!(v.resource_dependencies.len(), 2);
    assert_eq!(v.resource_dependencies[0].kind, "NaturalKey");
    assert_eq!(
        v.resource_dependencies[0].entity_type.as_deref(),
        Some("CBU")
    );
    assert_eq!(v.resource_dependencies[1].kind, "EntityUuid");
    assert!(v.resource_dependencies[1].entity_type.is_none());

    // FSM applicability survives both pre + post lists.
    let fsm = v.fsm_applicability.as_ref().expect("fsm");
    assert_eq!(fsm.entity, "CBU");
    assert_eq!(fsm.preconditions, vec!["absent"]);
    assert_eq!(fsm.postconditions, vec!["present", "active"]);
}

#[test]
fn round_trip_preserves_decision_entry_and_enum_output() {
    let m = Manifest::load_from_yaml(FULL_SURFACE_MANIFEST).expect("valid");
    let yaml = m.to_yaml().expect("serialise");
    let m2 = Manifest::load_from_yaml(&yaml).expect("re-parse");
    let d = m2.lookup_decision("cbu_type_routing").expect("present");

    assert_eq!(d.output.type_name, "CbuType");
    assert_eq!(d.output.enum_values, vec!["fund", "corporate", "trust"]);
    assert_eq!(d.description.as_deref(), Some("Routes CBU."));
    assert_eq!(d.inputs.len(), 1);
    assert_eq!(d.inputs[0].name, "cbu_client_type");
    assert_eq!(d.inputs[0].type_name, "CbuClientType");
}

#[test]
fn round_trip_preserves_all_type_kinds_and_their_attributes() {
    let m = Manifest::load_from_yaml(FULL_SURFACE_MANIFEST).expect("valid");
    let yaml = m.to_yaml().expect("serialise");
    let m2 = Manifest::load_from_yaml(&yaml).expect("re-parse");

    let entity = m2.lookup_type("CBU").expect("CBU");
    assert_eq!(entity.kind, "entity");
    assert_eq!(entity.uuid_type.as_deref(), Some("UUIDv7"));
    assert_eq!(entity.description.as_deref(), Some("Custody Banking Unit."));

    let enum_t = m2.lookup_type("CbuType").expect("enum");
    assert_eq!(enum_t.kind, "enum");
    assert_eq!(enum_t.values, vec!["fund", "corporate", "trust"]);

    let prim = m2.lookup_type("String").expect("prim");
    assert_eq!(prim.kind, "primitive");
    assert_eq!(prim.description.as_deref(), Some("Native String."));
}

#[test]
fn round_trip_preserves_lookup_indexes_after_to_yaml() {
    // The `verb_index` / `decision_index` / `type_index` fields are
    // `#[serde(skip)]` — they must be rebuilt by load_from_yaml on the
    // post-round-trip manifest. Without rebuild, lookup_verb would
    // return None even though the verb is present in the Vec.
    let m = Manifest::load_from_yaml(FULL_SURFACE_MANIFEST).expect("valid");
    let yaml = m.to_yaml().expect("serialise");
    let m2 = Manifest::load_from_yaml(&yaml).expect("re-parse");

    assert!(m2.lookup_verb("cbu.create").is_some());
    assert!(m2.lookup_decision("cbu_type_routing").is_some());
    assert!(m2.lookup_type("CBU").is_some());
    assert!(m2.lookup_type("CbuType").is_some());
    assert!(m2.lookup_type("String").is_some());
}
