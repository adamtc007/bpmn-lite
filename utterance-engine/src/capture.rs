//! Q9-GATED USER CAPTURE — feature `q9-capture` only. This entire module
//! is compiled ONLY when that feature is enabled at build time (see
//! `lib.rs`'s `#[cfg(feature = "q9-capture")] pub mod capture;`), which
//! is off by default and absent from every documented build/release
//! command in this repo (`scripts/check-q9-capture-gate.sh` enforces
//! that mechanically — DIR-004 Phase 1.2).
//!
//! DIR-004 Phase 1 ruling (structural separation, Option B): a
//! pre-charter build must not be ABLE to contain a live user-capture
//! path, not merely configured to leave it off at runtime. Before this
//! split, `CapturePipeline::off()` was reachable in every build — a
//! single application-code edit (swap `off()` for `on_under_charter()`)
//! would have silently activated real capture with no build-system
//! signal. Now that edit additionally requires `--features q9-capture`,
//! a visible, CI-checked build flag — the gate is a compile-time fact,
//! not a runtime convention. `utterance_engine::dev_capture` (always
//! compiled, unconditionally available to Adam's own testing) is the
//! deliberately DISTINCT module this split produces — no shared record
//! type, no shared store, per Adam's ruling.
//!
//! Two record streams exist and must not be conflated:
//! - The SESSION event log (T1 store) is operational data — the
//!   designer's own dialogue/undo history. Not this module.
//! - CAPTURE is corpus accrual for evaluation/training/audit — this
//!   module — and is gated by the Q9 charter (D17): until the charter
//!   is ratified and its reference is supplied, every capture call is
//!   SUPPRESSED (dropped, unrecoverable, deliberately).
//!
//! Dataset separation (charter deliverable): evaluation, training, and
//! audit are PHYSICALLY distinct sinks — one event goes to exactly one.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::policy::DecisionRecord;

/// The ratified charter reference (EOP-GOV-Q9-CHARTER-001, ratified by
/// Adam 2026-08-06). `under_ratified_charter` accepts EXACTLY this
/// string — a stale or amended reference is refused, per the charter's
/// §10.4 (capture under a stale reference is a defect). Amending the
/// charter means bumping this constant in the same change.
pub const RATIFIED_CHARTER_REF: &str = "Q9-CHARTER-001@v1.0";

/// Charter-mandated dataset separation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DatasetClass {
    Evaluation,
    Training,
    Audit,
}

/// One captured interaction: the full I28 closure plus the raw
/// utterance (permitted-fields/redaction rules are charter deliverables
/// applied at the sink when capture goes live).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureEvent {
    pub raw_utterance: String,
    pub record: DecisionRecord,
    pub dataset: DatasetClass,
}

/// One complete semantic-game turn. This stream is schema-separated from the
/// legacy decision-record stream so a corpus builder cannot silently treat a
/// thin legacy closure as a game-level training example.
#[derive(Clone, Debug, Serialize)]
pub struct GameCaptureEvent {
    raw_utterance: String,
    record: semantic_decision_contracts::GameTurnRecord,
    dataset: DatasetClass,
}

impl GameCaptureEvent {
    pub const MAX_RAW_UTTERANCE_BYTES: usize = 64 * 1024;

    pub fn new(
        raw_utterance: String,
        record: semantic_decision_contracts::GameTurnRecord,
        dataset: DatasetClass,
    ) -> anyhow::Result<Self> {
        if raw_utterance.len() > Self::MAX_RAW_UTTERANCE_BYTES {
            anyhow::bail!("game capture utterance exceeds the product byte limit");
        }
        Ok(Self {
            raw_utterance,
            record,
            dataset,
        })
    }

    pub fn raw_utterance(&self) -> &str {
        &self.raw_utterance
    }

    pub fn record(&self) -> &semantic_decision_contracts::GameTurnRecord {
        &self.record
    }

    pub fn dataset(&self) -> DatasetClass {
        self.dataset
    }
}

impl<'de> Deserialize<'de> for GameCaptureEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            raw_utterance: String,
            record: semantic_decision_contracts::GameTurnRecord,
            dataset: DatasetClass,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.raw_utterance, wire.record, wire.dataset).map_err(serde::de::Error::custom)
    }
}

/// Operator adjudication of one captured turn (charter §6 — the label
/// source). Variants are structured so an outcome cannot be recorded
/// without the data that makes it a label: a correction without the
/// correct candidate is unrepresentable, not merely invalid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AdjudicationOutcome {
    /// The served top candidate was what the operator meant.
    Accepted,
    /// The served top candidate was wrong; this one was meant.
    Corrected { correct_candidate_id: String },
    /// The operator picked a candidate from the served list themselves.
    ExplicitlySelected { candidate_id: String },
    /// The operator walked away — no candidate was right or wanted.
    Abandoned,
}

/// One adjudication, linked to its captured turn by the decision-record
/// hash. Lives in its own ledger (`adjudications.jsonl`), NOT in a
/// dataset class: the turn stays Evaluation-class (§3); corpus builds
/// join this ledger against captured events, and only then do
/// corrections enter Training with `corrected_user` provenance (§6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjudicationEvent {
    pub decision_record_hash: String,
    #[serde(flatten)]
    pub outcome: AdjudicationOutcome,
}

/// Structured game-level judgement linked by the canonical turn-record hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameAdjudicationEvent {
    adjudication: semantic_decision_contracts::GameTurnAdjudication,
}

impl GameAdjudicationEvent {
    pub fn new(adjudication: semantic_decision_contracts::GameTurnAdjudication) -> Self {
        Self { adjudication }
    }

    pub fn adjudication(&self) -> &semantic_decision_contracts::GameTurnAdjudication {
        &self.adjudication
    }
}

/// What happened to a capture call — the caller always learns the
/// truth; suppression is visible, never silent success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// Switch OFF (no ratified charter): event DROPPED by design.
    SuppressedNoCharter,
    /// Stored under the named dataset class.
    Stored(DatasetClass),
    /// Durable persistence failed: the event was NOT stored anywhere
    /// (memory included) and the caller is told so — a persist failure
    /// is never a silent success and never a silent drop.
    PersistFailed(DatasetClass),
}

/// The pipeline. `off()` is the only zero-argument constructor; turning
/// capture ON requires the ratified charter's reference — a mechanism
/// gate, not a boolean. Note: even under `q9-capture`, this type alone
/// does not make capture live anywhere — no callsite in this workspace
/// calls `on_under_charter` (grep before assuming otherwise).
pub struct CapturePipeline {
    /// `None` = switch OFF. `Some(charter_ref)` = ON under that charter.
    charter: Option<String>,
    /// Physically separate sinks (in-memory view; when `durable_dir` is
    /// set, every stored event is ALSO appended to that class's JSONL
    /// file before the in-memory push — persist-first, fail closed).
    sinks: BTreeMap<DatasetClass, Vec<CaptureEvent>>,
    /// Complete game-turn records remain physically and structurally separate
    /// from legacy capture events.
    game_sinks: BTreeMap<DatasetClass, Vec<GameCaptureEvent>>,
    /// `Some` = live charter-governed capture writes one append-only
    /// JSONL file per dataset class under this directory
    /// (`evaluation.jsonl` / `training.jsonl` / `audit.jsonl`) —
    /// physical separation on disk, mirroring the in-memory sinks.
    durable_dir: Option<PathBuf>,
}

/// One durable JSONL line: the event plus its charter lineage
/// (charter §7 — every stored event names the charter it was captured
/// under).
#[derive(Serialize, Deserialize)]
pub(crate) struct DurableCaptureLine {
    pub(crate) charter: String,
    pub(crate) captured_at_epoch_s: u64,
    pub(crate) event: CaptureEvent,
}

impl CapturePipeline {
    /// The default and only pre-charter state.
    pub fn off() -> Self {
        CapturePipeline {
            charter: None,
            sinks: BTreeMap::new(),
            game_sinks: BTreeMap::new(),
            durable_dir: None,
        }
    }

    /// Live, durable, charter-governed capture — the ONLY public way to
    /// turn capture on (EOP-GOV-Q9-CHARTER-001 §10.3, ratified
    /// 2026-08-06). The reference must equal [`RATIFIED_CHARTER_REF`]
    /// exactly: an empty, stale, or amended reference is refused (§10.4
    /// — a mechanism gate, not a string-presence check). The capture
    /// directory is created up front; failure to create it refuses
    /// construction (fail closed at startup, not at first event).
    pub fn under_ratified_charter(charter_ref: &str, capture_dir: &Path) -> anyhow::Result<Self> {
        let r = charter_ref.trim();
        if r != RATIFIED_CHARTER_REF {
            return Err(anyhow::anyhow!(
                "capture refused: charter reference {r:?} is not the ratified \
                 {RATIFIED_CHARTER_REF:?} (EOP-GOV-Q9-CHARTER-001 §10.4)"
            ));
        }
        std::fs::create_dir_all(capture_dir).map_err(|e| {
            anyhow::anyhow!(
                "capture refused: cannot create capture directory {}: {e}",
                capture_dir.display()
            )
        })?;
        Ok(CapturePipeline {
            charter: Some(r.to_owned()),
            sinks: BTreeMap::new(),
            game_sinks: BTreeMap::new(),
            durable_dir: Some(capture_dir.to_owned()),
        })
    }

    /// Turning capture on REQUIRES the ratified charter reference
    /// (D17). Empty/whitespace refs are refused — the gate cannot be
    /// satisfied by a placeholder.
    ///
    /// Deliberately `pub(crate)`, narrower than `off()`/`capture()`:
    /// no callsite outside this crate's own tests may flip capture on.
    /// External consumers (`bpmn-lite-server-designer`) construct only
    /// via `off()` — see this module's doc comment.
    #[cfg(test)]
    pub(crate) fn on_under_charter(charter_ref: &str) -> anyhow::Result<Self> {
        let r = charter_ref.trim();
        if r.is_empty() {
            return Err(anyhow::anyhow!(
                "capture cannot be enabled without a ratified Q9 charter reference (D17)"
            ));
        }
        Ok(CapturePipeline {
            charter: Some(r.to_owned()),
            sinks: BTreeMap::new(),
            game_sinks: BTreeMap::new(),
            durable_dir: None,
        })
    }

    /// Capture one interaction. OFF → the event is dropped and the
    /// caller told so. ON → stored under exactly one dataset class.
    pub fn capture(&mut self, event: CaptureEvent) -> CaptureOutcome {
        match self.charter.clone() {
            None => CaptureOutcome::SuppressedNoCharter,
            Some(charter) => {
                let class = event.dataset;
                // Persist FIRST: an event that cannot be durably written
                // is not stored anywhere and the caller is told so.
                if let Some(dir) = &self.durable_dir {
                    if let Err(e) = append_durable(dir, &charter, &event) {
                        tracing::error!(
                            class = ?class,
                            "q9 capture persist failed (event NOT stored): {e}"
                        );
                        return CaptureOutcome::PersistFailed(class);
                    }
                }
                self.sinks.entry(class).or_default().push(event);
                CaptureOutcome::Stored(class)
            }
        }
    }

    /// Read one dataset — the separation surface (a Training reader can
    /// never see Evaluation events and vice versa).
    #[cfg(test)]
    pub(crate) fn dataset(&self, class: DatasetClass) -> &[CaptureEvent] {
        self.sinks.get(&class).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Capture a complete game turn. The record's injected observation time is
    /// also the durable envelope time; this core path never reads ambient time.
    pub fn capture_game(&mut self, event: GameCaptureEvent) -> CaptureOutcome {
        match self.charter.clone() {
            None => CaptureOutcome::SuppressedNoCharter,
            Some(charter) => {
                let class = event.dataset();
                if let Some(dir) = &self.durable_dir {
                    if let Err(error) = append_durable_game(dir, &charter, &event) {
                        tracing::error!(
                            class = ?class,
                            "q9 game capture persist failed (event NOT stored): {error}"
                        );
                        return CaptureOutcome::PersistFailed(class);
                    }
                }
                self.game_sinks.entry(class).or_default().push(event);
                CaptureOutcome::Stored(class)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn game_dataset(&self, class: DatasetClass) -> &[GameCaptureEvent] {
        self.game_sinks
            .get(&class)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Record one operator adjudication (charter §6). Same gate and
    /// same honesty as `capture`: OFF → visibly suppressed; a failed
    /// durable write records nothing and says so. An empty or
    /// whitespace decision-record hash, or an empty corrected/selected
    /// candidate id, is refused as `PersistFailed` — a label that
    /// cannot be joined back to its turn is not a label.
    pub fn adjudicate(&mut self, event: AdjudicationEvent) -> AdjudicationRecordOutcome {
        let Some(charter) = self.charter.clone() else {
            return AdjudicationRecordOutcome::SuppressedNoCharter;
        };
        if event.decision_record_hash.trim().is_empty() {
            return AdjudicationRecordOutcome::Refused(
                "adjudication refused: empty decision_record_hash cannot be joined to a turn"
                    .into(),
            );
        }
        let named_candidate = match &event.outcome {
            AdjudicationOutcome::Corrected {
                correct_candidate_id,
            } => Some(correct_candidate_id),
            AdjudicationOutcome::ExplicitlySelected { candidate_id } => Some(candidate_id),
            AdjudicationOutcome::Accepted | AdjudicationOutcome::Abandoned => None,
        };
        if named_candidate.is_some_and(|c| c.trim().is_empty()) {
            return AdjudicationRecordOutcome::Refused(
                "adjudication refused: corrected/selected outcome names an empty candidate id"
                    .into(),
            );
        }
        let Some(dir) = &self.durable_dir else {
            return AdjudicationRecordOutcome::Refused(
                "adjudication refused: no durable ledger (in-memory pipelines take no labels)"
                    .into(),
            );
        };
        match append_adjudication(dir, &charter, &event) {
            Ok(()) => AdjudicationRecordOutcome::Stored,
            Err(e) => {
                tracing::error!("q9 adjudication persist failed (label NOT stored): {e}");
                AdjudicationRecordOutcome::PersistFailed
            }
        }
    }

    /// Record a validated game-level judgement in its own append-only ledger.
    pub fn adjudicate_game(&mut self, event: GameAdjudicationEvent) -> AdjudicationRecordOutcome {
        let Some(charter) = self.charter.clone() else {
            return AdjudicationRecordOutcome::SuppressedNoCharter;
        };
        let Some(dir) = &self.durable_dir else {
            return AdjudicationRecordOutcome::Refused(
                "game adjudication refused: no durable ledger".into(),
            );
        };
        let record =
            match find_game_record(&self.game_sinks, dir, event.adjudication().record_hash()) {
                Ok(Some(record)) => record,
                Ok(None) => {
                    return AdjudicationRecordOutcome::Refused(
                        "game adjudication refused: record hash does not match a captured turn"
                            .into(),
                    );
                }
                Err(error) => {
                    tracing::error!("q9 game adjudication lookup failed: {error}");
                    return AdjudicationRecordOutcome::PersistFailed;
                }
            };
        if let Err(error) = event.adjudication().validate_for_record(&record) {
            return AdjudicationRecordOutcome::Refused(format!(
                "game adjudication refused: {error}"
            ));
        }
        match append_game_adjudication(dir, &charter, &event) {
            Ok(()) => AdjudicationRecordOutcome::Stored,
            Err(error) => {
                tracing::error!(
                    "q9 game adjudication persist failed (judgement NOT stored): {error}"
                );
                AdjudicationRecordOutcome::PersistFailed
            }
        }
    }
}

const MAX_GAME_CAPTURE_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_GAME_CAPTURE_LINES: usize = 100_000;

fn find_game_record(
    memory: &BTreeMap<DatasetClass, Vec<GameCaptureEvent>>,
    dir: &Path,
    record_hash: &semantic_decision_contracts::GameTurnRecordHash,
) -> anyhow::Result<Option<semantic_decision_contracts::GameTurnRecord>> {
    if let Some(record) = memory
        .values()
        .flat_map(|events| events.iter())
        .find(|event| event.record.record_hash() == record_hash)
        .map(|event| event.record.clone())
    {
        return Ok(Some(record));
    }
    for class in [
        DatasetClass::Evaluation,
        DatasetClass::Training,
        DatasetClass::Audit,
    ] {
        let path = dir.join(game_class_file_name(class));
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_GAME_CAPTURE_FILE_BYTES {
            anyhow::bail!(
                "game capture file {} exceeds {} bytes",
                path.display(),
                MAX_GAME_CAPTURE_FILE_BYTES
            );
        }
        let content = std::fs::read_to_string(&path)?;
        for (index, line) in content.lines().enumerate() {
            if index >= MAX_GAME_CAPTURE_LINES {
                anyhow::bail!(
                    "game capture file {} exceeds {} lines",
                    path.display(),
                    MAX_GAME_CAPTURE_LINES
                );
            }
            let captured: DurableGameCaptureLine = serde_json::from_str(line)?;
            if captured.event.record().record_hash() == record_hash {
                return Ok(Some(captured.event.record().clone()));
            }
        }
    }
    Ok(None)
}

/// Outcome of recording an adjudication — same visibility contract as
/// [`CaptureOutcome`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdjudicationRecordOutcome {
    SuppressedNoCharter,
    Stored,
    Refused(String),
    PersistFailed,
}

/// Per-class file name — the on-disk half of the physical dataset
/// separation. One class, one file; never a shared stream.
pub(crate) fn class_file_name(class: DatasetClass) -> &'static str {
    match class {
        DatasetClass::Evaluation => "evaluation.jsonl",
        DatasetClass::Training => "training.jsonl",
        DatasetClass::Audit => "audit.jsonl",
    }
}

pub(crate) fn game_class_file_name(class: DatasetClass) -> &'static str {
    match class {
        DatasetClass::Evaluation => "evaluation.game-turns.jsonl",
        DatasetClass::Training => "training.game-turns.jsonl",
        DatasetClass::Audit => "audit.game-turns.jsonl",
    }
}

fn append_durable(dir: &Path, charter: &str, event: &CaptureEvent) -> anyhow::Result<()> {
    let line = DurableCaptureLine {
        charter: charter.to_owned(),
        captured_at_epoch_s: epoch_s(),
        event: event.clone(),
    };
    append_jsonl(
        &dir.join(class_file_name(event.dataset)),
        &serde_json::to_string(&line)?,
    )
}

#[derive(Serialize, Deserialize)]
pub(crate) struct DurableGameCaptureLine {
    pub(crate) charter: String,
    pub(crate) captured_at_epoch_ms: u64,
    pub(crate) event: GameCaptureEvent,
}

fn append_durable_game(dir: &Path, charter: &str, event: &GameCaptureEvent) -> anyhow::Result<()> {
    let line = DurableGameCaptureLine {
        charter: charter.to_owned(),
        captured_at_epoch_ms: event.record().observed_at_epoch_ms(),
        event: event.clone(),
    };
    append_jsonl(
        &dir.join(game_class_file_name(event.dataset())),
        &serde_json::to_string(&line)?,
    )
}

/// One durable adjudication-ledger line (charter §6/§7 lineage).
#[derive(Serialize, Deserialize)]
pub(crate) struct DurableAdjudicationLine {
    pub(crate) charter: String,
    pub(crate) adjudicated_at_epoch_s: u64,
    #[serde(flatten)]
    pub(crate) event: AdjudicationEvent,
}

/// The label ledger — deliberately NOT a dataset-class file (§3: one
/// event, one class; labels are a join surface, not a dataset).
pub(crate) const ADJUDICATION_LEDGER: &str = "adjudications.jsonl";
pub(crate) const GAME_ADJUDICATION_LEDGER: &str = "game-adjudications.jsonl";

fn append_adjudication(dir: &Path, charter: &str, event: &AdjudicationEvent) -> anyhow::Result<()> {
    let line = DurableAdjudicationLine {
        charter: charter.to_owned(),
        adjudicated_at_epoch_s: epoch_s(),
        event: event.clone(),
    };
    append_jsonl(
        &dir.join(ADJUDICATION_LEDGER),
        &serde_json::to_string(&line)?,
    )
}

#[derive(Serialize, Deserialize)]
pub(crate) struct DurableGameAdjudicationLine {
    pub(crate) charter: String,
    pub(crate) adjudicated_at_epoch_ms: u64,
    pub(crate) event: GameAdjudicationEvent,
}

fn append_game_adjudication(
    dir: &Path,
    charter: &str,
    event: &GameAdjudicationEvent,
) -> anyhow::Result<()> {
    let line = DurableGameAdjudicationLine {
        charter: charter.to_owned(),
        adjudicated_at_epoch_ms: event.adjudication().adjudicated_at_epoch_ms(),
        event: event.clone(),
    };
    append_jsonl(
        &dir.join(GAME_ADJUDICATION_LEDGER),
        &serde_json::to_string(&line)?,
    )
}

fn epoch_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn append_jsonl(path: &Path, serialized: &str) -> anyhow::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(serialized.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{build_board, EmptyUniverse, PolicyFilter};
    use crate::bpmn_board::{
        build_bpmn_design_position, build_bpmn_semantic_board, capture_bpmn_game_turn,
        decide_bpmn_game_disposition, finalize_bpmn_move_evidence, project_bpmn_attempt_history,
        update_bpmn_design_belief,
    };
    use crate::policy::decide;
    use crate::policy::DispositionConfig;
    use crate::retrieval::{LexicalTier0, Tier0Retriever};
    use designer_graph::board_candidate::{LegalityOracle, OperationKind, ProductionId};
    use designer_graph::schema::DesignerDag;
    use semantic_decision_contracts::{
        DesignFocus, DesignTurnId, EvidenceLane, FocusAbsenceReason, GameSessionId, GameTurnAnswer,
        GameTurnAnswerAbsenceReason, GameTurnAttempt, GameTurnCompilerResult, GameTurnJudgement,
        IntendedMove, MoveAttemptId,
    };

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

    fn one_event() -> CaptureEvent {
        let board = build_board(
            &AllLegal,
            None,
            None,
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .unwrap();
        let ev = LexicalTier0.retrieve("connect the nodes", &board).unwrap();
        let (_, record) = decide(
            &DispositionConfig::shadow_v1(),
            &board,
            &ev,
            &crate::context::minimal("pack.none", "g-test"),
        )
        .unwrap();
        CaptureEvent {
            raw_utterance: "connect the nodes".into(),
            record,
            dataset: DatasetClass::Evaluation,
        }
    }

    fn one_game_event() -> GameCaptureEvent {
        let dag = DesignerDag::new("capture-game");
        let revision = "a".repeat(64);
        let board =
            build_bpmn_semantic_board(&dag, None, &revision, &PolicyFilter::default()).unwrap();
        let (history_hash, history) = project_bpmn_attempt_history(&[]).unwrap();
        let position = build_bpmn_design_position(
            &dag,
            &board,
            &revision,
            &"b".repeat(64),
            "compiler-profile-v1",
            &history_hash,
            DesignFocus::absent(FocusAbsenceReason::NotProvided),
            None,
        )
        .unwrap();
        let evidence = crate::retrieval::LexicalTier0
            .retrieve("show options", &board)
            .unwrap();
        let evidence = finalize_bpmn_move_evidence(
            &board,
            &position,
            "show options",
            evidence,
            EvidenceLane::Lexical,
            vec!["test.capture".into()],
            &history,
        )
        .unwrap();
        let belief =
            update_bpmn_design_belief(&dag, &position, &evidence.move_evidence, &history, None)
                .unwrap();
        let disposition = decide_bpmn_game_disposition(
            &board,
            &position,
            &evidence.move_evidence,
            &belief,
            "show options",
            MoveAttemptId::new("capture-attempt").unwrap(),
            &history,
        )
        .unwrap();
        let attempt = disposition
            .attempt_receipt()
            .cloned()
            .map_or_else(GameTurnAttempt::not_attempted, GameTurnAttempt::terminal);
        let record = capture_bpmn_game_turn(
            GameSessionId::new("capture-session").unwrap(),
            DesignTurnId::new("capture-turn").unwrap(),
            1,
            1_786_128_020_000,
            &board,
            position,
            evidence.move_evidence,
            belief,
            disposition,
            "show options",
            GameTurnAnswer::not_observed(GameTurnAnswerAbsenceReason::NotRequested),
            None,
            None,
            attempt,
            GameTurnCompilerResult::not_requested(),
            Vec::new(),
        )
        .unwrap();
        GameCaptureEvent::new("show options".into(), record, DatasetClass::Evaluation).unwrap()
    }

    /// D17 red: switch OFF drops the event — visibly, unrecoverably.
    /// Charter gate red: empty ref refused.
    #[test]
    fn off_drops_and_charterless_on_is_refused() {
        let mut p = CapturePipeline::off();
        assert_eq!(p.capture(one_event()), CaptureOutcome::SuppressedNoCharter);
        assert!(
            p.dataset(DatasetClass::Evaluation).is_empty(),
            "nothing may persist"
        );
        assert!(CapturePipeline::on_under_charter("   ").is_err());
    }

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("q9cap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// §10.4 red/green: the public constructor is an exact-reference
    /// mechanism gate — stale, amended, or arbitrary non-empty strings
    /// are refused; only the ratified reference constructs.
    #[test]
    fn only_the_exact_ratified_charter_reference_constructs() {
        let dir = scratch_dir("ref");
        assert!(CapturePipeline::under_ratified_charter("", &dir).is_err());
        assert!(CapturePipeline::under_ratified_charter("Q9-CHARTER-001@v0.9", &dir).is_err());
        assert!(
            CapturePipeline::under_ratified_charter("some ratified charter", &dir).is_err(),
            "non-empty is not enough — the gate is the exact reference, not string presence"
        );
        assert!(CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Charter §3/§5/§7: a stored event lands in EXACTLY one class file,
    /// carries its charter lineage, and round-trips; the other class
    /// files do not even exist.
    #[test]
    fn durable_capture_writes_exactly_one_class_file_with_lineage() {
        let dir = scratch_dir("durable");
        let mut p = CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &dir).unwrap();
        let event = one_event();
        assert_eq!(
            p.capture(event.clone()),
            CaptureOutcome::Stored(DatasetClass::Evaluation)
        );

        let eval = std::fs::read_to_string(dir.join("evaluation.jsonl")).unwrap();
        let lines: Vec<&str> = eval.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed: DurableCaptureLine = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.charter, RATIFIED_CHARTER_REF);
        assert_eq!(parsed.event.raw_utterance, event.raw_utterance);
        assert_eq!(parsed.event.dataset, DatasetClass::Evaluation);

        assert!(
            !dir.join("training.jsonl").exists(),
            "training sink must not exist"
        );
        assert!(
            !dir.join("audit.jsonl").exists(),
            "audit sink must not exist"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn game_capture_and_adjudication_are_separate_restart_safe_ledgers() {
        let dir = scratch_dir("game-durable");
        let event = one_game_event();
        let record_hash = event.record().record_hash().clone();
        let adjudication = semantic_decision_contracts::GameTurnAdjudication::new(
            event.record(),
            "operator-1",
            1_786_128_021_000,
            GameTurnJudgement::AccidentalMove,
            IntendedMove::None,
            None,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            None,
        )
        .unwrap();
        let mut pipeline =
            CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &dir).unwrap();
        assert_eq!(
            pipeline.capture_game(event),
            CaptureOutcome::Stored(DatasetClass::Evaluation)
        );
        assert_eq!(pipeline.game_dataset(DatasetClass::Evaluation).len(), 1);
        assert!(!dir.join("evaluation.jsonl").exists());
        let game_file = std::fs::read_to_string(dir.join("evaluation.game-turns.jsonl")).unwrap();
        let captured: DurableGameCaptureLine = serde_json::from_str(game_file.trim()).unwrap();
        assert_eq!(captured.captured_at_epoch_ms, 1_786_128_020_000);
        assert_eq!(captured.event.record().record_hash(), &record_hash);

        // A fresh pipeline proves that adjudication joins against the durable
        // capture, rather than relying on process-local memory.
        let mut restarted =
            CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &dir).unwrap();
        assert_eq!(
            restarted.adjudicate_game(GameAdjudicationEvent::new(adjudication)),
            AdjudicationRecordOutcome::Stored
        );
        let ledger = std::fs::read_to_string(dir.join(GAME_ADJUDICATION_LEDGER)).unwrap();
        let labelled: DurableGameAdjudicationLine = serde_json::from_str(ledger.trim()).unwrap();
        assert_eq!(labelled.adjudicated_at_epoch_ms, 1_786_128_021_000);
        assert_eq!(labelled.event.adjudication().record_hash(), &record_hash);
        assert!(!dir.join(ADJUDICATION_LEDGER).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Charter §6: adjudications are gated exactly like capture (OFF →
    /// visibly suppressed), refuse un-joinable or candidate-less labels,
    /// and land in the ledger — not in any dataset-class file.
    #[test]
    fn adjudication_is_charter_gated_validated_and_ledgered() {
        let mut off = CapturePipeline::off();
        assert_eq!(
            off.adjudicate(AdjudicationEvent {
                decision_record_hash: "abc123".into(),
                outcome: AdjudicationOutcome::Accepted,
            }),
            AdjudicationRecordOutcome::SuppressedNoCharter
        );

        let dir = scratch_dir("adj");
        let mut p = CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &dir).unwrap();

        assert!(matches!(
            p.adjudicate(AdjudicationEvent {
                decision_record_hash: "   ".into(),
                outcome: AdjudicationOutcome::Accepted,
            }),
            AdjudicationRecordOutcome::Refused(_)
        ));
        assert!(matches!(
            p.adjudicate(AdjudicationEvent {
                decision_record_hash: "abc123".into(),
                outcome: AdjudicationOutcome::Corrected {
                    correct_candidate_id: " ".into()
                },
            }),
            AdjudicationRecordOutcome::Refused(_)
        ));

        assert_eq!(
            p.adjudicate(AdjudicationEvent {
                decision_record_hash: "abc123".into(),
                outcome: AdjudicationOutcome::Corrected {
                    correct_candidate_id: "op.insert_after".into()
                },
            }),
            AdjudicationRecordOutcome::Stored
        );
        let ledger = std::fs::read_to_string(dir.join("adjudications.jsonl")).unwrap();
        let lines: Vec<&str> = ledger.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed: DurableAdjudicationLine = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.charter, RATIFIED_CHARTER_REF);
        assert_eq!(parsed.event.decision_record_hash, "abc123");
        assert_eq!(
            parsed.event.outcome,
            AdjudicationOutcome::Corrected {
                correct_candidate_id: "op.insert_after".into()
            }
        );
        for class_file in ["evaluation.jsonl", "training.jsonl", "audit.jsonl"] {
            assert!(
                !dir.join(class_file).exists(),
                "{class_file} must not receive labels"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fail closed: when durable persistence cannot happen, the event is
    /// stored NOWHERE (memory included) and the caller is told.
    #[test]
    fn persist_failure_stores_nothing_and_is_visible() {
        let dir = scratch_dir("fail");
        let mut p = CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        // Make the path un-creatable: a FILE where the directory was.
        std::fs::write(&dir, b"not a directory").unwrap();
        assert_eq!(
            p.capture(one_event()),
            CaptureOutcome::PersistFailed(DatasetClass::Evaluation)
        );
        assert!(
            p.dataset(DatasetClass::Evaluation).is_empty(),
            "nothing may be half-stored"
        );
        let _ = std::fs::remove_file(&dir);
    }

    /// Green under a charter ref + physical dataset separation.
    #[test]
    fn on_stores_with_dataset_separation() {
        let mut p = CapturePipeline::on_under_charter("Q9-CHARTER-TEST-REF").unwrap();
        let mut train = one_event();
        train.dataset = DatasetClass::Training;
        assert_eq!(
            p.capture(one_event()),
            CaptureOutcome::Stored(DatasetClass::Evaluation)
        );
        assert_eq!(
            p.capture(train),
            CaptureOutcome::Stored(DatasetClass::Training)
        );
        assert_eq!(p.dataset(DatasetClass::Evaluation).len(), 1);
        assert_eq!(p.dataset(DatasetClass::Training).len(), 1);
        assert!(p.dataset(DatasetClass::Audit).is_empty());
    }
}
