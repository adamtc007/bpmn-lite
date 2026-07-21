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
        Self {
            offset: e.offset,
            message: e.message,
        }
    }
}

pub(super) struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    pub(crate) errors: Vec<ParseError>,
}

impl Parser {
    pub(super) fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
        }
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

    fn get_span_end(&self) -> usize {
        let pk = self.peek();
        if matches!(pk.kind, TokenKind::Eof) {
            pk.offset
        } else {
            pk.offset + 1
        }
    }

    fn expect_lparen(&mut self, context: &str) -> Option<usize> {
        if matches!(self.peek().kind, TokenKind::LParen) {
            let offset = self.peek().offset;
            self.advance();
            Some(offset)
        } else {
            self.error(format!(
                "expected '(' for {context}, found {}",
                self.peek().kind.description()
            ));
            None
        }
    }

    fn expect_rparen(&mut self, context: &str) -> bool {
        if matches!(self.peek().kind, TokenKind::RParen) {
            self.advance();
            true
        } else {
            self.error(format!(
                "expected ')' to close {context}, found {}",
                self.peek().kind.description()
            ));
            false
        }
    }

    fn expect_keyword(&mut self, name: &str) -> Option<()> {
        if matches!(&self.peek().kind, TokenKind::Keyword(k) if k == name) {
            self.advance();
            Some(())
        } else {
            self.error(format!(
                "expected ':{name}', found {}",
                self.peek().kind.description()
            ));
            None
        }
    }

    fn expect_symbol(&mut self, context: &str) -> Option<String> {
        if let TokenKind::Symbol(s) = &self.peek().kind {
            let s = s.clone();
            self.advance();
            Some(s)
        } else {
            self.error(format!(
                "expected symbol for {context}, found {}",
                self.peek().kind.description()
            ));
            None
        }
    }

    fn expect_str_lit(&mut self, context: &str) -> Option<String> {
        if let TokenKind::StrLit(s) = &self.peek().kind {
            let s = s.clone();
            self.advance();
            Some(s)
        } else {
            self.error(format!(
                "expected string literal for {context}, found {}",
                self.peek().kind.description()
            ));
            None
        }
    }

    fn error(&mut self, msg: String) {
        self.errors.push(ParseError {
            offset: self.peek().offset,
            message: msg,
        });
    }

    // ── Public entry ──────────────────────────────────────────────────────────

    pub(super) fn parse_workflow(&mut self) -> Option<WorkflowSource> {
        self.expect_lparen("workflow")?;

        if !matches!(&self.peek().kind, TokenKind::Symbol(s) if s == "workflow") {
            self.error(format!(
                "expected 'workflow', found {}",
                self.peek().kind.description()
            ));
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
                while !matches!(
                    self.peek().kind,
                    TokenKind::LParen | TokenKind::RParen | TokenKind::Eof
                ) {
                    self.advance();
                }
            }
        }

        self.expect_rparen("workflow");

        Some(WorkflowSource { name, nodes })
    }

    fn parse_node(&mut self) -> Option<NodeAst> {
        let start_offset = self.peek().offset;
        self.expect_lparen("node")?;

        let kind_sym = match &self.peek().kind {
            TokenKind::Symbol(s) => s.clone(),
            _ => {
                self.error(format!(
                    "expected node kind, found {}",
                    self.peek().kind.description()
                ));
                return None;
            }
        };
        self.advance();

        let node = match kind_sym.as_str() {
            "start-event" | "start" => self.parse_start(start_offset).map(NodeAst::Start),
            "end-event" | "end" => self.parse_end(start_offset).map(NodeAst::End),
            "service-task" => self.parse_service_task_old(start_offset).map(NodeAst::Task),
            "business-rule-task" => self
                .parse_business_rule_task_old(start_offset)
                .map(NodeAst::Task),
            "task" => self.parse_task(start_offset).map(NodeAst::Task),
            "exclusive-gateway" => self
                .parse_exclusive_gateway_old(start_offset)
                .map(NodeAst::Split),
            "split-xor" => self
                .parse_split(start_offset, SplitModeAst::Xor)
                .map(NodeAst::Split),
            "split-or" => self
                .parse_split(start_offset, SplitModeAst::Or)
                .map(NodeAst::Split),
            "split-and" => self
                .parse_split(start_offset, SplitModeAst::And)
                .map(NodeAst::Split),
            "join-xor" => self
                .parse_join(start_offset, JoinModeAst::Xor)
                .map(NodeAst::Join),
            "join-or" => self
                .parse_join(start_offset, JoinModeAst::Or)
                .map(NodeAst::Join),
            "join-and" => self
                .parse_join(start_offset, JoinModeAst::And)
                .map(NodeAst::Join),
            "loop" => self.parse_loop(start_offset).map(NodeAst::Loop),
            other => {
                self.error(format!("unknown node kind '{other}'"));
                None
            }
        };

        self.expect_rparen(&kind_sym);
        node
    }

    // ── Node parsers ──────────────────────────────────────────────────────────

    fn parse_start(&mut self, start_offset: usize) -> Option<StartAst> {
        let id = self.parse_kw_symbol("id")?;
        let next = self.parse_kw_symbol("next")?;
        let end_offset = self.get_span_end();
        let span = bpmn_lite_types::SourceSpan::new(start_offset as u32, end_offset as u32);
        Some(StartAst { id, next, span })
    }

    fn parse_end(&mut self, start_offset: usize) -> Option<EndAst> {
        let id = self.parse_kw_symbol("id")?;
        let status = if matches!(&self.peek().kind, TokenKind::Keyword(k) if k == "status") {
            self.advance();
            self.expect_str_lit("end-event status").unwrap_or_default()
        } else {
            String::new()
        };
        let end_offset = self.get_span_end();
        let span = bpmn_lite_types::SourceSpan::new(start_offset as u32, end_offset as u32);
        Some(EndAst { id, status, span })
    }

    fn parse_service_task_old(&mut self, start_offset: usize) -> Option<TaskAst> {
        let id = self.parse_kw_symbol("id")?;
        let verb = self.parse_kw_symbol("verb")?;
        let args = if matches!(&self.peek().kind, TokenKind::Keyword(k) if k == "args") {
            self.advance();
            self.parse_args_list()
        } else {
            Vec::new()
        };
        let next = self.parse_kw_symbol("next")?;
        let end_offset = self.get_span_end();
        let span = bpmn_lite_types::SourceSpan::new(start_offset as u32, end_offset as u32);
        Some(TaskAst {
            id,
            plug: verb,
            args,
            next,
            delivery_mode: None,
            span,
        })
    }

    fn parse_business_rule_task_old(&mut self, start_offset: usize) -> Option<TaskAst> {
        let id = self.parse_kw_symbol("id")?;
        let decision = self.parse_kw_symbol("decision")?;
        let next = self.parse_kw_symbol("next")?;
        let end_offset = self.get_span_end();
        let span = bpmn_lite_types::SourceSpan::new(start_offset as u32, end_offset as u32);
        Some(TaskAst {
            id,
            plug: decision,
            args: Vec::new(),
            next,
            delivery_mode: None,
            span,
        })
    }

    fn parse_task(&mut self, start_offset: usize) -> Option<TaskAst> {
        let id = self.parse_kw_symbol("id")?;
        let plug = self.parse_kw_symbol("plug")?;
        let args = if matches!(&self.peek().kind, TokenKind::Keyword(k) if k == "args") {
            self.advance();
            self.parse_args_list()
        } else {
            Vec::new()
        };
        let delivery_mode = if matches!(&self.peek().kind, TokenKind::Keyword(k) if k == "delivery-mode")
        {
            self.advance();
            self.expect_str_lit("delivery-mode")
        } else {
            None
        };
        let next = self.parse_kw_symbol("next")?;
        let end_offset = self.get_span_end();
        let span = bpmn_lite_types::SourceSpan::new(start_offset as u32, end_offset as u32);
        Some(TaskAst {
            id,
            plug,
            args,
            next,
            delivery_mode,
            span,
        })
    }

    fn parse_args_list(&mut self) -> Vec<(String, String)> {
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
                self.error(format!(
                    "expected :key in args, found {}",
                    self.peek().kind.description()
                ));
                break;
            }
        }
        self.expect_rparen(":args list");
        pairs
    }

    fn parse_exclusive_gateway_old(&mut self, start_offset: usize) -> Option<SplitAst> {
        let id = self.parse_kw_symbol("id")?;
        let mut flows = Vec::new();
        while matches!(self.peek().kind, TokenKind::LParen) {
            if let Some(flow) = self.parse_split_flow(true) {
                flows.push(flow);
            }
        }
        if flows.is_empty() {
            self.error("exclusive-gateway must have at least one flow".into());
        }
        let join = format!("{}-join", id);
        let end_offset = self.get_span_end();
        let span = bpmn_lite_types::SourceSpan::new(start_offset as u32, end_offset as u32);
        Some(SplitAst {
            id,
            mode: SplitModeAst::Xor,
            plug: None,
            flows,
            join,
            span,
        })
    }

    fn parse_split(&mut self, start_offset: usize, mode: SplitModeAst) -> Option<SplitAst> {
        let id = self.parse_kw_symbol("id")?;
        let plug = if mode != SplitModeAst::And {
            Some(self.parse_kw_symbol("plug")?)
        } else {
            None
        };
        let join = self.parse_kw_symbol("join")?;
        let mut flows = Vec::new();
        while matches!(self.peek().kind, TokenKind::LParen) {
            if let Some(flow) = self.parse_split_flow(mode != SplitModeAst::And) {
                flows.push(flow);
            }
        }
        let end_offset = self.get_span_end();
        let span = bpmn_lite_types::SourceSpan::new(start_offset as u32, end_offset as u32);
        Some(SplitAst {
            id,
            mode,
            plug,
            flows,
            join,
            span,
        })
    }

    fn parse_split_flow(&mut self, require_condition: bool) -> Option<SplitFlowAst> {
        self.expect_lparen("flow")?;
        if !matches!(&self.peek().kind, TokenKind::Symbol(s) if s == "flow") {
            self.error(format!(
                "expected 'flow', found {}",
                self.peek().kind.description()
            ));
            return None;
        }
        self.advance();

        let condition = if require_condition {
            self.expect_keyword("condition")?;
            Some(self.parse_condition()?)
        } else {
            None
        };
        let next = self.parse_kw_symbol("next")?;
        self.expect_rparen("flow");
        Some(SplitFlowAst { condition, next })
    }

    fn parse_condition(&mut self) -> Option<ConditionAst> {
        self.expect_lparen("condition")?;
        if !matches!(&self.peek().kind, TokenKind::Symbol(s) if s == "=") {
            self.error(format!(
                "expected '=' in condition, found {}",
                self.peek().kind.description()
            ));
            return None;
        }
        self.advance(); // consume `=`

        let placeholder = if let TokenKind::Placeholder(p) = &self.peek().kind {
            let p = p.clone();
            self.advance();
            p
        } else {
            self.error(format!(
                "expected @placeholder in condition, found {}",
                self.peek().kind.description()
            ));
            return None;
        };

        let value = self.expect_str_lit("condition value")?;
        self.expect_rparen("condition");
        Some(ConditionAst::Eq {
            placeholder: format!("@{placeholder}"),
            value,
        })
    }

    fn parse_join(&mut self, start_offset: usize, mode: JoinModeAst) -> Option<JoinAst> {
        let id = self.parse_kw_symbol("id")?;
        let split = self.parse_kw_symbol("split")?;
        let next = self.parse_kw_symbol("next")?;
        let end_offset = self.get_span_end();
        let span = bpmn_lite_types::SourceSpan::new(start_offset as u32, end_offset as u32);
        Some(JoinAst {
            id,
            mode,
            split,
            next,
            span,
        })
    }

    fn parse_loop(&mut self, start_offset: usize) -> Option<LoopAst> {
        let id = self.parse_kw_symbol("id")?;
        self.expect_keyword("ceiling")?;
        let ceiling = if let TokenKind::Symbol(s) = &self.peek().kind {
            let val = s.parse::<u32>().ok().unwrap_or(0);
            self.advance();
            val
        } else {
            self.error("expected integer ceiling".into());
            0
        };
        self.expect_keyword("body")?;
        self.expect_lparen("loop body")?;
        let mut body = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            if let Some(node) = self.parse_node() {
                body.push(node);
            } else {
                while !matches!(
                    self.peek().kind,
                    TokenKind::LParen | TokenKind::RParen | TokenKind::Eof
                ) {
                    self.advance();
                }
            }
        }
        self.expect_rparen("loop body");
        let next = self.parse_kw_symbol("next")?;
        let end_offset = self.get_span_end();
        let span = bpmn_lite_types::SourceSpan::new(start_offset as u32, end_offset as u32);
        Some(LoopAst {
            id,
            ceiling,
            body,
            next,
            span,
        })
    }

    fn parse_kw_symbol(&mut self, keyword: &str) -> Option<String> {
        self.expect_keyword(keyword)?;
        self.expect_symbol(&format!(":{keyword} value"))
    }
}

pub fn parse_workflow_str(source: &str) -> Result<WorkflowSource, String> {
    let (tokens, lex_errors) = crate::dsl::lexer::lex(source);
    if !lex_errors.is_empty() {
        return Err(format!("Lex errors: {}", lex_errors[0].message));
    }
    let mut p = Parser::new(tokens);
    let ast = p.parse_workflow();
    if !p.errors.is_empty() {
        return Err(format!("Parse errors: {}", p.errors[0].message));
    }
    ast.ok_or_else(|| "Empty workflow".to_string())
}

pub fn parse_node_str(source: &str) -> Result<NodeAst, String> {
    let (tokens, lex_errors) = crate::dsl::lexer::lex(source);
    if !lex_errors.is_empty() {
        return Err(format!("Lex errors: {}", lex_errors[0].message));
    }
    let mut p = Parser::new(tokens);
    let ast = p.parse_node();
    if !p.errors.is_empty() {
        return Err(format!("Parse errors: {}", p.errors[0].message));
    }
    ast.ok_or_else(|| "Empty node".to_string())
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
        let src = "(workflow test (start :id start :next next-node))";
        let ast = parse(src).expect("parse failed");
        assert_eq!(ast.name, "test");
        assert!(matches!(ast.nodes[0], NodeAst::Start(_)));
    }

    #[test]
    fn parse_service_task_no_args() {
        let src = "(workflow test (service-task :id create-cbu :verb cbu.create :next next-node))";
        let ast = parse(src).expect("parse failed");
        if let NodeAst::Task(t) = &ast.nodes[0] {
            assert_eq!(t.plug, "cbu.create");
            assert!(t.args.is_empty());
        } else {
            panic!("expected task");
        }
    }

    #[test]
    fn parse_service_task_with_args() {
        let src = r#"(workflow test (service-task :id add-fund :verb cbu.add-product :args (:product "CUSTODY_FUND") :next add-im))"#;
        let ast = parse(src).expect("parse failed");
        if let NodeAst::Task(t) = &ast.nodes[0] {
            assert_eq!(t.plug, "cbu.add-product");
            assert_eq!(t.args, vec![("product".into(), "CUSTODY_FUND".into())]);
        } else {
            panic!("expected task");
        }
    }

    #[test]
    fn parse_gateway_with_flows() {
        let src = r#"(workflow test
          (exclusive-gateway :id gw
            (flow :condition (= @cbu-type "fund") :next add-fund)
            (flow :condition (= @cbu-type "corporate") :next add-corp)))"#;
        let ast = parse(src).expect("parse failed");
        if let NodeAst::Split(gw) = &ast.nodes[0] {
            assert_eq!(gw.flows.len(), 2);
            let ConditionAst::Eq { placeholder, value } = gw.flows[0].condition.as_ref().unwrap();
            assert_eq!(placeholder, "@cbu-type");
            assert_eq!(value, "fund");
        } else {
            panic!("expected split");
        }
    }

    #[test]
    fn parse_rejects_unknown_node_kind() {
        let src = "(workflow test (unknown-node :id x))";
        assert!(parse(src).is_err());
    }
}
