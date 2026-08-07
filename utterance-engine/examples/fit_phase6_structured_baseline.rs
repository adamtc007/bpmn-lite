//! Fit the interpretable Phase 6 baseline from a chartered, frozen real-turn split.
//!
//! This tool never mutates a graph and never promotes a model. It refuses synthetic,
//! unadjudicated, unfrozen or cross-partition input through the library admission APIs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use utterance_engine::funnel::{
    evaluate_frozen_game_funnel, freeze_real_turn_split, load_frozen_real_turns,
    RealTurnSplitPolicy,
};
use utterance_engine::{
    StructuredChoiceCalibration, StructuredChoiceCalibrationObservation, StructuredChoiceFitConfig,
    StructuredChoiceModel, StructuredChoiceObservation,
};

#[derive(Serialize)]
struct BaselineReceipt<'a> {
    schema: &'static str,
    split_manifest: &'a utterance_engine::funnel::RealTurnSplitManifest,
    model: &'a StructuredChoiceModel,
    calibration: &'a StructuredChoiceCalibration,
    game_funnel: &'a utterance_engine::funnel::GameFunnelReport,
    promotion_authorized: bool,
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let capture_dir = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("usage: fit_phase6_structured_baseline <capture-dir> <training-end-epoch-ms> <validation-end-epoch-ms> <output-json>"))?,
    );
    let training_end_epoch_ms = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing training cutoff"))?
        .parse::<u64>()?;
    let validation_end_epoch_ms = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing validation cutoff"))?
        .parse::<u64>()?;
    let output = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("missing output path"))?,
    );
    if args.next().is_some() {
        anyhow::bail!("unexpected trailing arguments");
    }

    let policy = RealTurnSplitPolicy::phase6(training_end_epoch_ms, validation_end_epoch_ms)?;
    let manifest = freeze_real_turn_split(&capture_dir, &policy)?;
    let joined = load_frozen_real_turns(&capture_dir, &manifest)?;
    let assignments = manifest
        .assignments()
        .iter()
        .map(|assignment| (assignment.record_hash(), assignment))
        .collect::<BTreeMap<_, _>>();
    let mut training = Vec::new();
    let mut validation = Vec::new();
    for turn in &joined {
        let assignment = assignments
            .get(turn.record().record_hash().as_str())
            .ok_or_else(|| anyhow::anyhow!("joined turn is absent from its frozen manifest"))?;
        match turn.split() {
            utterance_engine::funnel::RealTurnSplit::Training => {
                if let Some(observation) = StructuredChoiceObservation::from_training(
                    turn.record(),
                    turn.adjudication(),
                    assignment,
                )? {
                    training.push(observation);
                }
            }
            utterance_engine::funnel::RealTurnSplit::Validation => {
                if let Some(observation) = StructuredChoiceCalibrationObservation::from_validation(
                    turn.record(),
                    turn.adjudication(),
                    assignment,
                )? {
                    validation.push(observation);
                }
            }
            utterance_engine::funnel::RealTurnSplit::Test => {}
        }
    }
    let model = StructuredChoiceModel::fit(
        &training,
        StructuredChoiceFitConfig::new(500, 0.05, 0.01, 8.0)?,
    )?;
    let calibration = StructuredChoiceCalibration::fit_validation(&model, &validation)?;
    let game_funnel = evaluate_frozen_game_funnel(&joined)?;
    let receipt = BaselineReceipt {
        schema: "semantic-gameboard-phase6-structured-baseline-receipt-v1",
        split_manifest: &manifest,
        model: &model,
        calibration: &calibration,
        game_funnel: &game_funnel,
        promotion_authorized: false,
    };
    std::fs::write(output, serde_json::to_vec_pretty(&receipt)?)?;
    Ok(())
}
