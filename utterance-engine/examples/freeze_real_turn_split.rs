//! Freeze an adjudicated Phase 6 game-turn split using explicit temporal cutoffs.

use std::path::PathBuf;

use utterance_engine::funnel::{freeze_real_turn_split, RealTurnSplitPolicy};

fn main() -> anyhow::Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let capture_dir = PathBuf::from(arguments.next().ok_or_else(|| {
        anyhow::anyhow!(
            "usage: freeze_real_turn_split <capture-dir> <training-end-ms> <validation-end-ms> <output-json>"
        )
    })?);
    let training_end_epoch_ms = parse_epoch_ms(arguments.next(), "training-end-ms")?;
    let validation_end_epoch_ms = parse_epoch_ms(arguments.next(), "validation-end-ms")?;
    let output = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing output-json"))?,
    );
    if arguments.next().is_some() {
        anyhow::bail!("unexpected trailing arguments");
    }

    let policy = RealTurnSplitPolicy::phase6(training_end_epoch_ms, validation_end_epoch_ms)?;
    let manifest = freeze_real_turn_split(&capture_dir, &policy)?;
    let encoded = serde_json::to_vec_pretty(&manifest)?;
    std::fs::write(&output, encoded)?;
    println!(
        "frozen {} adjudicated turns at {}",
        manifest.assignments().len(),
        manifest.manifest_hash().as_str()
    );
    Ok(())
}

fn parse_epoch_ms(value: Option<std::ffi::OsString>, name: &str) -> anyhow::Result<u64> {
    value
        .ok_or_else(|| anyhow::anyhow!("missing {name}"))?
        .into_string()
        .map_err(|_| anyhow::anyhow!("{name} must be UTF-8"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("{name} must be an unsigned integer"))
}
