#![no_main]

use bpmn_lite_types::{CanonicalEncode, Value};
use libfuzzer_sys::fuzz_target;

// Fuzz target for `Value::from_canonical_bytes`.
//
// `Value::Array` is explicitly depth/length-bounded on decode
// (`MAX_VALUE_ARRAY_LEN`/`MAX_VALUE_ARRAY_DEPTH` in `bpmn_lite_types::types`,
// checked via `CanonicalReader.array_depth`) precisely because a
// self-referential/deeply-nested array is the classic decoder
// stack-overflow shape. This target is defense-in-depth for that bound: it
// fuzzes the decoder directly, independent of the gRPC-boundary array-limit
// check added separately — the decoder itself must never panic or hang on
// arbitrary bytes even before any higher-layer limit is applied.
//
// Property under test: never panics, never hangs, never OOMs on ANY byte
// sequence; always returns `Ok` or a clean `Err(CanonicalDecodeError)`.
fuzz_target!(|data: &[u8]| {
    let _ = Value::from_canonical_bytes(data);
});
