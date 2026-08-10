#![no_main]

// `bpmn-lite-authoring::import_zeebe_bpmn` under hostile bytes — the
// SESE-pairing/restructuring layer identified as a coverage gap by the
// 2026-08-10 repo-wide fuzz-coverage audit. This is a DISTINCT code path
// from `bpmn-lite-engine/fuzz`'s `xml_compile` target: `bpmn-lite-engine`
// depends on `bpmn-lite-authoring` only as a dev-dependency (used solely by
// its own cross-crate compatibility tests), and `BpmnLiteEngine::compile`
// calls `bpmn_lite_compiler::parse_bpmn` directly — it never reaches
// `import_zeebe_bpmn`. `import_zeebe_bpmn` does its own additional
// split/join pairing and (when `permissive`) topology-restructuring work on
// top of the same `parse_bpmn` frontend `xml_compile` already covers, so
// this target's job is specifically that extra layer, not re-covering the
// XML frontend itself.
//
// Oracle:
//   Z-O1 no-panic — any byte sequence either imports to a
//                   `WorkflowExecutionPlan` or returns a typed error, under
//                   BOTH `permissive` settings (false and true exercise
//                   materially different control flow: strict SESE
//                   rejection vs. best-effort restructuring). A panic in
//                   XML parsing, split/join pairing, or the permissive
//                   restructuring path is the finding.

use bpmn_lite_authoring::import_zeebe_bpmn;
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let xml = String::from_utf8_lossy(data);

    let _ = import_zeebe_bpmn(&xml, "fuzz-wf", false);
    let _ = import_zeebe_bpmn(&xml, "fuzz-wf", true);
});
