//! dmn-lite catalogue manifest exporter.
//!
//! Reads `.dmn-lite` decision source files from a directory, parses
//! each via [`dmn_lite_parser`], filters to the publication allowlist,
//! and emits a v0.6 §7 manifest that federated DSL peers (bpmn-lite,
//! ob-poc) consume at their compile time.
//!
//! The exporter is intentionally narrow — it leans on the existing
//! parser AST rather than re-implementing s-expression parsing, but
//! it does *not* call the compiler. Profile v0.1 of dmn-lite allows
//! one decision per source file (the parser enforces this), so each
//! `.dmn-lite` file is shorthand for "one DecisionEntry".

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use dmn_lite_types::ast::{DecisionAst, LiteralAst, PredicateAst, Source, TypeRefAst, WhenAst};
use dsl_manifest::{DecisionEntry, DecisionOutput, InputSpec, Manifest, TypeEntry, VerbEntry};
use serde::Deserialize;

// ── Allowlist ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct Allowlist {
    #[serde(default)]
    pub public_verbs: Vec<String>,
    #[serde(default)]
    pub public_decisions: Vec<String>,
}

impl Allowlist {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("read allowlist at {}", path.as_ref().display()))?;
        let allow: Allowlist = serde_yaml::from_str(&text).context("parse allowlist")?;
        Ok(allow)
    }
}

// ── Exporter config ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ExporterConfig {
    pub domain: String,
    pub catalogue_version: String,
    pub generated_at: String,
    pub manifest_version: String,
    pub min_consumer_manifest_version: Option<String>,
}

impl ExporterConfig {
    pub fn new(domain: impl Into<String>, catalogue_version: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            catalogue_version: catalogue_version.into(),
            generated_at: iso8601_now(),
            manifest_version: "1.0".into(),
            min_consumer_manifest_version: Some("1.0".into()),
        }
    }
}

fn iso8601_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, min, sec) = naive_utc_from_unix(secs as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn naive_utc_from_unix(mut secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let sec = (secs.rem_euclid(60)) as u32;
    secs = secs.div_euclid(60);
    let min = (secs.rem_euclid(60)) as u32;
    secs = secs.div_euclid(60);
    let hour = (secs.rem_euclid(24)) as u32;
    let mut days = secs.div_euclid(24);

    days += 719_468;
    let era = if days >= 0 { days / 146_097 } else { (days - 146_096) / 146_097 };
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d, hour, min, sec)
}

// ── Parsing + conversion ─────────────────────────────────────────────

/// Read every `*.dmn-lite` file under `dir`, parse each into a single
/// decision (Profile v0.1 enforces one decision per file), and return
/// the decisions keyed by decision name.
pub(crate) fn load_source_catalogue(dir: &Path) -> Result<BTreeMap<String, DecisionAst>> {
    let mut out = BTreeMap::new();
    let read_dir = std::fs::read_dir(dir)
        .with_context(|| format!("read decisions dir {}", dir.display()))?;
    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("dmn-lite") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let source: Source = dmn_lite_parser::parse(&text)
            .map_err(|e| anyhow!("parse {}: {e:?}", path.display()))?;
        for decision in source.decisions {
            let name = decision.name.name.clone();
            if out.contains_key(&name) {
                bail!("duplicate decision name '{name}' across `.dmn-lite` files");
            }
            out.insert(name, decision);
        }
    }
    Ok(out)
}

fn convert_decision(decision: &DecisionAst) -> Result<DecisionEntry> {
    let inputs: Vec<InputSpec> = decision
        .inputs
        .iter()
        .map(|i| InputSpec {
            name: i.name.name.clone(),
            type_name: i.domain_ref.name.clone(),
            required: true,
        })
        .collect();

    let primary_output = decision.outputs.first().ok_or_else(|| {
        anyhow!(
            "decision '{}' has no outputs — v0.6 §7.3 requires at least one",
            decision.name.name
        )
    })?;

    let enum_values = collect_output_values(decision, &primary_output.name.name);

    let output = DecisionOutput {
        type_name: primary_output.domain_ref.name.clone(),
        enum_values,
    };

    Ok(DecisionEntry {
        id: decision.name.name.clone(),
        inputs,
        output,
        description: None,
    })
}

/// Walk every `:then ((<output> = literal))` assignment in the
/// decision and return the distinct symbolic values targeting
/// `output_name`, in first-seen order. Non-symbol literals are
/// ignored — the manifest enum_values field is a list of enum tokens.
fn collect_output_values(decision: &DecisionAst, output_name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for rule in &decision.rules {
        for assign in &rule.then {
            if assign.output.name != output_name {
                continue;
            }
            if let LiteralAst::Symbol(sym) = &assign.value
                && !out.contains(&sym.name)
            {
                out.push(sym.name.clone());
            }
        }
    }
    out
}

/// Walk every predicate referencing the input field `name` and return
/// the distinct symbolic literals it tests against. Matches `Eq`,
/// `NotEq`, and `InSet` shapes.
fn collect_input_values(decision: &DecisionAst, name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for rule in &decision.rules {
        if let WhenAst::Predicates(preds, _) = &rule.when {
            for p in preds {
                walk_predicate(p, name, &mut out);
            }
        }
    }
    out
}

fn walk_predicate(p: &PredicateAst, target_field: &str, out: &mut Vec<String>) {
    match p {
        PredicateAst::Eq { field, value, .. } | PredicateAst::NotEq { field, value, .. } => {
            if field.name == target_field
                && let LiteralAst::Symbol(sym) = value
                && !out.contains(&sym.name)
            {
                out.push(sym.name.clone());
            }
        }
        PredicateAst::InSet { field, values, .. } if field.name == target_field => {
            for v in values {
                if let LiteralAst::Symbol(sym) = v
                    && !out.contains(&sym.name)
                {
                    out.push(sym.name.clone());
                }
            }
        }
        PredicateAst::Not { inner, .. } => walk_predicate(inner, target_field, out),
        PredicateAst::And { items, .. } | PredicateAst::Or { items, .. } => {
            for it in items {
                walk_predicate(it, target_field, out);
            }
        }
        _ => {}
    }
}

/// Walk every input declaration in the decision and produce
/// `(domain_name, values, kind)` triples for type emission.
fn type_entries_for_decision(decision: &DecisionAst) -> Vec<TypeEntry> {
    let mut acc: BTreeMap<String, TypeEntry> = BTreeMap::new();
    for input in &decision.inputs {
        register_type(&mut acc, &input.domain_ref.name, &input.type_ref);
        if matches!(input.type_ref, TypeRefAst::Enum(_)) {
            let values = collect_input_values(decision, &input.name.name);
            extend_values(&mut acc, &input.domain_ref.name, values);
        }
    }
    for out in &decision.outputs {
        register_type(&mut acc, &out.domain_ref.name, &out.type_ref);
        if matches!(out.type_ref, TypeRefAst::Enum(_)) {
            let values = collect_output_values(decision, &out.name.name);
            extend_values(&mut acc, &out.domain_ref.name, values);
        }
    }
    acc.into_values().collect()
}

fn register_type(acc: &mut BTreeMap<String, TypeEntry>, domain: &str, type_ref: &TypeRefAst) {
    let kind = match type_ref {
        TypeRefAst::Enum(_) => "enum",
        TypeRefAst::Bool(_) | TypeRefAst::Integer(_) | TypeRefAst::Decimal(_) | TypeRefAst::String(_) => {
            "primitive"
        }
    };
    acc.entry(domain.to_owned()).or_insert_with(|| TypeEntry {
        name: domain.to_owned(),
        kind: kind.to_owned(),
        description: None,
        uuid_type: None,
        values: Vec::new(),
    });
}

fn extend_values(acc: &mut BTreeMap<String, TypeEntry>, domain: &str, mut values: Vec<String>) {
    if let Some(entry) = acc.get_mut(domain) {
        for v in values.drain(..) {
            if !entry.values.contains(&v) {
                entry.values.push(v);
            }
        }
    }
}

/// Convert a pre-parsed map of decisions through the allowlist.
pub(crate) fn export_from_sources(
    config: &ExporterConfig,
    sources: BTreeMap<String, DecisionAst>,
    allowlist: &Allowlist,
) -> Result<Manifest> {
    if !allowlist.public_verbs.is_empty() {
        bail!(
            "allowlist declares public_verbs ({}); dmn-lite owns decisions, not verbs",
            allowlist.public_verbs.join(", ")
        );
    }
    if allowlist.public_decisions.is_empty() {
        bail!("allowlist declares no public_decisions — nothing to export");
    }

    let mut decisions: Vec<DecisionEntry> = Vec::new();
    let mut types_by_name: BTreeMap<String, TypeEntry> = BTreeMap::new();
    let mut missing: Vec<String> = Vec::new();

    for id in &allowlist.public_decisions {
        match sources.get(id) {
            Some(d) => {
                let entry = convert_decision(d)?;
                for new_type in type_entries_for_decision(d) {
                    types_by_name
                        .entry(new_type.name.clone())
                        .and_modify(|existing| {
                            for v in &new_type.values {
                                if !existing.values.contains(v) {
                                    existing.values.push(v.clone());
                                }
                            }
                        })
                        .or_insert(new_type);
                }
                decisions.push(entry);
            }
            None => missing.push(id.clone()),
        }
    }
    if !missing.is_empty() {
        bail!(
            "allowlist references decisions not present in the catalogue: {}",
            missing.join(", ")
        );
    }

    let types: Vec<TypeEntry> = types_by_name.into_values().collect();

    let verbs: Vec<VerbEntry> = Vec::new();
    let yaml = build_yaml(config, &verbs, &decisions, &types);
    Manifest::load_from_yaml(&yaml).map_err(|e| anyhow!("emitted manifest failed validation: {e}"))
}

fn build_yaml(
    config: &ExporterConfig,
    _verbs: &[VerbEntry],
    decisions: &[DecisionEntry],
    types: &[TypeEntry],
) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "manifest_version: {:?}", config.manifest_version);
    let _ = writeln!(s, "domain: {:?}", config.domain);
    let _ = writeln!(s, "catalogue_version: {:?}", config.catalogue_version);
    let _ = writeln!(s, "generated_at: {:?}", config.generated_at);
    if let Some(min) = &config.min_consumer_manifest_version {
        let _ = writeln!(s, "min_consumer_manifest_version: {min:?}");
    }
    let _ = writeln!(s, "breaking_changes_since: []");
    s.push('\n');
    // A3 §2.4 — dmn-lite declares only InvocationService. No
    // EntityService (decisions are stateless; no entity state). No
    // SemOsService (decisions are self-contained).
    s.push_str("services:\n");
    s.push_str("  - kind: InvocationService\n");
    s.push_str("    available: true\n");
    s.push_str("    capabilities: [\"Submit\", \"Validate\"]\n");
    s.push('\n');
    s.push_str("verbs: []\n");
    s.push('\n');
    s.push_str("decisions:\n");
    for d in decisions {
        emit_decision(&mut s, d);
    }
    s.push('\n');
    s.push_str("types:\n");
    for t in types {
        emit_type(&mut s, t);
    }
    s
}

fn emit_decision(s: &mut String, d: &DecisionEntry) {
    use std::fmt::Write as _;
    let _ = writeln!(s, "  - id: {:?}", d.id);
    if d.inputs.is_empty() {
        s.push_str("    inputs: []\n");
    } else {
        s.push_str("    inputs:\n");
        for i in &d.inputs {
            let _ = writeln!(s, "      - name: {:?}", i.name);
            let _ = writeln!(s, "        type: {:?}", i.type_name);
            let _ = writeln!(s, "        required: {}", i.required);
        }
    }
    s.push_str("    output:\n");
    let _ = writeln!(s, "      type: {:?}", d.output.type_name);
    if !d.output.enum_values.is_empty() {
        s.push_str("      enum_values:\n");
        for v in &d.output.enum_values {
            let _ = writeln!(s, "        - {v:?}");
        }
    }
}

fn emit_type(s: &mut String, t: &TypeEntry) {
    use std::fmt::Write as _;
    let _ = writeln!(s, "  - name: {:?}", t.name);
    let _ = writeln!(s, "    kind: {:?}", t.kind);
    if let Some(uuid_type) = &t.uuid_type {
        let _ = writeln!(s, "    uuid_type: {uuid_type:?}");
    }
    if !t.values.is_empty() {
        s.push_str("    values:\n");
        for v in &t.values {
            let _ = writeln!(s, "      - {v:?}");
        }
    }
}

/// End-to-end: parse `dir`, intersect with `allowlist`, write YAML.
pub fn export_to_yaml(
    dir: &Path,
    allowlist: &Allowlist,
    config: &ExporterConfig,
) -> Result<String> {
    let sources = load_source_catalogue(dir)?;
    let manifest = export_from_sources(config, sources, allowlist)?;
    Ok(build_yaml(
        config,
        &[],
        &manifest.decisions,
        &manifest.types,
    ))
}

/// Convenience: read everything from disk + write the manifest to disk.
pub fn export_to_path(
    decisions_dir: &Path,
    allowlist_path: &Path,
    output_path: &Path,
    domain: &str,
    catalogue_version: &str,
) -> Result<PathBuf> {
    let allow = Allowlist::from_path(allowlist_path)?;
    let cfg = ExporterConfig::new(domain, catalogue_version);
    let yaml = export_to_yaml(decisions_dir, &allow, &cfg)?;
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(output_path, &yaml)
        .with_context(|| format!("write {}", output_path.display()))?;
    Ok(output_path.to_path_buf())
}

#[cfg(test)]
mod tests;
