//! Differential testing harness entry point — Phase 1.5.
//!
//! cargo test --test differential_runner
//! cargo test --release --test differential_runner     (preferred: ~10× faster)
//!
//! This binary proves that `vm::evaluate` and `reference::evaluate` produce
//! equivalent results across ≥1000 generated inputs per EBNF fixture per the
//! §8 equivalence contract in `docs/dmn-lite-bytecode.md`.

mod differential;
