#![no_main]

use bpmn_lite_engine_fuzz::drive_xml_compile;
use libfuzzer_sys::fuzz_target;

// EOP-FUZZ F8.1 — arbitrary bytes at the XML frontend. The
// parser/lowering/verifier chain is the system under test against HOSTILE
// input: X-O1 any bytes either compile or reject without panic; X-O2
// whatever compile ADMITS must then start, step, and cancel cleanly
// (admission is a promise, not a parse). Driver and receipts live in the
// fuzz lib.
fuzz_target!(|data: &[u8]| {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build current-thread tokio runtime");
    runtime.block_on(drive_xml_compile(data));
});
