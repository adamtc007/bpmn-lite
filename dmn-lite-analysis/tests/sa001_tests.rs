//! SA-001 (UNIQUE + catch-all) tests.

mod common;

use common::{INT_CAT, cat, verified};
use dmn_lite_analysis::analyse;
use dmn_lite_types::FindingKind;

#[test]
fn unique_with_catch_all_emits_sa001() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(
        report
            .findings
            .iter()
            .any(|f| matches!(f.kind, FindingKind::UniqueWithCatchAll { .. })),
        "expected UniqueWithCatchAll, got: {:?}",
        report.findings
    );
}

#[test]
fn unique_no_catch_all_does_not_emit_sa001() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r002 :when ((x = 2)) :then ((y = 2)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(
        !report
            .findings
            .iter()
            .any(|f| matches!(f.kind, FindingKind::UniqueWithCatchAll { .. })),
        "unexpected UniqueWithCatchAll"
    );
}

#[test]
fn first_with_catch_all_does_not_emit_sa001() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let report = analyse(&verified(src, &c), &c);
    assert!(
        !report
            .findings
            .iter()
            .any(|f| matches!(f.kind, FindingKind::UniqueWithCatchAll { .. })),
        "SA-001 should not fire for FIRST with catch-all"
    );
}
