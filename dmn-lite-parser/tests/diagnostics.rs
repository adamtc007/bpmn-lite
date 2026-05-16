//! Category 3 — Diagnostic tests: one test per ParseError variant.
//! Each test constructs a source that produces exactly that error.

use dmn_lite_parser::{ParseError, parse};

fn first_error(src: &str) -> ParseError {
    parse(src).unwrap_err().errors.into_iter().next().unwrap()
}

fn has_error<F: Fn(&ParseError) -> bool>(src: &str, pred: F) -> bool {
    parse(src).unwrap_err().errors.iter().any(pred)
}

#[test]
fn test_unexpected_char() {
    // `@` is not valid anywhere
    let e = first_error("@");
    assert!(
        matches!(e, ParseError::UnexpectedChar { ch: '@', .. }),
        "got {e:?}"
    );
}

#[test]
fn test_unexpected_eof() {
    // opening `(` but nothing after
    let e = first_error("(");
    assert!(matches!(e, ParseError::UnexpectedEof { .. }), "got {e:?}");
}

#[test]
fn test_unexpected_token_wrong_keyword() {
    // `(define-something …)` — not `define-decision`
    let e = first_error("(define-something foo)");
    assert!(matches!(e, ParseError::UnexpectedToken { .. }), "got {e:?}");
}

#[test]
fn test_malformed_string_invalid_escape() {
    let src = r#"(define-decision d :decision-id "\n" :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))"#;
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::MalformedString { .. }
    )));
}

#[test]
fn test_malformed_string_unterminated() {
    let src = r#"(define-decision d :decision-id "oops :hit-policy unique :inputs () :outputs () :rules ())"#;
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::MalformedString { .. }
    )));
}

#[test]
fn test_unknown_hit_policy() {
    let src = "(define-decision d :hit-policy mystery :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::UnknownHitPolicy { name, .. } if name == "mystery"
    )));
}

#[test]
fn test_unsupported_hit_policy_collect() {
    let src = "(define-decision d :hit-policy collect :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::UnsupportedHitPolicy { name, .. } if name == "collect"
    )));
}

#[test]
fn test_unsupported_hit_policy_any() {
    let src = "(define-decision d :hit-policy any :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::UnsupportedHitPolicy { .. }
    )));
}

#[test]
fn test_missing_hit_policy() {
    // Jump straight to :inputs
    let src = "(define-decision d :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::MissingField { keyword, .. } if keyword == ":hit-policy"
    )));
}

#[test]
fn test_missing_inputs() {
    let src = "(define-decision d :hit-policy unique :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::MissingField { keyword, .. } if keyword == ":inputs"
    )));
}

#[test]
fn test_missing_outputs() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :rules ((rule r1 :when (*) :then ((x = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::MissingField { keyword, .. } if keyword == ":outputs"
    )));
}

#[test]
fn test_empty_set_error() {
    let src = "(define-decision d :hit-policy unique :inputs ((status :type enum :domain S)) :outputs ((y :type enum :domain S)) :rules ((rule r1 :when ((status in ())) :then ((y = A)))))";
    assert!(has_error(src, |e| matches!(e, ParseError::EmptySet { .. })));
}

#[test]
fn test_too_few_predicates_and() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((and (x = 1))) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::TooFewPredicates { combinator, .. } if combinator == "and"
    )));
}

#[test]
fn test_too_few_predicates_or() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((or (x = 1))) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::TooFewPredicates { combinator, .. } if combinator == "or"
    )));
}

#[test]
fn test_wildcard_mixed_with_predicates() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (* (x = 1)) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::WildcardMixedWithPredicates { .. }
    )));
}

#[test]
fn test_multiple_catch_all_rules() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1))) (rule r2 :when (*) :then ((y = 2)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::MultipleCatchAllRules { .. }
    )));
}

#[test]
fn test_multiple_decisions_error() {
    // MultipleDecisions replaces the old UnsupportedConstruct path for two decisions.
    let src = r#"
(define-decision d1 :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))
(define-decision d2 :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))
"#;
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::MultipleDecisions { .. }
    )));
}

#[test]
fn test_span_points_at_error_location() {
    let src = "  @foo";
    let e = first_error(src);
    if let ParseError::UnexpectedChar { span, .. } = e {
        // The `@` is at byte offset 2
        assert_eq!(span.start, 2);
    } else {
        panic!("expected UnexpectedChar, got {e:?}");
    }
}
