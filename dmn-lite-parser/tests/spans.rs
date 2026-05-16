//! Category 2 — Source span correctness tests.
//! Verify that every AST node's span covers the exact bytes it should.

use dmn_lite_parser::{PredicateAst, WhenAst, parse};

fn span_text(src: &str, start: u32, end: u32) -> &str {
    &src[start as usize..end as usize]
}

#[test]
fn test_decision_span_covers_full_form() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    let ast = parse(src).unwrap();
    let d = &ast.decisions[0];
    // Span must start at `(` and end at closing `)`
    assert_eq!(span_text(src, d.span.start, d.span.end), src.trim());
}

#[test]
fn test_decision_name_span() {
    let src = "(define-decision my-decision :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    let d = &parse(src).unwrap().decisions[0];
    assert_eq!(
        span_text(src, d.name.span.start, d.name.span.end),
        "my-decision"
    );
}

#[test]
fn test_hit_policy_span() {
    let src = "(define-decision d :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    let d = &parse(src).unwrap().decisions[0];
    let hp_span = d.hit_policy.span();
    assert_eq!(span_text(src, hp_span.start, hp_span.end), "first");
}

#[test]
fn test_input_decl_span_covers_parens() {
    let src = "(define-decision d :hit-policy unique :inputs ((age :type integer :domain AgeYears)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    let d = &parse(src).unwrap().decisions[0];
    let decl_text = span_text(src, d.inputs[0].span.start, d.inputs[0].span.end);
    // Must include opening and closing parens
    assert!(decl_text.starts_with('('));
    assert!(decl_text.ends_with(')'));
    assert!(decl_text.contains("age"));
}

#[test]
fn test_input_name_span_no_whitespace() {
    let src = "(define-decision d :hit-policy unique :inputs ((  age  :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    let d = &parse(src).unwrap().decisions[0];
    let name_text = span_text(src, d.inputs[0].name.span.start, d.inputs[0].name.span.end);
    // Span must be exactly the symbol with no surrounding whitespace
    assert_eq!(name_text, "age");
}

#[test]
fn test_rule_span_covers_full_rule() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))";
    let d = &parse(src).unwrap().decisions[0];
    let r = &d.rules[0];
    let rule_text = span_text(src, r.span.start, r.span.end);
    assert!(rule_text.starts_with("(rule"));
    assert!(rule_text.ends_with(')'));
}

#[test]
fn test_predicate_span_covers_parens() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x = 5)) :then ((y = 1)))))";
    let d = &parse(src).unwrap().decisions[0];
    let WhenAst::Predicates(preds, _) = &d.rules[0].when else {
        panic!()
    };
    let p_text = span_text(src, preds[0].span().start, preds[0].span().end);
    assert!(p_text.starts_with('('));
    assert!(p_text.ends_with(')'));
    assert!(p_text.contains("x = 5"));
}

#[test]
fn test_literal_symbol_span_no_whitespace() {
    let src = "(define-decision d :hit-policy unique :inputs ((status :type enum :domain S)) :outputs ((y :type enum :domain S)) :rules ((rule r1 :when ((status =  ACTIVE )) :then ((y = OK)))))";
    let d = &parse(src).unwrap().decisions[0];
    let WhenAst::Predicates(preds, _) = &d.rules[0].when else {
        panic!()
    };
    let PredicateAst::Eq { value, .. } = &preds[0] else {
        panic!()
    };
    let lit_span = value.span();
    assert_eq!(span_text(src, lit_span.start, lit_span.end), "ACTIVE");
}

#[test]
fn test_string_literal_span_includes_quotes() {
    let src = r#"(define-decision d :decision-id "my.id" :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when (*) :then ((y = 1)))))"#;
    let d = &parse(src).unwrap().decisions[0];
    let id = d.decision_id.as_ref().unwrap();
    let id_text = span_text(src, id.span.start, id.span.end);
    // Span includes the surrounding double quotes
    assert_eq!(id_text, "\"my.id\"");
    assert_eq!(id.value, "my.id");
}

#[test]
fn test_parent_span_covers_children() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x = 5)) :then ((y = 1)))))";
    let d = &parse(src).unwrap().decisions[0];
    let r = &d.rules[0];
    // Rule span must cover the when span
    let WhenAst::Predicates(_, when_span) = &r.when else {
        panic!()
    };
    assert!(r.span.start <= when_span.start);
    assert!(r.span.end >= when_span.end);
}

#[test]
fn test_rule_id_span() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule my-rule :when (*) :then ((y = 1)))))";
    let d = &parse(src).unwrap().decisions[0];
    assert_eq!(
        span_text(src, d.rules[0].id.span.start, d.rules[0].id.span.end),
        "my-rule"
    );
}

#[test]
fn test_negative_number_span() {
    let src = "(define-decision d :hit-policy unique :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r1 :when ((x > -42)) :then ((y = 1)))))";
    let d = &parse(src).unwrap().decisions[0];
    let WhenAst::Predicates(preds, _) = &d.rules[0].when else {
        panic!()
    };
    let PredicateAst::Gt { value, .. } = &preds[0] else {
        panic!()
    };
    assert_eq!(span_text(src, value.span.start, value.span.end), "-42");
}
