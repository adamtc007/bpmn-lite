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
pub mod capture;
pub mod contract;
pub mod policy;
pub mod retrieval;
