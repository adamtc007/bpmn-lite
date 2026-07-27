//! Tier-0 retrieval interface + the interim lexical producer (WS-C
//! C-now item 4, first half).
//!
//! `Tier0Retriever` is THE seam every retrieval producer sits behind
//! (§10.6: tier-0 is high-recall retrieval; tier-1 ranks the retrieved
//! subset). Producers registered here supply EVIDENCE ONLY (I27) — the
//! output is an `SlmResult` fed to `policy::decide`, never a
//! disposition. The Candle embed-and-score producer (ruling E3, matcher
//! pinned at ob-poc-rust `ff3f12c`, `default-features = false`) plugs
//! in behind this same trait; per the programme ruling, the retired
//! keyword gate's successor is THIS lexical producer, retired in turn
//! when the embedder lands.

use crate::board::Board;
use crate::contract::{rank_canonically, FiniteScore, RankedCandidate, SlmResult};
use anyhow::Result;

/// A tier-0 retrieval producer: raw utterance + exact board in,
/// scored ranking out. Implementations MUST be deterministic for a
/// given (utterance, board) pair and MUST score only board candidates.
pub trait Tier0Retriever {
    /// Sealed identity of this producer (stands in for the model
    /// bundle hash until a tier-1 bundle exists — §10.8).
    fn bundle_identity(&self) -> String;
    fn retrieve(&self, raw_utterance: &str, board: &Board) -> Result<SlmResult>;
}

/// Deterministic lexical overlap scorer — the interim tier-0.
///
/// Scoring: lowercase token overlap between the utterance and each
/// candidate's description + canonical-id words; exact phrase match of
/// a candidate's description pins 1.0 (the pgvector matcher's
/// exact-match pin, reimplemented designer-side per the C5 trace note).
/// NONE_OF_THE_ABOVE scores the complement of the best overlap so
/// abstention is a live hypothesis, not a constant.
pub struct LexicalTier0;

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
        .map(str::to_owned)
        .collect()
}

impl Tier0Retriever for LexicalTier0 {
    fn bundle_identity(&self) -> String {
        "tier0.lexical.v1".to_owned()
    }

    fn retrieve(&self, raw_utterance: &str, board: &Board) -> Result<SlmResult> {
        let utter_tokens = tokens(raw_utterance);
        let utter_lower = raw_utterance.trim().to_lowercase();
        let mut best_overlap = 0.0f64;
        let mut ranking: Vec<RankedCandidate> = Vec::with_capacity(board.candidates.len());

        for c in &board.candidates {
            if c.canonical_id == crate::contract::NONE_OF_THE_ABOVE {
                continue; // scored after the loop from best_overlap
            }
            let mut cand_tokens = tokens(&c.description);
            cand_tokens.extend(tokens(&c.canonical_id.replace(['.', '_'], " ")));
            let hits = utter_tokens
                .iter()
                .filter(|t| cand_tokens.contains(t))
                .count();
            let denom = utter_tokens.len().max(1);
            let mut score = hits as f64 / denom as f64;
            if !utter_lower.is_empty() && utter_lower == c.description.trim().to_lowercase() {
                score = 1.0; // exact-match pin
            }
            best_overlap = best_overlap.max(score);
            ranking.push(RankedCandidate {
                candidate_id: c.canonical_id.clone(),
                score: FiniteScore::new(score)?,
            });
        }
        // Abstention hypothesis: strong when nothing overlaps.
        ranking.push(RankedCandidate {
            candidate_id: crate::contract::NONE_OF_THE_ABOVE.to_owned(),
            score: FiniteScore::new((1.0 - best_overlap).clamp(0.0, 0.99))?,
        });
        rank_canonically(&mut ranking);

        // retrieved_subset_hash: the exact ordered id list tier-1 would see.
        let mut pre = Vec::new();
        pre.extend_from_slice(b"subset.v1:");
        for rc in &ranking {
            pre.extend_from_slice(rc.candidate_id.len().to_string().as_bytes());
            pre.push(b':');
            pre.extend_from_slice(rc.candidate_id.as_bytes());
        }
        Ok(SlmResult {
            ranking,
            retrieved_subset_hash: blake3::hash(&pre).to_hex().to_string(),
            board_hash: board.board_hash.clone(),
            model_bundle_hash: self.bundle_identity(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{build_board, EmptyUniverse, PolicyFilter};
    use crate::policy::{decide, DispositionConfig, ProposalDisposition};
    use designer_graph::board_candidate::{LegalityOracle, OperationKind, ProductionId};

    struct AllLegal;
    impl LegalityOracle for AllLegal {
        type NodeKey = ();
        fn legal_operations(&self, _: Option<&()>) -> Vec<OperationKind> {
            OperationKind::ALL.to_vec()
        }
        fn legal_productions(&self, _: Option<&()>) -> Vec<ProductionId> {
            ProductionId::ALL.to_vec()
        }
    }

    /// THE pipeline-in-loop receipt (G2's re-scoped item, first light):
    /// board → tier-0 → deterministic policy → I28 record, end to end,
    /// no step skipped. A gibberish utterance abstains (OutOfScope);
    /// determinism across runs.
    #[test]
    fn full_pipeline_board_tier0_policy_record() {
        let board =
            build_board(&AllLegal, None, Some("rev0"), &EmptyUniverse, &PolicyFilter::default())
                .unwrap();
        let t0 = LexicalTier0;
        let cfg = DispositionConfig::shadow_v1();

        let utter = "zzz qqq xxyzzy nothing matches this";
        let ev1 = t0.retrieve(utter, &board).unwrap();
        let ev2 = t0.retrieve(utter, &board).unwrap();
        assert_eq!(ev1.retrieved_subset_hash, ev2.retrieved_subset_hash, "determinism");

        let (d, rec) = decide(&cfg, &board, &ev1, utter).unwrap();
        assert_eq!(d, ProposalDisposition::OutOfScope, "gibberish must abstain: {d:?}");
        assert_eq!(rec.board_hash, board.board_hash);
        assert_eq!(rec.model_bundle_hash, "tier0.lexical.v1");
        assert!(!rec.disposition_policy_hash.is_empty());
        assert_eq!(rec.ranking.len(), board.candidates.len());
    }

    /// Evidence stays on the board (I15 upheld by construction) and the
    /// exact-match pin scores 1.0.
    #[test]
    fn lexical_scores_are_boarded_and_exact_match_pins() {
        let board =
            build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default()).unwrap();
        let t0 = LexicalTier0;
        let exact = "Connect two existing nodes with a typed sequence flow";
        let ev = t0.retrieve(exact, &board).unwrap();
        for rc in &ev.ranking {
            assert!(board.contains(&rc.candidate_id), "off-board evidence: {}", rc.candidate_id);
        }
        let top = &ev.ranking[0];
        assert_eq!(top.candidate_id, "op.connect");
        assert_eq!(top.score.get(), 1.0, "exact description match pins 1.0");
    }
}
