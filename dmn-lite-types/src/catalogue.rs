//! Catalogue types: resolved Sem OS domain and value lookup.
//!
//! The `Catalogue` is the runtime view of a Sem OS snapshot after deserialization
//! and validation. Consumers call `resolve_domain` / `resolve_value` to map
//! symbolic names to their UUIDv7 identities.
//!
//! TOML loading lives in `dmn-lite-compiler::catalogue_loader` to avoid adding
//! a TOML parsing dependency to this foundational types crate.

use std::collections::HashMap;

use crate::ids::{DomainId, SnapshotId, ValueId};

/// Fully-loaded and validated Sem OS catalogue snapshot.
///
/// Constructed by `dmn_lite_compiler::catalogue_loader::load_catalogue_from_str`
/// or `load_catalogue_from_path`. All domain names and value symbols are
/// unique within the catalogue; all IDs are valid UUIDv7.
#[derive(Debug)]
pub struct Catalogue {
    /// Unique identifier for this snapshot.
    pub snapshot_id: SnapshotId,
    /// Human-readable version label (e.g., `"v0.1.0-stub"`).
    pub snapshot_version: String,
    /// ISO-8601 creation timestamp (informational; not parsed).
    pub created_at: String,
    domains_by_name: HashMap<String, DomainId>,
    domains: HashMap<DomainId, Domain>,
}

impl Catalogue {
    /// Construct a new catalogue from pre-validated components.
    ///
    /// Callers are `catalogue_loader` only; external code uses the loader
    /// functions instead of constructing directly.
    pub fn new(
        snapshot_id: SnapshotId,
        snapshot_version: String,
        created_at: String,
        domains: Vec<Domain>,
    ) -> Self {
        let domains_by_name: HashMap<String, DomainId> = domains
            .iter()
            .map(|d| (d.name.clone(), d.domain_id))
            .collect();
        let domains: HashMap<DomainId, Domain> =
            domains.into_iter().map(|d| (d.domain_id, d)).collect();
        Self {
            snapshot_id,
            snapshot_version,
            created_at,
            domains_by_name,
            domains,
        }
    }

    /// Look up a domain by its symbolic name (case-sensitive).
    pub fn resolve_domain(&self, name: &str) -> Option<&Domain> {
        let id = self.domains_by_name.get(name)?;
        self.domains.get(id)
    }

    /// The snapshot's unique identifier.
    pub fn snapshot_id(&self) -> SnapshotId {
        self.snapshot_id
    }

    /// Iterator over all domains in the catalogue (order is unspecified).
    pub fn domains(&self) -> impl Iterator<Item = &Domain> {
        self.domains.values()
    }
}

/// A single resolved domain from the Sem OS catalogue.
#[derive(Debug)]
pub struct Domain {
    /// The domain's unique UUIDv7 identifier.
    pub domain_id: DomainId,
    /// The domain's symbolic name (e.g., `"Jurisdiction"`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    values_by_symbol: HashMap<String, ValueId>,
    values: HashMap<ValueId, DomainValue>,
}

impl Domain {
    /// Construct a new domain from pre-validated components.
    pub fn new(
        domain_id: DomainId,
        name: String,
        description: String,
        values: Vec<DomainValue>,
    ) -> Self {
        let values_by_symbol: HashMap<String, ValueId> = values
            .iter()
            .map(|v| (v.symbol.clone(), v.value_id))
            .collect();
        let values: HashMap<ValueId, DomainValue> =
            values.into_iter().map(|v| (v.value_id, v)).collect();
        Self {
            domain_id,
            name,
            description,
            values_by_symbol,
            values,
        }
    }

    /// Resolve a value symbol to its `ValueId`. Returns `None` if the symbol
    /// is not a member of this domain.
    pub fn resolve_value(&self, symbol: &str) -> Option<ValueId> {
        self.values_by_symbol.get(symbol).copied()
    }

    /// True when `symbol` is a declared member of this domain.
    pub fn has_value(&self, symbol: &str) -> bool {
        self.values_by_symbol.contains_key(symbol)
    }

    /// Number of values declared in this domain.
    pub fn value_count(&self) -> usize {
        self.values.len()
    }

    /// Iterator over all values in the domain (order is unspecified).
    pub fn values(&self) -> impl Iterator<Item = &DomainValue> {
        self.values.values()
    }
}

/// A single resolved value within a domain.
#[derive(Debug)]
pub struct DomainValue {
    /// The value's unique UUIDv7 identifier.
    pub value_id: ValueId,
    /// The value's symbolic name (e.g., `"LU"`, `"SICAV"`).
    pub symbol: String,
}
