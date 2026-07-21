// Deliberate-violation fixture for scripts/check-canonical-invariant.sh
// --self-test (EOP-PLAN-BPMN-ISA-002 V2.1h.1). Not compiled, not part of
// any crate — proves the lint fires on a HashMap swap in a hash-domain
// type, rather than being a silently-toothless check.

use std::collections::HashMap;

pub struct Fiber {
    pub fiber_id: Uuid,
    pub pc: Addr,
    pub stack: Vec<Value>,
    pub regs: [Value; 8],
    pub wait: WaitState,
    pub loop_epoch: u32,
    pub control_stack: Vec<Handle>,
    // Deliberate violation: a HashMap field inside a hash-domain type.
    // check-canonical-invariant.sh must detect this.
    pub tag_scratch: HashMap<u32, u32>,
}
