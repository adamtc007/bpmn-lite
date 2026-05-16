//! Differential testing harness: VM vs reference evaluator.
//!
//! Modules:
//! - `fixtures`   — lazily compiled EBNF decisions (compile once, reuse forever)
//! - `strategies` — proptest strategies for generating well-typed `TypedInputContext`
//! - `compare`    — §8 equivalence contract implementation
//! - `booking`    — properties + edge cases for booking_eligibility
//! - `age_band`   — properties + edge cases for age_band
//! - `kyc_status` — properties + edge cases for kyc_status

pub mod age_band;
pub mod booking;
pub mod compare;
pub mod fixtures;
pub mod kyc_status;
pub mod strategies;

use dmn_lite_engine::{reference, vm};
use dmn_lite_types::{EvalError, values::TypedInputContext};
use proptest::test_runner::TestCaseError;

use crate::differential::{compare::compare_results, fixtures::Fixture};

/// Run both evaluators against `input` and assert §8 equivalence.
///
/// Returns `Ok(())` on agreement, `Err(TestCaseError::Fail)` with a
/// human-readable divergence description on disagreement.
pub fn assert_equivalent(
    fixture: &Fixture,
    input: &TypedInputContext,
) -> Result<(), TestCaseError> {
    let compiled = fixture.verified.as_compiled();
    let ref_result: Result<_, EvalError> =
        reference::evaluate(&compiled.typed_ir, input, fixture.source);
    let vm_result: Result<_, EvalError> = vm::evaluate(&fixture.verified, input, fixture.source);

    compare_results(&ref_result, &vm_result)
        .map_err(|report| TestCaseError::Fail(format!("{report:?}").into()))
}
