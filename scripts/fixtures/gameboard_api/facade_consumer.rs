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
    let _ = utterance_engine::bpmn_board::project_bpmn_attempt_history;
    let _ = utterance_engine::bpmn_board::record_bpmn_attempt;
    let _ = utterance_engine::bpmn_board::update_bpmn_design_belief;
    let _ = utterance_engine::bpmn_board::decide_bpmn_game_disposition;
    let _ = utterance_engine::bpmn_board::render_bpmn_game_disposition;
    let _ = utterance_engine::StructuredChoiceFitConfig::new;
    let _ = utterance_engine::StructuredChoiceModel::fit;
    let _ = utterance_engine::StructuredChoiceCalibration::fit_validation;
    let _ = utterance_engine::ResolverBoardPacket::ranked;
    let _ = utterance_engine::BoundedOfflineRankerRequest::new;
    let _ = utterance_engine::compare_resolvers;
    let _ = utterance_engine::MAX_RESOLVER_BOARD_SIZE;
    let _ = DispositionConfig::shadow_v2().policy_hash();
}
