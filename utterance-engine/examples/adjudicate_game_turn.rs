use std::path::{Path, PathBuf};

use semantic_decision_contracts::{
    FeedbackOptionKind, GameClarificationDimension, GameTurnAdjudication, GameTurnJudgement,
    GraphContentHash, GraphElementRef, IntendedMove, MoveArgument,
};
use serde::Deserialize;
use utterance_engine::capture::{
    AdjudicationRecordOutcome, CapturePipeline, GameAdjudicationEvent, GameCaptureEvent,
    RATIFIED_CHARTER_REF,
};

#[derive(Deserialize)]
struct CaptureLine {
    event: GameCaptureEvent,
}

#[derive(Deserialize)]
struct AdjudicationSpec {
    adjudicator: String,
    adjudicated_at_epoch_ms: u64,
    judgement: GameTurnJudgement,
    intended_move: IntendedMove,
    intended_anchor: Option<GraphElementRef>,
    #[serde(default)]
    intended_arguments: Vec<MoveArgument>,
    intended_motif: Option<String>,
    #[serde(default)]
    acceptable_clarifications: Vec<GameClarificationDimension>,
    #[serde(default)]
    acceptable_feedback: Vec<FeedbackOptionKind>,
    note_hash: Option<GraphContentHash>,
}

fn main() -> anyhow::Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let capture_dir = PathBuf::from(arguments.next().ok_or_else(|| {
        anyhow::anyhow!("usage: adjudicate_game_turn <capture-dir> <record-hash> <spec-json>")
    })?);
    let record_hash = arguments
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing record hash"))?
        .into_string()
        .map_err(|_| anyhow::anyhow!("record hash must be UTF-8"))?;
    let spec_path = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing adjudication spec path"))?,
    );
    if arguments.next().is_some() {
        anyhow::bail!("unexpected trailing arguments");
    }

    let record = find_record(&capture_dir, &record_hash)?;
    let spec: AdjudicationSpec = serde_json::from_slice(&std::fs::read(spec_path)?)?;
    let adjudication = GameTurnAdjudication::new(
        &record,
        spec.adjudicator,
        spec.adjudicated_at_epoch_ms,
        spec.judgement,
        spec.intended_move,
        spec.intended_anchor,
        spec.intended_arguments,
        spec.intended_motif,
        spec.acceptable_clarifications,
        spec.acceptable_feedback,
        spec.note_hash,
    )?;
    let mut pipeline = CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &capture_dir)?;
    match pipeline.adjudicate_game(GameAdjudicationEvent::new(adjudication)) {
        AdjudicationRecordOutcome::Stored => Ok(()),
        outcome => anyhow::bail!("adjudication was not stored: {outcome:?}"),
    }
}

fn find_record(
    capture_dir: &Path,
    expected_hash: &str,
) -> anyhow::Result<semantic_decision_contracts::GameTurnRecord> {
    const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
    const MAX_LINES: usize = 100_000;
    for file_name in [
        "evaluation.game-turns.jsonl",
        "training.game-turns.jsonl",
        "audit.game-turns.jsonl",
    ] {
        let path = capture_dir.join(file_name);
        if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_FILE_BYTES) {
            anyhow::bail!(
                "capture file {} exceeds the 64 MiB tool limit",
                path.display()
            );
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            if index >= MAX_LINES {
                anyhow::bail!("capture file {} exceeds the line limit", path.display());
            }
            let captured: CaptureLine = serde_json::from_str(line)?;
            if captured.event.record().record_hash().as_str() == expected_hash {
                return Ok(captured.event.record().clone());
            }
        }
    }
    anyhow::bail!("no captured game turn has record hash {expected_hash}")
}
