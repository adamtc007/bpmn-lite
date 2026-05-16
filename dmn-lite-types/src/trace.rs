//! Evaluation trace types.
//!
//! [`EvaluationTrace`] is the diagnostic artifact produced alongside every
//! evaluation result. It records per-rule and per-predicate outcomes in source
//! order, enabling Phase 1.5 differential testing and future explainability.
//!
//! The trace shape is a contract: Phase 1.4's bytecode VM will produce
//! compatible traces with the same invariants so that Phase 1.5 can diff them.

use crate::ids::{RuleId, SourceSpan};

/// Complete trace of a single decision evaluation.
///
/// Invariant: `rules[i]` corresponds to `decision.rules[i]` in source order.
/// Length equals the number of rules in the decision.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluationTrace {
    /// Per-rule trace, in source order.
    pub rules: Vec<RuleTrace>,
    /// Final outcome after hit policy is applied.
    pub outcome: TraceOutcome,
}

/// Trace for a single rule evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleTrace {
    /// Ordinal index of this rule in the decision.
    pub rule_id: RuleId,
    /// Rule identifier as written in the source.
    pub rule_name: String,
    /// True if this rule's when clause evaluated to true.
    ///
    /// Invariant: `matched == predicates.iter().all(|p| p.result)`.
    pub matched: bool,
    /// Per-predicate evaluation results, at the `:when`-clause level.
    ///
    /// For catch-all rules, contains a single entry with `result = true`.
    /// For rules with `(and ...)` or `(or ...)` predicates, the `and`/`or`
    /// is a single entry here; its internal sub-predicates are not expanded.
    pub predicates: Vec<PredicateTrace>,
    /// Source span of the `(rule ...)` form.
    pub source_span: SourceSpan,
}

/// Trace for a single predicate evaluation at the `:when`-clause level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateTrace {
    /// Whether this predicate evaluated to `true`.
    pub result: bool,
    /// Source span of the predicate form in the original source.
    pub source_span: SourceSpan,
    /// Human-readable description extracted from source bytes.
    ///
    /// Equals `source[span.start..span.end]` when source is available.
    /// Empty string if the evaluator was called without source.
    /// Not a structured representation — purely for diagnostics and test output.
    pub description: String,
}

/// Outcome of hit-policy application after all rules are evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceOutcome {
    /// Hit policy found exactly one matching rule (or the first match under FIRST).
    Match {
        /// The rule that produced the output.
        rule_id: RuleId,
    },
    /// No rule matched; the evaluator returned `EvalError::NoMatch`.
    NoMatch,
    /// UNIQUE policy found multiple matching rules.
    MultipleMatches {
        /// All matching rule IDs in source order.
        rules: Vec<RuleId>,
    },
    /// Evaluation produced an error (type mismatch, schema mismatch, etc.).
    EvalError,
}
