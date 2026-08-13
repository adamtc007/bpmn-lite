#![no_main]

// The direct-JSON `plan_body` ingestion path: `bpmn-lite-bus-handler`'s
// dispatch (`lib.rs`, the `dsl_body`-absent branch) accepts a
// `WorkflowExecutionPlan` straight from a wire-supplied JSON string —
// bypassing `dsl::compile` entirely — then calls
// `reverify_preserving_distrust()` before the plan ever reaches the
// engine. This is the one place a caller-shaped JSON blob, not a proved
// compile output, can become an execution plan; CLAUDE.md's keystone is
// "the compiler binds, runtime executes only concrete governed atomics" —
// this target fuzzes the boundary where that guarantee is re-derived
// rather than assumed.
//
// Oracles:
//   no-panic      — any byte sequence either deserializes to a
//                   WorkflowExecutionPlan or fails with a serde error; a
//                   panic in Deserialize or in reverify_preserving_distrust
//                   (which recomputes analyze_safety's SESE/guard-escape
//                   proof from the raw node graph) is the finding.
//   proof cannot  — a forged `mathematically_proved: true` on a
//   be forged     — structurally invalid graph (SESE violation, dangling
//                   guard escape, unsupported-node bypass) must not
//                   survive reverify_preserving_distrust: if
//                   analyze_safety records a breach, mathematically_proved
//                   must end up false, regardless of what the JSON
//                   claimed on the wire.

use bpmn_lite_compiler::dsl::WorkflowExecutionPlan;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(mut plan) = serde_json::from_str::<WorkflowExecutionPlan>(text) else {
        return;
    };
    plan.reverify_preserving_distrust();
    if !plan.unsafe_breeches().is_empty() {
        assert!(
            !plan.mathematically_proved(),
            "plan has recorded breaches {:?} but mathematically_proved() is still true",
            plan.unsafe_breeches()
        );
    }
});
