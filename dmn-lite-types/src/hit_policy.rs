//! Hit policy: `HitPolicy` enum.
//!
//! Profile v0.1 hit policies: `UNIQUE` and `FIRST`.
//!
//! - `UNIQUE`: zero or one rule may match; multiple matches are a runtime error
//!   unless statically proven impossible at compile time.
//! - `FIRST`: rules evaluated in source order; first match wins; rule order
//!   is semantically meaningful and the compiler must preserve it.
//!
//! Phase 1.0 status: empty. `HitPolicy` enum defined in Phase 1.2.
