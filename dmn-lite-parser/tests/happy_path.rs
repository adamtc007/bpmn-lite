//! Category 1 — Happy-path tests: one test per EBNF §5 example.
//! Each example must parse successfully and produce the expected AST structure.

use dmn_lite_parser::{
    HitPolicyAst, LiteralAst, PredicateAst, RangeBound, TypeRefAst, WhenAst, parse,
};

// ── §5.1 booking-eligibility ──────────────────────────────────────────────────

#[test]
fn test_booking_eligibility_parses() {
    let src = include_str!("fixtures/booking_eligibility.dmn-lite");
    let ast = parse(src).expect("§5.1 booking-eligibility must parse");

    assert_eq!(ast.decisions.len(), 1);
    let d = &ast.decisions[0];
    assert_eq!(d.name.name, "booking-eligibility");
    assert_eq!(
        d.decision_id.as_ref().unwrap().value,
        "booking_eligibility.v1"
    );
    assert!(matches!(d.hit_policy, HitPolicyAst::First(_)));
    assert_eq!(d.inputs.len(), 5);
    assert_eq!(d.outputs.len(), 2);
    assert_eq!(d.rules.len(), 3);
}

#[test]
fn test_booking_eligibility_inputs() {
    let src = include_str!("fixtures/booking_eligibility.dmn-lite");
    let d = &parse(src).unwrap().decisions[0];

    assert_eq!(d.inputs[0].name.name, "jurisdiction");
    assert!(matches!(d.inputs[0].type_ref, TypeRefAst::Enum(_)));
    assert_eq!(d.inputs[0].domain_ref.name, "Jurisdiction");

    assert_eq!(d.inputs[1].name.name, "client-type");
    assert_eq!(d.inputs[2].name.name, "product");
    assert_eq!(d.inputs[3].name.name, "booking-principal");
    assert_eq!(d.inputs[4].name.name, "source-of-funds");
}

#[test]
fn test_booking_eligibility_outputs() {
    let src = include_str!("fixtures/booking_eligibility.dmn-lite");
    let d = &parse(src).unwrap().decisions[0];

    assert_eq!(d.outputs[0].name.name, "eligibility");
    assert!(matches!(d.outputs[0].type_ref, TypeRefAst::Enum(_)));
    assert_eq!(d.outputs[0].domain_ref.name, "EligibilityOutcome");

    assert_eq!(d.outputs[1].name.name, "reason-code");
    assert_eq!(d.outputs[1].domain_ref.name, "BookingReasonCode");
}

#[test]
fn test_booking_eligibility_rule_r001() {
    let src = include_str!("fixtures/booking_eligibility.dmn-lite");
    let d = &parse(src).unwrap().decisions[0];
    let r001 = &d.rules[0];

    assert_eq!(r001.id.name, "r001");
    assert_eq!(r001.then.len(), 2);
    assert_eq!(r001.then[0].output.name, "eligibility");
    assert!(matches!(&r001.then[0].value, LiteralAst::Symbol(s) if s.name == "ELIGIBLE"));
    assert_eq!(r001.then[1].output.name, "reason-code");

    let WhenAst::Predicates(preds, _) = &r001.when else {
        panic!("expected predicates")
    };
    assert_eq!(preds.len(), 5);

    // jurisdiction = LU
    assert!(
        matches!(&preds[0], PredicateAst::Eq { field, value: LiteralAst::Symbol(v), .. }
        if field.name == "jurisdiction" && v.name == "LU")
    );

    // client-type = SICAV
    assert!(matches!(&preds[1], PredicateAst::Eq { field, .. } if field.name == "client-type"));

    // product in (CUSTODY FUND_ACCOUNTING)
    let PredicateAst::InSet { field, values, .. } = &preds[2] else {
        panic!("expected in-set")
    };
    assert_eq!(field.name, "product");
    assert_eq!(values.len(), 2);

    // booking-principal in (BNY_LUX BNY_IE)
    assert!(
        matches!(&preds[3], PredicateAst::InSet { field, values, .. }
        if field.name == "booking-principal" && values.len() == 2)
    );

    // source-of-funds != UNKNOWN
    assert!(
        matches!(&preds[4], PredicateAst::NotEq { field, .. } if field.name == "source-of-funds")
    );
}

#[test]
fn test_booking_eligibility_catchall_rule() {
    let src = include_str!("fixtures/booking_eligibility.dmn-lite");
    let d = &parse(src).unwrap().decisions[0];
    let r999 = &d.rules[2];
    assert_eq!(r999.id.name, "r999");
    assert!(matches!(r999.when, WhenAst::CatchAll(_)));
    assert_eq!(r999.then[0].output.name, "eligibility");
}

// ── §5.2 age-band-classification ─────────────────────────────────────────────

#[test]
fn test_age_band_parses() {
    let src = include_str!("fixtures/age_band.dmn-lite");
    let ast = parse(src).expect("§5.2 age-band-classification must parse");

    let d = &ast.decisions[0];
    assert_eq!(d.name.name, "age-band-classification");
    assert!(matches!(d.hit_policy, HitPolicyAst::First(_)));
    assert_eq!(d.inputs.len(), 1);
    assert!(matches!(d.inputs[0].type_ref, TypeRefAst::Integer(_)));
    assert_eq!(d.outputs.len(), 1);
    assert_eq!(d.rules.len(), 4);
}

#[test]
fn test_age_band_ranges() {
    let src = include_str!("fixtures/age_band.dmn-lite");
    let d = &parse(src).unwrap().decisions[0];

    // r-minor: age in [* .. 18)  → lower=Unbounded inclusive, upper=18 exclusive
    let WhenAst::Predicates(preds, _) = &d.rules[0].when else {
        panic!()
    };
    let PredicateAst::Range {
        lower,
        upper,
        lower_inclusive,
        upper_inclusive,
        ..
    } = &preds[0]
    else {
        panic!("expected range predicate in r-minor")
    };
    assert!(matches!(lower, RangeBound::Unbounded(_)));
    assert!(*lower_inclusive);
    assert!(matches!(upper, RangeBound::Value(n) if n.text == "18"));
    assert!(!upper_inclusive);

    // r-young-adult: age in [18 .. 25] → both inclusive
    let WhenAst::Predicates(preds2, _) = &d.rules[1].when else {
        panic!()
    };
    let PredicateAst::Range {
        lower,
        upper,
        lower_inclusive,
        upper_inclusive,
        ..
    } = &preds2[0]
    else {
        panic!("expected range in r-young-adult")
    };
    assert!(matches!(lower, RangeBound::Value(n) if n.text == "18"));
    assert!(*lower_inclusive);
    assert!(matches!(upper, RangeBound::Value(n) if n.text == "25"));
    assert!(*upper_inclusive);

    // r-senior: age in [65 .. *] → lower=65 inclusive, upper=Unbounded inclusive
    let WhenAst::Predicates(preds4, _) = &d.rules[3].when else {
        panic!()
    };
    let PredicateAst::Range {
        lower,
        upper,
        lower_inclusive,
        upper_inclusive,
        ..
    } = &preds4[0]
    else {
        panic!("expected range in r-senior")
    };
    assert!(matches!(lower, RangeBound::Value(n) if n.text == "65"));
    assert!(*lower_inclusive);
    assert!(matches!(upper, RangeBound::Unbounded(_)));
    assert!(*upper_inclusive);
}

// ── §5.3 kyc-status ──────────────────────────────────────────────────────────

#[test]
fn test_kyc_status_parses() {
    let src = include_str!("fixtures/kyc_status.dmn-lite");
    let ast = parse(src).expect("§5.3 kyc-status must parse");

    let d = &ast.decisions[0];
    assert_eq!(d.name.name, "kyc-status");
    assert!(matches!(d.hit_policy, HitPolicyAst::First(_)));
    assert_eq!(d.inputs.len(), 2);
    assert_eq!(d.outputs.len(), 1);
    assert_eq!(d.rules.len(), 4);
}

#[test]
fn test_kyc_status_bool_predicates() {
    let src = include_str!("fixtures/kyc_status.dmn-lite");
    let d = &parse(src).unwrap().decisions[0];

    // r-not-submitted: documents-submitted = false
    let WhenAst::Predicates(preds, _) = &d.rules[0].when else {
        panic!()
    };
    assert!(
        matches!(&preds[0], PredicateAst::Eq { field, value: LiteralAst::Boolean { value, ..}, .. }
            if field.name == "documents-submitted" && !value)
    );

    // r-passed: two predicates — documents-submitted = true AND review-outcome = PASS
    let WhenAst::Predicates(preds2, _) = &d.rules[1].when else {
        panic!()
    };
    assert_eq!(preds2.len(), 2);
    assert!(
        matches!(&preds2[0], PredicateAst::Eq { field, value: LiteralAst::Boolean { value, .. }, .. }
            if field.name == "documents-submitted" && *value)
    );
    assert!(
        matches!(&preds2[1], PredicateAst::Eq { field, value: LiteralAst::Symbol(v), .. }
            if field.name == "review-outcome" && v.name == "PASS")
    );
}

#[test]
fn test_kyc_status_catchall() {
    let src = include_str!("fixtures/kyc_status.dmn-lite");
    let d = &parse(src).unwrap().decisions[0];
    assert!(matches!(d.rules[3].when, WhenAst::CatchAll(_)));
    assert_eq!(d.rules[3].id.name, "r-fallback");
}

#[test]
fn test_no_decision_id_optional() {
    // Decision without :decision-id must parse fine
    let src = r#"
(define-decision minimal
  :hit-policy unique
  :inputs  ((x :type integer :domain Numbers))
  :outputs ((y :type integer :domain Numbers))
  :rules
    ((rule r1 :when ((x = 1)) :then ((y = 42)))))
"#;
    let d = &parse(src).unwrap().decisions[0];
    assert!(d.decision_id.is_none());
    assert_eq!(d.name.name, "minimal");
}
