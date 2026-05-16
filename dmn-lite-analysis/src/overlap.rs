//! Pairwise overlap analysis.
//!
//! For every rule pair `(r_a, r_b)` with `a < b`:
//! - Intersect their `RuleRegion`s field by field.
//! - If every field intersection is non-empty, the rules overlap.
//! - Emit an `Overlap` finding with an `OverlapSummary` describing the region.
//!
//! Severity:
//! - UNIQUE policy, two specific rules → `Warning` (decision will MultipleMatches
//!   for inputs in the overlap region).
//! - FIRST policy, two specific rules → `Info` (overlap is resolved by source order).
//! - Either policy, overlap with a catch-all rule → skip (catch-all covers everything;
//!   SA-001 or the natural FIRST semantics already address this).

use dmn_lite_types::{
    AnalysisFinding, FieldOverlap, FindingKind, OverlapSummary, Severity,
    ir::{HitPolicy, TypedDecision},
};

use crate::AnalysisConfig;
use crate::region::{FieldRegion, RuleRegion, intersect};

/// Run pairwise overlap analysis.
pub fn analyse(
    decision: &TypedDecision,
    regions: &[RuleRegion],
    config: &AnalysisConfig,
) -> Vec<AnalysisFinding> {
    let mut findings = Vec::new();
    let n = decision.rules.len();
    for a in 0..n {
        if regions[a].is_catch_all {
            continue;
        }
        for b in (a + 1)..n {
            if regions[b].is_catch_all {
                continue; // Catch-all overlap is implicit; skip emission.
            }
            if let Some(summary) = intersect_regions(&regions[a], &regions[b]) {
                let severity = match decision.hit_policy {
                    HitPolicy::Unique => Severity::Warning,
                    HitPolicy::First => Severity::Info,
                };
                if severity == Severity::Info && !config.emit_info {
                    continue;
                }
                findings.push(AnalysisFinding {
                    severity,
                    kind: FindingKind::Overlap {
                        rule_a: decision.rules[a].rule_id,
                        rule_b: decision.rules[b].rule_id,
                        overlap_summary: summary,
                    },
                    source_span: decision.rules[a].source_span,
                    description: format!(
                        "rules {} and {} share a non-empty input region",
                        decision.rules[a].rule_name, decision.rules[b].rule_name
                    ),
                });
            }
        }
    }
    findings
}

/// Intersect two `RuleRegion`s field by field.  Returns the per-field summary
/// when every field intersection is non-empty.
pub fn intersect_regions(a: &RuleRegion, b: &RuleRegion) -> Option<OverlapSummary> {
    let mut per_field: Vec<FieldOverlap> = Vec::with_capacity(a.fields.len());
    for (fa, fb) in a.fields.iter().zip(b.fields.iter()) {
        let merged = intersect(fa, fb);
        if matches!(merged, FieldRegion::Empty) {
            return None;
        }
        per_field.push(field_region_to_overlap(&merged));
    }
    Some(OverlapSummary { per_field })
}

fn field_region_to_overlap(r: &FieldRegion) -> FieldOverlap {
    match r {
        FieldRegion::Any => FieldOverlap::Any,
        FieldRegion::NotNull => FieldOverlap::Opaque {
            reason: "any non-null value".into(),
        },
        FieldRegion::NullOnly => FieldOverlap::Opaque {
            reason: "null only".into(),
        },
        FieldRegion::EnumSet { values, .. } => FieldOverlap::EnumSet {
            values: values.iter().copied().collect(),
        },
        FieldRegion::BoolSet(s) => {
            if s.len() == 1 {
                FieldOverlap::Boolean(*s.iter().next().unwrap())
            } else {
                FieldOverlap::Opaque {
                    reason: "both boolean values".into(),
                }
            }
        }
        FieldRegion::IntegerInterval(set) => {
            // Report the first interval for the summary; document if multiple.
            if let Some(first) = set.intervals.first() {
                let (lo, lo_inc) = match first.lower {
                    crate::intervals::Bound::Inclusive(v) => (Some(v), true),
                    crate::intervals::Bound::Exclusive(v) => (Some(v), false),
                    crate::intervals::Bound::Unbounded => (None, false),
                };
                let (up, up_inc) = match first.upper {
                    crate::intervals::Bound::Inclusive(v) => (Some(v), true),
                    crate::intervals::Bound::Exclusive(v) => (Some(v), false),
                    crate::intervals::Bound::Unbounded => (None, false),
                };
                FieldOverlap::IntegerInterval {
                    lower: lo,
                    upper: up,
                    lower_inclusive: lo_inc,
                    upper_inclusive: up_inc,
                }
            } else {
                FieldOverlap::Opaque {
                    reason: "empty interval (should not happen)".into(),
                }
            }
        }
        FieldRegion::DecimalOpaque => FieldOverlap::Opaque {
            reason: "decimal field (v0.1 limitation)".into(),
        },
        FieldRegion::StringSet(s) => FieldOverlap::StringOpaque {
            description: format!("string values: {s:?}"),
        },
        FieldRegion::StringOpaque => FieldOverlap::StringOpaque {
            description: "string field (opaque)".into(),
        },
        FieldRegion::Opaque { reason } => FieldOverlap::Opaque {
            reason: reason.clone(),
        },
        FieldRegion::Empty => unreachable!("Empty caller guards"),
    }
}
