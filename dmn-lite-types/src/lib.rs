//! dmn-lite core types.
//!
//! This crate owns the vocabulary shared between parser, compiler, analysis,
//! and engine. It contains no behaviour beyond simple constructors and
//! accessors. All semantic logic lives in the consuming crates.
//!
//! All submodules are private — every external consumer reaches the full
//! vocabulary flat (`dmn_lite_types::Foo`) via the prelude `pub use`s below.
//! `hit_policy` and `predicates` are empty Phase-1.0 stub files (their
//! promised types shipped elsewhere — `HitPolicy` lives in `ir`) and declare
//! no items at all.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod analysis;
mod ast;
mod catalogue;
mod compiled;
mod errors;
mod hit_policy;
mod ids;
mod instr;
mod ir;
mod predicates;
mod trace;
mod values;
mod verify;

pub use analysis::{
    AnalysisFinding, AnalysisReport, CostBound, FieldOverlap, FindingKind, GapSummary,
    OverlapSummary, Severity, UncoveredInputExample,
};
pub use ast::*;
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
pub use ir::*;
pub use trace::{EvaluationTrace, PredicateTrace, RuleTrace, TraceOutcome};
pub use values::{
    InputContextError, TypedInputContext, TypedInputContextBuilder, TypedOutputContext,
    compute_schema_hash,
};
pub use verify::{verify, VerifierError};
