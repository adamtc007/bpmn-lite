//! Private bounded projection of append-only gameboard attempts.

use semantic_decision_contracts::{
    validate_attempt_history, CorrectionKind, FeedbackOption, GraphContentHash, HistoryHash,
    LegalMoveId, MoveAttemptId, MoveAttemptOutcome, MoveAttemptReceipt, RuleExplanation,
    GAMEBOARD_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

use crate::bpmn_board::{BpmnBoardError, ResourceLimitExceeded};
use crate::bpmn_pack::{
    feedback_source, rule_source, APPLICABILITY_RULE_CODE, ARGUMENT_RULE_CODE,
    COMPILER_REFUSAL_RULE_CODE, EVIDENCE_RULE_CODE, POLICY_HIDDEN_RULE_CODE,
};

/// Public so fuzz targets (a separate crate, seeing only the public API) can
/// drive a tape past this bound deliberately instead of guessing/hardcoding
/// it — see `utterance-engine/fuzz/fuzz_targets/correction_history.rs`.
pub const MAX_HISTORY_ATTEMPTS: usize = 64;
pub const MAX_HISTORY_BYTES: usize = 64 * 1024;

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

pub(super) fn project(attempts: &[MoveAttemptReceipt]) -> Result<HistoryProjection, BpmnBoardError> {
    if attempts.len() > MAX_HISTORY_ATTEMPTS {
        return Err(ResourceLimitExceeded {
            field: "history projection attempts",
            limit: MAX_HISTORY_ATTEMPTS,
            actual: attempts.len(),
        }
        .into());
    }
    validate_attempt_history(attempts)?;
    let canonical = serde_json::to_vec(attempts)
        .map_err(|error| BpmnBoardError::Continuity(error.to_string()))?;
    if canonical.len() > MAX_HISTORY_BYTES {
        return Err(ResourceLimitExceeded {
            field: "history projection bytes",
            limit: MAX_HISTORY_BYTES,
            actual: canonical.len(),
        }
        .into());
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
        let error = match project(&over_limit) {
            Ok(_) => panic!("attempts beyond MAX_HISTORY_ATTEMPTS must be refused"),
            Err(error) => error,
        };
        assert!(matches!(
            &error,
            BpmnBoardError::ResourceLimit(limit)
                if limit.field == "history projection attempts" && limit.limit == MAX_HISTORY_ATTEMPTS
        ));
        // Session usable afterwards: a bounded, legitimate window still projects.
        assert!(project(&attempts).is_ok());
    }

    #[test]
    fn oversized_history_bytes_is_a_typed_resource_limit_refusal() {
        // receipt() (the only production constructor in this module) derives
        // rule/feedback content from a tiny fixed catalogue keyed by outcome,
        // so a realistic receipt never approaches MAX_HISTORY_BYTES on its
        // own — MAX_HISTORY_ATTEMPTS binds first in practice. project() is a
        // general-purpose bound on any &[MoveAttemptReceipt] though, so
        // construct receipts directly through the contract constructor with
        // large rule-explanation/feedback vectors to reach the byte ceiling
        // with an attempt count well under MAX_HISTORY_ATTEMPTS, proving the
        // byte guard is a real, independently reachable code path.
        fn heavy_attempt(tag: usize) -> MoveAttemptReceipt {
            let explanations = (0..300)
                .map(|index| {
                    semantic_decision_contracts::RuleExplanationId::new(format!(
                        "{:064x}",
                        tag * 10_000 + index
                    ))
                    .unwrap()
                })
                .collect::<Vec<_>>();
            let feedback = explanations
                .iter()
                .enumerate()
                .map(|(index, explanation_id)| {
                    semantic_decision_contracts::FeedbackOption::new(
                        semantic_decision_contracts::FeedbackOptionKind::Retry,
                        None,
                        semantic_decision_contracts::MessageKey::new(format!(
                            "feedback.{tag}.{index}"
                        ))
                        .unwrap(),
                        Some(explanation_id.clone()),
                        semantic_decision_contracts::DisclosureClass::Public,
                    )
                })
                .collect::<Vec<_>>();
            MoveAttemptReceipt::new(
                GAMEBOARD_SCHEMA_VERSION,
                MoveAttemptId::new(format!("heavy-attempt-{tag}")).unwrap(),
                DesignStateId::new("a".repeat(64)).unwrap(),
                None,
                GraphContentHash::new("b".repeat(64)).unwrap(),
                MoveAttemptOutcome::Incomplete,
                explanations,
                feedback,
                None,
                None,
            )
            .unwrap()
        }

        let heavy = (0..3).map(heavy_attempt).collect::<Vec<_>>();
        assert!(
            heavy.len() < MAX_HISTORY_ATTEMPTS,
            "must stay under the count limit so the byte limit is what trips"
        );
        let error = match project(&heavy) {
            Ok(_) => panic!(
                "expected a byte-limit refusal; the fixture serializes to {} bytes",
                serde_json::to_vec(&heavy).unwrap().len()
            ),
            Err(error) => error,
        };
        assert!(matches!(
            &error,
            BpmnBoardError::ResourceLimit(limit)
                if limit.field == "history projection bytes" && limit.limit == MAX_HISTORY_BYTES
        ));

        // Session usable afterwards: a small, legitimate window still projects.
        assert!(project(&[attempt(0, None)]).is_ok());
    }

    #[test]
    fn missing_or_cyclic_correction_targets_fail_closed() {
        assert!(project(&[attempt(1, Some(0))]).is_err());
    }
}
