//! Startup catalogue loader for the dmn-lite bus server.
//!
//! Walks `decisions_dir` for every `.dmn-lite` source, compiles each
//! through the dmn-lite compiler (Phase 1.4 verifier) using a Sem OS
//! catalogue TOML, and assembles a `local_decision_id → VerifiedDecision`
//! map. Decisions in `allowlist` are exposed over the bus; everything
//! else stays private (matches the v0.6 §7.5 publication contract).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use dmn_lite_compiler::{
    compile_and_verify, load_catalogue_from_path, CompileAndVerifyError,
};
use dmn_lite_types::compiled::VerifiedDecision;

/// One compiled decision plus the source text it came from (the source
/// is forwarded to the VM trace for human-readable predicate
/// descriptions).
pub(crate)struct CatalogueEntry {
    pub(crate)source_text: String,
    pub(crate)verified: VerifiedDecision,
}

/// Final assembled catalogue: every decision id → its compiled+verified
/// artifact. `len()` mirrors `allowlist`, not the raw directory listing.
pub(crate)struct DecisionCatalogue {
    by_id: HashMap<String, CatalogueEntry>,
}

impl DecisionCatalogue {
    pub(crate)fn get(&self, id: &str) -> Option<&CatalogueEntry> {
        self.by_id.get(id)
    }

    pub(crate)fn ids(&self) -> impl Iterator<Item = &str> {
        self.by_id.keys().map(String::as_str)
    }

    pub(crate)fn len(&self) -> usize {
        self.by_id.len()
    }
}

/// Load + compile + verify every `.dmn-lite` source whose decision id
/// appears in `allowlist`, using `catalogue_toml_path` for Sem OS
/// resolution. Decisions absent from the directory but listed in the
/// allowlist surface as `Err` so misconfiguration is caught at startup.
pub(crate)fn build(
    decisions_dir: &Path,
    catalogue_toml_path: &Path,
    allowlist: &[String],
) -> Result<DecisionCatalogue> {
    let catalogue = load_catalogue_from_path(catalogue_toml_path)
        .with_context(|| format!("load Sem OS catalogue from {}", catalogue_toml_path.display()))?;

    let mut by_id: HashMap<String, CatalogueEntry> = HashMap::new();
    let allow: std::collections::HashSet<&str> =
        allowlist.iter().map(String::as_str).collect();

    let read_dir = std::fs::read_dir(decisions_dir)
        .with_context(|| format!("read decisions dir {}", decisions_dir.display()))?;

    for entry in read_dir {
        let entry = entry?;
        let path: PathBuf = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("dmn-lite") {
            continue;
        }
        let source_text = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let source = dmn_lite_parser::parse(&source_text)
            .map_err(|e| anyhow!("parse {}: {e:?}", path.display()))?;

        let decision_name = source
            .decisions
            .first()
            .map(|d| d.name.name.clone())
            .ok_or_else(|| anyhow!("{}: file declares no decisions", path.display()))?;

        if !allow.contains(decision_name.as_str()) {
            tracing::debug!(
                decision = %decision_name,
                path = %path.display(),
                "skipping non-allowlisted decision"
            );
            continue;
        }

        let verified = compile_and_verify(source, &catalogue, &source_text).map_err(|e| match e {
            CompileAndVerifyError::Compile(errs) => {
                anyhow!("compile {}: {} error(s)", path.display(), errs.errors.len())
            }
            CompileAndVerifyError::Verify(v) => {
                anyhow!("verify {}: {v:?}", path.display())
            }
        })?;

        tracing::info!(
            decision = %decision_name,
            path = %path.display(),
            "loaded decision into bus catalogue"
        );

        by_id.insert(
            decision_name,
            CatalogueEntry {
                source_text,
                verified,
            },
        );
    }

    for declared in &allow {
        if !by_id.contains_key(*declared) {
            anyhow::bail!(
                "decision '{declared}' is allowlisted for bus publication but no \
                 `.dmn-lite` source was found in {}",
                decisions_dir.display()
            );
        }
    }

    Ok(DecisionCatalogue { by_id })
}
