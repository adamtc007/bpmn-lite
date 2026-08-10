//! Shared training/eval record shape (EOP-SPEC-SLM-TRAIN-001 v0.3). One
//! schema, produced by `examples/corpus_gen.rs` for training and
//! `examples/eval_enrich.rs` for the held-out eval slice — extracted
//! 2026-07-28 so the two never drift into two different record shapes
//! (which would silently break Phase D's ability to run the same
//! scoring code against both).

use serde::{Deserialize, Serialize};

pub const CORPUS_SCHEMA_ID: &str = "bpmn.semantic-corpus.v3";

pub fn corpus_schema_hash() -> String {
    blake3::hash(CORPUS_SCHEMA_ID.as_bytes())
        .to_hex()
        .to_string()
}

#[derive(Serialize, Deserialize)]
pub struct BankEntry {
    pub class_id: String,
    /// Canonical candidate id, or "abstain.none_of_the_above".
    pub label: String,
    pub regime: String,
    pub text: String,
    #[serde(default)]
    pub pair_group: Option<String>,
    /// Distinguishes sibling paraphrases within one (class,label).
    pub paraphrase_seq: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoardDump {
    #[serde(default = "legacy_board_schema")]
    pub schema: String,
    pub candidates: Vec<CandDump>,
    pub anchor: Option<String>,
    pub graph_identity: String,
    pub pack_identity: String,
    pub policy_denied: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_board: Option<semantic_decision_contracts::SemanticDecisionBoard>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandDump {
    pub canonical_id: String,
    pub description: String,
    pub schema_version: u32,
}

impl BoardDump {
    /// The one board→dump conversion (2026-07-29, DIR-004 Phase 1): lifted
    /// out of `eval_enrich.rs`/`starter_seed_eval.rs`'s identical inline
    /// mappings, now a third caller (`dev_capture.rs`) exists — same "one
    /// fixture set" rule this module's header already applies to record
    /// shape, extended to this conversion.
    pub fn from_board(board: &crate::board::Board) -> Self {
        Self::from_inference_board(board)
    }

    /// Capture either serving board without erasing semantic content.
    pub fn from_inference_board(board: &dyn crate::board::InferenceBoard) -> Self {
        BoardDump {
            schema: board.schema_label().to_string(),
            candidates: board
                .inference_candidates()
                .into_iter()
                .map(|c| CandDump {
                    canonical_id: c.canonical_id,
                    description: c.description,
                    schema_version: c.schema_version,
                })
                .collect(),
            anchor: board.anchor().map(str::to_string),
            graph_identity: board.graph_identity().to_string(),
            pack_identity: board.pack_identity().to_string(),
            policy_denied: Vec::new(),
            semantic_board: board.semantic_board().cloned(),
        }
    }
}

fn legacy_board_schema() -> String {
    "legacy_thin_v1".to_string()
}

#[derive(Serialize, Deserialize)]
pub struct Example {
    pub example_id: String,
    pub provenance: String,
    pub board_hash: String,
    pub context_projection: String,
    pub context_projection_hash: String,
    pub board: BoardDump,
    pub tier1_list: Vec<String>,
    pub retrieved_subset_hash: String,
    pub label: String,
    pub family_id: String,
    pub pair_group_id: Option<String>,
    pub style_regime: String,
    pub utterance: String,
    /// True iff `label` appears in `tier1_list` (the real tier-0
    /// retriever's K-prefix + NOTA). Always `true` for training records —
    /// `corpus_gen.rs` drops retrieval-misses before they reach the
    /// corpus (they would teach false abstention). Eval records keep
    /// misses instead of dropping them (`eval_enrich.rs`) — recall@K is
    /// exactly the fraction of eval records where this is `true`, so the
    /// field must survive into the record rather than being filtered out.
    pub gold_in_tier1: bool,
    /// Complete v3 semantic closure. Old v2 fixtures deserialize with None
    /// but are refused by the v3 trainer and bundle gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_v3: Option<SemanticCorpusClosure>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SemanticCorpusClosure {
    pub corpus_schema_id: String,
    pub corpus_schema_hash: String,
    pub semantic_board_schema_version: u32,
    pub semantic_pack_identity: String,
    pub turn_serializer_id: String,
    pub turn_serializer_hash: String,
    pub candidate_serializer_id: String,
    pub candidate_serializer_hash: String,
    pub pair_serializer_id: String,
    pub pair_serializer_hash: String,
    pub candidate_pairs: Vec<ScoredCandidatePair>,
    pub full_served_list: Vec<String>,
    pub board_inclusion_truth: bool,
    pub exact_match: crate::exact::ExactMatchDump,
    pub evidence_lanes: Vec<semantic_decision_contracts::EvidenceLane>,
    pub semantic_family: String,
    pub author_provenance: String,
    pub split_group: String,
    pub binding_requirements: Vec<semantic_decision_contracts::ArgumentSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoredCandidatePair {
    pub candidate_id: String,
    pub candidate_semantic_text: String,
    pub candidate_semantic_hash: String,
    pub pair: crate::pair::SerializedCandidatePair,
}

impl SemanticCorpusClosure {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        board: &semantic_decision_contracts::SemanticDecisionBoard,
        utterance: &str,
        turn: &crate::context::ContextProjection,
        gold: &str,
        semantic_family: String,
        author_provenance: String,
        split_group: String,
    ) -> anyhow::Result<Self> {
        let candidate_pairs = board
            .candidates
            .iter()
            .map(|candidate| {
                let candidate_semantic_text = crate::exact::serialize_candidate(candidate);
                Ok(ScoredCandidatePair {
                    candidate_id: candidate.canonical_id.as_str().to_string(),
                    candidate_semantic_hash: blake3::hash(candidate_semantic_text.as_bytes())
                        .to_hex()
                        .to_string(),
                    candidate_semantic_text,
                    pair: crate::pair::serialize_candidate_pair(
                        utterance,
                        turn,
                        candidate,
                        crate::pair::PairTokenBudget::default(),
                    )?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let full_served_list = board
            .candidates
            .iter()
            .map(|candidate| candidate.canonical_id.as_str().to_string())
            .collect::<Vec<_>>();
        let binding_requirements = board
            .candidates
            .iter()
            .find(|candidate| candidate.canonical_id.as_str() == gold)
            .map(|candidate| candidate.arguments.clone())
            .unwrap_or_default();
        Ok(Self {
            corpus_schema_id: CORPUS_SCHEMA_ID.to_string(),
            corpus_schema_hash: corpus_schema_hash(),
            semantic_board_schema_version: board.schema_version,
            semantic_pack_identity: board.semantic_snapshot.as_str().to_string(),
            turn_serializer_id: crate::pair::TURN_SERIALIZER_ID.to_string(),
            turn_serializer_hash: crate::pair::turn_serializer_hash(),
            candidate_serializer_id: crate::exact::CANDIDATE_SERIALIZER_ID.to_string(),
            candidate_serializer_hash: crate::exact::candidate_serializer_hash(),
            pair_serializer_id: crate::pair::PAIR_SERIALIZER_ID.to_string(),
            pair_serializer_hash: crate::pair::pair_serializer_hash(),
            candidate_pairs,
            full_served_list,
            board_inclusion_truth: board
                .candidates
                .iter()
                .any(|candidate| candidate.canonical_id.as_str() == gold),
            exact_match: crate::exact::ExactMatchDump::from(crate::exact::governed_exact(
                board, utterance,
            )),
            evidence_lanes: vec![semantic_decision_contracts::EvidenceLane::Lexical],
            semantic_family,
            author_provenance,
            split_group,
            binding_requirements,
        })
    }
}
