//! Canonical byte encoding for `FfiTemplate` identity.
//!
//! Per A2 §3. `compute_template_id(t)` produces the 32-byte BLAKE3 digest
//! of the canonical encoding. Determinism property:
//!
//!   ∀ t1 t2. structurally_equivalent(t1, t2) ↔ compute_template_id(t1) == compute_template_id(t2)
//!
//! "Structurally equivalent" means: same `owner_type`, same `input_schema`
//! (modulo order — sorted by name), same `output_schema` (modulo order),
//! same `idempotency`, same `owner_metadata` bytes. The catalogue tenant /
//! published_at / publisher are NOT part of the identity.
//!
//! Wire format:
//!
//! ```text
//! template_id = BLAKE3(
//!     encode_utf8(owner_type)
//!  || encode_u32le(input_schema.len())
//!  || for f in input_schema sorted by name:
//!         encode_utf8(f.name) || encode_bool(f.required) || encode_schema_kind(f.kind)
//!  || encode_u32le(output_schema.len())
//!  || for f in output_schema sorted by name:
//!         encode_utf8(f.name) || encode_bool(f.required) || encode_schema_kind(f.kind)
//!  || encode_idempotency(idempotency)
//!  || owner_metadata                              // verbatim bytes
//! )
//! ```
//!
//! Where:
//! - `encode_utf8(s)` = `encode_u32le(s.len_bytes()) || s.as_bytes()`
//! - `encode_u32le(n)` = `n.to_le_bytes()` (4 bytes)
//! - `encode_bool(b)` = `0x01` if true else `0x00`
//!
//! Schema kind tags:
//! - `0x00` Bool
//! - `0x01` I64
//! - `0x02` F64
//! - `0x03` String
//! - `0x04` SemOsDomain: 16-byte UUID || 32-byte version_hash
//! - `0x05` Opaque: encode_utf8(owner_format) || encode_u32le(owner_schema.len()) || owner_schema
//!
//! Idempotency tags:
//! - `0x00` Idempotent
//! - `0x01` NonIdempotent
//! - `0x02` IdempotentWithKey || encode_utf8(selector)

use crate::idempotency::Idempotency;
use crate::schema::{FieldSchema, SchemaKind};
use crate::template::FfiTemplate;

/// Compute the canonical BLAKE3 template id for an FfiTemplate.
///
/// The result does NOT depend on the input order of `input_schema` /
/// `output_schema` (they are sorted by name internally), the existing
/// `template.template_id` field (which is ignored — recomputed here),
/// the `tenant_id`, `published_at`, or `publisher` fields.
pub fn compute_template_id(template: &FfiTemplate) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    encode_template(template, &mut hasher);
    *hasher.finalize().as_bytes()
}

fn encode_template(t: &FfiTemplate, h: &mut blake3::Hasher) {
    encode_utf8(&t.owner_type, h);

    let mut sorted_inputs: Vec<&FieldSchema> = t.input_schema.iter().collect();
    sorted_inputs.sort_by(|a, b| a.name.cmp(&b.name));
    encode_u32le(sorted_inputs.len() as u32, h);
    for f in sorted_inputs {
        encode_field(f, h);
    }

    let mut sorted_outputs: Vec<&FieldSchema> = t.output_schema.iter().collect();
    sorted_outputs.sort_by(|a, b| a.name.cmp(&b.name));
    encode_u32le(sorted_outputs.len() as u32, h);
    for f in sorted_outputs {
        encode_field(f, h);
    }

    encode_idempotency(&t.idempotency, h);
    h.update(&t.owner_metadata);
}

fn encode_field(f: &FieldSchema, h: &mut blake3::Hasher) {
    encode_utf8(&f.name, h);
    encode_bool(f.required, h);
    encode_schema_kind(&f.kind, h);
}

fn encode_schema_kind(k: &SchemaKind, h: &mut blake3::Hasher) {
    match k {
        SchemaKind::Bool => {
            h.update(&[0x00]);
        }
        SchemaKind::I64 => {
            h.update(&[0x01]);
        }
        SchemaKind::F64 => {
            h.update(&[0x02]);
        }
        SchemaKind::String => {
            h.update(&[0x03]);
        }
        SchemaKind::SemOsDomain {
            domain_id,
            version_hash,
        } => {
            h.update(&[0x04]);
            h.update(domain_id.as_bytes()); // 16 bytes
            h.update(version_hash); // 32 bytes
        }
        SchemaKind::Opaque {
            owner_format,
            owner_schema,
        } => {
            h.update(&[0x05]);
            encode_utf8(owner_format, h);
            encode_u32le(owner_schema.len() as u32, h);
            h.update(owner_schema);
        }
    }
}

fn encode_idempotency(i: &Idempotency, h: &mut blake3::Hasher) {
    match i {
        Idempotency::Idempotent => {
            h.update(&[0x00]);
        }
        Idempotency::NonIdempotent => {
            h.update(&[0x01]);
        }
        Idempotency::IdempotentWithKey { selector } => {
            h.update(&[0x02]);
            encode_utf8(selector, h);
        }
    }
}

fn encode_utf8(s: &str, h: &mut blake3::Hasher) {
    encode_u32le(s.len() as u32, h);
    h.update(s.as_bytes());
}

fn encode_u32le(n: u32, h: &mut blake3::Hasher) {
    h.update(&n.to_le_bytes());
}

fn encode_bool(b: bool, h: &mut blake3::Hasher) {
    h.update(if b { &[0x01] } else { &[0x00] });
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn empty_template(owner_type: &str) -> FfiTemplate {
        FfiTemplate {
            template_id: [0u8; 32],
            owner_type: owner_type.to_string(),
            owner_metadata: vec![],
            input_schema: vec![],
            output_schema: vec![],
            idempotency: Idempotency::Idempotent,
            tenant_id: "tenant-a".to_string(),
            published_at: 0,
            publisher: "test".to_string(),
        }
    }

    #[test]
    fn determinism_same_input_same_hash() {
        let t = empty_template("dmn-lite");
        let h1 = compute_template_id(&t);
        let h2 = compute_template_id(&t);
        assert_eq!(h1, h2);
    }

    #[test]
    fn owner_type_affects_hash() {
        let a = empty_template("dmn-lite");
        let b = empty_template("http");
        assert_ne!(compute_template_id(&a), compute_template_id(&b));
    }

    #[test]
    fn owner_metadata_affects_hash() {
        let mut a = empty_template("dmn-lite");
        let mut b = empty_template("dmn-lite");
        a.owner_metadata = b"v1".to_vec();
        b.owner_metadata = b"v2".to_vec();
        assert_ne!(compute_template_id(&a), compute_template_id(&b));
    }

    #[test]
    fn tenant_published_publisher_do_not_affect_hash() {
        let mut a = empty_template("dmn-lite");
        let mut b = empty_template("dmn-lite");
        a.tenant_id = "tenant-x".to_string();
        a.published_at = 1234;
        a.publisher = "alice".to_string();
        b.tenant_id = "tenant-y".to_string();
        b.published_at = 9999;
        b.publisher = "bob".to_string();
        assert_eq!(compute_template_id(&a), compute_template_id(&b));
    }

    #[test]
    fn input_schema_order_does_not_affect_hash() {
        let mut a = empty_template("dmn-lite");
        let mut b = empty_template("dmn-lite");
        a.input_schema = vec![
            FieldSchema {
                name: "alpha".to_string(),
                kind: SchemaKind::Bool,
                required: true,
            },
            FieldSchema {
                name: "beta".to_string(),
                kind: SchemaKind::I64,
                required: false,
            },
        ];
        b.input_schema = vec![
            FieldSchema {
                name: "beta".to_string(),
                kind: SchemaKind::I64,
                required: false,
            },
            FieldSchema {
                name: "alpha".to_string(),
                kind: SchemaKind::Bool,
                required: true,
            },
        ];
        assert_eq!(compute_template_id(&a), compute_template_id(&b));
    }

    #[test]
    fn output_schema_order_does_not_affect_hash() {
        let mut a = empty_template("dmn-lite");
        let mut b = empty_template("dmn-lite");
        a.output_schema = vec![
            FieldSchema {
                name: "result".to_string(),
                kind: SchemaKind::String,
                required: true,
            },
            FieldSchema {
                name: "code".to_string(),
                kind: SchemaKind::I64,
                required: false,
            },
        ];
        b.output_schema = vec![
            FieldSchema {
                name: "code".to_string(),
                kind: SchemaKind::I64,
                required: false,
            },
            FieldSchema {
                name: "result".to_string(),
                kind: SchemaKind::String,
                required: true,
            },
        ];
        assert_eq!(compute_template_id(&a), compute_template_id(&b));
    }

    #[test]
    fn field_required_flag_affects_hash() {
        let mut a = empty_template("dmn-lite");
        let mut b = empty_template("dmn-lite");
        a.input_schema = vec![FieldSchema {
            name: "x".to_string(),
            kind: SchemaKind::Bool,
            required: true,
        }];
        b.input_schema = vec![FieldSchema {
            name: "x".to_string(),
            kind: SchemaKind::Bool,
            required: false,
        }];
        assert_ne!(compute_template_id(&a), compute_template_id(&b));
    }

    #[test]
    fn schema_kind_affects_hash() {
        let mut a = empty_template("dmn-lite");
        let mut b = empty_template("dmn-lite");
        a.input_schema = vec![FieldSchema {
            name: "x".to_string(),
            kind: SchemaKind::Bool,
            required: true,
        }];
        b.input_schema = vec![FieldSchema {
            name: "x".to_string(),
            kind: SchemaKind::I64,
            required: true,
        }];
        assert_ne!(compute_template_id(&a), compute_template_id(&b));
    }

    #[test]
    fn semos_domain_id_and_version_affect_hash() {
        let mut a = empty_template("dmn-lite");
        let mut b = empty_template("dmn-lite");
        let dom = Uuid::nil();
        a.input_schema = vec![FieldSchema {
            name: "x".to_string(),
            kind: SchemaKind::SemOsDomain {
                domain_id: dom,
                version_hash: [0u8; 32],
            },
            required: true,
        }];
        b.input_schema = vec![FieldSchema {
            name: "x".to_string(),
            kind: SchemaKind::SemOsDomain {
                domain_id: dom,
                version_hash: [1u8; 32],
            },
            required: true,
        }];
        assert_ne!(compute_template_id(&a), compute_template_id(&b));
    }

    #[test]
    fn opaque_format_and_schema_affect_hash() {
        let mut a = empty_template("custom");
        let mut b = empty_template("custom");
        a.input_schema = vec![FieldSchema {
            name: "x".to_string(),
            kind: SchemaKind::Opaque {
                owner_format: "fmt1".to_string(),
                owner_schema: b"x".to_vec(),
            },
            required: true,
        }];
        b.input_schema = vec![FieldSchema {
            name: "x".to_string(),
            kind: SchemaKind::Opaque {
                owner_format: "fmt1".to_string(),
                owner_schema: b"y".to_vec(),
            },
            required: true,
        }];
        assert_ne!(compute_template_id(&a), compute_template_id(&b));

        b.input_schema[0].kind = SchemaKind::Opaque {
            owner_format: "fmt2".to_string(),
            owner_schema: b"x".to_vec(),
        };
        assert_ne!(compute_template_id(&a), compute_template_id(&b));
    }

    #[test]
    fn idempotency_variants_affect_hash() {
        let a = {
            let mut t = empty_template("x");
            t.idempotency = Idempotency::Idempotent;
            compute_template_id(&t)
        };
        let b = {
            let mut t = empty_template("x");
            t.idempotency = Idempotency::NonIdempotent;
            compute_template_id(&t)
        };
        let c = {
            let mut t = empty_template("x");
            t.idempotency = Idempotency::IdempotentWithKey {
                selector: "req_id".to_string(),
            };
            compute_template_id(&t)
        };
        let d = {
            let mut t = empty_template("x");
            t.idempotency = Idempotency::IdempotentWithKey {
                selector: "other".to_string(),
            };
            compute_template_id(&t)
        };
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(c, d);
    }

    #[test]
    fn empty_template_hash_is_stable() {
        // Sanity: an empty (all-defaults) template produces a stable hash.
        // The exact bytes are an implementation detail of BLAKE3 over the
        // canonical encoding; the test just records the value to detect
        // accidental wire-format drift.
        let t = empty_template("dmn-lite");
        let h = compute_template_id(&t);
        // Encoded bytes:
        //   encode_utf8("dmn-lite") = 08 00 00 00 'd' 'm' 'n' '-' 'l' 'i' 't' 'e'
        //   encode_u32le(0)         = 00 00 00 00      (input_schema.len())
        //   encode_u32le(0)         = 00 00 00 00      (output_schema.len())
        //   encode_idempotency(Idempotent) = 00
        //   owner_metadata          = (empty)
        // Hash is BLAKE3 over the above 21 bytes.
        let expected: Vec<u8> = {
            let mut v = Vec::new();
            v.extend_from_slice(&8u32.to_le_bytes());
            v.extend_from_slice(b"dmn-lite");
            v.extend_from_slice(&0u32.to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
            v.push(0x00);
            v
        };
        let direct = *blake3::hash(&expected).as_bytes();
        assert_eq!(h, direct, "canonical encoding diverged from spec");
    }
}
