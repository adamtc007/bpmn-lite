use utterance_engine::policy::DispositionConfig;

fn main() {
    let _ = utterance_engine::pair::PAIR_SERIALIZER_ID;
    let _ = utterance_engine::bpmn_board::bpmn_semantic_snapshot_identity();
    let _ = utterance_engine::bpmn_board::project_design_position;
    let _ = DispositionConfig::shadow_v2().policy_hash();
}
