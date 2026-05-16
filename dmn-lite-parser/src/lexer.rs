//! Byte-oriented lexer for the dmn-lite s-expression DSL.
//!
//! Converts a `&str` source into a flat `Vec<Token>`. Every token carries a
//! `SourceSpan` that covers its exact bytes in the source (no surrounding
//! whitespace included). Lexer errors are collected rather than stopping;
//! the caller receives both tokens and any errors in one call.

use dmn_lite_types::{
    ParseError,
    ast::{NumberLitAst, StringLitAst, SymbolAst},
    ids::{NumberKind, SourceSpan},
};

// ── Token ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokenKind {
    LParen,
    RParen,
    LBracket,
    RBracket,
    /// `..` range separator — distinct from `.` inside a symbol.
    DotDot,
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    /// `*` wildcard / unbounded range bound.
    Star,
    Symbol(String),
    StrLit(String),
    IntLit(String),
    DecLit(String),
    Eof,
}

impl TokenKind {
    /// Short human-readable description used in diagnostic messages.
    pub(crate) fn description(&self) -> String {
        match self {
            Self::LParen => "'('".into(),
            Self::RParen => "')'".into(),
            Self::LBracket => "'['".into(),
            Self::RBracket => "']'".into(),
            Self::DotDot => "'..'".into(),
            Self::Eq => "'='".into(),
            Self::NotEq => "'!='".into(),
            Self::Lt => "'<'".into(),
            Self::Le => "'<='".into(),
            Self::Gt => "'>'".into(),
            Self::Ge => "'>='".into(),
            Self::Star => "'*'".into(),
            Self::Symbol(s) => format!("'{s}'"),
            Self::StrLit(_) => "string literal".into(),
            Self::IntLit(s) => format!("integer '{s}'"),
            Self::DecLit(s) => format!("decimal '{s}'"),
            Self::Eof => "end of input".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub span: SourceSpan,
}

// ── Public entry point ───────────────────────────────────────────────────────

/// Lex `source` into a token stream plus any lexer-level errors.
///
/// The returned token stream always ends with an `Eof` sentinel.
/// Errors do not stop lexing; the lexer skips offending bytes and continues.
pub(crate) fn lex(source: &str) -> (Vec<Token>, Vec<ParseError>) {
    Lexer::new(source).run()
}

// ── Lexer internals ──────────────────────────────────────────────────────────

struct Lexer<'s> {
    src: &'s str,
    bytes: &'s [u8],
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<ParseError>,
}

impl<'s> Lexer<'s> {
    fn new(src: &'s str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn run(mut self) -> (Vec<Token>, Vec<ParseError>) {
        while self.pos < self.bytes.len() {
            self.skip_whitespace_and_comments();
            if self.pos >= self.bytes.len() {
                break;
            }
            self.lex_one();
        }
        let eof_pos = self.pos as u32;
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: SourceSpan::new(eof_pos, eof_pos),
        });
        (self.tokens, self.errors)
    }

    // ── Skip ───────────────────────────────────────────────────────────────

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.bytes.get(self.pos) {
                Some(b' ' | b'\t' | b'\n' | b'\r') => {
                    self.pos += 1;
                }
                Some(b';') => {
                    // line comment: skip to newline or EOF
                    self.pos += 1;
                    while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    // ── Single token ───────────────────────────────────────────────────────

    fn lex_one(&mut self) {
        let start = self.pos;
        let b = self.bytes[self.pos];

        match b {
            b'(' => {
                self.pos += 1;
                self.push(TokenKind::LParen, start);
            }
            b')' => {
                self.pos += 1;
                self.push(TokenKind::RParen, start);
            }
            b'[' => {
                self.pos += 1;
                self.push(TokenKind::LBracket, start);
            }
            b']' => {
                self.pos += 1;
                self.push(TokenKind::RBracket, start);
            }
            b'*' => {
                self.pos += 1;
                self.push(TokenKind::Star, start);
            }
            b'=' => {
                self.pos += 1;
                self.push(TokenKind::Eq, start);
            }
            b'!' => {
                if self.bytes.get(self.pos + 1) == Some(&b'=') {
                    self.pos += 2;
                    self.push(TokenKind::NotEq, start);
                } else {
                    self.pos += 1;
                    let span = SourceSpan::new(start as u32, self.pos as u32);
                    self.errors
                        .push(ParseError::UnexpectedChar { ch: '!', span });
                }
            }
            b'<' => {
                if self.bytes.get(self.pos + 1) == Some(&b'=') {
                    self.pos += 2;
                    self.push(TokenKind::Le, start);
                } else {
                    self.pos += 1;
                    self.push(TokenKind::Lt, start);
                }
            }
            b'>' => {
                if self.bytes.get(self.pos + 1) == Some(&b'=') {
                    self.pos += 2;
                    self.push(TokenKind::Ge, start);
                } else {
                    self.pos += 1;
                    self.push(TokenKind::Gt, start);
                }
            }
            b'.' => {
                // `..` is a DotDot token; a lone `.` not inside a symbol is an error
                if self.bytes.get(self.pos + 1) == Some(&b'.') {
                    self.pos += 2;
                    self.push(TokenKind::DotDot, start);
                } else {
                    self.pos += 1;
                    let span = SourceSpan::new(start as u32, self.pos as u32);
                    self.errors
                        .push(ParseError::UnexpectedChar { ch: '.', span });
                }
            }
            b'"' => self.lex_string(start),
            b'-' => {
                // `-` followed by a digit → negative number literal
                if matches!(self.bytes.get(self.pos + 1), Some(b'0'..=b'9')) {
                    self.pos += 1; // skip the `-`; lex_number re-reads from `start`
                    self.lex_number(start);
                } else {
                    self.pos += 1;
                    let span = SourceSpan::new(start as u32, self.pos as u32);
                    self.errors
                        .push(ParseError::UnexpectedChar { ch: '-', span });
                }
            }
            b'0'..=b'9' => self.lex_number(start),
            b':' => self.lex_symbol(start),
            b if is_symbol_start(b) => self.lex_symbol(start),
            _ => {
                // Decode the Unicode scalar for the error message
                let ch = self.src[self.pos..].chars().next().unwrap_or('?');
                let ch_len = ch.len_utf8();
                self.pos += ch_len;
                let span = SourceSpan::new(start as u32, self.pos as u32);
                self.errors.push(ParseError::UnexpectedChar { ch, span });
            }
        }
    }

    // ── Symbol ─────────────────────────────────────────────────────────────

    fn lex_symbol(&mut self, start: usize) {
        // Consume symbol-start (already validated by caller), then symbol-continue.
        // `.` is a symbol-continue UNLESS the NEXT byte is also `.` (that would be `..`).
        self.pos += 1; // consume the starting byte
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if is_symbol_continue(b) {
                if b == b'.' {
                    // Stop before `..`; a lone `.` at the end of a symbol is allowed.
                    if self.bytes.get(self.pos + 1) == Some(&b'.') {
                        break;
                    }
                }
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = &self.src[start..self.pos];
        self.push(TokenKind::Symbol(text.to_owned()), start);
    }

    // ── String literal ─────────────────────────────────────────────────────

    fn lex_string(&mut self, start: usize) {
        debug_assert_eq!(self.bytes[self.pos], b'"');
        self.pos += 1; // skip opening `"`
        let mut value = String::new();

        loop {
            match self.bytes.get(self.pos) {
                None => {
                    let span = SourceSpan::new(start as u32, self.pos as u32);
                    self.errors.push(ParseError::MalformedString {
                        reason: "unterminated string literal".into(),
                        span,
                    });
                    return;
                }
                Some(b'"') => {
                    self.pos += 1; // skip closing `"`
                    break;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.bytes.get(self.pos) {
                        Some(b'"') => {
                            value.push('"');
                            self.pos += 1;
                        }
                        Some(b'\\') => {
                            value.push('\\');
                            self.pos += 1;
                        }
                        Some(&esc) => {
                            let span =
                                SourceSpan::new((self.pos - 1) as u32, (self.pos + 1) as u32);
                            self.errors.push(ParseError::MalformedString {
                                reason: format!("invalid escape sequence '\\{}'", esc as char),
                                span,
                            });
                            self.pos += 1;
                        }
                        None => {
                            let span = SourceSpan::new((self.pos - 1) as u32, self.pos as u32);
                            self.errors.push(ParseError::MalformedString {
                                reason: "unterminated escape sequence".into(),
                                span,
                            });
                            return;
                        }
                    }
                }
                Some(&b) => {
                    // Safe: ASCII characters are single-byte; for multi-byte UTF-8
                    // we need to decode the char properly.
                    if b < 0x80 {
                        value.push(b as char);
                        self.pos += 1;
                    } else {
                        let ch = self.src[self.pos..].chars().next().unwrap_or('?');
                        value.push(ch);
                        self.pos += ch.len_utf8();
                    }
                }
            }
        }

        self.push(TokenKind::StrLit(value), start);
    }

    // ── Number literal ─────────────────────────────────────────────────────

    fn lex_number(&mut self, start: usize) {
        // At `start` we may have a `-` (already positioned past the `-` check
        // in lex_one, but `start` still points at the `-`).
        // Consume digits, then optional `.` + more digits.
        while matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        // Check for decimal part: `.` followed by at least one digit (not `..`)
        let is_decimal = matches!(self.bytes.get(self.pos), Some(b'.'))
            && self.bytes.get(self.pos + 1) != Some(&b'.')
            && matches!(self.bytes.get(self.pos + 1), Some(b'0'..=b'9'));

        if is_decimal {
            self.pos += 1; // consume `.`
            while matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        let text = &self.src[start..self.pos];
        if is_decimal {
            self.push(TokenKind::DecLit(text.to_owned()), start);
        } else {
            self.push(TokenKind::IntLit(text.to_owned()), start);
        }
    }

    // ── Helpers ────────────────────────────────────────────────────────────

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: SourceSpan::new(start as u32, self.pos as u32),
        });
    }
}

// ── Character class predicates ───────────────────────────────────────────────

fn is_symbol_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_symbol_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.'
}

// ── Token stream helpers (used by parser) ────────────────────────────────────

/// Extract an owned `SymbolAst` from a `Symbol` token.
pub(crate) fn token_to_symbol(tok: &Token) -> Option<SymbolAst> {
    if let TokenKind::Symbol(name) = &tok.kind {
        Some(SymbolAst {
            name: name.clone(),
            span: tok.span,
        })
    } else {
        None
    }
}

/// Extract an owned `StringLitAst` from a `StrLit` token.
pub(crate) fn token_to_string_lit(tok: &Token) -> Option<StringLitAst> {
    if let TokenKind::StrLit(value) = &tok.kind {
        Some(StringLitAst {
            value: value.clone(),
            span: tok.span,
        })
    } else {
        None
    }
}

/// Extract an owned `NumberLitAst` from an `IntLit` or `DecLit` token.
pub(crate) fn token_to_number_lit(tok: &Token) -> Option<NumberLitAst> {
    match &tok.kind {
        TokenKind::IntLit(text) => Some(NumberLitAst {
            text: text.clone(),
            kind: NumberKind::Integer,
            span: tok.span,
        }),
        TokenKind::DecLit(text) => Some(NumberLitAst {
            text: text.clone(),
            kind: NumberKind::Decimal,
            span: tok.span,
        }),
        _ => None,
    }
}
