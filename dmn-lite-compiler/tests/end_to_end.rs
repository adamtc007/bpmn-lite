//! Vertical-slice end-to-end tests — Phase 1.3 §3.9 + Phase 1.4 VM e2e.
//!
//! Proves that source → AST → typed IR → evaluation → output composes
//! correctly across all three crates: parser, compiler, engine.
//! Phase 1.4 adds one VM path test: compile_and_verify → vm::evaluate.

use dmn_lite_compiler::{
    compile_and_verify, compile_to_ir, load_catalogue_from_str, lower_to_ir_with_warnings,
};
use dmn_lite_engine::{reference::evaluate, vm};
use dmn_lite_parser::parse;
use dmn_lite_types::{RuleId, TraceOutcome, ir::TypedValue, values::TypedInputContextBuilder};

const STUB: &str = include_str!("../../test-data/sem-os-stub.toml");
const BOOKING_SRC: &str =
    include_str!("../../dmn-lite-parser/tests/fixtures/booking_eligibility.dmn-lite");
const AGE_SRC: &str = include_str!("../../dmn-lite-parser/tests/fixtures/age_band.dmn-lite");
const KYC_SRC: &str = include_str!("../../dmn-lite-parser/tests/fixtures/kyc_status.dmn-lite");

fn enum_val(cat: &dmn_lite_compiler::Catalogue, domain: &str, sym: &str) -> TypedValue {
    let d = cat.resolve_domain(domain).expect("domain");
    TypedValue::Enum {
        domain_id: d.domain_id,
        value_id: d.resolve_value(sym).expect("value"),
    }
}

/// Primary vertical-slice proof: source → AST → typed IR → evaluation → output.
/// Uses r001 conditions on the booking_eligibility FIRST decision.
#[test]
fn vertical_slice_booking_eligibility_r001_match() {
    let ast = parse(BOOKING_SRC).expect("source should parse");
    let catalogue = load_catalogue_from_str(STUB).expect("catalogue must load");
    let decision = compile_to_ir(ast, &catalogue).expect("source should compile");

    assert_eq!(decision.name, "booking-eligibility");
    assert_eq!(decision.input_schema.len(), 5);
    assert_eq!(decision.output_schema.len(), 2);
    assert_eq!(decision.rules.len(), 3);

    let mut b = TypedInputContextBuilder::new(&decision.input_schema);
    b.set_by_name("jurisdiction", enum_val(&catalogue, "Jurisdiction", "LU"))
        .unwrap();
    b.set_by_name("client-type", enum_val(&catalogue, "CbuType", "SICAV"))
        .unwrap();
    b.set_by_name("product", enum_val(&catalogue, "ProductCode", "CUSTODY"))
        .unwrap();
    b.set_by_name(
        "booking-principal",
        enum_val(&catalogue, "BookingPrincipal", "BNY_LUX"),
    )
    .unwrap();
    b.set_by_name(
        "source-of-funds",
        enum_val(&catalogue, "SourceOfFunds", "SALARY"),
    )
    .unwrap();

    let result = evaluate(&decision, &b.build(), BOOKING_SRC).expect("r001 must match under FIRST");

    // FIRST returns r001 (index 0); r999 catch-all also evaluates true but
    // FIRST has already resolved.
    assert_eq!(
        result.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(0) }
    );
    assert_eq!(result.trace.rules.len(), 3);
    assert!(result.trace.rules[0].matched, "r001 matched");
    assert!(!result.trace.rules[1].matched, "r002 not matched");
    assert!(
        result.trace.rules[2].matched,
        "r999 catch-all evaluates true (FIRST already resolved)"
    );

    let eligibility = result
        .output
        .get_by_name(&decision.output_schema, "eligibility")
        .unwrap();
    assert_eq!(
        eligibility,
        &enum_val(&catalogue, "EligibilityOutcome", "ELIGIBLE")
    );

    let reason = result
        .output
        .get_by_name(&decision.output_schema, "reason-code")
        .unwrap();
    assert_eq!(
        reason,
        &enum_val(&catalogue, "BookingReasonCode", "STANDARD_LUX_SICAV")
    );

    // Predicate descriptions non-empty (source was supplied)
    for rule_trace in &result.trace.rules {
        for pred in &rule_trace.predicates {
            assert!(
                !pred.description.is_empty(),
                "all predicate descriptions must be non-empty when source is supplied"
            );
        }
    }
}

/// End-to-end with FIRST hit policy (age_band §5.2): proves range predicates,
/// numeric inputs, and ordered hit policy all compose correctly.
#[test]
fn vertical_slice_age_band_first_policy() {
    let catalogue = load_catalogue_from_str(STUB).unwrap();
    let res = lower_to_ir_with_warnings(parse(AGE_SRC).unwrap(), &catalogue);
    assert!(
        res.errors.is_empty(),
        "age_band compile errors: {:?}",
        res.errors
    );
    let decision = res.partial_decision.unwrap();

    // age = 40 should match r-adult (rule index 2, [26..64] inclusive)
    let mut b = TypedInputContextBuilder::new(&decision.input_schema);
    b.set_by_name("age", TypedValue::Integer(40)).unwrap();
    let result = evaluate(&decision, &b.build(), AGE_SRC).expect("40 → ADULT");

    let band = result
        .output
        .get_by_name(&decision.output_schema, "band")
        .unwrap();
    assert_eq!(band, &enum_val(&catalogue, "AgeBand", "ADULT"));
    assert_eq!(result.trace.rules.len(), 4, "age_band has 4 rules");
    assert!(matches!(result.trace.outcome, TraceOutcome::Match { .. }));
}

/// End-to-end with boolean inputs (kyc_status §5.3): proves bool-typed input
/// and enum output resolve and evaluate correctly.
#[test]
fn vertical_slice_kyc_status_bool_input() {
    let catalogue = load_catalogue_from_str(STUB).unwrap();
    let res = lower_to_ir_with_warnings(parse(KYC_SRC).unwrap(), &catalogue);
    assert!(
        res.errors.is_empty(),
        "kyc_status compile errors: {:?}",
        res.errors
    );
    let decision = res.partial_decision.unwrap();

    let mut b = TypedInputContextBuilder::new(&decision.input_schema);
    b.set_by_name("documents-submitted", TypedValue::Bool(true))
        .unwrap();
    b.set_by_name(
        "review-outcome",
        enum_val(&catalogue, "ReviewOutcome", "FAIL"),
    )
    .unwrap();
    let result = evaluate(&decision, &b.build(), KYC_SRC).expect("docs+fail → REJECTED");
    let status = result
        .output
        .get_by_name(&decision.output_schema, "kyc-status")
        .unwrap();
    assert_eq!(status, &enum_val(&catalogue, "KycStatus", "REJECTED"));
}

/// Phase 1.4 workspace-level e2e VM test:
/// source → parse → compile_and_verify → vm::evaluate → output.
///
/// Proves that the production stack VM executes the booking_eligibility
/// decision and produces the same output as the reference evaluator.
#[test]
fn vm_e2e_booking_eligibility_r001_matches() {
    let src = BOOKING_SRC;
    let catalogue = load_catalogue_from_str(STUB).expect("catalogue must load");
    let verified = compile_and_verify(parse(src).unwrap(), &catalogue, src)
        .expect("compile_and_verify must succeed");
    let compiled = verified.as_compiled();

    let mut b = TypedInputContextBuilder::new(&compiled.input_schema);
    b.set_by_name("jurisdiction", enum_val(&catalogue, "Jurisdiction", "LU"))
        .unwrap();
    b.set_by_name("client-type", enum_val(&catalogue, "CbuType", "SICAV"))
        .unwrap();
    b.set_by_name("product", enum_val(&catalogue, "ProductCode", "CUSTODY"))
        .unwrap();
    b.set_by_name(
        "booking-principal",
        enum_val(&catalogue, "BookingPrincipal", "BNY_LUX"),
    )
    .unwrap();
    b.set_by_name(
        "source-of-funds",
        enum_val(&catalogue, "SourceOfFunds", "SALARY"),
    )
    .unwrap();
    let ctx = b.build();

    let result = vm::evaluate(&verified, &ctx, src).expect("VM must succeed");
    // FIRST returns r001 (rule_id = 0).
    assert_eq!(
        result.trace.outcome,
        TraceOutcome::Match { rule_id: RuleId(0) }
    );
    // Output matches reference evaluator.
    let eligibility = result
        .output
        .get_by_name(&compiled.output_schema, "eligibility")
        .unwrap();
    assert_eq!(
        eligibility,
        &enum_val(&catalogue, "EligibilityOutcome", "ELIGIBLE")
    );
    let reason = result
        .output
        .get_by_name(&compiled.output_schema, "reason-code")
        .unwrap();
    assert_eq!(
        reason,
        &enum_val(&catalogue, "BookingReasonCode", "STANDARD_LUX_SICAV")
    );
}

/// Phase 1.6 workspace-level e2e analysis test:
/// source → parse → compile_and_verify → analyse.
///
/// Proves the full Phase 1 lifecycle composes correctly: parse, compile, verify,
/// (evaluate above), analyse.  The vertical slice is complete after this test.
#[test]
fn vertical_slice_with_analysis() {
    let src = BOOKING_SRC;
    let catalogue = load_catalogue_from_str(STUB).expect("catalogue must load");
    let verified = compile_and_verify(parse(src).unwrap(), &catalogue, src)
        .expect("compile_and_verify must succeed");

    let report = dmn_lite_analysis::analyse(&verified, &catalogue);

    // booking_eligibility uses FIRST + catch-all → no SA-001.
    assert!(
        !report.findings.iter().any(|f| matches!(
            f.kind,
            dmn_lite_types::FindingKind::UniqueWithCatchAll { .. }
        )),
        "FIRST + catch-all should not trigger SA-001"
    );

    // Cost bound: r001 has 5 predicates, r002 has 1, catch-all has 0 → 6.
    assert_eq!(report.cost_bound.total_predicates, 6);
    assert!(report.cost_bound.exact);

    // Catch-all covers gap → no Warning-severity Gap finding (Info is fine).
    let warning_gap = report.findings.iter().any(|f| {
        f.severity == dmn_lite_types::Severity::Warning
            && matches!(f.kind, dmn_lite_types::FindingKind::Gap { .. })
    });
    assert!(!warning_gap, "catch-all covers gap; no Warning expected");
}
