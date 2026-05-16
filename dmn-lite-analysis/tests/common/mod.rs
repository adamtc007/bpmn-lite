//! Shared helpers for dmn-lite-analysis integration tests.
//!
//! Each integration test binary is compiled separately, so dead_code warnings fire
//! for catalogues a particular binary doesn't reference.  The constants below carry
//! `#[allow(dead_code)]` to permit per-binary subsets.

#![allow(dead_code)]

use dmn_lite_compiler::{Catalogue, compile_and_verify, load_catalogue_from_str};
use dmn_lite_parser::parse;
use dmn_lite_types::compiled::VerifiedDecision;

/// Minimal catalogue with one non-enum domain `N` for integer fields.
pub const INT_CAT: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"
[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "integers"
"#;

/// Catalogue with a small enum domain `AB` (values A, B, C) used for overlap tests.
pub const AB_CAT: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"
[[domain]]
name = "AB"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "AB three-value enum"

[[domain.value]]
symbol = "A"
value_id = "019c0a5d-0000-7000-8001-000000000001"

[[domain.value]]
symbol = "B"
value_id = "019c0a5d-0000-7000-8001-000000000002"

[[domain.value]]
symbol = "C"
value_id = "019c0a5d-0000-7000-8001-000000000003"

[[domain]]
name = "R"
domain_id = "019c0a5d-0000-7000-8000-000000000002"
description = "result"

[[domain.value]]
symbol = "OK"
value_id = "019c0a5d-0000-7000-8002-000000000001"
"#;

/// Combined integer + enum catalogue with both `N` and `AB`/`R`.
pub const COMBO_CAT: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"
[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "integers"

[[domain]]
name = "AB"
domain_id = "019c0a5d-0000-7000-8000-000000000002"
description = "AB three-value enum"

[[domain.value]]
symbol = "A"
value_id = "019c0a5d-0000-7000-8002-000000000001"

[[domain.value]]
symbol = "B"
value_id = "019c0a5d-0000-7000-8002-000000000002"

[[domain.value]]
symbol = "C"
value_id = "019c0a5d-0000-7000-8002-000000000003"

[[domain]]
name = "R"
domain_id = "019c0a5d-0000-7000-8000-000000000003"
description = "result"

[[domain.value]]
symbol = "OK"
value_id = "019c0a5d-0000-7000-8003-000000000001"
"#;

/// Load a catalogue from string content.
pub fn cat(s: &str) -> Catalogue {
    load_catalogue_from_str(s).expect("catalogue must load")
}

/// Compile-and-verify a source against a catalogue.
pub fn verified(src: &str, c: &Catalogue) -> VerifiedDecision {
    compile_and_verify(parse(src).expect("parse"), c, src).expect("compile_and_verify")
}
