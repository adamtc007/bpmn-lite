//! Proptest strategies for generating well-typed `TypedInputContext` values.
//!
//! The key constraint: generated inputs must be *schema-valid* — correct slot
//! count, correct types, catalogue-legal enum values — so that both evaluators
//! can consume them without returning `SchemaHashMismatch` or `InputSchemaMismatch`.
//!
//! For each field:
//! - **5%** probability of "missing" (slot absent from context).
//! - **5%** probability of `TypedValue::Null` (explicitly null).
//! - **90%** probability of a typed value.
//!
//! For integer fields, 30% of the typed-value attempts use boundary values
//! extracted from the decision's range and comparison predicates; 70% use a
//! uniform random `i64`.  This ensures boundary conditions (the points most
//! likely to expose off-by-one bugs) receive proportional coverage.

use proptest::prelude::*;
use proptest::sample::select;

use dmn_lite_compiler::Catalogue;
use dmn_lite_types::{
    DomainId, FieldId,
    ir::{ResolvedType, TypedDecision, TypedPredicate, TypedValue, TypedWhen},
    values::{TypedInputContext, TypedInputContextBuilder},
};

// ── Public entry point ────────────────────────────────────────────────────────

/// Generate a well-typed `TypedInputContext` that conforms to `decision`'s
/// input schema and respects catalogue domain membership for enum fields.
///
/// Strategies are built once per property invocation (not per case) and reused
/// across all generated inputs, keeping proptest's shrinking intact.
pub fn input_strategy(
    decision: &TypedDecision,
    catalogue: &Catalogue,
) -> impl Strategy<Value = TypedInputContext> {
    let schema = decision.input_schema.clone();
    let boundaries = collect_integer_boundaries(decision);

    // Build one strategy per input field.
    let slot_strats: Vec<BoxedStrategy<Option<TypedValue>>> = schema
        .iter()
        .map(|f| slot_strategy(&f.field_type, catalogue, &boundaries))
        .collect();

    // Fold Vec<BoxedStrategy<Option<TypedValue>>> → Strategy<Vec<Option<TypedValue>>>.
    // This preserves proptest's shrinking: each slot shrinks independently.
    let initial: BoxedStrategy<Vec<Option<TypedValue>>> = Just(Vec::new()).boxed();
    let vec_strat = slot_strats.into_iter().fold(initial, |acc, s| {
        (acc, s)
            .prop_map(|(mut v, item)| {
                v.push(item);
                v
            })
            .boxed()
    });

    vec_strat.prop_map(move |slots| build_context(&schema, slots))
}

// ── Per-slot strategy ─────────────────────────────────────────────────────────

/// Strategy for a single input slot.  Mixes missing (None), null
/// (Some(Null)), and typed values according to fixed probability weights.
fn slot_strategy(
    field_type: &ResolvedType,
    catalogue: &Catalogue,
    boundaries: &[i64],
) -> BoxedStrategy<Option<TypedValue>> {
    let value_strat: BoxedStrategy<TypedValue> = match field_type {
        ResolvedType::Enum { domain_id } => enum_value_strategy(*domain_id, catalogue),
        ResolvedType::Bool => any::<bool>().prop_map(TypedValue::Bool).boxed(),
        ResolvedType::Integer => integer_strategy(boundaries),
        ResolvedType::Decimal => decimal_strategy(),
        ResolvedType::Str => string_strategy(),
    };

    // 5% missing, 5% null, 90% typed value.
    prop_oneof![
        5 => Just(None),
        5 => Just(Some(TypedValue::Null)),
        90 => value_strat.prop_map(Some),
    ]
    .boxed()
}

// ── Type-specific strategies ──────────────────────────────────────────────────

/// Generate a legal `TypedValue::Enum` for the given domain.
/// Selects uniformly from the domain's declared values (sorted by value_id
/// for determinism; proptest `select` shrinks toward index 0).
fn enum_value_strategy(domain_id: DomainId, catalogue: &Catalogue) -> BoxedStrategy<TypedValue> {
    // Collect sorted (value_id, domain_id) pairs — sorted so the strategy is
    // deterministic regardless of HashMap iteration order (§6.6 discipline).
    let domain = catalogue
        .domains()
        .find(|d| d.domain_id == domain_id)
        .expect("enum field's domain_id must be in catalogue");

    let mut pairs: Vec<(DomainId, dmn_lite_types::ValueId)> =
        domain.values().map(|v| (domain_id, v.value_id)).collect();
    pairs.sort_by_key(|(_, vid)| vid.0); // deterministic order

    if pairs.is_empty() {
        // Non-enum domain used with enum field type (shouldn't happen in v0.1).
        return Just(TypedValue::Null).boxed();
    }

    select(pairs)
        .prop_map(|(domain_id, value_id)| TypedValue::Enum {
            domain_id,
            value_id,
        })
        .boxed()
}

/// Boundary-aware integer strategy.
///
/// 30% of generated values are boundary values extracted from the decision's
/// range and comparison predicates (± 1 around each boundary, to catch
/// off-by-one errors in exclusive/inclusive bound handling).
/// 70% are uniformly random `i64`s.
fn integer_strategy(boundaries: &[i64]) -> BoxedStrategy<TypedValue> {
    if boundaries.is_empty() {
        return any::<i64>().prop_map(TypedValue::Integer).boxed();
    }
    let boundaries = boundaries.to_vec();
    prop_oneof![
        3 => select(boundaries).prop_map(TypedValue::Integer),
        7 => any::<i64>().prop_map(TypedValue::Integer),
    ]
    .boxed()
}

/// Finite-only `f64` strategy.  NaN and infinities are a v0.3+ concern.
fn decimal_strategy() -> BoxedStrategy<TypedValue> {
    // Use a range that always produces finite values without prop_filter
    // (prop_filter disables shrinking).
    (-1.0e15f64..1.0e15f64)
        .prop_map(TypedValue::Decimal)
        .boxed()
}

/// Short printable-ASCII strings.  Profile v0.1 doesn't exercise string
/// predicates heavily; coverage is intentionally light.
fn string_strategy() -> BoxedStrategy<TypedValue> {
    "[[:print:]]{0,20}".prop_map(TypedValue::Str).boxed()
}

// ── Boundary extraction ───────────────────────────────────────────────────────

/// Walk `decision`'s typed IR and collect every integer literal that appears
/// in a range or comparison predicate.  For each literal `v`, also add `v-1`
/// and `v+1` so boundary-adjacent values are exercised.
///
/// Returns a deduplicated, sorted list of boundary values.
pub fn collect_integer_boundaries(decision: &TypedDecision) -> Vec<i64> {
    let mut out: Vec<i64> = vec![0, 1, -1];
    for rule in &decision.rules {
        match &rule.when {
            TypedWhen::CatchAll(_) => {}
            TypedWhen::Predicates(preds, _) => {
                for pred in preds {
                    collect_from_pred(pred, &mut out);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn collect_from_pred(pred: &TypedPredicate, out: &mut Vec<i64>) {
    match pred {
        TypedPredicate::Range { lower, upper, .. } => {
            for bound in [lower.as_ref(), upper.as_ref()].into_iter().flatten() {
                if let TypedValue::Integer(v) = bound {
                    out.push(*v);
                    out.push(v.saturating_sub(1));
                    out.push(v.saturating_add(1));
                }
            }
        }
        TypedPredicate::Comparison {
            rhs: TypedValue::Integer(v),
            ..
        } => {
            out.push(*v);
            out.push(v.saturating_sub(1));
            out.push(v.saturating_add(1));
        }
        TypedPredicate::And { items, .. } | TypedPredicate::Or { items, .. } => {
            for p in items {
                collect_from_pred(p, out);
            }
        }
        TypedPredicate::Not { inner, .. } => collect_from_pred(inner, out),
        _ => {}
    }
}

// ── Context construction ──────────────────────────────────────────────────────

/// Convert `Vec<Option<TypedValue>>` (from strategy) into a `TypedInputContext`.
/// - `None` → field left missing
/// - `Some(TypedValue::Null)` → field explicitly set to null
/// - `Some(v)` → field set to `v`
fn build_context(
    schema: &[dmn_lite_types::ir::FieldSchema],
    slots: Vec<Option<TypedValue>>,
) -> TypedInputContext {
    let mut builder = TypedInputContextBuilder::new(schema);
    for (idx, slot) in slots.into_iter().enumerate() {
        match slot {
            None => {} // missing — do not set
            Some(TypedValue::Null) => {
                builder.set_null(FieldId(idx));
            }
            Some(v) => {
                builder.set(FieldId(idx), v);
            }
        }
    }
    builder.build()
}
