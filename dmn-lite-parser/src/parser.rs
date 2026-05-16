//! Recursive-descent parser for the dmn-lite s-expression DSL.
//!
//! Each non-terminal in the EBNF (`docs/dmn-lite-ebnf.md`) maps to one
//! `parse_*` function. Every function that can fail returns `Option<T>`.
//! Errors are collected in `Parser::errors` rather than short-circuiting.

use dmn_lite_types::{
    ParseError,
    ast::{
        AssignmentAst, DecisionAst, HitPolicyAst, InputDeclAst, LiteralAst, NumberLitAst,
        OutputDeclAst, PredicateAst, RangeBound, RuleAst, Source, StringLitAst, SymbolAst,
        TypeRefAst, WhenAst,
    },
    ids::SourceSpan,
};

use crate::lexer::{Token, TokenKind, token_to_number_lit, token_to_string_lit, token_to_symbol};

// ── Public entry point ────────────────────────────────────────────────────────

pub(crate) struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    pub(crate) errors: Vec<ParseError>,
}

impl Parser {
    pub(crate) fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
        }
    }

    pub(crate) fn parse_source(&mut self) -> Source {
        let start = self.peek().span.start;
        let mut decisions = Vec::new();

        if !self.peek_is_eof()
            && let Some(d) = self.parse_decision()
        {
            decisions.push(d);
        }

        // Profile v0.1: exactly one decision per source file.
        // Distinguish a second (define-decision ...) from trailing garbage.
        if !self.peek_is_eof() {
            let span = self.peek().span;
            let first_span = decisions
                .first()
                .map(|d| d.span)
                .unwrap_or(SourceSpan::new(start, start));
            let is_second_decision = matches!(self.peek().kind, TokenKind::LParen)
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Symbol(s)) if s == "define-decision"
                );
            if is_second_decision {
                self.errors.push(ParseError::MultipleDecisions {
                    span,
                    first_decision: first_span,
                });
            } else {
                self.errors.push(ParseError::UnexpectedToken {
                    expected: "end of input after decision".into(),
                    found: self.peek().kind.description(),
                    span,
                });
            }
        }

        // Empty source (no decisions, no other errors) must still be an error.
        if decisions.is_empty() && self.errors.is_empty() {
            let span = self.peek().span;
            self.errors.push(ParseError::UnexpectedEof {
                expected: "'(define-decision ...)'".into(),
                span,
            });
        }

        let end = self.peek().span.end;
        Source {
            decisions,
            span: SourceSpan::new(start, end),
        }
    }

    pub(crate) fn into_errors(self) -> Vec<ParseError> {
        self.errors
    }
}

// ── Decision ──────────────────────────────────────────────────────────────────

impl Parser {
    fn parse_decision(&mut self) -> Option<DecisionAst> {
        let open = self.expect_lparen("'(define-decision ...'")?;
        let start = open.span.start;

        self.expect_symbol_named("define-decision")?;
        let name = self.expect_symbol("decision name")?;

        let mut decision_id: Option<StringLitAst> = None;
        let mut hit_policy: Option<HitPolicyAst> = None;
        let mut inputs: Vec<InputDeclAst> = Vec::new();
        let mut outputs: Vec<OutputDeclAst> = Vec::new();
        let mut rules: Vec<RuleAst> = Vec::new();

        // Optional :decision-id before :hit-policy
        if self.peek_keyword(":decision-id") {
            self.bump();
            decision_id = self.parse_string_lit();
        }

        // Required :hit-policy
        if !self.peek_keyword(":hit-policy") {
            let span = self.peek().span;
            self.errors.push(ParseError::MissingField {
                keyword: ":hit-policy".into(),
                span,
            });
            self.recover_to_keyword_or_rparen();
        }
        if self.peek_keyword(":hit-policy") {
            self.bump();
            hit_policy = self.parse_hit_policy_kind();
        }

        // Required :inputs
        if !self.peek_keyword(":inputs") {
            let span = self.peek().span;
            self.errors.push(ParseError::MissingField {
                keyword: ":inputs".into(),
                span,
            });
            self.recover_to_keyword_or_rparen();
        }
        if self.peek_keyword(":inputs") {
            self.bump();
            inputs = self.parse_inputs_block();
        }

        // Required :outputs
        if !self.peek_keyword(":outputs") {
            let span = self.peek().span;
            self.errors.push(ParseError::MissingField {
                keyword: ":outputs".into(),
                span,
            });
            self.recover_to_keyword_or_rparen();
        }
        if self.peek_keyword(":outputs") {
            self.bump();
            outputs = self.parse_outputs_block();
        }

        // Required :rules
        if !self.peek_keyword(":rules") {
            let span = self.peek().span;
            self.errors.push(ParseError::MissingField {
                keyword: ":rules".into(),
                span,
            });
            self.recover_to_paren_close(1);
        } else {
            self.bump();
            rules = self.parse_rules_block();
        }

        let close = self.expect_rparen("end of decision")?;
        let span = SourceSpan::new(start, close.span.end);
        // Use a dummy hit-policy span if none was parsed (error already emitted).
        let hit_policy = hit_policy.unwrap_or(HitPolicyAst::Unique(span));

        Some(DecisionAst {
            name,
            decision_id,
            hit_policy,
            inputs,
            outputs,
            rules,
            span,
        })
    }
}

// ── Hit policy ────────────────────────────────────────────────────────────────

impl Parser {
    fn parse_hit_policy_kind(&mut self) -> Option<HitPolicyAst> {
        let span = self.peek().span;
        match self.peek().kind.clone() {
            TokenKind::Symbol(name) => {
                self.bump();
                match name.as_str() {
                    "unique" => Some(HitPolicyAst::Unique(span)),
                    "first" => Some(HitPolicyAst::First(span)),
                    "collect" | "any" | "rule_order" | "rule-order" => {
                        self.errors
                            .push(ParseError::UnsupportedHitPolicy { name, span });
                        None
                    }
                    other => {
                        self.errors.push(ParseError::UnknownHitPolicy {
                            name: other.to_owned(),
                            span,
                        });
                        None
                    }
                }
            }
            _ => {
                self.errors.push(ParseError::UnexpectedToken {
                    expected: "hit policy keyword ('unique' or 'first')".into(),
                    found: self.peek().kind.description(),
                    span,
                });
                None
            }
        }
    }
}

// ── Field declarations ────────────────────────────────────────────────────────

impl Parser {
    fn parse_inputs_block(&mut self) -> Vec<InputDeclAst> {
        if self.expect_lparen("'(' for :inputs").is_none() {
            return Vec::new();
        }
        let mut decls = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            match self.parse_field_decl_inner() {
                Some((name, type_ref, domain_ref, span)) => {
                    decls.push(InputDeclAst {
                        name,
                        type_ref,
                        domain_ref,
                        span,
                    });
                }
                None => {
                    self.recover_past_current_paren_close();
                }
            }
        }
        let _ = self.expect_rparen("')' to close :inputs");
        decls
    }

    fn parse_outputs_block(&mut self) -> Vec<OutputDeclAst> {
        if self.expect_lparen("'(' for :outputs").is_none() {
            return Vec::new();
        }
        let mut decls = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            match self.parse_field_decl_inner() {
                Some((name, type_ref, domain_ref, span)) => {
                    decls.push(OutputDeclAst {
                        name,
                        type_ref,
                        domain_ref,
                        span,
                    });
                }
                None => {
                    self.recover_past_current_paren_close();
                }
            }
        }
        let _ = self.expect_rparen("')' to close :outputs");
        decls
    }

    /// Common inner parse for both input and output declarations.
    /// Returns `(name, type_ref, domain_ref, span)`.
    fn parse_field_decl_inner(&mut self) -> Option<(SymbolAst, TypeRefAst, SymbolAst, SourceSpan)> {
        let open = self.expect_lparen("'(' to open field declaration")?;
        let start = open.span.start;

        let name = self.expect_symbol("field name")?;
        self.expect_symbol_named(":type")?;
        let type_ref = self.parse_type_ref()?;
        self.expect_symbol_named(":domain")?;
        let domain_ref = self.expect_symbol("domain name")?;

        let close = self.expect_rparen("')' to close field declaration")?;
        let span = SourceSpan::new(start, close.span.end);
        Some((name, type_ref, domain_ref, span))
    }

    fn parse_type_ref(&mut self) -> Option<TypeRefAst> {
        let span = self.peek().span;
        match self.peek().kind.clone() {
            TokenKind::Symbol(name) => {
                self.bump();
                match name.as_str() {
                    "enum" => Some(TypeRefAst::Enum(span)),
                    "bool" => Some(TypeRefAst::Bool(span)),
                    "integer" => Some(TypeRefAst::Integer(span)),
                    "decimal" => Some(TypeRefAst::Decimal(span)),
                    "string" => Some(TypeRefAst::String(span)),
                    other => {
                        self.errors.push(ParseError::UnexpectedToken {
                            expected: "type keyword (enum, bool, integer, decimal, string)".into(),
                            found: format!("'{other}'"),
                            span,
                        });
                        None
                    }
                }
            }
            _ => {
                self.errors.push(ParseError::UnexpectedToken {
                    expected: "type keyword".into(),
                    found: self.peek().kind.description(),
                    span,
                });
                None
            }
        }
    }
}

// ── Rules ─────────────────────────────────────────────────────────────────────

impl Parser {
    fn parse_rules_block(&mut self) -> Vec<RuleAst> {
        if self.expect_lparen("'(' for :rules").is_none() {
            return Vec::new();
        }
        let mut rules = Vec::new();
        let mut catch_all_span: Option<SourceSpan> = None;

        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            match self.parse_rule() {
                Some(r) => {
                    // Check for duplicate catch-all by borrowing r.when
                    if let WhenAst::CatchAll(s) = &r.when {
                        let s = *s;
                        if let Some(prev) = catch_all_span {
                            self.errors.push(ParseError::MultipleCatchAllRules {
                                span: s,
                                previous: prev,
                            });
                        } else {
                            catch_all_span = Some(s);
                        }
                    }
                    rules.push(r);
                }
                None => {
                    // parse_rule may have consumed partial tokens (e.g. consumed
                    // `(rule id :when` then failed mid-predicate-block, leaving us
                    // at `:then …`). Advance past whatever remains of the broken rule
                    // to prevent an infinite loop.
                    loop {
                        match self.peek().kind {
                            TokenKind::RParen | TokenKind::Eof => break,
                            TokenKind::LParen => {
                                self.recover_to_paren_close(1);
                                break;
                            }
                            _ => {
                                self.bump();
                            }
                        }
                    }
                }
            }
        }
        let _ = self.expect_rparen("')' to close :rules");
        rules
    }

    fn parse_rule(&mut self) -> Option<RuleAst> {
        let open = self.expect_lparen("'(' to open rule")?;
        let start = open.span.start;

        self.expect_symbol_named("rule")?;
        let id = self.expect_symbol("rule identifier")?;

        self.expect_symbol_named(":when")?;
        let when = self.parse_predicate_block()?;

        self.expect_symbol_named(":then")?;
        let then = self.parse_assignment_block();

        let close = self.expect_rparen("')' to close rule")?;
        let span = SourceSpan::new(start, close.span.end);
        Some(RuleAst {
            id,
            when,
            then,
            span,
        })
    }
}

// ── Predicate block ───────────────────────────────────────────────────────────

impl Parser {
    /// Parse `(*)` or `(pred+)`.
    fn parse_predicate_block(&mut self) -> Option<WhenAst> {
        let open = self.expect_lparen("'(' for :when")?;
        let start = open.span.start;

        // Catch-all: (*)
        if matches!(self.peek().kind, TokenKind::Star) {
            let star_span = self.peek().span;
            self.bump();
            if !matches!(self.peek().kind, TokenKind::RParen) {
                let span = star_span;
                self.errors
                    .push(ParseError::WildcardMixedWithPredicates { span });
                self.recover_to_paren_close(1);
                return None;
            }
            let close = self.expect_rparen("')' to close catch-all :when")?;
            return Some(WhenAst::CatchAll(SourceSpan::new(start, close.span.end)));
        }

        let mut preds = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            if matches!(self.peek().kind, TokenKind::Star) {
                let span = self.peek().span;
                self.errors
                    .push(ParseError::WildcardMixedWithPredicates { span });
                self.bump();
                continue;
            }
            if let Some(p) = self.parse_predicate() {
                preds.push(p);
            } else {
                self.recover_past_current_paren_close();
            }
        }
        let close = self.expect_rparen("')' to close :when block")?;
        let span = SourceSpan::new(start, close.span.end);
        Some(WhenAst::Predicates(preds, span))
    }

    fn parse_predicate(&mut self) -> Option<PredicateAst> {
        let open = self.expect_lparen("'(' to open predicate")?;
        let start = open.span.start;

        // Boolean combinators — dispatch on first symbol
        if self.peek_symbol_is("not") {
            self.bump();
            let inner = Box::new(self.parse_predicate()?);
            let close = self.expect_rparen("')' to close 'not'")?;
            return Some(PredicateAst::Not {
                inner,
                span: SourceSpan::new(start, close.span.end),
            });
        }
        if self.peek_symbol_is("and") {
            self.bump();
            return self.parse_combinator(start, "and");
        }
        if self.peek_symbol_is("or") {
            self.bump();
            return self.parse_combinator(start, "or");
        }

        // Field-reference predicates
        let field = match self.expect_symbol("field name in predicate") {
            Some(f) => f,
            None => {
                self.recover_to_paren_close(1);
                return None;
            }
        };

        // Equality
        if matches!(self.peek().kind, TokenKind::Eq) {
            self.bump();
            let value = self.parse_literal()?;
            let close = self.expect_rparen("')' to close predicate")?;
            return Some(PredicateAst::Eq {
                field,
                value,
                span: SourceSpan::new(start, close.span.end),
            });
        }
        // Inequality
        if matches!(self.peek().kind, TokenKind::NotEq) {
            self.bump();
            let value = self.parse_literal()?;
            let close = self.expect_rparen("')' to close predicate")?;
            return Some(PredicateAst::NotEq {
                field,
                value,
                span: SourceSpan::new(start, close.span.end),
            });
        }
        // Ordered comparisons — RHS must be numeric (D7)
        if let Some(make_pred) = self.try_consume_comparison_op() {
            let value = self.parse_numeric_literal()?;
            let close = self.expect_rparen("')' to close comparison")?;
            return Some(make_pred(
                field,
                value,
                SourceSpan::new(start, close.span.end),
            ));
        }
        // In (set or range)
        if self.peek_symbol_is("in") {
            self.bump();
            return self.parse_set_or_range(start, field);
        }
        // Null tests
        if self.peek_symbol_is("is-null") {
            self.bump();
            let close = self.expect_rparen("')' to close 'is-null'")?;
            return Some(PredicateAst::IsNull {
                field,
                span: SourceSpan::new(start, close.span.end),
            });
        }
        if self.peek_symbol_is("is-not-null") {
            self.bump();
            let close = self.expect_rparen("')' to close 'is-not-null'")?;
            return Some(PredicateAst::IsNotNull {
                field,
                span: SourceSpan::new(start, close.span.end),
            });
        }

        let span = self.peek().span;
        self.errors.push(ParseError::UnexpectedToken {
            expected: "predicate operator (=, !=, <, <=, >, >=, in, is-null, is-not-null)".into(),
            found: self.peek().kind.description(),
            span,
        });
        self.recover_to_paren_close(1);
        None
    }

    fn parse_combinator(&mut self, start: u32, combinator: &str) -> Option<PredicateAst> {
        let mut items = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            if let Some(p) = self.parse_predicate() {
                items.push(p);
            } else {
                self.recover_past_current_paren_close();
            }
        }
        if items.len() < 2 {
            let span = SourceSpan::new(start, self.peek().span.end);
            self.errors.push(ParseError::TooFewPredicates {
                combinator: combinator.to_owned(),
                span,
            });
        }
        let close = self.expect_rparen(&format!("')' to close '{combinator}'"))?;
        let span = SourceSpan::new(start, close.span.end);
        if combinator == "and" {
            Some(PredicateAst::And { items, span })
        } else {
            Some(PredicateAst::Or { items, span })
        }
    }

    /// Consume a comparison operator token and return a constructor for the matching variant.
    fn try_consume_comparison_op(
        &mut self,
    ) -> Option<fn(SymbolAst, NumberLitAst, SourceSpan) -> PredicateAst> {
        match self.peek().kind {
            TokenKind::Lt => {
                self.bump();
                Some(|f, v, s| PredicateAst::Lt {
                    field: f,
                    value: v,
                    span: s,
                })
            }
            TokenKind::Le => {
                self.bump();
                Some(|f, v, s| PredicateAst::Le {
                    field: f,
                    value: v,
                    span: s,
                })
            }
            TokenKind::Gt => {
                self.bump();
                Some(|f, v, s| PredicateAst::Gt {
                    field: f,
                    value: v,
                    span: s,
                })
            }
            TokenKind::Ge => {
                self.bump();
                Some(|f, v, s| PredicateAst::Ge {
                    field: f,
                    value: v,
                    span: s,
                })
            }
            _ => None,
        }
    }

    /// Dispatch to set-membership or range parse after `in` has been consumed.
    fn parse_set_or_range(&mut self, start: u32, field: SymbolAst) -> Option<PredicateAst> {
        match self.peek().kind {
            TokenKind::LBracket => self.parse_range(start, field),
            TokenKind::LParen => {
                if self.peek_is_range_after_lparen() {
                    self.parse_range(start, field)
                } else {
                    self.parse_set_membership(start, field)
                }
            }
            _ => {
                let span = self.peek().span;
                self.errors.push(ParseError::UnexpectedToken {
                    expected: "'(' for set or '['/('(' for range".into(),
                    found: self.peek().kind.description(),
                    span,
                });
                self.recover_to_paren_close(1);
                None
            }
        }
    }

    /// True if peeking reveals `(number .. ` or `(* ..` — an exclusive-lower range.
    fn peek_is_range_after_lparen(&self) -> bool {
        let inside = self.tokens.get(self.pos + 1).map(|t| &t.kind);
        let after = self.tokens.get(self.pos + 2).map(|t| &t.kind);
        match inside {
            Some(TokenKind::IntLit(_) | TokenKind::DecLit(_) | TokenKind::Star) => {
                matches!(after, Some(TokenKind::DotDot))
            }
            _ => false,
        }
    }

    fn parse_set_membership(&mut self, start: u32, field: SymbolAst) -> Option<PredicateAst> {
        let inner_open = self.expect_lparen("'(' for set values")?;

        if matches!(self.peek().kind, TokenKind::RParen) {
            let span = SourceSpan::new(inner_open.span.start, self.peek().span.end);
            self.errors.push(ParseError::EmptySet { span });
            self.bump();
            let close = self.expect_rparen("')' to close predicate")?;
            return Some(PredicateAst::InSet {
                field,
                values: Vec::new(),
                span: SourceSpan::new(start, close.span.end),
            });
        }

        let mut values = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            if let Some(lit) = self.parse_literal() {
                values.push(lit);
            } else {
                break;
            }
        }
        let _ = self.expect_rparen("')' to close set values");
        let close = self.expect_rparen("')' to close predicate")?;
        Some(PredicateAst::InSet {
            field,
            values,
            span: SourceSpan::new(start, close.span.end),
        })
    }

    fn parse_range(&mut self, start: u32, field: SymbolAst) -> Option<PredicateAst> {
        // Consume opening `[` or `(` and record inclusivity
        let lower_inclusive = match self.peek().kind {
            TokenKind::LBracket => {
                self.bump();
                true
            }
            TokenKind::LParen => {
                self.bump();
                false
            }
            _ => {
                let span = self.peek().span;
                self.errors.push(ParseError::UnexpectedToken {
                    expected: "'[' or '(' for range".into(),
                    found: self.peek().kind.description(),
                    span,
                });
                return None;
            }
        };

        let lower = self.parse_range_bound()?;
        self.expect_dotdot()?;
        let upper = self.parse_range_bound()?;

        let upper_inclusive = match self.peek().kind {
            TokenKind::RBracket => {
                self.bump();
                true
            }
            TokenKind::RParen => {
                self.bump();
                false
            }
            _ => {
                let span = self.peek().span;
                self.errors.push(ParseError::UnexpectedToken {
                    expected: "']' or ')' to close range".into(),
                    found: self.peek().kind.description(),
                    span,
                });
                return None;
            }
        };

        let close = self.expect_rparen("')' to close range predicate")?;
        Some(PredicateAst::Range {
            field,
            lower,
            upper,
            lower_inclusive,
            upper_inclusive,
            span: SourceSpan::new(start, close.span.end),
        })
    }

    fn parse_range_bound(&mut self) -> Option<RangeBound> {
        if matches!(self.peek().kind, TokenKind::Star) {
            let span = self.peek().span;
            self.bump();
            return Some(RangeBound::Unbounded(span));
        }
        Some(RangeBound::Value(self.parse_numeric_literal()?))
    }
}

// ── Assignments ───────────────────────────────────────────────────────────────

impl Parser {
    fn parse_assignment_block(&mut self) -> Vec<AssignmentAst> {
        if self.expect_lparen("'(' for :then").is_none() {
            return Vec::new();
        }
        let mut assignments = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            if let Some(a) = self.parse_assignment() {
                assignments.push(a);
            } else {
                self.recover_past_current_paren_close();
            }
        }
        let _ = self.expect_rparen("')' to close :then");
        assignments
    }

    fn parse_assignment(&mut self) -> Option<AssignmentAst> {
        let open = self.expect_lparen("'(' to open assignment")?;
        let start = open.span.start;
        let output = self.expect_symbol("output field name")?;
        if !matches!(self.peek().kind, TokenKind::Eq) {
            let span = self.peek().span;
            self.errors.push(ParseError::UnexpectedToken {
                expected: "'=' in assignment".into(),
                found: self.peek().kind.description(),
                span,
            });
            self.recover_to_paren_close(1);
            return None;
        }
        self.bump(); // `=`
        let value = self.parse_literal()?;
        let close = self.expect_rparen("')' to close assignment")?;
        Some(AssignmentAst {
            output,
            value,
            span: SourceSpan::new(start, close.span.end),
        })
    }
}

// ── Literals ──────────────────────────────────────────────────────────────────

impl Parser {
    fn parse_literal(&mut self) -> Option<LiteralAst> {
        let span = self.peek().span;
        match self.peek().kind.clone() {
            TokenKind::Symbol(name) => {
                self.bump();
                if name == "true" {
                    Some(LiteralAst::Boolean { value: true, span })
                } else if name == "false" {
                    Some(LiteralAst::Boolean { value: false, span })
                } else {
                    Some(LiteralAst::Symbol(SymbolAst { name, span }))
                }
            }
            TokenKind::StrLit(value) => {
                self.bump();
                Some(LiteralAst::String(StringLitAst { value, span }))
            }
            TokenKind::IntLit(_) | TokenKind::DecLit(_) => {
                let num = token_to_number_lit(&self.tokens[self.pos]).unwrap();
                self.bump();
                Some(LiteralAst::Number(num))
            }
            _ => {
                self.errors.push(ParseError::UnexpectedToken {
                    expected: "literal (symbol, string, number, boolean)".into(),
                    found: self.peek().kind.description(),
                    span,
                });
                None
            }
        }
    }

    fn parse_numeric_literal(&mut self) -> Option<NumberLitAst> {
        let span = self.peek().span;
        match &self.peek().kind {
            TokenKind::IntLit(_) | TokenKind::DecLit(_) => {
                let num = token_to_number_lit(&self.tokens[self.pos]).unwrap();
                self.bump();
                Some(num)
            }
            _ => {
                self.errors.push(ParseError::UnexpectedToken {
                    expected: "numeric literal".into(),
                    found: self.peek().kind.description(),
                    span,
                });
                None
            }
        }
    }

    fn parse_string_lit(&mut self) -> Option<StringLitAst> {
        let span = self.peek().span;
        if let TokenKind::StrLit(_) = &self.peek().kind {
            let s = token_to_string_lit(&self.tokens[self.pos]).unwrap();
            self.bump();
            Some(s)
        } else {
            self.errors.push(ParseError::UnexpectedToken {
                expected: "string literal".into(),
                found: self.peek().kind.description(),
                span,
            });
            None
        }
    }
}

// ── Low-level token helpers ───────────────────────────────────────────────────

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn bump(&mut self) -> Token {
        let tok = self.tokens[self.pos.min(self.tokens.len() - 1)].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn peek_is_eof(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }

    fn peek_keyword(&self, kw: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Symbol(n) if n == kw)
    }

    fn peek_symbol_is(&self, name: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Symbol(n) if n == name)
    }

    fn expect_lparen(&mut self, context: &str) -> Option<Token> {
        if matches!(self.peek().kind, TokenKind::LParen) {
            Some(self.bump())
        } else {
            let span = self.peek().span;
            self.errors.push(ParseError::UnexpectedToken {
                expected: format!("'(' for {context}"),
                found: self.peek().kind.description(),
                span,
            });
            None
        }
    }

    fn expect_rparen(&mut self, context: &str) -> Option<Token> {
        if matches!(self.peek().kind, TokenKind::RParen) {
            Some(self.bump())
        } else {
            let span = self.peek().span;
            self.errors.push(ParseError::UnexpectedToken {
                expected: format!("')' for {context}"),
                found: self.peek().kind.description(),
                span,
            });
            None
        }
    }

    fn expect_symbol(&mut self, context: &str) -> Option<SymbolAst> {
        if matches!(self.peek().kind, TokenKind::Symbol(_)) {
            let tok = self.bump();
            token_to_symbol(&tok)
        } else {
            let span = self.peek().span;
            self.errors.push(ParseError::UnexpectedToken {
                expected: format!("symbol ({context})"),
                found: self.peek().kind.description(),
                span,
            });
            None
        }
    }

    fn expect_symbol_named(&mut self, name: &str) -> Option<Token> {
        if self.peek_symbol_is(name) {
            Some(self.bump())
        } else if self.peek_is_eof() {
            let span = self.peek().span;
            self.errors.push(ParseError::UnexpectedEof {
                expected: format!("'{name}'"),
                span,
            });
            None
        } else {
            let span = self.peek().span;
            self.errors.push(ParseError::UnexpectedToken {
                expected: format!("'{name}'"),
                found: self.peek().kind.description(),
                span,
            });
            None
        }
    }

    fn expect_dotdot(&mut self) -> Option<Token> {
        if matches!(self.peek().kind, TokenKind::DotDot) {
            Some(self.bump())
        } else {
            let span = self.peek().span;
            self.errors.push(ParseError::UnexpectedToken {
                expected: "'..' range separator".into(),
                found: self.peek().kind.description(),
                span,
            });
            None
        }
    }

    // ── Error recovery ──────────────────────────────────────────────────────

    /// Skip tokens until `depth` unmatched `)` tokens have been consumed.
    fn recover_to_paren_close(&mut self, depth: usize) {
        let mut d = depth;
        while !self.peek_is_eof() {
            match self.peek().kind {
                TokenKind::LParen => {
                    d += 1;
                    self.bump();
                }
                TokenKind::RParen => {
                    self.bump();
                    d -= 1;
                    if d == 0 {
                        return;
                    }
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Skip past the next `(...)` group (one level).
    fn recover_past_current_paren_close(&mut self) {
        if matches!(self.peek().kind, TokenKind::LParen) {
            self.recover_to_paren_close(1);
        } else {
            while !matches!(
                self.peek().kind,
                TokenKind::LParen | TokenKind::RParen | TokenKind::Eof
            ) {
                self.bump();
            }
        }
    }

    /// Skip to the next `:<keyword>` symbol or `)` (for decision-level attr recovery).
    fn recover_to_keyword_or_rparen(&mut self) {
        while !self.peek_is_eof() {
            match &self.peek().kind {
                TokenKind::Symbol(n) if n.starts_with(':') => return,
                TokenKind::RParen => return,
                _ => {
                    self.bump();
                }
            }
        }
    }
}
