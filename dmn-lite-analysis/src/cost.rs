//! Cost bound computation (V&S §23 #15).
//!
//! In Profile v0.1 the cost is the total predicate count across all rules,
//! counted recursively into compound predicates.  Every v0.1 predicate is O(1):
//! comparison (≤ a few opcodes), set lookup (finite, linear scan over small set),
//! range check (constant), null test (constant), boolean combinator (constant
//! per child).
//!
//! Catch-all rules contribute 0 to the count (no predicates).

use dmn_lite_types::{
    CostBound,
    ir::{TypedDecision, TypedPredicate, TypedWhen},
};

/// Compute the cost bound for a verified decision's typed IR.
pub fn compute(decision: &TypedDecision) -> CostBound {
    let mut total: usize = 0;
    for rule in &decision.rules {
        match &rule.when {
            TypedWhen::CatchAll(_) => {} // 0 predicates
            TypedWhen::Predicates(preds, _) => {
                for p in preds {
                    total += count_predicate(p);
                }
            }
        }
    }
    CostBound {
        total_predicates: total,
        exact: true,
    }
}

fn count_predicate(p: &TypedPredicate) -> usize {
    match p {
        TypedPredicate::Comparison { .. }
        | TypedPredicate::InSet { .. }
        | TypedPredicate::Range { .. }
        | TypedPredicate::IsNull { .. }
        | TypedPredicate::IsNotNull { .. } => 1,
        TypedPredicate::Not { inner, .. } => 1 + count_predicate(inner),
        TypedPredicate::And { items, .. } | TypedPredicate::Or { items, .. } => {
            1 + items.iter().map(count_predicate).sum::<usize>()
        }
    }
}
