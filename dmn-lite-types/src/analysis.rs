//! Static analysis types — Phase 1.6.
//!
//! `AnalysisReport` is the output of `dmn-lite-analysis::analyse()`. It contains
//! zero or more `AnalysisFinding`s plus a computed evaluation cost bound. Analysis
//! is non-blocking: a decision with `Error`-severity findings still compiles,
//! verifies, and evaluates. Authors / build pipelines decide whether to reject.

use crate::ids::{RuleId, SourceSpan, ValueId};
use crate::ir::TypedValue;

// ── Top-level report ─────────────────────────────────────────────────────────

/// Result of running static analysis on a verified decision.
///
/// `findings` is sorted deterministically (Severity → rule_id → kind discriminant)
/// so two runs over the same input produce byte-identical reports.
///
/// `Eq` is not derived because `Gap` findings hold `TypedValue::Decimal(f64)` and
/// `f64` is not `Eq`; `PartialEq` is sufficient for testing and comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisReport {
    /// All findings produced by the analysis pipeline.
    pub findings: Vec<AnalysisFinding>,
    /// Computed worst-case evaluation cost (per `docs/dmn-lite-semantics.md` §4.7).
    /// The analyser computes this for every decision; if it exceeds a configured
    /// ceiling, a `CostCeilingExceeded` finding is included in `findings`.
    pub cost_bound: CostBound,
}

/// A single analysis finding.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisFinding {
    /// Severity classification (Error / Warning / Info).
    pub severity: Severity,
    /// What was found.
    pub kind: FindingKind,
    /// Source span the finding refers to (usually the offending rule).
    pub source_span: SourceSpan,
    /// Human-readable summary suitable for terminal output.
    pub description: String,
}

/// Severity classification for `AnalysisFinding`.
///
/// Severity drives build-pipeline behaviour: a deployment may fail the build on
/// any `Error`, surface `Warning` in a diagnostic, and silence `Info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The decision will execute but the authorship is structurally broken
    /// (e.g., UNIQUE+catch-all guaranteed to MultipleMatches; unreachable rule).
    Error,
    /// A likely authorship issue worth surfacing (e.g., overlap under UNIQUE).
    /// The author may have intended the overlap; the analyser cannot tell.
    Warning,
    /// Informational; not necessarily problematic (e.g., gap covered by catch-all).
    Info,
}

// ── FindingKind discriminator ────────────────────────────────────────────────

/// Classification of an `AnalysisFinding`.
#[derive(Debug, Clone, PartialEq)]
pub enum FindingKind {
    /// SA-001: UNIQUE hit policy with a catch-all rule.
    ///
    /// UNIQUE + catch-all is structurally broken — any specific rule that also
    /// matches causes a `MultipleMatches` error at evaluation. Recommended fix:
    /// switch to FIRST or remove the catch-all.
    UniqueWithCatchAll {
        /// The catch-all rule's identifier.
        catch_all_rule: RuleId,
    },

    /// Two rules' `:when` clauses intersect non-trivially.
    ///
    /// Under UNIQUE, the overlap region produces `MultipleMatches` at evaluation.
    /// Under FIRST, the overlap is resolved by source order (may be intentional).
    Overlap {
        /// First rule (lower `rule_id`).
        rule_a: RuleId,
        /// Second rule (higher `rule_id`).
        rule_b: RuleId,
        /// Per-field description of the overlap region.
        overlap_summary: OverlapSummary,
    },

    /// One rule is unreachable because an earlier rule under FIRST always matches
    /// a superset of its input region.
    ///
    /// The Phase 1.2 compiler already detects catch-all-followed-by-rule
    /// (`CompileError::UnreachableAfterCatchAll`); this finding catches the subtler
    /// shadowing cases where the earlier rule's predicates accept a strict superset.
    UnreachableRule {
        /// The rule that can never fire.
        unreachable: RuleId,
        /// The earlier rule whose region covers `unreachable`.
        shadowing: RuleId,
    },

    /// An input combination falls outside every rule's `:when` clause.
    ///
    /// If a catch-all is present, this finding is `Info` (the catch-all covers
    /// the gap). Otherwise it is `Warning` (the decision returns `NoMatch`).
    Gap {
        /// Per-finding gap description (representative examples + count).
        gap_summary: GapSummary,
        /// True when a catch-all rule exists in this decision.
        catch_all_present: bool,
    },

    /// Static evaluation cost bound exceeds the configured ceiling.
    ///
    /// V&S §23 #15: deployments may fail the build on this finding to enforce
    /// the bounded-computation invariant.
    CostCeilingExceeded {
        /// The total predicate count this decision would evaluate worst-case.
        computed: usize,
        /// The configured ceiling.
        ceiling: usize,
    },

    /// String-typed inputs reduce overlap/gap precision; findings referring to
    /// these fields may be incomplete or absent.
    AnalysisLimitedByStringInput {
        /// Names of the string-typed input fields that limited analysis.
        affected_fields: Vec<String>,
    },
}

// ── Overlap / Gap descriptors ────────────────────────────────────────────────

/// Per-field description of an `Overlap` finding's intersection region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlapSummary {
    /// One entry per input field in source order. Each entry describes how the
    /// two rules' constraints intersect on that field.
    pub per_field: Vec<FieldOverlap>,
}

/// How two rules' predicates intersect on a single input field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldOverlap {
    /// Neither rule constrains this field; the overlap region is unrestricted.
    Any,
    /// Both rules accept exactly this enum value subset.
    EnumSet {
        /// The intersected value IDs (sorted).
        values: Vec<ValueId>,
    },
    /// Both rules accept this integer interval.
    IntegerInterval {
        /// Lower bound, `None` = `-∞`.
        lower: Option<i64>,
        /// Upper bound, `None` = `+∞`.
        upper: Option<i64>,
        /// True when the lower bound is inclusive.
        lower_inclusive: bool,
        /// True when the upper bound is inclusive.
        upper_inclusive: bool,
    },
    /// Both rules accept this boolean value.
    Boolean(bool),
    /// String overlap (opaque in v0.1).
    StringOpaque {
        /// Qualitative description.
        description: String,
    },
    /// One or both rules use `is-null` / `is-not-null` / compound predicates and
    /// the analyser couldn't compute an exact intersection for this field.
    Opaque {
        /// Reason the field is opaque.
        reason: String,
    },
}

/// Description of an uncovered region in a `Gap` finding.
#[derive(Debug, Clone, PartialEq)]
pub struct GapSummary {
    /// Up to `AnalysisConfig::max_gap_examples` representative uncovered inputs.
    /// Witnesses are chosen so that examples differ in as many fields as possible.
    pub examples: Vec<UncoveredInputExample>,
    /// Exact count of uncovered combinations when the gap region is finite
    /// (all fields are enum/boolean), else `None`.
    pub approximate_count: Option<usize>,
}

/// One concrete uncovered input combination.
#[derive(Debug, Clone, PartialEq)]
pub struct UncoveredInputExample {
    /// One value per input field in source order.
    pub field_values: Vec<TypedValue>,
}

// ── CostBound ────────────────────────────────────────────────────────────────

/// Worst-case evaluation cost for a decision.
///
/// In Profile v0.1 the cost is the total predicate count across all rules — every
/// predicate is O(1) (comparison, set lookup, range check, null test, boolean
/// combinator over bounded children). Future profiles may introduce quantifiers
/// or aggregation; `exact` records whether the bound was computed exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostBound {
    /// Total predicate evaluations worst-case (all rules evaluated, no
    /// short-circuit, no quantifiers in v0.1).
    pub total_predicates: usize,
    /// `true` when the bound is exact (always true in v0.1).
    pub exact: bool,
}
