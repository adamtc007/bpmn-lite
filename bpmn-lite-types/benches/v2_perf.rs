#![forbid(unsafe_code)]
//! T11 (narrow) perf harness for the v2 frame format. Emits machine-readable
//! `key=value` metrics for the canonical STATE-frame encoder: per-fibre frame
//! cost (the "frame ∝ live tokens" claim, also gated exactly in the
//! persistence unit tests), encode latency, and the encode's output-buffer
//! allocation. The commits-∝-waits claim is proven in the kernel test suite
//! (it needs the kernel drive loop), not here.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use bpmn_lite_types::concurrency::ConcurrencyTable;
use bpmn_lite_types::persistence::PersistedSnapshotState;
use bpmn_lite_types::{
    Addr, EffectId, Fiber, ProcessInstance, ProcessState, Uuid,
    session_stack::SessionStackState,
};

fn instance() -> ProcessInstance {
    ProcessInstance {
        instance_id: Uuid::from_u128(1),
        tenant_id: "tenant".to_string(),
        process_key: "process".to_string(),
        bytecode_version: [1; 32],
        domain_payload: Arc::from("{}"),
        domain_payload_hash: EffectId::content_hash(b"{}"),
        session_stack: SessionStackState::default(),
        flags: BTreeMap::new(),
        counters: BTreeMap::new(),
        join_expected: BTreeMap::new(),
        state: ProcessState::Running,
        correlation_id: "correlation".to_string(),
        entry_id: Uuid::nil(),
        runbook_id: Uuid::nil(),
        created_at: 1,
        integrity_hash: None,
        quarantine_state: None,
        plan_hash: None,
        current_node_id: None,
        placeholder_values: None,
    }
}

fn state_with(live_fibers: usize) -> PersistedSnapshotState {
    let fibers = (0..live_fibers)
        .map(|index| Fiber::new(Uuid::from_u128(1_000 + index as u128), Addr::new(0)));
    PersistedSnapshotState::new(
        instance(),
        fibers,
        BTreeMap::new(),
        [],
        ConcurrencyTable::new(),
        [],
    )
}

fn frame_bytes(live_fibers: usize) -> usize {
    state_with(live_fibers)
        .try_canonical_hash_bytes()
        .unwrap()
        .len()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Frame ∝ live tokens: the exact per-fibre marginal byte cost.
    let base = frame_bytes(0);
    let per_fiber = frame_bytes(1) - base;

    // Latency: encode a representative 16-fibre frame repeatedly.
    let representative = state_with(16);
    const ITERATIONS: usize = 50_000;
    let start = Instant::now();
    let mut checksum = 0usize;
    for _ in 0..ITERATIONS {
        checksum = checksum.saturating_add(representative.try_canonical_hash_bytes()?.len());
    }
    let ns_per_encode = start.elapsed().as_nanos() / ITERATIONS as u128;

    // The encode's output buffer is its dominant heap allocation.
    let encoded = representative.try_canonical_hash_bytes()?;
    checksum = checksum.saturating_add(encoded.len());

    // Machine-independent gate: the frame is exactly linear in live tokens.
    // A superlinear term (program, history) creeping into the STATE frame
    // breaks this equality — running the bench is itself the check.
    assert!(per_fiber > 0, "one live fibre must add bytes");
    assert_eq!(
        encoded.len(),
        base + 16 * per_fiber,
        "16-fibre frame must equal base + 16·per_fiber (frame ∝ live tokens)"
    );

    println!(
        "frame_base_bytes={base} frame_bytes_per_fiber={per_fiber} \
         canonical_ns_per_encode={ns_per_encode} \
         representative_frame_bytes={} checksum={checksum}",
        encoded.len(),
    );
    Ok(())
}
