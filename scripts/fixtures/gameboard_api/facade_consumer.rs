use utterance_engine::policy::DispositionConfig;

fn main() {
    let _ = utterance_engine::pair::PAIR_SERIALIZER_ID;
    let _ = utterance_engine::bpmn_board::bpmn_semantic_snapshot_identity();
    let _ = utterance_engine::bpmn_board::project_design_position;
    let _ = utterance_engine::bpmn_board::build_bpmn_design_position;
    let _ = utterance_engine::bpmn_board::materialize_bpmn_workbook;
    let _ = utterance_engine::bpmn_board::preview_bpmn_workbook;
    let _ = utterance_engine::bpmn_board::bpmn_legal_move_id_for_operation;
    let _ = utterance_engine::bpmn_board::explain_bpmn_candidate;
    let _ = DispositionConfig::shadow_v2().policy_hash();
}
