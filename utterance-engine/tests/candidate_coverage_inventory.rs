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
    // G1.3 (2026-08-11): removed 5 EXCLUDED-BY-DESIGN entries with no
    // construction path (`op.attach_rollback_guard`, `op.call_subprocess`,
    // `op.create_race`, `prod.call_durable_subprocess`,
    // `prod.timer_message_race`) from both the live catalogue and this
    // receipt -- 26 -> 21. Discovered missed during G2 work: G1.3's own
    // verification never ran this integration test binary.
    // G7.2 (2026-08-13): added `op.create_data_object` -- 21 -> 22 (see
    // EOP-PLAN-BPMN-DESIGN-003 G7, super-user REPL test finding).
    assert_eq!(recorded.len(), 22);
    assert!(inventory["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .all(|candidate| candidate["semantic_contract"] == false));
}
