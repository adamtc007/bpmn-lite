//! WS-1.3 (EOP-PLAN-SEM-RESOLVER-001): the decomposed quality funnel
//! over charter-captured, operator-adjudicated turns — review P0's
//! instrument. Every labelled turn is attributed to the stage where it
//! succeeded or failed, so a miss is diagnosed as board exclusion vs
//! ranking vs disposition, never a single opaque accuracy number.
//!
//! Honesty rules:
//! - Stages a shadow BPMN record cannot measure (retrieval-subset
//!   inclusion — the record carries only the subset HASH; argument
//!   binding; execution) are reported as explicit `not_measured`
//!   counts, never silently folded into a pass rate.
//! - Multiple adjudications of the same turn: the LAST ledger line
//!   wins (a re-adjudication is a correction of the label), and the
//!   overwrite count is reported.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::capture::{
    class_file_name, game_class_file_name, AdjudicationOutcome, DatasetClass,
    DurableAdjudicationLine, DurableCaptureLine, DurableGameAdjudicationLine,
    DurableGameCaptureLine, ADJUDICATION_LEDGER, GAME_ADJUDICATION_LEDGER, RATIFIED_CHARTER_REF,
};
use crate::policy::{DecisionRecord, ProposalDisposition};

const GAME_SPLIT_SCHEMA_VERSION: u32 = 1;
const MAX_GAME_LEDGER_BYTES: u64 = 64 * 1024 * 1024;
const MAX_GAME_LEDGER_LINES: usize = 100_000;

/// Closed dataset partitions for adjudicated real-turn evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RealTurnSplit {
    Training,
    Validation,
    Test,
}

/// Explicit temporal policy used to freeze a real-turn split.
///
/// Session assignment uses the latest observed turn in the session. This keeps
/// each session intact and prevents a session containing future observations from
/// leaking into an earlier partition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RealTurnSplitPolicy {
    minimum_adjudicated_turns: usize,
    training_end_epoch_ms: u64,
    validation_end_epoch_ms: u64,
}

impl RealTurnSplitPolicy {
    /// Construct the Phase 6 split policy. The programme gate forbids a frozen
    /// real-turn split with fewer than 100 adjudicated turns.
    pub fn phase6(
        training_end_epoch_ms: u64,
        validation_end_epoch_ms: u64,
    ) -> anyhow::Result<Self> {
        if training_end_epoch_ms == 0 || training_end_epoch_ms >= validation_end_epoch_ms {
            anyhow::bail!("real-turn split cutoffs must be non-zero and strictly increasing");
        }
        Ok(Self {
            minimum_adjudicated_turns: 100,
            training_end_epoch_ms,
            validation_end_epoch_ms,
        })
    }

    pub fn minimum_adjudicated_turns(&self) -> usize {
        self.minimum_adjudicated_turns
    }

    pub fn training_end_epoch_ms(&self) -> u64 {
        self.training_end_epoch_ms
    }

    pub fn validation_end_epoch_ms(&self) -> u64 {
        self.validation_end_epoch_ms
    }
}

/// Counts for one family or risk stratum across the frozen partitions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RealTurnSplitCounts {
    training: usize,
    validation: usize,
    test: usize,
}

impl RealTurnSplitCounts {
    fn tally(&mut self, split: RealTurnSplit) {
        match split {
            RealTurnSplit::Training => self.training += 1,
            RealTurnSplit::Validation => self.validation += 1,
            RealTurnSplit::Test => self.test += 1,
        }
    }

    pub fn training(&self) -> usize {
        self.training
    }

    pub fn validation(&self) -> usize {
        self.validation
    }

    pub fn test(&self) -> usize {
        self.test
    }

    pub fn total(&self) -> usize {
        self.training + self.validation + self.test
    }
}

/// One content-addressed assignment. Raw utterance text is deliberately absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RealTurnSplitAssignment {
    record_hash: String,
    session_id: String,
    turn_sequence: u64,
    observed_at_epoch_ms: u64,
    semantic_family: String,
    risk_class: semantic_decision_contracts::HarmClass,
    judgement: semantic_decision_contracts::GameTurnJudgement,
    split: RealTurnSplit,
}

impl RealTurnSplitAssignment {
    pub fn record_hash(&self) -> &str {
        &self.record_hash
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn turn_sequence(&self) -> u64 {
        self.turn_sequence
    }

    pub fn observed_at_epoch_ms(&self) -> u64 {
        self.observed_at_epoch_ms
    }

    pub fn semantic_family(&self) -> &str {
        &self.semantic_family
    }

    pub fn risk_class(&self) -> semantic_decision_contracts::HarmClass {
        self.risk_class
    }

    pub fn judgement(&self) -> semantic_decision_contracts::GameTurnJudgement {
        self.judgement
    }

    pub fn split(&self) -> RealTurnSplit {
        self.split
    }
}

/// Canonical frozen split receipt over adjudicated real turns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RealTurnSplitManifest {
    schema_version: u32,
    policy: RealTurnSplitPolicy,
    assignments: Vec<RealTurnSplitAssignment>,
    split_counts: RealTurnSplitCounts,
    semantic_family_counts: BTreeMap<String, RealTurnSplitCounts>,
    risk_class_counts: BTreeMap<String, RealTurnSplitCounts>,
    adjudication_overwrites: usize,
    manifest_hash: semantic_decision_contracts::GraphContentHash,
}

impl RealTurnSplitManifest {
    pub fn policy(&self) -> &RealTurnSplitPolicy {
        &self.policy
    }

    pub fn assignments(&self) -> &[RealTurnSplitAssignment] {
        &self.assignments
    }

    pub fn split_counts(&self) -> RealTurnSplitCounts {
        self.split_counts
    }

    pub fn semantic_family_counts(&self) -> &BTreeMap<String, RealTurnSplitCounts> {
        &self.semantic_family_counts
    }

    pub fn risk_class_counts(&self) -> &BTreeMap<String, RealTurnSplitCounts> {
        &self.risk_class_counts
    }

    pub fn adjudication_overwrites(&self) -> usize {
        self.adjudication_overwrites
    }

    pub fn manifest_hash(&self) -> &semantic_decision_contracts::GraphContentHash {
        &self.manifest_hash
    }
}

/// One validated captured record, its latest adjudication and frozen partition.
#[derive(Clone, Debug)]
pub struct FrozenAdjudicatedRealTurn {
    record: semantic_decision_contracts::GameTurnRecord,
    adjudication: semantic_decision_contracts::GameTurnAdjudication,
    split: RealTurnSplit,
}

impl FrozenAdjudicatedRealTurn {
    pub fn record(&self) -> &semantic_decision_contracts::GameTurnRecord {
        &self.record
    }

    pub fn adjudication(&self) -> &semantic_decision_contracts::GameTurnAdjudication {
        &self.adjudication
    }

    pub fn split(&self) -> RealTurnSplit {
        self.split
    }
}

/// Honest measured/not-measured accounting for one real-turn funnel stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GameFunnelStage {
    eligible: usize,
    passed: usize,
    not_measured: usize,
}

impl GameFunnelStage {
    fn tally(&mut self, result: Option<bool>) {
        match result {
            Some(passed) => {
                self.eligible += 1;
                self.passed += usize::from(passed);
            }
            None => self.not_measured += 1,
        }
    }

    pub fn eligible(&self) -> usize {
        self.eligible
    }

    pub fn passed(&self) -> usize {
        self.passed
    }

    pub fn not_measured(&self) -> usize {
        self.not_measured
    }
}

/// Permanent game-level evaluation funnel over adjudicated real turns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GameFunnelReport {
    adjudicated_turns: usize,
    positive_labels: usize,
    judgements: BTreeMap<String, usize>,
    intended_move_representable: GameFunnelStage,
    intended_move_on_board: GameFunnelStage,
    top_one: GameFunnelStage,
    top_three: GameFunnelStage,
    disposition_correct: GameFunnelStage,
    argument_accuracy: GameFunnelStage,
    accepted_without_correction: GameFunnelStage,
    graph_delta_correct: GameFunnelStage,
    compiler_admission: GameFunnelStage,
    wrong_attempt_feedback_correct: GameFunnelStage,
    recovered_after_wrong_attempt: GameFunnelStage,
    recovered_turn_cost_total: usize,
    repeated_failure_turns: usize,
    reversals: usize,
    eventual_target_completion_not_measured: usize,
}

impl GameFunnelReport {
    pub fn adjudicated_turns(&self) -> usize {
        self.adjudicated_turns
    }

    pub fn positive_labels(&self) -> usize {
        self.positive_labels
    }

    pub fn judgements(&self) -> &BTreeMap<String, usize> {
        &self.judgements
    }

    pub fn intended_move_representable(&self) -> GameFunnelStage {
        self.intended_move_representable
    }

    pub fn intended_move_on_board(&self) -> GameFunnelStage {
        self.intended_move_on_board
    }

    pub fn top_one(&self) -> GameFunnelStage {
        self.top_one
    }

    pub fn top_three(&self) -> GameFunnelStage {
        self.top_three
    }

    pub fn disposition_correct(&self) -> GameFunnelStage {
        self.disposition_correct
    }

    pub fn argument_accuracy(&self) -> GameFunnelStage {
        self.argument_accuracy
    }

    pub fn accepted_without_correction(&self) -> GameFunnelStage {
        self.accepted_without_correction
    }

    pub fn graph_delta_correct(&self) -> GameFunnelStage {
        self.graph_delta_correct
    }

    pub fn compiler_admission(&self) -> GameFunnelStage {
        self.compiler_admission
    }

    pub fn wrong_attempt_feedback_correct(&self) -> GameFunnelStage {
        self.wrong_attempt_feedback_correct
    }

    pub fn recovered_after_wrong_attempt(&self) -> GameFunnelStage {
        self.recovered_after_wrong_attempt
    }

    pub fn recovered_turn_cost_total(&self) -> usize {
        self.recovered_turn_cost_total
    }

    pub fn repeated_failure_turns(&self) -> usize {
        self.repeated_failure_turns
    }

    pub fn reversals(&self) -> usize {
        self.reversals
    }

    pub fn eventual_target_completion_not_measured(&self) -> usize {
        self.eventual_target_completion_not_measured
    }
}

/// Evaluate the game-level funnel without converting non-positive interactions into labels.
pub fn evaluate_frozen_game_funnel(
    turns: &[FrozenAdjudicatedRealTurn],
) -> anyhow::Result<GameFunnelReport> {
    if turns.is_empty() {
        anyhow::bail!("game funnel requires adjudicated real turns");
    }
    let mut report = GameFunnelReport::default();
    for turn in turns {
        let record = turn.record();
        let adjudication = turn.adjudication();
        adjudication.validate_for_record(record)?;
        report.adjudicated_turns += 1;
        *report
            .judgements
            .entry(format!("{:?}", adjudication.judgement()).to_ascii_lowercase())
            .or_default() += 1;
        let positive = adjudication.positive_label();
        report.positive_labels += usize::from(positive.is_some());

        use semantic_decision_contracts::{
            CorrectionKind, GameDispositionKind, GameTurnCompilerResultKind, IntendedMove,
            MoveAttemptOutcome,
        };
        match adjudication.intended_move() {
            IntendedMove::OnBoard { .. } => {
                report.intended_move_representable.tally(Some(true));
                report.intended_move_on_board.tally(Some(true));
            }
            IntendedMove::OffBoard { .. } => {
                report.intended_move_representable.tally(Some(false));
                report.intended_move_on_board.tally(Some(false));
            }
            IntendedMove::None => {
                report.intended_move_representable.tally(None);
                report.intended_move_on_board.tally(None);
            }
        }

        if let Some(intended) = positive {
            let mut ranking = record.evidence().iter().collect::<Vec<_>>();
            ranking.sort_by(|left, right| {
                right
                    .final_score()
                    .get()
                    .total_cmp(&left.final_score().get())
                    .then_with(|| left.move_id().cmp(right.move_id()))
            });
            let rank = ranking
                .iter()
                .position(|evidence| evidence.move_id() == intended)
                .ok_or_else(|| {
                    anyhow::anyhow!("positive label is absent from complete evidence")
                })?
                + 1;
            report.top_one.tally(Some(rank == 1));
            report.top_three.tally(Some(rank <= 3));

            let disposition_result = if record.disposition().kind()
                == GameDispositionKind::ProposeMove
                && record.disposition().selected_moves() == std::slice::from_ref(intended)
            {
                Some(true)
            } else if record.disposition().kind() == GameDispositionKind::ClarifyMoves {
                record
                    .disposition()
                    .clarification_dimension()
                    .map(|dimension| {
                        adjudication
                            .acceptable_clarifications()
                            .contains(&dimension)
                    })
            } else {
                Some(false)
            };
            report.disposition_correct.tally(disposition_result);

            if adjudication.intended_arguments().is_empty() {
                report.argument_accuracy.tally(None);
            } else {
                let legal_move = record
                    .position()
                    .legal_moves()
                    .iter()
                    .find(|legal_move| legal_move.move_id() == intended)
                    .ok_or_else(|| {
                        anyhow::anyhow!("adjudicated move is absent from its position")
                    })?;
                let arguments_match = adjudication.intended_arguments().iter().all(|expected| {
                    legal_move.arguments().iter().any(|actual| {
                        actual.name() == expected.name()
                            && actual.kind() == expected.kind()
                            && actual.value() == expected.value()
                    })
                });
                report.argument_accuracy.tally(Some(arguments_match));
            }
        } else {
            report.top_one.tally(None);
            report.top_three.tally(None);
            report.disposition_correct.tally(None);
            report.argument_accuracy.tally(None);
        }

        let correction_history = record
            .related_attempts()
            .iter()
            .filter(|attempt| {
                matches!(
                    attempt.outcome(),
                    MoveAttemptOutcome::RejectedByUser
                        | MoveAttemptOutcome::Corrected
                        | MoveAttemptOutcome::Incomplete
                        | MoveAttemptOutcome::Ambiguous
                        | MoveAttemptOutcome::Inapplicable
                        | MoveAttemptOutcome::Stale
                        | MoveAttemptOutcome::CompilerRefused
                        | MoveAttemptOutcome::SystemFailure
                )
            })
            .collect::<Vec<_>>();
        if adjudication.judgement() == semantic_decision_contracts::GameTurnJudgement::AcceptedMove
        {
            report
                .accepted_without_correction
                .tally(Some(correction_history.is_empty()));
        } else {
            report.accepted_without_correction.tally(None);
        }

        match (positive, record.delta()) {
            (Some(intended), Some(delta)) => {
                let matches = record
                    .position()
                    .legal_moves()
                    .iter()
                    .find(|legal_move| legal_move.move_id() == intended)
                    .and_then(|legal_move| legal_move.preview())
                    == Some(delta);
                report.graph_delta_correct.tally(Some(matches));
            }
            (Some(_), None) => report.graph_delta_correct.tally(None),
            (None, _) => report.graph_delta_correct.tally(None),
        }

        match record.attempt().receipt().map(|attempt| attempt.outcome()) {
            Some(MoveAttemptOutcome::Applied)
            | Some(MoveAttemptOutcome::CompilerRefused)
            | Some(MoveAttemptOutcome::SystemFailure) => report.compiler_admission.tally(Some(
                record.compiler_result().kind() == GameTurnCompilerResultKind::Admitted,
            )),
            _ => report.compiler_admission.tally(None),
        }

        if adjudication.acceptable_feedback().is_empty() {
            report.wrong_attempt_feedback_correct.tally(None);
        } else {
            report.wrong_attempt_feedback_correct.tally(Some(
                record
                    .disposition()
                    .feedback_options()
                    .iter()
                    .any(|actual| adjudication.acceptable_feedback().contains(&actual.kind())),
            ));
        }

        if correction_history.is_empty() {
            report.recovered_after_wrong_attempt.tally(None);
        } else {
            let recovered = positive.is_some()
                && record.attempt().receipt().is_some_and(|attempt| {
                    matches!(
                        attempt.outcome(),
                        MoveAttemptOutcome::Applied | MoveAttemptOutcome::Corrected
                    )
                });
            report.recovered_after_wrong_attempt.tally(Some(recovered));
            if recovered {
                report.recovered_turn_cost_total += correction_history.len() + 1;
            }
            let mut outcomes = BTreeMap::<String, usize>::new();
            for attempt in &correction_history {
                *outcomes
                    .entry(format!("{:?}", attempt.outcome()))
                    .or_default() += 1;
            }
            report.repeated_failure_turns += usize::from(outcomes.values().any(|count| *count > 1));
        }
        report.reversals += record
            .related_attempts()
            .iter()
            .chain(record.attempt().receipt())
            .filter(|attempt| attempt.correction_kind() == Some(CorrectionKind::Undo))
            .count();
        report.eventual_target_completion_not_measured += 1;
    }
    Ok(report)
}

#[derive(Clone)]
struct EligibleRealTurn {
    record_hash: String,
    session_id: String,
    turn_sequence: u64,
    observed_at_epoch_ms: u64,
    semantic_family: String,
    risk_class: semantic_decision_contracts::HarmClass,
    judgement: semantic_decision_contracts::GameTurnJudgement,
}

/// Freeze the Evaluation-class, adjudicated game turns in one capture directory.
///
/// The function fails closed on malformed, oversized, duplicate or unjoined ledger
/// content and does not write an artifact itself. Callers choose the output path only
/// after receiving a fully validated manifest.
pub fn freeze_real_turn_split(
    dir: &Path,
    policy: &RealTurnSplitPolicy,
) -> anyhow::Result<RealTurnSplitManifest> {
    let capture_path = dir.join(game_class_file_name(DatasetClass::Evaluation));
    let adjudication_path = dir.join(GAME_ADJUDICATION_LEDGER);
    let captures = read_bounded_jsonl::<DurableGameCaptureLine>(&capture_path)?;
    let adjudications = read_bounded_jsonl::<DurableGameAdjudicationLine>(&adjudication_path)?;

    let mut records = BTreeMap::new();
    for line in captures {
        if line.charter != RATIFIED_CHARTER_REF {
            anyhow::bail!("game capture uses an unrecognised charter reference");
        }
        let record = line.event.record().clone();
        let hash = record.record_hash().as_str().to_string();
        if records.insert(hash.clone(), record).is_some() {
            anyhow::bail!("duplicate captured game record hash '{hash}'");
        }
    }

    let mut labels = BTreeMap::new();
    let mut overwrites = 0;
    for line in adjudications {
        if line.charter != RATIFIED_CHARTER_REF {
            anyhow::bail!("game adjudication uses an unrecognised charter reference");
        }
        let adjudication = line.event.adjudication().clone();
        let hash = adjudication.record_hash().as_str().to_string();
        let record = records
            .get(&hash)
            .ok_or_else(|| anyhow::anyhow!("adjudication names uncaptured game turn '{hash}'"))?;
        adjudication.validate_for_record(record)?;
        if labels.insert(hash, adjudication).is_some() {
            overwrites += 1;
        }
    }

    let eligible = labels
        .into_iter()
        .map(|(hash, adjudication)| {
            let record = &records[&hash];
            EligibleRealTurn {
                record_hash: hash,
                session_id: record.session_id().as_str().to_string(),
                turn_sequence: record.sequence(),
                observed_at_epoch_ms: record.observed_at_epoch_ms(),
                semantic_family: record.semantic_family().as_str().to_string(),
                risk_class: record.risk_class(),
                judgement: adjudication.judgement(),
            }
        })
        .collect();
    build_real_turn_split(policy, eligible, overwrites)
}

/// Rejoin the exact chartered records and latest adjudications named by a frozen manifest.
/// Any ledger or manifest drift fails closed before returning a training/evaluation row.
pub fn load_frozen_real_turns(
    dir: &Path,
    manifest: &RealTurnSplitManifest,
) -> anyhow::Result<Vec<FrozenAdjudicatedRealTurn>> {
    let captures = read_bounded_jsonl::<DurableGameCaptureLine>(
        &dir.join(game_class_file_name(DatasetClass::Evaluation)),
    )?;
    let adjudications =
        read_bounded_jsonl::<DurableGameAdjudicationLine>(&dir.join(GAME_ADJUDICATION_LEDGER))?;
    let mut records = BTreeMap::new();
    for line in captures {
        if line.charter != RATIFIED_CHARTER_REF {
            anyhow::bail!("game capture uses an unrecognised charter reference");
        }
        let record = line.event.record().clone();
        let hash = record.record_hash().as_str().to_string();
        if records.insert(hash.clone(), record).is_some() {
            anyhow::bail!("duplicate captured game record hash '{hash}'");
        }
    }
    let mut labels = BTreeMap::new();
    for line in adjudications {
        if line.charter != RATIFIED_CHARTER_REF {
            anyhow::bail!("game adjudication uses an unrecognised charter reference");
        }
        let adjudication = line.event.adjudication().clone();
        let hash = adjudication.record_hash().as_str().to_string();
        let record = records
            .get(&hash)
            .ok_or_else(|| anyhow::anyhow!("adjudication names uncaptured game turn '{hash}'"))?;
        adjudication.validate_for_record(record)?;
        labels.insert(hash, adjudication);
    }
    let assignments = manifest
        .assignments()
        .iter()
        .map(|assignment| (assignment.record_hash(), assignment))
        .collect::<BTreeMap<_, _>>();
    if assignments.len() != manifest.assignments().len() || assignments.len() != labels.len() {
        anyhow::bail!("frozen split assignments no longer match the adjudication ledger");
    }
    let mut joined = Vec::with_capacity(assignments.len());
    for (hash, assignment) in assignments {
        let record = records.remove(hash).ok_or_else(|| {
            anyhow::anyhow!("frozen split names a missing captured turn '{hash}'")
        })?;
        let adjudication = labels
            .remove(hash)
            .ok_or_else(|| anyhow::anyhow!("frozen split names a missing adjudication '{hash}'"))?;
        adjudication.validate_for_record(&record)?;
        if assignment.session_id() != record.session_id().as_str()
            || assignment.turn_sequence() != record.sequence()
            || assignment.observed_at_epoch_ms() != record.observed_at_epoch_ms()
            || assignment.semantic_family() != record.semantic_family().as_str()
            || assignment.risk_class() != record.risk_class()
            || assignment.judgement() != adjudication.judgement()
        {
            anyhow::bail!("frozen split metadata drifted from record '{hash}'");
        }
        joined.push(FrozenAdjudicatedRealTurn {
            record,
            adjudication,
            split: assignment.split(),
        });
    }
    Ok(joined)
}

fn read_bounded_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_GAME_LEDGER_BYTES {
        anyhow::bail!("ledger {} exceeds the 64 MiB limit", path.display());
    }
    let content = std::fs::read_to_string(path)?;
    content
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index >= MAX_GAME_LEDGER_LINES {
                anyhow::bail!("ledger {} exceeds the line limit", path.display());
            }
            serde_json::from_str(line).map_err(anyhow::Error::from)
        })
        .collect()
}

fn build_real_turn_split(
    policy: &RealTurnSplitPolicy,
    mut eligible: Vec<EligibleRealTurn>,
    adjudication_overwrites: usize,
) -> anyhow::Result<RealTurnSplitManifest> {
    if eligible.len() < policy.minimum_adjudicated_turns {
        anyhow::bail!(
            "real-turn split requires at least {} adjudicated turns; observed {}",
            policy.minimum_adjudicated_turns,
            eligible.len()
        );
    }
    let mut session_max_time = BTreeMap::<String, u64>::new();
    for turn in &eligible {
        session_max_time
            .entry(turn.session_id.clone())
            .and_modify(|value| *value = (*value).max(turn.observed_at_epoch_ms))
            .or_insert(turn.observed_at_epoch_ms);
    }
    eligible.sort_by(|left, right| {
        (&left.session_id, left.turn_sequence, &left.record_hash).cmp(&(
            &right.session_id,
            right.turn_sequence,
            &right.record_hash,
        ))
    });
    let mut split_counts = RealTurnSplitCounts::default();
    let mut semantic_family_counts = BTreeMap::<String, RealTurnSplitCounts>::new();
    let mut risk_class_counts = BTreeMap::<String, RealTurnSplitCounts>::new();
    let assignments = eligible
        .into_iter()
        .map(|turn| {
            let session_time = session_max_time[&turn.session_id];
            let split = if session_time <= policy.training_end_epoch_ms {
                RealTurnSplit::Training
            } else if session_time <= policy.validation_end_epoch_ms {
                RealTurnSplit::Validation
            } else {
                RealTurnSplit::Test
            };
            split_counts.tally(split);
            semantic_family_counts
                .entry(turn.semantic_family.clone())
                .or_default()
                .tally(split);
            risk_class_counts
                .entry(format!("{:?}", turn.risk_class).to_ascii_lowercase())
                .or_default()
                .tally(split);
            RealTurnSplitAssignment {
                record_hash: turn.record_hash,
                session_id: turn.session_id,
                turn_sequence: turn.turn_sequence,
                observed_at_epoch_ms: turn.observed_at_epoch_ms,
                semantic_family: turn.semantic_family,
                risk_class: turn.risk_class,
                judgement: turn.judgement,
                split,
            }
        })
        .collect::<Vec<_>>();
    if split_counts.training == 0 || split_counts.validation == 0 || split_counts.test == 0 {
        anyhow::bail!("real-turn split must contain training, validation and test turns");
    }

    #[derive(Serialize)]
    struct HashPreimage<'a> {
        schema_version: u32,
        policy: &'a RealTurnSplitPolicy,
        assignments: &'a [RealTurnSplitAssignment],
        split_counts: RealTurnSplitCounts,
        semantic_family_counts: &'a BTreeMap<String, RealTurnSplitCounts>,
        risk_class_counts: &'a BTreeMap<String, RealTurnSplitCounts>,
        adjudication_overwrites: usize,
    }
    let encoded = serde_json::to_vec(&HashPreimage {
        schema_version: GAME_SPLIT_SCHEMA_VERSION,
        policy,
        assignments: &assignments,
        split_counts,
        semantic_family_counts: &semantic_family_counts,
        risk_class_counts: &risk_class_counts,
        adjudication_overwrites,
    })?;
    let manifest_hash = semantic_decision_contracts::GraphContentHash::new(format!(
        "{:x}",
        Sha256::digest(encoded)
    ))?;
    Ok(RealTurnSplitManifest {
        schema_version: GAME_SPLIT_SCHEMA_VERSION,
        policy: policy.clone(),
        assignments,
        split_counts,
        semantic_family_counts,
        risk_class_counts,
        adjudication_overwrites,
        manifest_hash,
    })
}

/// One funnel stage: how many labelled turns it applied to, and how
/// many passed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Stage {
    pub eligible: usize,
    pub passed: usize,
}

impl Stage {
    fn tally(&mut self, passed: bool) {
        self.eligible += 1;
        if passed {
            self.passed += 1;
        }
    }
}

/// The decomposed report. Field order mirrors the funnel order.
#[derive(Clone, Debug, Default, Serialize)]
pub struct FunnelReport {
    /// Turns captured in the Evaluation class.
    pub captured_turns: usize,
    /// Captured turns with at least one adjudication (the labelled set —
    /// every stage below is computed over these only).
    pub labelled_turns: usize,
    /// Ledger lines whose hash matched no captured turn (a label that
    /// cannot be joined — surfaced, not dropped silently).
    pub unmatched_labels: usize,
    /// Re-adjudications that overwrote an earlier label.
    pub label_overwrites: usize,
    /// Labelled-turn count per outcome kind.
    pub labels: BTreeMap<String, usize>,
    /// Stage: the gold candidate was on the served board (candidate
    /// labels only; abandoned turns have no on-board gold by meaning).
    pub gold_on_board: Stage,
    /// Stage: the gold candidate was served top-1.
    pub top1: Stage,
    /// Stage: the disposition was right for the label — Accepted →
    /// `Candidate{gold}`; Corrected/Selected → anything but a confident
    /// wrong candidate; Abandoned → anything but a confident candidate.
    pub disposition_correct: Stage,
    /// Confidently-wrong count: disposition proposed `Candidate{x}`
    /// where the label says x was not what the operator meant. The
    /// review's sharpest promotion metric (<1% for mutating actions).
    pub confident_wrong: usize,
    /// Explicit measurement gaps (see module doc).
    pub retrieval_inclusion_not_measured: usize,
    pub binding_not_measured: usize,
    pub execution_not_measured: usize,
}

/// The gold candidate a label implies, if it implies one.
fn gold_of(record: &DecisionRecord, outcome: &AdjudicationOutcome) -> Option<String> {
    match outcome {
        AdjudicationOutcome::Accepted => {
            record.ranking.first().map(|(id, _)| id.clone())
        }
        AdjudicationOutcome::Corrected { correct_candidate_id } => {
            Some(correct_candidate_id.clone())
        }
        AdjudicationOutcome::ExplicitlySelected { candidate_id } => Some(candidate_id.clone()),
        AdjudicationOutcome::Abandoned => None,
    }
}

fn label_key(outcome: &AdjudicationOutcome) -> &'static str {
    match outcome {
        AdjudicationOutcome::Accepted => "accepted",
        AdjudicationOutcome::Corrected { .. } => "corrected",
        AdjudicationOutcome::ExplicitlySelected { .. } => "explicitly_selected",
        AdjudicationOutcome::Abandoned => "abandoned",
    }
}

/// Score one labelled turn into the report.
pub fn assess_turn(
    report: &mut FunnelReport,
    record: &DecisionRecord,
    outcome: &AdjudicationOutcome,
) {
    report.labelled_turns += 1;
    *report.labels.entry(label_key(outcome).to_owned()).or_default() += 1;
    report.retrieval_inclusion_not_measured += 1;
    report.binding_not_measured += 1;
    report.execution_not_measured += 1;

    let proposed = match &record.disposition {
        ProposalDisposition::Candidate { candidate_id } => Some(candidate_id.clone()),
        ProposalDisposition::MissingArguments { candidate_id, .. } => Some(candidate_id.clone()),
        ProposalDisposition::Ambiguous { .. }
        | ProposalDisposition::Compound { .. }
        | ProposalDisposition::OutOfScope
        | ProposalDisposition::EscalateToSage { .. } => None,
    };

    match gold_of(record, outcome) {
        Some(gold) => {
            report
                .gold_on_board
                .tally(record.ranking.iter().any(|(id, _)| id == &gold));
            report
                .top1
                .tally(record.ranking.first().is_some_and(|(id, _)| id == &gold));
            let confident_wrong = proposed.as_deref().is_some_and(|p| p != gold);
            if confident_wrong {
                report.confident_wrong += 1;
            }
            let correct = match outcome {
                AdjudicationOutcome::Accepted => proposed.as_deref() == Some(gold.as_str()),
                // For a corrected/selected turn the system was wrong at
                // top level; the disposition behaved correctly iff it
                // did not CONFIDENTLY commit to the wrong candidate.
                _ => !confident_wrong,
            };
            report.disposition_correct.tally(correct);
        }
        None => {
            // Abandoned: no on-board gold. Correct behaviour is any
            // non-committal disposition; a confident candidate is a
            // confident wrong.
            let confident_wrong = proposed.is_some();
            if confident_wrong {
                report.confident_wrong += 1;
            }
            report.disposition_correct.tally(!confident_wrong);
        }
    }
}

/// Build the report from a charter capture directory: joins the
/// Evaluation class file against the adjudication ledger by
/// `decision_record_hash`.
pub fn funnel_from_capture_dir(dir: &Path) -> anyhow::Result<FunnelReport> {
    let mut report = FunnelReport::default();

    let eval_path = dir.join(class_file_name(DatasetClass::Evaluation));
    let mut turns: BTreeMap<String, DecisionRecord> = BTreeMap::new();
    if eval_path.exists() {
        for line in std::fs::read_to_string(&eval_path)?.lines() {
            let parsed: DurableCaptureLine = serde_json::from_str(line)?;
            report.captured_turns += 1;
            turns.insert(
                parsed.event.record.decision_record_hash.clone(),
                parsed.event.record,
            );
        }
    }

    let ledger_path = dir.join(ADJUDICATION_LEDGER);
    let mut labels: BTreeMap<String, AdjudicationOutcome> = BTreeMap::new();
    if ledger_path.exists() {
        for line in std::fs::read_to_string(&ledger_path)?.lines() {
            let parsed: DurableAdjudicationLine = serde_json::from_str(line)?;
            if !turns.contains_key(&parsed.event.decision_record_hash) {
                report.unmatched_labels += 1;
                continue;
            }
            if labels
                .insert(parsed.event.decision_record_hash.clone(), parsed.event.outcome)
                .is_some()
            {
                report.label_overwrites += 1;
            }
        }
    }

    for (hash, outcome) in &labels {
        assess_turn(&mut report, &turns[hash], outcome);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::FiniteScore;

    fn eligible_turns(count: usize) -> Vec<EligibleRealTurn> {
        (0..count)
            .map(|index| {
                let session = index % 6;
                let observed_at_epoch_ms = match session {
                    0 | 1 => 10,
                    2 | 3 => 20,
                    _ => 30,
                };
                EligibleRealTurn {
                    record_hash: format!("{index:064x}"),
                    session_id: format!("session-{session}"),
                    turn_sequence: (index / 6) as u64,
                    observed_at_epoch_ms,
                    semantic_family: format!("family-{}", index % 2),
                    risk_class: if index % 2 == 0 {
                        semantic_decision_contracts::HarmClass::Reversible
                    } else {
                        semantic_decision_contracts::HarmClass::ReadOnly
                    },
                    judgement:
                        semantic_decision_contracts::GameTurnJudgement::ExploratoryHumanAttempt,
                }
            })
            .collect()
    }

    fn record(ranking: &[(&str, f64)], disposition: ProposalDisposition) -> DecisionRecord {
        DecisionRecord {
            board_hash: "b".into(),
            retrieved_subset_hash: "r".into(),
            model_bundle_hash: "m".into(),
            disposition_policy_hash: "p".into(),
            context_projection_hash: "c".into(),
            ranking: ranking
                .iter()
                .map(|(id, s)| ((*id).to_owned(), FiniteScore::new(*s).unwrap()))
                .collect(),
            disposition,
            evidence_trace: None,
            board: None,
            action_span_producer_hash: String::new(),
            decision_record_hash: "h".into(),
        }
    }

    #[test]
    fn real_turn_split_is_session_closed_temporal_stratified_and_content_addressed() {
        let policy = RealTurnSplitPolicy::phase6(10, 20).unwrap();
        let first = build_real_turn_split(&policy, eligible_turns(102), 2).unwrap();
        let replay =
            build_real_turn_split(&policy, eligible_turns(102).into_iter().rev().collect(), 2)
                .unwrap();
        assert_eq!(first, replay);
        assert_eq!(first.split_counts().total(), 102);
        assert_eq!(first.adjudication_overwrites(), 2);
        assert!(first
            .semantic_family_counts()
            .values()
            .all(|counts| counts.total() > 0));
        let mut by_session = BTreeMap::<&str, RealTurnSplit>::new();
        for assignment in first.assignments() {
            if let Some(previous) = by_session.insert(assignment.session_id(), assignment.split()) {
                assert_eq!(previous, assignment.split());
            }
        }
    }

    #[test]
    fn real_turn_split_refuses_below_gate_or_empty_temporal_partition() {
        let policy = RealTurnSplitPolicy::phase6(10, 20).unwrap();
        assert!(build_real_turn_split(&policy, eligible_turns(99), 0).is_err());
        let only_training = (0..100)
            .map(|index| EligibleRealTurn {
                record_hash: format!("{index:064x}"),
                session_id: format!("session-{index}"),
                turn_sequence: 0,
                observed_at_epoch_ms: 5,
                semantic_family: "family".into(),
                risk_class: semantic_decision_contracts::HarmClass::ReadOnly,
                judgement: semantic_decision_contracts::GameTurnJudgement::ExploratoryHumanAttempt,
            })
            .collect();
        assert!(build_real_turn_split(&policy, only_training, 0).is_err());
    }

    #[test]
    fn frozen_real_turn_loader_fails_closed_on_missing_ledgers() {
        let manifest = build_real_turn_split(
            &RealTurnSplitPolicy::phase6(10, 20).unwrap(),
            eligible_turns(102),
            0,
        )
        .unwrap();
        let dir =
            std::env::temp_dir().join(format!("q9-game-split-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_frozen_real_turns(&dir, &manifest).is_err());
        assert!(evaluate_frozen_game_funnel(&[]).is_err());
    }

    #[test]
    fn accepted_turn_passes_measured_stages() {
        let mut report = FunnelReport::default();
        let rec = record(
            &[("op.connect", 0.9), ("op.append_node", 0.1)],
            ProposalDisposition::Candidate { candidate_id: "op.connect".into() },
        );
        assess_turn(&mut report, &rec, &AdjudicationOutcome::Accepted);
        assert_eq!(report.gold_on_board, Stage { eligible: 1, passed: 1 });
        assert_eq!(report.top1, Stage { eligible: 1, passed: 1 });
        assert_eq!(report.disposition_correct, Stage { eligible: 1, passed: 1 });
        assert_eq!(report.confident_wrong, 0);
        assert_eq!(report.retrieval_inclusion_not_measured, 1, "gap stays explicit");
    }

    #[test]
    fn corrected_turn_attributes_the_stage_and_counts_confident_wrong() {
        let mut report = FunnelReport::default();
        // Served the wrong candidate confidently; gold was on the board.
        let rec = record(
            &[("op.connect", 0.9), ("op.insert_after", 0.1)],
            ProposalDisposition::Candidate { candidate_id: "op.connect".into() },
        );
        assess_turn(
            &mut report,
            &rec,
            &AdjudicationOutcome::Corrected { correct_candidate_id: "op.insert_after".into() },
        );
        assert_eq!(report.gold_on_board, Stage { eligible: 1, passed: 1 });
        assert_eq!(report.top1, Stage { eligible: 1, passed: 0 }, "failure lands at ranking");
        assert_eq!(report.disposition_correct, Stage { eligible: 1, passed: 0 });
        assert_eq!(report.confident_wrong, 1);

        // Gold entirely off the board: the failure attributes to board
        // construction, not ranking.
        let mut report = FunnelReport::default();
        let rec = record(
            &[("op.connect", 0.9)],
            ProposalDisposition::EscalateToSage { reason: "weak".into() },
        );
        assess_turn(
            &mut report,
            &rec,
            &AdjudicationOutcome::Corrected { correct_candidate_id: "op.never_built".into() },
        );
        assert_eq!(report.gold_on_board, Stage { eligible: 1, passed: 0 });
        assert_eq!(report.confident_wrong, 0, "escalation is not confident wrongness");
        assert_eq!(report.disposition_correct, Stage { eligible: 1, passed: 1 });
    }

    #[test]
    fn abandoned_turn_rewards_abstention_and_flags_confident_candidates() {
        let mut report = FunnelReport::default();
        let out_of_scope = record(&[("op.connect", 0.4)], ProposalDisposition::OutOfScope);
        assess_turn(&mut report, &out_of_scope, &AdjudicationOutcome::Abandoned);
        assert_eq!(report.disposition_correct, Stage { eligible: 1, passed: 1 });
        assert_eq!(report.confident_wrong, 0);
        assert_eq!(report.gold_on_board.eligible, 0, "no gold exists for abandonment");

        let confident = record(
            &[("op.connect", 0.9)],
            ProposalDisposition::Candidate { candidate_id: "op.connect".into() },
        );
        assess_turn(&mut report, &confident, &AdjudicationOutcome::Abandoned);
        assert_eq!(report.disposition_correct, Stage { eligible: 2, passed: 1 });
        assert_eq!(report.confident_wrong, 1);
    }

    #[test]
    fn capture_dir_join_counts_unmatched_and_overwrites() {
        use crate::capture::{
            AdjudicationEvent, AdjudicationRecordOutcome, CaptureEvent, CaptureOutcome,
            CapturePipeline, DatasetClass, RATIFIED_CHARTER_REF,
        };
        let dir = std::env::temp_dir().join(format!("q9funnel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut p = CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &dir).unwrap();
        let rec = record(
            &[("op.connect", 0.9), ("op.insert_after", 0.1)],
            ProposalDisposition::Candidate { candidate_id: "op.connect".into() },
        );
        assert_eq!(
            p.capture(CaptureEvent {
                raw_utterance: "connect them".into(),
                record: rec,
                dataset: DatasetClass::Evaluation,
            }),
            CaptureOutcome::Stored(DatasetClass::Evaluation)
        );
        // First label, then a re-adjudication that overwrites it, then
        // one label that joins to nothing.
        for (hash, outcome) in [
            ("h", AdjudicationOutcome::Accepted),
            ("h", AdjudicationOutcome::Corrected { correct_candidate_id: "op.insert_after".into() }),
            ("no-such-turn", AdjudicationOutcome::Accepted),
        ] {
            assert_eq!(
                p.adjudicate(AdjudicationEvent {
                    decision_record_hash: hash.into(),
                    outcome,
                }),
                AdjudicationRecordOutcome::Stored
            );
        }

        let report = funnel_from_capture_dir(&dir).unwrap();
        assert_eq!(report.captured_turns, 1);
        assert_eq!(report.labelled_turns, 1);
        assert_eq!(report.unmatched_labels, 1);
        assert_eq!(report.label_overwrites, 1);
        assert_eq!(report.labels["corrected"], 1, "last label wins");
        assert_eq!(report.top1, Stage { eligible: 1, passed: 0 });
        assert_eq!(report.confident_wrong, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
