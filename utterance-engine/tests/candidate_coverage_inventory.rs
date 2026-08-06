use std::collections::BTreeSet;

use designer_graph::board_candidate::{OperationKind, ProductionId};

#[test]
fn phase_one_inventory_matches_the_complete_candidate_catalogue() {
    let inventory: serde_json::Value = serde_json::from_str(include_str!(
        "../../docs/receipts/bpmn-candidate-coverage-v3.json"
    ))
    .expect("coverage inventory parses");
    let recorded = inventory["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .map(|candidate| candidate["id"].as_str().expect("candidate id"))
        .collect::<BTreeSet<_>>();
    let catalogue = OperationKind::ALL
        .iter()
        .map(|candidate| candidate.canonical_id())
        .chain(
            ProductionId::ALL
                .iter()
                .map(|candidate| candidate.canonical_id()),
        )
        .collect::<BTreeSet<_>>();

    assert_eq!(recorded, catalogue);
    assert_eq!(recorded.len(), 26);
    assert!(inventory["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .all(|candidate| candidate["semantic_contract"] == false));
}
