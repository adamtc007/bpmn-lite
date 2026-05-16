//! JSON path evaluator for FFI input/output bindings (A1 Δ9).
//!
//! Reads and writes values in `instance.domain_payload` via dotted paths.
//! Aligned with the C-minimal expression language (V&S v1.1 §10.6): flat
//! dotted paths into JSON objects, no array indexing, no wildcards, no
//! filters.
//!
//! Semantics:
//! - **read** fails explicitly on missing intermediates or non-object
//!   intermediates. There are no silent nulls.
//! - **write** auto-vivifies missing intermediate objects but refuses to
//!   traverse through non-object values that already exist. The compile-time
//!   type checker (Δ2 + Δ6) is the authority on whether a path is well-typed;
//!   runtime enforces only structural shape.
//! - **canonicalise_json** is deterministic for structurally-equivalent
//!   inputs because `serde_json::Map` is backed by `BTreeMap` under default
//!   features (no `preserve_order`). Output keys are alphabetically sorted.

use anyhow::{anyhow, Result};
use serde_json::Value;

/// Read the value at a dotted path. Returns an owned clone.
///
/// Empty path returns a clone of the root.
/// Missing intermediate field, missing terminal field, or non-object
/// intermediate → `Err`.
pub fn read(root: &Value, path: &[String]) -> Result<Value> {
    if path.is_empty() {
        return Ok(root.clone());
    }
    let mut cur = root;
    for (depth, segment) in path.iter().enumerate() {
        match cur {
            Value::Object(map) => {
                cur = map.get(segment).ok_or_else(|| {
                    anyhow!(
                        "json path: segment '{}' not found at depth {}",
                        segment,
                        depth
                    )
                })?;
            }
            other => {
                return Err(anyhow!(
                    "json path: cannot traverse '{}' at depth {}: parent is {}, not an object",
                    segment,
                    depth,
                    type_name(other)
                ));
            }
        }
    }
    Ok(cur.clone())
}

/// Write a value at a dotted path. Auto-vivifies missing intermediate objects.
///
/// Empty path replaces the entire root value.
/// Existing non-object intermediate → `Err`.
pub fn write_at_path(root: &mut Value, path: &[String], new_value: Value) -> Result<()> {
    if path.is_empty() {
        *root = new_value;
        return Ok(());
    }

    // Walk to the parent of the terminal segment.
    let mut cur = root;
    for (depth, segment) in path.iter().take(path.len() - 1).enumerate() {
        cur = match cur {
            Value::Object(map) => map
                .entry(segment.clone())
                .or_insert_with(|| Value::Object(serde_json::Map::new())),
            other => {
                return Err(anyhow!(
                    "json path: cannot write through '{}' at depth {}: parent is {}, not an object",
                    segment,
                    depth,
                    type_name(other)
                ));
            }
        };
        if !matches!(cur, Value::Object(_)) {
            return Err(anyhow!(
                "json path: cannot write through '{}' at depth {}: existing value is {}, not an object",
                segment,
                depth,
                type_name(cur)
            ));
        }
    }

    // Insert/overwrite at the terminal segment.
    let terminal = path.last().expect("path is non-empty here");
    match cur {
        Value::Object(map) => {
            map.insert(terminal.clone(), new_value);
            Ok(())
        }
        other => Err(anyhow!(
            "json path: cannot write '{}': parent is {}, not an object",
            terminal,
            type_name(other)
        )),
    }
}

/// Parse a JSON string into a `Value`.
pub fn parse_json(s: &str) -> Result<Value> {
    serde_json::from_str(s).map_err(|e| anyhow!("invalid JSON: {}", e))
}

/// Canonicalise a value to its sorted-keys serialisation.
///
/// Output is deterministic for structurally-equivalent inputs because
/// `serde_json::Map` is backed by `BTreeMap` under default features.
/// Round-trip property: `parse_json(s).and_then(|v| Ok(canonicalise_json(&v)))`
/// produces the same string for any pair of inputs that differ only in
/// key order or whitespace.
pub fn canonicalise_json(v: &Value) -> String {
    // `serde_json::to_string` cannot fail for a well-formed `Value`.
    serde_json::to_string(v).expect("Value always serialises")
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── read ──────────────────────────────────────────────────────

    #[test]
    fn read_existing_top_level_field() {
        let root = json!({"customer": "ACME"});
        let v = read(&root, &["customer".to_string()]).unwrap();
        assert_eq!(v, json!("ACME"));
    }

    #[test]
    fn read_deep_path() {
        let root = json!({"customer": {"address": {"city": "London"}}});
        let path = vec![
            "customer".to_string(),
            "address".to_string(),
            "city".to_string(),
        ];
        let v = read(&root, &path).unwrap();
        assert_eq!(v, json!("London"));
    }

    #[test]
    fn read_empty_path_returns_root_clone() {
        let root = json!({"a": 1});
        let v = read(&root, &[]).unwrap();
        assert_eq!(v, root);
    }

    #[test]
    fn read_root_level_scalar_root() {
        let root = json!(42);
        let v = read(&root, &[]).unwrap();
        assert_eq!(v, json!(42));
    }

    #[test]
    fn read_missing_field_errors() {
        let root = json!({"a": 1});
        let err = read(&root, &["b".to_string()]).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn read_missing_intermediate_errors() {
        let root = json!({"a": {"b": 1}});
        let path = vec!["a".to_string(), "missing".to_string(), "c".to_string()];
        let err = read(&root, &path).unwrap_err();
        assert!(err.to_string().contains("not found"));
        assert!(err.to_string().contains("depth 1"));
    }

    #[test]
    fn read_non_object_intermediate_errors() {
        let root = json!({"a": "string-not-object"});
        let path = vec!["a".to_string(), "b".to_string()];
        let err = read(&root, &path).unwrap_err();
        assert!(err.to_string().contains("not an object"));
    }

    #[test]
    fn read_returns_owned_clone() {
        // Mutating the returned value must not mutate the root.
        let root = json!({"a": {"b": 1}});
        let mut v = read(&root, &["a".to_string()]).unwrap();
        if let Value::Object(ref mut map) = v {
            map.insert("c".to_string(), json!(2));
        }
        assert_eq!(root, json!({"a": {"b": 1}}));
    }

    // ── write ─────────────────────────────────────────────────────

    #[test]
    fn write_new_top_level_field() {
        let mut root = json!({"a": 1});
        write_at_path(&mut root, &["b".to_string()], json!(2)).unwrap();
        assert_eq!(root, json!({"a": 1, "b": 2}));
    }

    #[test]
    fn write_overwrites_existing_field() {
        let mut root = json!({"a": 1});
        write_at_path(&mut root, &["a".to_string()], json!(99)).unwrap();
        assert_eq!(root, json!({"a": 99}));
    }

    #[test]
    fn write_deep_path_auto_vivifies_missing_objects() {
        let mut root = json!({});
        let path = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        write_at_path(&mut root, &path, json!("deep")).unwrap();
        assert_eq!(root, json!({"a": {"b": {"c": "deep"}}}));
    }

    #[test]
    fn write_deep_path_preserves_sibling_fields() {
        let mut root = json!({"a": {"existing": "sibling"}});
        let path = vec!["a".to_string(), "new_field".to_string()];
        write_at_path(&mut root, &path, json!(1)).unwrap();
        assert_eq!(root, json!({"a": {"existing": "sibling", "new_field": 1}}));
    }

    #[test]
    fn write_empty_path_replaces_root() {
        let mut root = json!({"a": 1});
        write_at_path(&mut root, &[], json!("replaced")).unwrap();
        assert_eq!(root, json!("replaced"));
    }

    #[test]
    fn write_through_non_object_intermediate_errors() {
        let mut root = json!({"a": "string"});
        let path = vec!["a".to_string(), "b".to_string()];
        let err = write_at_path(&mut root, &path, json!(1)).unwrap_err();
        assert!(err.to_string().contains("not an object"));
        // Root is left intact.
        assert_eq!(root, json!({"a": "string"}));
    }

    #[test]
    fn write_into_non_object_root_errors() {
        let mut root = json!("just-a-string");
        let err = write_at_path(&mut root, &["a".to_string()], json!(1)).unwrap_err();
        assert!(err.to_string().contains("not an object"));
    }

    // ── canonicalisation ──────────────────────────────────────────

    #[test]
    fn canonical_keys_are_sorted() {
        let v = json!({"z": 1, "a": 2, "m": 3});
        let s = canonicalise_json(&v);
        assert_eq!(s, r#"{"a":2,"m":3,"z":1}"#);
    }

    #[test]
    fn canonical_round_trip_is_stable_across_key_order() {
        let a = parse_json(r#"{"x": 1, "y": 2}"#).unwrap();
        let b = parse_json(r#"{"y": 2, "x": 1}"#).unwrap();
        assert_eq!(canonicalise_json(&a), canonicalise_json(&b));
    }

    #[test]
    fn canonical_round_trip_is_stable_across_whitespace() {
        let a = parse_json(r#"{"x":1,"y":2}"#).unwrap();
        let b = parse_json("{ \"x\" : 1 , \"y\" : 2 }").unwrap();
        assert_eq!(canonicalise_json(&a), canonicalise_json(&b));
    }

    #[test]
    fn canonical_escapes_strings_correctly() {
        let v = json!({"msg": "line1\nline2\t\"quoted\""});
        let s = canonicalise_json(&v);
        // serde_json escapes newlines, tabs, and quotes per RFC 8259.
        assert!(s.contains(r#"\n"#));
        assert!(s.contains(r#"\t"#));
        assert!(s.contains(r#"\""#));
    }

    #[test]
    fn canonical_preserves_integer_vs_float_form() {
        let v = parse_json("{\"i\": 1, \"f\": 1.0}").unwrap();
        let s = canonicalise_json(&v);
        // serde_json::Number distinguishes integer literals from float literals.
        assert!(s.contains("\"f\":1.0"));
        assert!(s.contains("\"i\":1") && !s.contains("\"i\":1.0"));
    }

    #[test]
    fn canonical_nested_objects_sort_recursively() {
        let v = json!({
            "outer_z": {"inner_z": 1, "inner_a": 2},
            "outer_a": {"inner_b": 3, "inner_a": 4}
        });
        let s = canonicalise_json(&v);
        assert_eq!(
            s,
            r#"{"outer_a":{"inner_a":4,"inner_b":3},"outer_z":{"inner_a":2,"inner_z":1}}"#
        );
    }

    // ── parse ─────────────────────────────────────────────────────

    #[test]
    fn parse_valid_json() {
        let v = parse_json(r#"{"a":1}"#).unwrap();
        assert_eq!(v, json!({"a": 1}));
    }

    #[test]
    fn parse_invalid_json_errors() {
        let err = parse_json("{not json}").unwrap_err();
        assert!(err.to_string().contains("invalid JSON"));
    }

    // ── round-trip read/write ─────────────────────────────────────

    #[test]
    fn read_then_write_then_read_round_trip() {
        let payload = r#"{"customer":{"jurisdiction":"LU"}}"#;
        let mut v = parse_json(payload).unwrap();

        // Read existing value.
        let path = vec!["customer".to_string(), "jurisdiction".to_string()];
        let original = read(&v, &path).unwrap();
        assert_eq!(original, json!("LU"));

        // Overwrite.
        write_at_path(&mut v, &path, json!("IE")).unwrap();
        let updated = read(&v, &path).unwrap();
        assert_eq!(updated, json!("IE"));

        // Canonical form reflects the write.
        let canon = canonicalise_json(&v);
        assert_eq!(canon, r#"{"customer":{"jurisdiction":"IE"}}"#);
    }
}
