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

use crate::capture::{
    class_file_name, AdjudicationOutcome, DatasetClass, DurableAdjudicationLine,
    DurableCaptureLine, ADJUDICATION_LEDGER,
};
use crate::policy::{DecisionRecord, ProposalDisposition};

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
