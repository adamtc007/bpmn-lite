//! Candidate-conditioned pair serialization shared by serving and corpora.

use sem_os_policy::decision_board::CandidateSemanticSlice;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::context::ContextProjection;

pub const PAIR_SERIALIZER_ID: &str = "bpmn.candidate-pair.v1";
pub const TURN_SERIALIZER_ID: &str = "bpmn.turn.ctxproj.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairTokenBudget {
    pub side_a_tokens: usize,
    pub side_b_tokens: usize,
}

impl Default for PairTokenBudget {
    fn default() -> Self {
        Self {
            side_a_tokens: 128,
            side_b_tokens: 128,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerializedCandidatePair {
    pub serializer_id: String,
    pub side_a: String,
    pub side_a_hash: String,
    pub side_b: String,
    pub side_b_hash: String,
    pub pair_hash: String,
    pub budget: PairTokenBudget,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SerializationError {
    #[error("pair token budgets must reserve at least {minimum} tokens per side")]
    BudgetTooSmall { minimum: usize },
}

/// Construct the one bounded utterance/turn × semantic-candidate pair.
///
/// Budgets are independently applied before tokenizer pair encoding. Each side
/// retains its opening and closing sentinels; side B also retains the effect
/// and contrast sentinels at its suffix even when the turn is pathological.
pub fn serialize_candidate_pair(
    utterance: &str,
    turn: &ContextProjection,
    candidate: &CandidateSemanticSlice,
    budget: PairTokenBudget,
) -> Result<SerializedCandidatePair, SerializationError> {
    const MINIMUM: usize = 16;
    if budget.side_a_tokens < MINIMUM || budget.side_b_tokens < MINIMUM {
        return Err(SerializationError::BudgetTooSmall { minimum: MINIMUM });
    }
    let side_a_raw = format!(
        "[UTTERANCE] {utterance} [TURN] {} [END_TURN]",
        turn.serialize_canonical()
    );
    let contrasts = candidate
        .negative_contrasts
        .iter()
        .map(|contrast| {
            format!(
                "{}: {}",
                contrast.candidate_id.as_str(),
                contrast.distinction
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let side_b_prefix = format!(
        "[CANDIDATE] {} [SEMANTICS] {}",
        candidate.canonical_id.as_str(),
        crate::exact::serialize_candidate(candidate)
    );
    let side_b_suffix = format!(
        "[EFFECT] {} [CONTRASTS] {} [END_CANDIDATE]",
        first_tokens(&candidate.effect, 4),
        first_tokens(&contrasts, 4),
    );
    let side_a = bounded_edges(&side_a_raw, budget.side_a_tokens, 3);
    let suffix_tokens = side_b_suffix.split_whitespace().count();
    let prefix_budget = budget.side_b_tokens - suffix_tokens;
    let side_b = format!(
        "{} {}",
        first_tokens(&side_b_prefix, prefix_budget),
        side_b_suffix
    );
    let side_a_hash = hash(&side_a);
    let side_b_hash = hash(&side_b);
    let pair_hash = hash(&format!(
        "{PAIR_SERIALIZER_ID}:{}:{side_a_hash}:{}:{side_b_hash}",
        budget.side_a_tokens, budget.side_b_tokens
    ));
    Ok(SerializedCandidatePair {
        serializer_id: PAIR_SERIALIZER_ID.to_string(),
        side_a,
        side_a_hash,
        side_b,
        side_b_hash,
        pair_hash,
        budget,
    })
}

pub fn pair_serializer_hash() -> String {
    hash(PAIR_SERIALIZER_ID)
}

pub fn turn_serializer_hash() -> String {
    hash(TURN_SERIALIZER_ID)
}

fn bounded_edges(value: &str, budget: usize, suffix_reserve: usize) -> String {
    let tokens = value.split_whitespace().collect::<Vec<_>>();
    if tokens.len() <= budget {
        return tokens.join(" ");
    }
    let suffix = suffix_reserve.min(budget / 2).min(tokens.len());
    let prefix = budget - suffix - 1;
    let mut selected = tokens[..prefix].to_vec();
    selected.push("[TRUNCATED]");
    selected.extend_from_slice(&tokens[tokens.len() - suffix..]);
    selected.join(" ")
}

fn first_tokens(value: &str, budget: usize) -> String {
    value
        .split_whitespace()
        .take(budget)
        .collect::<Vec<_>>()
        .join(" ")
}

fn hash(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use sem_os_ontology::verb_contract::{ActionClass, HarmClass};
    use sem_os_policy::decision_board::{
        CanonicalCandidateId, NegativeContrast,
    };

    use super::*;

    fn candidate() -> CandidateSemanticSlice {
        CandidateSemanticSlice {
            canonical_id: CanonicalCandidateId::new("op.insert_after").unwrap(),
            schema_version: 1,
            title: "Insert after".into(),
            intent_summary: "insert a node after an anchor".into(),
            action_class: ActionClass::Create,
            applicability: "normal flow".into(),
            effect: "EFFECT_SENTINEL rewires the existing route".into(),
            arguments: vec![],
            phrases: vec![],
            positive_examples: vec![],
            negative_contrasts: vec![NegativeContrast {
                candidate_id: CanonicalCandidateId::new("op.append_node").unwrap(),
                distinction: "CONTRAST_SENTINEL append extends an open route".into(),
            }],
            risk: HarmClass::Reversible,
            adapter_payload_hash: "payload".into(),
        }
    }

    #[test]
    fn long_turn_cannot_evict_candidate_effect_or_contrast() {
        let long_identity = (0..2_000).map(|_| "context").collect::<Vec<_>>().join("-");
        let turn = ContextProjection::new("pack", long_identity, None, vec![]).unwrap();
        let pair = serialize_candidate_pair(
            "insert something",
            &turn,
            &candidate(),
            PairTokenBudget {
                side_a_tokens: 24,
                side_b_tokens: 24,
            },
        )
        .unwrap();
        assert!(pair.side_a.contains("[END_TURN]"));
        assert!(pair.side_b.contains("[EFFECT]"));
        assert!(pair.side_b.contains("EFFECT_SENTINEL"));
        assert!(pair.side_b.contains("[CONTRASTS]"));
        assert!(pair.side_b.contains("CONTRAST_SENTINEL"));
        assert!(pair.side_b.contains("[END_CANDIDATE]"));
    }

    #[test]
    fn pair_hash_is_deterministic_and_budget_bound() {
        let turn = ContextProjection::new("pack", "graph", None, vec![]).unwrap();
        let budget = PairTokenBudget::default();
        let first = serialize_candidate_pair("insert", &turn, &candidate(), budget).unwrap();
        let second = serialize_candidate_pair("insert", &turn, &candidate(), budget).unwrap();
        assert_eq!(first, second);
        assert!(first.side_a.split_whitespace().count() <= budget.side_a_tokens);
        assert!(first.side_b.split_whitespace().count() <= budget.side_b_tokens);
    }
}
