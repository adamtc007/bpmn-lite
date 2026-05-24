//! Structural validation for a `Manifest` after YAML parsing.
//!
//! Validation rules — derived from v0.6 §7:
//!
//! - Top-level identity fields (`manifest_version`, `domain`, `catalogue_version`,
//!   `generated_at`) must be non-empty.
//! - Verb ids are unique; each verb has a non-empty `id`, `effect_class`,
//!   `authority_required`.
//! - Decision ids are unique; each decision has a non-empty `id` and a
//!   non-empty `output.type_name`.
//! - Type names are unique; each type has a non-empty `name` and a known
//!   `kind` (`"entity"`, `"enum"`, `"primitive"`).
//! - Enum types must declare at least one value.
//!
//! Type-reference resolution (e.g. the `type` field on an `InputSpec`) is
//! **not** validated here. Primitives like `"String"` and `"UUID"` are not
//! required to appear in the `types` list, and cross-domain types resolve at
//! the consumer side when imported alongside the manifest. The bus protocol
//! enforces full type matching at invocation time (v0.6 §6).

use std::collections::HashSet;

use crate::{Manifest, ManifestError};

const KNOWN_TYPE_KINDS: &[&str] = &["entity", "enum", "primitive"];

pub(crate) fn structural_validation(m: &Manifest) -> Result<(), ManifestError> {
    require_non_empty("manifest_version", &m.manifest_version)?;
    require_non_empty("domain", &m.domain)?;
    require_non_empty("catalogue_version", &m.catalogue_version)?;
    require_non_empty("generated_at", &m.generated_at)?;

    validate_verbs(m)?;
    validate_decisions(m)?;
    validate_types(m)?;

    Ok(())
}

fn validate_verbs(m: &Manifest) -> Result<(), ManifestError> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(m.verbs.len());
    for v in &m.verbs {
        require_non_empty("verb.id", &v.id)?;
        require_non_empty(&format!("verb '{}'.effect_class", v.id), &v.effect_class)?;
        require_non_empty(
            &format!("verb '{}'.authority_required", v.id),
            &v.authority_required,
        )?;
        if !seen.insert(v.id.as_str()) {
            return Err(ManifestError::Validation(format!(
                "duplicate verb id '{}' in domain '{}'",
                v.id, m.domain
            )));
        }
        for input in &v.signature.inputs {
            require_non_empty(&format!("verb '{}'.input.name", v.id), &input.name)?;
            require_non_empty(&format!("verb '{}'.input.type", v.id), &input.type_name)?;
        }
    }
    Ok(())
}

fn validate_decisions(m: &Manifest) -> Result<(), ManifestError> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(m.decisions.len());
    for d in &m.decisions {
        require_non_empty("decision.id", &d.id)?;
        require_non_empty(
            &format!("decision '{}'.output.type", d.id),
            &d.output.type_name,
        )?;
        if !seen.insert(d.id.as_str()) {
            return Err(ManifestError::Validation(format!(
                "duplicate decision id '{}' in domain '{}'",
                d.id, m.domain
            )));
        }
        for input in &d.inputs {
            require_non_empty(&format!("decision '{}'.input.name", d.id), &input.name)?;
            require_non_empty(&format!("decision '{}'.input.type", d.id), &input.type_name)?;
        }
    }
    Ok(())
}

fn validate_types(m: &Manifest) -> Result<(), ManifestError> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(m.types.len());
    for t in &m.types {
        require_non_empty("type.name", &t.name)?;
        if !KNOWN_TYPE_KINDS.contains(&t.kind.as_str()) {
            return Err(ManifestError::Validation(format!(
                "type '{}' has unknown kind '{}'; expected one of {:?}",
                t.name, t.kind, KNOWN_TYPE_KINDS
            )));
        }
        if t.kind == "enum" && t.values.is_empty() {
            return Err(ManifestError::Validation(format!(
                "enum type '{}' must declare at least one value",
                t.name
            )));
        }
        if !seen.insert(t.name.as_str()) {
            return Err(ManifestError::Validation(format!(
                "duplicate type name '{}' in domain '{}'",
                t.name, m.domain
            )));
        }
    }
    Ok(())
}

fn require_non_empty(field: &str, value: &str) -> Result<(), ManifestError> {
    if value.trim().is_empty() {
        Err(ManifestError::Validation(format!(
            "required field '{field}' is empty"
        )))
    } else {
        Ok(())
    }
}
