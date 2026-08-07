//! Private information-gain selection over admitted, position-bound moves.

use std::collections::BTreeMap;

use semantic_decision_contracts::{
    DesignPosition, GameClarificationDimension, LegalMove, LegalMoveId, MoveBindingState,
    MoveEvidence, SemanticDecisionBoard,
};

const MAX_ALTERNATIVES: usize = 3;

pub(super) struct Clarification {
    pub(super) moves: Vec<LegalMoveId>,
    pub(super) dimension: GameClarificationDimension,
    pub(super) governed_prompt: String,
    pub(super) information_gain: f64,
}

pub(super) fn select(
    board: &SemanticDecisionBoard,
    position: &DesignPosition,
    evidence: &[MoveEvidence],
) -> Option<Clarification> {
    let probabilities = evidence
        .iter()
        .map(|item| (item.move_id(), item.probability().get()))
        .collect::<BTreeMap<_, _>>();
    let mut alternatives = position
        .legal_moves()
        .iter()
        .filter(|legal_move| {
            legal_move.candidate_id().as_str()
                != semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
        })
        .filter_map(|legal_move| {
            probabilities
                .get(legal_move.move_id())
                .copied()
                .filter(|probability| *probability > 0.0)
                .map(|probability| (legal_move, probability))
        })
        .collect::<Vec<_>>();
    alternatives.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.move_id().cmp(right.0.move_id()))
    });
    alternatives.truncate(MAX_ALTERNATIVES);
    if alternatives.len() < 2 {
        return None;
    }

    let candidates = [
        move_question(board, &alternatives),
        focus_question(board, &alternatives),
        argument_question(board, &alternatives),
    ];
    candidates.into_iter().flatten().max_by(|left, right| {
        left.information_gain
            .total_cmp(&right.information_gain)
            .then_with(|| dimension_order(left.dimension).cmp(&dimension_order(right.dimension)))
    })
}

fn dimension_order(dimension: GameClarificationDimension) -> u8 {
    match dimension {
        GameClarificationDimension::Move => 3,
        GameClarificationDimension::Focus => 2,
        GameClarificationDimension::Argument => 1,
    }
}

fn information_gain<K: Ord>(
    alternatives: &[(&LegalMove, f64)],
    key: impl Fn(&LegalMove) -> K,
) -> f64 {
    let total = alternatives.iter().map(|(_, value)| *value).sum::<f64>();
    if total <= 0.0 {
        return 0.0;
    }
    let entropy = alternatives
        .iter()
        .map(|(_, value)| *value / total)
        .filter(|value| *value > 0.0)
        .map(|value| -value * value.log2())
        .sum::<f64>();
    let groups = alternatives.iter().fold(
        BTreeMap::<K, Vec<f64>>::new(),
        |mut groups, (legal_move, value)| {
            groups
                .entry(key(legal_move))
                .or_default()
                .push(*value / total);
            groups
        },
    );
    let conditional = groups
        .values()
        .map(|group| {
            let mass = group.iter().sum::<f64>();
            if mass <= 0.0 {
                return 0.0;
            }
            mass * group
                .iter()
                .map(|value| *value / mass)
                .filter(|value| *value > 0.0)
                .map(|value| -value * value.log2())
                .sum::<f64>()
        })
        .sum::<f64>();
    (entropy - conditional).max(0.0)
}

fn selected_moves(alternatives: &[(&LegalMove, f64)]) -> Vec<LegalMoveId> {
    alternatives
        .iter()
        .map(|(legal_move, _)| legal_move.move_id().clone())
        .collect()
}

fn move_question(
    board: &SemanticDecisionBoard,
    alternatives: &[(&LegalMove, f64)],
) -> Option<Clarification> {
    let candidates = alternatives
        .iter()
        .map(|(legal_move, _)| legal_move.candidate_id().as_str())
        .collect::<Vec<_>>();
    if candidates.windows(2).any(|pair| pair[0] == pair[1]) {
        return None;
    }
    let mut distinctions = Vec::new();
    for (index, candidate_id) in candidates.iter().enumerate() {
        let candidate = board.candidate(candidate_id)?;
        for other in candidates.iter().skip(index + 1) {
            let forward = candidate
                .negative_contrasts
                .iter()
                .find(|contrast| contrast.candidate_id.as_str() == *other)?;
            let reverse = board
                .candidate(other)?
                .negative_contrasts
                .iter()
                .find(|contrast| contrast.candidate_id.as_str() == *candidate_id)?;
            distinctions.push(forward.distinction.as_str());
            distinctions.push(reverse.distinction.as_str());
        }
    }
    distinctions.sort_unstable();
    distinctions.dedup();
    Some(Clarification {
        moves: selected_moves(alternatives),
        dimension: GameClarificationDimension::Move,
        governed_prompt: distinctions.join(" | "),
        information_gain: information_gain(alternatives, |legal_move| {
            legal_move.candidate_id().as_str().to_string()
        }),
    })
}

fn focus_question(
    board: &SemanticDecisionBoard,
    alternatives: &[(&LegalMove, f64)],
) -> Option<Clarification> {
    let anchors = alternatives
        .iter()
        .map(|(legal_move, _)| legal_move.anchor().map(|anchor| anchor.as_str()))
        .collect::<Vec<_>>();
    if anchors.iter().any(|anchor| anchor.is_none())
        || anchors.windows(2).all(|pair| pair[0] == pair[1])
    {
        return None;
    }
    let mut applicability = alternatives
        .iter()
        .filter_map(|(legal_move, _)| board.candidate(legal_move.candidate_id().as_str()))
        .map(|candidate| candidate.applicability.as_str())
        .collect::<Vec<_>>();
    applicability.sort_unstable();
    applicability.dedup();
    let governed_prompt = applicability.join(" | ");
    if governed_prompt.is_empty() {
        return None;
    }
    Some(Clarification {
        moves: selected_moves(alternatives),
        dimension: GameClarificationDimension::Focus,
        governed_prompt,
        information_gain: information_gain(alternatives, |legal_move| {
            legal_move
                .anchor()
                .map_or_else(String::new, |anchor| anchor.as_str().to_string())
        }),
    })
}

fn argument_question(
    board: &SemanticDecisionBoard,
    alternatives: &[(&LegalMove, f64)],
) -> Option<Clarification> {
    let missing = alternatives
        .iter()
        .map(|(legal_move, _)| match legal_move.binding_state() {
            MoveBindingState::Complete => Vec::new(),
            MoveBindingState::Incomplete {
                missing_arguments, ..
            } => missing_arguments
                .iter()
                .map(|argument| argument.as_str().to_string())
                .collect::<Vec<_>>(),
        })
        .collect::<Vec<_>>();
    if missing.iter().all(Vec::is_empty) || missing.windows(2).all(|pair| pair[0] == pair[1]) {
        return None;
    }
    let first_name = missing.iter().flatten().next()?;
    let governed_prompt = alternatives
        .iter()
        .filter_map(|(legal_move, _)| board.candidate(legal_move.candidate_id().as_str()))
        .flat_map(|candidate| candidate.arguments.iter())
        .find(|argument| &argument.name == first_name)?
        .clarification_prompt
        .clone();
    Some(Clarification {
        moves: selected_moves(alternatives),
        dimension: GameClarificationDimension::Argument,
        governed_prompt,
        information_gain: information_gain(alternatives, |legal_move| {
            match legal_move.binding_state() {
                MoveBindingState::Complete => String::new(),
                MoveBindingState::Incomplete {
                    missing_arguments, ..
                } => missing_arguments
                    .iter()
                    .map(|argument| argument.as_str())
                    .collect::<Vec<_>>()
                    .join("\0"),
            }
        }),
    })
}
