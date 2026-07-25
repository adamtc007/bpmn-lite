#![no_main]

use bpmn_lite_types::{CanonicalEncode, Fiber, JournalRecord, SnapshotEnvelope};
use libfuzzer_sys::fuzz_target;

// Fuzz target for the top-level persistence aggregates the two original
// targets left uncovered (EOP-FUZZ P0 `canonical_decode_envelope`):
//
//   selector % 3 == 0 → `Fiber::from_canonical_bytes` (tagged-binary path;
//                       largest fiber-side aggregate: control stack,
//                       session stack, wait state)
//   selector % 3 == 1 → `SnapshotEnvelope::decode` (Ring 1 storage format,
//                       serde_json + schema tripwire), then `state_hash()`
//                       (Ring 2 domain over hostile decoded state) and
//                       re-encode/re-decode
//   selector % 3 == 2 → `JournalRecord::decode` + re-encode/re-decode
//                       (feeds the F3 hostile-replay target)
//
// Properties under test:
// 1. Never panics/hangs/OOMs on any byte sequence (libFuzzer oracle) —
//    including `state_hash()` over a decoded-but-hostile snapshot, which
//    must return a clean `Err`, never panic (O1).
// 2. Canonical fixpoint (O6): if hostile bytes decode, the value's own
//    encoding must re-decode, and re-encoding the result must reproduce
//    the same bytes.
fuzz_target!(|data: &[u8]| {
    let Some((&selector, payload)) = data.split_first() else {
        return;
    };
    match selector % 3 {
        0 => {
            if let Ok(fiber) = Fiber::from_canonical_bytes(payload) {
                let encoded = fiber.to_canonical_bytes();
                let redecoded = Fiber::from_canonical_bytes(&encoded)
                    .expect("canonical encoding of a decoded Fiber must re-decode");
                assert_eq!(
                    encoded,
                    redecoded.to_canonical_bytes(),
                    "Fiber canonical encoding is not a fixpoint"
                );
            }
        }
        1 => {
            if let Ok(envelope) = SnapshotEnvelope::decode(payload) {
                let _ = envelope.state_hash();
                if let Ok(bytes) = envelope.canonical_bytes() {
                    SnapshotEnvelope::decode(&bytes)
                        .expect("canonical bytes of a decoded SnapshotEnvelope must re-decode");
                }
            }
        }
        _ => {
            if let Ok(record) = JournalRecord::decode(payload) {
                if let Ok(bytes) = record.canonical_bytes() {
                    JournalRecord::decode(&bytes)
                        .expect("canonical bytes of a decoded JournalRecord must re-decode");
                }
            }
        }
    }
});
