//! Unit tests for the dmn-lite manifest exporter.

use super::*;

const CBU_TYPE_ROUTING: &str = r#"
(define-decision cbu_type_routing
  :hit-policy first
  :inputs  ((cbu-client-type :type enum :domain CbuClientType))
  :outputs ((cbu-type :type enum :domain CbuType))
  :rules
    ((rule fund
       :when ((cbu-client-type = FUND_MANDATE))
       :then ((cbu-type = fund)))
     (rule corporate
       :when ((cbu-client-type = CORPORATE))
       :then ((cbu-type = corporate)))
     (rule trust
       :when ((cbu-client-type = TRUST))
       :then ((cbu-type = trust)))
     (rule fallback
       :when (*)
       :then ((cbu-type = corporate)))))
"#;

const ALLOWLIST: &str = r#"
public_verbs: []
public_decisions:
  - cbu_type_routing
"#;

fn write_fixture(dir: &std::path::Path) {
    std::fs::write(dir.join("cbu_type_routing.dmn-lite"), CBU_TYPE_ROUTING).unwrap();
}

#[test]
fn export_cbu_type_routing_round_trips_through_dsl_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());

    let allow: Allowlist = serde_yaml::from_str(ALLOWLIST).unwrap();
    let cfg = ExporterConfig::new("dmn-lite", "v1.0.0");

    let yaml = export_to_yaml(tmp.path(), &allow, &cfg).expect("export");
    let manifest = dsl_manifest::Manifest::load_from_yaml(&yaml).expect("re-parse");

    assert_eq!(manifest.domain, "dmn-lite");
    assert_eq!(manifest.catalogue_version, "v1.0.0");
    let ids: Vec<_> = manifest.decision_ids().collect();
    assert_eq!(ids, vec!["cbu_type_routing"]);
    assert!(manifest.verbs.is_empty());
}

#[test]
fn output_enum_values_are_extracted_from_rule_assignments() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let allow: Allowlist = serde_yaml::from_str(ALLOWLIST).unwrap();
    let cfg = ExporterConfig::new("dmn-lite", "v1.0.0");
    let yaml = export_to_yaml(tmp.path(), &allow, &cfg).unwrap();
    let manifest = dsl_manifest::Manifest::load_from_yaml(&yaml).unwrap();

    let d = manifest.lookup_decision("cbu_type_routing").expect("present");
    assert_eq!(d.output.type_name, "CbuType");
    // Rule outputs: fund, corporate, trust, corporate (fallback).
    // Distinct, first-seen order.
    assert_eq!(d.output.enum_values, vec!["fund", "corporate", "trust"]);
}

#[test]
fn input_enum_values_are_extracted_from_when_predicates() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let allow: Allowlist = serde_yaml::from_str(ALLOWLIST).unwrap();
    let cfg = ExporterConfig::new("dmn-lite", "v1.0.0");
    let yaml = export_to_yaml(tmp.path(), &allow, &cfg).unwrap();
    let manifest = dsl_manifest::Manifest::load_from_yaml(&yaml).unwrap();

    let client_type = manifest
        .lookup_type("CbuClientType")
        .expect("CbuClientType present");
    assert_eq!(client_type.kind, "enum");
    // :when predicates against cbu-client-type literal values
    assert_eq!(
        client_type.values,
        vec!["FUND_MANDATE", "CORPORATE", "TRUST"]
    );

    let cbu_type = manifest.lookup_type("CbuType").expect("CbuType present");
    assert_eq!(cbu_type.kind, "enum");
    assert_eq!(cbu_type.values, vec!["fund", "corporate", "trust"]);
}

#[test]
fn unknown_allowlisted_decision_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let allow = Allowlist {
        public_verbs: vec![],
        public_decisions: vec!["nonexistent_decision".into()],
    };
    let cfg = ExporterConfig::new("dmn-lite", "v1.0.0");
    let err = export_to_yaml(tmp.path(), &allow, &cfg).unwrap_err();
    assert!(
        err.to_string().contains("not present in the catalogue"),
        "got: {err}"
    );
}

#[test]
fn verbs_in_allowlist_are_rejected_for_dmn_lite() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let allow = Allowlist {
        public_verbs: vec!["cbu.create".into()],
        public_decisions: vec!["cbu_type_routing".into()],
    };
    let cfg = ExporterConfig::new("dmn-lite", "v1.0.0");
    let err = export_to_yaml(tmp.path(), &allow, &cfg).unwrap_err();
    assert!(
        err.to_string().contains("dmn-lite owns decisions, not verbs"),
        "got: {err}"
    );
}

#[test]
fn empty_allowlist_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let allow = Allowlist {
        public_verbs: vec![],
        public_decisions: vec![],
    };
    let cfg = ExporterConfig::new("dmn-lite", "v1.0.0");
    let err = export_to_yaml(tmp.path(), &allow, &cfg).unwrap_err();
    assert!(
        err.to_string().contains("nothing to export"),
        "got: {err}"
    );
}
