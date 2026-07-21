// Deliberate-violation fixture for scripts/check-canonical-invariant.sh
// --self-test (EOP-PLAN-BPMN-ISA-002 V2.1h.6). Not compiled, not part of
// any crate — proves the guard fires on serde_json::to_value reaching a
// hash-domain JSON field, the exact pattern found live in
// bpmn-lite-server/src/rest.rs during the V2.1h remediation
// (`inst.placeholder_values = serde_json::to_value(&pv).ok();`).

fn update_placeholder(inst: &mut ProcessInstance, pv: HashMap<String, serde_json::Value>) {
    // Deliberate violation: serde_json::to_value assigned into
    // placeholder_values. check-canonical-invariant.sh must detect this.
    inst.placeholder_values = serde_json::to_value(&pv).ok();
}
