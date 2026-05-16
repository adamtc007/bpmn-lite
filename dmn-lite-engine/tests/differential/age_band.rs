//! Differential tests for the `age_band` EBNF fixture (§5.2).
//!
//! Property: for any integer `age` input, the reference evaluator and the
//! stack VM produce equivalent results.
//!
//! The integer strategy mixes boundary-aware values (from the 4 range predicates:
//! 17, 18, 25, 26, 64, 65 and ±1) with uniform random i64s.

use proptest::prelude::*;

use dmn_lite_types::{FieldId, ir::TypedValue, values::TypedInputContextBuilder};

use crate::differential::{
    assert_equivalent,
    fixtures::age_band,
    strategies::{collect_integer_boundaries, input_strategy},
};

// ── Property test ─────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn vm_reference_agreement_age_band(
        input in {
            let f = age_band();
            input_strategy(&f.verified.as_compiled().typed_ir, &f.catalogue)
        }
    ) {
        assert_equivalent(age_band(), &input)?;
    }
}

// ── Deterministic edge-case tests (§3.6.1) ────────────────────────────────────

/// All fields null — both evaluators agree.
#[test]
fn edge_all_null() {
    let f = age_band();
    let compiled = f.verified.as_compiled();
    let mut b = TypedInputContextBuilder::new(&compiled.input_schema);
    b.set_null(FieldId(0));
    assert_equivalent(f, &b.build()).expect("all-null: no divergence");
}

/// All fields missing — both evaluators agree.
#[test]
fn edge_all_missing() {
    let f = age_band();
    let compiled = f.verified.as_compiled();
    let b = TypedInputContextBuilder::new(&compiled.input_schema);
    assert_equivalent(f, &b.build()).expect("all-missing: no divergence");
}

/// Range boundary values: 17, 18, 25, 26, 64, 65 and extremes.
///
/// These are the decision-critical boundary points extracted from the 4
/// range predicates in the age_band fixture.
#[test]
fn edge_range_boundary_values() {
    let f = age_band();
    let compiled = f.verified.as_compiled();
    let boundaries = collect_integer_boundaries(&compiled.typed_ir);

    // Always test the extracted boundaries + a few well-known points.
    let mut test_values = boundaries.clone();
    test_values.extend_from_slice(&[i64::MIN, i64::MAX, 0, -1, 100]);
    test_values.sort_unstable();
    test_values.dedup();

    for age in test_values {
        let mut b = TypedInputContextBuilder::new(&compiled.input_schema);
        b.set(FieldId(0), TypedValue::Integer(age));
        assert_equivalent(f, &b.build()).unwrap_or_else(|e| panic!("age={age} divergence: {e:?}"));
    }
}

/// Age band boundaries from the fixture: [*..18), [18..25], [26..64], [65..*].
/// Tests exact boundary values to verify inclusive/exclusive handling agrees.
#[test]
fn edge_specific_age_boundaries() {
    let f = age_band();
    let compiled = f.verified.as_compiled();

    // From the fixture: boundaries at 18, 25, 26, 64, 65.
    for age in [17i64, 18, 25, 26, 64, 65, 66, -100, 0, 1000] {
        let mut b = TypedInputContextBuilder::new(&compiled.input_schema);
        b.set(FieldId(0), TypedValue::Integer(age));
        assert_equivalent(f, &b.build()).unwrap_or_else(|e| panic!("age={age} divergence: {e:?}"));
    }
}
