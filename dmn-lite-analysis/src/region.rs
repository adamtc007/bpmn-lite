//! Per-rule input region: each rule is translated into a per-field constraint.
//!
//! The region for a rule is the set of inputs that satisfies the rule's `:when`
//! predicates.  For Profile v0.1, the implicit-AND structure of `:when` lets us
//! decompose this into independent per-field constraints (every predicate touches
//! exactly one field).  Compound predicates `(and ...)`, `(or ...)`, `(not ...)`
//! beyond the simple conjunction reduce per-field precision to `Opaque`; the rule
//! is still analysed for the non-opaque fields.
//!
//! The `RuleRegion` representation is the backbone of:
//! - overlap analysis (intersect two regions; non-empty intersection → overlap)
//! - unreachable analysis (one region ⊇ another)
//! - gap analysis (complement of union of all regions)

use std::collections::BTreeSet;

use dmn_lite_types::{
    Catalogue, DomainId, FieldId, ValueId,
    ir::{
        ComparisonOp, ResolvedType, TypedDecision, TypedPredicate, TypedRule, TypedValue, TypedWhen,
    },
};

use crate::intervals::IntervalSet;

// ── Region types ──────────────────────────────────────────────────────────────

/// Constraint on a single input field, derived from a rule's `:when` predicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldRegion {
    /// Empty — never matches (intersection of contradictory predicates).
    Empty,
    /// Any value including null (the field is unconstrained by this rule).
    Any,
    /// Any non-null value (from `is-not-null`).
    NotNull,
    /// Only null matches (from `is-null`).
    NullOnly,
    /// Specific enum values.  Always a subset of the field's domain.
    EnumSet {
        /// Domain the values belong to (carried for clarity in intersection logic).
        domain_id: DomainId,
        /// Sorted values that satisfy the constraint.
        values: BTreeSet<ValueId>,
    },
    /// Specific boolean values.
    BoolSet(BTreeSet<bool>),
    /// Integer interval set.
    IntegerInterval(IntervalSet),
    /// Decimal — limited analysis precision in Profile v0.1.
    DecimalOpaque,
    /// String equality set (from `(s = "v")` or `(s in ("a" "b"))`).
    StringSet(BTreeSet<String>),
    /// String opaque (e.g., contains a `!=` or `not`).
    StringOpaque,
    /// Could not express the field's constraint precisely (compound predicate /
    /// unsupported structure).  Conservatively treated as `Any` for intersections
    /// but flagged so callers know precision was reduced.
    Opaque {
        /// Reason for the loss of precision (for diagnostics).
        reason: String,
    },
}

/// Per-rule region: one `FieldRegion` per input field, in source order.
#[derive(Debug, Clone)]
pub struct RuleRegion {
    /// One entry per input field, indexed by `FieldId.0`.
    pub fields: Vec<FieldRegion>,
    /// True when this rule is a `:when (*)` catch-all.
    pub is_catch_all: bool,
}

// ── Top-level: compute all regions ────────────────────────────────────────────

/// Compute the rule region for every rule in source order.
pub fn compute_all(decision: &TypedDecision, catalogue: &Catalogue) -> Vec<RuleRegion> {
    decision
        .rules
        .iter()
        .map(|r| compute_one(r, &decision.input_schema, catalogue))
        .collect()
}

fn compute_one(
    rule: &TypedRule,
    schema: &[dmn_lite_types::ir::FieldSchema],
    catalogue: &Catalogue,
) -> RuleRegion {
    match &rule.when {
        TypedWhen::CatchAll(_) => RuleRegion {
            fields: vec![FieldRegion::Any; schema.len()],
            is_catch_all: true,
        },
        TypedWhen::Predicates(preds, _) => {
            // Each top-level predicate constrains exactly one field (in v0.1's
            // simple AND-of-comparisons fixtures). Compound predicates that span
            // multiple fields mark affected fields opaque.
            let mut fields: Vec<FieldRegion> = vec![FieldRegion::Any; schema.len()];
            for pred in preds {
                apply_predicate(pred, &mut fields, schema, catalogue);
            }
            RuleRegion {
                fields,
                is_catch_all: false,
            }
        }
    }
}

// ── Predicate → field region ──────────────────────────────────────────────────

fn apply_predicate(
    pred: &TypedPredicate,
    fields: &mut [FieldRegion],
    schema: &[dmn_lite_types::ir::FieldSchema],
    catalogue: &Catalogue,
) {
    match pred {
        TypedPredicate::Comparison { field, op, rhs, .. } => {
            apply_comparison(*field, *op, rhs, fields, schema, catalogue)
        }
        TypedPredicate::InSet { field, values, .. } => apply_in_set(*field, values, fields, schema),
        TypedPredicate::Range {
            field,
            lower,
            upper,
            lower_inclusive,
            upper_inclusive,
            ..
        } => apply_range(
            *field,
            lower,
            upper,
            *lower_inclusive,
            *upper_inclusive,
            fields,
        ),
        TypedPredicate::IsNull { field, .. } => {
            intersect_field(&mut fields[field.0], FieldRegion::NullOnly);
        }
        TypedPredicate::IsNotNull { field, .. } => {
            intersect_field(&mut fields[field.0], FieldRegion::NotNull);
        }
        TypedPredicate::Not { inner, .. } => apply_not(inner, fields, schema, catalogue),
        TypedPredicate::And { items, .. } => {
            for p in items {
                apply_predicate(p, fields, schema, catalogue);
            }
        }
        TypedPredicate::Or { items, .. } => {
            // Compute the union of regions across or-branches per field. For v0.1,
            // collect the field(s) each branch touches and OR them; fields touched
            // by some but not all branches → Opaque (loss of precision).
            apply_or(items, fields, schema, catalogue);
        }
    }
}

fn apply_comparison(
    field: FieldId,
    op: ComparisonOp,
    rhs: &TypedValue,
    fields: &mut [FieldRegion],
    schema: &[dmn_lite_types::ir::FieldSchema],
    catalogue: &Catalogue,
) {
    let field_type = &schema[field.0].field_type;
    let region = match (op, field_type, rhs) {
        (ComparisonOp::Eq, ResolvedType::Enum { domain_id }, TypedValue::Enum { value_id, .. }) => {
            FieldRegion::EnumSet {
                domain_id: *domain_id,
                values: [*value_id].into_iter().collect(),
            }
        }
        (
            ComparisonOp::NotEq,
            ResolvedType::Enum { domain_id },
            TypedValue::Enum { value_id, .. },
        ) => {
            let all = catalogue_values(*domain_id, catalogue);
            let mut values: BTreeSet<ValueId> = all.into_iter().collect();
            values.remove(value_id);
            FieldRegion::EnumSet {
                domain_id: *domain_id,
                values,
            }
        }
        (ComparisonOp::Eq, ResolvedType::Bool, TypedValue::Bool(b)) => {
            FieldRegion::BoolSet([*b].into_iter().collect())
        }
        (ComparisonOp::NotEq, ResolvedType::Bool, TypedValue::Bool(b)) => {
            let mut s = BTreeSet::new();
            s.insert(!*b);
            FieldRegion::BoolSet(s)
        }
        (op, ResolvedType::Integer, TypedValue::Integer(v)) => integer_comparison(op, *v),
        (_, ResolvedType::Decimal, _) => FieldRegion::DecimalOpaque,
        (op, ResolvedType::Str, TypedValue::Str(s)) => match op {
            ComparisonOp::Eq => {
                let mut set = BTreeSet::new();
                set.insert(s.clone());
                FieldRegion::StringSet(set)
            }
            _ => FieldRegion::StringOpaque,
        },
        _ => FieldRegion::Opaque {
            reason: "unsupported comparison/type combination".into(),
        },
    };
    intersect_field(&mut fields[field.0], region);
}

fn integer_comparison(op: ComparisonOp, v: i64) -> FieldRegion {
    let set = match op {
        ComparisonOp::Eq => IntervalSet::singleton(v),
        ComparisonOp::NotEq => IntervalSet::singleton(v).complement(),
        ComparisonOp::Lt => IntervalSet::from_range(None, Some(v), false, false),
        ComparisonOp::Le => IntervalSet::from_range(None, Some(v), false, true),
        ComparisonOp::Gt => IntervalSet::from_range(Some(v), None, false, false),
        ComparisonOp::Ge => IntervalSet::from_range(Some(v), None, true, false),
    };
    FieldRegion::IntegerInterval(set)
}

fn apply_in_set(
    field: FieldId,
    values: &[TypedValue],
    fields: &mut [FieldRegion],
    schema: &[dmn_lite_types::ir::FieldSchema],
) {
    let field_type = &schema[field.0].field_type;
    let region = match field_type {
        ResolvedType::Enum { domain_id } => {
            let vs: BTreeSet<ValueId> = values
                .iter()
                .filter_map(|v| match v {
                    TypedValue::Enum { value_id, .. } => Some(*value_id),
                    _ => None,
                })
                .collect();
            FieldRegion::EnumSet {
                domain_id: *domain_id,
                values: vs,
            }
        }
        ResolvedType::Integer => {
            let mut set = IntervalSet::empty();
            for v in values {
                if let TypedValue::Integer(i) = v {
                    set = set.union(&IntervalSet::singleton(*i));
                }
            }
            FieldRegion::IntegerInterval(set)
        }
        ResolvedType::Bool => {
            let s: BTreeSet<bool> = values
                .iter()
                .filter_map(|v| match v {
                    TypedValue::Bool(b) => Some(*b),
                    _ => None,
                })
                .collect();
            FieldRegion::BoolSet(s)
        }
        ResolvedType::Str => {
            let s: BTreeSet<String> = values
                .iter()
                .filter_map(|v| match v {
                    TypedValue::Str(x) => Some(x.clone()),
                    _ => None,
                })
                .collect();
            FieldRegion::StringSet(s)
        }
        ResolvedType::Decimal => FieldRegion::DecimalOpaque,
    };
    intersect_field(&mut fields[field.0], region);
}

fn apply_range(
    field: FieldId,
    lower: &Option<TypedValue>,
    upper: &Option<TypedValue>,
    lower_inc: bool,
    upper_inc: bool,
    fields: &mut [FieldRegion],
) {
    let lo = lower.as_ref().and_then(|v| match v {
        TypedValue::Integer(i) => Some(*i),
        _ => None,
    });
    let up = upper.as_ref().and_then(|v| match v {
        TypedValue::Integer(i) => Some(*i),
        _ => None,
    });
    // Decimal range: opaque.
    if matches!(lower, Some(TypedValue::Decimal(_)))
        || matches!(upper, Some(TypedValue::Decimal(_)))
    {
        intersect_field(&mut fields[field.0], FieldRegion::DecimalOpaque);
        return;
    }
    let set = IntervalSet::from_range(lo, up, lower_inc, upper_inc);
    intersect_field(&mut fields[field.0], FieldRegion::IntegerInterval(set));
}

fn apply_not(
    inner: &TypedPredicate,
    fields: &mut [FieldRegion],
    schema: &[dmn_lite_types::ir::FieldSchema],
    catalogue: &Catalogue,
) {
    // (not (f = v)) on enum/bool is invertible to an EnumSet/BoolSet.
    // For other forms, mark the touched field(s) opaque.
    match inner {
        TypedPredicate::Comparison { field, op, rhs, .. } => {
            // (not (f = v)) ≡ (f != v) for the non-null case, which is what regions model.
            let flipped = match op {
                ComparisonOp::Eq => Some(ComparisonOp::NotEq),
                ComparisonOp::NotEq => Some(ComparisonOp::Eq),
                ComparisonOp::Lt => Some(ComparisonOp::Ge),
                ComparisonOp::Le => Some(ComparisonOp::Gt),
                ComparisonOp::Gt => Some(ComparisonOp::Le),
                ComparisonOp::Ge => Some(ComparisonOp::Lt),
            };
            if let Some(f_op) = flipped {
                apply_comparison(*field, f_op, rhs, fields, schema, catalogue);
                return;
            }
        }
        TypedPredicate::IsNull { field, .. } => {
            intersect_field(&mut fields[field.0], FieldRegion::NotNull);
            return;
        }
        TypedPredicate::IsNotNull { field, .. } => {
            intersect_field(&mut fields[field.0], FieldRegion::NullOnly);
            return;
        }
        _ => {}
    }
    // Generic fallback: mark touched fields opaque.
    let mut touched: BTreeSet<usize> = BTreeSet::new();
    collect_touched_fields(inner, &mut touched);
    for idx in touched {
        intersect_field(
            &mut fields[idx],
            FieldRegion::Opaque {
                reason: "negation of compound predicate".into(),
            },
        );
    }
}

fn apply_or(
    items: &[TypedPredicate],
    fields: &mut [FieldRegion],
    schema: &[dmn_lite_types::ir::FieldSchema],
    catalogue: &Catalogue,
) {
    // For each branch, compute a fresh region for the fields it touches. Union
    // matching branches per field; fields touched by some branches but not all
    // become opaque (since the unconstrained branches admit any value).
    let mut branch_regions: Vec<Vec<FieldRegion>> = Vec::with_capacity(items.len());
    for p in items {
        let mut b = vec![FieldRegion::Any; schema.len()];
        apply_predicate(p, &mut b, schema, catalogue);
        branch_regions.push(b);
    }
    if branch_regions.is_empty() {
        return;
    }
    let n_fields = fields.len();
    for f_idx in 0..n_fields {
        let union = branch_regions
            .iter()
            .skip(1)
            .fold(branch_regions[0][f_idx].clone(), |acc, br| {
                union_field(&acc, &br[f_idx])
            });
        intersect_field(&mut fields[f_idx], union);
    }
}

fn collect_touched_fields(pred: &TypedPredicate, out: &mut BTreeSet<usize>) {
    match pred {
        TypedPredicate::Comparison { field, .. }
        | TypedPredicate::InSet { field, .. }
        | TypedPredicate::Range { field, .. }
        | TypedPredicate::IsNull { field, .. }
        | TypedPredicate::IsNotNull { field, .. } => {
            out.insert(field.0);
        }
        TypedPredicate::Not { inner, .. } => collect_touched_fields(inner, out),
        TypedPredicate::And { items, .. } | TypedPredicate::Or { items, .. } => {
            for p in items {
                collect_touched_fields(p, out);
            }
        }
    }
}

// ── Field region operations ───────────────────────────────────────────────────

/// In-place intersection: `*dst = dst ∩ other`.
pub fn intersect_field(dst: &mut FieldRegion, other: FieldRegion) {
    let new = intersect(dst, &other);
    *dst = new;
}

/// Intersection of two field regions.
pub fn intersect(a: &FieldRegion, b: &FieldRegion) -> FieldRegion {
    use FieldRegion::*;
    match (a, b) {
        (Empty, _) | (_, Empty) => Empty,
        (Any, x) | (x, Any) => x.clone(),
        (NullOnly, NullOnly) => NullOnly,
        (NullOnly, NotNull) | (NotNull, NullOnly) => Empty,
        (NullOnly, _) => Empty,
        (_, NullOnly) => Empty,
        (NotNull, NotNull) => NotNull,
        (NotNull, other) | (other, NotNull) => other.clone(),
        (
            EnumSet {
                domain_id: d1,
                values: v1,
            },
            EnumSet {
                domain_id: d2,
                values: v2,
            },
        ) => {
            if d1 != d2 {
                Empty
            } else {
                let common: BTreeSet<ValueId> = v1.intersection(v2).copied().collect();
                if common.is_empty() {
                    Empty
                } else {
                    EnumSet {
                        domain_id: *d1,
                        values: common,
                    }
                }
            }
        }
        (BoolSet(a), BoolSet(b)) => {
            let i: BTreeSet<bool> = a.intersection(b).copied().collect();
            if i.is_empty() { Empty } else { BoolSet(i) }
        }
        (IntegerInterval(a), IntegerInterval(b)) => {
            let i = a.intersect(b);
            if i.is_empty() {
                Empty
            } else {
                IntegerInterval(i)
            }
        }
        (DecimalOpaque, _) | (_, DecimalOpaque) => DecimalOpaque,
        (StringSet(a), StringSet(b)) => {
            let i: BTreeSet<String> = a.intersection(b).cloned().collect();
            if i.is_empty() { Empty } else { StringSet(i) }
        }
        (StringOpaque, _) | (_, StringOpaque) => StringOpaque,
        (Opaque { reason }, _) | (_, Opaque { reason }) => Opaque {
            reason: reason.clone(),
        },
        // Type mismatch (should not happen for a well-typed decision).
        _ => Empty,
    }
}

/// Union of two field regions.
fn union_field(a: &FieldRegion, b: &FieldRegion) -> FieldRegion {
    use FieldRegion::*;
    match (a, b) {
        (Empty, x) | (x, Empty) => x.clone(),
        (Any, _) | (_, Any) => Any,
        (NullOnly, NullOnly) => NullOnly,
        (NotNull, NotNull) => NotNull,
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
            EnumSet {
                domain_id: *d1,
                values: merged,
            }
        }
        (BoolSet(a), BoolSet(b)) => BoolSet(a.union(b).copied().collect()),
        (IntegerInterval(a), IntegerInterval(b)) => IntegerInterval(a.union(b)),
        (StringSet(a), StringSet(b)) => StringSet(a.union(b).cloned().collect()),
        // Mixed cases: lose precision.
        _ => Opaque {
            reason: "union of mixed region kinds".into(),
        },
    }
}

/// True when `a` is a subset of `b`.  Catch-all (Any on every field) is a
/// superset of everything; Empty is a subset of everything.
pub fn is_subset_of(a: &FieldRegion, b: &FieldRegion) -> bool {
    use FieldRegion::*;
    match (a, b) {
        (Empty, _) => true,
        (_, Any) => true,
        (Any, _) => false, // Any is only a subset of Any (caught above)
        (NotNull, NotNull) => true,
        (NullOnly, NullOnly) => true,
        (NotNull, NullOnly) | (NullOnly, NotNull) => false,
        (
            EnumSet {
                domain_id: d1,
                values: v1,
            },
            EnumSet {
                domain_id: d2,
                values: v2,
            },
        ) => d1 == d2 && v1.is_subset(v2),
        (BoolSet(a), BoolSet(b)) => a.is_subset(b),
        (IntegerInterval(a), IntegerInterval(b)) => {
            // a ⊆ b iff a ∩ complement(b) is empty.
            a.intersect(&b.complement()).is_empty()
        }
        (StringSet(a), StringSet(b)) => a.is_subset(b),
        (NotNull, EnumSet { .. } | BoolSet(_) | IntegerInterval(_) | StringSet(_)) => false,
        (EnumSet { .. } | BoolSet(_) | IntegerInterval(_) | StringSet(_), NotNull) => true, // any concrete value is non-null
        (DecimalOpaque, _) | (_, DecimalOpaque) => false, // can't tell
        (Opaque { .. }, _) | (_, Opaque { .. }) => false,
        (StringOpaque, _) | (_, StringOpaque) => false,
        _ => false,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// All `ValueId`s declared in a catalogue domain, looked up by `DomainId`.
pub fn catalogue_values(domain_id: DomainId, catalogue: &Catalogue) -> Vec<ValueId> {
    let domain = catalogue
        .domains()
        .find(|d| d.domain_id == domain_id)
        .expect("domain referenced in IR must exist in catalogue");
    let mut vs: Vec<ValueId> = domain.values().map(|v| v.value_id).collect();
    vs.sort_by_key(|v| v.0);
    vs
}
