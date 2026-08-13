#![no_main]

//! Fuzzes `DmnLiteOwner::invoke`'s untrusted `input_payload` decode path —
//! `build_input_context`/`json_to_typed_value` in
//! `dmn-lite-bridge/src/owner.rs`, the in-process FFI runtime-call
//! boundary. Structurally the same role as the already-fuzzed HTTP/gRPC
//! `owner_metadata_decode` targets (`bpmn-lite-ffi-http`,
//! `bpmn-lite-ffi-grpc`), but for the call *payload* rather than the
//! template config, and unlike those targets this path is pure/zero-I/O
//! (no live HTTP callout to mock), so it can be fuzzed in-process directly
//! through the real public `invoke` entry point.
//!
//! A fixed `VerifiedDecision` (compiled once, `OnceLock`) declares one
//! input field of every `dmn_lite_types::ResolvedType` variant
//! (bool/integer/decimal/string/enum) so every `json_to_typed_value` match
//! arm — including enum symbol resolution — is reachable from fuzzed
//! bytes.
//!
//! Oracle: no panic anywhere in decode -> build_input_context -> evaluate,
//! for any byte sequence, regardless of whether `invoke` resolves to
//! `Ok`/`Err`/`Incident`/`Success`/`NoMatch`.

use std::sync::OnceLock;

use dmn_lite_bridge::DmnLiteOwner;
use dmn_lite_compiler::{compile_and_verify, load_catalogue_from_str, Catalogue};
use dmn_lite_parser::parse;
use ffi_types::{
    FfiCall, FfiExecutionOwner, FfiTemplate, FieldSchema as FfiField, Idempotency, SchemaKind,
};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 16 * 1024;

const CATALOGUE_TOML: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "fuzz"
created_at = "2026-01-01T00:00:00Z"

[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "generic scalar domain"

[[domain]]
name = "J"
domain_id = "019c0a5d-0000-7000-8000-000000000002"
description = "enum domain"

[[domain.value]]
symbol = "A"
value_id = "019c0a5d-0000-7000-8000-000000000003"

[[domain.value]]
symbol = "B"
value_id = "019c0a5d-0000-7000-8000-000000000004"
"#;

const DECISION_SRC: &str = r#"(define-decision fuzz-multi-type :hit-policy first
    :inputs  ((flag   :type bool    :domain N)
              (count  :type integer :domain N)
              (amount :type decimal :domain N)
              (label  :type string  :domain N)
              (code   :type enum    :domain J))
    :outputs ((result :type bool :domain N))
    :rules   ((rule r001 :when ((flag = true)) :then ((result = true)))
              (rule r999 :when (*)             :then ((result = false)))))"#;

fn owner_and_template() -> &'static (DmnLiteOwner, FfiTemplate) {
    static CELL: OnceLock<(DmnLiteOwner, FfiTemplate)> = OnceLock::new();
    CELL.get_or_init(|| {
        let catalogue: Catalogue =
            load_catalogue_from_str(CATALOGUE_TOML).expect("fuzz catalogue must load");
        let decision = compile_and_verify(
            parse(DECISION_SRC).expect("fuzz decision must parse"),
            &catalogue,
            DECISION_SRC,
        )
        .expect("fuzz decision must compile and verify");

        let owner = DmnLiteOwner::new();
        let field = |name: &str| FfiField {
            name: name.to_string(),
            kind: SchemaKind::Bool,
            required: false,
        };
        let template = owner.register_decision(
            decision,
            vec![
                field("flag"),
                field("count"),
                field("amount"),
                field("label"),
                field("code"),
            ],
            vec![field("result")],
            Idempotency::Idempotent,
            "fuzz-tenant".to_string(),
            "fuzz".to_string(),
        );
        (owner, template)
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let (owner, template) = owner_and_template();
    let call = FfiCall {
        invocation_id: uuid::Uuid::nil(),
        template_id: template.template_id,
        tenant_id: "fuzz-tenant".to_string(),
        process_instance_id: uuid::Uuid::nil(),
        caller_task_id: "T1".to_string(),
        input_payload: data.to_vec(),
    };
    let _ = futures_executor::block_on(owner.invoke(call));
});
