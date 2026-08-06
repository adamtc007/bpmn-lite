use std::any::TypeId;

#[test]
fn compatibility_and_direct_contract_paths_are_identical() {
    assert_eq!(
        TypeId::of::<semantic_decision_contracts::SemanticDecisionBoard>(),
        TypeId::of::<sem_os_policy::decision_board::SemanticDecisionBoard>()
    );
    assert_eq!(
        TypeId::of::<semantic_decision_contracts::InferenceEvidence>(),
        TypeId::of::<sem_os_policy::decision_board::InferenceEvidence>()
    );
    assert_eq!(
        TypeId::of::<semantic_decision_contracts::ProposalWorkbook>(),
        TypeId::of::<sem_os_policy::decision_board::ProposalWorkbook>()
    );
}
