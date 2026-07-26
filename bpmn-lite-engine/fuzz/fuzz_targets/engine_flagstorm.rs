#![no_main]

use bpmn_lite_engine_fuzz::drive_flag_storm;
use libfuzzer_sys::fuzz_target;

// EOP-FUZZ F8.7 — inconsistent flag histories over generated graphs:
// every completion re-draws every routing flag from the tape, hammering
// split evaluation, guard rollback snapshots, and OR-subset
// synchronization under mid-run re-routing. Oracles: S-O1 no-panic;
// S-O2 activated tasks belong to the shape; S-O3 cancel discipline.
// Intent-derived conservation bounds are deliberately NOT asserted here
// (unsound under mutable flags) — engine_graph remains the conservation
// tier. Driver and receipts live in the fuzz lib.
fuzz_target!(|data: &[u8]| {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build current-thread tokio runtime");
    runtime.block_on(drive_flag_storm(data));
});
