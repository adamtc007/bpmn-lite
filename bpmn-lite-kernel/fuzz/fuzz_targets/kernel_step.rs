#![no_main]

use bpmn_lite_kernel_fuzz::{admit, gen_program, step_workflow, Tape};
use libfuzzer_sys::fuzz_target;

// EOP-FUZZ F2 flagship: byte tape → structured program generator → REAL
// public admission path (rejects are cheap and discarded) → adversarial
// command sequence against `kernel::apply` from start to quiescence/bound.
//
// Oracles (asserted inside `step_workflow`, so every violation is a
// libFuzzer crash with a reproducing input):
//   O1 no-panic, O2 K-invariants after every accepted transition,
//   O4 observed peaks ≤ VerifiedLimits, O5 Terminate always succeeds on
//   reachable non-terminal states.
fuzz_target!(|data: &[u8]| {
    let mut tape = Tape::new(data);
    let Some(workflow) = admit(gen_program(&mut tape)) else {
        return;
    };
    let _ = step_workflow(&workflow, &mut tape);
});
