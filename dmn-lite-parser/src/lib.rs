//! dmn-lite s-expression DSL parser.
//!
//! Accepts the dmn-lite s-expression DSL defined in `docs/dmn-lite-ebnf.md`
//! and produces a typed AST for consumption by the compiler. Source spans
//! are preserved for diagnostics and editor feedback.
//!
//! # Usage
//!
//! ```rust
//! use dmn_lite_parser::parse;
//!
//! let src = r#"
//! (define-decision my-decision
//!   :hit-policy unique
//!   :inputs  ((status :type enum :domain Status))
//!   :outputs ((result :type enum :domain Result))
//!   :rules
//!     ((rule r1
//!        :when ((status = ACTIVE))
//!        :then ((result = OK)))))
//! "#;
//!
//! let ast = parse(src).unwrap();
//! assert_eq!(ast.decisions[0].name.name, "my-decision");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod lexer;
mod parser;

// Re-export AST types so callers don't need to depend on dmn-lite-types directly.
pub use dmn_lite_types::ast::{
    AssignmentAst, DecisionAst, HitPolicyAst, InputDeclAst, LiteralAst, NumberLitAst,
    OutputDeclAst, PredicateAst, RangeBound, RuleAst, Source, StringLitAst, SymbolAst, TypeRefAst,
    WhenAst,
};
pub use dmn_lite_types::errors::ParseError;
pub use dmn_lite_types::ids::{NumberKind, SourceSpan};

use std::fmt;

/// A collection of parse errors from a single parse attempt.
///
/// The parser attempts error recovery and continues after each error, so
/// multiple errors may be reported from one call to [`parse`]. The
/// `partial_ast` field holds successfully-parsed decisions even when other
/// parts of the source are invalid.
#[derive(Debug, Clone)]
pub struct ParseErrors {
    /// All parse errors encountered during the parse pass.
    pub errors: Vec<ParseError>,
    /// Partially-parsed AST, if at least one decision was successfully parsed.
    ///
    /// `None` when the parser could not produce any usable output.
    pub partial_ast: Option<Source>,
}

impl fmt::Display for ParseErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, e) in self.errors.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseErrors {}

/// Parse a dmn-lite s-expression DSL source string into a [`Source`] AST.
///
/// Returns `Ok(Source)` when parsing succeeds with no errors.
/// Returns `Err(ParseErrors)` when one or more parse errors occur. The
/// `ParseErrors::partial_ast` field may contain successfully-parsed
/// decisions even on error (enabling LSP-style partial recovery).
///
/// # Example
///
/// ```rust
/// use dmn_lite_parser::parse;
/// let result = parse("(define-decision d :hit-policy unknown ...)");
/// assert!(result.is_err());
/// ```
pub fn parse(source: &str) -> Result<Source, ParseErrors> {
    let (tokens, lex_errors) = lexer::lex(source);
    let mut p = parser::Parser::new(tokens);
    let ast = p.parse_source();
    let mut errors = lex_errors;
    errors.extend(p.into_errors());

    if errors.is_empty() {
        Ok(ast)
    } else {
        let partial_ast = if ast.decisions.is_empty() {
            None
        } else {
            Some(ast)
        };
        Err(ParseErrors {
            errors,
            partial_ast,
        })
    }
}
