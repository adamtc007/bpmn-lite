//! Integration tests against the EBNF fixtures (§5.1, §5.2, §5.3).
//!
//! Each fixture is analysed and the resulting findings are inspected for
//! documented expectations.  These tests prove the analyser composes correctly
//! with the full pipeline (parse → compile → verify → analyse).

use dmn_lite_analysis::analyse;
use dmn_lite_compiler::{compile_and_verify, load_catalogue_from_str};
use dmn_lite_parser::parse;
use dmn_lite_types::{FindingKind, Severity};

const STUB: &str = include_str!("../../test-data/sem-os-stub.toml");
const BOOKING_SRC: &str =
    include_str!("../../dmn-lite-parser/tests/fixtures/booking_eligibility.dmn-lite");
const AGE_BAND_SRC: &str = include_str!("../../dmn-lite-parser/tests/fixtures/age_band.dmn-lite");
const KYC_SRC: &str = include_str!("../../dmn-lite-parser/tests/fixtures/kyc_status.dmn-lite");

fn analyse_fixture(src: &str) -> (dmn_lite_types::AnalysisReport, dmn_lite_compiler::Catalogue) {
    let c = load_catalogue_from_str(STUB).expect("catalogue must load");
    let v = compile_and_verify(parse(src).expect("parse"), &c, src).expect("compile_and_verify");
    let report = analyse(&v, &c);
    (report, c)
}

#[test]
fn booking_eligibility_analysis() {
    let (report, _) = analyse_fixture(BOOKING_SRC);
    // FIRST hit policy with catch-all → no SA-001 finding.
    assert!(
        !report
            .findings
            .iter()
            .any(|f| matches!(f.kind, FindingKind::UniqueWithCatchAll { .. })),
        "FIRST + catch-all should not trigger SA-001"
    );
    // Catch-all covers the gap → no Warning Gap finding.
    assert!(
        !report.findings.iter().any(|f| {
            f.severity == Severity::Warning && matches!(f.kind, FindingKind::Gap { .. })
        }),
        "catch-all covers any gap; severity should be Info if present"
    );
    // Cost bound is the sum of r001 + r002 predicate counts (catch-all = 0).
    // r001 has 5, r002 has 1 → 6.
    assert_eq!(report.cost_bound.total_predicates, 6);
}

#[test]
fn age_band_analysis() {
    let (report, _) = analyse_fixture(AGE_BAND_SRC);
    // 4 rules, each with 1 range predicate → 4 predicates.
    assert_eq!(report.cost_bound.total_predicates, 4);
    // The 4 ranges cover (-∞, +∞) so there should be no Warning gap.
    let warning_gap = report
        .findings
        .iter()
        .any(|f| f.severity == Severity::Warning && matches!(f.kind, FindingKind::Gap { .. }));
    assert!(!warning_gap, "age_band ranges should fully cover integers");
    // The 4 ranges are mutually exclusive → no overlap findings (or only Info ones).
    let warning_overlap = report
        .findings
        .iter()
        .any(|f| f.severity == Severity::Warning && matches!(f.kind, FindingKind::Overlap { .. }));
    assert!(!warning_overlap, "age_band ranges are mutually exclusive");
}

#[test]
fn kyc_status_analysis() {
    let (report, _) = analyse_fixture(KYC_SRC);
    // 3 specific rules + catch-all. Catch-all covers gap; FIRST policy → Info overlap
    // if any specific rules overlap.
    assert!(
        !report
            .findings
            .iter()
            .any(|f| matches!(f.kind, FindingKind::UniqueWithCatchAll { .. })),
        "FIRST policy — no SA-001"
    );
    // Predicate counts: r-not-submitted (1) + r-passed (2) + r-failed (2) + r-fallback (0) = 5.
    assert_eq!(report.cost_bound.total_predicates, 5);
}
