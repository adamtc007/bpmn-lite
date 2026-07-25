#![no_main]

use bpmn_lite_kernel_fuzz::{admit, gen_program, step_workflow, Tape};
use libfuzzer_sys::fuzz_target;

// EOP-FUZZ F3 — O3 replay determinism: step an admitted workflow live,
// journal every accepted transition, then `replay` the journal from
// genesis. `replay` itself re-applies every command and rejects on any
// per-record `state_hash` mismatch (`StateDivergence`), so a passing
// replay is a byte-level determinism proof; we additionally assert the
// final hashes agree. A journal the kernel produced that its own replay
// rejects — or replays to a different state — is a Ring 2 determinism
// defect.
fuzz_target!(|data: &[u8]| {
    let mut tape = Tape::new(data);
    let Some(workflow) = admit(gen_program(&mut tape)) else {
        return;
    };
    let outcome = step_workflow(&workflow, &mut tape);
    if !outcome.journal_complete || outcome.journal.is_empty() {
        // Hostile float payloads can make state_hash uncomputable; the
        // O1/O2/O4/O5 oracles inside step_workflow still ran.
        return;
    }
    let replayed =
        bpmn_lite_kernel::replay(&workflow, &outcome.genesis, &outcome.journal)
            .expect("O3: replay rejected a journal the kernel itself produced");
    let replayed_hash = replayed
        .state_hash()
        .expect("O3: replayed final state must hash");
    let live_hash = outcome
        .final_envelope
        .state_hash()
        .expect("O3: live final state must hash (journal_complete implies hashable)");
    assert_eq!(
        replayed_hash, live_hash,
        "O3: replay reproduced a different final state than live stepping"
    );
});
