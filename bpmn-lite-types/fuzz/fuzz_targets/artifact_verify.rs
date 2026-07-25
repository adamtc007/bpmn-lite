#![no_main]

use bpmn_lite_types::ExecutableWorkflow;
use libfuzzer_sys::fuzz_target;

// Fuzz target for the full public artifact-admission path (EOP-FUZZ P0):
// `ExecutableWorkflow::verify(&[u8])` = serde_json decode → ABI tripwire →
// canonical re-encode equality → `verify_program` (V-1..V-11) → limits
// recompute. This is the exact surface a hostile artifact byte-stream hits
// at rest; fuzzing it through the public entry (fork F-B) means every
// finding is reachable in production, not just in a fuzz-only API.
//
// Properties under test:
// 1. Never panics/hangs/OOMs on any byte sequence (libFuzzer oracle).
// 2. Admission idempotence: `verify` enforces canonical == input bytes, so
//    an admitted artifact's `canonical_bytes()` must equal the input and
//    must re-admit. An admitted-then-rejected artifact would mean admission
//    depends on something outside the bytes — a determinism defect.
fuzz_target!(|data: &[u8]| {
    if let Ok(workflow) = ExecutableWorkflow::verify(data) {
        let canonical = workflow
            .canonical_bytes()
            .expect("admitted artifact must re-encode canonically");
        assert_eq!(
            canonical.as_slice(),
            data,
            "verify admitted bytes that are not their own canonical encoding"
        );
        ExecutableWorkflow::verify(&canonical)
            .expect("canonical bytes of an admitted artifact must re-admit");
    }
});
