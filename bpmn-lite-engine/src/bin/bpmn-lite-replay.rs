use bpmn_lite_kernel::replay;
use bpmn_lite_types::{ExecutableWorkflow, JournalRecord, SnapshotEnvelope};
use std::error::Error;
use std::fs;
use std::io::{Error as IoError, ErrorKind};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let usage = || {
        IoError::new(
            ErrorKind::InvalidInput,
            "usage: bpmn-lite-replay <artifact-envelope> <snapshot-envelope> <journal-array>",
        )
    };
    let artifact_path = args.next().ok_or_else(usage)?;
    let snapshot_path = args.next().ok_or_else(usage)?;
    let journal_path = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "unexpected extra replay verifier argument",
        )
        .into());
    }

    let artifact = ExecutableWorkflow::verify(&fs::read(artifact_path)?)?;
    let checkpoint = SnapshotEnvelope::decode(&fs::read(snapshot_path)?)?;
    let journal_values: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(journal_path)?)?;
    let journal = journal_values
        .into_iter()
        .map(|value| {
            let bytes = serde_json::to_vec(&value)?;
            Ok(JournalRecord::decode(&bytes)?)
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let replayed = replay(&artifact, &checkpoint, &journal)?;
    println!("{}", hex::encode(replayed.state_hash()?));
    Ok(())
}
