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

pub mod board;
pub mod context;
// Q9-GATED user capture: compiled ONLY under `q9-capture` (off by
// default, absent from every release build in this repo -- DIR-004
// Phase 1.2, `scripts/check-q9-capture-gate.sh` enforces it). A
// pre-charter default build has NO live user-capture path to even find,
// let alone accidentally enable. See `capture.rs`'s module doc.
#[cfg(feature = "q9-capture")]
pub mod capture;
pub mod contract;
pub mod corpus_schema;
// Dev-session capture: Adam's own testing only, always compiled,
// structurally distinct from the Q9-gated path above (DIR-004 Phase 1,
// Option B ruling). See `dev_capture.rs`'s module doc.
pub mod dev_capture;
pub mod fixtures;
pub mod metrics;
pub mod policy;
pub mod retrieval;
#[cfg(feature = "candle-probe")]
pub mod trained_ranker;
