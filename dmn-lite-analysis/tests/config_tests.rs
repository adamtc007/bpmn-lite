//! AnalysisConfig tests.

mod common;

use common::{AB_CAT, INT_CAT, cat, verified};
use dmn_lite_analysis::{AnalysisConfig, analyse_with_config};
use dmn_lite_types::{FindingKind, Severity};

#[test]
fn custom_cost_ceiling_fires_finding() {
    let c = cat(INT_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1) (x = 2) (x = 3)) :then ((y = 1)))))"#;
    let cfg = AnalysisConfig {
        cost_ceiling: 2,
        ..Default::default()
    };
    let report = analyse_with_config(&verified(src, &c), &c, &cfg);
    assert!(
        report
            .findings
            .iter()
            .any(|f| matches!(f.kind, FindingKind::CostCeilingExceeded { .. }))
    );
}

#[test]
fn emit_info_false_suppresses_info_findings() {
    let c = cat(AB_CAT);
    // FIRST overlap on x=A is Info-severity; with emit_info=false it disappears.
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type enum :domain AB))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A)) :then ((y = OK)))
                  (rule r002 :when ((x = A)) :then ((y = OK)))))"#;
    let cfg_default = AnalysisConfig::default();
    let cfg_no_info = AnalysisConfig {
        emit_info: false,
        ..cfg_default
    };
    let report_with = analyse_with_config(&verified(src, &c), &c, &AnalysisConfig::default());
    let report_without = analyse_with_config(&verified(src, &c), &c, &cfg_no_info);
    let info_with = report_with
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Info)
        .count();
    let info_without = report_without
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Info)
        .count();
    assert!(info_with > 0);
    assert_eq!(info_without, 0);
}

#[test]
fn max_gap_examples_controls_count() {
    use common::COMBO_CAT;
    let c = cat(COMBO_CAT);
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type enum :domain AB) (b :type bool :domain N))
        :outputs ((y :type enum :domain R))
        :rules   ((rule r001 :when ((x = A) (b = true)) :then ((y = OK)))))"#;
    let cfg = AnalysisConfig {
        max_gap_examples: 1,
        ..Default::default()
    };
    let report = analyse_with_config(&verified(src, &c), &c, &cfg);
    let f = report
        .findings
        .iter()
        .find(|f| matches!(f.kind, FindingKind::Gap { .. }))
        .expect("expected gap");
    if let FindingKind::Gap { gap_summary, .. } = &f.kind {
        assert!(gap_summary.examples.len() <= 1);
    }
}

#[test]
fn default_config_values() {
    let cfg = AnalysisConfig::default();
    assert_eq!(cfg.cost_ceiling, 10_000);
    assert_eq!(cfg.max_gap_examples, 3);
    assert!(cfg.emit_info);
}
