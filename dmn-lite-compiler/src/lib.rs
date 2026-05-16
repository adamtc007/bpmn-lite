//! dmn-lite compiler: `Source` AST → `CompiledDecision` → `VerifiedDecision`.
//!
//! # Usage
//!
//! ```rust,ignore
//! use dmn_lite_compiler::{compile_and_verify, load_catalogue_from_path};
//! use dmn_lite_parser::parse;
//!
//! let catalogue = load_catalogue_from_path("test-data/sem-os-stub.toml".as_ref())
//!     .expect("catalogue must load");
//! let src = "...";
//! let verified = compile_and_verify(parse(src).unwrap(), &catalogue, src)
//!     .expect("must compile and verify");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod catalogue_loader;
mod emit;
mod hash;
mod lower;
pub mod verify;

pub use catalogue_loader::{load_catalogue_from_path, load_catalogue_from_str};
pub use dmn_lite_types::ir::TypedDecision;
pub use dmn_lite_types::{
    ArtifactHash, Catalogue, CatalogueError, CompileError, CompileWarning, CompiledDecision,
    VerifiedDecision,
};
pub use verify::VerifierError;

use dmn_lite_parser::Source;
use std::fmt;
use thiserror::Error;

// ── CompileErrors ─────────────────────────────────────────────────────────────

/// Errors and warnings from a single compile call.
///
/// `partial_decision` is `Some(TypedDecision)` when the input/output schemas
/// resolved successfully, even if some rules failed type-checking.
pub struct CompileErrors {
    /// All compile errors encountered.
    pub errors: Vec<CompileError>,
    /// Non-fatal diagnostics.
    pub warnings: Vec<CompileWarning>,
    /// Partially type-checked IR if schemas resolved despite rule errors.
    pub partial_decision: Option<TypedDecision>,
}

impl fmt::Display for CompileErrors {
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

impl fmt::Debug for CompileErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompileErrors")
            .field("errors", &self.errors)
            .field("warnings", &self.warnings)
            .field(
                "partial_decision",
                &self.partial_decision.as_ref().map(|_| "<TypedDecision>"),
            )
            .finish()
    }
}

impl std::error::Error for CompileErrors {}

// ── CompileAndVerifyError ─────────────────────────────────────────────────────

/// Error from [`compile_and_verify`]: either compilation failed or verification
/// failed on the resulting bytecode.
#[derive(Debug, Error)]
pub enum CompileAndVerifyError {
    /// Type-checking or semantic checks failed.
    #[error("compile failed: {0}")]
    Compile(CompileErrors),

    /// The compiled bytecode violated a verifier invariant.
    #[error("verification failed: {0}")]
    Verify(VerifierError),
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Compile a parsed [`Source`] to a [`CompiledDecision`] (bytecode + typed IR).
///
/// On success returns `Ok(CompiledDecision)`.
/// On semantic error returns `Err(CompileErrors)`.
///
/// The resulting decision is **unverified**. Pass to
/// [`verify::verify()`] or use [`compile_and_verify()`].
#[allow(clippy::result_large_err)]
pub fn compile(
    source: Source,
    catalogue: &Catalogue,
    source_text: &str,
) -> Result<CompiledDecision, CompileErrors> {
    let errs = lower_to_ir_with_warnings(source, catalogue);
    if !errs.errors.is_empty() {
        return Err(errs);
    }
    let typed = errs
        .partial_decision
        .expect("no errors → must have typed IR");
    let compiled = emit::emit(typed, source_text);
    Ok(compiled)
}

/// Compile and verify in one step.  Returns a [`VerifiedDecision`] ready for
/// the stack VM.
#[allow(clippy::result_large_err)]
pub fn compile_and_verify(
    source: Source,
    catalogue: &Catalogue,
    source_text: &str,
) -> Result<VerifiedDecision, CompileAndVerifyError> {
    let compiled =
        compile(source, catalogue, source_text).map_err(CompileAndVerifyError::Compile)?;
    verify::verify(compiled).map_err(CompileAndVerifyError::Verify)
}

/// Lower source to a typed predicate IR, returning all diagnostics including
/// warnings.  The returned `partial_decision` is `Some(TypedDecision)` when
/// schemas resolved; otherwise `None`.
pub fn lower_to_ir_with_warnings(source: Source, catalogue: &Catalogue) -> CompileErrors {
    let decision_ast = match source.decisions.into_iter().next() {
        Some(d) => d,
        None => {
            return CompileErrors {
                errors: vec![CompileError::EmptyInputs { span: source.span }],
                warnings: Vec::new(),
                partial_decision: None,
            };
        }
    };
    let result = lower::lower(&decision_ast, catalogue);
    CompileErrors {
        errors: result.errors,
        warnings: result.warnings,
        partial_decision: result.decision,
    }
}

/// Lower source to a typed predicate IR.
///
/// Convenience wrapper around [`lower_to_ir_with_warnings`] for callers that
/// only need the typed IR (e.g., the reference evaluator or tests).
#[allow(clippy::result_large_err)]
pub fn compile_to_ir(
    source: Source,
    catalogue: &Catalogue,
) -> Result<TypedDecision, CompileErrors> {
    let errs = lower_to_ir_with_warnings(source, catalogue);
    if errs.errors.is_empty() {
        Ok(errs.partial_decision.expect("no errors"))
    } else {
        Err(errs)
    }
}
