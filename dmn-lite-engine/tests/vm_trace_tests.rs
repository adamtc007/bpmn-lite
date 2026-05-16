//! VM trace tests — Phase 1.4 §3.7 (trace contract).

use dmn_lite_compiler::{compile_and_verify, load_catalogue_from_str};
use dmn_lite_engine::vm;
use dmn_lite_parser::parse;
use dmn_lite_types::{
    FieldId, RuleId, TraceOutcome, ir::TypedValue, values::TypedInputContextBuilder,
};

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

// ── Matched-rule trace ────────────────────────────────────────────────────────

#[test]
fn matched_rule_has_true_predicates() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let d = verified(src);
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(5));
    let out = vm::evaluate(&d, &b.build(), src).unwrap();
    let r001 = &out.trace.rules[0];
    assert!(r001.matched);
    assert!(!r001.predicates.is_empty());
    assert!(r001.predicates.iter().all(|p| p.result));
}

#[test]
fn non_matched_rule_trace_marked_false() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let d = verified(src);
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(99));
    let out = vm::evaluate(&d, &b.build(), src).unwrap();
    let r001 = &out.trace.rules[0];
    assert!(!r001.matched);
    assert!(!r001.predicates.is_empty());
    assert!(!r001.predicates[0].result);
}

#[test]
fn first_skipped_rules_have_empty_predicate_trace() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r002 :when ((x = 2)) :then ((y = 2)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let d = verified(src);
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(1)); // matches r001, r002 and r999 skipped
    let out = vm::evaluate(&d, &b.build(), src).unwrap();
    assert_eq!(out.trace.rules.len(), 3);
    // r002 was never entered
    assert!(
        out.trace.rules[1].predicates.is_empty(),
        "r002 skipped — no predicates"
    );
}

#[test]
fn catch_all_produces_synthetic_catchall_predicate() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let d = verified(src);
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(99)); // misses r001, hits r999
    let out = vm::evaluate(&d, &b.build(), src).unwrap();
    let r999 = &out.trace.rules[1];
    assert!(r999.matched);
    assert_eq!(r999.predicates.len(), 1);
    assert!(r999.predicates[0].result);
    assert_eq!(r999.predicates[0].description, "catch-all");
}

#[test]
fn brfalse_exited_rule_not_matched() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let d = verified(src);
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(9)); // x=5 fails
    let out = vm::evaluate(&d, &b.build(), src).unwrap();
    let r001 = &out.trace.rules[0];
    assert!(!r001.matched);
    assert!(!r001.predicates.is_empty(), "failing predicate recorded");
}

// ── Trace completeness ────────────────────────────────────────────────────────

#[test]
fn trace_includes_all_rules() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r002 :when ((x = 2)) :then ((y = 2)))
                  (rule r003 :when ((x = 3)) :then ((y = 3)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let d = verified(src);
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(1));
    let out = vm::evaluate(&d, &b.build(), src).unwrap();
    assert_eq!(out.trace.rules.len(), 4);
}

#[test]
fn trace_rules_sorted_by_rule_id() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r002 :when ((x = 2)) :then ((y = 2)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let d = verified(src);
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(2));
    let out = vm::evaluate(&d, &b.build(), src).unwrap();
    let ids: Vec<usize> = out.trace.rules.iter().map(|r| r.rule_id.0).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "{:?}", ids);
}

#[test]
fn rule_names_in_trace_match_source() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule my-rule :when ((x = 1)) :then ((y = 1)))
                  (rule catch-all :when (*) :then ((y = 0)))))"#;
    let d = verified(src);
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(1));
    let out = vm::evaluate(&d, &b.build(), src).unwrap();
    assert_eq!(out.trace.rules[0].rule_name, "my-rule");
    assert_eq!(out.trace.rules[1].rule_name, "catch-all");
}

// ── Trace outcome ─────────────────────────────────────────────────────────────

#[test]
fn trace_outcome_match_correct_rule_id() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r002 :when ((x = 2)) :then ((y = 2)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let d = verified(src);
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(2));
    let out = vm::evaluate(&d, &b.build(), src).unwrap();
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(1) }
    );
}

// ── Predicate descriptions ────────────────────────────────────────────────────

#[test]
fn predicate_descriptions_non_empty_when_source_supplied() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let d = verified(src);
    let mut b = TypedInputContextBuilder::new(&d.as_compiled().input_schema);
    b.set(FieldId(0), TypedValue::Integer(99)); // triggers BrFalse on r001
    let out = vm::evaluate(&d, &b.build(), src).unwrap();
    let r001_preds = &out.trace.rules[0].predicates;
    assert!(!r001_preds.is_empty());
    assert!(
        !r001_preds[0].description.is_empty(),
        "non-empty description with source"
    );
}
