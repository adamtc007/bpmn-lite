//! Differential tests for the `kyc_status` EBNF fixture (§5.3).
//!
//! Two input fields: `documents-submitted` (bool) and `review-outcome` (enum).
//! Both evaluators are expected to agree across all 2 × N combinations plus
//! null/missing variants.

use proptest::prelude::*;

use dmn_lite_types::{FieldId, ir::TypedValue, values::TypedInputContextBuilder};

use crate::differential::{assert_equivalent, fixtures::kyc, strategies::input_strategy};

// ── Property test ─────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn vm_reference_agreement_kyc_status(
        input in {
            let f = kyc();
            input_strategy(&f.verified.as_compiled().typed_ir, &f.catalogue)
        }
    ) {
        assert_equivalent(kyc(), &input)?;
    }
}

// ── Deterministic edge-case tests (§3.6.1) ────────────────────────────────────

/// All fields null.
#[test]
fn edge_all_null() {
    let f = kyc();
    let compiled = f.verified.as_compiled();
    let mut b = TypedInputContextBuilder::new(&compiled.input_schema);
    for i in 0..compiled.input_schema.len() {
        b.set_null(FieldId(i));
    }
    assert_equivalent(f, &b.build()).expect("all-null: no divergence");
}

/// All fields missing.
#[test]
fn edge_all_missing() {
    let f = kyc();
    let compiled = f.verified.as_compiled();
    let b = TypedInputContextBuilder::new(&compiled.input_schema);
    assert_equivalent(f, &b.build()).expect("all-missing: no divergence");
}

/// Both boolean values of `documents-submitted` (field 0), review-outcome null.
#[test]
fn edge_both_bool_values() {
    let f = kyc();
    let compiled = f.verified.as_compiled();
    for docs in [true, false] {
        let mut b = TypedInputContextBuilder::new(&compiled.input_schema);
        b.set(FieldId(0), TypedValue::Bool(docs));
        b.set_null(FieldId(1));
        assert_equivalent(f, &b.build())
            .unwrap_or_else(|e| panic!("docs={docs} divergence: {e:?}"));
    }
}

/// Every value of `review-outcome` (field 1) with docs-submitted = true.
#[test]
fn edge_every_review_outcome_value() {
    let f = kyc();
    let compiled = f.verified.as_compiled();
    let schema = &compiled.input_schema[1];
    let domain_id = match &schema.field_type {
        dmn_lite_types::ir::ResolvedType::Enum { domain_id } => *domain_id,
        _ => panic!("review-outcome must be enum"),
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
        b.set(FieldId(0), TypedValue::Bool(true));
        b.set(
            FieldId(1),
            TypedValue::Enum {
                domain_id,
                value_id,
            },
        );
        assert_equivalent(f, &b.build())
            .unwrap_or_else(|e| panic!("review-outcome divergence: {e:?}"));
    }
}
