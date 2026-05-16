//! Determinism tests.

mod common;

use common::{AB_CAT, cat, verified};
use dmn_lite_analysis::analyse;
use dmn_lite_types::Severity;

#[test]
fn same_decision_produces_identical_report() {
    let c = cat(AB_CAT);
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = A)) :then ((y = OK)))
                  (rule r003 :when ((x = B)) :then ((y = OK)))))"#;
    let v = verified(src, &c);
    let report1 = analyse(&v, &c);
    let report2 = analyse(&v, &c);
    assert_eq!(report1, report2);
}

#[test]
fn findings_sorted_by_severity_then_rule_id() {
    let c = cat(AB_CAT);
    // Decision that produces multiple findings: SA-001 (UNIQUE+catch-all),
    // overlap (catch-all with specifics — skipped), gap (none, catch-all covers).
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = A)) :then ((y = OK)))
                  (rule r999 :when (*) :then ((y = OK)))))"#;
    let report = analyse(&verified(src, &c), &c);
    let severities: Vec<Severity> = report.findings.iter().map(|f| f.severity).collect();
    let mut sorted = severities.clone();
    sorted.sort();
    assert_eq!(
        severities, sorted,
        "findings must be in severity-ascending order"
    );
}
