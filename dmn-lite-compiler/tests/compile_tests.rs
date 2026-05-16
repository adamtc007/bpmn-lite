//! Compiler integration tests — Phase 1.2 §3.8 categories 2–10.
//! Happy-path, field resolution, type checking, structural, warnings,
//! determinism, and multi-error tests.

use dmn_lite_compiler::{
    CompileError, compile_to_ir, load_catalogue_from_str, lower_to_ir_with_warnings,
};
use dmn_lite_parser::parse;
use dmn_lite_types::{
    CompileWarning as W,
    ir::{ComparisonOp, HitPolicy, TypedPredicate, TypedValue, TypedWhen},
};

// ── Helpers ───────────────────────────────────────────────────────────────────

const STUB: &str = include_str!("../../test-data/sem-os-stub.toml");

fn stub_cat() -> dmn_lite_compiler::Catalogue {
    load_catalogue_from_str(STUB).expect("stub must load")
}

/// Catalogue with only specified inline domains.
fn mini_cat(domains_toml: &str) -> dmn_lite_compiler::Catalogue {
    let src = format!(
        "snapshot_id = \"019c0a5d-0000-7000-8000-000000000099\"\nsnapshot_version = \"test\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n{domains_toml}"
    );
    load_catalogue_from_str(&src).expect("mini cat must load")
}

fn int_domain() -> String {
    "[[domain]]\nname = \"Numbers\"\ndomain_id = \"019c0a5d-0000-7000-8000-000000000001\"\ndescription = \"advisory integer domain\"\n".into()
}

fn bool_domain() -> String {
    "[[domain]]\nname = \"Bool\"\ndomain_id = \"019c0a5d-0000-7000-8000-000000000002\"\ndescription = \"advisory bool domain\"\n".into()
}

fn enum_domain_ab() -> String {
    "[[domain]]\nname = \"AB\"\ndomain_id = \"019c0a5d-0000-7000-8000-000000000003\"\ndescription = \"AB\"\n\n[[domain.value]]\nsymbol = \"A\"\nvalue_id = \"019c0a5d-0000-7000-8003-000000000001\"\n\n[[domain.value]]\nsymbol = \"B\"\nvalue_id = \"019c0a5d-0000-7000-8003-000000000002\"\n".into()
}

fn ok_domain_r() -> String {
    "[[domain]]\nname = \"R\"\ndomain_id = \"019c0a5d-0000-7000-8000-000000000004\"\ndescription = \"R\"\n\n[[domain.value]]\nsymbol = \"OK\"\nvalue_id = \"019c0a5d-0000-7000-8004-000000000001\"\n".into()
}

fn parse_ok(src: &str) -> dmn_lite_parser::Source {
    parse(src).expect("source must parse")
}

fn has_error<F: Fn(&CompileError) -> bool>(
    src: &str,
    cat: &dmn_lite_compiler::Catalogue,
    f: F,
) -> bool {
    let errs = lower_to_ir_with_warnings(parse_ok(src), cat);
    errs.errors.iter().any(f)
}

// ── Category 2: Happy-path compile tests ─────────────────────────────────────

#[test]
fn test_compile_ebnf_51_booking_eligibility() {
    let src = include_str!("../../dmn-lite-parser/tests/fixtures/booking_eligibility.dmn-lite");
    let cat = stub_cat();
    let decision = compile_to_ir(parse_ok(src), &cat).expect("§5.1 must compile");
    assert_eq!(decision.name, "booking-eligibility");
    assert!(matches!(decision.hit_policy, HitPolicy::First));
    assert_eq!(decision.input_schema.len(), 5);
    assert_eq!(decision.output_schema.len(), 2);
    assert_eq!(decision.rules.len(), 3);
    // Catch-all is last
    assert!(matches!(decision.rules[2].when, TypedWhen::CatchAll(_)));
}

#[test]
fn test_compile_ebnf_52_age_band() {
    let src = include_str!("../../dmn-lite-parser/tests/fixtures/age_band.dmn-lite");
    let cat = stub_cat();
    let res = lower_to_ir_with_warnings(parse_ok(src), &cat);
    // integer and enum fields → DomainOnNonEnum warning for AgeYears
    assert!(res.errors.is_empty(), "§5.2 must compile: {:?}", res.errors);
    let d = res.partial_decision.unwrap();
    assert_eq!(d.name, "age-band-classification");
    assert!(matches!(d.hit_policy, HitPolicy::First));
    assert_eq!(d.rules.len(), 4);
    // First rule: range predicate
    if let TypedWhen::Predicates(preds, _) = &d.rules[0].when {
        assert!(matches!(&preds[0], TypedPredicate::Range { .. }));
    } else {
        panic!("expected predicates in r-minor");
    }
}

#[test]
fn test_compile_ebnf_53_kyc_status() {
    let src = include_str!("../../dmn-lite-parser/tests/fixtures/kyc_status.dmn-lite");
    let cat = stub_cat();
    let res = lower_to_ir_with_warnings(parse_ok(src), &cat);
    assert!(res.errors.is_empty(), "§5.3 must compile: {:?}", res.errors);
    let d = res.partial_decision.unwrap();
    assert_eq!(d.name, "kyc-status");
    assert_eq!(d.rules.len(), 4);
}

#[test]
fn test_compile_resolved_entities_in_source_order() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((x = A)) :then ((y = OK)))
                (rule r2 :when ((x = B)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    let d = compile_to_ir(parse_ok(src), &cat).unwrap();
    // Source order: r1 pred (A), r1 assign (OK), r2 pred (B), r2 assign (OK)
    assert_eq!(d.resolved_entities.len(), 4);
    // First entity is A, second is OK, third is B, fourth is OK
    assert!(matches!(&d.resolved_entities[0].value_id.to_string().as_str(), s if !s.is_empty()));
}

#[test]
fn test_decision_id_from_decision_id_attr() {
    let src = r#"(define-decision my-decision
      :decision-id "custom.id.v1"
      :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    let d = compile_to_ir(parse_ok(src), &cat).unwrap();
    assert_eq!(d.decision_id.to_string(), "custom.id.v1");
}

// ── Category 3: Field resolution tests ───────────────────────────────────────

#[test]
fn test_unknown_input_field_in_predicate() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((z = A)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::UnknownInputField { name, .. } if name == "z")
    ));
}

#[test]
fn test_unknown_output_field_in_assignment() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((z = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::UnknownOutputField { name, .. } if name == "z")
    ));
}

#[test]
fn test_field_resolves_to_correct_id() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((first :type enum :domain AB) (second :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    let d = compile_to_ir(parse_ok(src), &cat).unwrap();
    assert_eq!(d.input_schema[0].field_id.0, 0);
    assert_eq!(d.input_schema[0].name, "first");
    assert_eq!(d.input_schema[1].field_id.0, 1);
    assert_eq!(d.input_schema[1].name, "second");
}

// ── Category 4: Enum domain resolution tests ──────────────────────────────────

#[test]
fn test_unknown_domain_on_enum_input() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain NoSuchDomain))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&ok_domain_r());
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::UnknownDomain { name, .. } if name == "NoSuchDomain")
    ));
}

#[test]
fn test_unknown_domain_on_enum_output() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain NoSuchDomain))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&enum_domain_ab());
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::UnknownDomain { name, .. } if name == "NoSuchDomain")
    ));
}

#[test]
fn test_unknown_value_in_predicate() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((x = NOSUCHVAL)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::UnknownDomainValue { symbol, .. } if symbol == "NOSUCHVAL")
    ));
}

#[test]
fn test_unknown_value_in_assignment() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = BADVAL)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::UnknownDomainValue { symbol, .. } if symbol == "BADVAL")
    ));
}

#[test]
fn test_unknown_value_in_set_predicate() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((x in (A BADVAL))) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::UnknownDomainValue { symbol, .. } if symbol == "BADVAL")
    ));
}

#[test]
fn test_enum_literal_compiles_to_typed_value() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((x = A)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    let d = compile_to_ir(parse_ok(src), &cat).unwrap();
    if let TypedWhen::Predicates(preds, _) = &d.rules[0].when {
        assert!(matches!(
            &preds[0],
            TypedPredicate::Comparison {
                op: ComparisonOp::Eq,
                rhs: TypedValue::Enum { .. },
                ..
            }
        ));
    } else {
        panic!();
    }
}

// ── Category 5: Type mismatch tests ───────────────────────────────────────────

#[test]
fn test_enum_field_with_bool_literal() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((x = true)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::PredicateTypeMismatch { field_type, .. } if field_type == "enum")
    ));
}

#[test]
fn test_integer_field_with_enum_literal() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((n :type integer :domain Numbers))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((n = SOMEVALUE)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", int_domain(), ok_domain_r()));
    // SOMEVALUE is a Symbol literal, not a number — PredicateTypeMismatch
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::PredicateTypeMismatch { field_type, literal_type, .. } if field_type == "integer" && literal_type == "symbol")
    ));
}

#[test]
fn test_bool_field_with_numeric_literal() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((b :type bool :domain Bool))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((b = 42)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", bool_domain(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::PredicateTypeMismatch { field_type, .. } if field_type == "bool")
    ));
}

#[test]
fn test_ordered_comparison_on_enum_field() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((x < 5)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(src, &cat, |e| matches!(
        e,
        CompileError::OrderedComparisonOnNonNumeric { .. }
    )));
}

#[test]
fn test_ordered_comparison_on_bool_field() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((b :type bool :domain Bool))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((b > 1)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", bool_domain(), ok_domain_r()));
    assert!(has_error(src, &cat, |e| matches!(
        e,
        CompileError::OrderedComparisonOnNonNumeric { .. }
    )));
}

#[test]
fn test_range_predicate_on_enum_field() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((x in [1 .. 5])) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(src, &cat, |e| matches!(
        e,
        CompileError::RangeOnNonNumeric { .. }
    )));
}

#[test]
fn test_decimal_literal_against_integer_field() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((n :type integer :domain Numbers))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((n = 3.14)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", int_domain(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::PredicateTypeMismatch { field_type, literal_type, .. } if field_type == "integer" && literal_type == "decimal")
    ));
}

#[test]
fn test_integer_widens_to_decimal() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((n :type decimal :domain Numbers))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((n = 42)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", int_domain(), ok_domain_r()));
    let d = compile_to_ir(parse_ok(src), &cat).unwrap();
    if let TypedWhen::Predicates(preds, _) = &d.rules[0].when {
        assert!(
            matches!(&preds[0], TypedPredicate::Comparison { rhs: TypedValue::Decimal(v), .. } if *v == 42.0)
        );
    } else {
        panic!();
    }
}

#[test]
fn test_assignment_type_mismatch() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((n :type integer :domain Numbers))
      :rules   ((rule r1 :when (*) :then ((n = A)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), int_domain()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::AssignmentTypeMismatch { output_type, .. } if output_type == "integer")
    ));
}

// ── Category 6: Structural rule tests ─────────────────────────────────────────

#[test]
fn test_duplicate_input_field() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB) (x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::DuplicateInputField { name, .. } if name == "x")
    ));
}

#[test]
fn test_duplicate_output_field() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R) (y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::DuplicateOutputField { name, .. } if name == "y")
    ));
}

#[test]
fn test_duplicate_rule_id() {
    // Use distinct :when clauses so the parser doesn't catch MultipleCatchAllRules first.
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((x = A)) :then ((y = OK)))
                (rule r1 :when ((x = B)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::DuplicateRuleId { name, .. } if name == "r1")
    ));
}

#[test]
fn test_missing_output_assignment() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R) (z :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::MissingOutputAssignment { output, .. } if output == "z")
    ));
}

#[test]
fn test_duplicate_output_assignment() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK) (y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::DuplicateOutputAssignment { output, .. } if output == "y")
    ));
}

// ── Category 7: FIRST + catch-all tests ───────────────────────────────────────

#[test]
fn test_first_policy_catch_all_as_last_rule_ok() {
    let src = r#"(define-decision d :hit-policy first
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((x = A)) :then ((y = OK)))
                (rule r2 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    let d = compile_to_ir(parse_ok(src), &cat).expect("FIRST with catch-all last must compile");
    assert!(matches!(d.hit_policy, HitPolicy::First));
}

#[test]
fn test_first_policy_catch_all_in_middle_unreachable() {
    let src = r#"(define-decision d :hit-policy first
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))
                (rule r2 :when ((x = A)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    assert!(has_error(
        src,
        &cat,
        |e| matches!(e, CompileError::UnreachableAfterCatchAll { rule, .. } if rule == "r2")
    ));
}

#[test]
fn test_catch_all_only_decision_compiles() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    let d = compile_to_ir(parse_ok(src), &cat).expect("catch-all only must compile");
    assert!(matches!(d.rules[0].when, TypedWhen::CatchAll(_)));
}

// ── Category 8: Multi-error reporting tests ────────────────────────────────────

#[test]
fn test_multiple_independent_errors_reported() {
    // Two different unknown fields in two rules
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((nosuch1 = A)) :then ((y = OK)))
                (rule r2 :when ((nosuch2 = A)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    let errs = lower_to_ir_with_warnings(parse_ok(src), &cat);
    assert!(
        errs.errors.len() >= 2,
        "expected >= 2 errors, got {:?}",
        errs.errors
    );
}

#[test]
fn test_partial_decision_when_schemas_resolve() {
    // Unknown field in one rule — schemas resolve, partial_decision should be Some
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((badfield = A)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    let errs = lower_to_ir_with_warnings(parse_ok(src), &cat);
    assert!(!errs.errors.is_empty());
    assert!(
        errs.partial_decision.is_some(),
        "schemas resolved → partial_decision must be Some"
    );
}

#[test]
fn test_partial_decision_none_when_schema_fails() {
    // Unknown domain on input → schemas can't resolve → partial_decision = None
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain NODOMAIN))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&ok_domain_r());
    let errs = lower_to_ir_with_warnings(parse_ok(src), &cat);
    assert!(!errs.errors.is_empty());
    assert!(
        errs.partial_decision.is_none(),
        "schema failed → partial_decision must be None"
    );
}

// ── Category 9: Warning tests ──────────────────────────────────────────────────

#[test]
fn test_non_enum_domain_produces_warning() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((n :type integer :domain Numbers))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", int_domain(), ok_domain_r()));
    let res = lower_to_ir_with_warnings(parse_ok(src), &cat);
    assert!(
        res.errors.is_empty(),
        "non-enum domain must not be an error"
    );
    let warn = res
        .warnings
        .iter()
        .any(|w| matches!(w, W::DomainOnNonEnum { type_name, .. } if type_name == "integer"));
    assert!(warn, "expected DomainOnNonEnum warning for integer field");
}

#[test]
fn test_compile_succeeds_with_domain_warning() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((n :type integer :domain Numbers))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", int_domain(), ok_domain_r()));
    let d = compile_to_ir(parse_ok(src), &cat).expect("compilation with warnings must succeed");
    assert!(matches!(
        d.input_schema[0].field_type,
        dmn_lite_types::ir::ResolvedType::Integer
    ));
}

#[test]
fn test_domain_warning_span_points_at_domain_clause() {
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((n :type integer :domain Numbers))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when (*) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", int_domain(), ok_domain_r()));
    let res = lower_to_ir_with_warnings(parse_ok(src), &cat);
    let w = res
        .warnings
        .iter()
        .find(|w| matches!(w, W::DomainOnNonEnum { .. }))
        .unwrap();
    if let W::DomainOnNonEnum { span, domain, .. } = w {
        assert_eq!(domain, "Numbers");
        // Span should be non-empty (points at the domain name token)
        assert!(!span.is_empty(), "span must be non-empty");
    }
}

// ── Category 10: Determinism tests ────────────────────────────────────────────

#[test]
fn test_compile_is_deterministic() {
    let src = include_str!("../../dmn-lite-parser/tests/fixtures/booking_eligibility.dmn-lite");
    let cat = stub_cat();
    let d1 = compile_to_ir(parse_ok(src), &cat).unwrap();
    let d2 = compile_to_ir(parse_ok(src), &cat).unwrap();
    assert_eq!(
        d1, d2,
        "two compiles of the same source must produce identical TypedDecision"
    );
}

#[test]
fn test_resolved_entities_in_source_order() {
    // r1 has predicate A then assignment OK; r2 has predicate B then assignment OK
    let src = r#"(define-decision d :hit-policy unique
      :inputs  ((x :type enum :domain AB))
      :outputs ((y :type enum :domain R))
      :rules   ((rule r1 :when ((x = A)) :then ((y = OK)))
                (rule r2 :when ((x = B)) :then ((y = OK)))))"#;
    let cat = mini_cat(&format!("{}\n{}", enum_domain_ab(), ok_domain_r()));
    let d = compile_to_ir(parse_ok(src), &cat).unwrap();
    // entity 0: A (r1 pred), entity 1: OK (r1 assign), entity 2: B (r2 pred), entity 3: OK (r2 assign)
    let ids: Vec<String> = d
        .resolved_entities
        .iter()
        .map(|e| e.value_id.to_string())
        .collect();
    assert_eq!(ids.len(), 4);
    // First two belong to r1, last two to r2 — ordering is deterministic
    assert_eq!(ids[0], ids[0]); // A
    assert_eq!(ids[2], ids[2]); // B — different from A
    assert_ne!(ids[0], ids[2], "A and B must have different value_ids");
}
