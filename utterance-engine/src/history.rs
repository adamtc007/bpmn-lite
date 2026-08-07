//! Private bounded projection of append-only gameboard attempts.

use semantic_decision_contracts::{
    validate_attempt_history, CorrectionKind, FeedbackOption, GraphContentHash, HistoryHash,
    LegalMoveId, MoveAttemptId, MoveAttemptOutcome, MoveAttemptReceipt, RuleExplanation,
    GAMEBOARD_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

use crate::bpmn_pack::{
    feedback_source, rule_source, APPLICABILITY_RULE_CODE, ARGUMENT_RULE_CODE,
    COMPILER_REFUSAL_RULE_CODE, EVIDENCE_RULE_CODE, POLICY_HIDDEN_RULE_CODE,
};

pub(super) const MAX_HISTORY_ATTEMPTS: usize = 64;
pub(super) const MAX_HISTORY_BYTES: usize = 64 * 1024;

pub(super) struct HistoryProjection {
    attempts: Vec<MoveAttemptReceipt>,
    hash: HistoryHash,
}

impl HistoryProjection {
    pub(super) fn attempts(&self) -> &[MoveAttemptReceipt] {
        &self.attempts
    }

    pub(super) fn hash(&self) -> &HistoryHash {
        &self.hash
    }
}

pub(super) fn project(attempts: &[MoveAttemptReceipt]) -> anyhow::Result<HistoryProjection> {
    if attempts.len() > MAX_HISTORY_ATTEMPTS {
        anyhow::bail!(
            "history projection exceeds the product limit of {MAX_HISTORY_ATTEMPTS} attempts"
        );
    }
    validate_attempt_history(attempts)?;
    let canonical = serde_json::to_vec(attempts)?;
    if canonical.len() > MAX_HISTORY_BYTES {
        anyhow::bail!("history projection exceeds the product limit of {MAX_HISTORY_BYTES} bytes");
    }
    let mut hasher = Sha256::new();
    hasher.update(b"bpmn-lite-gameboard-history-v1");
    hasher.update((canonical.len() as u64).to_be_bytes());
    hasher.update(&canonical);
    Ok(HistoryProjection {
        attempts: attempts.to_vec(),
        hash: HistoryHash::new(format!("{:x}", hasher.finalize()))?,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn receipt(
    semantic_snapshot: &str,
    attempt_id: MoveAttemptId,
    position_id: semantic_decision_contracts::DesignStateId,
    attempted_move: Option<LegalMoveId>,
    observed_intent: &str,
    outcome: MoveAttemptOutcome,
    correction_of: Option<MoveAttemptId>,
    correction_kind: Option<CorrectionKind>,
) -> anyhow::Result<MoveAttemptReceipt> {
    let rule_code = match outcome {
        MoveAttemptOutcome::Incomplete => Some(ARGUMENT_RULE_CODE),
        MoveAttemptOutcome::Ambiguous | MoveAttemptOutcome::Inapplicable => {
            Some(APPLICABILITY_RULE_CODE)
        }
        MoveAttemptOutcome::DisclosureSafeRefusal => Some(POLICY_HIDDEN_RULE_CODE),
        MoveAttemptOutcome::Stale
        | MoveAttemptOutcome::CompilerRefused
        | MoveAttemptOutcome::SystemFailure => Some(COMPILER_REFUSAL_RULE_CODE),
        MoveAttemptOutcome::RejectedByUser | MoveAttemptOutcome::Corrected => {
            Some(EVIDENCE_RULE_CODE)
        }
        MoveAttemptOutcome::Applied => None,
    };
    let mut explanations = Vec::new();
    let mut feedback = Vec::new();
    if let Some(rule_code) = rule_code {
        let source = rule_source(rule_code);
        let explanation = RuleExplanation::new(
            GAMEBOARD_SCHEMA_VERSION,
            source.rule_code.clone(),
            source.message_key.clone(),
            Vec::new(),
            semantic_snapshot,
            source.disclosure,
        )?;
        for option_id in &source.feedback_options {
            let source = feedback_source(option_id.as_str());
            feedback.push(FeedbackOption::new(
                source.kind,
                None,
                source.prompt_key.clone(),
                Some(explanation.explanation_id().clone()),
                source.disclosure,
            ));
        }
        explanations.push(explanation.explanation_id().clone());
    }
    let mut intent_hasher = Sha256::new();
    intent_hasher.update(b"bpmn-lite-observed-intent-v1");
    intent_hasher.update((observed_intent.len() as u64).to_be_bytes());
    intent_hasher.update(observed_intent.as_bytes());
    Ok(MoveAttemptReceipt::new(
        GAMEBOARD_SCHEMA_VERSION,
        attempt_id,
        position_id,
        attempted_move,
        GraphContentHash::new(format!("{:x}", intent_hasher.finalize()))?,
        outcome,
        explanations,
        feedback,
        correction_of,
        correction_kind,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use semantic_decision_contracts::{DesignStateId, LegalMoveId};

    fn attempt(index: usize, correction: Option<usize>) -> MoveAttemptReceipt {
        receipt(
            "snapshot",
            MoveAttemptId::new(format!("attempt-{index}")).unwrap(),
            DesignStateId::new("a".repeat(64)).unwrap(),
            Some(LegalMoveId::new(format!("{index:064x}")).unwrap()),
            "test input",
            if correction.is_some() {
                MoveAttemptOutcome::Corrected
            } else {
                MoveAttemptOutcome::RejectedByUser
            },
            correction.map(|target| MoveAttemptId::new(format!("attempt-{target}")).unwrap()),
            correction.map(|_| CorrectionKind::FollowUp),
        )
        .unwrap()
    }

    #[test]
    fn projection_is_canonical_bounded_and_keeps_corrections() {
        let attempts = vec![attempt(0, None), attempt(1, Some(0))];
        let first = project(&attempts).unwrap();
        let second = project(&attempts).unwrap();
        assert_eq!(first.hash(), second.hash());
        assert_eq!(first.attempts(), attempts);

        let over_limit = (0..=MAX_HISTORY_ATTEMPTS)
            .map(|index| attempt(index, None))
            .collect::<Vec<_>>();
        assert!(project(&over_limit).is_err());
    }

    #[test]
    fn missing_or_cyclic_correction_targets_fail_closed() {
        assert!(project(&[attempt(1, Some(0))]).is_err());
    }
}
