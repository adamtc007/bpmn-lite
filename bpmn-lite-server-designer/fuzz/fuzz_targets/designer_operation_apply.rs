#![no_main]

//! Fuzzes the untrusted-network entry point for the AST-mutator
//! architecture: `Vec<designer_graph::ops::Operation>` decoded from raw
//! bytes, then staged and admitted exactly as
//! `bpmn-lite-server-designer/src/rest.rs`'s `reconstruct_designer_dag`
//! (session-edit-log replay) and `session_graph_edit_endpoint` (the live
//! `SessionGraphEditBody.operations` write path) both do:
//! `serde_json::from_slice::<Vec<Operation>>` -> `apply_production` ->
//! `StagedCandidate::admit()`. Oracle: no panic anywhere in that chain,
//! for any byte sequence — a hostile-but-well-formed op tape must be
//! refused with a typed error, never crash the session it's staged
//! against.

use designer_graph::ops::Operation;
use designer_graph::productions::apply_production;
use designer_graph::schema::{DesignerDag, Provenance};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 32 * 1024;
const SEED_START_ID: &str = "start";

fn seed_start_key() -> designer_graph::schema::NodeKey {
    designer_graph::schema::NodeKey(uuid::Uuid::from_bytes([
        0xb1, 0x3f, 0x1a, 0x02, 0xd2, 0x99, 0x4a, 0x71, 0x9c, 0x3e, 0x7a, 0x21, 0x5c, 0x0e, 0x8b,
        0x44,
    ]))
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let Ok(ops) = serde_json::from_slice::<Vec<Operation>>(data) else {
        return;
    };

    let mut dag = DesignerDag::new("fuzz-designer-operation-apply");
    dag.seed(
        seed_start_key(),
        bpmn_lite_compiler::IRNode::Start {
            id: SEED_START_ID.into(),
        },
        Provenance::default(),
    )
    .expect("seeding a fresh DesignerDag with a single Start node never fails");

    let Ok(staged) = apply_production(&dag, ops, Provenance::default()) else {
        return;
    };

    // A staged candidate must always be admit-checkable: the full
    // to_ir/verify/lower theorem chain either admits or returns typed
    // VerifyErrors, never panics — this is the same guarantee
    // `session_graph_edit_endpoint` relies on before persisting anything.
    let _ = staged.candidate.admit();
});
