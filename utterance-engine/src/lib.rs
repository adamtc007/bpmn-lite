//! WS-C `utterance-engine` (EOP-PLAN-BPMN-DESIGN-003 v0.2).
//!
//! Binding invariants (EOP-VS-BPMN-DESIGN-003 v0.6):
//! - I15: closed, content-addressed candidate board; model output
//!   outside the board is rejected.
//! - I26: tier-1 always receives the raw utterance; a Sage hypothesis
//!   is additional features, never a substitute.
//! - I27: models supply evidence; the deterministic disposition policy
//!   issues every `ProposalDisposition`.
//! - I28: every decision record closes over board / retrieved-subset /
//!   model-bundle / disposition-policy / context-projection hashes;
//!   scores are finite; ties break canonically; recorded values are the
//!   historical truth.
//! - D18: promotion ceiling shadow → suggest-only → staged-patch.
//! - Plan §E rulings: E2 (board universe via registry provider trait),
//!   E3 (in-process embed+score tier-0), E5 (thresholds in a versioned
//!   config hashed into `disposition_policy_hash` — never inline).
//!
//! **Module-boundary note (pub-scope audit, 2026-07-29):** `board`,
//! `context`, `corpus_schema`, `dev_capture`, `policy`, and `retrieval`
//! are deliberate, addressed-by-name API surface — same judgment as
//! `designer-graph`'s split (each maps to a distinct conceptual
//! sub-domain; the sole external consumer, `bpmn-lite-server-designer`,
//! already addresses them by module-qualified path). `contract`,
//! `fixtures`, and `trained_ranker` stay `pub mod` because this crate's own
//! `examples/*.rs` binaries AND `fuzz/` targets need genuine `pub`
//! visibility to reach them (an example or fuzz target compiles as an
//! external consumer of the library, same as any other crate — H5,
//! EOP-PLAN-CRATE-HYGIENE-001, re-verified this with bare-identifier greps
//! against the whole workspace). `metrics` was already `#[cfg(test)] mod
//! metrics` (private) as of that plan's H2 — dropped from this list, no
//! longer part of the question. Do not flatten into a crate-root facade
//! without re-litigating that call.

mod argument_evidence;
mod belief;
pub mod board;
pub mod bpmn_board;
mod bpmn_pack;
mod clarification;
pub mod context;
mod fusion;
mod game_state;
mod graph_features;
mod history;
mod legal_moves;
mod motifs;
mod resolver_comparison;
mod structured_choice;
// Q9-GATED user capture: compiled ONLY under `q9-capture` (off by
// default, absent from every release build in this repo -- DIR-004
// Phase 1.2, `scripts/check-q9-capture-gate.sh` enforces it). A
// pre-charter default build has NO live user-capture path to even find,
// let alone accidentally enable. See `capture.rs`'s module doc.
#[cfg(feature = "q9-capture")]
pub mod capture;
// WS-1.3: the decomposed quality funnel over captured+adjudicated turns
// — meaningless without the capture module, so it shares the gate.
pub mod contract;
pub mod corpus_schema;
#[cfg(feature = "q9-capture")]
pub mod funnel;
// Dev-session capture: Adam's own testing only, always compiled,
// structurally distinct from the Q9-gated path above (DIR-004 Phase 1,
// Option B ruling). See `dev_capture.rs`'s module doc.
pub mod dev_capture;
pub mod disposition;
pub mod exact;
pub mod fixtures;
// Gate-time evaluation harness (WS-C C-now item 5; V&S §10.7): measures
// recall/precision decomposition over a labeled case set for a human to
// judge at gate time ("this module measures, it never judges" -- its own
// doc comment). No production code path calls it and nothing outside this
// crate references it -- it's exercised entirely from its own #[cfg(test)]
// submodules, so it's test-only scaffolding, not a runtime dependency.
#[cfg(test)]
mod metrics;
pub mod pair;
pub mod policy;
#[cfg(test)]
mod property_tests;
pub mod retrieval;
pub use history::{MAX_HISTORY_ATTEMPTS, MAX_HISTORY_BYTES};
pub use legal_moves::MAX_ENUMERATION_CANDIDATES;
pub use resolver_comparison::{
    compare_resolvers, BoundedOfflineCandidateText, BoundedOfflineRankerRequest,
    ProportionInterval, ResolverBoardPacket, ResolverBoundaryRefusal, ResolverCandidateScore,
    ResolverComparisonCase, ResolverComparisonReport, ResolverMetrics, ResolverVariant,
    MAX_OFFLINE_CANDIDATE_BYTES, MAX_OFFLINE_PROMPT_BYTES, MAX_OFFLINE_TOKEN_BUDGET,
    MAX_RESOLVER_BOARD_SIZE,
};
pub use structured_choice::{
    BoardSizeBucket, StructuredChoiceCalibration, StructuredChoiceCalibrationCell,
    StructuredChoiceCalibrationObservation, StructuredChoiceFitConfig, StructuredChoiceModel,
    StructuredChoiceObservation, StructuredChoiceScore,
};
#[cfg(feature = "candle-probe")]
pub mod trained_ranker;
