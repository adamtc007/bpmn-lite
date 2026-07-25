#![no_main]

use bpmn_lite_engine_fuzz::drive_graph;
use libfuzzer_sys::fuzz_target;

// EOP-FUZZ F6 — the SHAPE is the fuzzed variable: tape → SESE shape
// grammar → real BPMN XML → REAL compiler (G-A must-admit: a rejection is
// a lowering finding) → tape-driven token interleavings under the
// shape-derived conservation oracle (G-T: per task, distinct job keys ≤
// the bound the authored shape implies) + E-O5 cancel discipline.
// Grammar, emission, oracles, and receipts live in the fuzz lib.
fuzz_target!(|data: &[u8]| {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build current-thread tokio runtime");
    runtime.block_on(drive_graph(data));
});
