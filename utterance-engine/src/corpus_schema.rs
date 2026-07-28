//! Shared training/eval record shape (EOP-SPEC-SLM-TRAIN-001 v0.3). One
//! schema, produced by `examples/corpus_gen.rs` for training and
//! `examples/eval_enrich.rs` for the held-out eval slice — extracted
//! 2026-07-28 so the two never drift into two different record shapes
//! (which would silently break Phase D's ability to run the same
//! scoring code against both).

use serde::{Deserialize, Serialize};

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

#[derive(Serialize, Deserialize)]
pub struct BoardDump {
    pub candidates: Vec<CandDump>,
    pub anchor: Option<String>,
    pub graph_identity: String,
    pub pack_identity: String,
    pub policy_denied: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct CandDump {
    pub canonical_id: String,
    pub description: String,
    pub schema_version: u32,
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
}
