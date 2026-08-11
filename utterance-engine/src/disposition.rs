//! Strict action-span boundary for atomic BPMN inference.
//!
//! V1 does not execute compound utterances. This interface reserves the
//! producer seam, while the only implementation deliberately recognizes the
//! high-precision form `<governed exact phrase>; <governed exact phrase>`.

use crate::exact::{governed_exact, ExactMatch};
use semantic_decision_contracts::{
    DesignBelief, DesignFocus, DesignPosition, EvidenceLane, GameDisposition, LegalMoveId,
    MoveAttemptId, MoveAttemptOutcome, MoveAttemptReceipt, MoveBindingState, MoveEvidence,
    SemanticDecisionBoard,
};

pub const NO_ACTION_SPAN_PRODUCER_ID: &str = "bpmn.action-span.none.v1";
pub const STRICT_ACTION_SPAN_PRODUCER_ID: &str =
    "bpmn.action-span.strict-semicolon-governed-exact.v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionSpanEvidence {
    pub spans: Vec<String>,
}

/// G2.3: a compound-utterance chain the CALLER has already validated by
/// actually materialising and compiler-admitting span 1, then resolving
/// span 2 against the position span 1 produces -- `utterance-engine` has
/// no production path to do that itself (workbook construction is a
/// server-layer concern, `bpmn-lite-server-designer::proposal::start_workbook`;
/// see the G2.3 receipt for the Rule 7 substrate check that ruled this).
/// `compound_plan` trusts `moves` only when `spans` matches what it itself
/// detects for the SAME utterance -- a stale or mismatched chain is never
/// silently reused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedChain {
    pub spans: Vec<String>,
    pub moves: Vec<LegalMoveId>,
}

pub trait ActionSpanProducer: Send + Sync {
    fn producer_id(&self) -> &'static str;
    fn detect(&self, utterance: &str, board: &SemanticDecisionBoard) -> Option<ActionSpanEvidence>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StrictCompoundSyntax;

impl ActionSpanProducer for StrictCompoundSyntax {
    fn producer_id(&self) -> &'static str {
        STRICT_ACTION_SPAN_PRODUCER_ID
    }

    fn detect(&self, utterance: &str, board: &SemanticDecisionBoard) -> Option<ActionSpanEvidence> {
        let spans = utterance
            .split(';')
            .map(str::trim)
            .filter(|span| !span.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if spans.len() != 2 || utterance.matches(';').count() != 1 {
            return None;
        }
        let each_is_governed = spans.iter().all(|span| {
            matches!(
                governed_exact(board, span),
                ExactMatch::Unique(_) | ExactMatch::Collision(_)
            )
        });
        each_is_governed.then_some(ActionSpanEvidence { spans })
    }
}

pub fn producer_hash(producer_id: &str) -> String {
    blake3::hash(producer_id.as_bytes()).to_hex().to_string()
}

const GAME_POLICY_ID: &str = "bpmn.game-disposition.information-gain.v3";
const PROPOSAL_SCORE_FLOOR: f64 = 0.62;
const WEAK_EVIDENCE_FLOOR: f64 = 0.25;
const ARGUMENT_EVIDENCE_FLOOR: f64 = 0.75;
const CLARIFICATION_INFORMATION_GAIN_FLOOR: f64 = 0.50;

pub(crate) fn game_policy_identity() -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(GAME_POLICY_ID.as_bytes());
    for value in [
        PROPOSAL_SCORE_FLOOR,
        WEAK_EVIDENCE_FLOOR,
        ARGUMENT_EVIDENCE_FLOOR,
        CLARIFICATION_INFORMATION_GAIN_FLOOR,
    ] {
        hasher.update(&value.to_bits().to_le_bytes());
    }
    format!("{GAME_POLICY_ID}@{}", hasher.finalize().to_hex())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn decide_game(
    board: &SemanticDecisionBoard,
    position: &DesignPosition,
    evidence: &[MoveEvidence],
    belief: &DesignBelief,
    observed_intent: &str,
    attempt_id: MoveAttemptId,
    attempts: &[MoveAttemptReceipt],
    resolved_chain: Option<&ResolvedChain>,
) -> anyhow::Result<GameDisposition> {
    validate_game_inputs(position, evidence, belief)?;
    let ranked = rank_moves(position, evidence, belief);
    let non_abstention = ranked
        .iter()
        .filter(|ranked| {
            ranked.candidate_id != semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
        })
        .collect::<Vec<_>>();
    let top = ranked
        .first()
        .ok_or_else(|| anyhow::anyhow!("game disposition requires a non-empty legal move set"))?;

    if matches!(position.focus(), DesignFocus::Unknown { .. }) {
        let receipt = attempt_receipt(
            position,
            attempt_id,
            None,
            observed_intent,
            MoveAttemptOutcome::Inapplicable,
        )?;
        return Ok(GameDisposition::change_focus_or_context(
            position,
            receipt.clone(),
            receipt.feedback_options().to_vec(),
        )?);
    }

    if let Some(plan) = compound_plan(board, position, evidence, observed_intent, resolved_chain) {
        return Ok(GameDisposition::compound_plan(
            position,
            plan,
            governed_compound_prompt(board, observed_intent),
        )?);
    }

    if let Some(previous) = attempts.last() {
        if matches!(
            previous.outcome(),
            MoveAttemptOutcome::RejectedByUser | MoveAttemptOutcome::Applied
        ) && non_abstention
            .first()
            .is_none_or(|ranked| ranked.final_score < PROPOSAL_SCORE_FLOOR)
        {
            let correction_moves = non_abstention
                .iter()
                .filter(|ranked| previous.attempted_move() != Some(&ranked.move_id))
                .take(3)
                .map(|ranked| ranked.move_id.clone())
                .collect::<Vec<_>>();
            if !correction_moves.is_empty() {
                let correction_attempt = crate::history::receipt(
                    position.semantic_snapshot().as_str(),
                    attempt_id,
                    position.state_id().clone(),
                    None,
                    observed_intent,
                    MoveAttemptOutcome::RejectedByUser,
                    Some(previous.attempt_id().clone()),
                    Some(semantic_decision_contracts::CorrectionKind::FollowUp),
                )?;
                return Ok(GameDisposition::offer_correction(
                    position,
                    correction_moves,
                    correction_attempt,
                )?);
            }
        }
    }

    let recent_failures = attempts
        .iter()
        .rev()
        .take_while(|attempt| {
            !matches!(
                attempt.outcome(),
                MoveAttemptOutcome::Applied | MoveAttemptOutcome::Corrected
            )
        })
        .count();
    if recent_failures >= 3 {
        let receipt = attempt_receipt(
            position,
            attempt_id,
            None,
            observed_intent,
            MoveAttemptOutcome::Inapplicable,
        )?;
        return Ok(GameDisposition::escalate(
            position,
            governed_escalation_prompt(board),
            Some(receipt.clone()),
            receipt.feedback_options().to_vec(),
        )?);
    }

    if top.candidate_id == semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
        || non_abstention
            .first()
            .is_none_or(|ranked| ranked.final_score < WEAK_EVIDENCE_FLOOR)
    {
        let receipt = attempt_receipt(
            position,
            attempt_id,
            None,
            observed_intent,
            MoveAttemptOutcome::Inapplicable,
        )?;
        if matches!(position.focus(), DesignFocus::Unknown { .. })
            && !receipt.feedback_options().is_empty()
        {
            return Ok(GameDisposition::change_focus_or_context(
                position,
                receipt.clone(),
                receipt.feedback_options().to_vec(),
            )?);
        }
        if attempts.last().is_some_and(|attempt| {
            matches!(
                attempt.outcome(),
                MoveAttemptOutcome::CompilerRefused
                    | MoveAttemptOutcome::Stale
                    | MoveAttemptOutcome::Inapplicable
                    | MoveAttemptOutcome::DisclosureSafeRefusal
                    | MoveAttemptOutcome::SystemFailure
            )
        }) {
            return Ok(GameDisposition::explain_attempt(position, receipt)?);
        }
        return Ok(GameDisposition::out_of_scope(position, receipt)?);
    }

    let top = non_abstention[0];
    let legal_move = position
        .legal_moves()
        .iter()
        .find(|legal_move| legal_move.move_id() == &top.move_id)
        .ok_or_else(|| anyhow::anyhow!("ranked move disappeared from the position"))?;
    let missing_arguments = match legal_move.binding_state() {
        MoveBindingState::Complete => Vec::new(),
        MoveBindingState::Incomplete {
            missing_arguments, ..
        } => missing_arguments
            .iter()
            .map(|argument| argument.as_str().to_string())
            .collect::<Vec<_>>(),
    };
    if top.final_score >= PROPOSAL_SCORE_FLOOR {
        if !missing_arguments.is_empty()
            && typed_argument_score(top.evidence) < ARGUMENT_EVIDENCE_FLOOR
        {
            let governed_prompt =
                governed_argument_prompt(board, &top.candidate_id, &missing_arguments)?;
            let receipt = attempt_receipt(
                position,
                attempt_id,
                Some(top.move_id.clone()),
                observed_intent,
                MoveAttemptOutcome::Incomplete,
            )?;
            return Ok(GameDisposition::request_move_arguments(
                position,
                top.move_id.clone(),
                missing_arguments,
                governed_prompt,
                receipt,
            )?);
        }
        return Ok(GameDisposition::propose_move(
            position,
            top.move_id.clone(),
        )?);
    }

    if let Some(clarification) = crate::clarification::select(board, position, evidence) {
        if clarification.information_gain >= CLARIFICATION_INFORMATION_GAIN_FLOOR {
            let receipt = attempt_receipt(
                position,
                attempt_id,
                None,
                observed_intent,
                MoveAttemptOutcome::Ambiguous,
            )?;
            return Ok(GameDisposition::clarify_moves(
                position,
                clarification.moves,
                clarification.dimension,
                clarification.governed_prompt,
                receipt,
            )?);
        }
    }

    let recovery = non_abstention
        .iter()
        .take(3)
        .map(|ranked| ranked.move_id.clone())
        .collect::<Vec<_>>();
    let receipt = attempt_receipt(
        position,
        attempt_id,
        Some(top.move_id.clone()),
        observed_intent,
        MoveAttemptOutcome::Inapplicable,
    )?;
    if recovery.is_empty() {
        Ok(GameDisposition::escalate(
            position,
            governed_escalation_prompt(board),
            Some(receipt.clone()),
            receipt.feedback_options().to_vec(),
        )?)
    } else {
        Ok(GameDisposition::offer_recovery_moves(
            position, recovery, receipt,
        )?)
    }
}

struct RankedMove<'a> {
    move_id: LegalMoveId,
    candidate_id: String,
    final_score: f64,
    probability: f64,
    evidence: &'a MoveEvidence,
}

fn validate_game_inputs(
    position: &DesignPosition,
    evidence: &[MoveEvidence],
    belief: &DesignBelief,
) -> anyhow::Result<()> {
    if belief.position_id() != position.state_id() {
        anyhow::bail!("game disposition belief belongs to a different position");
    }
    if evidence.len() != position.legal_moves().len() {
        anyhow::bail!("game disposition requires one evidence vector per legal move");
    }
    let expected = position
        .legal_moves()
        .iter()
        .map(|legal_move| legal_move.move_id())
        .collect::<std::collections::BTreeSet<_>>();
    let observed = evidence
        .iter()
        .map(MoveEvidence::move_id)
        .collect::<std::collections::BTreeSet<_>>();
    if expected != observed || observed.len() != evidence.len() {
        anyhow::bail!("game disposition evidence is incomplete, duplicated or off-board");
    }
    Ok(())
}

fn rank_moves<'a>(
    position: &DesignPosition,
    evidence: &'a [MoveEvidence],
    belief: &DesignBelief,
) -> Vec<RankedMove<'a>> {
    let belief = belief
        .likely_moves()
        .iter()
        .map(|likely| (likely.move_id(), likely.probability().get()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut ranked = evidence
        .iter()
        .filter_map(|item| {
            position
                .legal_moves()
                .iter()
                .find(|legal_move| legal_move.move_id() == item.move_id())
                .map(|legal_move| RankedMove {
                    move_id: item.move_id().clone(),
                    candidate_id: legal_move.candidate_id().as_str().to_string(),
                    final_score: item.final_score().get(),
                    probability: 0.75 * item.probability().get()
                        + 0.25 * belief.get(item.move_id()).copied().unwrap_or(0.0),
                    evidence: item,
                })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .probability
            .total_cmp(&left.probability)
            .then_with(|| right.final_score.total_cmp(&left.final_score))
            .then_with(|| left.move_id.cmp(&right.move_id))
    });
    ranked
}

fn typed_argument_score(evidence: &MoveEvidence) -> f64 {
    evidence
        .lanes()
        .iter()
        .find(|lane| lane.lane == EvidenceLane::TypedArgument)
        .map_or(0.0, |lane| lane.score.get())
}

fn attempt_receipt(
    position: &DesignPosition,
    attempt_id: MoveAttemptId,
    attempted_move: Option<LegalMoveId>,
    observed_intent: &str,
    outcome: MoveAttemptOutcome,
) -> anyhow::Result<MoveAttemptReceipt> {
    crate::history::receipt(
        position.semantic_snapshot().as_str(),
        attempt_id,
        position.state_id().clone(),
        attempted_move,
        observed_intent,
        outcome,
        None,
        None,
    )
}

fn compound_plan(
    board: &SemanticDecisionBoard,
    position: &DesignPosition,
    evidence: &[MoveEvidence],
    observed_intent: &str,
    resolved_chain: Option<&ResolvedChain>,
) -> Option<Vec<LegalMoveId>> {
    let spans = StrictCompoundSyntax.detect(observed_intent, board)?.spans;
    if let Some(chain) = resolved_chain {
        // G2.3: the caller (server layer -- see `ResolvedChain`'s doc
        // comment) already materialised span 1, compiler-admitted it, and
        // resolved span 2 against the position span 1 actually produces.
        // Trust it ONLY for these exact spans; a mismatched/stale chain
        // (e.g. left over from a different utterance) is never reused --
        // fall through to `None` (no compound plan at all) rather than
        // silently resolving span 2 against the wrong position.
        return (chain.spans == spans).then(|| chain.moves.clone());
    }
    // No pre-validated chain (e.g. a caller with no `DesignerDag` access,
    // or a test exercising this function directly): resolve every span
    // against the SAME unchanged `position`. Correct for span 1; for
    // span 2+ this is a known approximation (G2.3 receipt) -- it can only
    // ever find span 2 legal when it happens to ALSO be legal before
    // span 1 is applied, never when span 1 is what makes it legal, and it
    // cannot catch span 2 becoming illegal because of span 1.
    let probabilities = evidence
        .iter()
        .map(|item| (item.move_id(), item.probability().get()))
        .collect::<std::collections::BTreeMap<_, _>>();
    spans
        .iter()
        .map(|span| match governed_exact(board, span) {
            ExactMatch::Unique(candidate) => position
                .legal_moves()
                .iter()
                .filter(|legal_move| legal_move.candidate_id().as_str() == candidate)
                .max_by(|left, right| {
                    probabilities
                        .get(left.move_id())
                        .copied()
                        .unwrap_or(0.0)
                        .total_cmp(&probabilities.get(right.move_id()).copied().unwrap_or(0.0))
                        .then_with(|| right.move_id().cmp(left.move_id()))
                })
                .map(|legal_move| legal_move.move_id().clone()),
            ExactMatch::None | ExactMatch::Collision(_) => None,
        })
        .collect()
}

fn governed_argument_prompt(
    board: &SemanticDecisionBoard,
    candidate_id: &str,
    missing: &[String],
) -> anyhow::Result<String> {
    let candidate = board
        .candidate(candidate_id)
        .ok_or_else(|| anyhow::anyhow!("selected candidate is absent from the semantic board"))?;
    let prompts = missing
        .iter()
        .map(|name| {
            candidate
                .arguments
                .iter()
                .find(|argument| &argument.name == name)
                .map(|argument| argument.clarification_prompt.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing argument has no governed prompt"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(prompts.join(" "))
}

fn governed_compound_prompt(board: &SemanticDecisionBoard, observed_intent: &str) -> String {
    StrictCompoundSyntax
        .detect(observed_intent, board)
        .into_iter()
        .flat_map(|evidence| evidence.spans)
        .filter_map(|span| match governed_exact(board, &span) {
            ExactMatch::Unique(candidate) => {
                board.candidate(&candidate).map(|item| item.effect.clone())
            }
            ExactMatch::None | ExactMatch::Collision(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn governed_escalation_prompt(board: &SemanticDecisionBoard) -> String {
    board
        .candidates
        .iter()
        .find(|candidate| {
            candidate.canonical_id.as_str() != semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
        })
        .map_or_else(
            || board.domain.as_str().to_string(),
            |candidate| candidate.applicability.clone(),
        )
}
