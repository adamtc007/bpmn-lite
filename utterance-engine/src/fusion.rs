//! Private complete per-move evidence production and canonical fusion.

use std::collections::{BTreeMap, BTreeSet};

use semantic_decision_contracts::{
    CandidateEvidence, DesignPosition, EvidenceLane, FiniteScore as SharedFiniteScore,
    InferenceEvidence, LaneScore, LegalMove, MoveAttemptOutcome, MoveAttemptReceipt, MoveEvidence,
    ProducerIdentity, RuleCode, SemanticDecisionBoard, GAMEBOARD_SCHEMA_VERSION,
};

use crate::argument_evidence;
use crate::bpmn_pack::{compiled_semantic_pack, EVIDENCE_RULE_CODE};
use crate::contract::{rank_canonically, EvidenceTrace, FiniteScore, RankedCandidate, SlmResult};
use crate::exact::{
    candidate_serializer_hash, governed_exact, ExactMatch, GOVERNED_EXACT_BUNDLE_ID,
};
use crate::graph_features;

const FUSION_ALGORITHM_ID: &str = "bpmn.evidence-fusion.weighted-rule-dominance.v1";

struct EvidenceContext<'a> {
    board: &'a SemanticDecisionBoard,
    position: &'a DesignPosition,
    utterance: &'a str,
    base_scores: BTreeMap<&'a str, f64>,
    active_lane: EvidenceLane,
    exact: ExactMatch,
    extracted: Vec<argument_evidence::ExtractedArgument>,
    attempts: &'a [MoveAttemptReceipt],
}

trait MoveEvidenceProducer {
    fn produce(
        &self,
        context: &EvidenceContext<'_>,
        legal_move: &LegalMove,
    ) -> Vec<(EvidenceLane, f64)>;
}

struct LanguageProducer;

impl MoveEvidenceProducer for LanguageProducer {
    fn produce(
        &self,
        context: &EvidenceContext<'_>,
        legal_move: &LegalMove,
    ) -> Vec<(EvidenceLane, f64)> {
        let candidate_id = legal_move.candidate_id().as_str();
        let governed_exact = match &context.exact {
            ExactMatch::None => 0.0,
            ExactMatch::Unique(candidate) => f64::from(candidate == candidate_id),
            ExactMatch::Collision(candidates) => {
                if candidates.iter().any(|candidate| candidate == candidate_id) {
                    0.5
                } else {
                    0.0
                }
            }
        };
        let utterance = context.utterance.to_lowercase();
        let explicit_reference = utterance.contains(&candidate_id.to_lowercase())
            || utterance.contains(&legal_move.move_id().as_str().to_lowercase());
        let exact = governed_exact.max(f64::from(explicit_reference));
        let grammar = context
            .board
            .candidate(candidate_id)
            .map(|candidate| governed_token_score(context.utterance, candidate))
            .unwrap_or(0.0);
        vec![
            (EvidenceLane::GovernedExact, exact),
            (EvidenceLane::DeterministicGrammar, grammar),
        ]
    }
}

struct ArgumentProducer;

impl MoveEvidenceProducer for ArgumentProducer {
    fn produce(
        &self,
        context: &EvidenceContext<'_>,
        legal_move: &LegalMove,
    ) -> Vec<(EvidenceLane, f64)> {
        vec![(
            EvidenceLane::TypedArgument,
            argument_evidence::score(legal_move, context.utterance, &context.extracted),
        )]
    }
}

struct GraphProducer;

impl MoveEvidenceProducer for GraphProducer {
    fn produce(
        &self,
        context: &EvidenceContext<'_>,
        legal_move: &LegalMove,
    ) -> Vec<(EvidenceLane, f64)> {
        vec![
            (
                EvidenceLane::GraphLocality,
                graph_features::locality(context.position, legal_move, context.utterance),
            ),
            (
                EvidenceLane::StructuralCompletion,
                graph_features::structural_completion(legal_move),
            ),
        ]
    }
}

struct ModelProducer;

impl MoveEvidenceProducer for ModelProducer {
    fn produce(
        &self,
        context: &EvidenceContext<'_>,
        legal_move: &LegalMove,
    ) -> Vec<(EvidenceLane, f64)> {
        vec![(
            context.active_lane,
            context
                .base_scores
                .get(legal_move.candidate_id().as_str())
                .copied()
                .unwrap_or(0.0)
                .clamp(-1.0, 1.0),
        )]
    }
}

struct HistoryProducer;

impl MoveEvidenceProducer for HistoryProducer {
    fn produce(
        &self,
        context: &EvidenceContext<'_>,
        legal_move: &LegalMove,
    ) -> Vec<(EvidenceLane, f64)> {
        let mut history: f64 = 0.0;
        let mut correction: f64 = 0.0;
        for attempt in context.attempts {
            if attempt.attempted_move() != Some(legal_move.move_id()) {
                continue;
            }
            match attempt.outcome() {
                MoveAttemptOutcome::RejectedByUser => history = history.min(-1.0),
                MoveAttemptOutcome::Corrected => correction = correction.min(-1.0),
                MoveAttemptOutcome::Applied => history = history.max(0.25),
                MoveAttemptOutcome::Incomplete
                | MoveAttemptOutcome::Ambiguous
                | MoveAttemptOutcome::Inapplicable
                | MoveAttemptOutcome::DisclosureSafeRefusal
                | MoveAttemptOutcome::Stale
                | MoveAttemptOutcome::CompilerRefused
                | MoveAttemptOutcome::SystemFailure => history = history.min(-0.25),
            }
        }
        vec![
            (EvidenceLane::History, history),
            (EvidenceLane::Correction, correction),
        ]
    }
}

struct AbstentionProducer;

impl MoveEvidenceProducer for AbstentionProducer {
    fn produce(
        &self,
        context: &EvidenceContext<'_>,
        legal_move: &LegalMove,
    ) -> Vec<(EvidenceLane, f64)> {
        let score = if legal_move.candidate_id().as_str()
            == semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
        {
            let best_non_abstention = context
                .base_scores
                .iter()
                .filter(|(candidate, _)| {
                    **candidate != semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
                })
                .map(|(_, score)| *score)
                .fold(0.0, f64::max);
            (1.0 - best_non_abstention).clamp(0.0, 1.0)
        } else {
            0.0
        };
        vec![(EvidenceLane::Abstention, score)]
    }
}

pub(super) fn fuse_move_evidence(
    board: &SemanticDecisionBoard,
    position: &DesignPosition,
    utterance: &str,
    result: SlmResult,
    active_lane: EvidenceLane,
    bundle_identities: Vec<String>,
    attempts: &[MoveAttemptReceipt],
) -> anyhow::Result<SlmResult> {
    let producers = standard_producers();
    fuse_move_evidence_with_producers(
        board,
        position,
        utterance,
        result,
        active_lane,
        bundle_identities,
        attempts,
        &producers,
    )
}

#[allow(clippy::too_many_arguments)]
fn fuse_move_evidence_with_producers(
    board: &SemanticDecisionBoard,
    position: &DesignPosition,
    utterance: &str,
    mut result: SlmResult,
    active_lane: EvidenceLane,
    mut bundle_identities: Vec<String>,
    attempts: &[MoveAttemptReceipt],
    producers: &[&dyn MoveEvidenceProducer],
) -> anyhow::Result<SlmResult> {
    if !matches!(
        active_lane,
        EvidenceLane::Lexical | EvidenceLane::Embedding | EvidenceLane::CandleCrossEncoder
    ) {
        anyhow::bail!("active statistical evidence lane is not a serving producer lane");
    }
    let board_ids = board
        .candidates
        .iter()
        .map(|candidate| candidate.canonical_id.as_str())
        .collect::<BTreeSet<_>>();
    let result_ids = result
        .ranking
        .iter()
        .map(|candidate| candidate.candidate_id.as_str())
        .collect::<BTreeSet<_>>();
    if result.ranking.len() != result_ids.len() || board_ids != result_ids {
        anyhow::bail!("move evidence requires every semantic board candidate exactly once");
    }

    let policy = EvidenceFusionPolicy::admitted()?;
    let exact = governed_exact(board, utterance);
    let base_scores = result
        .ranking
        .iter()
        .map(|candidate| (candidate.candidate_id.as_str(), candidate.score.get()))
        .collect();
    let context = EvidenceContext {
        board,
        position,
        utterance,
        base_scores,
        active_lane,
        exact: exact.clone(),
        extracted: argument_evidence::extract(utterance),
        attempts,
    };
    let mut pending = Vec::with_capacity(position.legal_moves().len());
    for legal_move in position.legal_moves() {
        let mut raw = BTreeMap::<EvidenceLane, Vec<f64>>::new();
        for lane in EvidenceLane::ALL {
            raw.insert(lane, Vec::new());
        }
        for producer in producers {
            for (lane, score) in producer.produce(&context, legal_move) {
                if !score.is_finite() {
                    anyhow::bail!("evidence producer emitted a non-finite score");
                }
                raw.get_mut(&lane)
                    .ok_or_else(|| anyhow::anyhow!("undeclared evidence lane"))?
                    .push(score);
            }
        }
        // Policy v1 assigns each lane to exactly one producer. Producer output
        // is already bounded, so the recorded lane value is both the raw lane
        // score and its identity-normalized value. This avoids a duplicate DTO.
        if raw.values().any(|values| values.len() > 1) {
            anyhow::bail!("evidence lane has more than one owning producer");
        }
        let lane_scores = EvidenceLane::ALL
            .into_iter()
            .map(|lane| {
                let values = &raw[&lane];
                let normalized = values.iter().copied().sum::<f64>().clamp(-1.0, 1.0);
                Ok(LaneScore {
                    lane,
                    score: SharedFiniteScore::new(normalized)?,
                })
            })
            .collect::<Result<Vec<_>, semantic_decision_contracts::DecisionBoardError>>()?;
        let base_score = context
            .base_scores
            .get(legal_move.candidate_id().as_str())
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let mut final_score = (policy.fuse(&lane_scores).max(base_score)
            + policy.negative_history_adjustment(&lane_scores))
        .clamp(0.0, 1.0);
        if matches_exact(&exact, legal_move.candidate_id().as_str()) {
            final_score = 0.55 + 0.45 * final_score;
        } else if !matches!(exact, ExactMatch::None) {
            final_score *= 0.45;
        }
        pending.push((legal_move, lane_scores, final_score.clamp(0.0, 1.0)));
    }
    let probabilities = softmax(pending.iter().map(|(_, _, score)| *score));
    let producer = ProducerIdentity::new(policy.identity.clone())?;
    let evidence_rule = RuleCode::new(EVIDENCE_RULE_CODE)?;
    let mut move_evidence = pending
        .iter()
        .zip(probabilities)
        .map(|((legal_move, lanes, final_score), probability)| {
            let final_score = SharedFiniteScore::new(*final_score)?;
            let probability = SharedFiniteScore::new(probability)?;
            Ok::<_, anyhow::Error>(MoveEvidence::new(
                GAMEBOARD_SCHEMA_VERSION,
                legal_move.move_id().clone(),
                lanes.clone(),
                final_score,
                probability,
                vec![evidence_rule.clone()],
                producer.clone(),
            )?)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    move_evidence.sort_by(|left, right| left.move_id().cmp(right.move_id()));

    let mut candidates = Vec::with_capacity(board.candidates.len());
    for candidate in &board.candidates {
        let matching = move_evidence
            .iter()
            .filter(|evidence| {
                position
                    .legal_moves()
                    .iter()
                    .find(|legal_move| legal_move.move_id() == evidence.move_id())
                    .is_some_and(|legal_move| legal_move.candidate_id() == &candidate.canonical_id)
            })
            .collect::<Vec<_>>();
        let final_score = matching
            .iter()
            .map(|evidence| evidence.final_score().get())
            .fold(0.0, f64::max);
        let lane_scores = EvidenceLane::ALL
            .into_iter()
            .map(|lane| {
                let value = if matching.is_empty() {
                    0.0
                } else {
                    matching
                        .iter()
                        .filter_map(|evidence| {
                            evidence
                                .lanes()
                                .iter()
                                .find(|score| score.lane == lane)
                                .map(|score| score.score.get())
                        })
                        .fold(-1.0, f64::max)
                };
                LaneScore {
                    lane,
                    score: SharedFiniteScore::new(value).expect("finite normalized lane"),
                }
            })
            .collect();
        candidates.push(CandidateEvidence {
            candidate_id: candidate.canonical_id.clone(),
            lane_scores,
            final_score: SharedFiniteScore::new(final_score)?,
        });
    }
    let turn_context_hash =
        blake3::hash(format!("{}\0{}", position.state_id().as_str(), utterance).as_bytes())
            .to_hex()
            .to_string();
    let inference = InferenceEvidence::new(
        board,
        turn_context_hash,
        candidate_serializer_hash(),
        policy.identity.clone(),
        candidates,
    )?;
    result.ranking = inference
        .ranked
        .iter()
        .map(|candidate| {
            Ok(RankedCandidate {
                candidate_id: candidate.candidate_id.as_str().to_string(),
                score: FiniteScore::new(candidate.final_score.get())?,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    rank_canonically(&mut result.ranking);
    bundle_identities.push(policy.identity.clone());
    if !matches!(exact, ExactMatch::None) {
        bundle_identities.push(GOVERNED_EXACT_BUNDLE_ID.to_string());
    }
    bundle_identities.sort();
    bundle_identities.dedup();
    result.evidence_trace = Some(EvidenceTrace {
        candidate_serializer_hash: candidate_serializer_hash(),
        turn_serializer_hash: crate::pair::turn_serializer_hash(),
        lanes: EvidenceLane::ALL.to_vec(),
        bundle_identities,
        exact_collision: match exact {
            ExactMatch::Collision(candidates) => candidates,
            ExactMatch::None | ExactMatch::Unique(_) => Vec::new(),
        },
        served_full_board: true,
    });
    result.inference_evidence = Some(inference);
    result.move_evidence = move_evidence;
    Ok(result)
}

fn standard_producers() -> [&'static dyn MoveEvidenceProducer; 6] {
    [
        &LanguageProducer,
        &ArgumentProducer,
        &GraphProducer,
        &ModelProducer,
        &HistoryProducer,
        &AbstentionProducer,
    ]
}

struct EvidenceFusionPolicy {
    weights: BTreeMap<EvidenceLane, f64>,
    identity: String,
}

impl EvidenceFusionPolicy {
    fn admitted() -> anyhow::Result<Self> {
        let source = compiled_semantic_pack().evidence();
        let weights = source
            .features
            .iter()
            .map(|feature| (feature.lane, f64::from(feature.weight_millis)))
            .collect::<BTreeMap<_, _>>();
        if weights.len() != EvidenceLane::ALL.len()
            || EvidenceLane::ALL
                .into_iter()
                .any(|lane| !weights.contains_key(&lane))
        {
            anyhow::bail!("admitted BPMN fusion policy does not cover every evidence lane");
        }
        let canonical = serde_json::to_vec(source)?;
        let mut preimage = FUSION_ALGORITHM_ID.as_bytes().to_vec();
        preimage.push(0);
        preimage.extend(canonical);
        let identity = format!(
            "{}@{}",
            FUSION_ALGORITHM_ID,
            blake3::hash(&preimage).to_hex()
        );
        Ok(Self { weights, identity })
    }

    fn fuse(&self, scores: &[LaneScore]) -> f64 {
        let numerator = scores
            .iter()
            .map(|score| self.weights[&score.lane] * score.score.get())
            .sum::<f64>();
        let denominator = scores
            .iter()
            .filter(|score| score.score.get() != 0.0)
            .map(|score| self.weights[&score.lane])
            .sum::<f64>();
        if denominator == 0.0 {
            0.0
        } else {
            (numerator / denominator).clamp(0.0, 1.0)
        }
    }

    fn negative_history_adjustment(&self, scores: &[LaneScore]) -> f64 {
        let numerator = scores
            .iter()
            .filter(|score| matches!(score.lane, EvidenceLane::History | EvidenceLane::Correction))
            .map(|score| self.weights[&score.lane] * score.score.get().min(0.0))
            .sum::<f64>();
        numerator / self.weights.values().sum::<f64>()
    }
}

fn matches_exact(exact: &ExactMatch, candidate: &str) -> bool {
    match exact {
        ExactMatch::None => false,
        ExactMatch::Unique(value) => value == candidate,
        ExactMatch::Collision(values) => values.iter().any(|value| value == candidate),
    }
}

fn governed_token_score(
    utterance: &str,
    candidate: &semantic_decision_contracts::CandidateSemanticSlice,
) -> f64 {
    let utterance = tokens(utterance);
    if utterance.is_empty() {
        return 0.0;
    }
    let positive = candidate
        .phrases
        .iter()
        .map(|phrase| tokens(&phrase.text))
        .chain(
            candidate
                .positive_examples
                .iter()
                .map(|example| tokens(example)),
        )
        .map(|cue| token_overlap(&utterance, &cue))
        .fold(0.0, f64::max);
    let negative = candidate
        .negative_contrasts
        .iter()
        .map(|contrast| token_overlap(&utterance, &tokens(&contrast.distinction)))
        .fold(0.0, f64::max);
    (positive - negative * 0.5).clamp(-1.0, 1.0)
}

fn tokens(value: &str) -> BTreeSet<String> {
    value
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| token.len() > 1)
        .map(ToOwned::to_owned)
        .collect()
}

fn token_overlap(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f64 {
    if right.is_empty() {
        return 0.0;
    }
    left.intersection(right).count() as f64 / right.len() as f64
}

fn softmax(scores: impl IntoIterator<Item = f64>) -> Vec<f64> {
    let scores = scores.into_iter().collect::<Vec<_>>();
    if scores.is_empty() {
        return Vec::new();
    }
    let max = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let exponentials = scores
        .iter()
        .map(|score| (score - max).exp())
        .collect::<Vec<_>>();
    let sum = exponentials.iter().sum::<f64>();
    exponentials.into_iter().map(|value| value / sum).collect()
}

#[cfg(test)]
mod tests {
    use bpmn_lite_compiler::IRNode;
    use designer_graph::ops::{apply, Operation};
    use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
    use semantic_decision_contracts::{
        CorrectionKind, DesignFocus, GraphContentHash, GraphElementRef, MoveAttemptId,
    };
    use uuid::Uuid;

    use super::*;
    use crate::board::PolicyFilter;
    use crate::bpmn_board::{build_bpmn_design_position, build_bpmn_semantic_board};

    fn fixture() -> (SemanticDecisionBoard, DesignPosition) {
        let start = NodeKey(Uuid::from_u128(1));
        let task = NodeKey(Uuid::from_u128(2));
        let mut dag = DesignerDag::new("evidence-fixture");
        dag.seed(
            start,
            IRNode::Start { id: "start".into() },
            Provenance::default(),
        )
        .unwrap();
        let dag = apply(
            &dag,
            Operation::AppendNode {
                anchor: start,
                key: task,
                node: IRNode::ServiceTask {
                    id: "task-1".into(),
                    name: "Review".into(),
                    task_type: "review".into(),
                },
                edge_id: "flow-1".into(),
            },
            Provenance::default(),
        )
        .unwrap()
        .candidate;
        let revision = "a".repeat(64);
        let board = build_bpmn_semantic_board(
            &dag,
            Some((task, "task-1")),
            &revision,
            &PolicyFilter::default(),
        )
        .unwrap();
        let position = build_bpmn_design_position(
            &dag,
            &board,
            &revision,
            &"b".repeat(64),
            "compiler-profile-v1",
            &"c".repeat(64),
            DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
            None,
        )
        .unwrap();
        (board, position)
    }

    fn base(board: &SemanticDecisionBoard) -> SlmResult {
        SlmResult {
            ranking: board
                .candidates
                .iter()
                .map(|candidate| RankedCandidate {
                    candidate_id: candidate.canonical_id.as_str().to_string(),
                    score: FiniteScore::new(0.2).unwrap(),
                })
                .collect(),
            retrieved_subset_hash: "full-board".into(),
            board_hash: board.board_hash.as_str().to_string(),
            model_bundle_hash: "test.lexical".into(),
            evidence_trace: None,
            inference_evidence: None,
            move_evidence: Vec::new(),
        }
    }

    fn lane_for_position(
        result: &SlmResult,
        position: &DesignPosition,
        candidate: &str,
        lane: EvidenceLane,
    ) -> f64 {
        result
            .move_evidence
            .iter()
            .filter_map(|evidence| {
                position
                    .legal_moves()
                    .iter()
                    .find(|legal_move| legal_move.move_id() == evidence.move_id())
                    .filter(|legal_move| legal_move.candidate_id().as_str() == candidate)
                    .and_then(|_| evidence.lanes().iter().find(|score| score.lane == lane))
                    .map(|score| score.score.get())
            })
            .fold(-1.0, f64::max)
    }

    fn final_for_position(result: &SlmResult, position: &DesignPosition, candidate: &str) -> f64 {
        result
            .move_evidence
            .iter()
            .filter_map(|evidence| {
                position
                    .legal_moves()
                    .iter()
                    .find(|legal_move| legal_move.move_id() == evidence.move_id())
                    .filter(|legal_move| legal_move.candidate_id().as_str() == candidate)
                    .map(|_| evidence.final_score().get())
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn every_legal_move_has_one_complete_finite_vector_and_candidate_projection() {
        let (board, position) = fixture();
        let first = fuse_move_evidence(
            &board,
            &position,
            "insert after task-1",
            base(&board),
            EvidenceLane::Lexical,
            vec!["test.lexical".into()],
            &[],
        )
        .unwrap();
        assert_eq!(first.move_evidence.len(), position.legal_moves().len());
        assert!(first.move_evidence.iter().all(|evidence| {
            evidence.lanes().len() == EvidenceLane::ALL.len()
                && evidence
                    .lanes()
                    .iter()
                    .all(|lane| lane.score.get().is_finite())
                && evidence.final_score().get().is_finite()
                && evidence.probability().get().is_finite()
        }));
        assert_eq!(
            first.inference_evidence.as_ref().unwrap().ranked.len(),
            board.candidates.len()
        );

        let mut reordered = base(&board);
        reordered.ranking.reverse();
        let second = fuse_move_evidence(
            &board,
            &position,
            "insert after task-1",
            reordered,
            EvidenceLane::Lexical,
            vec!["test.lexical".into()],
            &[],
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&first.move_evidence).unwrap(),
            serde_json::to_value(&second.move_evidence).unwrap()
        );
        assert_eq!(
            first.inference_evidence.unwrap().evidence_record_hash,
            second.inference_evidence.unwrap().evidence_record_hash
        );
    }

    #[test]
    fn typed_values_node_references_and_negative_contrasts_move_only_evidence() {
        let (board, position) = fixture();
        let resolve = |utterance: &str| {
            fuse_move_evidence(
                &board,
                &position,
                utterance,
                base(&board),
                EvidenceLane::Lexical,
                vec!["test.lexical".into()],
                &[],
            )
            .unwrap()
        };
        let baseline = resolve("change the workflow");
        let move_set_hash = position.move_set_hash().as_str().to_owned();
        let explicit_candidate = resolve("use op.insert_after here");
        assert!(
            lane_for_position(
                &explicit_candidate,
                &position,
                "op.insert_after",
                EvidenceLane::GovernedExact,
            ) > lane_for_position(
                &baseline,
                &position,
                "op.insert_after",
                EvidenceLane::GovernedExact,
            )
        );
        let duration = resolve("interrupt task-1 after 5 minutes");
        assert!(
            lane_for_position(
                &duration,
                &position,
                "prod.interrupting_timeout",
                EvidenceLane::TypedArgument,
            ) > lane_for_position(
                &baseline,
                &position,
                "prod.interrupting_timeout",
                EvidenceLane::TypedArgument,
            )
        );
        let count = resolve("create 3 parallel branches at task-1");
        assert!(
            lane_for_position(
                &count,
                &position,
                "op.create_parallel_region",
                EvidenceLane::TypedArgument,
            ) > lane_for_position(
                &baseline,
                &position,
                "op.create_parallel_region",
                EvidenceLane::TypedArgument,
            )
        );
        assert!(
            lane_for_position(
                &count,
                &position,
                "op.create_parallel_region",
                EvidenceLane::GraphLocality,
            ) > lane_for_position(
                &baseline,
                &position,
                "op.create_parallel_region",
                EvidenceLane::GraphLocality,
            )
        );
        let positive = resolve("run every branch concurrently and join them all");
        let contrast = resolve("conditionally selected inclusive branches");
        assert!(
            lane_for_position(
                &positive,
                &position,
                "op.create_parallel_region",
                EvidenceLane::DeterministicGrammar,
            ) > lane_for_position(
                &contrast,
                &position,
                "op.create_parallel_region",
                EvidenceLane::DeterministicGrammar,
            )
        );
        assert_eq!(position.move_set_hash().as_str(), move_set_hash);
    }

    #[test]
    fn producer_bundle_and_candidate_order_do_not_change_the_fused_record() {
        let (board, position) = fixture();
        let mut producers = standard_producers();
        let first = fuse_move_evidence_with_producers(
            &board,
            &position,
            "insert after task-1",
            base(&board),
            EvidenceLane::Lexical,
            vec!["z.bundle".into(), "a.bundle".into()],
            &[],
            &producers,
        )
        .unwrap();

        producers.reverse();
        let mut reordered = base(&board);
        reordered.ranking.reverse();
        let second = fuse_move_evidence_with_producers(
            &board,
            &position,
            "insert after task-1",
            reordered,
            EvidenceLane::Lexical,
            vec!["a.bundle".into(), "z.bundle".into()],
            &[],
            &producers,
        )
        .unwrap();

        assert_eq!(
            serde_json::to_value(&first.move_evidence).unwrap(),
            serde_json::to_value(&second.move_evidence).unwrap()
        );
        assert_eq!(
            first.inference_evidence.unwrap().evidence_record_hash,
            second.inference_evidence.unwrap().evidence_record_hash
        );
    }

    #[test]
    fn rejected_and_corrected_attempts_are_negative_context_not_positive_labels() {
        let (board, position) = fixture();
        let target = position
            .legal_moves()
            .iter()
            .find(|legal_move| legal_move.candidate_id().as_str() == "op.insert_after")
            .unwrap();
        let receipt = |outcome: MoveAttemptOutcome, attempt_id: &str, correction: Option<&str>| {
            MoveAttemptReceipt::new(
                GAMEBOARD_SCHEMA_VERSION,
                MoveAttemptId::new(attempt_id).unwrap(),
                position.state_id().clone(),
                Some(target.move_id().clone()),
                GraphContentHash::new("d".repeat(64)).unwrap(),
                outcome,
                Vec::new(),
                Vec::new(),
                correction.map(|value| MoveAttemptId::new(value).unwrap()),
                correction.map(|_| CorrectionKind::Replacement),
            )
            .unwrap()
        };
        let rejected = receipt(MoveAttemptOutcome::RejectedByUser, "attempt-rejected", None);
        let corrected = receipt(
            MoveAttemptOutcome::Corrected,
            "attempt-corrected",
            Some("attempt-original"),
        );
        let result = fuse_move_evidence(
            &board,
            &position,
            "insert after",
            base(&board),
            EvidenceLane::Lexical,
            vec!["test.lexical".into()],
            &[rejected, corrected],
        )
        .unwrap();
        let without_history = fuse_move_evidence(
            &board,
            &position,
            "insert after",
            base(&board),
            EvidenceLane::Lexical,
            vec!["test.lexical".into()],
            &[],
        )
        .unwrap();
        assert_eq!(
            lane_for_position(&result, &position, "op.insert_after", EvidenceLane::History,),
            -1.0
        );
        assert_eq!(
            lane_for_position(
                &result,
                &position,
                "op.insert_after",
                EvidenceLane::Correction,
            ),
            -1.0
        );
        assert!(
            final_for_position(&result, &position, "op.insert_after")
                < final_for_position(&without_history, &position, "op.insert_after")
        );
    }
}
