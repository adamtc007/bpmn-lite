#![no_main]

// `HttpTemplateConfig::from_owner_metadata` under hostile bytes — one of
// the audit's smaller-severity gaps: decoding externally-controlled/stored
// bytes (an HTTP FFI template's `owner_metadata`, published outside this
// process and re-parsed on every registration) into a typed config, the
// same external-trust-boundary shape `wire_decode` (bpmn-lite-server-runner)
// already covers for the gRPC wire, but previously uncovered here.
//
// Oracle: no-panic — any byte sequence either decodes + validates to an
// `HttpTemplateConfig` (URL parses, every path_param has a matching `{}`
// placeholder, success_status_codes non-empty) or returns a typed error.
// `idempotency` is fixed to `Idempotent` — it is stored as-is by
// `from_owner_metadata` and never branches decode/validation logic, so it
// contributes no coverage either way.

use bpmn_lite_ffi_http::{HttpIdempotency, HttpTemplateConfig};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 16 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let _ = HttpTemplateConfig::from_owner_metadata(data, HttpIdempotency::Idempotent);
});
