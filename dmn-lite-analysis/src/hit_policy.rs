//! SA-001 — UNIQUE hit policy with a catch-all rule.
//!
//! `:hit-policy unique` with a `:when (*)` catch-all is structurally broken: any
//! input that matches any specific rule also matches the catch-all, producing
//! `MultipleMatches` at evaluation.  The recommended fix is to switch to FIRST
//! or remove the catch-all.

use dmn_lite_types::{
    AnalysisFinding, FindingKind, Severity,
    ir::{HitPolicy, TypedDecision, TypedWhen},
};

/// Run the SA-001 check.  Returns one finding or `None`.
pub fn check(decision: &TypedDecision) -> Option<AnalysisFinding> {
    if decision.hit_policy != HitPolicy::Unique {
        return None;
    }
    let catch_all = decision
        .rules
        .iter()
        .find(|r| matches!(r.when, TypedWhen::CatchAll(_)))?;
    Some(AnalysisFinding {
        severity: Severity::Error,
        kind: FindingKind::UniqueWithCatchAll {
            catch_all_rule: catch_all.rule_id,
        },
        source_span: catch_all.source_span,
        description: format!(
            "rule {} is a catch-all under :hit-policy unique; will always produce MultipleMatches when any specific rule fires",
            catch_all.rule_name
        ),
    })
}
