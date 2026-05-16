//! Differential tests for the `booking_eligibility` EBNF fixture (§5.1).
//!
//! Property: for any well-typed input (5 enum fields from the stub catalogue),
//! the reference evaluator and the stack VM produce equivalent results per the
//! §8 equivalence contract.

use proptest::prelude::*;

use dmn_lite_types::{FieldId, ir::TypedValue, values::TypedInputContextBuilder};

use crate::differential::{assert_equivalent, fixtures::booking, strategies::input_strategy};

// ── Property tests ────────────────────────────────────────────────────────────

// VM and reference agree across 1000 generated inputs.
// (NotEq null semantics bug fixed in Phase 1.5a.)
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn vm_reference_agreement_booking_eligibility(
        input in {
            let f = booking();
            input_strategy(&f.verified.as_compiled().typed_ir, &f.catalogue)
        }
    ) {
        assert_equivalent(booking(), &input)?;
    }
}

// ── Deterministic edge-case tests (§3.6.1) ────────────────────────────────────

/// All fields null — both evaluators should agree (catch-all r999 matches).
#[test]
fn edge_all_null_fields() {
    let f = booking();
    let compiled = f.verified.as_compiled();
    let mut b = TypedInputContextBuilder::new(&compiled.input_schema);
    for i in 0..compiled.input_schema.len() {
        b.set_null(FieldId(i));
    }
    assert_equivalent(f, &b.build()).expect("all-null: no divergence");
}

/// All fields missing — both evaluators should agree.
#[test]
fn edge_all_missing_fields() {
    let f = booking();
    let compiled = f.verified.as_compiled();
    let b = TypedInputContextBuilder::new(&compiled.input_schema);
    assert_equivalent(f, &b.build()).expect("all-missing: no divergence");
}

/// Every value of `jurisdiction` (field 0) while other fields are fixed.
/// Exercises each of the 7 Jurisdiction catalogue values.
#[test]
fn edge_every_jurisdiction_value() {
    let f = booking();
    let compiled = f.verified.as_compiled();
    let jurisdiction_schema = &compiled.input_schema[0];
    let domain_id = match &jurisdiction_schema.field_type {
        dmn_lite_types::ir::ResolvedType::Enum { domain_id } => *domain_id,
        _ => panic!("jurisdiction must be enum"),
    };
    let domain = f
        .catalogue
        .domains()
        .find(|d| d.domain_id == domain_id)
        .expect("domain must be in catalogue");
    let mut value_ids: Vec<_> = domain.values().map(|v| v.value_id).collect();
    value_ids.sort_by_key(|v| v.0);

    for value_id in value_ids {
        let mut b = TypedInputContextBuilder::new(&compiled.input_schema);
        b.set(
            FieldId(0),
            TypedValue::Enum {
                domain_id,
                value_id,
            },
        );
        assert_equivalent(f, &b.build())
            .unwrap_or_else(|e| panic!("jurisdiction divergence: {e:?}"));
    }
}

/// Every value of `client-type` (field 1) while other fields are fixed.
#[test]
fn edge_every_client_type_value() {
    let f = booking();
    let compiled = f.verified.as_compiled();
    let schema = &compiled.input_schema[1];
    let domain_id = match &schema.field_type {
        dmn_lite_types::ir::ResolvedType::Enum { domain_id } => *domain_id,
        _ => panic!("client-type must be enum"),
    };
    let domain = f
        .catalogue
        .domains()
        .find(|d| d.domain_id == domain_id)
        .unwrap();
    let mut value_ids: Vec<_> = domain.values().map(|v| v.value_id).collect();
    value_ids.sort_by_key(|v| v.0);

    for value_id in value_ids {
        let mut b = TypedInputContextBuilder::new(&compiled.input_schema);
        b.set(
            FieldId(1),
            TypedValue::Enum {
                domain_id,
                value_id,
            },
        );
        assert_equivalent(f, &b.build())
            .unwrap_or_else(|e| panic!("client-type divergence: {e:?}"));
    }
}
