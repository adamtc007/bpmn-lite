//! Private bounded belief update. Belief is evidence and never legality.

use std::collections::{BTreeMap, BTreeSet};

use designer_graph::schema::DesignerDag;
use semantic_decision_contracts::{
    DesignBelief, DesignPosition, FiniteScore, MotifHypothesis, MoveAttemptOutcome,
    MoveAttemptReceipt, MoveEvidence, MoveProbability, ProducerIdentity, GAMEBOARD_SCHEMA_VERSION,
};

use crate::bpmn_pack::compiled_semantic_pack;
use crate::motifs::{self, MotifState};

const MAX_LIKELY_MOVES: usize = 32;
const MAX_MOTIFS: usize = 16;
const BELIEF_PRODUCER: &str = "bpmn.design-belief.log-linear.v1";

pub(super) fn update(
    dag: &DesignerDag,
    position: &DesignPosition,
    evidence: &[MoveEvidence],
    attempts: &[MoveAttemptReceipt],
    previous: Option<&DesignBelief>,
) -> anyhow::Result<DesignBelief> {
    if evidence.len() != position.legal_moves().len() {
        anyhow::bail!("belief update requires exactly one evidence record per legal move");
    }
    let legal_ids = position
        .legal_moves()
        .iter()
        .map(|legal_move| legal_move.move_id())
        .collect::<BTreeSet<_>>();
    if evidence
        .iter()
        .any(|item| !legal_ids.contains(item.move_id()))
    {
        anyhow::bail!("belief evidence names a move outside the current legal set");
    }

    let rejected = attempts
        .iter()
        .filter(|attempt| {
            matches!(
                attempt.outcome(),
                MoveAttemptOutcome::RejectedByUser | MoveAttemptOutcome::Corrected
            )
        })
        .filter_map(MoveAttemptReceipt::attempted_move)
        .collect::<BTreeSet<_>>();
    let mut weighted = evidence
        .iter()
        .map(|item| {
            let penalty = if rejected.contains(item.move_id()) {
                0.1
            } else {
                1.0
            };
            (item.move_id().clone(), item.probability().get() * penalty)
        })
        .collect::<Vec<_>>();
    weighted.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    weighted.truncate(MAX_LIKELY_MOVES);
    let total = weighted
        .iter()
        .map(|(_, probability)| *probability)
        .sum::<f64>();
    let likely_moves = weighted
        .into_iter()
        .map(|(move_id, probability)| {
            let probability = if total > 1.0 {
                probability / total
            } else {
                probability
            };
            Ok(MoveProbability::new(
                move_id,
                FiniteScore::new(probability)?,
            )?)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let probability_by_candidate = position
        .legal_moves()
        .iter()
        .filter_map(|legal_move| {
            evidence
                .iter()
                .find(|item| item.move_id() == legal_move.move_id())
                .map(|item| (legal_move.candidate_id().as_str(), item.probability().get()))
        })
        .fold(
            BTreeMap::<&str, f64>::new(),
            |mut scores, (candidate, probability)| {
                *scores.entry(candidate).or_default() += probability;
                scores
            },
        );
    let previous_motifs = previous
        .into_iter()
        .flat_map(DesignBelief::motifs)
        .map(|motif| (motif.motif_id(), motif.probability().get()))
        .collect::<BTreeMap<_, _>>();
    let pack = compiled_semantic_pack();
    let facts = motifs::graph_facts(dag)?;
    let mut hypotheses = motifs::match_pack(pack, &facts)
        .into_iter()
        .filter(|matched| matched.state == MotifState::Active)
        .map(|matched| {
            let support = matched
                .source
                .likely_next_candidates
                .iter()
                .map(|candidate| {
                    probability_by_candidate
                        .get(candidate.as_str())
                        .copied()
                        .unwrap_or(0.0)
                })
                .sum::<f64>();
            let contrast = matched
                .source
                .discriminating_contrasts
                .iter()
                .map(|candidate| {
                    probability_by_candidate
                        .get(candidate.as_str())
                        .copied()
                        .unwrap_or(0.0)
                })
                .sum::<f64>();
            let prior = previous_motifs
                .get(matched.source.id.as_str())
                .copied()
                .unwrap_or(0.25);
            let logit = (0.55 * prior + 0.75 * support - 0.35 * contrast).clamp(0.0, 1.0);
            Ok(MotifHypothesis::new(
                matched.source.id.as_str(),
                FiniteScore::new(logit)?,
                format!(
                    "{}:{}:{}",
                    pack.receipt().artifact_hash.as_str(),
                    matched.source.id.as_str(),
                    matched.source.version
                ),
            )?)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    hypotheses.sort_by(|left, right| {
        right
            .probability()
            .get()
            .total_cmp(&left.probability().get())
            .then_with(|| left.motif_id().cmp(right.motif_id()))
    });
    hypotheses.truncate(MAX_MOTIFS);

    Ok(DesignBelief::new(
        GAMEBOARD_SCHEMA_VERSION,
        position.state_id().clone(),
        likely_moves,
        hypotheses,
        Vec::new(),
        ProducerIdentity::new(format!(
            "{BELIEF_PRODUCER}:{}",
            pack.receipt().artifact_hash.as_str()
        ))?,
    )?)
}
