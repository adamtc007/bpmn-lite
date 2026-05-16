//! Arity enforcement tests — Phase 1.1a §3.2.
//!
//! Profile v0.1 requires exactly one decision per source file
//! (`source-file ::= ws* decision ws*`). These tests verify that the parser
//! enforces arity-1 at parse time, producing the correct diagnostics and
//! partial-AST shapes.

use dmn_lite_parser::{ParseError, parse};

const DECISION: &str = "(define-decision d :hit-policy unique \
    :inputs ((x :type integer :domain N)) \
    :outputs ((y :type integer :domain N)) \
    :rules ((rule r1 :when (*) :then ((y = 1)))))";

/// Build a source string with `n` identical valid decisions back-to-back.
fn n_decisions(n: usize) -> String {
    std::iter::repeat_n(DECISION, n)
        .collect::<Vec<_>>()
        .join("\n")
}

// 1. Single decision parses cleanly — already covered by happy-path tests,
//    but included here as an explicit arity-contract anchor.
#[test]
fn test_single_decision_succeeds() {
    let result = parse(DECISION);
    assert!(result.is_ok(), "single decision must parse without errors");
    let ast = result.unwrap();
    assert_eq!(ast.decisions.len(), 1);
    assert_eq!(ast.decisions[0].name.name, "d");
}

// 2. Two decisions → MultipleDecisions error pointing at the second form.
#[test]
fn test_two_decisions_emits_multiple_decisions_error() {
    let src = n_decisions(2);
    let err = parse(&src).unwrap_err();

    let multiple = err
        .errors
        .iter()
        .find(|e| matches!(e, ParseError::MultipleDecisions { .. }));
    assert!(
        multiple.is_some(),
        "expected MultipleDecisions error, got: {:?}",
        err.errors
    );

    // Verify spans: second form starts after the first decision.
    if let Some(ParseError::MultipleDecisions {
        span,
        first_decision,
    }) = multiple
    {
        // first_decision.start is before span.start (second decision is later)
        assert!(
            first_decision.start < span.start,
            "first_decision span ({first_decision}) must precede second decision span ({span})"
        );
    }
}

// 3. Three decisions → exactly one MultipleDecisions error (second form only).
#[test]
fn test_three_decisions_emits_single_error() {
    let src = n_decisions(3);
    let err = parse(&src).unwrap_err();

    let multiple_errors: Vec<_> = err
        .errors
        .iter()
        .filter(|e| matches!(e, ParseError::MultipleDecisions { .. }))
        .collect();

    assert_eq!(
        multiple_errors.len(),
        1,
        "expected exactly one MultipleDecisions error, got {}: {:?}",
        multiple_errors.len(),
        err.errors
    );
}

// 4. Two decisions → partial_ast contains only the first decision.
#[test]
fn test_two_decisions_returns_partial_ast() {
    let src = n_decisions(2);
    let err = parse(&src).unwrap_err();

    assert!(
        err.partial_ast.is_some(),
        "partial_ast must be Some when first decision succeeded"
    );
    let partial = err.partial_ast.unwrap();
    assert_eq!(
        partial.decisions.len(),
        1,
        "partial_ast must contain exactly the first decision"
    );
    assert_eq!(partial.decisions[0].name.name, "d");
}

// 5. Valid decision followed by garbage → UnexpectedToken, not MultipleDecisions.
#[test]
fn test_decision_followed_by_garbage() {
    let src = format!("{DECISION} (foo bar)");
    let err = parse(&src).unwrap_err();

    // Must not produce MultipleDecisions
    let has_multiple = err
        .errors
        .iter()
        .any(|e| matches!(e, ParseError::MultipleDecisions { .. }));
    assert!(
        !has_multiple,
        "trailing garbage must produce UnexpectedToken, not MultipleDecisions"
    );

    let has_unexpected = err
        .errors
        .iter()
        .any(|e| matches!(e, ParseError::UnexpectedToken { .. }));
    assert!(
        has_unexpected,
        "expected UnexpectedToken for trailing garbage, got: {:?}",
        err.errors
    );
}

// 6. Valid decision followed by whitespace and comments only → no errors.
#[test]
fn test_decision_followed_by_comment_only() {
    let src = format!("{DECISION}\n; trailing comment\n  ; another\n");
    let result = parse(&src);
    assert!(
        result.is_ok(),
        "trailing comments and whitespace must not produce errors: {:?}",
        result.unwrap_err().errors
    );
}
