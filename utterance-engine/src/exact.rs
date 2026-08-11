//! Versioned semantic candidate serialization and governed exact evidence.

use std::collections::BTreeSet;

pub use semantic_decision_contracts::EvidenceLane;
use semantic_decision_contracts::{CandidateSemanticSlice, SemanticDecisionBoard};
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::contract::{rank_canonically, EvidenceTrace, FiniteScore, SlmResult};

/// Identity of the only candidate textualization used by BPMN retrieval.
pub const CANDIDATE_SERIALIZER_ID: &str = "bpmn.candidate-semantic-json.v1";
/// Identity of the governed exact normalizer and collision policy.
pub const GOVERNED_EXACT_BUNDLE_ID: &str = "bpmn.governed-exact.nfkc-lowercase-whitespace.v1";

#[derive(Serialize)]
struct CandidateModelText<'a> {
    schema: &'static str,
    canonical_id: &'a str,
    title: &'a str,
    intent_summary: &'a str,
    applicability: &'a str,
    effect: &'a str,
    arguments: Vec<ArgumentText<'a>>,
    phrases: Vec<PhraseText<'a>>,
    positive_examples: &'a [String],
    negative_contrasts: Vec<ContrastText<'a>>,
}

#[derive(Serialize)]
struct ArgumentText<'a> {
    name: &'a str,
    kind: String,
}

#[derive(Serialize)]
struct PhraseText<'a> {
    role: String,
    text: &'a str,
}

#[derive(Serialize)]
struct ContrastText<'a> {
    candidate_id: &'a str,
    distinction: &'a str,
}

/// Serialize every retrieval-relevant semantic field exactly once.
pub fn serialize_candidate(candidate: &CandidateSemanticSlice) -> String {
    let value = CandidateModelText {
        schema: CANDIDATE_SERIALIZER_ID,
        canonical_id: candidate.canonical_id.as_str(),
        title: &candidate.title,
        intent_summary: &candidate.intent_summary,
        applicability: &candidate.applicability,
        effect: &candidate.effect,
        arguments: candidate
            .arguments
            .iter()
            .map(|argument| ArgumentText {
                name: &argument.name,
                kind: format!("{:?}", argument.kind),
            })
            .collect(),
        phrases: candidate
            .phrases
            .iter()
            .map(|phrase| PhraseText {
                role: format!("{:?}", phrase.role),
                text: &phrase.text,
            })
            .collect(),
        positive_examples: &candidate.positive_examples,
        negative_contrasts: candidate
            .negative_contrasts
            .iter()
            .map(|contrast| ContrastText {
                candidate_id: contrast.candidate_id.as_str(),
                distinction: &contrast.distinction,
            })
            .collect(),
    };
    serde_json::to_string(&value).expect("candidate model text is JSON serializable")
}

/// Content identity for the serializer contract, independent of a candidate.
pub fn candidate_serializer_hash() -> String {
    blake3::hash(CANDIDATE_SERIALIZER_ID.as_bytes())
        .to_hex()
        .to_string()
}

/// Exact-lane result. A collision is evidence, never authority to choose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub enum ExactMatch {
    None,
    Unique(String),
    Collision(Vec<String>),
}

/// Stable corpus representation of the governed exact result.
pub type ExactMatchDump = ExactMatch;

/// Normalize governed phrases using Unicode NFKC, Unicode lowercase and
/// canonical whitespace. Punctuation is intentionally not fuzzily discarded.
pub fn normalize_phrase(value: &str) -> String {
    value
        .nfkc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Match only phrases visible on the current semantic board.
pub fn governed_exact(board: &SemanticDecisionBoard, utterance: &str) -> ExactMatch {
    let normalized = normalize_phrase(utterance);
    if normalized.is_empty() {
        return ExactMatch::None;
    }
    let matches = board
        .candidates
        .iter()
        .filter(|candidate| {
            candidate
                .phrases
                .iter()
                .any(|phrase| normalize_phrase(&phrase.text) == normalized)
        })
        .map(|candidate| candidate.canonical_id.as_str().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => ExactMatch::None,
        [candidate] => ExactMatch::Unique(candidate.clone()),
        _ => ExactMatch::Collision(matches),
    }
}

/// Close model evidence over the complete semantic board and apply governed
/// exact evidence without allowing a collision to choose a candidate.
pub fn finalize_semantic_evidence(
    board: &SemanticDecisionBoard,
    utterance: &str,
    mut result: SlmResult,
    mut lanes: Vec<EvidenceLane>,
    mut bundle_identities: Vec<String>,
) -> anyhow::Result<SlmResult> {
    let board_ids = board
        .candidates
        .iter()
        .map(|candidate| candidate.canonical_id.as_str().to_string())
        .collect::<BTreeSet<_>>();
    let result_ids = result
        .ranking
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect::<BTreeSet<_>>();
    if result.ranking.len() != result_ids.len() || result_ids != board_ids {
        anyhow::bail!("semantic serving evidence must contain every board candidate exactly once");
    }

    let exact = governed_exact(board, utterance);
    let exact_ids = match &exact {
        ExactMatch::None => BTreeSet::new(),
        ExactMatch::Unique(candidate) => BTreeSet::from([candidate.clone()]),
        ExactMatch::Collision(candidates) => candidates.iter().cloned().collect(),
    };
    if !exact_ids.is_empty() {
        lanes.push(EvidenceLane::GovernedExact);
        bundle_identities.push(GOVERNED_EXACT_BUNDLE_ID.to_string());
        for candidate in &mut result.ranking {
            let score = if exact_ids.contains(&candidate.candidate_id) {
                1.0
            } else {
                candidate.score.get().min(0.99)
            };
            candidate.score = FiniteScore::new(score)?;
        }
        rank_canonically(&mut result.ranking);
    }
    lanes.sort();
    lanes.dedup();
    bundle_identities.sort();
    bundle_identities.dedup();
    result.evidence_trace = Some(EvidenceTrace {
        candidate_serializer_hash: candidate_serializer_hash(),
        turn_serializer_hash: crate::pair::turn_serializer_hash(),
        lanes,
        bundle_identities,
        exact_collision: match exact {
            ExactMatch::Collision(candidates) => candidates,
            ExactMatch::None | ExactMatch::Unique(_) => Vec::new(),
        },
        served_full_board: true,
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use semantic_decision_contracts::{
        ActionClass, CanonicalCandidateId, DomainIdentity, GraphRevision, HarmClass,
        PhraseEvidence, PhraseRole, ResolvedPosition, SnapshotIdentity,
    };

    use super::*;
    use crate::contract::{RankedCandidate, SlmResult};

    fn candidate(id: &str, phrase: &str) -> CandidateSemanticSlice {
        CandidateSemanticSlice {
            canonical_id: CanonicalCandidateId::new(id).unwrap(),
            schema_version: 1,
            title: id.to_string(),
            intent_summary: format!("intent {id}"),
            action_class: ActionClass::Create,
            applicability: "when legal".into(),
            effect: "changes the graph".into(),
            arguments: vec![],
            phrases: vec![PhraseEvidence {
                text: phrase.into(),
                locale: "en-GB".into(),
                role: PhraseRole::Canonical,
                provenance: "test".into(),
            }],
            positive_examples: vec![format!("example {id}")],
            negative_contrasts: vec![],
            risk: HarmClass::Reversible,
            adapter_payload_hash: format!("payload-{id}"),
        }
    }

    fn board(candidates: Vec<CandidateSemanticSlice>) -> SemanticDecisionBoard {
        SemanticDecisionBoard::new(
            1,
            DomainIdentity::new("bpmn.test").unwrap(),
            SnapshotIdentity::new("snapshot").unwrap(),
            GraphRevision::new("revision").unwrap(),
            ResolvedPosition {
                anchor: None,
                context_hash: "context".into(),
            },
            candidates,
            "policy".into(),
        )
        .unwrap()
    }

    fn evidence(board: &SemanticDecisionBoard) -> SlmResult {
        SlmResult {
            ranking: board
                .candidates
                .iter()
                .map(|candidate| RankedCandidate {
                    candidate_id: candidate.canonical_id.as_str().to_string(),
                    score: FiniteScore::new(0.1).unwrap(),
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

    #[test]
    fn exact_is_unicode_case_and_whitespace_normalized() {
        let board = board(vec![candidate("op.one", "Ａdd   A\u{301} Task")]);
        assert_eq!(
            governed_exact(&board, "  add á TASK  "),
            ExactMatch::Unique("op.one".into())
        );
    }

    #[test]
    fn collision_expands_in_canonical_order_instead_of_choosing_first() {
        let board = board(vec![
            candidate("op.zed", "add task"),
            candidate("op.alpha", "add task"),
        ]);
        assert_eq!(
            governed_exact(&board, "ADD TASK"),
            ExactMatch::Collision(vec!["op.alpha".into(), "op.zed".into()])
        );
        let result = finalize_semantic_evidence(
            &board,
            "ADD TASK",
            evidence(&board),
            vec![EvidenceLane::Lexical],
            vec!["test.lexical".into()],
        )
        .unwrap();
        assert_eq!(result.ranking[0].score.get(), 1.0);
        assert_eq!(result.ranking[1].score.get(), 1.0);
        assert_eq!(
            result.evidence_trace.unwrap().exact_collision,
            vec!["op.alpha", "op.zed"]
        );
    }

    /// G1.4 blind-review regression (bpmn-lite-server-designer's
    /// `retrieve_utterance_evidence` exact-match short-circuit): this
    /// function only forces the MATCHED candidate to 1.0 — every other
    /// candidate is capped at `min(score, 0.99)`, never reset. If the
    /// base ranking handed a losing candidate a high raw score (e.g.
    /// `LexicalTier0`'s coincidental token-overlap or its own separate
    /// exact-pin against `description`), that score survives the cap
    /// and can collapse `policy::decide`'s separation_margin (default
    /// 0.15), flipping what should be an unambiguous auto-apply into
    /// `Ambiguous`/`EscalateToSage`. This proves BOTH halves: the raw
    /// vulnerability (a naively-fed 0.95 loser survives with a margin
    /// too small to clear a real separation_margin), and that the
    /// fix — zeroing every candidate's score before calling this
    /// function, as `retrieve_utterance_evidence` now does on its
    /// short-circuit path — restores the maximal margin the short-
    /// circuit's losslessness claim actually requires.
    #[test]
    fn exact_match_boost_does_not_reset_a_high_losing_score() {
        let board = board(vec![
            candidate("op.matched", "add the task"),
            candidate("op.rival", "totally different phrase"),
        ]);
        assert_eq!(
            governed_exact(&board, "add the task"),
            ExactMatch::Unique("op.matched".into())
        );

        // Unfixed: a losing candidate arrives with a high raw score
        // (as LexicalTier0's token-overlap/exact-pin can produce).
        let mut naive = evidence(&board);
        for rc in &mut naive.ranking {
            rc.score = FiniteScore::new(if rc.candidate_id == "op.rival" {
                0.95
            } else {
                0.1
            })
            .unwrap();
        }
        let naive_result = finalize_semantic_evidence(
            &board,
            "add the task",
            naive,
            vec![EvidenceLane::Lexical],
            vec!["test.lexical".into()],
        )
        .unwrap();
        let top = naive_result.ranking[0].score.get();
        let rival = naive_result
            .ranking
            .iter()
            .find(|rc| rc.candidate_id == "op.rival")
            .unwrap()
            .score
            .get();
        assert_eq!(top, 1.0, "the matched candidate is always boosted to 1.0");
        assert_eq!(rival, 0.95, "the rival's high raw score survives the 0.99 cap unreset");
        assert!(
            top - rival < 0.15,
            "vulnerability check: an unfixed 0.95 rival collapses a realistic 0.15 separation_margin"
        );

        // Fixed: every candidate's score zeroed before this call (what
        // `retrieve_utterance_evidence` now does on its short-circuit
        // path) restores the maximal margin.
        let mut zeroed = evidence(&board);
        for rc in &mut zeroed.ranking {
            rc.score = FiniteScore::new(0.0).unwrap();
        }
        let fixed_result = finalize_semantic_evidence(
            &board,
            "add the task",
            zeroed,
            vec![EvidenceLane::Lexical],
            vec!["test.lexical".into()],
        )
        .unwrap();
        let top = fixed_result.ranking[0].score.get();
        let rival = fixed_result
            .ranking
            .iter()
            .find(|rc| rc.candidate_id == "op.rival")
            .unwrap()
            .score
            .get();
        assert_eq!(top, 1.0);
        assert_eq!(rival, 0.0, "zeroing beforehand leaves the rival at the floor, not just capped");
        assert!(top - rival >= 0.15, "fix check: margin is maximal, clears any realistic separation_margin");
    }

    #[test]
    fn unique_exact_is_recorded_and_incomplete_full_board_is_refused() {
        let board = board(vec![candidate("op.one", "add task")]);
        let result = finalize_semantic_evidence(
            &board,
            "ADD TASK",
            evidence(&board),
            vec![EvidenceLane::Lexical],
            vec!["test.lexical".into()],
        )
        .unwrap();
        assert_eq!(result.ranking[0].candidate_id, "op.one");
        assert_eq!(result.ranking[0].score.get(), 1.0);
        let trace = result.evidence_trace.unwrap();
        assert!(trace.lanes.contains(&EvidenceLane::GovernedExact));
        assert!(trace.served_full_board);

        let mut truncated = evidence(&board);
        truncated.ranking.pop();
        assert!(finalize_semantic_evidence(
            &board,
            "no exact match",
            truncated,
            vec![EvidenceLane::Lexical],
            vec!["test.lexical".into()],
        )
        .unwrap_err()
        .to_string()
        .contains("every board candidate"));
    }

    #[test]
    fn serializer_changes_with_semantic_fields_and_is_order_stable() {
        let first = candidate("op.one", "add task");
        let mut changed = first.clone();
        changed.effect = "a different effect".into();
        assert_ne!(serialize_candidate(&first), serialize_candidate(&changed));
        assert_eq!(serialize_candidate(&first), serialize_candidate(&first));
        assert_eq!(candidate_serializer_hash().len(), 64);
    }
}
