//! Byte-oriented lexer for the bpmn-dsl s-expression workflow language.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokenKind {
    LParen,
    RParen,
    /// `:keyword` — e.g. `:id`, `:verb`, `:next`, `:args`, `:decision`, `:condition`, `:status`, `:product`
    Keyword(String),
    /// Bare symbol: workflow node kinds (`start-event`, `service-task`, …),
    /// workflow/node names, verb FQNs (`cbu.create`, `cbu.add-product`), and `=`.
    Symbol(String),
    /// `@cbu`, `@cbu-type` — placeholder references.
    Placeholder(String),
    /// `"string literal"`
    StrLit(String),
    Eof,
}

impl TokenKind {
    pub(crate) fn description(&self) -> String {
        match self {
            Self::LParen => "'('".into(),
            Self::RParen => "')'".into(),
            Self::Keyword(k) => format!("':{k}'"),
            Self::Symbol(s) => format!("'{s}'"),
            Self::Placeholder(p) => format!("'@{p}'"),
            Self::StrLit(s) => format!("\"{}\"", s),
            Self::Eof => "end of input".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub kind: TokenKind,
    /// Byte offset where this token starts in the source.
    pub offset: usize,
}

/// Lex a bpmn-dsl source string into a token stream.
/// Always ends with an `Eof` token.
pub(crate) fn lex(source: &str) -> (Vec<Token>, Vec<LexError>) {
    Lexer::new(source).run()
}

#[derive(Debug, Clone)]
pub(crate) struct LexError {
    pub offset: usize,
    pub message: String,
}

// ── Lexer ────────────────────────────────────────────────────────────────────

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<LexError>,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            src: source.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn run(mut self) -> (Vec<Token>, Vec<LexError>) {
        loop {
            self.skip_whitespace_and_comments();
            if self.pos >= self.src.len() {
                break;
            }
            let ch = self.src[self.pos];
            match ch {
                b'(' => { self.emit(TokenKind::LParen); self.pos += 1; }
                b')' => { self.emit(TokenKind::RParen); self.pos += 1; }
                b'"' => self.lex_string(),
                b':' => self.lex_keyword(),
                b'@' => self.lex_placeholder(),
                _ if is_symbol_start(ch) => self.lex_symbol(),
                other => {
                    self.errors.push(LexError {
                        offset: self.pos,
                        message: format!("unexpected character '{}'", other as char),
                    });
                    self.pos += 1;
                }
            }
        }
        let eof_offset = self.pos;
        self.tokens.push(Token { kind: TokenKind::Eof, offset: eof_offset });
        (self.tokens, self.errors)
    }

    fn emit(&mut self, kind: TokenKind) {
        self.tokens.push(Token { kind, offset: self.pos });
    }

    fn skip_whitespace_and_comments(&mut self) {
        while self.pos < self.src.len() {
            match self.src[self.pos] {
                // v0.6 §10 uses comma as a visual separator inside :inputs lists.
                // Treat it as whitespace — it carries no semantic meaning.
                b' ' | b'\t' | b'\n' | b'\r' | b',' => { self.pos += 1; }
                b';' => {
                    // Line comment: skip to end of line
                    while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn lex_string(&mut self) {
        let start = self.pos;
        self.pos += 1; // skip opening "
        let mut s = String::new();
        loop {
            if self.pos >= self.src.len() {
                self.errors.push(LexError { offset: start, message: "unterminated string".into() });
                break;
            }
            match self.src[self.pos] {
                b'"' => { self.pos += 1; break; }
                b'\\' => {
                    self.pos += 1;
                    if self.pos < self.src.len() {
                        s.push(self.src[self.pos] as char);
                        self.pos += 1;
                    }
                }
                ch => { s.push(ch as char); self.pos += 1; }
            }
        }
        self.tokens.push(Token { kind: TokenKind::StrLit(s), offset: start });
    }

    fn lex_keyword(&mut self) {
        let start = self.pos;
        self.pos += 1; // skip ':'
        let name = self.take_symbol_chars();
        if name.is_empty() {
            self.errors.push(LexError { offset: start, message: "bare ':' is not a keyword".into() });
        } else {
            self.tokens.push(Token { kind: TokenKind::Keyword(name), offset: start });
        }
    }

    fn lex_placeholder(&mut self) {
        let start = self.pos;
        self.pos += 1; // skip '@'
        let name = self.take_symbol_chars();
        if name.is_empty() {
            self.errors.push(LexError { offset: start, message: "bare '@' is not a placeholder".into() });
        } else {
            self.tokens.push(Token { kind: TokenKind::Placeholder(name), offset: start });
        }
    }

    fn lex_symbol(&mut self) {
        let start = self.pos;
        let name = self.take_symbol_chars();
        self.tokens.push(Token { kind: TokenKind::Symbol(name), offset: start });
    }

    /// Consume characters that are valid inside a symbol or keyword name.
    /// Symbols can include letters, digits, `-`, `_`, `.` — covering verb FQNs
    /// like `cbu.add-product`, node kinds like `start-event`, and `=`.
    fn take_symbol_chars(&mut self) -> String {
        let mut s = String::new();
        while self.pos < self.src.len() && is_symbol_continue(self.src[self.pos]) {
            s.push(self.src[self.pos] as char);
            self.pos += 1;
        }
        s
    }
}

fn is_symbol_start(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'=' || ch == b'-'
}

/// Symbol continuation characters.
///
/// Includes `:` so namespaced verb references like `ob-poc:cbu.create` lex as a
/// single Symbol token (v0.6 T1 lexer mechanical update, T0 audit gap B).
/// Keyword tokens still start with a leading `:` (handled separately in `run()`),
/// so `:id` is a Keyword while `ob-poc:cbu.create` is a Symbol.
fn is_symbol_continue(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b'.' | b'=' | b':')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (tokens, errors) = lex(src);
        assert!(errors.is_empty(), "lex errors: {:?}", errors);
        tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lex_parens_and_keyword() {
        let k = kinds("(:id foo)");
        assert!(matches!(k[0], TokenKind::LParen));
        assert!(matches!(&k[1], TokenKind::Keyword(s) if s == "id"));
        assert!(matches!(&k[2], TokenKind::Symbol(s) if s == "foo"));
        assert!(matches!(k[3], TokenKind::RParen));
    }

    #[test]
    fn lex_verb_fqn() {
        let k = kinds(":verb cbu.add-product");
        assert!(matches!(&k[0], TokenKind::Keyword(s) if s == "verb"));
        assert!(matches!(&k[1], TokenKind::Symbol(s) if s == "cbu.add-product"));
    }

    #[test]
    fn lex_placeholder_and_string() {
        let k = kinds("(= @cbu-type \"fund\")");
        assert!(matches!(k[0], TokenKind::LParen));
        assert!(matches!(&k[1], TokenKind::Symbol(s) if s == "="));
        assert!(matches!(&k[2], TokenKind::Placeholder(s) if s == "cbu-type"));
        assert!(matches!(&k[3], TokenKind::StrLit(s) if s == "fund"));
        assert!(matches!(k[4], TokenKind::RParen));
    }

    #[test]
    fn lex_instrument_matrix_verb() {
        let k = kinds(":verb instrument-matrix.attach");
        assert!(matches!(&k[1], TokenKind::Symbol(s) if s == "instrument-matrix.attach"));
    }

    #[test]
    fn lex_namespaced_verb_as_single_symbol() {
        let k = kinds(":verb ob-poc:cbu.create");
        assert!(matches!(&k[0], TokenKind::Keyword(s) if s == "verb"));
        assert!(
            matches!(&k[1], TokenKind::Symbol(s) if s == "ob-poc:cbu.create"),
            "got: {:?}", &k[1]
        );
    }

    #[test]
    fn lex_namespaced_decision_as_single_symbol() {
        let k = kinds(":decision dmn-lite:cbu_type_routing");
        assert!(matches!(&k[1], TokenKind::Symbol(s) if s == "dmn-lite:cbu_type_routing"));
    }

    #[test]
    fn lex_comma_treated_as_whitespace() {
        let k = kinds("(a @b, c \"d\")");
        // Tokens: LParen Symbol("a") Placeholder("b") Symbol("c") StrLit("d") RParen Eof
        assert!(matches!(k[0], TokenKind::LParen));
        assert!(matches!(&k[1], TokenKind::Symbol(s) if s == "a"));
        assert!(matches!(&k[2], TokenKind::Placeholder(s) if s == "b"));
        assert!(matches!(&k[3], TokenKind::Symbol(s) if s == "c"));
        assert!(matches!(&k[4], TokenKind::StrLit(s) if s == "d"));
        assert!(matches!(k[5], TokenKind::RParen));
    }
}
