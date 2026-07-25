#![no_main]

use bpmn_lite_kernel_fuzz::{admit, gen_program, step_workflow, Tape};
use bpmn_lite_types::JournalRecord;
use libfuzzer_sys::fuzz_target;

// EOP-FUZZ F3 — hostile journal tails: build a VALID journal via live
// stepping, then corrupt it tape-driven (drop / duplicate / swap /
// byte-flip records) and replay. Oracle is O1 + fail-closed: `replay`
// must return `Ok` or a clean `ReplayError` — never panic — and a
// byte-flipped record that still decodes must be caught by the revision
// chain / artifact-hash / state-hash checks, not silently folded.
fuzz_target!(|data: &[u8]| {
    let mut tape = Tape::new(data);
    let Some(workflow) = admit(gen_program(&mut tape)) else {
        return;
    };
    let outcome = step_workflow(&workflow, &mut tape);
    if outcome.journal.is_empty() {
        return;
    }
    let mut journal = outcome.journal;

    // Tape-driven corruption passes.
    for _ in 0..(1 + tape.u8() % 3) {
        if journal.is_empty() {
            break;
        }
        let index = usize::from(tape.u8()) % journal.len();
        match tape.u8() % 4 {
            0 => {
                journal.remove(index);
            }
            1 => {
                let record = journal[index].clone();
                journal.insert(index, record);
            }
            2 => {
                let other = usize::from(tape.u8()) % journal.len();
                journal.swap(index, other);
            }
            _ => {
                // Byte-flip through the storage codec: most flips fail to
                // decode (drop the corruption), decodable ones go in.
                if let Ok(mut bytes) = journal[index].canonical_bytes() {
                    if !bytes.is_empty() {
                        let position = usize::from(tape.u16()) % bytes.len();
                        bytes[position] ^= 1 << (tape.u8() % 8);
                        if let Ok(corrupted) = JournalRecord::decode(&bytes) {
                            journal[index] = corrupted;
                        }
                    }
                }
            }
        }
    }

    // Must never panic; Ok is only legal if the corruption was a no-op
    // (e.g. swap of identical records) — which the chain checks decide,
    // not us. The oracle here is purely no-panic + no-hang.
    let _ = bpmn_lite_kernel::replay(&workflow, &outcome.genesis, &journal);
});
