#![no_main]

// `GrpcTemplateConfig::from_owner_metadata` under hostile bytes — the gRPC
// FFI owner's mirror of `bpmn-lite-ffi-http/fuzz`'s
// `owner_metadata_decode` target: decoding externally-controlled/stored
// bytes (a gRPC FFI template's `owner_metadata`) into a typed config.
//
// Oracle: no-panic — any byte sequence either decodes + validates to a
// `GrpcTemplateConfig` (non-empty endpoint) or returns a typed error.

use bpmn_lite_ffi_grpc::GrpcTemplateConfig;
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 16 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let _ = GrpcTemplateConfig::from_owner_metadata(data);
});
