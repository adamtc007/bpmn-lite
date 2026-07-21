#![forbid(unsafe_code)]

use bpmn_lite_kernel::{DeterministicContext, apply};
use bpmn_lite_types::session_stack::SessionStackState;
use bpmn_lite_types::{
    Addr, ArtifactEnvelope, Command, EffectId, ExecutableWorkflow, Fiber, Instr, ProcessInstance,
    ProcessState, Snapshot, Uuid, legacy_program,
};
use std::collections::BTreeMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let program = legacy_program! {
        bytecode_version: [7u8; 32],
        program: vec![Instr::Fork { targets: vec![Addr::new(1), Addr::new(2)].into() }, Instr::End, Instr::End],
        debug_map: BTreeMap::new(),
        join_plan: BTreeMap::new(),
        wait_plan: BTreeMap::new(),
        message_name_map: BTreeMap::new(),
        race_plan: BTreeMap::new(),
        boundary_map: BTreeMap::new(),
        write_set: BTreeMap::new(),
        task_manifest: vec![],
        error_route_map: BTreeMap::new(),
        flag_symbol_table: BTreeMap::new(),
        data_objects: BTreeMap::new(),
        ffi_task_decls: BTreeMap::new(),
    };
    let workflow = ExecutableWorkflow::from_verified_envelope(
        ArtifactEnvelope::from_legacy_program(program, "wasm-replay-fixture")?,
    )?;
    let instance_id = Uuid::from_u128(11);
    let instance = ProcessInstance {
        instance_id,
        tenant_id: "tenant-a".to_string(),
        process_key: "wasm-replay-fixture".to_string(),
        bytecode_version: workflow.hash().into_bytes(),
        domain_payload: "{}".into(),
        domain_payload_hash: EffectId::content_hash(b"{}"),
        session_stack: SessionStackState::default(),
        flags: BTreeMap::new(),
        counters: BTreeMap::new(),
        join_expected: BTreeMap::new(),
        state: ProcessState::Running,
        correlation_id: "fixture".to_string(),
        entry_id: Uuid::nil(),
        runbook_id: Uuid::nil(),
        created_at: 1,
        integrity_hash: None,
        quarantine_state: None,
        plan_hash: None,
        current_node_id: None,
        placeholder_values: None,
    };
    let snapshot = Snapshot::new(instance, [Fiber::new(Uuid::from_u128(12), 0)]);
    let context = DeterministicContext::new(100, Uuid::from_u128(13), 1);
    let transition = apply(
        &workflow,
        &snapshot,
        &Command::Tick { fiber_id: None },
        &context,
    )?;
    for byte in transition.canonical_bytes()? {
        print!("{byte:02x}");
    }
    println!();
    Ok(())
}
