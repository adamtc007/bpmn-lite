use utterance_engine::{bpmn_pack, game_state, legal_moves};

fn main() {
    let _ = std::any::type_name_of_val(&bpmn_pack::semantic_snapshot_identity);
    let _ = std::any::type_name::<game_state::BpmnGameState>();
    let _ = std::any::type_name_of_val(&legal_moves::enumerate);
}
