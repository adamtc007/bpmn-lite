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

use crate::board::InferenceBoard;
use crate::contract::{SlmResult, NONE_OF_THE_ABOVE};

#[cfg(test)]
use crate::board::Board;

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

    /// Phase 8 policy: permits contract-derived clarification on semantic
    /// boards while retaining the same unratified shadow thresholds.
    pub fn shadow_v2() -> Self {
        DispositionConfig {
            policy_version: 2,
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
/// `decide`. v1 maps every insufficient-separation case to
/// `EscalateToSage`: score topology cannot distinguish ambiguity from
/// compound intent. v2 makes `Ambiguous` reachable only where the two
/// semantic candidates carry reciprocal, governed negative contrasts.
/// `Compound` is issued only by `decide_with_action_spans` when its
/// independently identified producer supplies strict span evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProposalDisposition {
    /// Sufficiently separated top candidate → deterministic binding
    /// proceeds (Repl still re-establishes everything).
    Candidate { candidate_id: String },
    /// Reachable from v2 only with a reciprocal governed contrast.
    Ambiguous {
        top_candidates: Vec<String>,
        question: String,
    },
    /// UNREACHABLE in v1: requires the option-(a) slot resolvers.
    MissingArguments {
        candidate_id: String,
        missing: Vec<String>,
    },
    /// Reachable only with independently identified action-span evidence.
    Compound { spans: Vec<String> },
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_trace: Option<crate::contract::EvidenceTrace>,
    /// Resolvable board dump. `None` only for records written before Phase 8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<crate::corpus_schema::BoardDump>,
    /// Identity of the producer that established the atomic/compound boundary.
    #[serde(default)]
    pub action_span_producer_hash: String,
    /// Content address of the complete historical decision closure.
    pub decision_record_hash: String,
}

/// THE disposition function. Fail-closed: a ranking naming any
/// candidate not on the board is an ERROR (I15 — model output outside
/// the board is rejected), never a silent skip.
pub fn decide(
    config: &DispositionConfig,
    board: &dyn InferenceBoard,
    result: &SlmResult,
    context: &crate::context::ContextProjection,
) -> Result<(ProposalDisposition, DecisionRecord)> {
    if result.board_hash != board.board_hash() {
        return Err(anyhow!(
            "SlmResult board_hash {} does not match the presented board {} — evidence \
             from a different board is inadmissible (I28)",
            result.board_hash,
            board.board_hash()
        ));
    }
    for rc in &result.ranking {
        if !board.contains(&rc.candidate_id) {
            return Err(anyhow!(
                "ranking names '{}' which is not on board {} — off-board model output \
                 is rejected (I15)",
                rc.candidate_id,
                board.board_hash()
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
                    if config.policy_version >= 2 {
                        contract_clarification(board, &top.candidate_id, &second.candidate_id)
                            .map(|question| ProposalDisposition::Ambiguous {
                                top_candidates: vec![
                                    top.candidate_id.clone(),
                                    second.candidate_id.clone(),
                                ],
                                question,
                            })
                            .unwrap_or_else(|| ProposalDisposition::EscalateToSage {
                                reason: "close candidates lack a governed discriminating contrast"
                                    .to_string(),
                            })
                    } else {
                        ProposalDisposition::EscalateToSage {
                            reason: format!(
                                "insufficient separation ({:.3} < {:.3}): ambiguity vs compound intent indistinguishable without span evidence",
                                top.score.get() - second.score.get(),
                                config.separation_margin
                            ),
                        }
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

    let mut record = DecisionRecord {
        board_hash: board.board_hash().to_string(),
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
        evidence_trace: result.evidence_trace.clone(),
        board: Some(crate::corpus_schema::BoardDump::from_inference_board(board)),
        action_span_producer_hash: crate::disposition::producer_hash(
            crate::disposition::NO_ACTION_SPAN_PRODUCER_ID,
        ),
        decision_record_hash: String::new(),
    };
    record.decision_record_hash = decision_record_hash(&record)?;
    Ok((disposition, record))
}

fn decision_record_hash(record: &DecisionRecord) -> Result<String> {
    let preimage = serde_json::to_vec(&(
        &record.board_hash,
        &record.retrieved_subset_hash,
        &record.model_bundle_hash,
        &record.disposition_policy_hash,
        &record.context_projection_hash,
        &record.ranking,
        &record.disposition,
        &record.evidence_trace,
        &record.board,
        &record.action_span_producer_hash,
    ))?;
    Ok(blake3::hash(&preimage).to_hex().to_string())
}

fn contract_clarification(
    board: &dyn InferenceBoard,
    first_id: &str,
    second_id: &str,
) -> Option<String> {
    let semantic = board.semantic_board()?;
    let first = semantic
        .candidates
        .iter()
        .find(|candidate| candidate.canonical_id.as_str() == first_id)?;
    let second = semantic
        .candidates
        .iter()
        .find(|candidate| candidate.canonical_id.as_str() == second_id)?;
    let first_distinction = first
        .negative_contrasts
        .iter()
        .find(|contrast| contrast.candidate_id.as_str() == second_id)?;
    let second_distinction = second
        .negative_contrasts
        .iter()
        .find(|contrast| contrast.candidate_id.as_str() == first_id)?;
    Some(format!(
        "Did you mean '{}' ({}) or '{}' ({})?",
        first.title, first_distinction.distinction, second.title, second_distinction.distinction,
    ))
}

/// Apply deterministic policy with an independently identified action-span
/// producer. Strict compound evidence overrides scalar score topology; two
/// score peaks alone never become two actions.
pub fn decide_with_action_spans(
    config: &DispositionConfig,
    board: &dyn InferenceBoard,
    result: &SlmResult,
    context: &crate::context::ContextProjection,
    utterance: &str,
    producer: &dyn crate::disposition::ActionSpanProducer,
) -> Result<(ProposalDisposition, DecisionRecord)> {
    let (mut disposition, mut record) = decide(config, board, result, context)?;
    record.action_span_producer_hash = crate::disposition::producer_hash(producer.producer_id());
    if let Some(semantic) = board.semantic_board() {
        if let Some(evidence) = producer.detect(utterance, semantic) {
            disposition = ProposalDisposition::Compound {
                spans: evidence.spans,
            };
            record.disposition = disposition.clone();
        }
    }
    record.decision_record_hash = decision_record_hash(&record)?;
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
        build_board(
            &AllLegal,
            None,
            None,
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .unwrap()
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
            evidence_trace: None,
            inference_evidence: None,
            move_evidence: Vec::new(),
        }
    }

    fn semantic_board_at(
        policy: &PolicyFilter,
        revision: &str,
    ) -> semantic_decision_contracts::SemanticDecisionBoard {
        let state = crate::fixtures::enumeration_classes()
            .unwrap()
            .into_iter()
            .find(|state| state.class_id == "mid_sequence_task")
            .unwrap();
        crate::bpmn_board::build_bpmn_semantic_board(
            &state.dag,
            state.anchor_key.zip(state.anchor_id),
            revision,
            policy,
        )
        .unwrap()
    }

    fn semantic_board(
        policy: &PolicyFilter,
    ) -> semantic_decision_contracts::SemanticDecisionBoard {
        semantic_board_at(policy, "rev-policy-v2")
    }

    fn semantic_result(
        board: &semantic_decision_contracts::SemanticDecisionBoard,
        ranking: Vec<(&str, f64)>,
    ) -> SlmResult {
        let mut ranking = ranking
            .into_iter()
            .map(|(id, score)| RankedCandidate {
                candidate_id: id.to_string(),
                score: FiniteScore::new(score).unwrap(),
            })
            .collect::<Vec<_>>();
        crate::contract::rank_canonically(&mut ranking);
        SlmResult {
            ranking,
            retrieved_subset_hash: "semantic-subset-v1".into(),
            board_hash: board.board_hash.as_str().to_string(),
            model_bundle_hash: "semantic-test-bundle-v1".into(),
            evidence_trace: None,
            inference_evidence: None,
            move_evidence: Vec::new(),
        }
    }

    #[test]
    fn clear_winner_selects_and_record_closes_over_all_hashes() {
        let b = board();
        let cfg = DispositionConfig::shadow_v1();
        let r = result(&b, vec![("op.append_node", 0.90), ("op.connect", 0.40)]);
        let (d, rec) = decide(
            &cfg,
            &b,
            &r,
            &crate::context::minimal("pack.none", "g-test"),
        )
        .unwrap();
        assert_eq!(
            d,
            ProposalDisposition::Candidate {
                candidate_id: "op.append_node".into()
            }
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
        let r = result(
            &b,
            vec![("op.append_node", 0.80), ("op.insert_after", 0.75)],
        );
        let (d, _) = decide(
            &cfg,
            &b,
            &r,
            &crate::context::minimal("pack.none", "g-test"),
        )
        .unwrap();
        assert!(
            matches!(&d, ProposalDisposition::EscalateToSage { reason } if reason.contains("separation")),
            "close scores must escalate, not render Ambiguous: {d:?}"
        );

        let r = result(&b, vec![("op.append_node", 0.30)]);
        let (d, _) = decide(
            &cfg,
            &b,
            &r,
            &crate::context::minimal("pack.none", "g-test"),
        )
        .unwrap();
        assert!(
            matches!(&d, ProposalDisposition::EscalateToSage { reason } if reason.contains("floor"))
        );
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
        assert!(decide(
            &cfg,
            &b,
            &r,
            &crate::context::minimal("pack.none", "g-test")
        )
        .unwrap_err()
        .to_string()
        .contains("more than once"));

        // Misordered: low score listed first; re-sort selects the real top.
        let r = result(&b, vec![("op.connect", 0.55), ("op.append_node", 0.95)]);
        let (d, _) = decide(
            &cfg,
            &b,
            &r,
            &crate::context::minimal("pack.none", "g-test"),
        )
        .unwrap();
        assert_eq!(
            d,
            ProposalDisposition::Candidate {
                candidate_id: "op.append_node".into()
            }
        );

        let r = result(&b, vec![]);
        assert!(decide(
            &cfg,
            &b,
            &r,
            &crate::context::minimal("pack.none", "g-test")
        )
        .unwrap_err()
        .to_string()
        .contains("malfunction"));
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
            (vec![("op.append_node", 0.90), ("op.connect", 0.40)], |d| {
                matches!(d, ProposalDisposition::Candidate { .. })
            }),
            (
                vec![("op.append_node", 0.80), ("op.insert_after", 0.70)],
                |d| matches!(d, ProposalDisposition::EscalateToSage { .. }),
            ),
            (vec![("op.append_node", 0.49)], |d| {
                matches!(d, ProposalDisposition::EscalateToSage { .. })
            }),
            (
                vec![(NONE_OF_THE_ABOVE, 0.99), ("op.append_node", 0.10)],
                |d| matches!(d, ProposalDisposition::OutOfScope),
            ),
        ];
        for (ranking, check) in table {
            let r = result(&b, ranking.clone());
            let (d, _) = decide(
                &cfg,
                &b,
                &r,
                &crate::context::minimal("pack.none", "g-test"),
            )
            .unwrap();
            assert!(check(&d), "decision table drift for {ranking:?}: {d:?}");
        }
    }

    const GOLDEN_SHADOW_V1_POLICY_HASH: &str =
        "b93371789f5202158f286d44555087dba5da4b059b01b929a279479bc60815b2";

    #[test]
    fn abstention_top_is_out_of_scope() {
        let b = board();
        let r = result(
            &b,
            vec![(NONE_OF_THE_ABOVE, 0.95), ("op.append_node", 0.20)],
        );
        let (d, _) = decide(
            &DispositionConfig::shadow_v1(),
            &b,
            &r,
            &crate::context::minimal("pack.none", "g-test"),
        )
        .unwrap();
        assert_eq!(d, ProposalDisposition::OutOfScope);
    }

    /// I15 red: off-board model output is an ERROR; I28 red: evidence
    /// from a different board is inadmissible.
    #[test]
    fn off_board_and_wrong_board_evidence_are_rejected() {
        let b = board();
        let r = result(&b, vec![("verb.made_up_by_model", 0.99)]);
        let err = decide(
            &DispositionConfig::shadow_v1(),
            &b,
            &r,
            &crate::context::minimal("pack.none", "g-test"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not on board"));

        let mut wrong = result(&b, vec![("op.append_node", 0.9)]);
        wrong.board_hash = "deadbeef".into();
        let err = decide(
            &DispositionConfig::shadow_v1(),
            &b,
            &wrong,
            &crate::context::minimal("pack.none", "g-test"),
        )
        .unwrap_err();
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

    #[test]
    fn shadow_v2_uses_only_governed_reciprocal_contrasts_for_clarification() {
        let semantic_board = semantic_board(&PolicyFilter::default());
        let semantic_evidence = semantic_result(
            &semantic_board,
            vec![
                ("op.attach_guard", 0.80),
                ("op.attach_rearming_guard", 0.75),
            ],
        );
        let context = crate::context::minimal("pack.semantic", "rev-policy-v2");
        let (first, first_record) = decide(
            &DispositionConfig::shadow_v2(),
            &semantic_board,
            &semantic_evidence,
            &context,
        )
        .unwrap();
        let (second, second_record) = decide(
            &DispositionConfig::shadow_v2(),
            &semantic_board,
            &semantic_evidence,
            &context,
        )
        .unwrap();

        match &first {
            ProposalDisposition::Ambiguous {
                top_candidates,
                question,
            } => {
                assert_eq!(
                    top_candidates,
                    &["op.attach_guard", "op.attach_rearming_guard"]
                );
                assert!(question.contains("Attach an interrupting guard"));
                assert!(question.contains("Attach a rearming guard"));
                assert!(question.contains("does not"));
            }
            other => panic!("expected governed ambiguity, got {other:?}"),
        }
        assert_eq!(first, second);
        assert_eq!(
            first_record.decision_record_hash,
            second_record.decision_record_hash
        );
        assert_eq!(
            first_record
                .board
                .as_ref()
                .unwrap()
                .semantic_board
                .as_ref()
                .unwrap()
                .board_hash
                .as_str(),
            first_record.board_hash
        );

        let legacy = board();
        let close = result(
            &legacy,
            vec![("op.append_node", 0.80), ("op.insert_after", 0.75)],
        );
        let (disposition, _) = decide(
            &DispositionConfig::shadow_v2(),
            &legacy,
            &close,
            &crate::context::minimal("pack.none", "legacy"),
        )
        .unwrap();
        assert!(matches!(
            disposition,
            ProposalDisposition::EscalateToSage { .. }
        ));
    }

    #[test]
    fn strict_compound_boundary_requires_two_governed_semicolon_spans() {
        let board = semantic_board(&PolicyFilter::default());
        let result = semantic_result(&board, vec![("op.attach_guard", 0.90)]);
        let context = crate::context::minimal("pack.semantic", "rev-policy-v2");
        let producer = crate::disposition::StrictCompoundSyntax;

        let (compound, record) = decide_with_action_spans(
            &DispositionConfig::shadow_v2(),
            &board,
            &result,
            &context,
            "attach an interrupting guard; attach a rearming guard",
            &producer,
        )
        .unwrap();
        assert_eq!(
            compound,
            ProposalDisposition::Compound {
                spans: vec![
                    "attach an interrupting guard".into(),
                    "attach a rearming guard".into(),
                ]
            }
        );
        assert_eq!(
            record.action_span_producer_hash,
            crate::disposition::producer_hash(crate::disposition::STRICT_ACTION_SPAN_PRODUCER_ID)
        );

        let (atomic, _) = decide_with_action_spans(
            &DispositionConfig::shadow_v2(),
            &board,
            &result,
            &context,
            "attach an interrupting guard and attach a rearming guard",
            &producer,
        )
        .unwrap();
        assert!(matches!(atomic, ProposalDisposition::Candidate { .. }));
    }

    #[test]
    fn policy_filter_hides_denied_candidate_and_record_hash_closes_over_producers() {
        let visible = semantic_board(&PolicyFilter::default());
        let mut denied = PolicyFilter::default();
        denied.denied.insert("op.attach_guard".into());
        let hidden = semantic_board(&denied);
        assert!(!hidden.contains("op.attach_guard"));
        assert_ne!(visible.board_hash, hidden.board_hash);

        let inadmissible = semantic_result(&hidden, vec![("op.attach_guard", 0.99)]);
        let error = decide(
            &DispositionConfig::shadow_v2(),
            &hidden,
            &inadmissible,
            &crate::context::minimal("pack.semantic", "rev-policy-v2"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("not on board"));

        let mut result = semantic_result(&visible, vec![("op.attach_guard", 0.90)]);
        result.evidence_trace = Some(crate::contract::EvidenceTrace {
            candidate_serializer_hash: "serializer-v1".into(),
            turn_serializer_hash: "turn-serializer-v1".into(),
            lanes: vec![semantic_decision_contracts::EvidenceLane::Lexical],
            bundle_identities: vec!["lexical-v1".into()],
            exact_collision: Vec::new(),
            served_full_board: true,
        });
        let context = crate::context::minimal("pack.semantic", "rev-policy-v2");
        let (_, baseline) =
            decide(&DispositionConfig::shadow_v2(), &visible, &result, &context).unwrap();

        let mut different_bundle = result.clone();
        different_bundle.model_bundle_hash = "semantic-test-bundle-v2".into();
        let (_, bundle_record) = decide(
            &DispositionConfig::shadow_v2(),
            &visible,
            &different_bundle,
            &context,
        )
        .unwrap();
        assert_ne!(
            baseline.decision_record_hash,
            bundle_record.decision_record_hash
        );

        let mut different_subset = result.clone();
        different_subset.retrieved_subset_hash = "semantic-subset-v2".into();
        let (_, subset_record) = decide(
            &DispositionConfig::shadow_v2(),
            &visible,
            &different_subset,
            &context,
        )
        .unwrap();
        assert_ne!(
            baseline.decision_record_hash,
            subset_record.decision_record_hash
        );

        let (_, context_record) = decide(
            &DispositionConfig::shadow_v2(),
            &visible,
            &result,
            &crate::context::minimal("pack.semantic", "different-context"),
        )
        .unwrap();
        assert_ne!(
            baseline.decision_record_hash,
            context_record.decision_record_hash
        );

        let mut altered_policy = DispositionConfig::shadow_v2();
        altered_policy.separation_margin = 0.16;
        let (_, policy_record) = decide(&altered_policy, &visible, &result, &context).unwrap();
        assert_ne!(
            baseline.decision_record_hash,
            policy_record.decision_record_hash
        );

        let mut different_ranking = result.clone();
        different_ranking.ranking[0].score = FiniteScore::new(0.91).unwrap();
        let (_, ranking_record) = decide(
            &DispositionConfig::shadow_v2(),
            &visible,
            &different_ranking,
            &context,
        )
        .unwrap();
        assert_ne!(
            baseline.decision_record_hash,
            ranking_record.decision_record_hash
        );

        let mut different_trace = result.clone();
        different_trace
            .evidence_trace
            .as_mut()
            .unwrap()
            .candidate_serializer_hash = "serializer-v2".into();
        let (_, trace_record) = decide(
            &DispositionConfig::shadow_v2(),
            &visible,
            &different_trace,
            &context,
        )
        .unwrap();
        assert_ne!(
            baseline.decision_record_hash,
            trace_record.decision_record_hash
        );

        let mut different_turn_serializer = result.clone();
        different_turn_serializer
            .evidence_trace
            .as_mut()
            .unwrap()
            .turn_serializer_hash = "turn-serializer-v2".into();
        let (_, turn_serializer_record) = decide(
            &DispositionConfig::shadow_v2(),
            &visible,
            &different_turn_serializer,
            &context,
        )
        .unwrap();
        assert_ne!(
            baseline.decision_record_hash,
            turn_serializer_record.decision_record_hash,
            "turn_serializer_hash must move the per-utterance audit hash, not only the corpus/bundle hashes it was previously sealed into"
        );

        let different_board = semantic_board_at(&PolicyFilter::default(), "rev-policy-v3");
        let mut different_board_result = result.clone();
        different_board_result.board_hash = different_board.board_hash.as_str().to_string();
        let (_, board_record) = decide(
            &DispositionConfig::shadow_v2(),
            &different_board,
            &different_board_result,
            &context,
        )
        .unwrap();
        assert_ne!(
            baseline.decision_record_hash,
            board_record.decision_record_hash
        );

        let (_, span_record) = decide_with_action_spans(
            &DispositionConfig::shadow_v2(),
            &visible,
            &result,
            &context,
            "attach an interrupting guard",
            &crate::disposition::StrictCompoundSyntax,
        )
        .unwrap();
        assert_ne!(
            baseline.decision_record_hash,
            span_record.decision_record_hash
        );
    }

    #[test]
    fn impossible_position_scores_abstention_on_the_actual_empty_semantic_board() {
        let state = crate::fixtures::enumeration_classes()
            .unwrap()
            .into_iter()
            .find(|state| state.class_id == "empty_graph")
            .unwrap();
        let board = crate::bpmn_board::build_bpmn_semantic_board(
            &state.dag,
            None,
            "rev-empty",
            &PolicyFilter::default(),
        )
        .unwrap();
        assert_eq!(board.candidates.len(), 1);
        let evidence = crate::retrieval::Tier0Retriever::retrieve(
            &crate::retrieval::LexicalTier0,
            "attach an interrupting guard",
            &board,
        )
        .unwrap();
        let (disposition, _) = decide(
            &DispositionConfig::shadow_v2(),
            &board,
            &evidence,
            &crate::context::minimal("pack.semantic", "rev-empty"),
        )
        .unwrap();
        assert_eq!(disposition, ProposalDisposition::OutOfScope);
    }
}
