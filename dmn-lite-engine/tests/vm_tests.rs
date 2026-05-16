//! Stack VM tests — Phase 1.4 §3.7.
//!
//! Exercises `dmn_lite_engine::vm::evaluate` against decisions compiled with
//! `compile_and_verify`.

use dmn_lite_compiler::{compile_and_verify, load_catalogue_from_str};
use dmn_lite_engine::vm;
use dmn_lite_parser::parse;
use dmn_lite_types::{
    EvalError, FieldId, RuleId, TraceOutcome, ir::TypedValue, values::TypedInputContextBuilder,
};

// ── Catalogue helpers ─────────────────────────────────────────────────────────

const INT_CAT: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"
[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "integers"
"#;

fn int_cat() -> dmn_lite_compiler::Catalogue {
    load_catalogue_from_str(INT_CAT).expect("int_cat must load")
}

fn verified(src: &str) -> dmn_lite_types::compiled::VerifiedDecision {
    let cat = int_cat();
    compile_and_verify(parse(src).expect("parse"), &cat, src).expect("compile_and_verify")
}

fn eval_x(d: &dmn_lite_types::compiled::VerifiedDecision, x: i64) -> TypedValue {
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(x));
    let out = vm::evaluate(d, &b.build(), "").expect("evaluate");
    out.output.get(FieldId(0)).clone()
}

// ── Schema validation ─────────────────────────────────────────────────────────

/// Input built against a different schema produces SchemaHashMismatch.
#[test]
fn schema_hash_mismatch_returns_error() {
    let d1 = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when (*) :then ((y = 1)))))"#,
    );
    let d2 = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((z :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when (*) :then ((y = 1)))))"#,
    );
    let ctx = TypedInputContextBuilder::new(&d2.as_compiled().input_schema).build();
    let err = vm::evaluate(&d1, &ctx, "").unwrap_err();
    assert_eq!(err, EvalError::SchemaHashMismatch);
}

// ── Comparison predicates ─────────────────────────────────────────────────────

#[test]
fn comparison_eq_match() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 100)))
                  (rule r999 :when (*) :then ((y = -1)))))"#,
    );
    assert_eq!(eval_x(&d, 5), TypedValue::Integer(100));
}

#[test]
fn comparison_eq_no_match_falls_through() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 100)))
                  (rule r999 :when (*) :then ((y = -1)))))"#,
    );
    assert_eq!(eval_x(&d, 6), TypedValue::Integer(-1));
}

#[test]
fn comparison_lt_match_below_bound() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x < 10)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 9), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 10), TypedValue::Integer(0));
}

#[test]
fn comparison_le_match_at_boundary() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x <= 10)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 10), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 11), TypedValue::Integer(0));
}

#[test]
fn comparison_gt_match_above_bound() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x > 10)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 11), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 10), TypedValue::Integer(0));
}

#[test]
fn comparison_ge_match_at_boundary() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x >= 10)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 10), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 9), TypedValue::Integer(0));
}

#[test]
fn comparison_not_eq_match() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x != 5)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 6), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 5), TypedValue::Integer(0));
}

// ── InSet predicate ───────────────────────────────────────────────────────────

#[test]
fn in_set_member_matches() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in (1 2 3))) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 1), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 2), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 3), TypedValue::Integer(1));
}

#[test]
fn in_set_non_member_no_match() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in (1 2 3))) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 4), TypedValue::Integer(0));
    assert_eq!(eval_x(&d, 0), TypedValue::Integer(0));
}

// ── Range predicate ───────────────────────────────────────────────────────────

#[test]
fn range_inclusive_match_inside_and_boundary() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in [5 .. 10])) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 5), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 7), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 10), TypedValue::Integer(1));
}

#[test]
fn range_inclusive_no_match_outside() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in [5 .. 10])) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 4), TypedValue::Integer(0));
    assert_eq!(eval_x(&d, 11), TypedValue::Integer(0));
}

#[test]
fn range_exclusive_no_match_at_boundary() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in (5 .. 10))) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 5), TypedValue::Integer(0));
    assert_eq!(eval_x(&d, 10), TypedValue::Integer(0));
    assert_eq!(eval_x(&d, 6), TypedValue::Integer(1));
}

// ── IsNull / IsNotNull ────────────────────────────────────────────────────────

#[test]
fn is_null_matches_null_input() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x is-null)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set_null(FieldId(0));
    let out = vm::evaluate(&d, &b.build(), "").unwrap();
    assert_eq!(out.output.get(FieldId(0)), &TypedValue::Integer(1));
}

#[test]
fn is_not_null_matches_present_value() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x is-not-null)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 42), TypedValue::Integer(1));
}

// ── Not predicate ─────────────────────────────────────────────────────────────

#[test]
fn not_predicate_inverts() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((not (x = 5))) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 6), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 5), TypedValue::Integer(0));
}

// ── And predicate ─────────────────────────────────────────────────────────────

#[test]
fn and_both_true_matches() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((and (x > 0) (x < 10))) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 5), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 0), TypedValue::Integer(0));
    assert_eq!(eval_x(&d, 10), TypedValue::Integer(0));
}

// ── Or predicate ──────────────────────────────────────────────────────────────

#[test]
fn or_one_true_matches() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((or (x = 1) (x = 99))) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 1), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 99), TypedValue::Integer(1));
    assert_eq!(eval_x(&d, 5), TypedValue::Integer(0));
}

// ── FIRST hit policy ──────────────────────────────────────────────────────────

#[test]
fn first_no_match_returns_error() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 99)) :then ((y = 1)))))"#,
    );
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(1));
    let err = vm::evaluate(&d, &b.build(), "").unwrap_err();
    assert_eq!(err, EvalError::NoMatch);
}

#[test]
fn first_catch_all_always_matches() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r999 :when (*) :then ((y = 42)))))"#,
    );
    assert_eq!(eval_x(&d, 0), TypedValue::Integer(42));
    assert_eq!(eval_x(&d, -100), TypedValue::Integer(42));
}

// ── UNIQUE hit policy ─────────────────────────────────────────────────────────

#[test]
fn unique_single_match_succeeds() {
    let d = verified(
        r#"(define-decision t :hit-policy unique
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 5)))
                  (rule r002 :when ((x = 6)) :then ((y = 6)))))"#,
    );
    assert_eq!(eval_x(&d, 5), TypedValue::Integer(5));
    assert_eq!(eval_x(&d, 6), TypedValue::Integer(6));
}

#[test]
fn unique_no_match_returns_error() {
    let d = verified(
        r#"(define-decision t :hit-policy unique
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 5)))))"#,
    );
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(1));
    assert_eq!(
        vm::evaluate(&d, &b.build(), "").unwrap_err(),
        EvalError::NoMatch
    );
}

#[test]
fn unique_multiple_matches_returns_error() {
    let d = verified(
        r#"(define-decision t :hit-policy unique
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x > 0)) :then ((y = 1)))
                  (rule r002 :when ((x > 0)) :then ((y = 2)))))"#,
    );
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(5));
    let err = vm::evaluate(&d, &b.build(), "").unwrap_err();
    assert!(matches!(
        err,
        EvalError::MultipleMatches { rules } if rules == vec![RuleId(0), RuleId(1)]
    ));
}

// ── Output assignment ─────────────────────────────────────────────────────────

#[test]
fn correct_output_from_matched_rule() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 111)))
                  (rule r002 :when ((x = 2)) :then ((y = 222)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    assert_eq!(eval_x(&d, 1), TypedValue::Integer(111));
    assert_eq!(eval_x(&d, 2), TypedValue::Integer(222));
    assert_eq!(eval_x(&d, 7), TypedValue::Integer(0));
}

// ── Trace outcome ─────────────────────────────────────────────────────────────

#[test]
fn trace_outcome_match_carries_correct_rule_id() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 100)))
                  (rule r999 :when (*) :then ((y = -1)))))"#,
    );
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(5));
    let out = vm::evaluate(&d, &b.build(), "").unwrap();
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(0) }
    );
}

#[test]
fn trace_outcome_no_match_on_miss() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 99)) :then ((y = 1)))))"#,
    );
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(1));
    assert_eq!(
        vm::evaluate(&d, &b.build(), "").unwrap_err(),
        EvalError::NoMatch
    );
}

// ── Null semantics regression — Phase 1.5a ────────────────────────────────────

/// NotEq with a null/missing field must return false, not true.
///
/// Two-valued null semantics (semantics.md §3.2): any comparison whose field
/// is null produces `false`, including `!=`. Before the fix, the VM computed
/// `!values_equal(Null, X) = !false = true`, making the predicate incorrectly
/// pass. Found by the Phase 1.5 differential harness.
#[test]
fn not_eq_null_field_returns_false() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x != 5)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    // With x present and != 5, r001 should match.
    assert_eq!(eval_x(&d, 6), TypedValue::Integer(1));
    // With x missing (null), `null != 5` must be false → r001 fails → r999 catches.
    let ctx = TypedInputContextBuilder::new(&d.as_compiled().input_schema).build();
    let out = vm::evaluate(&d, &ctx, "").expect("r999 must match");
    assert_eq!(
        out.output.get(FieldId(0)),
        &TypedValue::Integer(0),
        "null != 5 must be false (two-valued null semantics)"
    );
}

/// NotEq with an explicitly-null field also returns false.
#[test]
fn not_eq_explicit_null_field_returns_false() {
    let d = verified(
        r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x != 99)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#,
    );
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set_null(FieldId(0));
    let out = vm::evaluate(&d, &b.build(), "").expect("r999 must match");
    assert_eq!(
        out.output.get(FieldId(0)),
        &TypedValue::Integer(0),
        "explicit null != 99 must be false"
    );
}
