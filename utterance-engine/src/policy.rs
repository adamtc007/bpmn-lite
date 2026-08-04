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

    /// Content address over a HAND-BUILT preimage (blind-review C4):
    /// f64 thresholds enter as IEEE-754 bit patterns, never as decimal
    /// text, so the hash cannot drift with a serializer's float
    /// formatting or a struct-field reorder.
    pub fn policy_hash(&self) -> String {
        let mut preimage = Vec::new();
        preimage.extend_from_slice(b"dispcfg.v1:");
        preimage.extend_from_slice(&self.policy_version.to_le_bytes());
        preimage.extend_from_slice(&self.acceptance_floor.to_bits().to_le_bytes());
        preimage.extend_from_slice(&self.separation_margin.to_bits().to_le_bytes());
        blake3::hash(&preimage).to_hex().to_string()
    }
}

/// V&S §9.2's ratified shape (I21: ambiguous, missing-argument,
/// compound, and out-of-scope are all expressible). Issued ONLY by
/// `decide`. v1 REACHABILITY (blind-review B2 disposition, recorded in
/// the plan): `MissingArguments` and `Compound` are UNREACHABLE until
/// the option-(a) slot resolvers and a certified action-span producer
/// exist respectively — §10.3 rules that score topology cannot
/// distinguish ambiguity from compound intent, so v1 maps EVERY
/// insufficient-separation case to `EscalateToSage`, never to a
/// rendered "did you mean A or B?" that may mask a compound request.
/// `Ambiguous` becomes reachable only when a certified producer can
/// distinguish the cases; making it reachable earlier is a policy
/// version bump plus a plan amendment, not a threshold tweak.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProposalDisposition {
    /// Sufficiently separated top candidate → deterministic binding
    /// proceeds (Repl still re-establishes everything).
    Candidate { candidate_id: String },
    /// UNREACHABLE in v1 (see enum docs).
    Ambiguous { top_candidates: Vec<String> },
    /// UNREACHABLE in v1: requires the option-(a) slot resolvers.
    MissingArguments { candidate_id: String, missing: Vec<String> },
    /// UNREACHABLE in v1: requires certified action-span evidence.
    Compound,
    /// The abstention hypothesis won: the board does not contain the
    /// answer. D19-rider denial rendering applies downstream.
    OutOfScope,
    /// Weak evidence OR insufficient separation (ambiguity/compound
    /// indistinguishable without span evidence, §10.3) → Sage analysis
    /// against the SAME board (D20 governs any board change).
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
    /// Session/context features as projected. Preimage carries a
    /// projection-schema version tag (review N3) so a widened
    /// projection is distinguishable in kind from v1's
    /// raw-utterance-only form.
    pub context_projection_hash: String,
    /// The ranking as evidence entered policy (canonically re-sorted),
    /// scores kept as FiniteScore so a stored record cannot
    /// reintroduce non-finite values on round-trip (review N5).
    pub ranking: Vec<(String, crate::contract::FiniteScore)>,
    pub disposition: ProposalDisposition,
}

/// THE disposition function. Fail-closed: a ranking naming any
/// candidate not on the board is an ERROR (I15 — model output outside
/// the board is rejected), never a silent skip.
pub fn decide(
    config: &DispositionConfig,
    board: &Board,
    result: &SlmResult,
    context: &crate::context::ContextProjection,
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

    // Blind-review C5: policy owns the canonical order (I28 tie-break) —
    // re-sort rather than trusting the producer; duplicate candidate ids
    // are producer malfunction, refused.
    let mut ranking = result.ranking.clone();
    crate::contract::rank_canonically(&mut ranking);
    for pair in ranking.windows(2) {
        if pair[0].candidate_id == pair[1].candidate_id {
            return Err(anyhow!(
                "ranking names '{}' more than once — duplicate evidence is refused",
                pair[0].candidate_id
            ));
        }
    }

    let disposition = match ranking.as_slice() {
        [] => {
            // A conforming producer can never emit an empty ranking
            // (NOTA is on every board): producer malfunction, fail
            // closed (review N1 strict reading).
            return Err(anyhow!("empty ranking — producer malfunction, no evidence"));
        }
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
                    // §10.3 ruling: multi-peak does NOT distinguish
                    // ambiguity from compound — escalate, never render
                    // a masking A-or-B clarification (review B2).
                    ProposalDisposition::EscalateToSage {
                        reason: format!(
                            "insufficient separation ({:.3} < {:.3}): ambiguity vs                              compound intent indistinguishable without span evidence",
                            top.score.get() - second.score.get(),
                            config.separation_margin
                        ),
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
        // DIR-002 A1: derived from the ONE canonical serializer — the
        // same bytes the training corpus embeds. Never caller-supplied.
        context_projection_hash: context.hash(),
        ranking: ranking
            .iter()
            .map(|rc| (rc.candidate_id.clone(), rc.score))
            .collect(),
        disposition: disposition.clone(),
    };
    Ok((disposition, record))
}

/// Config-by-hash registry (blind-review N3 rider): an I28 record is
/// reproducible only if `disposition_policy_hash` resolves to the config
/// it named — a supplied hash is an unverifiable claim, so registration
/// derives it. General I28 reproducibility machinery, not tied to any
/// one capture path — moved here from `capture.rs` (2026-07-29,
/// DIR-004 Phase 1) so a Q9-gated capture path can be feature-gated out
/// entirely without taking this with it; dev-session capture needs the
/// same resolution and must not depend on the Q9-gated module to get it.
#[derive(Default)]
pub struct ConfigRegistry {
    configs: std::collections::BTreeMap<String, DispositionConfig>,
}

impl ConfigRegistry {
    pub fn register(&mut self, config: DispositionConfig) -> String {
        let hash = config.policy_hash();
        self.configs.insert(hash.clone(), config);
        hash
    }

    pub fn resolve(&self, policy_hash: &str) -> Option<&DispositionConfig> {
        self.configs.get(policy_hash)
    }
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
        build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default()).unwrap()
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
        let (d, rec) = decide(&cfg, &b, &r, &crate::context::minimal("pack.none", "g-test")).unwrap();
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

    /// §10.3 ruling (review B2): insufficient separation ESCALATES —
    /// ambiguity vs compound is indistinguishable without span
    /// evidence, so no masking A-or-B clarification is rendered.
    #[test]
    fn close_scores_and_weak_evidence_both_escalate() {
        let b = board();
        let cfg = DispositionConfig::shadow_v1();
        let r = result(&b, vec![("op.append_node", 0.80), ("op.insert_after", 0.75)]);
        let (d, _) = decide(&cfg, &b, &r, &crate::context::minimal("pack.none", "g-test")).unwrap();
        assert!(
            matches!(&d, ProposalDisposition::EscalateToSage { reason } if reason.contains("separation")),
            "close scores must escalate, not render Ambiguous: {d:?}"
        );

        let r = result(&b, vec![("op.append_node", 0.30)]);
        let (d, _) = decide(&cfg, &b, &r, &crate::context::minimal("pack.none", "g-test")).unwrap();
        assert!(matches!(&d, ProposalDisposition::EscalateToSage { reason } if reason.contains("floor")));
    }

    /// Review C5 reds: duplicate ids refused; misordered producer input
    /// is re-sorted canonically (policy owns the order), so the true
    /// top wins regardless of emission order. Review N1: empty ranking
    /// is producer malfunction, an error.
    #[test]
    fn ranking_hygiene_is_policy_owned() {
        let b = board();
        let cfg = DispositionConfig::shadow_v1();
        let r = result(&b, vec![("op.append_node", 0.9), ("op.append_node", 0.9)]);
        assert!(decide(&cfg, &b, &r, &crate::context::minimal("pack.none", "g-test")).unwrap_err().to_string().contains("more than once"));

        // Misordered: low score listed first; re-sort selects the real top.
        let r = result(&b, vec![("op.connect", 0.55), ("op.append_node", 0.95)]);
        let (d, _) = decide(&cfg, &b, &r, &crate::context::minimal("pack.none", "g-test")).unwrap();
        assert_eq!(d, ProposalDisposition::Candidate { candidate_id: "op.append_node".into() });

        let r = result(&b, vec![]);
        assert!(decide(&cfg, &b, &r, &crate::context::minimal("pack.none", "g-test")).unwrap_err().to_string().contains("malfunction"));
    }

    /// Review N4: golden decision table cementing policy_version 1
    /// semantics — a semantic edit to decide() without a version bump
    /// breaks this, not just a comment. Review C4: golden policy hash.
    #[test]
    fn policy_v1_decision_table_and_hash_are_golden() {
        let cfg = DispositionConfig::shadow_v1();
        assert_eq!(cfg.policy_version, 1);
        assert_eq!(
            cfg.policy_hash(),
            GOLDEN_SHADOW_V1_POLICY_HASH,
            "shadow_v1 policy hash drifted — semantic or encoding change without a bump"
        );
        let b = board();
        type DecisionCase<'a> = (Vec<(&'a str, f64)>, fn(&ProposalDisposition) -> bool);
        let table: Vec<DecisionCase<'_>> = vec![
            (vec![("op.append_node", 0.90), ("op.connect", 0.40)],
             |d| matches!(d, ProposalDisposition::Candidate { .. })),
            (vec![("op.append_node", 0.80), ("op.insert_after", 0.70)],
             |d| matches!(d, ProposalDisposition::EscalateToSage { .. })),
            (vec![("op.append_node", 0.49)],
             |d| matches!(d, ProposalDisposition::EscalateToSage { .. })),
            (vec![(NONE_OF_THE_ABOVE, 0.99), ("op.append_node", 0.10)],
             |d| matches!(d, ProposalDisposition::OutOfScope)),
        ];
        for (ranking, check) in table {
            let r = result(&b, ranking.clone());
            let (d, _) = decide(&cfg, &b, &r, &crate::context::minimal("pack.none", "g-test")).unwrap();
            assert!(check(&d), "decision table drift for {ranking:?}: {d:?}");
        }
    }

    const GOLDEN_SHADOW_V1_POLICY_HASH: &str = "b93371789f5202158f286d44555087dba5da4b059b01b929a279479bc60815b2";

    #[test]
    fn abstention_top_is_out_of_scope() {
        let b = board();
        let r = result(&b, vec![(NONE_OF_THE_ABOVE, 0.95), ("op.append_node", 0.20)]);
        let (d, _) = decide(&DispositionConfig::shadow_v1(), &b, &r, &crate::context::minimal("pack.none", "g-test")).unwrap();
        assert_eq!(d, ProposalDisposition::OutOfScope);
    }

    /// I15 red: off-board model output is an ERROR; I28 red: evidence
    /// from a different board is inadmissible.
    #[test]
    fn off_board_and_wrong_board_evidence_are_rejected() {
        let b = board();
        let r = result(&b, vec![("verb.made_up_by_model", 0.99)]);
        let err = decide(&DispositionConfig::shadow_v1(), &b, &r, &crate::context::minimal("pack.none", "g-test")).unwrap_err();
        assert!(err.to_string().contains("not on board"));

        let mut wrong = result(&b, vec![("op.append_node", 0.9)]);
        wrong.board_hash = "deadbeef".into();
        let err = decide(&DispositionConfig::shadow_v1(), &b, &wrong, &crate::context::minimal("pack.none", "g-test")).unwrap_err();
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

    /// N3 rider: a recorded policy hash resolves back to its config.
    #[test]
    fn config_registry_makes_records_reproducible() {
        let mut reg = ConfigRegistry::default();
        let cfg = DispositionConfig::shadow_v1();
        let hash = reg.register(cfg.clone());
        assert_eq!(hash, cfg.policy_hash(), "hash is derived, not supplied");
        let resolved = reg.resolve(&hash).expect("resolvable");
        assert_eq!(resolved.policy_hash(), hash);
        assert!(reg.resolve("unknown").is_none());
    }
}
