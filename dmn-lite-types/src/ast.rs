//! Abstract syntax tree for the dmn-lite s-expression DSL.
//!
//! Produced by `dmn-lite-parser::parse()`, consumed by `dmn-lite-compiler`.
//! Every node carries a [`SourceSpan`] for diagnostics.
//!
//! The AST mirrors the EBNF grammar in `docs/dmn-lite-ebnf.md` exactly.
//! No symbol resolution or type-checking is performed at this stage — all
//! identifiers are stored as raw strings. Phase 1.2 lowers the AST to a
//! typed predicate IR after resolving symbols against Sem OS domains.

use crate::ids::{NumberKind, SourceSpan};

/// A parsed dmn-lite source unit.
///
/// Profile v0.1 requires exactly one decision per source file. The parser
/// enforces this at parse time; the `decisions` vec will have length 1 on
/// success, or length 0..=1 on partial recovery from errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Parsed decisions (arity 1 in v0.1).
    pub decisions: Vec<DecisionAst>,
    /// Span covering the entire source text.
    pub span: SourceSpan,
}

/// A single `(define-decision …)` form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionAst {
    /// The decision name symbol, e.g. `booking-eligibility`.
    pub name: SymbolAst,
    /// Optional `:decision-id "…"` advisory metadata string.
    pub decision_id: Option<StringLitAst>,
    /// Hit policy declared with `:hit-policy`.
    pub hit_policy: HitPolicyAst,
    /// Input field declarations from `:inputs ((…) …)`.
    pub inputs: Vec<InputDeclAst>,
    /// Output field declarations from `:outputs ((…) …)`.
    pub outputs: Vec<OutputDeclAst>,
    /// Rule list from `:rules ((rule …) …)`.
    pub rules: Vec<RuleAst>,
    /// Span from opening `(` to closing `)`.
    pub span: SourceSpan,
}

/// A single input field declaration: `(name :type T :domain D)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDeclAst {
    /// Field name symbol.
    pub name: SymbolAst,
    /// Declared type — one of the five v0.1 type keywords.
    pub type_ref: TypeRefAst,
    /// Domain reference symbol (required for all types in v0.1 grammar).
    pub domain_ref: SymbolAst,
    /// Span of the entire declaration including parens.
    pub span: SourceSpan,
}

/// A single output field declaration: `(name :type T :domain D)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDeclAst {
    /// Field name symbol.
    pub name: SymbolAst,
    /// Declared type — one of the five v0.1 type keywords.
    pub type_ref: TypeRefAst,
    /// Domain reference symbol.
    pub domain_ref: SymbolAst,
    /// Span of the entire declaration including parens.
    pub span: SourceSpan,
}

/// One of the five Profile v0.1 type keywords.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRefAst {
    /// The `enum` keyword.
    Enum(SourceSpan),
    /// The `bool` keyword.
    Bool(SourceSpan),
    /// The `integer` keyword.
    Integer(SourceSpan),
    /// The `decimal` keyword.
    Decimal(SourceSpan),
    /// The `string` keyword.
    String(SourceSpan),
}

impl TypeRefAst {
    /// The source span of the keyword token.
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Enum(s)
            | Self::Bool(s)
            | Self::Integer(s)
            | Self::Decimal(s)
            | Self::String(s) => *s,
        }
    }
}

/// Hit policy from `:hit-policy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitPolicyAst {
    /// `unique` — at most one rule may match; multiple matches are a runtime error.
    Unique(SourceSpan),
    /// `first` — rules evaluated in order; first match wins.
    First(SourceSpan),
}

impl HitPolicyAst {
    /// The source span of the hit-policy keyword token.
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Unique(s) | Self::First(s) => *s,
        }
    }
}

/// A single rule: `(rule id :when … :then …)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleAst {
    /// Rule identifier symbol, e.g. `r001`.
    pub id: SymbolAst,
    /// The `:when` clause.
    pub when: WhenAst,
    /// The `:then` output assignments.
    pub then: Vec<AssignmentAst>,
    /// Span from opening `(` to closing `)`.
    pub span: SourceSpan,
}

/// The `:when` clause of a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhenAst {
    /// `:when (*)` — always-true catch-all rule.
    CatchAll(SourceSpan),
    /// `:when ((pred1) (pred2) …)` — implicit conjunction of predicates.
    Predicates(Vec<PredicateAst>, SourceSpan),
}

/// A single predicate expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateAst {
    /// `(field = value)` equality test.
    Eq {
        /// The input field being tested.
        field: SymbolAst,
        /// The value the field is compared against.
        value: LiteralAst,
        /// Span of the entire predicate including parens.
        span: SourceSpan,
    },
    /// `(field != value)` inequality test.
    NotEq {
        /// The input field being tested.
        field: SymbolAst,
        /// The value the field is compared against.
        value: LiteralAst,
        /// Span of the entire predicate including parens.
        span: SourceSpan,
    },
    /// `(field < value)` strictly-less-than comparison (numeric types only).
    Lt {
        /// The input field being tested.
        field: SymbolAst,
        /// The numeric bound.
        value: NumberLitAst,
        /// Span of the entire predicate including parens.
        span: SourceSpan,
    },
    /// `(field <= value)` less-than-or-equal comparison (numeric types only).
    Le {
        /// The input field being tested.
        field: SymbolAst,
        /// The numeric bound.
        value: NumberLitAst,
        /// Span of the entire predicate including parens.
        span: SourceSpan,
    },
    /// `(field > value)` strictly-greater-than comparison (numeric types only).
    Gt {
        /// The input field being tested.
        field: SymbolAst,
        /// The numeric bound.
        value: NumberLitAst,
        /// Span of the entire predicate including parens.
        span: SourceSpan,
    },
    /// `(field >= value)` greater-than-or-equal comparison (numeric types only).
    Ge {
        /// The input field being tested.
        field: SymbolAst,
        /// The numeric bound.
        value: NumberLitAst,
        /// Span of the entire predicate including parens.
        span: SourceSpan,
    },
    /// `(field in (v1 v2 …))` set-membership test.
    InSet {
        /// The input field being tested.
        field: SymbolAst,
        /// The set of candidate values (at least one, enforced by parser).
        values: Vec<LiteralAst>,
        /// Span of the entire predicate including parens.
        span: SourceSpan,
    },
    /// `(field in [a .. b])` range test (numeric types only).
    ///
    /// Bracket character determines inclusivity:
    /// `[` inclusive, `(` exclusive on each side independently.
    Range {
        /// The input field being tested.
        field: SymbolAst,
        /// Lower bound value or unbounded.
        lower: RangeBound,
        /// Upper bound value or unbounded.
        upper: RangeBound,
        /// True when `[` was used on the lower side.
        lower_inclusive: bool,
        /// True when `]` was used on the upper side.
        upper_inclusive: bool,
        /// Span of the entire predicate including parens.
        span: SourceSpan,
    },
    /// `(field is-null)` null test.
    IsNull {
        /// The input field being tested.
        field: SymbolAst,
        /// Span of the entire predicate including parens.
        span: SourceSpan,
    },
    /// `(field is-not-null)` not-null test.
    IsNotNull {
        /// The input field being tested.
        field: SymbolAst,
        /// Span of the entire predicate including parens.
        span: SourceSpan,
    },
    /// `(not pred)` negation — inverts the inner predicate.
    Not {
        /// The predicate to negate.
        inner: Box<PredicateAst>,
        /// Span of the entire `(not …)` form.
        span: SourceSpan,
    },
    /// `(and pred1 pred2 …)` conjunction — all must be true (at least two).
    And {
        /// The predicates to conjoin (at least two).
        items: Vec<PredicateAst>,
        /// Span of the entire `(and …)` form.
        span: SourceSpan,
    },
    /// `(or pred1 pred2 …)` disjunction — at least one must be true (at least two).
    Or {
        /// The predicates to disjoin (at least two).
        items: Vec<PredicateAst>,
        /// Span of the entire `(or …)` form.
        span: SourceSpan,
    },
}

impl PredicateAst {
    /// The source span of this predicate.
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Eq { span, .. }
            | Self::NotEq { span, .. }
            | Self::Lt { span, .. }
            | Self::Le { span, .. }
            | Self::Gt { span, .. }
            | Self::Ge { span, .. }
            | Self::InSet { span, .. }
            | Self::Range { span, .. }
            | Self::IsNull { span, .. }
            | Self::IsNotNull { span, .. }
            | Self::Not { span, .. }
            | Self::And { span, .. }
            | Self::Or { span, .. } => *span,
        }
    }
}

/// A bound in a range predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeBound {
    /// `*` — the bound is unbounded (open).
    Unbounded(SourceSpan),
    /// A concrete numeric value.
    Value(NumberLitAst),
}

impl RangeBound {
    /// The source span of this bound.
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Unbounded(s) => *s,
            Self::Value(n) => n.span,
        }
    }
}

/// An output assignment in a `:then` block: `(output = value)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentAst {
    /// The output field name.
    pub output: SymbolAst,
    /// The value assigned.
    pub value: LiteralAst,
    /// Span of the entire assignment including parens.
    pub span: SourceSpan,
}

/// A literal value in a predicate or assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiteralAst {
    /// An unresolved symbol, e.g. an enum value name like `LU` or `SICAV`.
    Symbol(SymbolAst),
    /// A double-quoted string literal.
    String(StringLitAst),
    /// A numeric literal (integer or decimal).
    Number(NumberLitAst),
    /// `true` or `false`.
    Boolean {
        /// The boolean value.
        value: bool,
        /// Span of the `true` or `false` token.
        span: SourceSpan,
    },
}

impl LiteralAst {
    /// The source span of this literal.
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Symbol(s) => s.span,
            Self::String(s) => s.span,
            Self::Number(n) => n.span,
            Self::Boolean { span, .. } => *span,
        }
    }
}

/// An identifier or keyword in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolAst {
    /// The raw symbol text as written in the source.
    pub name: String,
    /// Source span of the symbol token.
    pub span: SourceSpan,
}

/// A double-quoted string literal with escape sequences resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringLitAst {
    /// The string value with escape sequences resolved (`\"` → `"`, `\\` → `\`).
    pub value: String,
    /// Source span including the surrounding quotes.
    pub span: SourceSpan,
}

/// A numeric literal stored as its source text.
///
/// The text is not parsed to a numeric type at this stage; Phase 1.2 does
/// that once the declared field type is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberLitAst {
    /// Source text exactly as written, e.g. `"42"`, `"-3.14"`.
    pub text: String,
    /// Whether this was an integer or decimal literal at the source level.
    pub kind: NumberKind,
    /// Source span of the literal token.
    pub span: SourceSpan,
}
