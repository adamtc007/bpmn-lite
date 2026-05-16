//! Artifact hash tests — Phase 1.4 §3.5.

use dmn_lite_compiler::{compile, load_catalogue_from_str};
use dmn_lite_parser::parse;

const INT_CAT: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"
[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "integers"
"#;

fn int_cat() -> dmn_lite_compiler::Catalogue {
    load_catalogue_from_str(INT_CAT).expect("int_cat must load")
}

fn hash_of(src: &str) -> dmn_lite_types::ArtifactHash {
    let cat = int_cat();
    compile(parse(src).expect("parse"), &cat, src)
        .expect("compile")
        .artifact_hash
}

// ── Determinism ───────────────────────────────────────────────────────────────

#[test]
fn same_source_same_hash() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))))"#;
    assert_eq!(hash_of(src), hash_of(src));
}

#[test]
fn different_source_different_hash() {
    let src1 = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))))"#;
    let src2 = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 2)) :then ((y = 2)))))"#;
    assert_ne!(hash_of(src1), hash_of(src2));
}

// ── Source normalisation ──────────────────────────────────────────────────────

#[test]
fn comments_stripped_before_hash() {
    let with_comment = r#"(define-decision t :hit-policy first
        ; this is a comment
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))))"#;
    let without_comment = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))))"#;
    assert_eq!(hash_of(with_comment), hash_of(without_comment));
}

#[test]
fn extra_whitespace_collapsed_before_hash() {
    let compact = "(define-decision t :hit-policy first :inputs ((x :type integer :domain N)) :outputs ((y :type integer :domain N)) :rules ((rule r001 :when ((x = 1)) :then ((y = 1)))))";
    let spaced = "(define-decision t   :hit-policy  first \n  :inputs ((x  :type  integer  :domain  N))  :outputs  ((y  :type  integer  :domain  N))  :rules  ((rule  r001  :when  ((x = 1))  :then  ((y = 1)))))";
    assert_eq!(hash_of(compact), hash_of(spaced));
}

// ── Sensitivity ───────────────────────────────────────────────────────────────

#[test]
fn rule_name_affects_hash() {
    let src_a = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))))"#;
    let src_b = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r999 :when ((x = 1)) :then ((y = 1)))))"#;
    assert_ne!(hash_of(src_a), hash_of(src_b));
}

#[test]
fn field_name_affects_hash() {
    let src_a = r#"(define-decision t :hit-policy first
        :inputs  ((alpha :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((alpha = 1)) :then ((y = 1)))))"#;
    let src_b = r#"(define-decision t :hit-policy first
        :inputs  ((beta :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((beta = 1)) :then ((y = 1)))))"#;
    assert_ne!(hash_of(src_a), hash_of(src_b));
}

#[test]
fn constant_value_affects_hash() {
    let src_a = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))))"#;
    let src_b = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 99)) :then ((y = 99)))))"#;
    assert_ne!(hash_of(src_a), hash_of(src_b));
}

/// Hash is 32 bytes (BLAKE3 digest length).
#[test]
fn hash_is_32_bytes() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when (*) :then ((y = 0)))))"#;
    let h = hash_of(src);
    assert_eq!(h.as_bytes().len(), 32);
}
