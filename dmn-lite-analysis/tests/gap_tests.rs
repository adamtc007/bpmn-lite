//! Gap analysis tests.

mod common;

use common::{AB_CAT, COMBO_CAT, INT_CAT, cat, verified};
use dmn_lite_analysis::{AnalysisConfig, analyse, analyse_with_config};
use dmn_lite_types::{FindingKind, Severity};

fn gap_finding(
    report: &dmn_lite_types::AnalysisReport,
) -> Option<&dmn_lite_types::AnalysisFinding> {
    report
        .findings
        .iter()
        .find(|f| matches!(f.kind, FindingKind::Gap { .. }))
}

#[test]
fn enum_missing_value_emits_gap() {
    let c = cat(AB_CAT);
    // AB has 3 values; rules cover only A and B → C is uncovered.
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = B)) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let f = gap_finding(&report).expect("expected Gap finding for C");
    assert_eq!(f.severity, Severity::Warning, "no catch-all → Warning");
    if let FindingKind::Gap {
        gap_summary,
        catch_all_present,
    } = &f.kind
    {
        assert!(!catch_all_present);
        assert!(!gap_summary.examples.is_empty());
    }
}

#[test]
fn catch_all_present_makes_gap_info() {
    let c = cat(AB_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r999 :when (*) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    // With catch-all, per-field union becomes Any → no gap finding.
    assert!(
        gap_finding(&report).is_none(),
        "catch-all covers everything → no gap finding (only Info)"
    );
}

#[test]
fn full_coverage_no_gap() {
    let c = cat(AB_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = B)) :then ((y = OK)))
                  (rule r003 :when ((x = C)) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(gap_finding(&report).is_none(), "all three values covered");
}

#[test]
fn integer_range_gap_detected() {
    let c = cat(INT_CAT);
    // r001 covers [0..50], r002 covers [60..100] → 51..59 is a gap.
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in [0 .. 50])) :then ((y = 1)))
                  (rule r002 :when ((x in [60 .. 100])) :then ((y = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let f = gap_finding(&report).expect("expected integer-range gap");
    if let FindingKind::Gap { gap_summary, .. } = &f.kind {
        assert!(!gap_summary.examples.is_empty());
        // Integer gap with open-ended complement → approximate_count is None.
        assert!(gap_summary.approximate_count.is_none());
    }
}

#[test]
fn approximate_count_none_for_open_integer_gap() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5)) :then ((y = 1)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let f = gap_finding(&report).expect("gap");
    if let FindingKind::Gap { gap_summary, .. } = &f.kind {
        assert!(
            gap_summary.approximate_count.is_none(),
            "integer = infinite gap"
        );
    }
}

#[test]
fn finite_enum_gap_has_count() {
    let c = cat(AB_CAT);
    // Cover A, B; C uncovered. With ≥ 1 uncovered value, approximate_count is Some.
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = B)) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let f = gap_finding(&report).expect("gap");
    if let FindingKind::Gap { gap_summary, .. } = &f.kind {
        assert_eq!(gap_summary.approximate_count, Some(1));
    }
}

#[test]
fn gap_summary_examples_capped_by_config() {
    let c = cat(COMBO_CAT);
    // Two-field decision where both have gaps.
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type enum :domain AB) (b :type bool :domain N))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A) (b = true)) :then ((y = OK)))))"#;
    let cfg = AnalysisConfig {
        max_gap_examples: 1,
        ..Default::default()
    };
    let report = analyse_with_config(&verified(src, &c), &c, &cfg);
    let f = gap_finding(&report).expect("gap");
    if let FindingKind::Gap { gap_summary, .. } = &f.kind {
        assert!(gap_summary.examples.len() <= 1);
    }
}

#[test]
fn boolean_uncovered_value_emits_gap() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((b :type bool :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((b = true)) :then ((y = 1)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let f = gap_finding(&report).expect("expected gap on b=false");
    if let FindingKind::Gap { gap_summary, .. } = &f.kind {
        assert!(!gap_summary.examples.is_empty());
    }
}
