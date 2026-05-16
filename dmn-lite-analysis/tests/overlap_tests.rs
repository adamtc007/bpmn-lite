//! Overlap analysis tests.

mod common;

use common::{AB_CAT, COMBO_CAT, INT_CAT, cat, verified};
use dmn_lite_analysis::analyse;
use dmn_lite_types::{FindingKind, RuleId, Severity};

fn overlaps(report: &dmn_lite_types::AnalysisReport) -> Vec<(RuleId, RuleId)> {
    report
        .findings
        .iter()
        .filter_map(|f| match &f.kind {
            FindingKind::Overlap { rule_a, rule_b, .. } => Some((*rule_a, *rule_b)),
            _ => None,
        })
        .collect()
}

#[test]
fn enum_overlapping_sets_emit_overlap() {
    let c = cat(AB_CAT);
    // UNIQUE so overlap is Warning.
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x in (A B))) :then ((y = OK)))
                  (rule r002 :when ((x in (B C))) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let pairs = overlaps(&report);
    assert!(!pairs.is_empty(), "expected overlap on value B");
    assert_eq!(pairs[0], (RuleId(0), RuleId(1)));
}

#[test]
fn enum_disjoint_sets_no_overlap() {
    let c = cat(AB_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = B)) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(overlaps(&report).is_empty(), "A and B are disjoint");
}

#[test]
fn integer_overlapping_ranges_emit_overlap() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in [0 .. 10])) :then ((y = 1)))
                  (rule r002 :when ((x in [5 .. 15])) :then ((y = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let pairs = overlaps(&report);
    assert_eq!(pairs.len(), 1, "expected exactly one overlap pair");
    assert_eq!(pairs[0], (RuleId(0), RuleId(1)));
}

#[test]
fn integer_disjoint_ranges_no_overlap() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in [0 .. 10])) :then ((y = 1)))
                  (rule r002 :when ((x in [20 .. 30])) :then ((y = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(overlaps(&report).is_empty());
}

#[test]
fn one_rule_unconstrained_field_overlaps() {
    let c = cat(COMBO_CAT);
    // r001 constrains x, r002 constrains y. Both rules have "Any" on the
    // other field → overlap is non-empty.
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type enum :domain AB) (y :type integer :domain N))
        :outputs ((out :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((out = OK)))
                  (rule r002 :when ((y = 5)) :then ((out = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let pairs = overlaps(&report);
    assert_eq!(pairs.len(), 1);
}

#[test]
fn catch_all_skipped_in_overlap_emission() {
    let c = cat(AB_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r999 :when (*) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    // Overlap between r001 and r999 (catch-all) must NOT be emitted.
    let pairs = overlaps(&report);
    assert!(
        !pairs.iter().any(|(a, b)| a.0 == 1 || b.0 == 1),
        "catch-all overlaps should be skipped"
    );
}

#[test]
fn three_way_overlap_emits_all_pairs() {
    let c = cat(AB_CAT);
    // r001=A, r002=A, r003=A (all overlap with each other on value A).
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = A)) :then ((y = OK)))
                  (rule r003 :when ((x = A)) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let pairs = overlaps(&report);
    // (0,1), (0,2), (1,2) — three pairs.
    assert_eq!(pairs.len(), 3);
}

#[test]
fn overlap_rule_ids_sorted_with_lower_first() {
    let c = cat(AB_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = A)) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let pairs = overlaps(&report);
    assert!(pairs.iter().all(|(a, b)| a.0 < b.0));
}

#[test]
fn unique_overlap_severity_is_warning() {
    let c = cat(AB_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = A)) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let f = report
        .findings
        .iter()
        .find(|f| matches!(f.kind, FindingKind::Overlap { .. }))
        .expect("overlap finding");
    assert_eq!(f.severity, Severity::Warning);
}

#[test]
fn first_overlap_severity_is_info() {
    let c = cat(AB_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = A)) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let f = report
        .findings
        .iter()
        .find(|f| matches!(f.kind, FindingKind::Overlap { .. }))
        .expect("overlap finding");
    assert_eq!(f.severity, Severity::Info);
}

#[test]
fn boolean_overlapping_values_emit_overlap() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((b :type bool :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((b = true)) :then ((y = 1)))
                  (rule r002 :when ((b = true)) :then ((y = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let pairs = overlaps(&report);
    assert_eq!(pairs.len(), 1);
}

#[test]
fn boolean_disjoint_values_no_overlap() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((b :type bool :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((b = true)) :then ((y = 1)))
                  (rule r002 :when ((b = false)) :then ((y = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(overlaps(&report).is_empty());
}
