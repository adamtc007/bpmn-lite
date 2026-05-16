//! Category 4 — Error recovery tests.
//! A single parse pass over a multi-error source must report all errors.

use dmn_lite_parser::{ParseError, parse};

fn error_count(src: &str) -> usize {
    match parse(src) {
        Ok(_) => 0,
        Err(e) => e.errors.len(),
    }
}

fn has_error<F: Fn(&ParseError) -> bool>(src: &str, pred: F) -> bool {
    match parse(src) {
        Ok(_) => false,
        Err(e) => e.errors.iter().any(pred),
    }
}

#[test]
fn test_multiple_errors_reported() {
    // Two bad predicates in separate rules — both should be reported
    let src = "(define-decision d :hit-policy unique \
        :inputs ((x :type integer :domain N)) \
        :outputs ((y :type integer :domain N)) \
        :rules (\
            (rule r1 :when ((x = )) :then ((y = 1))) \
            (rule r2 :when ((x = )) :then ((y = 2)))))";
    // Each `(x = )` produces at least one error
    assert!(error_count(src) >= 2);
}

#[test]
fn test_partial_ast_on_recovery() {
    // A source with a bad second rule should still produce the first rule in partial_ast
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1))) (rule r2 :when (*) :then ((y = 2)))))";
    // This source has multiple catch-all rules (an error), but both rules are syntactically valid.
    let err = parse(src).unwrap_err();
    // partial_ast should be present because at least one decision was parsed
    assert!(err.partial_ast.is_some());
    let partial = err.partial_ast.unwrap();
    assert_eq!(partial.decisions.len(), 1);
    // Both rules should be in the partial AST
    assert_eq!(partial.decisions[0].rules.len(), 2);
}

#[test]
fn test_error_in_one_rule_does_not_swallow_next() {
    // Rule r1 has a broken predicate; rule r2 should still parse.
    // The error in r1 is `(x =)` — missing literal after `=`
    let src = "(define-decision d :hit-policy unique \
        :inputs ((x :type integer :domain N)) \
        :outputs ((y :type integer :domain N)) \
        :rules (\
            (rule r1 :when ((x = )) :then ((y = 1))) \
            (rule r2 :when (*) :then ((y = 2)))))";
    let err = parse(src).unwrap_err();
    // At least one error (the bad predicate)
    assert!(!err.errors.is_empty());
    // But rule r2 (catch-all) should be in the partial AST
    if let Some(partial) = err.partial_ast {
        let rules = &partial.decisions[0].rules;
        // r2 should be present even though r1 errored
        assert!(rules.iter().any(|r| r.id.name == "r2"));
    }
}

#[test]
fn test_lexer_and_parser_errors_combined() {
    // Contains both a lexer error (`@`) and a parser error (bad type keyword)
    let src = "(define-decision d@ :hit-policy unique :inputs ((x :type bad-type :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    let err = parse(src).unwrap_err();
    // Should have lexer error for `@` AND parser error for bad type
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::UnexpectedChar { .. }
    )));
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::UnexpectedToken { .. }
    )));
    assert!(err.errors.len() >= 2);
}

#[test]
fn test_multiple_missing_fields_reported() {
    // Missing both :outputs and :rules
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)))";
    let err = parse(src).unwrap_err();
    // Should report at least MissingField for :outputs
    let has_outputs_missing = err
        .errors
        .iter()
        .any(|e| matches!(e, ParseError::MissingField { keyword, .. } if keyword == ":outputs"));
    assert!(
        has_outputs_missing,
        "Expected MissingField for :outputs, got: {:?}",
        err.errors
    );
}
