//! Category 7 — Rejection of out-of-profile constructs.
//! Parser must reject Profile v0.2+ constructs with UnsupportedConstruct or
//! UnsupportedHitPolicy errors (not silently accept them).

use dmn_lite_parser::{ParseError, parse};

fn has_error<F: Fn(&ParseError) -> bool>(src: &str, pred: F) -> bool {
    match parse(src) {
        Ok(_) => false,
        Err(e) => e.errors.iter().any(pred),
    }
}

// ── Hit-policy rejections ─────────────────────────────────────────────────────

#[test]
fn test_reject_hit_policy_collect() {
    let src = "(define-decision d :hit-policy collect :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::UnsupportedHitPolicy { name, .. } if name == "collect"
    )));
}

#[test]
fn test_reject_hit_policy_any() {
    let src = "(define-decision d :hit-policy any :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::UnsupportedHitPolicy { .. }
    )));
}

#[test]
fn test_reject_hit_policy_rule_order() {
    let src = "(define-decision d :hit-policy rule_order :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::UnsupportedHitPolicy { .. }
    )));
}

#[test]
fn test_unknown_hit_policy_is_unknown_not_unsupported() {
    let src = "(define-decision d :hit-policy whatever :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::UnknownHitPolicy { .. }
    )));
}

// ── Multi-decision source files ───────────────────────────────────────────────

#[test]
fn test_reject_multi_decision_source() {
    let src = r#"
(define-decision d1 :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))
(define-decision d2 :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))
"#;
    // Multi-decision sources now emit MultipleDecisions (not UnsupportedConstruct).
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::MultipleDecisions { .. }
    )));
}

// ── Type keyword rejections ───────────────────────────────────────────────────

#[test]
fn test_reject_type_keyword_boolean() {
    // `boolean` is not valid — should be `bool`
    let src = "(define-decision d :hit-policy unique :inputs ((x :type boolean :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(
        src,
        |e| matches!(e, ParseError::UnexpectedToken { found, .. } if found.contains("boolean"))
    ));
}

#[test]
fn test_reject_type_keyword_number() {
    // `number` is not valid — should be `integer` or `decimal`
    let src = "(define-decision d :hit-policy unique :inputs ((x :type number :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    assert!(has_error(
        src,
        |e| matches!(e, ParseError::UnexpectedToken { found, .. } if found.contains("number"))
    ));
}

// ── Wildcard restrictions ─────────────────────────────────────────────────────

#[test]
fn test_reject_wildcard_mixed_with_predicates() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x = 1) *) :then ((y = 1)))))";
    assert!(has_error(src, |e| matches!(
        e,
        ParseError::WildcardMixedWithPredicates { .. }
    )));
}
