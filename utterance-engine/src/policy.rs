//! Deterministic disposition policy + I28 decision record (WS-C C-now
//! item 3; V&S §10.3/§9.2, I27/I28).
//!
//! THE rule (I27): models supply evidence; THIS function issues every
//! disposition. No model — tier-0, tier-1, or Sage — ever returns a
//! `ProposalDisposition`. WS-B's UI calls `decide` from its first
//! commit (plan day-one rule); tier-1 insertion later registers one
//! more evidence producer and adds record fields — nothing here changes.
//!
//! Thresholds (plan §E5): a VERSIONED config hashed into
//! `disposition_policy_hash` — never inline literals. Initial values
//! are PLACEHOLDERs, low-stakes in shadow, recalibrated at G3 where
//! the threshold values are Adam's to set.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::contract::{SlmResult, NONE_OF_THE_ABOVE};

/// E5: the versioned threshold config. Serialized (stable field order —
/// serde struct order) and blake3-hashed into `disposition_policy_hash`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DispositionConfig {
    /// Bump on ANY semantic change to `decide`, not just field edits.
    pub policy_version: u32,
    /// PLACEHOLDER (G3 recalibrates): minimum top-1 score to consider
    /// selection at all; below → Escalate (weak evidence).
    pub acceptance_floor: f64,
    /// PLACEHOLDER (G3 recalibrates): minimum top-1 − top-2 gap for an
    /// unambiguous selection; below → Ambiguous.
    pub separation_margin: f64,
}

impl DispositionConfig {
    /// The shadow-phase placeholder config (E5).
    pub fn shadow_v1() -> Self {
        DispositionConfig {
            policy_version: 1,
            acceptance_floor: 0.50,
            separation_margin: 0.15,
        }
    }

    pub fn policy_hash(&self) -> String {
        let json = serde_json::to_string(self).expect("config serializes");
        blake3::hash(json.as_bytes()).to_hex().to_string()
    }
}

/// V&S §9.2 shape, designer-surface v1. Issued ONLY by `decide`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProposalDisposition {
    /// Sufficiently separated top candidate → deterministic binding
    /// proceeds (Repl still re-establishes everything).
    Candidate { candidate_id: String },
    /// Top scores insufficiently separated → clarification, rendered by
    /// Sage (D7: Sage renders, policy decides).
    Ambiguous { top_candidates: Vec<String> },
    /// The abstention hypothesis won: the board does not contain the
    /// answer. D19-rider denial rendering applies downstream.
    OutOfScope,
    /// Weak/novel/compound-suspected → Sage analysis against the SAME
    /// board (D20 governs any board change).
    EscalateToSage { reason: String },
}

/// I28: the decision record closes over everything the disposition
/// depended on. Recorded values are the historical truth; re-inference
/// is forensic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub board_hash: String,
    pub retrieved_subset_hash: String,
    pub model_bundle_hash: String,
    pub disposition_policy_hash: String,
    /// Session/context features as projected (v1: the raw utterance
    /// hash — widened when the projection grows features).
    pub context_projection_hash: String,
    /// The ranking verbatim, as evidence entered policy.
    pub ranking: Vec<(String, f64)>,
    pub disposition: ProposalDisposition,
}

/// THE disposition function. Fail-closed: a ranking naming any
/// candidate not on the board is an ERROR (I15 — model output outside
/// the board is rejected), never a silent skip.
pub fn decide(
    config: &DispositionConfig,
    board: &Board,
    result: &SlmResult,
    raw_utterance: &str,
) -> Result<(ProposalDisposition, DecisionRecord)> {
    if result.board_hash != board.board_hash {
        return Err(anyhow!(
            "SlmResult board_hash {} does not match the presented board {} — evidence \
             from a different board is inadmissible (I28)",
            result.board_hash,
            board.board_hash
        ));
    }
    for rc in &result.ranking {
        if !board.contains(&rc.candidate_id) {
            return Err(anyhow!(
                "ranking names '{}' which is not on board {} — off-board model output \
                 is rejected (I15)",
                rc.candidate_id,
                board.board_hash
            ));
        }
    }

    let disposition = match result.ranking.as_slice() {
        [] => ProposalDisposition::EscalateToSage {
            reason: "empty ranking — no evidence to select on".to_owned(),
        },
        [top, rest @ ..] => {
            if top.candidate_id == NONE_OF_THE_ABOVE {
                ProposalDisposition::OutOfScope
            } else if top.score.get() < config.acceptance_floor {
                ProposalDisposition::EscalateToSage {
                    reason: format!(
                        "top score {:.3} below acceptance floor {:.3}",
                        top.score.get(),
                        config.acceptance_floor
                    ),
                }
            } else if let Some(second) = rest.first() {
                if top.score.get() - second.score.get() < config.separation_margin {
                    ProposalDisposition::Ambiguous {
                        top_candidates: vec![
                            top.candidate_id.clone(),
                            second.candidate_id.clone(),
                        ],
                    }
                } else {
                    ProposalDisposition::Candidate {
                        candidate_id: top.candidate_id.clone(),
                    }
                }
            } else {
                ProposalDisposition::Candidate {
                    candidate_id: top.candidate_id.clone(),
                }
            }
        }
    };

    let record = DecisionRecord {
        board_hash: board.board_hash.clone(),
        retrieved_subset_hash: result.retrieved_subset_hash.clone(),
        model_bundle_hash: result.model_bundle_hash.clone(),
        disposition_policy_hash: config.policy_hash(),
        context_projection_hash: blake3::hash(raw_utterance.as_bytes()).to_hex().to_string(),
        ranking: result
            .ranking
            .iter()
            .map(|rc| (rc.candidate_id.clone(), rc.score.get()))
            .collect(),
        disposition: disposition.clone(),
    };
    Ok((disposition, record))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{build_board, EmptyUniverse, PolicyFilter};
    use crate::contract::{FiniteScore, RankedCandidate};
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

    fn board() -> Board {
        build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default())
    }

    fn result(board: &Board, ranking: Vec<(&str, f64)>) -> SlmResult {
        SlmResult {
            ranking: ranking
                .into_iter()
                .map(|(id, s)| RankedCandidate {
                    candidate_id: id.to_owned(),
                    score: FiniteScore::new(s).unwrap(),
                })
                .collect(),
            retrieved_subset_hash: "subset0".into(),
            board_hash: board.board_hash.clone(),
            model_bundle_hash: "tier0.v1".into(),
        }
    }

    #[test]
    fn clear_winner_selects_and_record_closes_over_all_hashes() {
        let b = board();
        let cfg = DispositionConfig::shadow_v1();
        let r = result(&b, vec![("op.append_node", 0.90), ("op.connect", 0.40)]);
        let (d, rec) = decide(&cfg, &b, &r, "add a task").unwrap();
        assert_eq!(
            d,
            ProposalDisposition::Candidate { candidate_id: "op.append_node".into() }
        );
        assert_eq!(rec.board_hash, b.board_hash);
        assert_eq!(rec.disposition_policy_hash, cfg.policy_hash());
        assert!(!rec.retrieved_subset_hash.is_empty());
        assert!(!rec.model_bundle_hash.is_empty());
        assert!(!rec.context_projection_hash.is_empty());
        assert_eq!(rec.ranking.len(), 2);
    }

    #[test]
    fn close_scores_are_ambiguous_and_weak_escalates() {
        let b = board();
        let cfg = DispositionConfig::shadow_v1();
        let r = result(&b, vec![("op.append_node", 0.80), ("op.insert_after", 0.75)]);
        let (d, _) = decide(&cfg, &b, &r, "x").unwrap();
        assert!(matches!(d, ProposalDisposition::Ambiguous { .. }));

        let r = result(&b, vec![("op.append_node", 0.30)]);
        let (d, _) = decide(&cfg, &b, &r, "x").unwrap();
        assert!(matches!(d, ProposalDisposition::EscalateToSage { .. }));
    }

    #[test]
    fn abstention_top_is_out_of_scope() {
        let b = board();
        let r = result(&b, vec![(NONE_OF_THE_ABOVE, 0.95), ("op.append_node", 0.20)]);
        let (d, _) = decide(&DispositionConfig::shadow_v1(), &b, &r, "x").unwrap();
        assert_eq!(d, ProposalDisposition::OutOfScope);
    }

    /// I15 red: off-board model output is an ERROR; I28 red: evidence
    /// from a different board is inadmissible.
    #[test]
    fn off_board_and_wrong_board_evidence_are_rejected() {
        let b = board();
        let r = result(&b, vec![("verb.made_up_by_model", 0.99)]);
        let err = decide(&DispositionConfig::shadow_v1(), &b, &r, "x").unwrap_err();
        assert!(err.to_string().contains("not on board"));

        let mut wrong = result(&b, vec![("op.append_node", 0.9)]);
        wrong.board_hash = "deadbeef".into();
        let err = decide(&DispositionConfig::shadow_v1(), &b, &wrong, "x").unwrap_err();
        assert!(err.to_string().contains("does not match"));
    }

    /// E5: threshold change ⇒ different policy hash (the hash is the
    /// config's content address, so records are reproducible).
    #[test]
    fn policy_hash_tracks_config_content() {
        let a = DispositionConfig::shadow_v1();
        let mut b = DispositionConfig::shadow_v1();
        assert_eq!(a.policy_hash(), b.policy_hash());
        b.separation_margin = 0.20;
        assert_ne!(a.policy_hash(), b.policy_hash());
    }
}
