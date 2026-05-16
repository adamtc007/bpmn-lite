//! Reference evaluator over [`TypedDecision`] typed predicate IR.
//!
//! This is the **differential testing oracle** for the bytecode VM (Phase 1.4).
//! It is a direct, mechanical translation of `dmn-lite-semantics.md §4` into
//! Rust. It is deliberately not optimised:
//!
//! - Every predicate in every rule is evaluated regardless of earlier results.
//! - `(and p1 p2 p3)` evaluates all three even if `p1` is false.
//! - `(or p1 p2 p3)` evaluates all three even if `p1` is true.
//! - All rules are evaluated under both UNIQUE and FIRST; hit policy is
//!   applied after evaluation.
//!
//! This no-short-circuit discipline is load-bearing for Phase 1.5: if the VM
//! incorrectly skips a predicate, the differential harness will catch it
//! because the reference trace will show a result where the VM trace has none.

use std::cmp::Ordering;

use dmn_lite_types::{
    EvalError, EvaluationTrace, FieldId, PredicateTrace, RuleId, RuleTrace, TraceOutcome,
    TypedInputContext, TypedOutputContext,
    ids::SourceSpan,
    ir::{
        ComparisonOp, HitPolicy, ResolvedType, TypedAssignment, TypedDecision, TypedPredicate,
        TypedRule, TypedValue, TypedWhen,
    },
    values::compute_schema_hash,
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Output of a successful reference evaluation.
#[derive(Debug, Clone)]
pub struct EvaluationOutput {
    /// The matched rule's output bindings.
    pub output: TypedOutputContext,
    /// Full per-rule, per-predicate evaluation trace.
    pub trace: EvaluationTrace,
}

/// Evaluate a typed decision against a typed input context.
///
/// Returns [`EvaluationOutput`] on success, [`EvalError`] on runtime errors
/// (schema mismatch, missing input, hit-policy violation, etc.).
///
/// The `source` string is used to extract human-readable descriptions for
/// each predicate in the trace (Option A from the Phase 1.3 design). Pass
/// an empty string if the original source is unavailable; descriptions will
/// be empty but the trace will still be structurally correct.
///
/// **No short-circuit evaluation.** Every predicate in every rule is
/// evaluated. See the module-level documentation for why this matters.
pub fn evaluate(
    decision: &TypedDecision,
    input: &TypedInputContext,
    source: &str,
) -> Result<EvaluationOutput, EvalError> {
    // ── 1. Validate input against decision schema ───────────────────────────
    let expected_hash = compute_schema_hash(&decision.input_schema);
    if expected_hash != input.schema_hash {
        return Err(EvalError::SchemaHashMismatch);
    }
    if input.len() != decision.input_schema.len() {
        return Err(EvalError::InputSchemaMismatch {
            expected: decision.input_schema.len(),
            actual: input.len(),
        });
    }
    validate_input_types(decision, input)?;

    // ── 2. Evaluate every rule (no short-circuit across rules) ──────────────
    let mut rule_traces: Vec<RuleTrace> = Vec::with_capacity(decision.rules.len());
    let mut matched_rules: Vec<(RuleId, &[TypedAssignment])> = Vec::new();

    for rule in &decision.rules {
        let (rule_matched, pred_traces) = eval_rule(rule, input, source);
        if rule_matched {
            matched_rules.push((rule.rule_id, &rule.then));
        }
        rule_traces.push(RuleTrace {
            rule_id: rule.rule_id,
            rule_name: rule.rule_name.clone(),
            matched: rule_matched,
            predicates: pred_traces,
            source_span: rule.source_span,
        });
    }

    // ── 3. Apply hit policy ─────────────────────────────────────────────────
    let (winner_id, winner_then, outcome) = apply_hit_policy(decision.hit_policy, &matched_rules)?;

    // ── 4. Build output context ─────────────────────────────────────────────
    let output = build_output_context(&decision.output_schema, winner_then);

    let trace = EvaluationTrace {
        rules: rule_traces,
        outcome: TraceOutcome::Match { rule_id: winner_id },
    };
    let _ = outcome; // outcome consumed into trace above

    Ok(EvaluationOutput { output, trace })
}

// ── Input validation ──────────────────────────────────────────────────────────

fn validate_input_types(
    decision: &TypedDecision,
    input: &TypedInputContext,
) -> Result<(), EvalError> {
    for field in &decision.input_schema {
        let fid = field.field_id;
        let value = match input.get(fid) {
            None => continue,                   // missing — checked at predicate time
            Some(TypedValue::Null) => continue, // null — checked at predicate time
            Some(v) => v,
        };
        check_input_type_match(fid, &field.name, &field.field_type, value)?;
    }
    Ok(())
}

fn check_input_type_match(
    field_id: FieldId,
    field_name: &str,
    expected: &ResolvedType,
    actual: &TypedValue,
) -> Result<(), EvalError> {
    let ok = match (expected, actual) {
        (ResolvedType::Enum { domain_id }, TypedValue::Enum { domain_id: d, .. }) => {
            if domain_id != d {
                return Err(EvalError::InputDomainMismatch {
                    field: field_name.to_owned(),
                    field_id,
                    domain: domain_id.to_string(),
                    symbol: format!("domain {d}"),
                });
            }
            true
        }
        (ResolvedType::Enum { .. }, _) => false,
        (ResolvedType::Bool, TypedValue::Bool(_)) => true,
        (ResolvedType::Bool, _) => false,
        (ResolvedType::Integer, TypedValue::Integer(_)) => true,
        (ResolvedType::Integer, _) => false,
        // Decimal accepts integers (widened at evaluation time)
        (ResolvedType::Decimal, TypedValue::Decimal(_) | TypedValue::Integer(_)) => true,
        (ResolvedType::Decimal, _) => false,
        (ResolvedType::Str, TypedValue::Str(_)) => true,
        (ResolvedType::Str, _) => false,
    };
    if !ok {
        return Err(EvalError::InputTypeMismatch {
            field: field_name.to_owned(),
            field_id,
            expected: expected.type_name().into(),
            actual: value_type_name(actual).into(),
        });
    }
    Ok(())
}

// ── Rule evaluation ───────────────────────────────────────────────────────────

fn eval_rule(
    rule: &TypedRule,
    input: &TypedInputContext,
    source: &str,
) -> (bool, Vec<PredicateTrace>) {
    match &rule.when {
        TypedWhen::CatchAll(span) => {
            let trace = PredicateTrace {
                result: true,
                source_span: *span,
                description: "catch-all".into(),
            };
            (true, vec![trace])
        }
        TypedWhen::Predicates(preds, _) => {
            // No short-circuit: evaluate ALL predicates regardless of earlier results.
            let pred_traces: Vec<PredicateTrace> = preds
                .iter()
                .map(|p| {
                    let result = eval_predicate(p, input);
                    PredicateTrace {
                        result,
                        source_span: pred_span(p),
                        description: extract_source(source, pred_span(p)),
                    }
                })
                .collect();
            // A rule matches iff all its predicates hold (implicit conjunction).
            let matched = pred_traces.iter().all(|p| p.result);
            (matched, pred_traces)
        }
    }
}

// ── Hit-policy enforcement ────────────────────────────────────────────────────

fn apply_hit_policy<'a>(
    hit_policy: HitPolicy,
    matched: &'a [(RuleId, &'a [TypedAssignment])],
) -> Result<(RuleId, &'a [TypedAssignment], TraceOutcome), EvalError> {
    match hit_policy {
        HitPolicy::Unique => match matched {
            [] => Err(EvalError::NoMatch),
            [(id, then)] => Ok((*id, then, TraceOutcome::Match { rule_id: *id })),
            _ => {
                let ids: Vec<RuleId> = matched.iter().map(|(id, _)| *id).collect();
                Err(EvalError::MultipleMatches { rules: ids })
            }
        },
        HitPolicy::First => match matched.first() {
            None => Err(EvalError::NoMatch),
            Some((id, then)) => Ok((*id, then, TraceOutcome::Match { rule_id: *id })),
        },
    }
}

// ── Output context construction ───────────────────────────────────────────────

fn build_output_context(
    output_schema: &[dmn_lite_types::ir::FieldSchema],
    assignments: &[TypedAssignment],
) -> TypedOutputContext {
    let mut slots: Vec<TypedValue> = vec![TypedValue::Null; output_schema.len()];
    for a in assignments {
        slots[a.output_field.0] = a.value.clone();
    }
    TypedOutputContext::from_slots(output_schema, slots)
}

// ── Predicate evaluation ──────────────────────────────────────────────────────
// One function per predicate kind, matching semantics doc §4.3 ordering.
// No short-circuit inside `and`/`or` — all sub-predicates always evaluated.

fn eval_predicate(pred: &TypedPredicate, input: &TypedInputContext) -> bool {
    match pred {
        TypedPredicate::Comparison { field, op, rhs, .. } => {
            eval_comparison(*field, *op, rhs, input)
        }
        TypedPredicate::InSet { field, values, .. } => eval_in_set(*field, values, input),
        TypedPredicate::Range {
            field,
            lower,
            upper,
            lower_inclusive,
            upper_inclusive,
            ..
        } => eval_range(
            *field,
            lower,
            upper,
            *lower_inclusive,
            *upper_inclusive,
            input,
        ),
        TypedPredicate::IsNull { field, .. } => eval_is_null(*field, input),
        TypedPredicate::IsNotNull { field, .. } => eval_is_not_null(*field, input),
        TypedPredicate::Not { inner, .. } => !eval_predicate(inner, input),
        TypedPredicate::And { items, .. } => {
            // No short-circuit: collect ALL results before applying conjunction.
            let results: Vec<bool> = items.iter().map(|p| eval_predicate(p, input)).collect();
            results.iter().all(|&r| r)
        }
        TypedPredicate::Or { items, .. } => {
            // No short-circuit: collect ALL results before applying disjunction.
            let results: Vec<bool> = items.iter().map(|p| eval_predicate(p, input)).collect();
            results.iter().any(|&r| r)
        }
    }
}

/// `(field = rhs)` or `(field != rhs)` — semantics doc §4.3.
///
/// Null on either side of `=` or `!=` produces `false` (two-valued logic).
fn eval_comparison(
    field: FieldId,
    op: ComparisonOp,
    rhs: &TypedValue,
    input: &TypedInputContext,
) -> bool {
    let lhs = match input.get(field) {
        None | Some(TypedValue::Null) => return false,
        Some(v) => v,
    };
    match op {
        ComparisonOp::Eq => values_equal(lhs, rhs),
        ComparisonOp::NotEq => {
            // Null on RHS also returns false (consistent with §4.3 "both null → false").
            if matches!(rhs, TypedValue::Null) {
                return false;
            }
            !values_equal(lhs, rhs)
        }
        ComparisonOp::Lt => compare_numeric(lhs, rhs).is_some_and(|o| o == Ordering::Less),
        ComparisonOp::Le => compare_numeric(lhs, rhs).is_some_and(|o| o != Ordering::Greater),
        ComparisonOp::Gt => compare_numeric(lhs, rhs).is_some_and(|o| o == Ordering::Greater),
        ComparisonOp::Ge => compare_numeric(lhs, rhs).is_some_and(|o| o != Ordering::Less),
    }
}

/// `(field in (v1 v2 ...))` — set membership.
///
/// Null input returns `false` per §4.3.
fn eval_in_set(field: FieldId, values: &[TypedValue], input: &TypedInputContext) -> bool {
    let lhs = match input.get(field) {
        None | Some(TypedValue::Null) => return false,
        Some(v) => v,
    };
    values.iter().any(|v| values_equal(lhs, v))
}

/// `(field in [lower .. upper])` — range membership.
///
/// Null input returns `false` per §4.3. `None` bound means unbounded.
fn eval_range(
    field: FieldId,
    lower: &Option<TypedValue>,
    upper: &Option<TypedValue>,
    lower_inclusive: bool,
    upper_inclusive: bool,
    input: &TypedInputContext,
) -> bool {
    let lhs = match input.get(field) {
        None | Some(TypedValue::Null) => return false,
        Some(v) => v,
    };

    // Check lower bound.
    if let Some(lo) = lower {
        match compare_numeric(lhs, lo) {
            None => return false, // type mismatch — shouldn't happen after Phase 1.2
            Some(Ordering::Less) => return false,
            Some(Ordering::Equal) if !lower_inclusive => return false,
            _ => {}
        }
    }

    // Check upper bound.
    if let Some(hi) = upper {
        match compare_numeric(lhs, hi) {
            None => return false,
            Some(Ordering::Greater) => return false,
            Some(Ordering::Equal) if !upper_inclusive => return false,
            _ => {}
        }
    }

    true
}

/// `(field is-null)` — true iff field is missing or explicitly null.
fn eval_is_null(field: FieldId, input: &TypedInputContext) -> bool {
    match input.get(field) {
        None | Some(TypedValue::Null) => true,
        Some(_) => false,
    }
}

/// `(field is-not-null)` — true iff field is present and non-null.
fn eval_is_not_null(field: FieldId, input: &TypedInputContext) -> bool {
    match input.get(field) {
        None | Some(TypedValue::Null) => false,
        Some(_) => true,
    }
}

// ── Value comparison helpers ──────────────────────────────────────────────────

/// Structural equality over typed values.
///
/// Uses `PartialEq` which handles `f64::NAN != f64::NAN` correctly.
fn values_equal(a: &TypedValue, b: &TypedValue) -> bool {
    a == b
}

/// Numeric comparison, with integer→decimal widening.
///
/// Returns `None` for incompatible type pairs (should not occur in
/// well-typed decisions, but handled gracefully here).
fn compare_numeric(a: &TypedValue, b: &TypedValue) -> Option<Ordering> {
    match (a, b) {
        (TypedValue::Integer(x), TypedValue::Integer(y)) => x.partial_cmp(y),
        (TypedValue::Integer(x), TypedValue::Decimal(y)) => (*x as f64).partial_cmp(y),
        (TypedValue::Decimal(x), TypedValue::Integer(y)) => x.partial_cmp(&(*y as f64)),
        (TypedValue::Decimal(x), TypedValue::Decimal(y)) => x.partial_cmp(y),
        _ => None,
    }
}

// ── Span and description helpers ──────────────────────────────────────────────

/// Extract the source span of a predicate (for trace recording).
fn pred_span(pred: &TypedPredicate) -> SourceSpan {
    match pred {
        TypedPredicate::Comparison { source_span, .. }
        | TypedPredicate::InSet { source_span, .. }
        | TypedPredicate::Range { source_span, .. }
        | TypedPredicate::IsNull { source_span, .. }
        | TypedPredicate::IsNotNull { source_span, .. }
        | TypedPredicate::Not { source_span, .. }
        | TypedPredicate::And { source_span, .. }
        | TypedPredicate::Or { source_span, .. } => *source_span,
    }
}

/// Extract the source text for a span (Option A: re-read source bytes).
///
/// Returns an empty string if the source is empty or the span is out of range.
fn extract_source(source: &str, span: SourceSpan) -> String {
    source
        .get(span.start as usize..span.end as usize)
        .unwrap_or("")
        .to_owned()
}

fn value_type_name(v: &TypedValue) -> &'static str {
    match v {
        TypedValue::Enum { .. } => "enum",
        TypedValue::Bool(_) => "bool",
        TypedValue::Integer(_) => "integer",
        TypedValue::Decimal(_) => "decimal",
        TypedValue::Str(_) => "string",
        TypedValue::Null => "null",
    }
}
