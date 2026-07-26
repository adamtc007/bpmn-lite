#![no_main]

use bpmn_lite_engine_fuzz::fault::drive_recovery;
use libfuzzer_sys::fuzz_target;

// EOP-FUZZ F8.2/F8.3 — store faults + engine restarts over a generated
// graph. R-O1 injected Unavailable (before OR after the durable effect)
// never panics; R-O2 token conservation holds across faults and engine
// restarts (job keys are redelivery-stable); R-O3 once the storm stops,
// the instance is finishable or cancellable by an engine we own — a
// stuck instance is the finding. Driver, FaultStore, and receipts live
// in the fuzz lib's `fault` module.
fuzz_target!(|data: &[u8]| {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build current-thread tokio runtime");
    runtime.block_on(drive_recovery(data));
});
