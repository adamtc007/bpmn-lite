#![no_main]

use bpmn_lite_kernel_fuzz::{admit, gen_program, Tape};
use libfuzzer_sys::fuzz_target;

// EOP-FUZZ F3 (P1) — verifier-admission throughput target: the same
// structured generator as kernel_step but WITHOUT the stepping loop, so
// every exec is one `verify_program` pass (via the public
// `from_legacy_program` → `from_verified_envelope` path, fork F-B) at
// maximum rate. This subsumes v2_verifier.rs's never-panics proptest with
// coverage feedback: the generator's hostile arm (dangling jumps,
// unpaired joins, stack probes, corr-source metadata off message words)
// keeps the reject branches hot while the structured arms keep the
// admit/abstract-interpretation branches hot.
//
// Oracle: O1 — admission must return Some/None, never panic/hang/OOM.
fuzz_target!(|data: &[u8]| {
    let mut tape = Tape::new(data);
    let _ = admit(gen_program(&mut tape));
});
