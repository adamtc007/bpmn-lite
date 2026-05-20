//! Recursive-descent parser for the bpmn-dsl s-expression workflow language.
//!
//! Each grammar form maps to one `parse_*` function.
//! Errors are collected; parsing continues after each error.

use super::ast::*;
use super::lexer::{LexError, Token, TokenKind};

#[derive(Debug, Clone)]
pub(super) struct ParseError {
    pub offset: usize,
    pub message: String,
}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        Self { offset: e.offset, message: e.message }
    }
}

pub(super) struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    pub(crate) errors: Vec<ParseError>,
}

impl Parser {
    pub(super) fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0, errors: Vec::new() }
    }

    pub(super) fn into_errors(self) -> Vec<ParseError> {
        self.errors
    }

    // ── Token navigation ──────────────────────────────────────────────────────

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn advance(&mut self) -> &Token {
        let t = &self.tokens[self.pos];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn expect_lparen(&mut self, context: &str) -> Option<usize> {
        if matches!(self.peek().kind, TokenKind::LParen) {
            let offset = self.peek().offset;
            self.advance();
            Some(offset)
        } else {
            self.error(format!("expected '(' for {context}, found {}", self.peek().kind.description()));
            None
        }
    }

    fn expect_rparen(&mut self, context: &str) -> bool {
        if matches!(self.peek().kind, TokenKind::RParen) {
            self.advance();
            true
        } else {
            self.error(format!("expected ')' to close {context}, found {}", self.peek().kind.description()));
            false
        }
    }

    fn expect_keyword(&mut self, name: &str) -> Option<()> {
        if matches!(&self.peek().kind, TokenKind::Keyword(k) if k == name) {
            self.advance();
            Some(())
        } else {
            self.error(format!("expected ':{name}', found {}", self.peek().kind.description()));
            None
        }
    }

    fn expect_symbol(&mut self, context: &str) -> Option<String> {
        if let TokenKind::Symbol(s) = &self.peek().kind {
            let s = s.clone();
            self.advance();
            Some(s)
        } else {
            self.error(format!("expected symbol for {context}, found {}", self.peek().kind.description()));
            None
        }
    }

    fn expect_str_lit(&mut self, context: &str) -> Option<String> {
        if let TokenKind::StrLit(s) = &self.peek().kind {
            let s = s.clone();
            self.advance();
            Some(s)
        } else {
            self.error(format!("expected string literal for {context}, found {}", self.peek().kind.description()));
            None
        }
    }

    fn error(&mut self, msg: String) {
        self.errors.push(ParseError { offset: self.peek().offset, message: msg });
    }

    // ── Public entry ──────────────────────────────────────────────────────────

    pub(super) fn parse_workflow(&mut self) -> Option<WorkflowSource> {
        self.expect_lparen("workflow")?;

        if !matches!(&self.peek().kind, TokenKind::Symbol(s) if s == "workflow") {
            self.error(format!("expected 'workflow', found {}", self.peek().kind.description()));
            return None;
        }
        self.advance();

        let name = self.expect_symbol("workflow name")?;

        let mut nodes = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            if let Some(node) = self.parse_node() {
                nodes.push(node);
            } else {
                // Skip to next '(' to attempt recovery
                while !matches!(self.peek().kind, TokenKind::LParen | TokenKind::RParen | TokenKind::Eof) {
                    self.advance();
                }
            }
        }

        self.expect_rparen("workflow");

        Some(WorkflowSource { name, nodes })
    }

    fn parse_node(&mut self) -> Option<NodeAst> {
        self.expect_lparen("node")?;

        let kind_sym = match &self.peek().kind {
            TokenKind::Symbol(s) => s.clone(),
            _ => {
                self.error(format!("expected node kind, found {}", self.peek().kind.description()));
                return None;
            }
        };
        self.advance();

        let node = match kind_sym.as_str() {
            "start-event" => self.parse_start_event().map(NodeAst::StartEvent),
            "service-task" => self.parse_service_task().map(NodeAst::ServiceTask),
            "business-rule-task" => self.parse_business_rule_task().map(NodeAst::BusinessRuleTask),
            "exclusive-gateway" => self.parse_exclusive_gateway().map(NodeAst::ExclusiveGateway),
            "end-event" => self.parse_end_event().map(NodeAst::EndEvent),
            other => {
                self.error(format!("unknown node kind '{other}'"));
                None
            }
        };

        self.expect_rparen(&kind_sym);
        node
    }

    // ── Node parsers ──────────────────────────────────────────────────────────

    fn parse_start_event(&mut self) -> Option<StartEventAst> {
        let id = self.parse_kw_symbol("id")?;
        let next = self.parse_kw_symbol("next")?;
        Some(StartEventAst { id, next })
    }

    fn parse_service_task(&mut self) -> Option<ServiceTaskAst> {
        let id = self.parse_kw_symbol("id")?;
        let verb = self.parse_kw_symbol("verb")?;
        // Optional :args
        let args = if matches!(&self.peek().kind, TokenKind::Keyword(k) if k == "args") {
            self.advance(); // consume :args
            self.parse_args_list()
        } else {
            Vec::new()
        };
        let next = self.parse_kw_symbol("next")?;
        Some(ServiceTaskAst { id, verb, args, next })
    }

    fn parse_args_list(&mut self) -> Vec<(String, String)> {
        // (:key "value" ...)
        if self.expect_lparen(":args list").is_none() {
            return Vec::new();
        }
        let mut pairs = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            if let TokenKind::Keyword(k) = &self.peek().kind {
                let key = k.clone();
                self.advance();
                if let Some(val) = self.expect_str_lit(&format!(":{key} value")) {
                    pairs.push((key, val));
                }
            } else {
                self.error(format!("expected :key in args, found {}", self.peek().kind.description()));
                break;
            }
        }
        self.expect_rparen(":args list");
        pairs
    }

    fn parse_business_rule_task(&mut self) -> Option<BusinessRuleTaskAst> {
        let id = self.parse_kw_symbol("id")?;
        let decision = self.parse_kw_symbol("decision")?;
        let next = self.parse_kw_symbol("next")?;
        Some(BusinessRuleTaskAst { id, decision, next })
    }

    fn parse_exclusive_gateway(&mut self) -> Option<ExclusiveGatewayAst> {
        let id = self.parse_kw_symbol("id")?;
        let mut flows = Vec::new();
        while matches!(self.peek().kind, TokenKind::LParen) {
            if let Some(flow) = self.parse_flow() {
                flows.push(flow);
            }
        }
        if flows.is_empty() {
            self.error("exclusive-gateway must have at least one flow".into());
        }
        Some(ExclusiveGatewayAst { id, flows })
    }

    fn parse_flow(&mut self) -> Option<FlowAst> {
        self.expect_lparen("flow")?;
        if !matches!(&self.peek().kind, TokenKind::Symbol(s) if s == "flow") {
            self.error(format!("expected 'flow', found {}", self.peek().kind.description()));
            return None;
        }
        self.advance();

        self.expect_keyword("condition")?;
        let condition = self.parse_condition()?;
        let next = self.parse_kw_symbol("next")?;
        self.expect_rparen("flow");
        Some(FlowAst { condition, next })
    }

    fn parse_condition(&mut self) -> Option<ConditionAst> {
        // `(= @placeholder "value")`
        self.expect_lparen("condition")?;
        if !matches!(&self.peek().kind, TokenKind::Symbol(s) if s == "=") {
            self.error(format!("expected '=' in condition, found {}", self.peek().kind.description()));
            return None;
        }
        self.advance(); // consume `=`

        let placeholder = if let TokenKind::Placeholder(p) = &self.peek().kind {
            let p = p.clone();
            self.advance();
            p
        } else {
            self.error(format!("expected @placeholder in condition, found {}", self.peek().kind.description()));
            return None;
        };

        let value = self.expect_str_lit("condition value")?;
        self.expect_rparen("condition");
        Some(ConditionAst::Eq { placeholder: format!("@{placeholder}"), value })
    }

    fn parse_end_event(&mut self) -> Option<EndEventAst> {
        let id = self.parse_kw_symbol("id")?;
        let status = if matches!(&self.peek().kind, TokenKind::Keyword(k) if k == "status") {
            self.advance();
            self.expect_str_lit("end-event status").unwrap_or_default()
        } else {
            String::new()
        };
        Some(EndEventAst { id, status })
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn parse_kw_symbol(&mut self, keyword: &str) -> Option<String> {
        self.expect_keyword(keyword)?;
        self.expect_symbol(&format!(":{keyword} value"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::lexer::lex;
    use super::*;

    fn parse(src: &str) -> Result<WorkflowSource, Vec<ParseError>> {
        let (tokens, lex_errors) = lex(src);
        let mut p = Parser::new(tokens);
        let mut errors: Vec<ParseError> = lex_errors.into_iter().map(Into::into).collect();
        let ast = p.parse_workflow();
        errors.extend(p.into_errors());
        if errors.is_empty() {
            Ok(ast.unwrap())
        } else {
            Err(errors)
        }
    }

    #[test]
    fn parse_start_event() {
        let src = "(workflow test (start-event :id start :next next-node))";
        let ast = parse(src).expect("parse failed");
        assert_eq!(ast.name, "test");
        assert!(matches!(ast.nodes[0], NodeAst::StartEvent(_)));
    }

    #[test]
    fn parse_service_task_no_args() {
        let src = "(workflow test (service-task :id create-cbu :verb cbu.create :next next-node))";
        let ast = parse(src).expect("parse failed");
        if let NodeAst::ServiceTask(t) = &ast.nodes[0] {
            assert_eq!(t.verb, "cbu.create");
            assert!(t.args.is_empty());
        } else {
            panic!("expected service task");
        }
    }

    #[test]
    fn parse_service_task_with_args() {
        let src = r#"(workflow test (service-task :id add-fund :verb cbu.add-product :args (:product "CUSTODY_FUND") :next add-im))"#;
        let ast = parse(src).expect("parse failed");
        if let NodeAst::ServiceTask(t) = &ast.nodes[0] {
            assert_eq!(t.verb, "cbu.add-product");
            assert_eq!(t.args, vec![("product".into(), "CUSTODY_FUND".into())]);
        } else {
            panic!("expected service task");
        }
    }

    #[test]
    fn parse_gateway_with_flows() {
        let src = r#"(workflow test
          (exclusive-gateway :id gw
            (flow :condition (= @cbu-type "fund") :next add-fund)
            (flow :condition (= @cbu-type "corporate") :next add-corp)))"#;
        let ast = parse(src).expect("parse failed");
        if let NodeAst::ExclusiveGateway(gw) = &ast.nodes[0] {
            assert_eq!(gw.flows.len(), 2);
            let ConditionAst::Eq { placeholder, value } = &gw.flows[0].condition;
            assert_eq!(placeholder, "@cbu-type");
            assert_eq!(value, "fund");
        } else {
            panic!("expected gateway");
        }
    }

    #[test]
    fn parse_rejects_unknown_node_kind() {
        let src = "(workflow test (unknown-node :id x))";
        assert!(parse(src).is_err());
    }
}
