//! TOML catalogue loader for `dmn-lite-compiler`.
//!
//! Reads a TOML file (or string) conforming to the Sem OS stub catalogue
//! format and produces a validated [`Catalogue`]. All UUIDs are validated as
//! UUIDv7 (RFC 9562). Uniqueness of domain names, domain IDs, value symbols,
//! and value IDs is enforced.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use uuid::{Uuid, Version};

use dmn_lite_types::{
    Catalogue, CatalogueError, Domain, DomainId, DomainValue, SnapshotId, ValueId,
};

// ── TOML deserialization shapes ───────────────────────────────────────────────

#[derive(Deserialize)]
struct CatalogueToml {
    snapshot_id: String,
    snapshot_version: String,
    created_at: String,
    #[serde(default, rename = "domain")]
    domains: Vec<DomainToml>,
}

#[derive(Deserialize)]
struct DomainToml {
    name: String,
    domain_id: String,
    description: String,
    #[serde(default, rename = "value")]
    values: Vec<ValueToml>,
}

#[derive(Deserialize)]
struct ValueToml {
    symbol: String,
    value_id: String,
}

// ── Public loader API ─────────────────────────────────────────────────────────

/// Load and validate a catalogue from a TOML file on disk.
pub fn load_catalogue_from_path(path: &Path) -> Result<Catalogue, CatalogueError> {
    let source = std::fs::read_to_string(path).map_err(|e| CatalogueError::Io {
        path: path.to_string_lossy().into_owned(),
        message: e.to_string(),
    })?;
    load_catalogue_from_str(&source)
}

/// Load and validate a catalogue from a TOML string.
pub fn load_catalogue_from_str(toml_source: &str) -> Result<Catalogue, CatalogueError> {
    let raw: CatalogueToml = toml::from_str(toml_source).map_err(|e| CatalogueError::Toml {
        message: e.to_string(),
    })?;

    let snapshot_id = parse_uuid_v7(&raw.snapshot_id)
        .map(SnapshotId)
        .ok_or_else(|| CatalogueError::InvalidSnapshotId {
            value: raw.snapshot_id.clone(),
        })?;

    let mut domains_by_name: HashMap<String, ()> = HashMap::new();
    let mut domains_by_id: HashMap<String, String> = HashMap::new(); // id → first_name
    let mut domains = Vec::with_capacity(raw.domains.len());

    for d in raw.domains {
        // Duplicate domain name check
        if domains_by_name.contains_key(&d.name) {
            return Err(CatalogueError::DuplicateDomainName { name: d.name });
        }
        domains_by_name.insert(d.name.clone(), ());

        // Validate domain_id as UUIDv7
        let domain_uuid =
            parse_uuid_v7(&d.domain_id).ok_or_else(|| CatalogueError::InvalidDomainId {
                domain_name: d.name.clone(),
                value: d.domain_id.clone(),
            })?;

        // Duplicate domain_id check
        if let Some(first) = domains_by_id.get(&d.domain_id) {
            return Err(CatalogueError::DuplicateDomainId {
                value: d.domain_id.clone(),
                first_domain: first.clone(),
                second_domain: d.name.clone(),
            });
        }
        domains_by_id.insert(d.domain_id.clone(), d.name.clone());

        let domain_id = DomainId(domain_uuid);
        let mut values_by_symbol: HashMap<String, String> = HashMap::new(); // symbol → value_id str
        let mut values_by_id: HashMap<String, String> = HashMap::new(); // id → first_symbol
        let mut values = Vec::with_capacity(d.values.len());

        for v in d.values {
            // Duplicate value symbol check
            if values_by_symbol.contains_key(&v.symbol) {
                return Err(CatalogueError::DuplicateValueSymbol {
                    domain_name: d.name.clone(),
                    symbol: v.symbol.clone(),
                });
            }
            values_by_symbol.insert(v.symbol.clone(), v.value_id.clone());

            // Validate value_id as UUIDv7
            let value_uuid =
                parse_uuid_v7(&v.value_id).ok_or_else(|| CatalogueError::InvalidValueId {
                    domain_name: d.name.clone(),
                    symbol: v.symbol.clone(),
                    value: v.value_id.clone(),
                })?;

            // Duplicate value_id check
            if let Some(first) = values_by_id.get(&v.value_id) {
                return Err(CatalogueError::DuplicateValueId {
                    domain_name: d.name.clone(),
                    value: v.value_id.clone(),
                    first_symbol: first.clone(),
                    second_symbol: v.symbol.clone(),
                });
            }
            values_by_id.insert(v.value_id.clone(), v.symbol.clone());

            values.push(DomainValue {
                value_id: ValueId(value_uuid),
                symbol: v.symbol,
            });
        }

        domains.push(Domain::new(domain_id, d.name, d.description, values));
    }

    Ok(Catalogue::new(
        snapshot_id,
        raw.snapshot_version,
        raw.created_at,
        domains,
    ))
}

// ── UUID validation ───────────────────────────────────────────────────────────

/// Parse a UUID string and verify it is version 7 (SortRand / RFC 9562 UUIDv7).
fn parse_uuid_v7(s: &str) -> Option<Uuid> {
    let uuid = Uuid::parse_str(s).ok()?;
    if uuid.get_version() == Some(Version::SortRand) {
        Some(uuid)
    } else {
        None
    }
}
