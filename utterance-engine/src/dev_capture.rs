//! Dev-session capture — Adam's own testing sessions ONLY. Always
//! compiled (no feature gate): this is deliberately separate from, and
//! independent of, the Q9-gated `capture` module (DIR-004 Phase 1,
//! "Option B — structural separation, not convention", ruled 2026-07-29
//! after the fork surfaced in EOP-DIR-BPMN-DESIGN-003-003's close-out).
//!
//! The fork this module resolves: `capture::CapturePipeline::
//! on_under_charter` is a single generic string-gated switch — nothing
//! stopped a caller from passing an ad-hoc "charter ref" and getting
//! live capture with no real charter behind it, distinguishable from
//! the real Q9 path only by naming convention. This module makes the
//! distinction a TYPE, not a string:
//! - `DevSessionSubject` has exactly one variant (`Adam`) — there is no
//!   value of this type that means anyone else, so a record's subject
//!   cannot silently drift without a code change and a review.
//! - `DEV_SESSION_PROVENANCE` is a module constant, not a caller-settable
//!   field — `DevSessionStore::capture` is the only place that stamps it.
//! - `DevSessionStore::open` requires both a session id and a consent
//!   statement (stated at session start) at CONSTRUCTION, the same
//!   pattern `CapturePipeline::on_under_charter` uses for the charter
//!   ref — empty/whitespace either is refused, not defaulted.
//! - This module lives in its own file with its own store type — no
//!   shared "capture record with a source field" (the design DIR-004
//!   explicitly rejects), and it does not depend on `capture.rs` (which
//!   isn't even compiled by default — see that module's header).
//!
//! "Train-on-able, not hash-only" (directive 1.3): `DecisionRecord`
//! alone (board_hash, context_projection_hash, ...) cannot be tokenized
//! for training — you need the actual candidate descriptions and the
//! actual context text, not their hashes. `DevSessionRecord` carries
//! both in full (`BoardDump` + `ContextProjection::serialize_canonical`
//! output), so a captured session interaction is corpus-ready without
//! any further lookup.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::contract::FiniteScore;
use crate::corpus_schema::BoardDump;
use crate::policy::ProposalDisposition;

/// The provenance every `DevSessionRecord` carries — a constant, never a
/// field the caller supplies. `starter-seed-v1`'s disputed items graduate
/// to `adam-adjudicated` (see `examples/adjudicate_starter_seed.rs`); real
/// session capture graduates to this.
pub const DEV_SESSION_PROVENANCE: &str = "dev-session-adam-v1";

/// Exactly one variant, deliberately: makes "subject is always Adam" a
/// compile-time fact for this module, not a runtime string that could
/// drift to someone else's data without a code change and review.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DevSessionSubject {
    Adam,
}

/// The full I28 closure for one captured interaction, train-on-able.
/// `session_id`/`subject`/`consent_statement_timestamp`/`provenance` are
/// stamped by `DevSessionStore::capture` from the store's own
/// construction-time state — never supplied per event, so one open
/// session cannot silently claim a different session id or consent
/// statement mid-stream.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevSessionRecord {
    pub session_id: String,
    pub subject: DevSessionSubject,
    pub consent_statement_timestamp: String,
    pub provenance: String,
    pub raw_utterance: String,
    pub board_hash: String,
    pub board: BoardDump,
    pub context_projection: String,
    pub context_projection_hash: String,
    pub retrieved_subset_hash: String,
    pub model_bundle_hash: String,
    pub disposition_policy_hash: String,
    pub ranking: Vec<(String, FiniteScore)>,
    pub disposition: ProposalDisposition,
}

/// Per-event payload `DevSessionStore::capture` needs — everything the
/// I28 closure carries EXCEPT the store-owned session/consent/subject/
/// provenance fields above.
pub struct DevSessionCaptureInput {
    pub raw_utterance: String,
    pub board_hash: String,
    pub board: BoardDump,
    pub context_projection: String,
    pub context_projection_hash: String,
    pub retrieved_subset_hash: String,
    pub model_bundle_hash: String,
    pub disposition_policy_hash: String,
    pub ranking: Vec<(String, FiniteScore)>,
    pub disposition: ProposalDisposition,
}

/// One open dev-testing session. The only constructor requires a
/// non-empty session id and a non-empty consent statement (stated at
/// session start) — mirrors `CapturePipeline::on_under_charter`'s
/// empty-ref refusal, applied here to Adam's own consent rather than a
/// charter reference.
pub struct DevSessionStore {
    session_id: String,
    consent_statement_timestamp: String,
    records: Vec<DevSessionRecord>,
}

impl DevSessionStore {
    pub fn open(session_id: &str, consent_statement_timestamp: &str) -> Result<Self> {
        let session_id = session_id.trim();
        let consent = consent_statement_timestamp.trim();
        if session_id.is_empty() {
            bail!("dev-session capture requires a non-empty session id");
        }
        if consent.is_empty() {
            bail!(
                "dev-session capture requires consent stated at session start \
                 (D17's spirit, applied here to Adam's own self-testing use — \
                 not a charter, but still not silent)"
            );
        }
        Ok(DevSessionStore {
            session_id: session_id.to_string(),
            consent_statement_timestamp: consent.to_string(),
            records: Vec::new(),
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Capture one interaction's full closure. Always stores — this
    /// module has no suppressed/off state, unlike `capture::
    /// CapturePipeline`: a `DevSessionStore` only exists once `open` has
    /// already required consent, so there is nothing left to gate.
    pub fn capture(&mut self, input: DevSessionCaptureInput) -> &DevSessionRecord {
        let record = DevSessionRecord {
            session_id: self.session_id.clone(),
            subject: DevSessionSubject::Adam,
            consent_statement_timestamp: self.consent_statement_timestamp.clone(),
            provenance: DEV_SESSION_PROVENANCE.to_string(),
            raw_utterance: input.raw_utterance,
            board_hash: input.board_hash,
            board: input.board,
            context_projection: input.context_projection,
            context_projection_hash: input.context_projection_hash,
            retrieved_subset_hash: input.retrieved_subset_hash,
            model_bundle_hash: input.model_bundle_hash,
            disposition_policy_hash: input.disposition_policy_hash,
            ranking: input.ranking,
            disposition: input.disposition,
        };
        self.records.push(record);
        self.records.last().expect("just pushed")
    }

    pub fn records(&self) -> &[DevSessionRecord] {
        &self.records
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{build_board, EmptyUniverse, PolicyFilter};
    use crate::policy::{decide, DispositionConfig};
    use crate::retrieval::{LexicalTier0, Tier0Retriever};
    use designer_graph::board_candidate::{LegalityOracle, OperationKind, ProductionId};

    struct AllLegal;
    impl LegalityOracle for AllLegal {
        type NodeKey = ();
        fn legal_operations(&self, _: Option<&()>) -> Vec<OperationKind> {
            OperationKind::ALL.to_vec()
        }
        fn legal_productions(&self, _: Option<&()>) -> Vec<ProductionId> {
            ProductionId::ALL.to_vec()
        }
    }

    fn one_input() -> DevSessionCaptureInput {
        let board =
            build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default()).unwrap();
        let context = crate::context::minimal("pack.none", "g-test");
        let evidence = LexicalTier0.retrieve("connect the nodes", &board).unwrap();
        let (disposition, record) =
            decide(&DispositionConfig::shadow_v1(), &board, &evidence, &context).unwrap();
        DevSessionCaptureInput {
            raw_utterance: "connect the nodes".into(),
            board_hash: record.board_hash.clone(),
            board: BoardDump::from_board(&board),
            context_projection: context.serialize_canonical(),
            context_projection_hash: record.context_projection_hash.clone(),
            retrieved_subset_hash: record.retrieved_subset_hash.clone(),
            model_bundle_hash: record.model_bundle_hash.clone(),
            disposition_policy_hash: record.disposition_policy_hash.clone(),
            ranking: record.ranking.clone(),
            disposition,
        }
    }

    /// Construction red: empty session id / empty consent are refused —
    /// no silent "unconsented" store can come into existence.
    #[test]
    fn open_refuses_empty_session_id_and_empty_consent() {
        assert!(DevSessionStore::open("", "2026-07-29T09:00:00Z").is_err());
        assert!(DevSessionStore::open("sess-1", "").is_err());
        assert!(DevSessionStore::open("  ", "2026-07-29T09:00:00Z").is_err());
        assert!(DevSessionStore::open("sess-1", "   ").is_err());
    }

    /// Green: capture stamps subject/provenance/session/consent from the
    /// store, never from the caller — the record is train-on-able (board
    /// dump + context TEXT present, not just their hashes).
    #[test]
    fn capture_stamps_type_carried_provenance_and_is_train_on_able() {
        let mut store = DevSessionStore::open("sess-1", "2026-07-29T09:00:00Z").unwrap();
        let record = store.capture(one_input()).clone();
        assert_eq!(record.session_id, "sess-1");
        assert_eq!(record.consent_statement_timestamp, "2026-07-29T09:00:00Z");
        assert_eq!(record.subject, DevSessionSubject::Adam);
        assert_eq!(record.provenance, DEV_SESSION_PROVENANCE);
        assert_eq!(record.provenance, "dev-session-adam-v1");
        assert!(!record.board.candidates.is_empty(), "board dump must carry real candidates, not just a hash");
        assert!(!record.context_projection.is_empty(), "context text must be present, not hash-only");
        assert_eq!(store.records().len(), 1);
        assert_eq!(store.session_id(), "sess-1");
    }

    /// Multiple captures in one session all carry the SAME session/
    /// consent/subject — per-event data cannot override store state.
    #[test]
    fn multiple_captures_share_one_sessions_consent() {
        let mut store = DevSessionStore::open("sess-2", "2026-07-29T10:00:00Z").unwrap();
        store.capture(one_input());
        store.capture(one_input());
        assert_eq!(store.records().len(), 2);
        for r in store.records() {
            assert_eq!(r.session_id, "sess-2");
            assert_eq!(r.consent_statement_timestamp, "2026-07-29T10:00:00Z");
        }
    }
}
