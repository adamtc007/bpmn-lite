//! Identifier and source-location types shared across parser, compiler, and engine.

use std::fmt;

use uuid::Uuid;

/// Byte-offset span into the original source text.
///
/// Every AST and IR node carries a span so that diagnostics can point at the
/// exact bytes that caused an error. Byte offsets (not char or line/column
/// indices) are used because they are encoding-deterministic and required for
/// V&S §11.5 source-normalisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    /// Inclusive start byte offset.
    pub start: u32,
    /// Exclusive end byte offset.
    pub end: u32,
}

impl SourceSpan {
    /// Construct a span from start (inclusive) and end (exclusive) byte offsets.
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Length in bytes.
    pub fn len(self) -> u32 {
        self.end - self.start
    }

    /// True when the span covers zero bytes.
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Merge two spans into the smallest span that covers both.
    ///
    /// Used by parent AST nodes to derive their span from their children.
    pub fn merge(self, other: SourceSpan) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

impl fmt::Display for SourceSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// Whether a numeric literal was written as an integer or decimal.
///
/// Stored on `NumberLitAst` at parse time so Phase 1.2 type-checking can
/// reject (for example) a decimal literal assigned to an `integer`-typed
/// output without re-scanning the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberKind {
    /// No decimal point: `42`, `-7`.
    Integer,
    /// Has a decimal point: `3.14`, `-0.5`.
    Decimal,
}

// ── Catalogue identity types ──────────────────────────────────────────────────

/// Unique identifier for a Sem OS catalogue snapshot.
///
/// Stored as a UUIDv7 (RFC 9562). The timestamp prefix provides
/// chronological ordering; the random suffix ensures global uniqueness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapshotId(pub Uuid);

/// Unique identifier for an enum domain within the Sem OS catalogue.
///
/// Stored as a UUIDv7. Stable across catalogue snapshot versions when the
/// domain itself is unchanged; a new `DomainId` is issued when the domain
/// is superseded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DomainId(pub Uuid);

/// Unique identifier for a single enum value within a domain.
///
/// Stored as a UUIDv7. Together with its parent `DomainId`, a `ValueId`
/// forms the early-bound canonical reference to an enum literal in compiled
/// decisions and audit logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(pub Uuid);

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for DomainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for ValueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── IR identity types ─────────────────────────────────────────────────────────

/// Ordinal index of a field in a decision's input or output schema.
///
/// `FieldId(0)` is the first declared input/output; indices are assigned
/// in source order and are stable for the lifetime of a compiled decision.
/// The evaluator uses `FieldId` as the key into `TypedInputContext` and
/// `TypedOutputContext`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldId(pub usize);

/// Ordinal index of a rule in a decision's rule list.
///
/// `RuleId(0)` is the first rule in source order. Used by the evaluator to
/// record which rule matched and by the hit-policy accumulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RuleId(pub usize);

/// Identifier for a compiled decision.
///
/// Derived from the `:decision-id` string literal if present, or from the
/// decision name symbol otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DecisionId(pub String);

impl fmt::Display for FieldId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "field#{}", self.0)
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rule#{}", self.0)
    }
}

impl fmt::Display for DecisionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A hash of a decision's input or output schema, used to detect mismatches
/// between a `TypedInputContext` and the decision it is evaluated against.
///
/// Computed from the schema's field count, names, types, and domain IDs using
/// Rust's `DefaultHasher`. Stable within a single process run; not guaranteed
/// stable across Rust versions or process restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SchemaHash(pub u64);

// ── Bytecode pool index types ─────────────────────────────────────────────────

/// Index into `CompiledDecision::const_pool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConstId(pub u32);

/// Index into `CompiledDecision::const_set_pool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConstSetId(pub u32);

/// Index into `CompiledDecision::range_pool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RangeId(pub u32);

/// Ordinal index of an output field in the output schema.
///
/// Used by `StoreOutput` / `StoreOutputTos` to identify which output slot
/// receives the stored value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputFieldId(pub u32);

// ── Reserved future-profile IDs ───────────────────────────────────────────────
// These exist so the Instr enum can declare v0.2+ variants now
// (reserved, emitter-never-produces, verifier-rejects).

/// Identifier for a governed function / BKM (Profile v0.5+).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BkmId(pub u32);

/// Identifier for a quantifier bound variable (Profile v0.2+).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindingId(pub u32);

/// Identifier for a path expression (Profile v0.4+).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathId(pub u32);

/// Aggregation operation kind for `AggregateBegin` (Profile v0.2+).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateOpKind {
    /// Count of matching elements.
    Count,
    /// Sum of numeric values.
    Sum,
    /// Minimum numeric value.
    Min,
    /// Maximum numeric value.
    Max,
    /// Arithmetic mean of numeric values.
    Mean,
}
