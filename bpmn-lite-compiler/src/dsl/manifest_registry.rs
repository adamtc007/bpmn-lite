//! Manifest-backed `PlaceholderRegistry` for namespaced bpmn-dsl references.
//!
//! v0.6 T1 task 3 — splits `domain:verb` references at the first `:`,
//! resolves the local id against an imported [`Manifest`], and produces
//! structured [`SymbolResolution`] outcomes so the linter can emit
//! distinct compile errors for unknown-domain vs unknown-verb-in-domain.
//!
//! Placeholder semantics (which slots a verb produces / consumes) are
//! supplied by a delegate `PlaceholderRegistry`. The manifest itself only
//! gates existence; deriving `BindingDecl` from `VerbEntry.signature`
//! lives in a later tranche (v0.6 §3 calls it out as compiler inference,
//! but the demo path keeps the stub bindings for now — see T1 task 5).

use std::collections::HashMap;
use std::sync::Arc;

use dsl_manifest::Manifest;

use super::linter::{BindingDecl, PlaceholderRegistry, SymbolResolution};

/// Layered registry: existence checks via imported manifests keyed by
/// domain prefix; placeholder semantics via a delegate registry that
/// holds the (currently hardcoded) demo bindings.
pub struct ManifestPlaceholderRegistry<R: PlaceholderRegistry> {
    manifests: HashMap<String, Arc<Manifest>>,
    delegate: R,
}

impl<R: PlaceholderRegistry> ManifestPlaceholderRegistry<R> {
    /// New registry over `delegate`; no manifests imported yet.
    pub fn new(delegate: R) -> Self {
        Self {
            manifests: HashMap::new(),
            delegate,
        }
    }

    /// Import a manifest under its self-declared `domain` field. Subsequent
    /// references like `<domain>:<verb-id>` resolve here.
    pub fn import(&mut self, manifest: Manifest) {
        self.manifests
            .insert(manifest.domain.clone(), Arc::new(manifest));
    }

    /// True if `domain` has been imported.
    pub fn has_domain(&self, domain: &str) -> bool {
        self.manifests.contains_key(domain)
    }
}

impl<R: PlaceholderRegistry> PlaceholderRegistry for ManifestPlaceholderRegistry<R> {
    fn verb_bindings(&self, fqn: &str) -> Option<BindingDecl> {
        match split_namespaced(fqn) {
            Some((domain, local)) => {
                let m = self.manifests.get(domain)?;
                m.lookup_verb(local)?;
                Some(self.delegate.verb_bindings(local).unwrap_or_default())
            }
            None => self.delegate.verb_bindings(fqn),
        }
    }

    fn decision_bindings(&self, fqn: &str) -> Option<BindingDecl> {
        match split_namespaced(fqn) {
            Some((domain, local)) => {
                let m = self.manifests.get(domain)?;
                m.lookup_decision(local)?;
                Some(self.delegate.decision_bindings(local).unwrap_or_default())
            }
            None => self.delegate.decision_bindings(fqn),
        }
    }

    fn resolve_verb(&self, fqn: &str) -> SymbolResolution {
        match split_namespaced(fqn) {
            Some((domain, local)) => match self.manifests.get(domain) {
                None => SymbolResolution::UnknownDomain {
                    domain: domain.to_owned(),
                },
                Some(m) => {
                    if m.lookup_verb(local).is_some() {
                        SymbolResolution::Known
                    } else {
                        SymbolResolution::UnknownInDomain {
                            domain: domain.to_owned(),
                            known_count: m.verb_ids().count(),
                        }
                    }
                }
            },
            None => self.delegate.resolve_verb(fqn),
        }
    }

    fn resolve_decision(&self, fqn: &str) -> SymbolResolution {
        match split_namespaced(fqn) {
            Some((domain, local)) => match self.manifests.get(domain) {
                None => SymbolResolution::UnknownDomain {
                    domain: domain.to_owned(),
                },
                Some(m) => {
                    if m.lookup_decision(local).is_some() {
                        SymbolResolution::Known
                    } else {
                        SymbolResolution::UnknownInDomain {
                            domain: domain.to_owned(),
                            known_count: m.decision_ids().count(),
                        }
                    }
                }
            },
            None => self.delegate.resolve_decision(fqn),
        }
    }
}

/// Split `domain:local` at the first colon. Returns `None` if the input
/// has no colon (native bpmn-lite reference).
fn split_namespaced(fqn: &str) -> Option<(&str, &str)> {
    let (domain, local) = fqn.split_once(':')?;
    if domain.is_empty() || local.is_empty() {
        None
    } else {
        Some((domain, local))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::linter::StubPlaceholderRegistry;

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

    fn demo_registry() -> ManifestPlaceholderRegistry<StubPlaceholderRegistry> {
        let mut reg =
            ManifestPlaceholderRegistry::new(StubPlaceholderRegistry::new().with_demo_bindings());
        reg.import(Manifest::load_from_yaml(OB_POC_YAML).expect("ob-poc"));
        reg.import(Manifest::load_from_yaml(DMN_LITE_YAML).expect("dmn-lite"));
        reg
    }

    #[test]
    fn resolves_known_namespaced_verb() {
        let reg = demo_registry();
        assert_eq!(
            reg.resolve_verb("ob-poc:cbu.create"),
            SymbolResolution::Known
        );
    }

    #[test]
    fn unknown_domain_returns_unknown_domain() {
        let reg = demo_registry();
        match reg.resolve_verb("mystery:cbu.create") {
            SymbolResolution::UnknownDomain { domain } => assert_eq!(domain, "mystery"),
            other => panic!("expected UnknownDomain, got {other:?}"),
        }
    }

    #[test]
    fn unknown_verb_in_known_domain_returns_unknown_in_domain() {
        let reg = demo_registry();
        match reg.resolve_verb("ob-poc:cbu.does-not-exist") {
            SymbolResolution::UnknownInDomain {
                domain,
                known_count,
            } => {
                assert_eq!(domain, "ob-poc");
                assert_eq!(known_count, 3);
            }
            other => panic!("expected UnknownInDomain, got {other:?}"),
        }
    }

    #[test]
    fn resolves_known_namespaced_decision() {
        let reg = demo_registry();
        assert_eq!(
            reg.resolve_decision("dmn-lite:cbu_type_routing"),
            SymbolResolution::Known
        );
    }

    #[test]
    fn unknown_decision_in_known_domain() {
        let reg = demo_registry();
        match reg.resolve_decision("dmn-lite:not_a_decision") {
            SymbolResolution::UnknownInDomain {
                domain,
                known_count,
            } => {
                assert_eq!(domain, "dmn-lite");
                assert_eq!(known_count, 1);
            }
            other => panic!("expected UnknownInDomain, got {other:?}"),
        }
    }

    #[test]
    fn bare_unnamespaced_verb_delegates_to_inner_stub() {
        // Native (unprefixed) verbs still resolve via the stub registry —
        // current behaviour preserved per v0.6 T1 task 3.
        let reg = demo_registry();
        assert_eq!(reg.resolve_verb("cbu.create"), SymbolResolution::Known);
        assert!(reg.verb_bindings("cbu.create").is_some());
    }

    #[test]
    fn verb_bindings_strip_namespace_before_delegate() {
        // Namespaced verb's placeholder semantics come from the delegate stub
        // keyed by the local id ("cbu.create"), so `produces @cbu` flows through.
        let reg = demo_registry();
        let decl = reg
            .verb_bindings("ob-poc:cbu.create")
            .expect("known verb returns bindings");
        assert_eq!(decl.produces.as_deref(), Some("@cbu"));
    }

    #[test]
    fn decision_bindings_strip_namespace_before_delegate() {
        let reg = demo_registry();
        let decl = reg
            .decision_bindings("dmn-lite:cbu_type_routing")
            .expect("known decision returns bindings");
        assert_eq!(decl.produces.as_deref(), Some("@cbu-type"));
    }

    #[test]
    fn bare_colon_at_start_or_end_treated_as_unnamespaced() {
        // ':foo' and 'foo:' are degenerate; treat as native (will likely
        // be Unresolved by the delegate but should not blow up).
        assert!(split_namespaced(":foo").is_none());
        assert!(split_namespaced("foo:").is_none());
    }
}
