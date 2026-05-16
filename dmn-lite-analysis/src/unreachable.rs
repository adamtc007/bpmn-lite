//! Unreachable rule analysis (FIRST hit policy only).
//!
//! Under FIRST, `r_b` is unreachable when some earlier `r_a` accepts a superset
//! of `r_b`'s input region.  Equivalent to: `r_b.region ⊆ r_a.region` and `a < b`.
//!
//! Catch-all-followed-by-rule under FIRST is already caught by the Phase 1.2
//! compiler (`CompileError::UnreachableAfterCatchAll`); this analyser **does not
//! re-emit it**.  The interesting cases here are the subtler shadowings where an
//! earlier specific rule's predicates accept a strict superset of a later rule.
//!
//! Under UNIQUE the semantics are different (all rules evaluate; multiple matches
//! produce an error), so unreachable analysis is FIRST-only.

use dmn_lite_types::{
    AnalysisFinding, FindingKind, Severity,
    ir::{HitPolicy, TypedDecision},
};

use crate::region::{RuleRegion, is_subset_of};

/// Run the unreachable-rule analysis (FIRST policy only).
pub fn analyse(decision: &TypedDecision, regions: &[RuleRegion]) -> Vec<AnalysisFinding> {
    if decision.hit_policy != HitPolicy::First {
        return Vec::new();
    }
    let mut findings = Vec::new();
    let n = decision.rules.len();
    for b in 0..n {
        if regions[b].is_catch_all {
            // Catch-alls cannot be shadowed: their region is "everything".
            continue;
        }
        for a in 0..b {
            // Skip catch-all-shadows-rule: already caught by Phase 1.2 compiler.
            if regions[a].is_catch_all {
                continue;
            }
            if region_is_subset(&regions[b], &regions[a]) {
                findings.push(AnalysisFinding {
                    severity: Severity::Error,
                    kind: FindingKind::UnreachableRule {
                        unreachable: decision.rules[b].rule_id,
                        shadowing: decision.rules[a].rule_id,
                    },
                    source_span: decision.rules[b].source_span,
                    description: format!(
                        "rule {} is unreachable: rule {} accepts a superset of its input region",
                        decision.rules[b].rule_name, decision.rules[a].rule_name
                    ),
                });
                break; // One shadowing rule per unreachable rule is enough.
            }
        }
    }
    findings
}

/// True when every field of `inner` is a subset of the corresponding field of `outer`.
fn region_is_subset(inner: &RuleRegion, outer: &RuleRegion) -> bool {
    inner
        .fields
        .iter()
        .zip(outer.fields.iter())
        .all(|(a, b)| is_subset_of(a, b))
}
