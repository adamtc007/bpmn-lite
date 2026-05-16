//! Error and warning vocabulary for dmn-lite.

use thiserror::Error;

use crate::ids::SourceSpan;

// ── Parse errors ──────────────────────────────────────────────────────────────

/// Lexical or syntactic errors produced by `dmn-lite-parser`.
///
/// Every variant carries a [`SourceSpan`] so the caller can report the
/// exact source location of the problem.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    /// An unrecognised byte was encountered during lexing.
    #[error("unexpected character '{ch}' at {span}")]
    UnexpectedChar {
        /// The unexpected character.
        ch: char,
        /// Location of the character.
        span: SourceSpan,
    },

    /// The input ended before the parser expected it to.
    #[error("unexpected end of input; expected {expected}")]
    UnexpectedEof {
        /// Description of what was expected.
        expected: String,
        /// Position where more input was expected.
        span: SourceSpan,
    },

    /// A token appeared where a different token was required.
    #[error("unexpected token '{found}'; expected {expected} at {span}")]
    UnexpectedToken {
        /// Description of what was expected.
        expected: String,
        /// What was actually found.
        found: String,
        /// Location of the unexpected token.
        span: SourceSpan,
    },

    /// A string literal contained an invalid escape sequence or was unterminated.
    #[error("malformed string literal at {span}: {reason}")]
    MalformedString {
        /// Description of the problem.
        reason: String,
        /// Start of the malformed literal.
        span: SourceSpan,
    },

    /// A number literal could not be parsed.
    #[error("malformed number literal '{text}' at {span}")]
    MalformedNumber {
        /// The literal text that failed to parse.
        text: String,
        /// Location of the literal.
        span: SourceSpan,
    },

    /// A hit-policy keyword was not recognised.
    #[error("unknown hit policy '{name}'; expected 'unique' or 'first'")]
    UnknownHitPolicy {
        /// The unrecognised keyword.
        name: String,
        /// Location of the keyword.
        span: SourceSpan,
    },

    /// A valid hit-policy keyword was used that is not supported in Profile v0.1.
    #[error("hit policy '{name}' is not supported in Profile v0.1")]
    UnsupportedHitPolicy {
        /// The unsupported keyword (e.g. `collect`, `any`, `rule_order`).
        name: String,
        /// Location of the keyword.
        span: SourceSpan,
    },

    /// The same attribute keyword appeared more than once in a decision.
    #[error("duplicate field '{keyword}' in decision")]
    DuplicateField {
        /// The keyword that was repeated.
        keyword: String,
        /// Location of the second occurrence.
        span: SourceSpan,
    },

    /// A required attribute keyword was absent from a decision.
    #[error("missing required field '{keyword}'")]
    MissingField {
        /// The missing keyword.
        keyword: String,
        /// Location where the field was expected.
        span: SourceSpan,
    },

    /// A set-membership predicate `in ()` had an empty literal list.
    #[error("empty set '()' is not valid in a set-membership predicate at {span}")]
    EmptySet {
        /// Location of the empty `()`.
        span: SourceSpan,
    },

    /// `and` or `or` was given fewer than two predicates.
    #[error("'{combinator}' requires at least two predicates at {span}")]
    TooFewPredicates {
        /// The combinator keyword (`and` or `or`).
        combinator: String,
        /// Location of the combinator form.
        span: SourceSpan,
    },

    /// A wildcard `*` appeared alongside other predicates in a `:when` block.
    #[error("wildcard '*' cannot be mixed with other predicates at {span}")]
    WildcardMixedWithPredicates {
        /// Location of the wildcard.
        span: SourceSpan,
    },

    /// More than one catch-all rule was found in a single decision.
    #[error("multiple catch-all rules in decision; second at {span}, first at {previous}")]
    MultipleCatchAllRules {
        /// Location of the second catch-all.
        span: SourceSpan,
        /// Location of the first catch-all.
        previous: SourceSpan,
    },

    /// A construct that belongs to a later Profile version was encountered.
    #[error("'{name}' is not supported in Profile v0.1 (planned for Profile {profile})")]
    UnsupportedConstruct {
        /// The name of the unsupported construct.
        name: String,
        /// The Profile version where it will be introduced.
        profile: String,
        /// Location of the construct.
        span: SourceSpan,
    },

    /// A source file contains more than one `(define-decision ...)` form.
    ///
    /// Profile v0.1 supports exactly one decision per source file
    /// (`source-file ::= ws* decision ws*`). The first decision is returned
    /// in the partial AST; subsequent decisions are not parsed.
    #[error(
        "source file contains more than one decision; multi-decision sources are not supported in Profile v0.1"
    )]
    MultipleDecisions {
        /// Span of the second (or subsequent) `(define-decision ...)` form.
        span: SourceSpan,
        /// Span of the first decision, for cross-reference in diagnostics.
        first_decision: SourceSpan,
    },
}

// ── Catalogue errors ──────────────────────────────────────────────────────────

/// Errors produced while loading or validating a Sem OS catalogue.
///
/// All fields use owned `String` values (not `std::io::Error` or
/// `toml::de::Error`) so that `CatalogueError` can derive `Clone + PartialEq
/// + Eq` and be embedded in `CompileError`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CatalogueError {
    /// The catalogue file could not be read from disk.
    #[error("failed to read catalogue file '{path}': {message}")]
    Io {
        /// Path to the catalogue file.
        path: String,
        /// OS error message.
        message: String,
    },

    /// The catalogue file is not valid TOML.
    #[error("failed to parse catalogue TOML: {message}")]
    Toml {
        /// TOML parse error message.
        message: String,
    },

    /// A `domain_id` field is not a valid UUIDv7.
    #[error("invalid domain_id '{value}' for domain '{domain_name}': not a valid UUIDv7")]
    InvalidDomainId {
        /// The domain whose ID is malformed.
        domain_name: String,
        /// The raw string that failed to parse.
        value: String,
    },

    /// A `value_id` field is not a valid UUIDv7.
    #[error(
        "invalid value_id '{value}' for symbol '{symbol}' in domain '{domain_name}': not a valid UUIDv7"
    )]
    InvalidValueId {
        /// The domain containing the malformed value.
        domain_name: String,
        /// The value symbol whose ID is malformed.
        symbol: String,
        /// The raw string that failed to parse.
        value: String,
    },

    /// The `snapshot_id` field is not a valid UUIDv7.
    #[error("invalid snapshot_id '{value}': not a valid UUIDv7")]
    InvalidSnapshotId {
        /// The raw string that failed to parse.
        value: String,
    },

    /// Two domains share the same name.
    #[error("duplicate domain name '{name}' in catalogue")]
    DuplicateDomainName {
        /// The repeated domain name.
        name: String,
    },

    /// Two domains share the same `domain_id`.
    #[error(
        "duplicate domain_id '{value}' in catalogue (used by '{first_domain}' and '{second_domain}')"
    )]
    DuplicateDomainId {
        /// The repeated UUID.
        value: String,
        /// Name of the domain that first used this ID.
        first_domain: String,
        /// Name of the domain that reused this ID.
        second_domain: String,
    },

    /// Two values within a domain share the same symbol.
    #[error("duplicate value symbol '{symbol}' in domain '{domain_name}'")]
    DuplicateValueSymbol {
        /// The domain containing the duplicate.
        domain_name: String,
        /// The repeated symbol.
        symbol: String,
    },

    /// Two values within a domain share the same `value_id`.
    #[error(
        "duplicate value_id '{value}' in domain '{domain_name}' (used by '{first_symbol}' and '{second_symbol}')"
    )]
    DuplicateValueId {
        /// The domain containing the duplicate.
        domain_name: String,
        /// The repeated UUID.
        value: String,
        /// Symbol that first used this ID.
        first_symbol: String,
        /// Symbol that reused this ID.
        second_symbol: String,
    },
}

// ── Compile errors ────────────────────────────────────────────────────────────

/// Static semantic errors produced by `dmn-lite-compiler::compile()`.
///
/// Every variant carries a [`SourceSpan`] pointing at the offending AST node.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompileError {
    // ── Catalogue resolution ────────────────────────────────────────────────
    /// A `:domain` reference names a domain not present in the catalogue.
    #[error("unknown domain '{name}'")]
    UnknownDomain {
        /// The unresolved domain name.
        name: String,
        /// Location of the domain reference.
        span: SourceSpan,
    },

    /// An enum literal does not belong to the declared domain.
    #[error("value '{symbol}' is not a member of domain '{domain}'")]
    UnknownDomainValue {
        /// The domain the literal was checked against.
        domain: String,
        /// The unresolved literal symbol.
        symbol: String,
        /// Location of the literal.
        span: SourceSpan,
    },

    // ── Type / domain consistency ───────────────────────────────────────────
    /// An `enum`-typed field is missing its required `:domain` clause.
    #[error("enum-typed field '{field}' requires a :domain clause")]
    MissingDomainOnEnum {
        /// The field name.
        field: String,
        /// Location of the field declaration.
        span: SourceSpan,
    },

    // ── Field reference resolution ──────────────────────────────────────────
    /// A predicate references an input field that was not declared.
    #[error("unknown input field '{name}'")]
    UnknownInputField {
        /// The unresolved field name.
        name: String,
        /// Location of the field reference.
        span: SourceSpan,
    },

    /// An assignment references an output field that was not declared.
    #[error("unknown output field '{name}'")]
    UnknownOutputField {
        /// The unresolved field name.
        name: String,
        /// Location of the field reference.
        span: SourceSpan,
    },

    // ── Duplicate declarations ──────────────────────────────────────────────
    /// Two input fields share the same name.
    #[error("duplicate input field name '{name}'")]
    DuplicateInputField {
        /// The repeated field name.
        name: String,
        /// Location of the second declaration.
        span: SourceSpan,
        /// Location of the first declaration.
        previous: SourceSpan,
    },

    /// Two output fields share the same name.
    #[error("duplicate output field name '{name}'")]
    DuplicateOutputField {
        /// The repeated field name.
        name: String,
        /// Location of the second declaration.
        span: SourceSpan,
        /// Location of the first declaration.
        previous: SourceSpan,
    },

    /// Two rules share the same identifier.
    #[error("duplicate rule identifier '{name}'")]
    DuplicateRuleId {
        /// The repeated rule identifier.
        name: String,
        /// Location of the second rule.
        span: SourceSpan,
        /// Location of the first rule.
        previous: SourceSpan,
    },

    // ── Predicate type checking ─────────────────────────────────────────────
    /// A literal's type does not match the field's declared type.
    #[error(
        "type mismatch in predicate: field '{field}' has type '{field_type}', literal has type '{literal_type}'"
    )]
    PredicateTypeMismatch {
        /// The field being tested.
        field: String,
        /// The field's declared type.
        field_type: String,
        /// The literal's inferred type.
        literal_type: String,
        /// Location of the predicate.
        span: SourceSpan,
    },

    /// An ordered comparison (`<`, `<=`, `>`, `>=`) was used on a non-numeric field.
    #[error(
        "ordered comparison ({op}) is only valid for numeric fields; '{field}' has type '{field_type}'"
    )]
    OrderedComparisonOnNonNumeric {
        /// The field name.
        field: String,
        /// The field's declared type.
        field_type: String,
        /// The comparison operator used.
        op: String,
        /// Location of the predicate.
        span: SourceSpan,
    },

    /// A range predicate was used on a non-numeric field.
    #[error("range predicate is only valid for numeric fields; '{field}' has type '{field_type}'")]
    RangeOnNonNumeric {
        /// The field name.
        field: String,
        /// The field's declared type.
        field_type: String,
        /// Location of the range predicate.
        span: SourceSpan,
    },

    /// A set-membership predicate contains an element that doesn't match the field type.
    #[error(
        "set members must share the type of field '{field}' ({field_type}); element {index} has type '{element_type}'"
    )]
    SetMemberTypeMismatch {
        /// The field name.
        field: String,
        /// The field's declared type.
        field_type: String,
        /// The element's inferred type.
        element_type: String,
        /// Zero-based index of the offending element.
        index: usize,
        /// Location of the offending element.
        span: SourceSpan,
    },

    // ── Assignment type checking ────────────────────────────────────────────
    /// An assignment value's type does not match the output field's declared type.
    #[error(
        "type mismatch in assignment: output '{output}' has type '{output_type}', literal has type '{literal_type}'"
    )]
    AssignmentTypeMismatch {
        /// The output field name.
        output: String,
        /// The field's declared type.
        output_type: String,
        /// The literal's inferred type.
        literal_type: String,
        /// Location of the assignment.
        span: SourceSpan,
    },

    // ── Rule structural checks ──────────────────────────────────────────────
    /// A rule does not assign a value to every declared output field.
    #[error("rule '{rule}' is missing assignment for output '{output}'")]
    MissingOutputAssignment {
        /// The rule identifier.
        rule: String,
        /// The output field that was not assigned.
        output: String,
        /// Span of the rule's `:then` block.
        span: SourceSpan,
    },

    /// A rule assigns the same output field more than once.
    #[error("rule '{rule}' assigns output '{output}' more than once")]
    DuplicateOutputAssignment {
        /// The rule identifier.
        rule: String,
        /// The output field assigned twice.
        output: String,
        /// Location of the second assignment.
        span: SourceSpan,
        /// Location of the first assignment.
        previous: SourceSpan,
    },

    // ── Catch-all rules ─────────────────────────────────────────────────────
    /// The decision contains more than one catch-all rule.
    #[error("decision contains multiple catch-all rules")]
    MultipleCatchAllRules {
        /// Location of the second catch-all.
        span: SourceSpan,
        /// Location of the first catch-all.
        previous: SourceSpan,
    },

    /// Under `FIRST` hit policy, a normal rule follows a catch-all and is unreachable.
    #[error("under FIRST hit policy, rule '{rule}' is unreachable: preceded by a catch-all rule")]
    UnreachableAfterCatchAll {
        /// The unreachable rule's identifier.
        rule: String,
        /// Location of the unreachable rule.
        span: SourceSpan,
        /// Location of the preceding catch-all.
        catch_all: SourceSpan,
    },

    // ── Structural errors ───────────────────────────────────────────────────
    /// A decision declares no input fields.
    #[error("decision must declare at least one input field")]
    EmptyInputs {
        /// Span of the `:inputs` block.
        span: SourceSpan,
    },

    /// A decision declares no output fields.
    #[error("decision must declare at least one output field")]
    EmptyOutputs {
        /// Span of the `:outputs` block.
        span: SourceSpan,
    },

    // ── Catalogue wrapper ────────────────────────────────────────────────────
    /// A catalogue-level error propagated into compilation.
    #[error("catalogue error: {source}")]
    Catalogue {
        /// The underlying catalogue error.
        #[from]
        source: CatalogueError,
    },
}

// ── Compile warnings ──────────────────────────────────────────────────────────

/// Non-fatal diagnostics produced by `dmn-lite-compiler::compile()`.
///
/// A compile succeeds even when warnings are present. Warnings are surfaced
/// through `CompileErrors::warnings` so callers can decide whether to surface
/// them to authors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileWarning {
    /// A non-enum field declares a `:domain` clause.
    ///
    /// In Profile v0.1, domain references on `bool`, `integer`, `decimal`, and
    /// `string` fields are resolved (the domain must exist in the catalogue) but
    /// value-level membership is not enforced. The domain reference is preserved
    /// on the `FieldSchema` for future profile compatibility. This warning alerts
    /// authors that the domain is advisory, not enforced.
    DomainOnNonEnum {
        /// The field name.
        field: String,
        /// The field's type keyword.
        type_name: String,
        /// The domain name as written.
        domain: String,
        /// Location of the `:domain` clause.
        span: SourceSpan,
    },

    /// A decision declares no rules.
    ///
    /// A decision with no rules always returns `NoMatch` regardless of input.
    EmptyRules {
        /// Span of the `:rules` block.
        span: SourceSpan,
    },
}

// ── Evaluation errors ─────────────────────────────────────────────────────────

/// Runtime evaluation errors produced by `dmn-lite-engine`.
///
/// `NoMatch` and `MultipleMatches` represent hit-policy outcomes that are
/// errors within the evaluator. Callers convert them to domain-level outcomes
/// (e.g., `DecisionOutcome::NoMatch`) at the invocation boundary (V&S §11.4).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvalError {
    /// No rule matched under UNIQUE or FIRST hit policy.
    #[error("no rule matched")]
    NoMatch,

    /// UNIQUE hit policy found multiple matching rules.
    #[error("UNIQUE hit policy matched multiple rules: {rules:?}")]
    MultipleMatches {
        /// All matching rule IDs in source order.
        rules: Vec<crate::ids::RuleId>,
    },

    /// The input slot count does not match the decision's input schema arity.
    #[error("input slot count {actual} does not match decision input schema arity {expected}")]
    InputSchemaMismatch {
        /// Expected (schema arity).
        expected: usize,
        /// Actual (slots provided).
        actual: usize,
    },

    /// An input value's runtime type does not match the field's declared schema type.
    #[error(
        "input field '{field}' (FieldId {field_id}) has type '{actual}' but schema expects '{expected}'"
    )]
    InputTypeMismatch {
        /// Field name.
        field: String,
        /// Field ordinal.
        field_id: crate::ids::FieldId,
        /// Expected type name.
        expected: String,
        /// Actual type name.
        actual: String,
    },

    /// An enum input value's domain does not match the field's declared domain.
    #[error(
        "input field '{field}' (FieldId {field_id}) value has domain mismatch; expected domain '{domain}'"
    )]
    InputDomainMismatch {
        /// Field name.
        field: String,
        /// Field ordinal.
        field_id: crate::ids::FieldId,
        /// Expected domain ID (as string).
        domain: String,
        /// Symbol or value description provided (best-effort).
        symbol: String,
    },

    /// The `TypedInputContext` was built against a different schema than the decision.
    #[error(
        "schema hash mismatch: input context was built for a different schema than the decision"
    )]
    SchemaHashMismatch,
}
