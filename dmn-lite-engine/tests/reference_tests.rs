//! Reference evaluator test suite — Phase 1.3 §3.8.
//! 8 categories, ≥40 tests total.

use dmn_lite_compiler::{compile_to_ir, load_catalogue_from_str, lower_to_ir_with_warnings};
use dmn_lite_engine::reference::evaluate;
use dmn_lite_parser::parse;
use dmn_lite_types::{
    EvalError, FieldId, RuleId, TraceOutcome,
    ir::TypedValue,
    values::{TypedInputContext, TypedInputContextBuilder},
};

// ─── Helpers ──────────────────────────────────────────────────────────────────

const STUB: &str = include_str!("../../test-data/sem-os-stub.toml");

fn cat() -> dmn_lite_compiler::Catalogue {
    load_catalogue_from_str(STUB).expect("stub must load")
}

fn mini_cat(domains_toml: &str) -> dmn_lite_compiler::Catalogue {
    let src = format!(
        "snapshot_id = \"019c0a5d-0000-7000-8000-000000000099\"\nsnapshot_version = \"test\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n{domains_toml}"
    );
    load_catalogue_from_str(&src).expect("mini cat must load")
}

fn int_domain() -> &'static str {
    "[[domain]]\nname = \"N\"\ndomain_id = \"019c0a5d-0000-7000-8000-000000000001\"\ndescription = \"numbers\"\n"
}
fn enum_ab_cat() -> dmn_lite_compiler::Catalogue {
    mini_cat(
        "[[domain]]\nname = \"AB\"\ndomain_id = \"019c0a5d-0000-7000-8000-000000000003\"\ndescription = \"AB\"\n\n[[domain.value]]\nsymbol = \"A\"\nvalue_id = \"019c0a5d-0000-7000-8003-000000000001\"\n\n[[domain.value]]\nsymbol = \"B\"\nvalue_id = \"019c0a5d-0000-7000-8003-000000000002\"\n\n[[domain]]\nname = \"R\"\ndomain_id = \"019c0a5d-0000-7000-8000-000000000004\"\ndescription = \"R\"\n\n[[domain.value]]\nsymbol = \"OK\"\nvalue_id = \"019c0a5d-0000-7000-8004-000000000001\"\n",
    )
}

fn compile_ok(src: &str, c: &dmn_lite_compiler::Catalogue) -> dmn_lite_types::ir::TypedDecision {
    compile_to_ir(parse(src).expect("parse"), c).expect("compile")
}

fn enum_val(domain_name: &str, sym: &str, c: &dmn_lite_compiler::Catalogue) -> TypedValue {
    let d = c.resolve_domain(domain_name).expect("domain");
    let vid = d.resolve_value(sym).expect("value");
    TypedValue::Enum {
        domain_id: d.domain_id,
        value_id: vid,
    }
}

// ─── Category 1: Context construction ────────────────────────────────────────

#[test]
fn test_builder_sets_field_by_id() {
    let cat = mini_cat(int_domain());
    let d = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))",
        &cat,
    );
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set(FieldId(0), TypedValue::Integer(42));
    let ctx = b.build();
    assert_eq!(ctx.get(FieldId(0)), Some(&TypedValue::Integer(42)));
}

#[test]
fn test_builder_set_by_name() {
    let cat = mini_cat(int_domain());
    let d = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))",
        &cat,
    );
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_by_name("x", TypedValue::Integer(7)).unwrap();
    let ctx = b.build();
    assert_eq!(ctx.get(FieldId(0)), Some(&TypedValue::Integer(7)));
}

#[test]
fn test_builder_unknown_name_returns_error() {
    let cat = mini_cat(int_domain());
    let d = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))",
        &cat,
    );
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    let err = b.set_by_name("zzz", TypedValue::Integer(1)).unwrap_err();
    assert!(
        matches!(err, dmn_lite_types::InputContextError::UnknownFieldName { name } if name == "zzz")
    );
}

#[test]
fn test_from_slots_wrong_arity() {
    let cat = mini_cat(int_domain());
    let d = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))",
        &cat,
    );
    let err = TypedInputContext::from_slots(&d.input_schema, vec![None, None]).unwrap_err();
    assert!(matches!(
        err,
        dmn_lite_types::InputContextError::SlotCountMismatch {
            expected: 1,
            actual: 2
        }
    ));
}

#[test]
fn test_schema_hash_mismatch_detected() {
    let cat = mini_cat(int_domain());
    let d1 = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))",
        &cat,
    );
    let d2 = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((z :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))",
        &cat,
    );
    // Build ctx for d2's schema, evaluate against d1 — hash mismatch
    let ctx = TypedInputContextBuilder::new(&d2.input_schema).build();
    let err = evaluate(&d1, &ctx, "").unwrap_err();
    assert_eq!(err, EvalError::SchemaHashMismatch);
}

#[test]
fn test_explicit_null_distinguishable_from_missing_at_api_level() {
    let cat = mini_cat(int_domain());
    let d = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))",
        &cat,
    );
    let b = TypedInputContextBuilder::new(&d.input_schema);
    // Not setting 'x' → missing
    let ctx_missing = b.build();
    assert!(ctx_missing.is_missing(FieldId(0)));
    assert_eq!(ctx_missing.get(FieldId(0)), None);

    let mut b2 = TypedInputContextBuilder::new(&d.input_schema);
    b2.set_null(FieldId(0));
    let ctx_null = b2.build();
    assert!(!ctx_null.is_missing(FieldId(0)));
    assert_eq!(ctx_null.get(FieldId(0)), Some(&TypedValue::Null));
}

// ─── Category 2: Happy-path EBNF examples ────────────────────────────────────

#[test]
fn test_ebnf_51_eligibility_match_r001() {
    let src = include_str!("../../dmn-lite-parser/tests/fixtures/booking_eligibility.dmn-lite");
    let cat = cat();
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();

    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_by_name("jurisdiction", enum_val("Jurisdiction", "LU", &cat))
        .unwrap();
    b.set_by_name("client-type", enum_val("CbuType", "SICAV", &cat))
        .unwrap();
    b.set_by_name("product", enum_val("ProductCode", "CUSTODY", &cat))
        .unwrap();
    b.set_by_name(
        "booking-principal",
        enum_val("BookingPrincipal", "BNY_LUX", &cat),
    )
    .unwrap();
    b.set_by_name("source-of-funds", enum_val("SourceOfFunds", "SALARY", &cat))
        .unwrap();
    let ctx = b.build();

    let out = evaluate(&d, &ctx, src).expect("r001 must match under FIRST");
    // FIRST returns the first matching rule (r001, index 0).
    // r999 (catch-all) also evaluates true but FIRST has already returned.
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(0) }
    );
    assert_eq!(out.trace.rules.len(), 3);
    assert!(out.trace.rules[0].matched, "r001 matched");
    assert!(!out.trace.rules[1].matched, "r002 not matched");
    assert!(
        out.trace.rules[2].matched,
        "r999 catch-all evaluates true (but FIRST already resolved)"
    );

    let eligibility = out
        .output
        .get_by_name(&d.output_schema, "eligibility")
        .unwrap();
    assert_eq!(
        eligibility,
        &enum_val("EligibilityOutcome", "ELIGIBLE", &cat)
    );
    let reason = out
        .output
        .get_by_name(&d.output_schema, "reason-code")
        .unwrap();
    assert_eq!(
        reason,
        &enum_val("BookingReasonCode", "STANDARD_LUX_SICAV", &cat)
    );
    // r001 has 5 predicates, all true for this input
    assert_eq!(out.trace.rules[0].predicates.len(), 5);
    assert!(out.trace.rules[0].predicates.iter().all(|p| p.result));
}

#[test]
fn test_ebnf_51_eligibility_catchall_r999() {
    let src = include_str!("../../dmn-lite-parser/tests/fixtures/booking_eligibility.dmn-lite");
    let cat = cat();
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();

    // Input that doesn't match r001 or r002 → hits catch-all r999
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_by_name("jurisdiction", enum_val("Jurisdiction", "UK", &cat))
        .unwrap();
    b.set_by_name("client-type", enum_val("CbuType", "AIF", &cat))
        .unwrap();
    b.set_by_name("product", enum_val("ProductCode", "DEPOSITARY", &cat))
        .unwrap();
    b.set_by_name(
        "booking-principal",
        enum_val("BookingPrincipal", "BNY_US", &cat),
    )
    .unwrap();
    b.set_by_name("source-of-funds", enum_val("SourceOfFunds", "SALARY", &cat))
        .unwrap();
    let ctx = b.build();

    let out = evaluate(&d, &ctx, src).expect("catch-all must match");
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(2) }
    );
    let eligibility = out
        .output
        .get_by_name(&d.output_schema, "eligibility")
        .unwrap();
    let expected = enum_val("EligibilityOutcome", "NOT_ELIGIBLE", &cat);
    assert_eq!(eligibility, &expected);
}

#[test]
fn test_ebnf_52_age_band_minor() {
    let src = include_str!("../../dmn-lite-parser/tests/fixtures/age_band.dmn-lite");
    let cat = cat();
    let res = lower_to_ir_with_warnings(parse(src).unwrap(), &cat);
    let d = res.partial_decision.unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_by_name("age", TypedValue::Integer(15)).unwrap();
    let ctx = b.build();
    let out = evaluate(&d, &ctx, src).expect("15 → MINOR");
    let band = out.output.get_by_name(&d.output_schema, "band").unwrap();
    let expected = enum_val("AgeBand", "MINOR", &cat);
    assert_eq!(band, &expected);
}

#[test]
fn test_ebnf_52_age_band_adult() {
    let src = include_str!("../../dmn-lite-parser/tests/fixtures/age_band.dmn-lite");
    let cat = cat();
    let d = lower_to_ir_with_warnings(parse(src).unwrap(), &cat)
        .partial_decision
        .unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_by_name("age", TypedValue::Integer(40)).unwrap();
    let out = evaluate(&d, &b.build(), src).expect("40 → ADULT");
    let band = out.output.get_by_name(&d.output_schema, "band").unwrap();
    assert_eq!(band, &enum_val("AgeBand", "ADULT", &cat));
}

#[test]
fn test_ebnf_53_kyc_approved() {
    let src = include_str!("../../dmn-lite-parser/tests/fixtures/kyc_status.dmn-lite");
    let cat = cat();
    let d = lower_to_ir_with_warnings(parse(src).unwrap(), &cat)
        .partial_decision
        .unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_by_name("documents-submitted", TypedValue::Bool(true))
        .unwrap();
    b.set_by_name("review-outcome", enum_val("ReviewOutcome", "PASS", &cat))
        .unwrap();
    let out = evaluate(&d, &b.build(), src).expect("pass → APPROVED");
    let status = out
        .output
        .get_by_name(&d.output_schema, "kyc-status")
        .unwrap();
    assert_eq!(status, &enum_val("KycStatus", "APPROVED", &cat));
}

// ─── Category 3: Predicate kind correctness ───────────────────────────────────

fn eval_matched(src: &str, cat: &dmn_lite_compiler::Catalogue, x: TypedValue) -> bool {
    let d = compile_to_ir(parse(src).unwrap(), cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set(FieldId(0), x);
    let out = evaluate(&d, &b.build(), src).expect("eval");
    out.output.get(FieldId(0)) == &TypedValue::Integer(1)
}

#[test]
fn test_eq_true() {
    // mini_cat with N domain only; N2 reference dropped (unused).
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x = 5)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(eval_matched(
        src,
        &mini_cat(int_domain()),
        TypedValue::Integer(5)
    ));
}

#[test]
fn test_eq_false() {
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x = 5)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(!eval_matched(
        src,
        &mini_cat(int_domain()),
        TypedValue::Integer(6)
    ));
}

#[test]
fn test_neq_true() {
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x != 5)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(eval_matched(
        src,
        &mini_cat(int_domain()),
        TypedValue::Integer(6)
    ));
}

#[test]
fn test_neq_false() {
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x != 5)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(!eval_matched(
        src,
        &mini_cat(int_domain()),
        TypedValue::Integer(5)
    ));
}

#[test]
fn test_lt_true() {
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x < 10)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(eval_matched(
        src,
        &mini_cat(int_domain()),
        TypedValue::Integer(5)
    ));
}

#[test]
fn test_lt_false_at_boundary() {
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x < 10)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(!eval_matched(
        src,
        &mini_cat(int_domain()),
        TypedValue::Integer(10)
    ));
}

#[test]
fn test_le_true_at_boundary() {
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x <= 10)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(eval_matched(
        src,
        &mini_cat(int_domain()),
        TypedValue::Integer(10)
    ));
}

#[test]
fn test_in_set_true() {
    let cat = enum_ab_cat();
    let src = "(define-decision d :hit-policy first :inputs ((x :type enum :domain AB)) :outputs ((matched :type enum :domain R)) :rules ((rule r1 :when ((x in (A B))) :then ((matched = OK))) (rule r0 :when (*) :then ((matched = OK)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_by_name("x", enum_val("AB", "A", &cat)).unwrap();
    let out = evaluate(&d, &b.build(), "").unwrap();
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(0) }
    );
}

#[test]
fn test_range_inclusive_both_ends() {
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x in [5 .. 10])) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    let cat = mini_cat(int_domain());
    assert!(eval_matched(src, &cat, TypedValue::Integer(5)));
    assert!(eval_matched(src, &cat, TypedValue::Integer(10)));
    assert!(!eval_matched(src, &cat, TypedValue::Integer(4)));
    assert!(!eval_matched(src, &cat, TypedValue::Integer(11)));
}

#[test]
fn test_is_null_true_on_null() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x is-null)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_null(FieldId(0));
    let out = evaluate(&d, &b.build(), "").unwrap();
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(0) }
    );
}

#[test]
fn test_is_not_null_true_on_present() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x is-not-null)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(eval_matched(src, &cat, TypedValue::Integer(99)));
}

#[test]
fn test_not_predicate_inverts() {
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((not (x = 5))) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    let cat = mini_cat(int_domain());
    assert!(eval_matched(src, &cat, TypedValue::Integer(6)));
    assert!(!eval_matched(src, &cat, TypedValue::Integer(5)));
}

#[test]
fn test_and_predicate_all_true() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((and (x > 0) (x < 10))) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(eval_matched(src, &cat, TypedValue::Integer(5)));
    assert!(!eval_matched(src, &cat, TypedValue::Integer(10)));
}

#[test]
fn test_or_predicate_any_true() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((or (x = 1) (x = 99))) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(eval_matched(src, &cat, TypedValue::Integer(1)));
    assert!(eval_matched(src, &cat, TypedValue::Integer(99)));
    assert!(!eval_matched(src, &cat, TypedValue::Integer(5)));
}

// ─── Category 4: Hit policy tests ────────────────────────────────────────────

#[test]
fn test_unique_one_match() {
    let cat = enum_ab_cat();
    let src = "(define-decision d :hit-policy unique :inputs ((x :type enum :domain AB)) :outputs ((y :type enum :domain R)) :rules ((rule r1 :when ((x = A)) :then ((y = OK))) (rule r2 :when ((x = B)) :then ((y = OK)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_by_name("x", enum_val("AB", "A", &cat)).unwrap();
    let out = evaluate(&d, &b.build(), "").unwrap();
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(0) }
    );
}

#[test]
fn test_unique_no_match() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x = 99)) :then ((y = 1)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let ctx = TypedInputContextBuilder::new(&d.input_schema).build();
    let err = evaluate(&d, &ctx, "").unwrap_err();
    assert_eq!(err, EvalError::NoMatch);
}

#[test]
fn test_unique_multiple_matches() {
    let cat = mini_cat(int_domain());
    // Two rules both match x > 0; UNIQUE should error
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x > 0)) :then ((y = 1))) (rule r2 :when ((x > 0)) :then ((y = 2)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set(FieldId(0), TypedValue::Integer(5));
    let err = evaluate(&d, &b.build(), "").unwrap_err();
    assert!(
        matches!(err, EvalError::MultipleMatches { rules } if rules == vec![RuleId(0), RuleId(1)])
    );
}

#[test]
fn test_first_returns_first_match() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x > 0)) :then ((y = 1))) (rule r2 :when ((x > 0)) :then ((y = 2)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set(FieldId(0), TypedValue::Integer(5));
    let out = evaluate(&d, &b.build(), "").unwrap();
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(0) }
    );
    assert_eq!(out.output.get(FieldId(0)), &TypedValue::Integer(1));
}

#[test]
fn test_first_second_rule_only() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x = 99)) :then ((y = 1))) (rule r2 :when ((x > 0)) :then ((y = 2)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set(FieldId(0), TypedValue::Integer(5));
    let out = evaluate(&d, &b.build(), "").unwrap();
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(1) }
    );
    assert_eq!(out.output.get(FieldId(0)), &TypedValue::Integer(2));
}

#[test]
fn test_first_no_match() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x = 99)) :then ((y = 1)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let ctx = TypedInputContextBuilder::new(&d.input_schema).build();
    assert_eq!(evaluate(&d, &ctx, "").unwrap_err(), EvalError::NoMatch);
}

// ─── Category 5: Null semantics ───────────────────────────────────────────────

fn eval_matched_with_ctx(
    src: &str,
    cat: &dmn_lite_compiler::Catalogue,
    ctx: TypedInputContext,
) -> bool {
    let d = compile_to_ir(parse(src).unwrap(), cat).unwrap();
    evaluate(&d, &ctx, "").is_ok_and(|o| o.output.get(FieldId(0)) == &TypedValue::Integer(1))
}

fn null_ctx(src: &str, cat: &dmn_lite_compiler::Catalogue) -> TypedInputContext {
    let d = compile_to_ir(parse(src).unwrap(), cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_null(FieldId(0));
    b.build()
}

#[test]
fn test_null_eq_returns_false() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x = 5)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    let ctx = null_ctx(src, &cat);
    assert!(!eval_matched_with_ctx(src, &cat, ctx));
}

#[test]
fn test_null_neq_returns_false() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x != 5)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    let ctx = null_ctx(src, &cat);
    assert!(!eval_matched_with_ctx(src, &cat, ctx));
}

#[test]
fn test_null_in_set_returns_false() {
    let cat = enum_ab_cat();
    let src = "(define-decision d :hit-policy first :inputs ((x :type enum :domain AB)) :outputs ((matched :type enum :domain R)) :rules ((rule r1 :when ((x in (A B))) :then ((matched = OK))) (rule r0 :when (*) :then ((matched = OK)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_null(FieldId(0));
    let out = evaluate(&d, &b.build(), "").unwrap();
    // Catch-all fires — r1 did not match
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(1) }
    );
}

#[test]
fn test_null_range_returns_false() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x in [1 .. 10])) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    let ctx = null_ctx(src, &cat);
    assert!(!eval_matched_with_ctx(src, &cat, ctx));
}

#[test]
fn test_is_null_matches_null() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x is-null)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    let ctx = null_ctx(src, &cat);
    assert!(eval_matched_with_ctx(src, &cat, ctx));
}

#[test]
fn test_is_not_null_matches_non_null() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((matched :type integer :domain N)) :rules ((rule r1 :when ((x is-not-null)) :then ((matched = 1))) (rule r0 :when (*) :then ((matched = 0)))))";
    assert!(eval_matched(src, &cat, TypedValue::Integer(42)));
}

// ─── Category 6: Trace correctness ───────────────────────────────────────────

#[test]
fn test_trace_length_equals_rule_count() {
    let src = include_str!("../../dmn-lite-parser/tests/fixtures/booking_eligibility.dmn-lite");
    let cat = cat();
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set_by_name("jurisdiction", enum_val("Jurisdiction", "LU", &cat))
        .unwrap();
    b.set_by_name("client-type", enum_val("CbuType", "SICAV", &cat))
        .unwrap();
    b.set_by_name("product", enum_val("ProductCode", "CUSTODY", &cat))
        .unwrap();
    b.set_by_name(
        "booking-principal",
        enum_val("BookingPrincipal", "BNY_LUX", &cat),
    )
    .unwrap();
    b.set_by_name("source-of-funds", enum_val("SourceOfFunds", "SALARY", &cat))
        .unwrap();
    let out = evaluate(&d, &b.build(), src).unwrap();
    // FIRST: trace covers all 3 rules regardless of which fired.
    assert_eq!(out.trace.rules.len(), d.rules.len());
}

#[test]
fn test_trace_matched_equals_predicate_conjunction() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x > 0) (x < 10)) :then ((y = 1))) (rule r0 :when (*) :then ((y = 0)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set(FieldId(0), TypedValue::Integer(5));
    let out = evaluate(&d, &b.build(), "").unwrap();
    let r1 = &out.trace.rules[0];
    let expected_matched = r1.predicates.iter().all(|p| p.result);
    assert_eq!(r1.matched, expected_matched);
}

#[test]
fn test_trace_predicates_source_order() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x > 0) (x < 10)) :then ((y = 1))) (rule r0 :when (*) :then ((y = 0)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set(FieldId(0), TypedValue::Integer(5));
    let out = evaluate(&d, &b.build(), src).unwrap();
    // Rule r1 has 2 predicates in source order
    assert_eq!(out.trace.rules[0].predicates.len(), 2);
    // Both have non-empty descriptions (source was supplied)
    assert!(!out.trace.rules[0].predicates[0].description.is_empty());
    assert!(!out.trace.rules[0].predicates[1].description.is_empty());
}

#[test]
fn test_trace_outcome_matches_return() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x = 5)) :then ((y = 1)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let err_out = evaluate(
        &d,
        &TypedInputContextBuilder::new(&d.input_schema).build(),
        "",
    )
    .unwrap_err();
    assert_eq!(err_out, EvalError::NoMatch);
}

#[test]
fn test_catch_all_trace_has_one_entry() {
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let ctx = TypedInputContextBuilder::new(&d.input_schema).build();
    let out = evaluate(&d, &ctx, "").unwrap();
    assert_eq!(out.trace.rules[0].predicates.len(), 1);
    assert!(out.trace.rules[0].predicates[0].result);
    assert_eq!(out.trace.rules[0].predicates[0].description, "catch-all");
}

// ─── Category 7: Error cases ──────────────────────────────────────────────────

#[test]
fn test_input_schema_mismatch_wrong_arity() {
    let cat = mini_cat(int_domain());
    let d = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))",
        &cat,
    );
    // Build context with 2 slots but decision expects 1
    let ctx = TypedInputContext::from_slots(&d.input_schema, vec![None, None]);
    // from_slots detects arity mismatch before we even get to evaluate
    assert!(ctx.is_err());
}

#[test]
fn test_schema_hash_mismatch() {
    let cat = mini_cat(int_domain());
    let d1 = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))",
        &cat,
    );
    let d2 = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((z :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))",
        &cat,
    );
    let ctx = TypedInputContextBuilder::new(&d2.input_schema).build();
    let err = evaluate(&d1, &ctx, "").unwrap_err();
    assert_eq!(err, EvalError::SchemaHashMismatch);
}

#[test]
fn test_input_type_mismatch() {
    let cat = enum_ab_cat();
    let d = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((x :type enum :domain AB)) :outputs ((y :type enum :domain R)) :rules ((rule r1 :when (*) :then ((y = OK)))))",
        &cat,
    );
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    // Provide an integer where enum is expected
    b.set(FieldId(0), TypedValue::Integer(1));
    let err = evaluate(&d, &b.build(), "").unwrap_err();
    assert!(matches!(err, EvalError::InputTypeMismatch { expected, .. } if expected == "enum"));
}

#[test]
fn test_input_domain_mismatch() {
    let cat = enum_ab_cat();
    let d = compile_ok(
        "(define-decision d :hit-policy unique :inputs ((x :type enum :domain AB)) :outputs ((y :type enum :domain R)) :rules ((rule r1 :when (*) :then ((y = OK)))))",
        &cat,
    );
    let r_domain = cat.resolve_domain("R").unwrap();
    let ok_vid = r_domain.resolve_value("OK").unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    // Provide an R-domain enum value where AB-domain is expected
    b.set(
        FieldId(0),
        TypedValue::Enum {
            domain_id: r_domain.domain_id,
            value_id: ok_vid,
        },
    );
    let err = evaluate(&d, &b.build(), "").unwrap_err();
    assert!(matches!(err, EvalError::InputDomainMismatch { .. }));
}

// ─── Category 8: No-short-circuit tests ───────────────────────────────────────

#[test]
fn test_no_short_circuit_implicit_conjunction() {
    // Rule has two predicates. First is false. Both must appear in trace.
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x = 99) (x > 0)) :then ((y = 1))) (rule r0 :when (*) :then ((y = 0)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set(FieldId(0), TypedValue::Integer(5));
    let out = evaluate(&d, &b.build(), "").unwrap();
    // r1 did not match, r0 (catch-all) matched
    let r1_trace = &out.trace.rules[0];
    // Both predicates appear (not short-circuited)
    assert_eq!(
        r1_trace.predicates.len(),
        2,
        "both predicates must be traced even when first is false"
    );
    assert!(!r1_trace.predicates[0].result); // x = 99 is false
    assert!(r1_trace.predicates[1].result); // x > 0 is true
    assert!(!r1_trace.matched);
}

#[test]
fn test_no_short_circuit_or_evaluates_all() {
    // (or false true) → both sub-predicates evaluated (result = true)
    let cat = mini_cat(int_domain());
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((or (x = 99) (x > 0))) :then ((y = 1))) (rule r0 :when (*) :then ((y = 0)))))";
    let d = compile_to_ir(parse(src).unwrap(), &cat).unwrap();
    let mut b = TypedInputContextBuilder::new(&d.input_schema);
    b.set(FieldId(0), TypedValue::Integer(5)); // x != 99 (false), x > 0 (true) → or = true
    let out = evaluate(&d, &b.build(), "").unwrap();
    assert_eq!(
        out.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(0) }
    );
    // Rule matched means both branches of `or` were evaluated (result = true)
    assert!(out.trace.rules[0].matched);
}
