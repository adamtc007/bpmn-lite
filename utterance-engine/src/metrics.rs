//! Metrics harness (WS-C C-now item 5; V&S §10.7).
//!
//! The pipeline is measured PER TIER, because tier-1 can only rank what
//! tier-0 admits (R1-5). This harness computes the §10.7 decomposition
//! over a labeled case set and provides the position-invariance check
//! as a reusable assertion for ANY producer. G3's thresholds are Adam's
//! at gate time — this module measures, it never judges.

use anyhow::Result;

use crate::board::Board;
use crate::policy::{decide, DispositionConfig, ProposalDisposition};
use crate::retrieval::Tier0Retriever;

/// One labeled case: the utterance and the oracle-correct candidate
/// (canonical id), or `None` when the board should not contain an
/// answer (the abstention cases §10.7 requires).
#[derive(Clone, Debug)]
pub(crate) struct LabeledCase {
    pub utterance: String,
    pub oracle: Option<String>,
}

/// The §10.7 decomposition over one evaluation run. Counts, not
/// verdicts — rates derived on read so zero-denominator cases stay
/// visible instead of collapsing to fake 100%s.
#[derive(Clone, Debug, Default)]
pub(crate) struct MetricsReport {
    pub cases: usize,
    /// Oracle candidate present on the board at all (board completeness).
    pub oracle_on_board: usize,
    /// Oracle within the top-K of tier-0's ranking (recall@K),
    /// counted over oracle-on-board cases only.
    pub oracle_in_top_k: usize,
    /// Top-1 == oracle, counted over oracle-in-top-K cases only
    /// (ranking-given-inclusion).
    pub top1_correct: usize,
    /// decide() produced Candidate(oracle) (end-to-end).
    pub end_to_end_correct: usize,
    /// Oracle-absent cases where the pipeline abstained (OutOfScope)
    /// or escalated — the model must not confidently rank a board
    /// that doesn't contain the answer.
    pub absent_cases: usize,
    pub absent_handled: usize,
}

impl MetricsReport {
    fn board_completeness(&self) -> Option<f64> {
        let denom = self.cases - self.absent_cases;
        (denom > 0).then(|| self.oracle_on_board as f64 / denom as f64)
    }
    pub(crate) fn recall_at_k(&self) -> Option<f64> {
        (self.oracle_on_board > 0).then(|| self.oracle_in_top_k as f64 / self.oracle_on_board as f64)
    }
    fn ranking_given_inclusion(&self) -> Option<f64> {
        (self.oracle_in_top_k > 0).then(|| self.top1_correct as f64 / self.oracle_in_top_k as f64)
    }
    pub(crate) fn end_to_end(&self) -> Option<f64> {
        let denom = self.cases - self.absent_cases;
        (denom > 0).then(|| self.end_to_end_correct as f64 / denom as f64)
    }
    fn abstention_coverage(&self) -> Option<f64> {
        (self.absent_cases > 0).then(|| self.absent_handled as f64 / self.absent_cases as f64)
    }
}

/// Run the decomposition: one fixed board, one producer, one config.
/// (Per-position boards ride the same shape later — the harness takes
/// the board the session would have built.)
pub(crate) fn evaluate<T: Tier0Retriever>(
    retriever: &T,
    board: &Board,
    config: &DispositionConfig,
    cases: &[LabeledCase],
    k: usize,
    context: &crate::context::ContextProjection,
) -> Result<MetricsReport> {
    let mut report = MetricsReport {
        cases: cases.len(),
        ..Default::default()
    };
    for case in cases {
        let evidence = retriever.retrieve(&case.utterance, board)?;
        let (disposition, _record) = decide(config, board, &evidence, context)?;
        match &case.oracle {
            None => {
                report.absent_cases += 1;
                if matches!(
                    disposition,
                    ProposalDisposition::OutOfScope | ProposalDisposition::EscalateToSage { .. }
                ) {
                    report.absent_handled += 1;
                }
            }
            Some(oracle) => {
                if board.contains(oracle) {
                    report.oracle_on_board += 1;
                    let in_top_k = evidence
                        .ranking
                        .iter()
                        .take(k)
                        .any(|rc| &rc.candidate_id == oracle);
                    if in_top_k {
                        report.oracle_in_top_k += 1;
                        if evidence.ranking.first().map(|rc| &rc.candidate_id) == Some(oracle) {
                            report.top1_correct += 1;
                        }
                    }
                }
                if disposition
                    == (ProposalDisposition::Candidate {
                        candidate_id: oracle.clone(),
                    })
                {
                    report.end_to_end_correct += 1;
                }
            }
        }
    }
    Ok(report)
}

/// §10.7 position-invariance check, reusable against ANY producer: the
/// ranking must not change when the board's candidate order is
/// permuted (same content, same hash). Order-sensitivity is a
/// batching/model defect, not something canonical ordering "prevents"
/// (R2-r2). Returns Err naming the first divergence.
fn assert_position_invariant<T: Tier0Retriever>(
    retriever: &T,
    utterance: &str,
    board: &Board,
) -> Result<()> {
    let baseline = retriever.retrieve(utterance, board)?;
    let mut permuted = board.clone();
    permuted.candidates.reverse();
    let alt = retriever.retrieve(utterance, &permuted)?;
    for (a, b) in baseline.ranking.iter().zip(alt.ranking.iter()) {
        if a.candidate_id != b.candidate_id || a.score.get() != b.score.get() {
            return Err(anyhow::anyhow!(
                "position sensitivity: '{}' scored {} baseline vs '{}' {} permuted",
                a.candidate_id,
                a.score.get(),
                b.candidate_id,
                b.score.get()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{build_board, EmptyUniverse, PolicyFilter};
    use crate::retrieval::LexicalTier0;
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

    /// Harness self-test over the lexical producer: exact-match case
    /// lands end-to-end; gibberish abstention case is covered; the
    /// decomposition's denominators behave.
    #[test]
    fn decomposition_over_lexical_baseline() {
        let board =
            build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default()).unwrap();
        let cases = vec![
            LabeledCase {
                utterance: "Connect two existing nodes with a typed sequence flow".into(),
                oracle: Some("op.connect".into()),
            },
            LabeledCase {
                utterance: "zzz qqq xyzzy".into(),
                oracle: None,
            },
        ];
        let report = evaluate(
            &LexicalTier0,
            &board,
            &DispositionConfig::shadow_v1(),
            &cases, 5, &crate::context::minimal("pack.none", "g-test"))
        .unwrap();
        assert_eq!(report.cases, 2);
        assert_eq!(report.board_completeness(), Some(1.0));
        assert_eq!(report.recall_at_k(), Some(1.0));
        assert_eq!(report.ranking_given_inclusion(), Some(1.0));
        assert_eq!(report.end_to_end(), Some(1.0));
        assert_eq!(report.abstention_coverage(), Some(1.0));
    }

    /// Zero-denominator honesty: an all-absent case set reports None
    /// for the oracle-based rates instead of fake 100%s.
    #[test]
    fn zero_denominators_stay_visible() {
        let board =
            build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default()).unwrap();
        let cases = vec![LabeledCase {
            utterance: "xyzzy".into(),
            oracle: None,
        }];
        let report = evaluate(
            &LexicalTier0,
            &board,
            &DispositionConfig::shadow_v1(),
            &cases, 5, &crate::context::minimal("pack.none", "g-test"))
        .unwrap();
        assert_eq!(report.board_completeness(), None);
        assert_eq!(report.recall_at_k(), None);
        assert_eq!(report.end_to_end(), None);
        assert_eq!(report.abstention_coverage(), Some(1.0));
    }

    /// The lexical producer is position-invariant (canonical re-sort
    /// makes it so by construction — the check exists for producers
    /// where it is NOT structural, e.g. batched tier-1 inference).
    #[test]
    fn lexical_producer_is_position_invariant() {
        let board =
            build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default()).unwrap();
        assert_position_invariant(&LexicalTier0, "connect the nodes", &board).unwrap();
    }
}

#[cfg(test)]
mod seed_corpus_baseline {
    use super::*;
    use crate::board::{build_board, EmptyUniverse, PolicyFilter};
    use crate::policy::DispositionConfig;
    use crate::retrieval::LexicalTier0;
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

    /// C-seed.2: the tier-0 baseline over the synthetic seed corpus
    /// (charter-independent by construction — see the corpus file's
    /// provenance field). Rates are RECORDED via stderr for the plan
    /// receipts; only sanity floors are cemented so corpus growth
    /// doesn't churn this test. G3 thresholds are Adam's, on REAL data.
    #[test]
    fn seed_corpus_v1_lexical_baseline() {
        let raw = include_str!("../seed/seed_corpus_v1.json");
        let parsed: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed["version"], "synthetic.seed.v1");
        assert!(parsed["provenance"].as_str().unwrap().contains("synthetic.llm"));
        let cases: Vec<LabeledCase> = parsed["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| LabeledCase {
                utterance: c["utterance"].as_str().unwrap().to_owned(),
                oracle: c["oracle"].as_str().map(str::to_owned),
            })
            .collect();
        assert!(cases.len() >= 90, "corpus shrank unexpectedly");

        let board =
            build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default()).unwrap();
        let report = evaluate(
            &LexicalTier0,
            &board,
            &DispositionConfig::shadow_v1(),
            &cases, 5, &crate::context::minimal("pack.none", "g-test"))
        .unwrap();

        eprintln!(
            "SEED-BASELINE tier0.lexical.v1 over {}: completeness={:?} recall@5={:?} \
             ranking|inclusion={:?} end_to_end={:?} abstention={:?}",
            parsed["version"],
            report.board_completeness(),
            report.recall_at_k(),
            report.ranking_given_inclusion(),
            report.end_to_end(),
            report.abstention_coverage()
        );

        // Sanity floors only (recorded rates live in the plan receipts):
        assert_eq!(report.board_completeness(), Some(1.0), "every oracle is on the board");
        assert!(report.recall_at_k().unwrap() > 0.5, "lexical recall@5 collapsed");
        assert!(report.abstention_coverage().unwrap() > 0.8, "off-board handling collapsed");
    }
}
