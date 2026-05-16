//! Gap analysis: find input combinations covered by no rule.
//!
//! A gap exists when the union of all rules' input regions is a strict subset of
//! the full input space.  The analyser:
//! - Walks the per-field input domains (enum / boolean / integer; opaque for string/decimal).
//! - Computes the union of per-field allowed values across all rules.
//! - Complements that union within the field's domain to find the field-level gap.
//! - Picks witnesses from the gap.
//!
//! Limitation: in Profile v0.1 the analyser uses a **per-field independent** model
//! of gap, not the full Cartesian gap.  This is sound (every reported gap is a real
//! gap) but not complete — there exist gaps where every field is individually
//! covered but no rule covers a specific combination.  Computing the precise
//! Cartesian gap is exponential in field count; v0.1 trades precision for tractability.
//! Catch-all coverage is reported as `Info` regardless.

use std::collections::BTreeSet;

use dmn_lite_types::{
    AnalysisFinding, Catalogue, FindingKind, GapSummary, Severity, UncoveredInputExample, ValueId,
    ir::{ResolvedType, TypedDecision, TypedValue},
};

use crate::AnalysisConfig;
use crate::region::{FieldRegion, RuleRegion, catalogue_values};

/// Run the gap analysis pipeline.
pub fn analyse(
    decision: &TypedDecision,
    regions: &[RuleRegion],
    catalogue: &Catalogue,
    config: &AnalysisConfig,
) -> Vec<AnalysisFinding> {
    let catch_all_present = regions.iter().any(|r| r.is_catch_all);

    // Compute per-field union across all non-catch-all rules.
    let per_field_union = compute_per_field_union(decision, regions, catalogue);

    // Find per-field gaps.  A field has a gap if its union ≠ full domain.
    let mut field_gap_witnesses: Vec<Option<TypedValue>> =
        Vec::with_capacity(decision.input_schema.len());
    let mut field_has_gap: Vec<bool> = Vec::with_capacity(decision.input_schema.len());

    for (idx, schema) in decision.input_schema.iter().enumerate() {
        let (has_gap, witness) = field_gap(&per_field_union[idx], &schema.field_type, catalogue);
        field_gap_witnesses.push(witness);
        field_has_gap.push(has_gap);
    }

    let any_gap = field_has_gap.iter().any(|g| *g);

    if !any_gap {
        // Decision fully covers the input space at the per-field level.
        return Vec::new();
    }

    // Build representative witnesses by combining per-field gap witnesses with
    // sample valid values for non-gap fields.
    let witnesses = collect_witnesses(
        decision,
        &per_field_union,
        &field_has_gap,
        &field_gap_witnesses,
        catalogue,
        config.max_gap_examples,
    );

    let approximate_count = finite_gap_count(decision, &per_field_union, &field_has_gap, catalogue);

    let severity = if catch_all_present {
        Severity::Info
    } else {
        Severity::Warning
    };

    if severity == Severity::Info && !config.emit_info {
        return Vec::new();
    }

    let description = if catch_all_present {
        format!(
            "gap region exists in specific rules; the catch-all rule covers it ({} representative inputs)",
            witnesses.len()
        )
    } else {
        format!(
            "gap region exists; no rule (and no catch-all) matches {} representative inputs",
            witnesses.len()
        )
    };

    vec![AnalysisFinding {
        severity,
        kind: FindingKind::Gap {
            gap_summary: GapSummary {
                examples: witnesses,
                approximate_count,
            },
            catch_all_present,
        },
        source_span: decision.source_span,
        description,
    }]
}

// ── Per-field union across rules ─────────────────────────────────────────────

fn compute_per_field_union(
    decision: &TypedDecision,
    regions: &[RuleRegion],
    catalogue: &Catalogue,
) -> Vec<FieldRegion> {
    let n_fields = decision.input_schema.len();
    let mut unions: Vec<FieldRegion> = vec![FieldRegion::Empty; n_fields];

    // If any rule is a catch-all, the per-field union is "Any" for every field.
    if regions.iter().any(|r| r.is_catch_all) {
        return vec![FieldRegion::Any; n_fields];
    }

    for region in regions {
        for (idx, fr) in region.fields.iter().enumerate() {
            unions[idx] = union_two(&unions[idx], fr, catalogue);
        }
    }
    unions
}

fn union_two(a: &FieldRegion, b: &FieldRegion, catalogue: &Catalogue) -> FieldRegion {
    use FieldRegion::*;
    match (a, b) {
        (Empty, x) | (x, Empty) => x.clone(),
        (Any, _) | (_, Any) => Any,
        (NotNull, NotNull) => NotNull,
        (NullOnly, NullOnly) => NullOnly,
        (NotNull, NullOnly) | (NullOnly, NotNull) => Any,
        (
            EnumSet {
                domain_id: d1,
                values: v1,
            },
            EnumSet {
                domain_id: d2,
                values: v2,
            },
        ) if d1 == d2 => {
            let merged: BTreeSet<ValueId> = v1.union(v2).copied().collect();
            // If the union covers the full domain, collapse to Any.
            let all: BTreeSet<ValueId> = catalogue_values(*d1, catalogue).into_iter().collect();
            if merged == all {
                NotNull
            } else {
                EnumSet {
                    domain_id: *d1,
                    values: merged,
                }
            }
        }
        (BoolSet(a), BoolSet(b)) => {
            let merged: BTreeSet<bool> = a.union(b).copied().collect();
            if merged.len() == 2 {
                NotNull
            } else {
                BoolSet(merged)
            }
        }
        (IntegerInterval(a), IntegerInterval(b)) => IntegerInterval(a.union(b)),
        (StringSet(a), StringSet(b)) => StringSet(a.union(b).cloned().collect()),
        // Any decimal / opaque / mixed-kind union → opaque (precision reduced).
        _ => Opaque {
            reason: "mixed-region union; gap precision reduced".into(),
        },
    }
}

// ── Per-field gap detection ──────────────────────────────────────────────────

/// Returns `(has_gap, sample_uncovered_value)` for the given union and field type.
fn field_gap(
    union: &FieldRegion,
    field_type: &ResolvedType,
    catalogue: &Catalogue,
) -> (bool, Option<TypedValue>) {
    use FieldRegion::*;
    match (union, field_type) {
        (Any, _) | (NotNull, _) => (false, None),
        (Empty, _) => {
            // No rule constrains this field — the entire domain is uncovered.
            (true, sample_value(field_type, catalogue))
        }
        (
            EnumSet {
                domain_id, values, ..
            },
            ResolvedType::Enum { .. },
        ) => {
            let all: BTreeSet<ValueId> = catalogue_values(*domain_id, catalogue)
                .into_iter()
                .collect();
            let uncovered: Vec<ValueId> = all.difference(values).copied().collect();
            match uncovered.first() {
                None => (false, None),
                Some(v) => (
                    true,
                    Some(TypedValue::Enum {
                        domain_id: *domain_id,
                        value_id: *v,
                    }),
                ),
            }
        }
        (BoolSet(s), ResolvedType::Bool) => {
            for b in [false, true] {
                if !s.contains(&b) {
                    return (true, Some(TypedValue::Bool(b)));
                }
            }
            (false, None)
        }
        (IntegerInterval(set), ResolvedType::Integer) => {
            let complement = set.complement();
            if complement.is_empty() {
                (false, None)
            } else {
                (true, complement.pick_witness().map(TypedValue::Integer))
            }
        }
        (NullOnly, _) => (true, sample_value(field_type, catalogue)),
        // Decimal / opaque / mixed → treat conservatively as "no detectable gap".
        // Returning false here avoids false positives.
        _ => (false, None),
    }
}

fn sample_value(field_type: &ResolvedType, catalogue: &Catalogue) -> Option<TypedValue> {
    match field_type {
        ResolvedType::Bool => Some(TypedValue::Bool(false)),
        ResolvedType::Integer => Some(TypedValue::Integer(0)),
        ResolvedType::Decimal => Some(TypedValue::Decimal(0.0)),
        ResolvedType::Str => Some(TypedValue::Str(String::new())),
        ResolvedType::Enum { domain_id } => {
            let vs = catalogue_values(*domain_id, catalogue);
            vs.first().map(|v| TypedValue::Enum {
                domain_id: *domain_id,
                value_id: *v,
            })
        }
    }
}

// ── Witness selection (§3.4.6 fields-different heuristic) ────────────────────

fn collect_witnesses(
    decision: &TypedDecision,
    union: &[FieldRegion],
    field_has_gap: &[bool],
    field_gap_witnesses: &[Option<TypedValue>],
    catalogue: &Catalogue,
    max: usize,
) -> Vec<UncoveredInputExample> {
    let n_fields = decision.input_schema.len();
    // Each witness varies one or more gap-fields to their uncovered value, leaving
    // other fields at a valid sample.  This is the "fields-different" heuristic in
    // its simplest form: examples 1..k each emphasise a different gap-field.
    let gap_fields: Vec<usize> = (0..n_fields).filter(|i| field_has_gap[*i]).collect();
    if gap_fields.is_empty() {
        return Vec::new();
    }

    // Default values for fields without a gap (a covered value).
    let defaults: Vec<TypedValue> = (0..n_fields)
        .map(|i| {
            sample_covered_value(&union[i], &decision.input_schema[i].field_type, catalogue)
                .unwrap_or(TypedValue::Null)
        })
        .collect();

    let mut witnesses: Vec<UncoveredInputExample> = Vec::new();
    for &gf in &gap_fields {
        if witnesses.len() >= max {
            break;
        }
        let Some(gap_val) = field_gap_witnesses[gf].clone() else {
            continue;
        };
        let mut fields = defaults.clone();
        fields[gf] = gap_val;
        witnesses.push(UncoveredInputExample {
            field_values: fields,
        });
    }
    witnesses
}

/// Pick a value the existing rules already cover (used as a default for fields
/// without a gap, when constructing witnesses for other fields' gaps).
fn sample_covered_value(
    union: &FieldRegion,
    field_type: &ResolvedType,
    catalogue: &Catalogue,
) -> Option<TypedValue> {
    use FieldRegion::*;
    match union {
        Any | NotNull => sample_value(field_type, catalogue),
        EnumSet {
            domain_id, values, ..
        } => values.iter().next().map(|v| TypedValue::Enum {
            domain_id: *domain_id,
            value_id: *v,
        }),
        BoolSet(s) => s.iter().next().map(|b| TypedValue::Bool(*b)),
        IntegerInterval(set) => set.pick_witness().map(TypedValue::Integer),
        StringSet(s) => s.iter().next().cloned().map(TypedValue::Str),
        // For fields that have no covering value (Empty / Opaque / NullOnly),
        // fall back to a generic sample so the witness is at least constructible.
        _ => sample_value(field_type, catalogue),
    }
}

// ── Finite gap count (only when every field is enum/boolean) ─────────────────

fn finite_gap_count(
    decision: &TypedDecision,
    union: &[FieldRegion],
    field_has_gap: &[bool],
    catalogue: &Catalogue,
) -> Option<usize> {
    let mut total: usize = 1;
    for (idx, schema) in decision.input_schema.iter().enumerate() {
        let domain_size = match &schema.field_type {
            ResolvedType::Bool => 2,
            ResolvedType::Enum { domain_id } => catalogue_values(*domain_id, catalogue).len(),
            // Integer/decimal/string — gap can be infinite.
            _ => return None,
        };
        let uncovered = if !field_has_gap[idx] {
            domain_size
        } else {
            match &union[idx] {
                FieldRegion::EnumSet { values, .. } => domain_size.saturating_sub(values.len()),
                FieldRegion::BoolSet(s) => 2usize.saturating_sub(s.len()),
                FieldRegion::Empty => domain_size,
                _ => return None,
            }
        };
        total = total.checked_mul(if field_has_gap[idx] {
            uncovered
        } else {
            domain_size
        })?;
    }
    // Subtract the covered cross-product — but per-field analysis can't compute
    // this exactly.  Return the upper bound (uncovered fields * full domains of
    // the rest), which is a useful approximate count.
    Some(total)
}
