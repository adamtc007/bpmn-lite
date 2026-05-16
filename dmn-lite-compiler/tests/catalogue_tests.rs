//! Catalogue loader tests — Phase 1.2 §3.8 category 1.
//! Covers: valid TOML, malformed UUIDs, duplicates, missing fields.

use dmn_lite_compiler::{CatalogueError, load_catalogue_from_str};

const STUB: &str = include_str!("../../test-data/sem-os-stub.toml");

// ── Helper ────────────────────────────────────────────────────────────────────

fn minimal_catalogue(extra: &str) -> String {
    format!(
        r#"
snapshot_id      = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "v0.0.1-test"
created_at       = "2026-01-01T00:00:00Z"
{extra}
"#
    )
}

fn one_domain(name: &str, did: &str, vals: &str) -> String {
    format!(
        r#"
[[domain]]
name        = "{name}"
domain_id   = "{did}"
description = "test domain"
{vals}
"#
    )
}

fn one_value(sym: &str, vid: &str) -> String {
    format!(
        r#"
[[domain.value]]
symbol   = "{sym}"
value_id = "{vid}"
"#
    )
}

// 1. Valid stub loads and contains expected domains
#[test]
fn test_stub_loads_successfully() {
    let cat = load_catalogue_from_str(STUB).expect("stub must load");
    assert_eq!(
        cat.snapshot_id().to_string(),
        "019c0a5d-0000-7000-8000-000000000000"
    );
    assert_eq!(cat.snapshot_version, "v0.1.0-stub");
}

// 2. Stub contains Jurisdiction domain with LU value
#[test]
fn test_stub_jurisdiction_domain() {
    let cat = load_catalogue_from_str(STUB).unwrap();
    let jur = cat
        .resolve_domain("Jurisdiction")
        .expect("Jurisdiction must exist");
    assert_eq!(
        jur.resolve_value("LU").unwrap().to_string(),
        "019c0a5d-0000-7000-8001-000000000001"
    );
    assert_eq!(
        jur.resolve_value("IE").unwrap().to_string(),
        "019c0a5d-0000-7000-8001-000000000002"
    );
    assert!(jur.resolve_value("XX").is_none());
}

// 3. Stub covers EBNF §5.2 domains
#[test]
fn test_stub_age_band_domain() {
    let cat = load_catalogue_from_str(STUB).unwrap();
    let band = cat.resolve_domain("AgeBand").expect("AgeBand must exist");
    assert!(band.has_value("MINOR"));
    assert!(band.has_value("YOUNG_ADULT"));
    assert!(band.has_value("ADULT"));
    assert!(band.has_value("SENIOR"));
}

// 4. Stub covers EBNF §5.3 domains
#[test]
fn test_stub_kyc_domains() {
    let cat = load_catalogue_from_str(STUB).unwrap();
    assert!(cat.resolve_domain("TruthValue").is_some());
    let ro = cat
        .resolve_domain("ReviewOutcome")
        .expect("ReviewOutcome must exist");
    assert!(ro.has_value("PASS") && ro.has_value("FAIL"));
    let ks = cat
        .resolve_domain("KycStatus")
        .expect("KycStatus must exist");
    assert!(ks.has_value("PENDING_DOCUMENTS") && ks.has_value("APPROVED"));
}

// 5. Invalid snapshot_id (not UUIDv7)
#[test]
fn test_invalid_snapshot_id() {
    let src = r#"
snapshot_id      = "not-a-uuid"
snapshot_version = "v0"
created_at       = "2026-01-01T00:00:00Z"
"#;
    let err = load_catalogue_from_str(src).unwrap_err();
    assert!(
        matches!(err, CatalogueError::InvalidSnapshotId { .. }),
        "got {err:?}"
    );
}

// 6. UUIDv4 snapshot_id rejected (not v7)
#[test]
fn test_uuid_v4_snapshot_rejected() {
    let src = r#"
snapshot_id      = "550e8400-e29b-41d4-a716-446655440000"
snapshot_version = "v0"
created_at       = "2026-01-01T00:00:00Z"
"#;
    let err = load_catalogue_from_str(src).unwrap_err();
    assert!(
        matches!(err, CatalogueError::InvalidSnapshotId { .. }),
        "UUID v4 should be rejected"
    );
}

// 7. Invalid domain_id
#[test]
fn test_invalid_domain_id() {
    let dom = one_domain("X", "not-a-uuid", "");
    let src = minimal_catalogue(&dom);
    let err = load_catalogue_from_str(&src).unwrap_err();
    assert!(
        matches!(err, CatalogueError::InvalidDomainId { domain_name, .. } if domain_name == "X")
    );
}

// 8. Invalid value_id
#[test]
fn test_invalid_value_id() {
    let val = one_value("FOO", "bad-id");
    let dom = one_domain("X", "019c0a5d-0000-7000-8000-000000000099", &val);
    let src = minimal_catalogue(&dom);
    let err = load_catalogue_from_str(&src).unwrap_err();
    assert!(matches!(err, CatalogueError::InvalidValueId { symbol, .. } if symbol == "FOO"));
}

// 9. Duplicate domain name
#[test]
fn test_duplicate_domain_name() {
    let dom1 = one_domain("Dup", "019c0a5d-0000-7000-8000-000000000001", "");
    let dom2 = one_domain("Dup", "019c0a5d-0000-7000-8000-000000000002", "");
    let src = minimal_catalogue(&format!("{dom1}{dom2}"));
    let err = load_catalogue_from_str(&src).unwrap_err();
    assert!(matches!(err, CatalogueError::DuplicateDomainName { name } if name == "Dup"));
}

// 10. Duplicate domain_id
#[test]
fn test_duplicate_domain_id() {
    let shared_id = "019c0a5d-0000-7000-8000-000000000001";
    let dom1 = one_domain("A", shared_id, "");
    let dom2 = one_domain("B", shared_id, "");
    let src = minimal_catalogue(&format!("{dom1}{dom2}"));
    let err = load_catalogue_from_str(&src).unwrap_err();
    assert!(matches!(err, CatalogueError::DuplicateDomainId { .. }));
}

// 11. Duplicate value symbol within a domain
#[test]
fn test_duplicate_value_symbol() {
    let v1 = one_value("FOO", "019c0a5d-0000-7000-8001-000000000001");
    let v2 = one_value("FOO", "019c0a5d-0000-7000-8001-000000000002");
    let dom = one_domain(
        "X",
        "019c0a5d-0000-7000-8000-000000000001",
        &format!("{v1}{v2}"),
    );
    let src = minimal_catalogue(&dom);
    let err = load_catalogue_from_str(&src).unwrap_err();
    assert!(matches!(err, CatalogueError::DuplicateValueSymbol { symbol, .. } if symbol == "FOO"));
}

// 12. Duplicate value_id within a domain
#[test]
fn test_duplicate_value_id() {
    let shared_vid = "019c0a5d-0000-7000-8001-000000000001";
    let v1 = one_value("A", shared_vid);
    let v2 = one_value("B", shared_vid);
    let dom = one_domain(
        "X",
        "019c0a5d-0000-7000-8000-000000000001",
        &format!("{v1}{v2}"),
    );
    let src = minimal_catalogue(&dom);
    let err = load_catalogue_from_str(&src).unwrap_err();
    assert!(matches!(err, CatalogueError::DuplicateValueId { .. }));
}

// 13. Empty domain (no values) — valid
#[test]
fn test_empty_domain_valid() {
    let dom = one_domain("Empty", "019c0a5d-0000-7000-8000-000000000001", "");
    let src = minimal_catalogue(&dom);
    let cat = load_catalogue_from_str(&src).expect("empty domain is valid");
    let d = cat.resolve_domain("Empty").unwrap();
    assert_eq!(d.value_count(), 0);
}

// 14. Missing required field → TOML parse error
#[test]
fn test_missing_snapshot_id_field() {
    let src = r#"snapshot_version = "v0"\ncreated_at = "2026-01-01T00:00:00Z""#;
    let err = load_catalogue_from_str(src).unwrap_err();
    assert!(
        matches!(err, CatalogueError::Toml { .. }),
        "missing field should be a TOML error"
    );
}
