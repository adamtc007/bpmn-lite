// scripts/fixtures/glossary_violations.rs
//
// Deliberately violates every rule enforced by scripts/check-glossary.sh
// (EOP-PLAN-BPMN-ISA-002 V1.5). Not compiled — referenced only by
// scripts/check-glossary.sh --self-test, which asserts each rule's pattern
// fires against this fixture. If a future edit to the lint's patterns stops
// catching one of these, the self-test fails loudly instead of the gate
// going quietly toothless.

// Violation 1: bytecode/instruction type named Token (bound term: Instr / cell).
enum Token {
    Push,
    Jump,
}

// Violation 2: runtime state named after Boundary event (bound term: Guard).
struct BoundaryEventState {
    armed: bool,
}

// Violation 3: concurrency table named after Scope (bound term: ConcurrencyTable).
struct ScopeTable {
    records: Vec<u32>,
}
