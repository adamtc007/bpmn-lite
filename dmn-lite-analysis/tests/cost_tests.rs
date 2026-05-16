//! Cost bound tests.

mod common;

use common::{INT_CAT, cat, verified};
use dmn_lite_analysis::{AnalysisConfig, analyse, analyse_with_config};
use dmn_lite_types::FindingKind;

#[test]
fn cost_bound_matches_predicate_count_simple() {
    // 2 rules, each with 1 predicate = 2 predicates total.
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r002 :when ((x = 2)) :then ((y = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert_eq!(report.cost_bound.total_predicates, 2);
    assert!(report.cost_bound.exact);
}

#[test]
fn cost_bound_counts_multiple_predicates_per_rule() {
    // 2 rules × 3 predicates = 6.
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N) (y :type integer :domain N) (z :type integer :domain N))
        :outputs ((out :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1) (y = 2) (z = 3)) :then ((out = 1)))
                  (rule r002 :when ((x = 4) (y = 5) (z = 6)) :then ((out = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert_eq!(report.cost_bound.total_predicates, 6);
}

#[test]
fn cost_bound_catch_all_contributes_zero() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert_eq!(report.cost_bound.total_predicates, 1, "catch-all adds 0");
}

#[test]
fn cost_ceiling_exceeded_emits_error_finding() {
    // 1 rule × 1 predicate = 1; ceiling = 0 → exceeded.
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))))"#;
    let cfg = AnalysisConfig {
        cost_ceiling: 0,
        ..Default::default()
    };
    let report = analyse_with_config(&verified(src, &c), &c, &cfg);
    let cost_finding = report
        .findings
        .iter()
        .find(|f| matches!(f.kind, FindingKind::CostCeilingExceeded { .. }));
    assert!(cost_finding.is_some(), "expected CostCeilingExceeded");
    if let Some(f) = cost_finding {
        assert_eq!(f.severity, dmn_lite_types::Severity::Error);
        if let FindingKind::CostCeilingExceeded { computed, ceiling } = f.kind {
            assert_eq!(computed, 1);
            assert_eq!(ceiling, 0);
        }
    }
}

#[test]
fn cost_ceiling_not_exceeded_no_finding() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(
        !report
            .findings
            .iter()
            .any(|f| matches!(f.kind, FindingKind::CostCeilingExceeded { .. })),
        "no ceiling-exceeded finding expected under default ceiling 10,000"
    );
}

#[test]
fn cost_bound_compound_predicate_counts_recursively() {
    // (and (x = 1) (y = 2)) → 3 (the And node + 2 children).
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N) (y :type integer :domain N))
        :outputs ((out :type integer :domain N))
        :rules   ((rule r001 :when ((and (x = 1) (y = 2))) :then ((out = 1)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert_eq!(report.cost_bound.total_predicates, 3);
}
