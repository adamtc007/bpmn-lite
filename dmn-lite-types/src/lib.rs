//! dmn-lite core types.
//!
//! This crate owns the vocabulary shared between parser, compiler, analysis,
//! and engine. It contains no behaviour beyond simple constructors and
//! accessors. All semantic logic lives in the consuming crates.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod analysis;
pub mod ast;
pub mod catalogue;
pub mod compiled;
pub mod errors;
pub mod hit_policy;
pub mod ids;
pub mod instr;
pub mod ir;
pub mod predicates;
pub mod trace;
pub mod values;

pub use analysis::{
    AnalysisFinding, AnalysisReport, CostBound, FieldOverlap, FindingKind, GapSummary,
    OverlapSummary, Severity, UncoveredInputExample,
};
pub use catalogue::{Catalogue, Domain, DomainValue};
pub use compiled::{
    ArtifactHash, CompileContext, CompiledDecision, RangeEntry, RuleMapEntry, VerifiedDecision,
};
pub use errors::{CatalogueError, CompileError, CompileWarning, EvalError, ParseError};
pub use ids::{
    AggregateOpKind, BindingId, BkmId, ConstId, ConstSetId, DecisionId, DomainId, FieldId,
    NumberKind, OutputFieldId, PathId, RangeId, RuleId, SchemaHash, SnapshotId, SourceSpan,
    ValueId,
};
pub use instr::Instr;
pub use trace::{EvaluationTrace, PredicateTrace, RuleTrace, TraceOutcome};
pub use values::{
    InputContextError, TypedInputContext, TypedInputContextBuilder, TypedOutputContext,
    compute_schema_hash,
};
