//! Private graph-position evidence derived only from the admitted position.

use semantic_decision_contracts::{DesignFocus, DesignPosition, LegalMove};

pub(super) fn locality(position: &DesignPosition, legal_move: &LegalMove, utterance: &str) -> f64 {
    let Some(anchor) = legal_move.anchor() else {
        return 0.0;
    };
    let explicit = utterance
        .to_lowercase()
        .contains(&anchor.as_str().to_lowercase());
    let focused = match position.focus() {
        DesignFocus::Element { element } => element == anchor,
        DesignFocus::Subgraph { elements } => elements.contains(anchor),
        DesignFocus::Absent { .. } | DesignFocus::Unknown { .. } => false,
    };
    match (explicit, focused) {
        (true, true) => 1.0,
        (true, false) => 0.9,
        (false, true) => 0.7,
        (false, false) => 0.0,
    }
}

pub(super) fn structural_completion(legal_move: &LegalMove) -> f64 {
    if legal_move.candidate_id().as_str() == semantic_decision_contracts::ABSTENTION_CANDIDATE_ID {
        0.0
    } else if legal_move.binding_state().is_complete() && legal_move.preview().is_some() {
        1.0
    } else if legal_move.binding_state().is_complete() {
        0.75
    } else {
        0.25
    }
}
