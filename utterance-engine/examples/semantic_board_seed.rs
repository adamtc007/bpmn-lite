//! Emit the canonical empty-position semantic board for fuzz seeding.

use utterance_engine::board::PolicyFilter;

fn main() {
    let state = utterance_engine::fixtures::enumeration_classes()
        .expect("enumeration fixtures")
        .into_iter()
        .find(|state| state.class_id == "empty_graph")
        .expect("empty graph fixture");
    let board = utterance_engine::bpmn_board::build_bpmn_semantic_board(
        &state.dag,
        None,
        "fuzz-seed-empty",
        &PolicyFilter::default(),
    )
    .expect("empty semantic board");
    println!("{}", serde_json::to_string(&board).unwrap());
}
