//! Unreachable-rule analysis tests.

mod common;

use common::{AB_CAT, INT_CAT, cat, verified};
use dmn_lite_analysis::analyse;
use dmn_lite_types::{FindingKind, RuleId, Severity};

fn unreachable(report: &dmn_lite_types::AnalysisReport) -> Vec<(RuleId, RuleId)> {
    report
        .findings
        .iter()
        .filter_map(|f| match &f.kind {
            FindingKind::UnreachableRule {
                unreachable,
                shadowing,
            } => Some((*unreachable, *shadowing)),
            _ => None,
        })
        .collect()
}

#[test]
fn strict_subset_under_first_is_unreachable() {
    let c = cat(INT_CAT);
    // r001 accepts x in [0..100], r002 accepts x in [10..20] → r002 unreachable.
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in [0 .. 100])) :then ((y = 1)))
                  (rule r002 :when ((x in [10 .. 20])) :then ((y = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let pairs = unreachable(&report);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, RuleId(1)); // r002 unreachable
    assert_eq!(pairs[0].1, RuleId(0)); // shadowed by r001
}

#[test]
fn not_contained_not_flagged() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in [0 .. 50])) :then ((y = 1)))
                  (rule r002 :when ((x in [25 .. 75])) :then ((y = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(
        unreachable(&report).is_empty(),
        "[25..75] is not contained in [0..50]"
    );
}

#[test]
fn equal_regions_under_first_both_findings_emitted() {
    let c = cat(AB_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = A)) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let pairs = unreachable(&report);
    assert_eq!(pairs.len(), 1, "r002 unreachable (region equal to r001)");
    assert_eq!(pairs[0], (RuleId(1), RuleId(0)));
}

#[test]
fn three_rule_only_shadowing_rule_referenced() {
    let c = cat(INT_CAT);
    // r001=[0..5], r002=[0..100], r003=[10..20].
    // r003 ⊆ r002 (specifically shadowed by the immediate superset).
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in [0 .. 5])) :then ((y = 1)))
                  (rule r002 :when ((x in [0 .. 100])) :then ((y = 2)))
                  (rule r003 :when ((x in [10 .. 20])) :then ((y = 3)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let pairs = unreachable(&report);
    let r003_findings: Vec<_> = pairs.iter().filter(|(u, _)| u.0 == 2).collect();
    assert_eq!(
        r003_findings.len(),
        1,
        "r003 has exactly one shadow finding"
    );
    // First earlier shadow wins (r001 is shadowing r003? No — r001 is [0..5],
    // does not contain [10..20]. r002 is [0..100], contains [10..20] → shadow r002).
    assert_eq!(r003_findings[0].1, RuleId(1));
}

#[test]
fn unique_policy_no_unreachable_emitted() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in [0 .. 100])) :then ((y = 1)))
                  (rule r002 :when ((x in [10 .. 20])) :then ((y = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(
        unreachable(&report).is_empty(),
        "UNIQUE does not produce unreachable findings"
    );
}

#[test]
fn catch_all_does_not_shadow_in_analysis_layer() {
    // Phase 1.2 compiler would reject this source, so we test with catch-all
    // last (well-formed under FIRST). Catch-all cannot shadow anything before it;
    // no rule after catch-all (would be rejected by compiler).
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(
        unreachable(&report).is_empty(),
        "no unreachable findings expected — catch-all is at end and analyser doesn't re-emit"
    );
}

#[test]
fn unreachable_severity_is_error() {
    let c = cat(AB_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x in (A B))) :then ((y = OK)))
                  (rule r002 :when ((x = A)) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let f = report
        .findings
        .iter()
        .find(|f| matches!(f.kind, FindingKind::UnreachableRule { .. }))
        .expect("unreachable finding");
    assert_eq!(f.severity, Severity::Error);
}
