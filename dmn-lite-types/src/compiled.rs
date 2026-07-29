//! Compiled decision artifact: `CompiledDecision` and `VerifiedDecision`.
//!
//! `CompiledDecision` is the immutable output of `dmn-lite-compiler::emit()`.
//! It contains the bytecode program, pool tables, source map, schemas, and the
//! preserved `TypedDecision` for reference-evaluator use.
//!
//! `VerifiedDecision` is a `CompiledDecision` that has passed the bytecode
//! verifier (`dmn-lite-compiler::verify()`).  **Only `verify()` should call
//! `VerifiedDecision::new_verified()`**; the stack VM accepts only this type.
//! See §3.11 of the Phase 1.4 prompt for the documentary-discipline rationale.

use std::sync::Arc;
use std::time::SystemTime;

use crate::ids::{DecisionId, RuleId, SnapshotId, SourceSpan};
use crate::instr::Instr;
use crate::ir::{FieldSchema, HitPolicy, TypedDecision, TypedValue};

// ── Supporting types ──────────────────────────────────────────────────────────

/// A single entry in the range pool — precompiled bounds for a `RangeCheck`.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeEntry {
    /// Lower bound value; `None` = unbounded.
    pub lower: Option<TypedValue>,
    /// Upper bound value; `None` = unbounded.
    pub upper: Option<TypedValue>,
    /// True when the lower bound is inclusive (`[`).
    pub lower_inclusive: bool,
    /// True when the upper bound is inclusive (`]`).
    pub upper_inclusive: bool,
}

/// An entry in the rule map: maps a rule to its bytecode entry address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMapEntry {
    /// Ordinal index of the rule.
    pub rule_id: RuleId,
    /// Rule identifier as written in the source (e.g., `"r001"`).
    pub rule_name: String,
    /// Absolute instruction address where this rule's bytecode begins.
    pub entry_addr: u32,
    /// Source span of the `(rule ...)` form.
    pub source_span: SourceSpan,
}

/// Metadata recorded alongside the compiled artifact.
#[derive(Debug, Clone)]
pub struct CompileContext {
    /// Sem OS snapshot ID used during compilation (metadata, not part of the
    /// artifact hash per `dmn-lite-semantics.md` §3.2.4).
    pub sem_os_snapshot_id: SnapshotId,
    /// Wall-clock time when compilation completed.
    pub compiled_at: SystemTime,
    /// `CARGO_PKG_VERSION` of `dmn-lite-compiler` at build time.
    pub compiler_version: String,
}

/// BLAKE3 artifact hash (32 bytes).
///
/// Computed over normalised source + resolved entity IDs + compiled IR per
/// `dmn-lite-semantics.md` §3.2.4.  Two compilations of identical source
/// against identical catalogue produce the same hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArtifactHash([u8; 32]);

impl ArtifactHash {
    /// Zero hash used as a placeholder before the hash is computed.
    pub const ZERO: Self = Self([0u8; 32]);

    /// Wrap a raw BLAKE3 digest.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw 32-byte digest.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

// ── CompiledDecision ──────────────────────────────────────────────────────────

/// Immutable compiled artifact produced by `dmn-lite-compiler`.
///
/// Contains the bytecode program, pool tables, schemas, rule map, artifact hash,
/// compile-time metadata, and the preserved `TypedDecision` for the reference
/// evaluator and Phase 1.5 differential testing.
///
/// A `CompiledDecision` is **unverified** until it passes `verify()`.  Pass it
/// to `verify()` to obtain a `VerifiedDecision` that the stack VM accepts.
#[derive(Debug, Clone)]
pub struct CompiledDecision {
    /// Decision identity.
    pub decision_id: DecisionId,
    /// Decision name as written in source.
    pub name: String,
    /// Hit policy.
    pub hit_policy: HitPolicy,
    /// Input field schema in source order.
    pub input_schema: Vec<FieldSchema>,
    /// Output field schema in source order.
    pub output_schema: Vec<FieldSchema>,

    /// Early-bound constant values, indexed by `ConstId`.
    pub const_pool: Vec<TypedValue>,
    /// Precompiled constant sets for `InSet`, indexed by `ConstSetId`.
    /// Each set is stored sorted for determinism.
    pub const_set_pool: Vec<Arc<[TypedValue]>>,
    /// Precompiled range entries for `RangeCheck`, indexed by `RangeId`.
    pub range_pool: Vec<RangeEntry>,

    /// The bytecode instruction stream.
    pub instructions: Vec<Instr>,
    /// Source spans parallel to `instructions` (same length).
    pub source_spans: Vec<SourceSpan>,
    /// Mapping from `RuleId` to the instruction address where that rule starts.
    pub rule_map: Vec<RuleMapEntry>,

    /// Artifact hash per `dmn-lite-semantics.md` §3.2.4.
    pub artifact_hash: ArtifactHash,
    /// Compile-time metadata.
    pub compile_context: CompileContext,

    /// The typed predicate IR preserved for the reference evaluator and
    /// Phase 1.5 differential testing.  The bytecode VM does not use this;
    /// call `dmn_lite_engine::reference::evaluate(&compiled.typed_ir, ...)`
    /// to exercise the reference evaluator.
    pub typed_ir: TypedDecision,
}

// ── VerifiedDecision ──────────────────────────────────────────────────────────

/// A `CompiledDecision` that has passed the bytecode verifier.
///
/// The stack VM (`dmn_lite_engine::vm::evaluate`) accepts only this type,
/// not a bare `CompiledDecision`.  This prevents unverified bytecode from
/// reaching the VM.
///
/// **Construction discipline, compiler-enforced (pub-scope audit,
/// 2026-07-29):** `new_verified` is `pub(crate)` — the only code in the
/// entire workspace that can construct a `VerifiedDecision` is this
/// crate's own [`crate::verify()`]. Earlier this discipline was
/// documentary only (`new_verified` was `pub fn`, callable from
/// anywhere), with the stated reason being a circular dependency: the
/// verifier lived in `dmn-lite-compiler`, which depends on
/// `dmn-lite-types`, so `dmn-lite-types` couldn't gate construction
/// without depending back on `dmn-lite-compiler`. Fixed by moving the
/// verifier itself into `dmn-lite-types` (it only ever touched
/// `CompiledDecision`/`Instr`/`HitPolicy`, all defined here already) —
/// `dmn-lite-compiler::{verify, VerifierError}` are now a thin
/// pass-through re-export for existing call sites. See Phase 1.4 §3.11
/// for the original construction-discipline rationale.
///
/// Compiler-checked receipt for the claim above — this doctest compiles
/// as an external crate (same as any real downstream consumer, e.g.
/// `dmn-lite-compiler`), so it proves the construction path is actually
/// sealed, not just documented as sealed:
///
/// ```compile_fail
/// # let decision: dmn_lite_types::CompiledDecision = todo!();
/// // pub(crate) to dmn-lite-types — unreachable from here.
/// let _ = dmn_lite_types::VerifiedDecision::new_verified(decision);
/// ```
#[derive(Debug, Clone)]
pub struct VerifiedDecision(CompiledDecision);

impl VerifiedDecision {
    /// Wrap a compiled decision after it has been verified.
    ///
    /// `pub(crate)`: only [`crate::verify()`] may call this — see the
    /// struct's doc comment.
    pub(crate) fn new_verified(decision: CompiledDecision) -> Self {
        Self(decision)
    }

    /// Read the underlying compiled artifact.
    pub fn as_compiled(&self) -> &CompiledDecision {
        &self.0
    }
}
