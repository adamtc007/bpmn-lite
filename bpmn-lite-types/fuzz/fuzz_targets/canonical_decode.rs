#![no_main]

use bpmn_lite_types::{CanonicalEncode, ConcurrencyTable};
use libfuzzer_sys::fuzz_target;

// Fuzz target for the canonical decoder's top-level aggregate entry point.
//
// `ConcurrencyTable` is the largest/most nested type exercising the
// tagged-binary `CanonicalEncode` decode path in one call: a `BTreeMap` of
// `ConcurrencyRecord`, each of which nests `RecordKind`/`RecordState`/
// `RecordCounters` — so a single `from_canonical_bytes` call here walks
// most of the decoder's tag/length/nested-container logic.
//
// Property under test: `from_canonical_bytes` must never panic, never hang,
// never OOM on ANY byte sequence — it must always return either `Ok` or a
// clean `Err(CanonicalDecodeError)`. No further assertions are needed;
// libFuzzer's own crash/hang/OOM detection is the oracle.
fuzz_target!(|data: &[u8]| {
    let _ = ConcurrencyTable::from_canonical_bytes(data);
});
