#![no_main]

// The bpmn-lite-authoring YAML workflow-DSL frontend under hostile bytes —
// a third, distinct untrusted-text entry point into the same
// WorkflowGraphDto/IR pipeline alongside the BPMN-XML frontend
// (`bpmn-lite-engine/fuzz`'s `xml_compile`) and the S-expression DSL
// frontend (`bpmn-lite-compiler/fuzz`'s `dsl_compile`). Identified as a
// coverage gap by the 2026-08-10 repo-wide fuzz-coverage audit: `serde_yaml`
// deserializers are generally more panic-prone than `serde_json`'s
// historically hardened path, and this entry point had zero fuzz coverage.
//
// Oracles:
//   Y-O1 no-panic (parse)   — any byte sequence either parses to a
//                             `WorkflowGraphDto` via `parse_workflow_yaml`
//                             or returns a typed error; a panic in
//                             `serde_yaml`'s deserializer or in
//                             `WorkflowGraphDto`'s own Deserialize impl is
//                             the finding.
//   Y-O2 no-panic (compile) — defense-in-depth only, NOT gate parity: a
//                             successfully-parsed DTO is additionally fed
//                             through `compile_program_from_dto` (the same
//                             DTO -> IR -> bytecode pipeline
//                             `bpmn-lite-engine`'s own tests use). A
//                             grammar-valid YAML document may legitimately
//                             fail structural/graph validation (dangling
//                             edges, non-SESE topology, etc.) — only a
//                             panic there is a finding.

use bpmn_lite_authoring::{compile_program_from_dto, parse_workflow_yaml};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let source = String::from_utf8_lossy(data);

    let Ok(dto) = parse_workflow_yaml(&source) else {
        return; // Y-O1: rejection is the legal outcome for hostile bytes
    };

    // Y-O2: a grammar-valid DTO may legitimately fail to compile — only a
    // panic here is a finding.
    let _ = compile_program_from_dto(&dto);
});
