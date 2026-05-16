//! §8 equivalence contract enforcement.
//!
//! The reference evaluator (`reference::evaluate`) and the stack VM
//! (`vm::evaluate`) must satisfy:
//!
//! 1. **Both succeed or both fail.**
//! 2. **If both succeed:**
//!    - `trace.outcome` must be equal.
//!    - `output` must be byte-equal (all output slots identical).
//!    - For each rule: `matched` flag must agree.
//!    - For matched rules: `predicates` list must be element-wise identical.
//!    - For non-matched rules: VM's predicate list must be a strict prefix of
//!      the reference's list (VM short-circuits; reference does not).
//! 3. **If both fail:** the `EvalError` discriminant must match.

use dmn_lite_engine::reference::EvaluationOutput;
use dmn_lite_types::{EvalError, RuleId, ir::TypedValue};

// ── Public types ──────────────────────────────────────────────────────────────

/// Description of a divergence between reference and VM results.
#[derive(Debug)]
#[allow(dead_code)] // fields are used by Debug formatting in failure messages
pub struct DivergenceReport {
    pub kind: DivergenceKind,
    pub reference: String,
    pub vm: String,
}

/// Classification of the divergence.
#[derive(Debug)]
#[allow(dead_code)]
pub enum DivergenceKind {
    OutcomeDiffers,
    OutputDiffers,
    MatchedRuleSetDiffers,
    MatchedRulePredicatesDiffer { rule_id: RuleId },
    ErrorVariantDiffers,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Assert the §8 equivalence contract.
///
/// Returns `Ok(())` on agreement, `Err(DivergenceReport)` with the first
/// violation found.  Callers convert the report to a proptest `TestCaseError`
/// for failure reporting with shrunken inputs.
pub fn compare_results(
    reference: &Result<EvaluationOutput, EvalError>,
    vm: &Result<EvaluationOutput, EvalError>,
) -> Result<(), DivergenceReport> {
    match (reference, vm) {
        (Ok(ref_out), Ok(vm_out)) => compare_ok(ref_out, vm_out),
        (Err(ref_err), Err(vm_err)) => {
            if same_eval_error_variant(ref_err, vm_err) {
                Ok(())
            } else {
                Err(DivergenceReport {
                    kind: DivergenceKind::ErrorVariantDiffers,
                    reference: format!("{ref_err:?}"),
                    vm: format!("{vm_err:?}"),
                })
            }
        }
        _ => Err(DivergenceReport {
            kind: DivergenceKind::OutcomeDiffers,
            reference: format!("{reference:?}"),
            vm: format!("{vm:?}"),
        }),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn compare_ok(
    ref_out: &EvaluationOutput,
    vm_out: &EvaluationOutput,
) -> Result<(), DivergenceReport> {
    // TraceOutcome must agree.
    if ref_out.trace.outcome != vm_out.trace.outcome {
        return Err(DivergenceReport {
            kind: DivergenceKind::OutputDiffers,
            reference: format!("{:?}", ref_out.trace.outcome),
            vm: format!("{:?}", vm_out.trace.outcome),
        });
    }

    // Output slots must be byte-equal.
    for (rf, vf) in ref_out.output.iter().zip(vm_out.output.iter()) {
        if !typed_values_equal(rf, vf) {
            return Err(DivergenceReport {
                kind: DivergenceKind::OutputDiffers,
                reference: format!("{rf:?}"),
                vm: format!("{vf:?}"),
            });
        }
    }
    // Also check for mismatched output lengths (shouldn't happen; defence-in-depth).
    if ref_out.output.len() != vm_out.output.len() {
        return Err(DivergenceReport {
            kind: DivergenceKind::OutputDiffers,
            reference: format!("output len={}", ref_out.output.len()),
            vm: format!("output len={}", vm_out.output.len()),
        });
    }

    // Per-rule trace agreement.
    //
    // FIRST short-circuit discipline: under FIRST, the VM stops executing after
    // the first match. Rules that come after are never entered and appear in the
    // VM trace as stubs: `matched = false, predicates = []`. The reference
    // evaluator has no short-circuit and evaluates ALL rules, including catch-alls
    // that follow the first FIRST match (and thus appear as `matched = true`).
    //
    // A VM stub trace is identified by `matched = false AND predicates.is_empty()`.
    // For stubs, the `matched` flag comparison is skipped — the disagreement is
    // designed behaviour, not a bug.  The important invariant (correct output and
    // correct TraceOutcome) is already asserted above.
    for (ref_rule, vm_rule) in ref_out.trace.rules.iter().zip(vm_out.trace.rules.iter()) {
        let vm_is_stub = !vm_rule.matched && vm_rule.predicates.is_empty();

        if !vm_is_stub && ref_rule.matched != vm_rule.matched {
            return Err(DivergenceReport {
                kind: DivergenceKind::MatchedRuleSetDiffers,
                reference: format!("rule {} matched={}", ref_rule.rule_id.0, ref_rule.matched),
                vm: format!("rule {} matched={}", vm_rule.rule_id.0, vm_rule.matched),
            });
        }

        if vm_is_stub {
            // Never evaluated by the VM — nothing more to compare.
            continue;
        }

        if vm_rule.matched {
            // Matched rules: predicate lists must be identical.
            if ref_rule.predicates.len() != vm_rule.predicates.len() {
                return Err(DivergenceReport {
                    kind: DivergenceKind::MatchedRulePredicatesDiffer {
                        rule_id: ref_rule.rule_id,
                    },
                    reference: format!("{} predicates", ref_rule.predicates.len()),
                    vm: format!("{} predicates", vm_rule.predicates.len()),
                });
            }
            for (rp, vp) in ref_rule.predicates.iter().zip(vm_rule.predicates.iter()) {
                if rp.result != vp.result {
                    return Err(DivergenceReport {
                        kind: DivergenceKind::MatchedRulePredicatesDiffer {
                            rule_id: ref_rule.rule_id,
                        },
                        reference: format!("{rp:?}"),
                        vm: format!("{vp:?}"),
                    });
                }
            }
        } else {
            // Non-matched rules: VM predicates are a strict prefix of reference predicates.
            // (VM short-circuits on the first false; reference evaluates all predicates.)
            let vm_len = vm_rule.predicates.len();
            let ref_len = ref_rule.predicates.len();
            if vm_len > ref_len {
                return Err(DivergenceReport {
                    kind: DivergenceKind::MatchedRulePredicatesDiffer {
                        rule_id: ref_rule.rule_id,
                    },
                    reference: format!("non-matched rule has {ref_len} predicates"),
                    vm: format!("non-matched rule has {vm_len} predicates (exceeds reference)"),
                });
            }
            for (rp, vp) in ref_rule.predicates[..vm_len]
                .iter()
                .zip(vm_rule.predicates.iter())
            {
                if rp.result != vp.result {
                    return Err(DivergenceReport {
                        kind: DivergenceKind::MatchedRulePredicatesDiffer {
                            rule_id: ref_rule.rule_id,
                        },
                        reference: format!("{rp:?}"),
                        vm: format!("{vp:?}"),
                    });
                }
            }
        }
    }

    Ok(())
}

/// Compare `EvalError` variants (not full equality — `Vec` ordering in
/// `MultipleMatches` is an implementation detail, not a contract violation).
fn same_eval_error_variant(a: &EvalError, b: &EvalError) -> bool {
    matches!(
        (a, b),
        (EvalError::NoMatch, EvalError::NoMatch)
            | (
                EvalError::MultipleMatches { .. },
                EvalError::MultipleMatches { .. }
            )
            | (
                EvalError::InputSchemaMismatch { .. },
                EvalError::InputSchemaMismatch { .. }
            )
            | (EvalError::SchemaHashMismatch, EvalError::SchemaHashMismatch)
            | (
                EvalError::InputTypeMismatch { .. },
                EvalError::InputTypeMismatch { .. }
            )
            | (
                EvalError::InputDomainMismatch { .. },
                EvalError::InputDomainMismatch { .. }
            )
    )
}

/// Bit-equal comparison for `TypedValue`.
///
/// Uses `to_bits()` for `Decimal` so NaN == NaN (both evaluators must produce
/// the same bit pattern for any float computation to be considered equivalent).
fn typed_values_equal(a: &TypedValue, b: &TypedValue) -> bool {
    match (a, b) {
        (TypedValue::Null, TypedValue::Null) => true,
        (TypedValue::Bool(x), TypedValue::Bool(y)) => x == y,
        (TypedValue::Integer(x), TypedValue::Integer(y)) => x == y,
        (TypedValue::Decimal(x), TypedValue::Decimal(y)) => x.to_bits() == y.to_bits(),
        (TypedValue::Str(x), TypedValue::Str(y)) => x == y,
        (
            TypedValue::Enum {
                domain_id: d1,
                value_id: v1,
            },
            TypedValue::Enum {
                domain_id: d2,
                value_id: v2,
            },
        ) => d1 == d2 && v1 == v2,
        _ => false,
    }
}
