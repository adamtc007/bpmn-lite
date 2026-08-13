use crate::{ArtifactStoreError, ClaimError, CommitError, CommitOutcome, StoreResult};
use async_trait::async_trait;
use bpmn_lite_types::RuntimeEvent;
use bpmn_lite_types::*;
use uuid::Uuid;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TemplateSummary {
    pub name: String,
    pub latest_version: u32,
    pub plan_hash: [u8; 32],
    pub created_at: String,
}

// ── Designer sessions (EOP-SAGE-REPL-BPMN-001 T1) ──────────────────────

/// One append-only event in a design session. Revisions carry the FULL
/// DSL source (undo = re-append an earlier revision's source; nothing is
/// ever deleted), utterances carry the REPL dialogue.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DesignSessionEventKind {
    Revision {
        dsl_source: String,
        note: String,
    },
    Utterance {
        text: String,
        response: String,
        /// DIR-002 "AFTER" item 2: the SERIALIZED context projection
        /// (ctxproj canonical text) — stored richly, not hash-only, so
        /// charter-era session records are trainable. blake3 of these
        /// bytes must equal the decision record's
        /// `context_projection_hash` (recorded values are the truth;
        /// recomputation is the audit). Additive + defaulted so earlier
        /// events decode unchanged.
        #[serde(default)]
        context_projection: Option<String>,
        /// WS-B day-one wiring (DESIGN-003): the I28 decision record
        /// for this utterance, serialized JSON. Additive + defaulted so
        /// pre-shadow events decode unchanged; None only for events
        /// recorded before the disposition pipeline ran.
        #[serde(default)]
        decision_record_json: Option<String>,
        /// Phase 4 Semantic Gameboard: a terminal typed attempt receipt.
        /// Kept opaque so persistence does not depend on capability contracts.
        #[serde(default)]
        gameboard_attempt_receipt_json: Option<String>,
        /// Non-authoritative belief snapshot produced for this position.
        #[serde(default)]
        gameboard_belief_json: Option<String>,
        /// Phase 5 deterministic, position-bound interaction packet.
        #[serde(default)]
        gameboard_disposition_json: Option<String>,
        /// Bounded history projection used to construct the observed position.
        #[serde(default)]
        history_projection_hash: Option<String>,
    },
    /// WS-B.4 (2026-07-27): a validated slice of the Designer edit log —
    /// a `Vec<designer_graph::ops::Operation>` that staged and admitted
    /// against the session's DesignerDag reconstruction at append time.
    /// Stored as OPAQUE JSON deliberately (schema.rs's own module doc:
    /// "the durable surface is the edit log... the DAG is its replay
    /// product" — this store crate has no `designer-graph` dependency
    /// and none is added; only the server layer, which already depends
    /// on `designer-graph`, ever deserializes `operations_json`). A
    /// session accumulating GraphEdit events is DesignerDag-backed —
    /// its context projections become training-grade (`project_ir` over
    /// a real `IRGraph`, not the DSL-source census fallback) and its
    /// board legality runs the real `PositionalLegality` oracle instead
    /// of `WholeGraphLegality`. Sessions with zero GraphEdit events stay
    /// on the legacy DSL-source path — purely additive, no session's
    /// existing behavior changes underneath it.
    GraphEdit {
        operations_json: String,
        note: String,
    },
    /// Phase 8: one durable link in an utterance-derived proposal chain.
    /// The store keeps the shared workbook and optional bound plan opaque,
    /// just as `GraphEdit` keeps Designer operations opaque. Server code owns
    /// their schemas; the append-only event log owns their historical bytes.
    ProposalAudit {
        workbook_json: String,
        #[serde(default)]
        bound_plan_json: Option<String>,
        outcome: String,
        #[serde(default)]
        dry_run_diagnostics: Vec<String>,
        dry_run_diagnostics_hash: String,
        decision_record_hash: String,
        #[serde(default)]
        related_event_seq: Option<u64>,
        /// Optional terminal Semantic Gameboard attempt, opaque to the store.
        #[serde(default)]
        gameboard_attempt_receipt_json: Option<String>,
    },
    /// G6.1: session undo as truncated replay. Marks that, from this event
    /// forward, replay of the tape excludes every event with
    /// `target_seq < seq <= this event's own seq` — the range this undo
    /// retired. Nothing is deleted (same invariant as `Revision`'s
    /// re-append pattern): the excluded events remain in storage and are
    /// visible to any explicit historical `as_of_seq` view that itself sits
    /// at or before `target_seq`. A later event appended after this one's
    /// own seq is forward progress from the undone position and stays
    /// visible — see `DesignSessionRecord::visible_events`.
    Undo {
        target_seq: u64,
    },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DesignSessionEvent {
    pub seq: u64,
    pub kind: DesignSessionEventKind,
    pub at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DesignSessionStatus {
    Draft,
    Saved,
}

/// Full session aggregate: metadata + the ordered event log. The
/// CURRENT source is the last Revision event's `dsl_source` (derived,
/// never stored separately — one source of truth).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DesignSessionRecord {
    pub id: Uuid,
    pub tenant_id: String,
    pub name: String,
    pub status: DesignSessionStatus,
    /// (template_name, version, plan_hash) once saved-as-template.
    pub template_ref: Option<(String, u32, [u8; 32])>,
    // H3 (EOP-PLAN-CRATE-HYGIENE-001, R5): pub(crate) — `visible_events`/
    // `graph_edit_payloads_as_of` exist to preserve the undo-jump-chain
    // invariant over this collection; a public field let external code
    // bypass it via direct mutation. Cross-crate reads go through
    // `events()` below; cross-crate construction goes through `new()`
    // below (`bpmn-lite-store-postgres` reconstructs a record from
    // persisted rows). `store_memory.rs` (same crate, sibling module)
    // still appends directly via this field.
    pub(crate) events: Vec<DesignSessionEvent>,
    pub created_at: String,
}

impl DesignSessionRecord {
    /// Reconstruct a record from its persisted parts (e.g. a store
    /// backend replaying rows into the aggregate). Not a query-time
    /// mutation path — `events` here is meant to already be the full,
    /// ordered log as persisted.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Uuid,
        tenant_id: String,
        name: String,
        status: DesignSessionStatus,
        template_ref: Option<(String, u32, [u8; 32])>,
        events: Vec<DesignSessionEvent>,
        created_at: String,
    ) -> Self {
        Self {
            id,
            tenant_id,
            name,
            status,
            template_ref,
            events,
            created_at,
        }
    }

    /// Read-only view — see the field's own doc comment for why direct
    /// mutation isn't exposed.
    pub fn events(&self) -> &[DesignSessionEvent] {
        &self.events
    }

    /// G6.1: the events visible under truncated replay, given an optional
    /// `as_of_seq` bound. This is a JUMP CHAIN, not a static union of
    /// excluded ranges (an earlier flat-range formulation was wrong: it let
    /// an OLDER undo's exclusion outlive a NEWER undo that re-targets
    /// forward past it, permanently hiding a position a later undo asked
    /// to return to — caught in blind review, see
    /// `the_undo_marker_event_itself_always_stays_visible`/
    /// `a_later_undo_target_stays_reachable_after_an_earlier_undo`).
    ///
    /// Recursive definition, `visible(bound)`:
    /// 1. `candidates` = events with `seq <= bound`.
    /// 2. If no `Undo` event is among `candidates`, `visible(bound)` = every
    ///    `candidates` seq.
    /// 3. Otherwise let `U` be the LATEST (highest-seq) `Undo{target_seq}`
    ///    event among `candidates`. `visible(bound)` = `visible(target_seq)`
    ///    (recursively resolve the position `U` jumped back to) UNION `U`'s
    ///    own seq (the marker itself always stays visible — it is not a
    ///    `GraphEdit`/`Revision` so it never affects `graph_edit_payloads`/
    ///    `current_source`, but it must count toward the live head) UNION
    ///    every `candidates` seq strictly greater than `U`'s own seq
    ///    (forward progress made AFTER that undo).
    ///
    /// Terminates: each recursive step's bound (`target_seq`) is strictly
    /// less than the undo event's own seq, which is `<=` the current bound
    /// — strictly decreasing.
    pub fn visible_events(&self, as_of_seq: Option<u64>) -> Vec<&DesignSessionEvent> {
        let bound = as_of_seq.unwrap_or(u64::MAX);
        let visible_seqs = self.visible_seqs_as_of(bound);
        self.events
            .iter()
            .filter(|event| visible_seqs.contains(&event.seq))
            .collect()
    }

    fn visible_seqs_as_of(&self, bound: u64) -> std::collections::BTreeSet<u64> {
        let candidates: Vec<&DesignSessionEvent> =
            self.events.iter().filter(|e| e.seq <= bound).collect();
        let latest_undo = candidates.iter().rev().find_map(|e| match &e.kind {
            DesignSessionEventKind::Undo { target_seq } => Some((e.seq, *target_seq)),
            _ => None,
        });
        match latest_undo {
            None => candidates.iter().map(|e| e.seq).collect(),
            Some((undo_seq, target_seq)) => {
                let mut visible = self.visible_seqs_as_of(target_seq);
                visible.insert(undo_seq);
                visible.extend(candidates.iter().filter(|e| e.seq > undo_seq).map(|e| e.seq));
                visible
            }
        }
    }

    /// The session's current DSL source: the last visible Revision event.
    pub fn current_source(&self) -> Option<&str> {
        self.current_source_as_of(None)
    }

    /// G6.1: `current_source`, bounded to a truncated replay view.
    pub fn current_source_as_of(&self, as_of_seq: Option<u64>) -> Option<&str> {
        self.visible_events(as_of_seq)
            .into_iter()
            .rev()
            .find_map(|event| match &event.kind {
                DesignSessionEventKind::Revision { dsl_source, .. } => Some(dsl_source.as_str()),
                DesignSessionEventKind::Utterance { .. }
                | DesignSessionEventKind::GraphEdit { .. }
                | DesignSessionEventKind::ProposalAudit { .. }
                | DesignSessionEventKind::Undo { .. } => None,
            })
    }

    /// WS-B.4: every `GraphEdit` operations payload, in event order — the
    /// exact replay sequence a `DesignerDag` reconstruction folds over.
    /// Opaque here (`&str`, undeserialized) by the same store/server
    /// split `GraphEdit`'s own doc comment states.
    pub fn graph_edit_payloads(&self) -> Vec<&str> {
        self.graph_edit_payloads_as_of(None)
    }

    /// G6.1: `graph_edit_payloads`, bounded to a truncated replay view —
    /// the exact input `reconstruct_designer_dag`'s `as_of_seq` bound folds.
    pub fn graph_edit_payloads_as_of(&self, as_of_seq: Option<u64>) -> Vec<&str> {
        self.visible_events(as_of_seq)
            .into_iter()
            .filter_map(|event| match &event.kind {
                DesignSessionEventKind::GraphEdit {
                    operations_json, ..
                } => Some(operations_json.as_str()),
                DesignSessionEventKind::Revision { .. }
                | DesignSessionEventKind::Utterance { .. }
                | DesignSessionEventKind::ProposalAudit { .. }
                | DesignSessionEventKind::Undo { .. } => None,
            })
            .collect()
    }

    /// G6.1: whether the event at `related_seq` (e.g. a `ProposalAudit`'s
    /// `related_event_seq` pointer) is still visible under the given
    /// `as_of_seq` bound — i.e. whether the pointer is dangling because the
    /// event it names was excised by an intervening undo, or lies in the
    /// future of an explicit historical view.
    pub fn related_event_is_visible(&self, related_seq: u64, as_of_seq: Option<u64>) -> bool {
        self.visible_events(as_of_seq)
            .iter()
            .any(|event| event.seq == related_seq)
    }

    /// Whether this session has ever accumulated a graph edit — the
    /// switch between the legacy DSL-source path and the DesignerDag-
    /// backed path (WS-B.4 module doc on `GraphEdit`).
    pub fn is_graph_backed(&self) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event.kind, DesignSessionEventKind::GraphEdit { .. }))
    }
}

/// Durable record of one dev-capture session (G1.2). Replaces an
/// in-memory-only `Mutex<HashMap>` that silently lost every captured
/// interaction on restart — DIR-004 Phase 1's "train-on-able, not
/// hash-only" guarantee is worthless if the records evaporate before
/// they can ever be exported for training. Opaque JSON records,
/// mirroring `GraphEdit`'s opaque-payload discipline: this crate has no
/// `utterance-engine` dependency, so `utterance_engine::dev_capture::
/// DevSessionRecord` is only ever deserialized at the server layer.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DevCaptureSessionRecord {
    pub session_id: String,
    pub consent_statement_timestamp: String,
    pub records_json: Vec<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DesignSessionSummary {
    pub id: Uuid,
    pub name: String,
    pub status: DesignSessionStatus,
    pub revisions: u32,
    pub updated_at: String,
}

#[cfg(test)]
mod g6_1_undo_tests {
    use super::*;

    fn revision(seq: u64, source: &str) -> DesignSessionEvent {
        DesignSessionEvent {
            seq,
            kind: DesignSessionEventKind::Revision {
                dsl_source: source.into(),
                note: String::new(),
            },
            at: "t".into(),
        }
    }

    fn graph_edit(seq: u64, tag: &str) -> DesignSessionEvent {
        DesignSessionEvent {
            seq,
            kind: DesignSessionEventKind::GraphEdit {
                operations_json: tag.into(),
                note: String::new(),
            },
            at: "t".into(),
        }
    }

    fn undo(seq: u64, target_seq: u64) -> DesignSessionEvent {
        DesignSessionEvent {
            seq,
            kind: DesignSessionEventKind::Undo { target_seq },
            at: "t".into(),
        }
    }

    fn record(events: Vec<DesignSessionEvent>) -> DesignSessionRecord {
        DesignSessionRecord {
            id: Uuid::nil(),
            tenant_id: "t".into(),
            name: "s".into(),
            status: DesignSessionStatus::Draft,
            template_ref: None,
            events,
            created_at: "t".into(),
        }
    }

    #[test]
    fn no_undo_event_visible_events_is_the_whole_log() {
        let r = record(vec![graph_edit(0, "a"), graph_edit(1, "b")]);
        assert_eq!(r.visible_events(None).len(), 2);
        assert_eq!(r.graph_edit_payloads_as_of(None), vec!["a", "b"]);
    }

    #[test]
    fn undo_excises_the_range_it_targets_but_keeps_earlier_and_later_events() {
        // seq: 0=a, 1=b, 2=c, 3=Undo{target_seq:0} -> visible: [a]
        let r = record(vec![
            graph_edit(0, "a"),
            graph_edit(1, "b"),
            graph_edit(2, "c"),
            undo(3, 0),
        ]);
        assert_eq!(r.graph_edit_payloads_as_of(None), vec!["a"]);
    }

    #[test]
    fn edits_appended_after_the_undo_marker_stay_visible() {
        // 0=a,1=b,2=Undo{target_seq:0},3=d -> visible: [a, d] (b excised, d kept)
        let r = record(vec![
            graph_edit(0, "a"),
            graph_edit(1, "b"),
            undo(2, 0),
            graph_edit(3, "d"),
        ]);
        assert_eq!(r.graph_edit_payloads_as_of(None), vec!["a", "d"]);
    }

    #[test]
    fn explicit_historical_view_before_the_undo_ignores_it() {
        // as_of_seq=1 predates the undo at seq=2 entirely, so nothing is excised.
        let r = record(vec![
            graph_edit(0, "a"),
            graph_edit(1, "b"),
            undo(2, 0),
            graph_edit(3, "d"),
        ]);
        assert_eq!(r.graph_edit_payloads_as_of(Some(1)), vec!["a", "b"]);
    }

    #[test]
    fn repeated_undo_compounds_exclusions() {
        // 0=a,1=b,2=c,3=Undo{target:1} (excises b? no: excises (1,3]=c... wait c is seq2<=3)
        // Explicit: excise (1,3] = {2,3} -> visible after first undo: [a,b]
        // then 4=d, 5=Undo{target:0} -> excise (0,5] = {1,2,3,4} -> visible: [a]
        let r = record(vec![
            graph_edit(0, "a"),
            graph_edit(1, "b"),
            graph_edit(2, "c"),
            undo(3, 1),
            graph_edit(4, "d"),
            undo(5, 0),
        ]);
        assert_eq!(r.graph_edit_payloads_as_of(None), vec!["a"]);
        // Historical view as of the first undo (seq 3) should show [a, b].
        assert_eq!(r.graph_edit_payloads_as_of(Some(3)), vec!["a", "b"]);
    }

    #[test]
    fn current_source_respects_the_excised_range() {
        let r = record(vec![
            revision(0, "v0"),
            revision(1, "v1"),
            undo(2, 0),
        ]);
        assert_eq!(r.current_source(), Some("v0"));
        assert_eq!(r.current_source_as_of(Some(1)), Some("v1"));
    }

    #[test]
    fn related_event_is_visible_detects_a_dangling_pointer_after_undo() {
        let r = record(vec![graph_edit(0, "a"), graph_edit(1, "b"), undo(2, 0)]);
        assert!(r.related_event_is_visible(0, None));
        assert!(!r.related_event_is_visible(1, None), "seq 1 was excised by the undo");
        assert!(r.related_event_is_visible(1, Some(1)), "an as_of_seq view before the undo still sees it");
    }

    /// Regression (blind review, G6.1): the `Undo` marker event's OWN seq
    /// must never be excised by the very range it defines — `(target_seq,
    /// seq]` (inclusive) would have the undo event exclude itself, so
    /// `visible_events(None)`'s max seq (the session's live head) silently
    /// collapses back toward `target_seq` after every undo instead of
    /// tracking the true latest event. This previously let a session's
    /// computed "current head" fall below its true head, incorrectly
    /// refusing a later, perfectly legal `/undo` call to a MORE RECENT
    /// target than the collapsed (wrong) head, and — in the degenerate
    /// case of undoing to `target_seq: 0` — could brick the undo endpoint
    /// entirely (nothing is `< 0` for a `u64` head).
    #[test]
    fn the_undo_marker_event_itself_always_stays_visible() {
        let r = record(vec![graph_edit(0, "a"), graph_edit(1, "b"), undo(2, 0)]);
        let visible_seqs: Vec<u64> = r.visible_events(None).iter().map(|e| e.seq).collect();
        assert_eq!(
            visible_seqs,
            vec![0, 2],
            "the Undo event at seq 2 must remain visible even though it excises seq 1"
        );
        assert_eq!(
            visible_seqs.iter().max().copied(),
            Some(2),
            "the session's live head must be the Undo marker's own seq, not target_seq"
        );
    }

    /// Regression (blind review, G6.1): a second, later-targeted undo must
    /// remain reachable after an earlier undo — the live head must always
    /// be the true latest event's seq, so a caller re-targeting to a MORE
    /// recent (but still prior) position never gets wrongly refused.
    #[test]
    fn a_later_undo_target_stays_reachable_after_an_earlier_undo() {
        // 0=a, 1=b, 2=c, Undo{target:0}@3 -> live head must be 3, not 0.
        let r = record(vec![
            graph_edit(0, "a"),
            graph_edit(1, "b"),
            graph_edit(2, "c"),
            undo(3, 0),
        ]);
        let head = r.visible_events(None).iter().map(|e| e.seq).max();
        assert_eq!(head, Some(3));
        // target_seq=2 is strictly before the true head (3) and must be a
        // legal re-target, even though it is AFTER the excised range's
        // upper bound under the buggy inclusive-hi computation.
        assert!(matches!(head, Some(h) if 2 < h));
    }
}

/// Transactional runtime claims, aggregate reads, and fenced transition commits.
#[async_trait]
pub trait RuntimeStore: Send + Sync + JournalReader {
    // ── Instance ──

    async fn load_instance(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
    ) -> StoreResult<Option<ProcessInstance>>;

    // ── Fibers ──

    async fn load_fiber(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        fiber_id: Uuid,
    ) -> StoreResult<Option<Fiber>>;

    async fn load_fibers(&self, tenant_id: &TenantId, instance_id: Uuid)
        -> StoreResult<Vec<Fiber>>;

    // ── Dedupe cache (engine-side) ──

    async fn dedupe_get(
        &self,
        tenant_id: &TenantId,
        key: &str,
    ) -> StoreResult<Option<JobCompletion>>;

    // ── Job queue ──

    async fn dequeue_jobs(
        &self,
        task_types: &[String],
        max: usize,
        tenant_id: &TenantId,
        worker_id: &str,
        lease_ms: u64,
    ) -> StoreResult<Vec<JobActivation>>;

    async fn validate_job_claim(
        &self,
        tenant_id: &TenantId,
        job_key: &str,
        worker_id: &str,
        claim_token: &str,
    ) -> StoreResult<bool>;

    // ── Dead-letter queue ──

    async fn dead_letter_put(
        &self,
        name: u32,
        corr_key: &Value,
        payload: &[u8],
        ttl_ms: u64,
    ) -> StoreResult<()>;

    async fn dead_letter_take(&self, name: u32, corr_key: &Value) -> StoreResult<Option<Vec<u8>>>;

    async fn claim_buffered_message(
        &self,
        tenant_id: &TenantId,
        message_name: &str,
        correlation_key: &str,
        claim_ms: u64,
    ) -> StoreResult<Option<ClaimedBufferedMessage>>;

    async fn reclaim_stale_buffered_message_claims(&self) -> StoreResult<u32>;

    async fn prune_expired_messages(&self) -> StoreResult<u32>;

    // ── Incidents ──

    async fn load_incidents(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Vec<Incident>>;

    // ── Durability maintenance ──

    /// Reclaim jobs whose persisted `claim_expires_at` has passed.
    ///
    /// F-06 remediation: authority is the deadline recorded at dequeue
    /// time (`claim_expires_at`), never a separately supplied timeout —
    /// a caller-chosen timeout shorter than the issued lease reclaims a
    /// still-valid claim out from under its worker, and a longer one
    /// leaves an actually-expired claim unrecoverable. Returns count
    /// reclaimed.
    async fn reclaim_stale_jobs(&self) -> StoreResult<u32>;

    /// Prune dedupe entries older than the given age. Returns count pruned.
    async fn prune_dedupe_cache(&self, older_than_ms: u64) -> StoreResult<u32>;

    /// List all running (non-terminal) instance IDs.
    async fn list_running_instances(&self, tenant_id: &TenantId) -> StoreResult<Vec<Uuid>>;

    /// Claim a bounded batch of running instances for scheduler work.
    async fn claim_running_instances(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<Uuid>>;

    /// Claim the mutation right for one process instance.
    ///
    /// This is a transition guard, not a scheduler batch primitive. Every
    /// mutating engine path should claim this before changing instance state.
    async fn claim_instance_for_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
    ) -> std::result::Result<Option<Claim>, ClaimError>;

    /// Fence an instance and return its complete persisted aggregate plus the
    /// command to apply. Engine execution uses this method instead of issuing
    /// independent instance/fiber/incident loads.
    async fn claim_work_for_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        owner: &str,
        lease_ms: u64,
        command: Command,
    ) -> std::result::Result<Option<ClaimedWork>, ClaimError> {
        let Some(claim) = self
            .claim_instance_for_transition(tenant_id, instance_id, owner, lease_ms)
            .await?
        else {
            return Ok(None);
        };
        let load_result = self.load_snapshot_envelope(tenant_id, instance_id).await;
        let (envelope, claim_error) = match load_result {
            Ok(Some(envelope)) => (Some(envelope), None),
            Ok(None) => (
                None,
                Some(ClaimError::Integrity(
                    "claimed instance has no snapshot".to_string(),
                )),
            ),
            Err(error) => (None, Some(ClaimError::Unavailable(error.to_string()))),
        };
        if let Some(claim_error) = claim_error {
            if let Err(release_error) = self
                .release_instance_transition(tenant_id, instance_id, claim.lease_token())
                .await
            {
                return Err(ClaimError::Unavailable(format!(
                    "{claim_error}; transition lease release also failed: {release_error}"
                )));
            }
            return Err(claim_error);
        }
        let envelope = envelope.ok_or_else(|| {
            ClaimError::Integrity("claimed instance snapshot load returned no result".to_string())
        })?;
        let snapshot = envelope.state().to_runtime_snapshot();
        Ok(Some(ClaimedWork::new(claim, snapshot, command)))
    }

    /// Commit one complete logical transition under optimistic revision and
    /// monotonic fencing checks.
    async fn commit_transition(
        &self,
        claim: &Claim,
        transition: &Transition,
    ) -> std::result::Result<CommitOutcome, CommitError>;

    async fn lookup_start_instance(
        &self,
        tenant_id: &TenantId,
        idempotency_key: Uuid,
    ) -> StoreResult<Option<Uuid>>;

    /// Claim a bounded set of due timer deliveries without holding locks while
    /// the owning instances are loaded and transitioned.
    async fn claim_due_timers(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedTimer>>;

    /// Release a timer delivery lease after a transient failure before commit.
    async fn release_timer_claim(&self, timer: &ClaimedTimer) -> StoreResult<()>;

    /// Lease committed external effects for dispatch. The lease transaction is
    /// short and is closed before any network or FFI call begins.
    async fn claim_pending_effects(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        now_ms: u64,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedEffect>>;

    /// Persist a remote response exactly once and release its dispatch lease.
    async fn record_effect_response(
        &self,
        effect: &ClaimedEffect,
        response: &EffectResponse,
    ) -> StoreResult<bool>;

    /// Load durable responses awaiting normal fenced kernel application.
    async fn load_effect_responses(
        &self,
        tenant_id: &TenantId,
        limit: usize,
    ) -> StoreResult<Vec<PendingEffectResponse>>;

    /// Release a dispatch lease after transport failure without advancing state.
    async fn release_effect_claim(&self, effect: &ClaimedEffect) -> StoreResult<()>;

    /// Atomically release a dispatch lease and persist the deterministic retry
    /// attempt/due time selected by the versioned policy.
    async fn schedule_effect_retry(
        &self,
        effect: &ClaimedEffect,
        decision: RetryDecision,
        error: &str,
    ) -> StoreResult<()>;

    /// Release a previously claimed transition guard if `lease_token`
    /// still matches the live acquisition (Phase 2, F-04). A stale
    /// token — one belonging to a superseded generation — is a no-op,
    /// never touching whatever currently holds the lease. Owner strings
    /// are not sufficient identity: they are reused across concurrent
    /// operations from the same engine instance and across expiry
    /// boundaries.
    async fn release_instance_transition(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        lease_token: &str,
    ) -> StoreResult<()>;

    // ── Durable activation queue (Phase 3A scaffolding,
    // docs/todo/PHASE3-durable-activation-queue.md). Unwired store
    // primitives — nothing in the engine calls these yet. ──

    /// Idempotently record that `command` is ready to apply against
    /// `instance_id`. `command_id` is the durable dedupe identity: the
    /// same command reported ready twice enqueues once. Returns whether
    /// this call actually inserted a new row (`false` on a duplicate).
    async fn enqueue_activation(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        command_id: Uuid,
        command_kind: &str,
        command: &Command,
        available_at_ms: Option<u64>,
    ) -> StoreResult<bool>;

    /// Claim a bounded batch of ready activations (`FOR UPDATE SKIP
    /// LOCKED`, ordered by priority then arrival). At most one CLAIMED
    /// activation per instance is enforced by the store (a partial
    /// unique index in the PostgreSQL implementation), not by caller
    /// discipline — I-8.
    async fn claim_ready_activations(
        &self,
        tenant_id: &TenantId,
        owner: &str,
        limit: usize,
        lease_ms: u64,
    ) -> StoreResult<Vec<ClaimedActivation>>;

    /// Extend an already-claimed activation's deadline without changing
    /// its identity. `None` means the claim was lost (reclaimed by
    /// another owner after expiry) — the caller must stop treating the
    /// activation as theirs, not retry the renewal.
    async fn renew_activation_claim(
        &self,
        activation: &ClaimedActivation,
        lease_ms: u64,
    ) -> StoreResult<Option<ClaimedActivation>>;

    /// Release a claimed activation back to `ready`, optionally deferred
    /// until `not_before_ms`, incrementing its attempt count. A no-op if
    /// `activation`'s token no longer matches the live claim.
    async fn release_activation_to_ready(
        &self,
        activation: &ClaimedActivation,
        not_before_ms: Option<u64>,
    ) -> StoreResult<()>;

    /// Mark a claimed activation completed. Returns `false` (not an
    /// error) if the token no longer matches — an idempotent replay
    /// after an already-consumed activation is a no-op, not a conflict.
    async fn consume_activation(&self, activation: &ClaimedActivation) -> StoreResult<bool>;

    /// Move a claimed activation to `dead_lettered` with a diagnostic
    /// reason, ending its retry lifecycle without blocking other
    /// activations for the same or other instances.
    async fn dead_letter_activation(
        &self,
        activation: &ClaimedActivation,
        reason: &str,
    ) -> StoreResult<()>;

    /// Return every `claimed` activation whose persisted
    /// `claim_expires_at` has passed to `ready` (F-06's lesson applied
    /// from the start: authority is the deadline recorded at claim time,
    /// never a caller-supplied timeout). Returns the count reclaimed.
    async fn reclaim_expired_activations(&self) -> StoreResult<u32>;

    /// Read the current arrive count for a join barrier.
    async fn join_get(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        join_id: JoinId,
    ) -> StoreResult<u16>;
}

/// Immutable executable artifact and legacy migration-plan repository.
#[async_trait]
pub trait ArtifactRepository: Send + Sync {
    // ── Program store (versioned bytecode) ──

    async fn store_program(&self, version: [u8; 32], program: &CompiledProgram) -> StoreResult<()>;

    async fn load_program(&self, version: [u8; 32]) -> StoreResult<Option<CompiledProgram>>;

    async fn store_artifact(
        &self,
        artifact: &ExecutableWorkflow,
    ) -> std::result::Result<(), ArtifactStoreError>;

    async fn load_artifact(
        &self,
        hash: ArtifactHash,
    ) -> std::result::Result<Option<ExecutableWorkflow>, ArtifactStoreError>;

    /// T3 — Store a serialised WorkflowExecutionPlan keyed by BLAKE3 hash.
    /// Idempotent: ON CONFLICT DO NOTHING in Postgres; no-op if already present in MemoryStore.
    async fn store_plan(&self, plan_hash: [u8; 32], plan_json: &str) -> StoreResult<()>;

    /// T3 — Load a WorkflowExecutionPlan by BLAKE3 hash. Returns None if not found.
    async fn load_plan(&self, plan_hash: [u8; 32]) -> StoreResult<Option<String>>;
}

/// Read-only event, snapshot, journal, and payload-history access.
#[async_trait]
pub trait JournalReader: Send + Sync {
    // ── Event log (append-only) ──

    /// Append an event and return its sequence number.
    async fn read_events(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        from_seq: u64,
    ) -> StoreResult<Vec<(u64, RuntimeEvent)>>;

    /// Load the authoritative, versioned aggregate checkpoint.
    async fn load_snapshot_envelope(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
    ) -> StoreResult<Option<SnapshotEnvelope>>;

    /// Read the deterministic journal tail in revision order.
    async fn read_journal(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        after_revision: Option<u64>,
    ) -> StoreResult<Vec<JournalRecord>>;

    // ── Payload history (for PITR) ──

    async fn load_payload_version(
        &self,
        tenant_id: &TenantId,
        instance_id: Uuid,
        hash: &[u8; 32],
    ) -> StoreResult<Option<String>>;
}

/// Administrative, tenant-directory, and projection/catalogue access.
#[async_trait]
pub trait AdminProjectionStore: Send + Sync {
    // ── Template catalog ──

    async fn store_template(
        &self,
        name: &str,
        version: u32,
        plan_hash: [u8; 32],
        dsl_body: &str,
    ) -> StoreResult<()>;

    async fn load_template_version(
        &self,
        name: &str,
        version: u32,
    ) -> StoreResult<Option<(String, [u8; 32])>>;

    async fn load_latest_template_version(
        &self,
        name: &str,
    ) -> StoreResult<Option<(u32, String, [u8; 32])>>;

    async fn list_templates(&self) -> StoreResult<Vec<TemplateSummary>>;

    /// Lightweight readiness probe for the backing store.
    async fn health_check(&self) -> StoreResult<()>;

    // ── Tenant directory ──

    /// Register a tenant on first use. Idempotent — safe to call on every
    /// process start. The tenants table has no RLS so the scheduler can
    /// enumerate all tenants without a tenant context set.
    async fn ensure_tenant(&self, tenant_id: &TenantId) -> StoreResult<()>;

    /// List all known tenant IDs. Used by the scheduler to enumerate tenants
    /// for per-tenant tick batches.
    async fn list_tenants(&self) -> StoreResult<Vec<String>>;

    /// List tenant IDs assigned to a specific pool. Returns an empty vec if the
    /// pool doesn't exist or has no tenants. Used by bpmn-controller to query
    /// pool membership.
    async fn list_tenants_in_pool(&self, pool_id: &str) -> StoreResult<Vec<String>>;

    // ── Designer sessions (EOP-SAGE-REPL-BPMN-001 T1) ──
    // Tenant-scoped, append-only event log. NOTE: unlike the template
    // catalog (which predates tenancy on this surface), sessions carry
    // tenant_id from birth; RLS alignment for the postgres tables rides
    // the T5 deployment review.

    /// Create a session with its seed source as Revision event seq 0.
    /// `id` is caller-generated (deterministic-id discipline).
    async fn create_design_session(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
        name: &str,
        dsl_source: &str,
    ) -> StoreResult<()>;

    async fn load_design_session(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
    ) -> StoreResult<Option<DesignSessionRecord>>;

    async fn list_design_sessions(
        &self,
        tenant_id: &TenantId,
    ) -> StoreResult<Vec<DesignSessionSummary>>;

    /// Append one event; returns the assigned sequence number.
    async fn append_design_session_event(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
        kind: &DesignSessionEventKind,
    ) -> StoreResult<u64>;

    /// Mark saved-as-template, recording the template pin on the session.
    async fn mark_design_session_saved(
        &self,
        tenant_id: &TenantId,
        id: Uuid,
        template_name: &str,
        template_version: u32,
        plan_hash: [u8; 32],
    ) -> StoreResult<()>;

    // ── Dev-capture sessions (G1.2, DIR-004 Phase 1/2) ──
    // Adam-only, always-compiled (see `utterance_engine::dev_capture`'s
    // module doc). Session-id-scoped, not tenant-scoped — dev capture is
    // not a tenant-facing feature.

    /// Open a dev-capture session with its consent statement, stated once
    /// at session start. Refuses if a session with this id is already
    /// open — a later call must not be able to silently swap the
    /// recorded consent statement for one already capturing interactions.
    async fn open_dev_capture_session(
        &self,
        session_id: &str,
        consent_statement_timestamp: &str,
    ) -> StoreResult<()>;

    /// Append one captured interaction's full closure (opaque JSON —
    /// `utterance_engine::dev_capture::DevSessionRecord`, serialized by
    /// the caller). Returns the assigned sequence number.
    async fn append_dev_capture_record(
        &self,
        session_id: &str,
        record_json: String,
    ) -> StoreResult<u64>;

    /// Load a dev-capture session and every record captured so far.
    async fn load_dev_capture_session(
        &self,
        session_id: &str,
    ) -> StoreResult<Option<DevCaptureSessionRecord>>;
}

/// Composite capability object used where an engine needs all four store surfaces.
pub trait WorkflowStore:
    RuntimeStore + ArtifactRepository + JournalReader + AdminProjectionStore
{
}

impl<T> WorkflowStore for T where
    T: RuntimeStore + ArtifactRepository + JournalReader + AdminProjectionStore
{
}

/// Test and migration fixture helper that still exercises the sole writable
/// store boundary. Production start paths should add the root fiber and start
/// event to the same transition.
pub async fn commit_initial_snapshot(
    store: &dyn WorkflowStore,
    instance: ProcessInstance,
) -> std::result::Result<CommitOutcome, CommitError> {
    let tenant_id = TenantId::new(instance.tenant_id.clone())
        .map_err(|error| CommitError::Integrity(error.to_string()))?;
    let claim = Claim::new(tenant_id, instance.instance_id, 0, 0, "");
    store
        .commit_transition(&claim, &TransitionBuilder::new(instance).build())
        .await
}

/// Commit a complete fixture/admin snapshot through the fenced write path.
pub async fn commit_snapshot(
    store: &dyn WorkflowStore,
    owner: &str,
    instance: ProcessInstance,
) -> std::result::Result<CommitOutcome, CommitError> {
    let tenant_id = TenantId::new(instance.tenant_id.clone())
        .map_err(|error| CommitError::Integrity(error.to_string()))?;
    let instance_id = instance.instance_id;
    let claim = store
        .claim_instance_for_transition(&tenant_id, instance_id, owner, 30_000)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?
        .ok_or(CommitError::Conflict)?;
    let lease_token = claim.lease_token().to_string();
    let outcome = store
        .commit_transition(&claim, &TransitionBuilder::new(instance).build())
        .await?;
    store
        .release_instance_transition(&tenant_id, instance_id, &lease_token)
        .await
        .map_err(|error| CommitError::Unavailable(error.to_string()))?;
    Ok(outcome)
}

#[derive(Debug, Clone)]
pub enum TickOperation {
    SaveFiber {
        fiber: Fiber,
    },
    DeleteFiber {
        fiber_id: Uuid,
    },
    DeleteAllFibers,
    JoinArrive {
        join_id: JoinId,
    },
    JoinReset {
        join_id: JoinId,
    },
    EnqueueJob {
        job: JobActivation,
    },
    AckJob {
        job_key: String,
    },
    CancelJobsForInstance,
    AppendEvent {
        event: RuntimeEvent,
    },
    SaveIncident {
        incident: Incident,
    },
    SaveInstance {
        instance: ProcessInstance,
    },
    UpdateInstanceState {
        state: ProcessState,
    },
    ReleaseBufferedMessageClaim {
        message: ClaimedBufferedMessage,
    },
    ConsumeBufferedMessage {
        message: ClaimedBufferedMessage,
    },
    InsertPendingInvocation {
        pending: crate::pending::PendingInvocation,
    },
    InsertOutbox {
        id: Uuid,
        target_domain: String,
        target_endpoint: String,
        payload: Vec<u8>,
        idempotency_key: Uuid,
        callout_id: Uuid,
    },
    TakePendingInvocation {
        execution_id: Uuid,
    },
    StartChild {
        child: Box<ChildStart>,
    },
    ScheduleTimer {
        effect: DurableEffect,
    },
    TimerMutation {
        mutation: TimerMutation,
    },
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("TakePendingInvocation: already consumed")]
pub struct AlreadyConsumedError;

/// T4 compatibility bridge. T7 deletes `TickOperation` and this conversion
/// after every producer builds `Transition` directly.
#[doc(hidden)]
pub fn transition_from_tick_ops(current: &ProcessInstance, ops: &[TickOperation]) -> Transition {
    let mut snapshot = current.clone();
    for op in ops {
        match op {
            TickOperation::SaveInstance { instance } => snapshot = instance.clone(),
            TickOperation::UpdateInstanceState { state } => snapshot.state = state.clone(),
            _ => {}
        }
    }

    let mut builder = TransitionBuilder::new(snapshot);
    let mut delete_all_fibers = false;
    let delete_all_joins = false;
    let mut cancel_jobs = false;
    for op in ops {
        builder = match op {
            TickOperation::SaveFiber { fiber } => builder.upsert_fiber(fiber.clone()),
            TickOperation::DeleteFiber { fiber_id } => builder.delete_fiber(*fiber_id),
            TickOperation::DeleteAllFibers => {
                delete_all_fibers = true;
                builder
            }
            TickOperation::JoinArrive { join_id } => {
                builder.join_mutation(JoinMutation::Arrive(*join_id))
            }
            TickOperation::JoinReset { join_id } => {
                builder.join_mutation(JoinMutation::Reset(*join_id))
            }
            TickOperation::EnqueueJob { job } => builder.enqueue_job(job.clone()),
            TickOperation::AckJob { job_key } => builder.ack_job(job_key.clone()),
            TickOperation::CancelJobsForInstance => {
                cancel_jobs = true;
                builder
            }
            TickOperation::AppendEvent { event } => builder.event(event.clone()),
            TickOperation::SaveIncident { incident } => builder.incident(incident.clone()),
            TickOperation::ReleaseBufferedMessageClaim { message } => {
                builder.buffered_message(BufferedMessageMutation::Release(message.clone()))
            }
            TickOperation::ConsumeBufferedMessage { message } => {
                builder.buffered_message(BufferedMessageMutation::Consume(message.clone()))
            }
            TickOperation::InsertPendingInvocation { pending } => {
                let identity = PendingInvocationIdentity {
                    callout_id: pending.callout_id,
                    process_instance_id: pending.process_instance_id,
                    node_id: pending.node_id.clone(),
                    target_domain: pending.target_domain.clone(),
                    verb_id: pending.verb_id.clone(),
                    idempotency_key: pending.idempotency_key,
                };
                let timing = PendingInvocationTiming {
                    execution_id: pending.execution_id,
                    submitted_at: pending.submitted_at.timestamp_millis(),
                    ack_received_at: pending
                        .ack_received_at
                        .map(|value| value.timestamp_millis()),
                    timeout_at: pending.timeout_at.map(|value| value.timestamp_millis()),
                };
                builder.pending_invocation(PendingInvocationWrite::new(identity, timing))
            }
            TickOperation::InsertOutbox {
                id,
                target_domain,
                target_endpoint,
                payload,
                idempotency_key,
                callout_id,
            } => builder.outbox(OutboxWrite::new(
                *id,
                target_domain.clone(),
                target_endpoint.clone(),
                payload.clone(),
                *idempotency_key,
                *callout_id,
            )),
            TickOperation::TakePendingInvocation { execution_id } => {
                builder.take_pending(*execution_id)
            }
            TickOperation::StartChild { child } => builder.child_start((**child).clone()),
            TickOperation::ScheduleTimer { effect } => builder.effect(effect.clone()),
            TickOperation::TimerMutation { mutation } => builder.timer_mutation(mutation.clone()),
            TickOperation::SaveInstance { .. } | TickOperation::UpdateInstanceState { .. } => {
                builder
            }
        };
    }
    if delete_all_fibers || delete_all_joins || cancel_jobs {
        builder = builder.terminal_cleanup(TerminalCleanup::new(
            delete_all_fibers,
            delete_all_joins,
            cancel_jobs,
        ));
    }
    builder.build()
}
