//! Typed predicate IR: `Predicate`, field references, typed value operands.
//!
//! The predicate IR is the compiler output and the reference evaluator input.
//! It preserves the semantic meaning of each rule test after symbol resolution
//! and type/domain checking. The bytecode VM does not use this IR directly;
//! the compiler lowers it to the instruction stream in `instr`.
//!
//! Profile v0.1 predicates: `Any`, `Eq`, `NotEq`, `InSet`, `Range`,
//! `IsNull`, `IsNotNull`, `All`, `AnyOf`, `Not`.
//!
//! Phase 1.0 status: empty. Predicate IR defined in Phase 1.2.
