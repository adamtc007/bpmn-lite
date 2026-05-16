//! Typed predicate IR — the intermediate representation produced by
//! `dmn-lite-compiler` and consumed by the reference evaluator (Phase 1.3)
//! and bytecode emitter (Phase 1.4).
//!
//! Every symbol is resolved: field references are `FieldId` ordinals, enum
//! literals are `(DomainId, ValueId)` pairs, numeric literals are `i64`/`f64`.
//! No unresolved strings remain after compilation.
//!
//! The `resolved_entities` vector on `TypedDecision` records every resolved
//! enum literal in source order — the input to the artifact hash in Phase 1.4
//! (V&S §11.5 / dmn-lite-semantics.md §3.2.4).

use crate::ids::{DecisionId, DomainId, FieldId, RuleId, SourceSpan, ValueId};

/// Hit policy for a compiled decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitPolicy {
    /// At most one rule may match; multiple matches are a runtime error.
    Unique,
    /// Rules evaluated in source order; first match wins.
    First,
}

/// Fully-typed, fully-resolved decision IR.
///
/// Produced by `dmn_lite_compiler::compile()`. Consumed by the reference
/// evaluator and the bytecode emitter. Shape is stable across Phases 1.3–1.4.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedDecision {
    /// Decision identity derived from `:decision-id` or the decision name.
    pub decision_id: DecisionId,
    /// The decision name as written in the source.
    pub name: String,
    /// Evaluated hit policy.
    pub hit_policy: HitPolicy,
    /// Declared inputs in source order. `FieldId` = ordinal index.
    pub input_schema: Vec<FieldSchema>,
    /// Declared outputs in source order. `FieldId` = ordinal index.
    pub output_schema: Vec<FieldSchema>,
    /// Rules in source order.
    pub rules: Vec<TypedRule>,
    /// Every resolved enum literal in source order (input: rule order →
    /// predicate order → assignment order). Used as artifact hash input in
    /// Phase 1.4.
    pub resolved_entities: Vec<EntityRef>,
    /// Source span of the `(define-decision ...)` form.
    pub source_span: SourceSpan,
}

/// Schema entry for a single input or output field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSchema {
    /// Ordinal index of this field in the schema vector.
    pub field_id: FieldId,
    /// Field name as written in the source.
    pub name: String,
    /// Resolved type of the field.
    pub field_type: ResolvedType,
    /// Source span of the field declaration.
    pub source_span: SourceSpan,
}

/// Resolved type of a field after domain lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResolvedType {
    /// Enum type: all values must be members of `domain_id`.
    Enum {
        /// The domain this enum is drawn from.
        domain_id: DomainId,
    },
    /// Boolean: only `true` and `false` are valid.
    Bool,
    /// Integer (64-bit signed).
    Integer,
    /// Decimal (64-bit float per bytecode spec §2.4).
    Decimal,
    /// String: any UTF-8 string is accepted.
    Str,
}

impl ResolvedType {
    /// Human-readable name for use in diagnostics.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Enum { .. } => "enum",
            Self::Bool => "bool",
            Self::Integer => "integer",
            Self::Decimal => "decimal",
            Self::Str => "string",
        }
    }

    /// True for `Integer` or `Decimal` (types that admit ordered comparisons
    /// and range predicates).
    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Integer | Self::Decimal)
    }
}

/// A single resolved and type-checked rule.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedRule {
    /// Ordinal index of this rule in the decision.
    pub rule_id: RuleId,
    /// Rule identifier as written (e.g., `"r001"`).
    pub rule_name: String,
    /// Resolved `:when` clause.
    pub when: TypedWhen,
    /// Resolved `:then` assignments, one per declared output in source order.
    pub then: Vec<TypedAssignment>,
    /// Source span of the `(rule ...)` form.
    pub source_span: SourceSpan,
}

/// Resolved `:when` clause.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedWhen {
    /// Always-true catch-all `:when (*)`.
    CatchAll(SourceSpan),
    /// Predicate conjunction. All predicates must hold for the rule to fire.
    Predicates(Vec<TypedPredicate>, SourceSpan),
}

/// A resolved, type-checked predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedPredicate {
    /// `(field op rhs)` — equality, inequality, or ordered comparison.
    Comparison {
        /// The input field being tested (by ordinal).
        field: FieldId,
        /// The comparison operator.
        op: ComparisonOp,
        /// The resolved right-hand-side value.
        rhs: TypedValue,
        /// Source span.
        source_span: SourceSpan,
    },
    /// `(field in (v1 v2 ...))` — set membership.
    InSet {
        /// The input field being tested.
        field: FieldId,
        /// The set of candidate values (at least one).
        values: Vec<TypedValue>,
        /// Source span.
        source_span: SourceSpan,
    },
    /// `(field in [lower .. upper])` — range membership (numeric types).
    Range {
        /// The input field being tested.
        field: FieldId,
        /// Lower bound, or `None` for unbounded (`*`).
        lower: Option<TypedValue>,
        /// Upper bound, or `None` for unbounded (`*`).
        upper: Option<TypedValue>,
        /// True when `[` was used on the lower side (inclusive).
        lower_inclusive: bool,
        /// True when `]` was used on the upper side (inclusive).
        upper_inclusive: bool,
        /// Source span.
        source_span: SourceSpan,
    },
    /// `(field is-null)`.
    IsNull {
        /// The input field being tested.
        field: FieldId,
        /// Source span.
        source_span: SourceSpan,
    },
    /// `(field is-not-null)`.
    IsNotNull {
        /// The input field being tested.
        field: FieldId,
        /// Source span.
        source_span: SourceSpan,
    },
    /// `(not pred)` — negation.
    Not {
        /// The predicate to negate.
        inner: Box<TypedPredicate>,
        /// Source span.
        source_span: SourceSpan,
    },
    /// `(and pred1 pred2 ...)` — conjunction (at least two).
    And {
        /// The predicates to conjoin.
        items: Vec<TypedPredicate>,
        /// Source span.
        source_span: SourceSpan,
    },
    /// `(or pred1 pred2 ...)` — disjunction (at least two).
    Or {
        /// The predicates to disjoin.
        items: Vec<TypedPredicate>,
        /// Source span.
        source_span: SourceSpan,
    },
}

/// Comparison operator for `TypedPredicate::Comparison`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonOp {
    /// `=`
    Eq,
    /// `!=`
    NotEq,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

impl ComparisonOp {
    /// Display string for use in diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::NotEq => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

/// A resolved, typed value in a predicate or assignment.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedValue {
    /// A resolved enum value: domain + value identity.
    Enum {
        /// The domain this value belongs to.
        domain_id: DomainId,
        /// The specific value's identity.
        value_id: ValueId,
    },
    /// A boolean literal.
    Bool(bool),
    /// A 64-bit signed integer literal.
    Integer(i64),
    /// A 64-bit floating-point decimal literal (v0.1 per bytecode spec §2.4).
    Decimal(f64),
    /// A UTF-8 string literal.
    Str(String),
    /// Null / absent value (used in `is-null` / `is-not-null` evaluation).
    Null,
}

/// A resolved output assignment in a `:then` block.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedAssignment {
    /// The output field receiving the value (by ordinal).
    pub output_field: FieldId,
    /// The resolved value to assign.
    pub value: TypedValue,
    /// Source span.
    pub source_span: SourceSpan,
}

/// A resolved enum entity reference, recorded in source order for artifact
/// hash composition (V&S §11.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityRef {
    /// The domain of the resolved value.
    pub domain_id: DomainId,
    /// The resolved value identity.
    pub value_id: ValueId,
    /// Source span of the original symbol literal.
    pub source_span: SourceSpan,
}
