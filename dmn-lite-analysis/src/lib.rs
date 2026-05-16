//! dmn-lite static analysis: overlap, gap, unreachable rules, hit-policy diagnostics.
//!
//! # Pipeline
//!
//! The analyser runs five independent sub-analyses on a `VerifiedDecision`:
//!
//! 1. **Hit-policy structural check (SA-001)** — UNIQUE + catch-all is structurally
//!    broken (always `MultipleMatches`).
//! 2. **Cost bound** — total predicate count summed across rules.  Per V&S §23 #15
//!    a configurable ceiling produces a `Severity::Error` finding.
//! 3. **Pairwise overlap** — for each rule pair `(r_a, r_b)` with `a < b`, intersect
//!    their `:when` regions; emit `Overlap` if non-empty.
//! 4. **Unreachable rule** (FIRST only) — `r_b` is unreachable if some earlier `r_a`
//!    accepts a superset of `r_b`'s region.  Catch-all-followed-by-rule is already
//!    caught by the compiler (Phase 1.2); this catches the subtler shadowing cases.
//! 5. **Gap analysis** — complement of the union of all rule regions.  If a catch-all
//!    exists the gap is "covered" (Info finding); otherwise Warning.
//!
//! # Non-blocking
//!
//! Analysis produces findings; it never modifies the decision and never blocks
//! execution.  Build pipelines may fail the build on any `Severity::Error` finding
//! to enforce the bounded-computation invariant (V&S §23 #15) and authorship
//! discipline (SA-001, UnreachableRule).
//!
//! # Scope notes (Profile v0.1)
//!
//! - Integer ranges are analysed exactly via `intervals::IntervalSet`.
//! - Decimal (`f64`) ranges are deferred to Profile v0.3; decimal fields are
//!   marked opaque.
//! - String fields are treated as opaque in overlap/gap (Profile v0.1 has no
//!   semantic constraints on string values).
//! - `(and ...)` / `(or ...)` / `(not ...)` compound predicates beyond the simple
//!   `:when` conjunction reduce per-field precision (field marked opaque); the
//!   rule itself is still analysed for the non-opaque fields.
//! - `is-null` / `is-not-null` are modelled as separate `NullOnly` / `NotNull`
//!   field regions.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cost;
mod gap;
mod hit_policy;
pub mod intervals;
mod overlap;
mod region;
mod unreachable;

use dmn_lite_types::{
    AnalysisFinding, AnalysisReport, Catalogue, CostBound, FindingKind, Severity,
    compiled::VerifiedDecision,
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Configuration for the analyser.
///
/// Defaults are sensible for build-pipeline use: cost ceiling = 10,000 predicates,
/// 3 gap examples, Info findings enabled.
#[derive(Debug, Clone)]
pub struct AnalysisConfig {
    /// Maximum predicate count before emitting `CostCeilingExceeded`.
    pub cost_ceiling: usize,
    /// Maximum number of representative gap examples to compute.
    pub max_gap_examples: usize,
    /// Whether to emit `Severity::Info` findings (catch-all gap coverage,
    /// FIRST-policy overlap notes, etc.).
    pub emit_info: bool,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            cost_ceiling: 10_000,
            max_gap_examples: 3,
            emit_info: true,
        }
    }
}

/// Run static analysis with default configuration.
pub fn analyse(decision: &VerifiedDecision, catalogue: &Catalogue) -> AnalysisReport {
    analyse_with_config(decision, catalogue, &AnalysisConfig::default())
}

/// Run static analysis with a caller-provided configuration.
pub fn analyse_with_config(
    decision: &VerifiedDecision,
    catalogue: &Catalogue,
    config: &AnalysisConfig,
) -> AnalysisReport {
    let typed = &decision.as_compiled().typed_ir;
    let mut findings: Vec<AnalysisFinding> = Vec::new();

    // 1. SA-001 (hit-policy structural check)
    if let Some(f) = hit_policy::check(typed) {
        findings.push(f);
    }

    // 2. Cost bound
    let cost_bound: CostBound = cost::compute(typed);
    if cost_bound.total_predicates > config.cost_ceiling {
        findings.push(AnalysisFinding {
            severity: Severity::Error,
            kind: FindingKind::CostCeilingExceeded {
                computed: cost_bound.total_predicates,
                ceiling: config.cost_ceiling,
            },
            source_span: typed.source_span,
            description: format!(
                "evaluation cost ({} predicates) exceeds ceiling ({})",
                cost_bound.total_predicates, config.cost_ceiling
            ),
        });
    }

    // 3. Compute rule regions once (memoised across overlap/unreachable/gap).
    let regions = region::compute_all(typed, catalogue);

    // Note any string-typed inputs (analysis is reduced precision over them).
    let string_fields: Vec<String> = typed
        .input_schema
        .iter()
        .filter(|f| matches!(f.field_type, dmn_lite_types::ir::ResolvedType::Str))
        .map(|f| f.name.clone())
        .collect();
    if !string_fields.is_empty() && config.emit_info {
        findings.push(AnalysisFinding {
            severity: Severity::Info,
            kind: FindingKind::AnalysisLimitedByStringInput {
                affected_fields: string_fields.clone(),
            },
            source_span: typed.source_span,
            description: format!(
                "string-typed input fields {string_fields:?} reduce overlap/gap precision in Profile v0.1"
            ),
        });
    }

    // 4. Pairwise overlap
    findings.extend(overlap::analyse(typed, &regions, config));

    // 5. Unreachable rule (FIRST only)
    findings.extend(unreachable::analyse(typed, &regions));

    // 6. Gap analysis
    findings.extend(gap::analyse(typed, &regions, catalogue, config));

    // 7. Filter Info findings if disabled
    if !config.emit_info {
        findings.retain(|f| f.severity != Severity::Info);
    }

    // 8. Deterministic sort: severity, then primary rule_id, then kind discriminant.
    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(primary_rule_id(&a.kind).cmp(&primary_rule_id(&b.kind)))
            .then(kind_discriminant(&a.kind).cmp(&kind_discriminant(&b.kind)))
    });

    AnalysisReport {
        findings,
        cost_bound,
    }
}

// ── Sort key helpers ──────────────────────────────────────────────────────────

fn primary_rule_id(kind: &FindingKind) -> usize {
    match kind {
        FindingKind::UniqueWithCatchAll { catch_all_rule } => catch_all_rule.0,
        FindingKind::Overlap { rule_a, .. } => rule_a.0,
        FindingKind::UnreachableRule { unreachable, .. } => unreachable.0,
        FindingKind::Gap { .. } => usize::MAX - 1,
        FindingKind::CostCeilingExceeded { .. } => usize::MAX - 2,
        FindingKind::AnalysisLimitedByStringInput { .. } => usize::MAX - 3,
    }
}

fn kind_discriminant(kind: &FindingKind) -> u8 {
    match kind {
        FindingKind::UniqueWithCatchAll { .. } => 0,
        FindingKind::Overlap { .. } => 1,
        FindingKind::UnreachableRule { .. } => 2,
        FindingKind::Gap { .. } => 3,
        FindingKind::CostCeilingExceeded { .. } => 4,
        FindingKind::AnalysisLimitedByStringInput { .. } => 5,
    }
}
