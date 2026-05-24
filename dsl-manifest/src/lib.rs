//! Catalogue manifest — published API contract for federated DSL domains.
//!
//! A `Manifest` is the YAML document each domain publishes describing its
//! **public verb surface**: verb names, typed signatures, effect classes,
//! resource dependencies, FSM applicability, authority requirements. The
//! manifest is the v0.6 stored-procedure-style API contract (§5.1, §5.3, §7).
//!
//! Consumer domains import the manifest at build time. Their compilers
//! validate every cross-domain verb reference against the imported manifest.
//! Internal verbs (not in the manifest) are private to the owning domain.
//!
//! # Surface
//!
//! - `Manifest::load_from_yaml(text)` — parse + structural validation
//! - `Manifest::load_from_path(path)` — read + parse
//! - `Manifest::lookup_verb(id)` / `lookup_decision(id)` / `lookup_type(name)`
//! - `Manifest::canonical_yaml()` — round-trip serialisation for verification
//!
//! # Out of scope
//!
//! - Manifest **generation** from a domain's catalogue (T2B per-domain export binaries)
//! - Bus protocol / wire format (`dsl-bus-protocol`)
//! - Runtime delivery (`dsl-bus-client` / `dsl-bus-server`)

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

mod validate;

// ── Manifest top-level ────────────────────────────────────────────────────────

/// One catalogue manifest published by a domain.
///
/// See v0.6 §7.1 for the YAML structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub manifest_version: String,
    pub domain: String,
    pub catalogue_version: String,
    pub generated_at: String,
    #[serde(default)]
    pub generated_from_snapshot: Option<String>,

    #[serde(default)]
    pub min_consumer_manifest_version: Option<String>,
    #[serde(default)]
    pub breaking_changes_since: Vec<String>,

    /// A3 §2.4 — Capability declarations for the federated services
    /// this domain exposes. Consumers read this to know whether to call
    /// `EntityService` / `SemOsService` against the domain. Absent =
    /// pre-A3 manifest (treated as InvocationService-only).
    #[serde(default)]
    pub services: Vec<ServiceDeclaration>,

    #[serde(default)]
    pub verbs: Vec<VerbEntry>,
    #[serde(default)]
    pub decisions: Vec<DecisionEntry>,
    #[serde(default)]
    pub types: Vec<TypeEntry>,

    // Lookups built after load; not serialised.
    #[serde(skip)]
    verb_index: HashMap<String, usize>,
    #[serde(skip)]
    decision_index: HashMap<String, usize>,
    #[serde(skip)]
    type_index: HashMap<String, usize>,
}

impl Manifest {
    /// Parse a manifest from a YAML string, then run structural validation.
    pub fn load_from_yaml(text: &str) -> Result<Self, ManifestError> {
        let mut m: Manifest =
            serde_yaml::from_str(text).map_err(|e| ManifestError::Parse(e.to_string()))?;
        validate::structural_validation(&m)?;
        m.rebuild_indexes();
        Ok(m)
    }

    /// Read a manifest from disk, parse, and validate.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| ManifestError::Io(e.to_string()))?;
        Self::load_from_yaml(&text)
    }

    /// Serialise back to YAML. Used by test round-tripping and by export tooling.
    pub fn to_yaml(&self) -> Result<String, ManifestError> {
        serde_yaml::to_string(self).map_err(|e| ManifestError::Serialize(e.to_string()))
    }

    /// Look up a verb by id (unqualified — the manifest's `domain` is implicit).
    pub fn lookup_verb(&self, id: &str) -> Option<&VerbEntry> {
        self.verb_index.get(id).and_then(|i| self.verbs.get(*i))
    }

    /// Look up a decision by id (unqualified).
    pub fn lookup_decision(&self, id: &str) -> Option<&DecisionEntry> {
        self.decision_index.get(id).and_then(|i| self.decisions.get(*i))
    }

    /// Look up a type definition by name.
    pub fn lookup_type(&self, name: &str) -> Option<&TypeEntry> {
        self.type_index.get(name).and_then(|i| self.types.get(*i))
    }

    /// Iterate verb ids exposed by this manifest.
    pub fn verb_ids(&self) -> impl Iterator<Item = &str> {
        self.verbs.iter().map(|v| v.id.as_str())
    }

    /// Iterate decision ids exposed by this manifest.
    pub fn decision_ids(&self) -> impl Iterator<Item = &str> {
        self.decisions.iter().map(|d| d.id.as_str())
    }

    /// Iterate the federated services declared by this manifest. Empty
    /// for pre-A3 manifests (which were InvocationService-only by
    /// implicit convention).
    pub fn declared_services(&self) -> &[ServiceDeclaration] {
        &self.services
    }

    /// True if the domain declares the named service. A3 §6 discipline
    /// point #3: consumers consult this before calling — they don't
    /// probe blindly.
    pub fn declares_service(&self, kind: ServiceKind) -> bool {
        self.services.iter().any(|s| s.available && s.kind == kind)
    }

    fn rebuild_indexes(&mut self) {
        self.verb_index = self
            .verbs
            .iter()
            .enumerate()
            .map(|(i, v)| (v.id.clone(), i))
            .collect();
        self.decision_index = self
            .decisions
            .iter()
            .enumerate()
            .map(|(i, d)| (d.id.clone(), i))
            .collect();
        self.type_index = self
            .types
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.clone(), i))
            .collect();
    }
}

// ── Service declarations (A3 §2.4) ──────────────────────────────────

/// One federated service a domain declares it implements. Per A3 §6
/// discipline #3, this declaration is authoritative: a domain that
/// declares a service must register it (even as stub returning
/// NOT_IMPLEMENTED), and a domain that doesn't declare it must not
/// register the gRPC route (so callers see UNIMPLEMENTED, not a
/// confused stub).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceDeclaration {
    pub kind: ServiceKind,
    pub available: bool,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// The three federated service kinds defined in v0.6-A3 §1. Capability
/// matrix: every domain *may* expose InvocationService; only domains
/// that hold entity state expose EntityService; only domains that
/// publish semantic grounding expose SemOsService.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ServiceKind {
    InvocationService,
    EntityService,
    SemOsService,
}

// ── Verb entry ────────────────────────────────────────────────────────────────

/// One verb in a published catalogue.
///
/// See v0.6 §7.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerbEntry {
    pub id: String,
    pub signature: Signature,
    pub effect_class: String,
    #[serde(default)]
    pub coordination_policy: Option<String>,
    #[serde(default)]
    pub transaction_policy: Option<String>,
    #[serde(default)]
    pub resource_dependencies: Vec<ResourceDependency>,
    #[serde(default)]
    pub fsm_applicability: Option<FsmApplicability>,
    pub authority_required: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Verb signature: typed inputs and an optional produced output type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
    #[serde(default)]
    pub output: Option<OutputSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_required() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputSpec {
    /// Type of binding produced (e.g. `"CBU"`). `None` means the verb produces
    /// no new placeholder.
    pub produces: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDependency {
    pub kind: String,         // "NaturalKey" | "EntityUuid" | ...
    pub from_input: String,   // which input contributes the key
    #[serde(default)]
    pub entity_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsmApplicability {
    pub entity: String,
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub postconditions: Vec<String>,
}

// ── Decision entry ────────────────────────────────────────────────────────────

/// One DMN decision in a published catalogue.
///
/// See v0.6 §7.3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionEntry {
    pub id: String,
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
    pub output: DecisionOutput,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionOutput {
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default)]
    pub enum_values: Vec<String>,
}

// ── Type entry ────────────────────────────────────────────────────────────────

/// One named type referenced by verbs or decisions in this manifest.
///
/// See v0.6 §7.4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeEntry {
    pub name: String,
    pub kind: String, // "entity" | "enum" | "primitive"
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub uuid_type: Option<String>,
    #[serde(default)]
    pub values: Vec<String>, // for enum kind
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("manifest YAML parse error: {0}")]
    Parse(String),
    #[error("manifest YAML serialise error: {0}")]
    Serialize(String),
    #[error("manifest file I/O error: {0}")]
    Io(String),
    #[error("manifest validation: {0}")]
    Validation(String),
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
