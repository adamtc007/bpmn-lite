//! Strict action-span boundary for atomic BPMN inference.
//!
//! V1 does not execute compound utterances. This interface reserves the
//! producer seam, while the only implementation deliberately recognizes the
//! high-precision form `<governed exact phrase>; <governed exact phrase>`.

use crate::exact::{governed_exact, ExactMatch};
use sem_os_policy::decision_board::SemanticDecisionBoard;

pub const NO_ACTION_SPAN_PRODUCER_ID: &str = "bpmn.action-span.none.v1";
pub const STRICT_ACTION_SPAN_PRODUCER_ID: &str =
    "bpmn.action-span.strict-semicolon-governed-exact.v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionSpanEvidence {
    pub spans: Vec<String>,
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
