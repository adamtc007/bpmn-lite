//! dmn-lite engine: reference evaluator and bytecode stack VM.
//!
//! Two evaluators share the same input/output contract:
//!
//! - [`reference`] module: reference evaluator over typed predicate IR
//!   (Phase 1.3). Used as the differential testing oracle for the VM.
//!   Correct but not optimised; never short-circuits.
//!
//! - [`vm`] module: production bytecode stack machine (Phase 1.4). Accepts
//!   only a [`VerifiedDecision`] produced by the bytecode verifier.
//!
//! The engine depends only on `dmn-lite-types`. It has no knowledge of the
//! compiler implementation — only of the [`VerifiedDecision`] artifact shape.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod reference;
pub mod vm;

use dmn_lite_types::compiled::VerifiedDecision;
use dmn_lite_types::{EvalError, TypedInputContext};

pub use reference::{EvaluationOutput, evaluate as reference_evaluate};

/// Evaluate a verified decision against a typed input context using the
/// production stack VM.
///
/// The `source` string is forwarded to the VM for human-readable predicate
/// descriptions in the evaluation trace. Pass `""` if the original source is
/// unavailable.
pub fn evaluate(
    decision: &VerifiedDecision,
    input: &TypedInputContext,
    source: &str,
) -> Result<EvaluationOutput, EvalError> {
    vm::evaluate(decision, input, source)
}
