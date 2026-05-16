//! Bytecode instruction set for the dmn-lite stack VM.
//!
//! Specified in `docs/dmn-lite-bytecode.md` §3.  Every `Instr` variant has a
//! well-defined stack effect; the verifier enforces these invariants before
//! any execution occurs.
//!
//! Reserved variants (§3.9) are included for enum stability across profiles.
//! The v0.1 emitter **must not** produce them; the verifier rejects any v0.1
//! artifact that contains them.

use crate::ids::{
    AggregateOpKind, BindingId, BkmId, ConstId, ConstSetId, FieldId, OutputFieldId, PathId,
    RangeId, RuleId,
};

/// A single bytecode instruction executed by the dmn-lite stack VM.
///
/// Stack effects and full semantics are in `docs/dmn-lite-bytecode.md` §3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instr {
    // === Stack manipulation (§3.1) ===
    /// Push an input field's value onto the data stack.
    ///
    /// Stack: `[] → [value]`
    LoadField(FieldId),

    /// Push an early-bound constant from the const pool.
    ///
    /// Stack: `[] → [value]`
    PushConst(ConstId),

    /// Push a precompiled constant set from the set pool.
    ///
    /// Stack: `[] → [set]`
    PushConstSet(ConstSetId),

    /// Discard the top stack value.
    ///
    /// Stack: `[v] → []`
    Pop,

    /// Duplicate the top stack value.
    ///
    /// Stack: `[v] → [v, v]`
    Dup,

    // === Comparison (§3.2) ===
    /// `a == b` under typed equality.
    ///
    /// Stack: `[a, b] → [bool]`
    Eq,

    /// `!(a == b)` — inequality.
    ///
    /// Stack: `[a, b] → [bool]`
    NotEq,

    /// `a < b` (numeric types only).
    ///
    /// Stack: `[a, b] → [bool]`
    Lt,

    /// `a <= b` (numeric types only).
    ///
    /// Stack: `[a, b] → [bool]`
    Le,

    /// `a > b` (numeric types only).
    ///
    /// Stack: `[a, b] → [bool]`
    Gt,

    /// `a >= b` (numeric types only).
    ///
    /// Stack: `[a, b] → [bool]`
    Ge,

    // === Set membership (§3.3) ===
    /// `value ∈ set` — set membership test.
    ///
    /// Stack: `[value, set] → [bool]`
    InSet,

    // === Range tests (§3.4) ===
    /// Range membership test against a precompiled range from the pool.
    ///
    /// Stack: `[value] → [bool]`
    RangeCheck(RangeId),

    // === Null tests (§3.5) ===
    /// `value is null` — true for missing or explicit-null values.
    ///
    /// Stack: `[value] → [bool]`
    IsNull,

    /// `value is not null` — true for present non-null values.
    ///
    /// Stack: `[value] → [bool]`
    IsNotNull,

    // === Boolean combinators (§3.6) ===
    /// `a && b` — both operands already on the stack (no short-circuit).
    ///
    /// Stack: `[a, b] → [bool]`
    And,

    /// `a || b` — both operands already on the stack (no short-circuit).
    ///
    /// Stack: `[a, b] → [bool]`
    Or,

    /// `!a` — boolean negation.
    ///
    /// Stack: `[a] → [bool]`
    Not,

    // === Control flow (§3.7) ===
    /// Unconditional jump to an absolute instruction address.
    ///
    /// Stack: no effect
    Br(u32),

    /// Pop the top boolean; jump to `addr` if it is `false`.
    ///
    /// Stack: `[bool] → []`
    BrFalse(u32),

    /// Pop the top boolean; jump to `addr` if it is `true`.
    ///
    /// Stack: `[bool] → []`
    BrTrue(u32),

    // === Rule and output (§3.8) ===
    /// Record that rule `RuleId` has matched in the accumulator.
    ///
    /// Stack: no effect
    RuleMatched(RuleId),

    /// Pop the top value and write it to the output frame at `OutputFieldId`.
    ///
    /// Stack: `[value] → []`
    StoreOutputTos(OutputFieldId),

    /// Push the constant at `ConstId` and immediately write it to the output
    /// frame at `OutputFieldId`.  Compound shorthand for `PushConst + StoreOutputTos`.
    ///
    /// Stack: no effect (push then pop cancel out)
    StoreOutput(OutputFieldId, ConstId),

    /// Terminate execution and apply the hit policy.
    ///
    /// Stack: no effect (terminates)
    EndDecision,

    // === Reserved for future profiles (§3.9) ===
    // The v0.1 emitter MUST NOT produce these.  The verifier rejects any
    // v0.1 artifact that contains them.
    /// (Profile v0.5+) Call a governed function / BKM.
    #[doc(hidden)]
    Call(BkmId),

    /// (Profile v0.5+) Return from a governed function / BKM.
    #[doc(hidden)]
    Return,

    /// (Profile v0.2+) Begin a universal quantifier loop.
    #[doc(hidden)]
    ForAllBegin {
        /// The collection field being iterated.
        collection: FieldId,
        /// Bound variable slot.
        bound_var: BindingId,
        /// Jump target when the quantifier is exhausted.
        end: u32,
    },

    /// (Profile v0.2+) End a universal quantifier loop.
    #[doc(hidden)]
    ForAllEnd,

    /// (Profile v0.2+) Begin a bounded aggregation loop.
    #[doc(hidden)]
    AggregateBegin {
        /// The collection field being iterated.
        collection: FieldId,
        /// Bound variable slot.
        bound_var: BindingId,
        /// Aggregation operation.
        op: AggregateOpKind,
        /// Jump target when the aggregation is complete.
        end: u32,
    },

    /// (Profile v0.2+) End a bounded aggregation loop.
    #[doc(hidden)]
    AggregateEnd,

    /// (Profile v0.4+) Load a value via a path expression.
    #[doc(hidden)]
    LoadPath(PathId),
}

impl Instr {
    /// True if this is a v0.2+ reserved instruction that the verifier rejects
    /// in Profile v0.1 artifacts.
    pub fn is_reserved(&self) -> bool {
        matches!(
            self,
            Self::Call(_)
                | Self::Return
                | Self::ForAllBegin { .. }
                | Self::ForAllEnd
                | Self::AggregateBegin { .. }
                | Self::AggregateEnd
                | Self::LoadPath(_)
        )
    }

    /// Human-readable mnemonic for diagnostics and test output.
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Self::LoadField(_) => "LoadField",
            Self::PushConst(_) => "PushConst",
            Self::PushConstSet(_) => "PushConstSet",
            Self::Pop => "Pop",
            Self::Dup => "Dup",
            Self::Eq => "Eq",
            Self::NotEq => "NotEq",
            Self::Lt => "Lt",
            Self::Le => "Le",
            Self::Gt => "Gt",
            Self::Ge => "Ge",
            Self::InSet => "InSet",
            Self::RangeCheck(_) => "RangeCheck",
            Self::IsNull => "IsNull",
            Self::IsNotNull => "IsNotNull",
            Self::And => "And",
            Self::Or => "Or",
            Self::Not => "Not",
            Self::Br(_) => "Br",
            Self::BrFalse(_) => "BrFalse",
            Self::BrTrue(_) => "BrTrue",
            Self::RuleMatched(_) => "RuleMatched",
            Self::StoreOutputTos(_) => "StoreOutputTos",
            Self::StoreOutput(_, _) => "StoreOutput",
            Self::EndDecision => "EndDecision",
            Self::Call(_) => "Call",
            Self::Return => "Return",
            Self::ForAllBegin { .. } => "ForAllBegin",
            Self::ForAllEnd => "ForAllEnd",
            Self::AggregateBegin { .. } => "AggregateBegin",
            Self::AggregateEnd => "AggregateEnd",
            Self::LoadPath(_) => "LoadPath",
        }
    }
}
