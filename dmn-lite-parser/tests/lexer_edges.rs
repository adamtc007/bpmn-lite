//! Category 5 — Lexer edge cases.
//! Tests targeting tokenisation corner cases independent of higher-level grammar.

use dmn_lite_parser::{ParseError, parse};

fn mk_simple(pred_body: &str) -> String {
    format!(
        "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (({pred_body})) :then ((y = 1)))))"
    )
}

fn mk_string_id(id: &str) -> String {
    format!(
        "(define-decision d :decision-id {id} :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))"
    )
}

fn has_error<F: Fn(&ParseError) -> bool>(src: &str, pred: F) -> bool {
    match parse(src) {
        Ok(_) => false,
        Err(e) => e.errors.iter().any(pred),
    }
}

// 1. Empty input
#[test]
fn test_empty_input() {
    let err = parse("").unwrap_err();
    // Should produce MissingField or UnexpectedEof — not panic
    assert!(!err.errors.is_empty());
}

// 2. Whitespace-only input
#[test]
fn test_whitespace_only_input() {
    let err = parse("   \n\t  ").unwrap_err();
    assert!(!err.errors.is_empty());
}

// 3. Comments-only input
#[test]
fn test_comments_only_input() {
    let err = parse("; this is a comment\n; another comment").unwrap_err();
    assert!(!err.errors.is_empty());
}

// 4. String with \" escape
#[test]
fn test_string_escaped_quote() {
    let src = mk_string_id(r#""he said \"hello\"""#);
    let d = parse(&src).unwrap().decisions.into_iter().next().unwrap();
    assert_eq!(d.decision_id.unwrap().value, r#"he said "hello""#);
}

// 5. String with \\ escape
#[test]
fn test_string_escaped_backslash() {
    let src = mk_string_id(r#""c:\\path""#);
    let d = parse(&src).unwrap().decisions.into_iter().next().unwrap();
    assert_eq!(d.decision_id.unwrap().value, r"c:\path");
}

// 6. String with invalid \n escape → MalformedString
#[test]
fn test_string_invalid_escape() {
    let src = mk_string_id(r#""\n""#);
    assert!(has_error(&src, |e| matches!(
        e,
        ParseError::MalformedString { .. }
    )));
}

// 7. String with Unicode
#[test]
fn test_string_unicode() {
    let src = mk_string_id("\"café\"");
    let d = parse(&src).unwrap().decisions.into_iter().next().unwrap();
    assert_eq!(d.decision_id.unwrap().value, "café");
}

// 8. Negative integer literal
#[test]
fn test_negative_integer_literal() {
    use dmn_lite_parser::{NumberKind, PredicateAst, WhenAst};
    let src = mk_simple("x > -42");
    let d = parse(&src).unwrap().decisions.into_iter().next().unwrap();
    let WhenAst::Predicates(preds, _) = &d.rules[0].when else {
        panic!()
    };
    let PredicateAst::Gt { value, .. } = &preds[0] else {
        panic!()
    };
    assert_eq!(value.text, "-42");
    assert_eq!(value.kind, NumberKind::Integer);
}

// 9. Decimal literal
#[test]
fn test_decimal_literal() {
    use dmn_lite_parser::{NumberKind, PredicateAst, WhenAst};
    let src = mk_simple("x > 3.14");
    let d = parse(&src).unwrap().decisions.into_iter().next().unwrap();
    let WhenAst::Predicates(preds, _) = &d.rules[0].when else {
        panic!()
    };
    let PredicateAst::Gt { value, .. } = &preds[0] else {
        panic!()
    };
    assert_eq!(value.text, "3.14");
    assert_eq!(value.kind, NumberKind::Decimal);
}

// 10. Negative decimal literal
#[test]
fn test_negative_decimal_literal() {
    use dmn_lite_parser::{NumberKind, PredicateAst, WhenAst};
    let src = mk_simple("x > -3.14");
    let d = parse(&src).unwrap().decisions.into_iter().next().unwrap();
    let WhenAst::Predicates(preds, _) = &d.rules[0].when else {
        panic!()
    };
    let PredicateAst::Gt { value, .. } = &preds[0] else {
        panic!()
    };
    assert_eq!(value.text, "-3.14");
    assert_eq!(value.kind, NumberKind::Decimal);
}

// 11. Adjacent tokens with no whitespace separator
#[test]
fn test_adjacent_tokens_no_whitespace() {
    // `(x=ACTIVE)` with no whitespace should still tokenise correctly
    let src = "(define-decision d :hit-policy unique :inputs ((x :type enum :domain S)) :outputs ((y :type enum :domain S)) :rules ((rule r1 :when ((x=ACTIVE)) :then ((y=OK)))))";
    let d = parse(src).unwrap().decisions.into_iter().next().unwrap();
    assert_eq!(d.rules[0].then[0].output.name, "y");
}

// 12. Comment at end of source with no trailing newline
#[test]
fn test_comment_at_eof_no_newline() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1))))) ; final comment";
    // Must not panic or crash
    let result = parse(src);
    assert!(
        result.is_ok(),
        "comment at EOF should not cause a parse error: {result:?}"
    );
}

// 13. Symbol with dot (e.g., booking_eligibility.v1 as field or id)
#[test]
fn test_symbol_with_dot_allowed() {
    let src = "(define-decision booking.v1 :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    let d = parse(src).unwrap().decisions.into_iter().next().unwrap();
    assert_eq!(d.name.name, "booking.v1");
}

// 14. `..` does not get absorbed into a symbol
#[test]
fn test_dotdot_not_absorbed_into_symbol() {
    use dmn_lite_parser::{PredicateAst, RangeBound, WhenAst};
    let src = "(define-decision d :hit-policy unique :inputs ((age :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((age in [1..10])) :then ((y = 1)))))";
    let d = parse(src).unwrap().decisions.into_iter().next().unwrap();
    let WhenAst::Predicates(preds, _) = &d.rules[0].when else {
        panic!()
    };
    let PredicateAst::Range { lower, upper, .. } = &preds[0] else {
        panic!("expected range")
    };
    assert!(matches!(lower, RangeBound::Value(n) if n.text == "1"));
    assert!(matches!(upper, RangeBound::Value(n) if n.text == "10"));
}
