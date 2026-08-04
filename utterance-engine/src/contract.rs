//! The STABLE tier-1 contract (V&S §10.3, R2-2 narrowed form) — a
//! scalar-scoring ranker is ALL it promises. Slot/span evidence enters
//! only as the output of a separately identified, separately evaluated
//! producer (Q7 ruled option (a): deterministic resolvers).

use anyhow::{anyhow, Result};
use sem_os_policy::decision_board::EvidenceLane;
use serde::{Deserialize, Serialize};

/// The explicit abstention candidate present on EVERY board (R2-r1).
/// Canonical id joins the same namespace as `designer-graph`'s
/// `op.*`/`prod.*` ids.
pub const NONE_OF_THE_ABOVE: &str = "abstain.none_of_the_above";

#[cfg(test)]
mod alignment {
    /// Cement: this constant IS designer-graph's Abstain canonical id.
    #[test]
    fn abstention_id_aligned_with_designer_graph() {
        assert_eq!(
            super::NONE_OF_THE_ABOVE,
            designer_graph::board_candidate::CandidateId::Abstain.canonical_id()
        );
    }
}

/// A finite score. NaN/Infinity are TYPED REJECTS — the model layer
/// inherits the canonical encoder's rule (R2-3). Construction is the
/// only entry; the inner value is private.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct FiniteScore(f64);

impl FiniteScore {
    pub fn new(value: f64) -> Result<Self> {
        if !value.is_finite() {
            return Err(anyhow!("FiniteScore rejects non-finite value {value}"));
        }
        Ok(FiniteScore(value))
    }
    pub fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for FiniteScore {
    type Error = String;
    fn try_from(v: f64) -> std::result::Result<Self, String> {
        FiniteScore::new(v).map_err(|e| e.to_string())
    }
}
impl From<FiniteScore> for f64 {
    fn from(s: FiniteScore) -> f64 {
        s.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RankedCandidate {
    /// The candidate's canonical id (the board-hash namespace:
    /// `op.*` / `prod.*` / `abstain.none_of_the_above`) — NEVER a Rust
    /// enum serde form (designer-graph hash-preimage contract).
    pub candidate_id: String,
    pub score: FiniteScore,
}

/// Auditable evidence metadata for the semantic serving path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceTrace {
    pub candidate_serializer_hash: String,
    pub lanes: Vec<EvidenceLane>,
    pub bundle_identities: Vec<String>,
    pub exact_collision: Vec<String>,
    pub served_full_board: bool,
}

/// V&S §10.3 verbatim shape. Hashes are hex-encoded blake3.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlmResult {
    /// Sorted best-first under `rank_canonically`.
    pub ranking: Vec<RankedCandidate>,
    /// The exact tier-0 output tier-1 saw.
    pub retrieved_subset_hash: String,
    /// The exact inference board (§11.7).
    pub board_hash: String,
    /// The sealed model bundle (§10.8). Tier-0-only results use the
    /// tier-0 identity string until a tier-1 bundle exists.
    pub model_bundle_hash: String,
    /// Present on the semantic path; absent on compatibility evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_trace: Option<EvidenceTrace>,
}

/// THE canonical ordering (R2-3): descending score; equal scores break
/// on ascending canonical candidate id. Total, deterministic, and the
/// only ordering any ranking in this crate may use.
pub fn rank_canonically(candidates: &mut [RankedCandidate]) {
    candidates.sort_by(|a, b| {
        b.score
            .get()
            .partial_cmp(&a.score.get())
            .expect("FiniteScore construction bans non-finite values")
            .then_with(|| a.candidate_id.cmp(&b.candidate_id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RED: non-finite scores are typed rejects at construction AND at
    /// serde decode; GREEN: finite admits.
    #[test]
    fn finite_score_rejects_non_finite() {
        assert!(FiniteScore::new(f64::NAN).is_err());
        assert!(FiniteScore::new(f64::INFINITY).is_err());
        assert!(FiniteScore::new(f64::NEG_INFINITY).is_err());
        assert!(FiniteScore::new(0.73).is_ok());
        // serde path inherits the rule via try_from
        let bad: std::result::Result<FiniteScore, _> = serde_json::from_str("null");
        assert!(bad.is_err());
    }

    /// Canonical tie-break: equal scores order by candidate id,
    /// deterministically, regardless of input order.
    #[test]
    fn ranking_tie_breaks_on_canonical_id() {
        let mk = |id: &str, s: f64| RankedCandidate {
            candidate_id: id.into(),
            score: FiniteScore::new(s).unwrap(),
        };
        let mut a = vec![
            mk("prod.request_and_wait", 0.5),
            mk("op.append_node", 0.5),
            mk(NONE_OF_THE_ABOVE, 0.9),
        ];
        let mut b = a.clone();
        b.reverse();
        rank_canonically(&mut a);
        rank_canonically(&mut b);
        let ids: Vec<&str> = a.iter().map(|c| c.candidate_id.as_str()).collect();
        assert_eq!(
            ids,
            vec![NONE_OF_THE_ABOVE, "op.append_node", "prod.request_and_wait"]
        );
        assert_eq!(
            ids,
            b.iter().map(|c| c.candidate_id.as_str()).collect::<Vec<_>>(),
            "ordering must be input-order invariant"
        );
    }
}
